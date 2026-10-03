"""Serial loopback-only MLX server for constrained Apple-silicon evaluation.

Load, warm and evaluate on the same thread. Readiness follows successful inference,
not merely loading weights. Pass --warmup-body an exported client request to warm
the exact production rubric; the default is a generic choice/score smoke request.
The server remains an evaluation server, with one request at a time. Client
timeouts cannot cancel an in-progress Metal forward. Optional absolute deadlines
let it reject expired queued requests before doing another expensive forward.
"""
import argparse
from contextlib import contextmanager
import copy
import hashlib
import json
import math
from pathlib import Path
import socket
import sys
import tempfile
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

DEFAULT_CHECKPOINT = 'StrandsAgents/strands-decider-2B-hobson-v19'
DEFAULT_CHECKPOINT_REVISION = 'bb282d786bc251fd4e3068de3ada9ddbb38127cd'
DEFAULT_BASE_MODEL = 'Qwen/Qwen3.5-2B-Base'
DEFAULT_BASE_REVISION = 'b1485b2fa6dfa1287294f269f5fb618e03d52d7c'
MAX_BODY_BYTES = 65536
MIB = 1024 * 1024
WIRED_HEADROOM_BYTES = 512 * MIB


def positive_int(value):
    value = int(value)
    if value <= 0:
        raise argparse.ArgumentTypeError('must be positive')
    return value


def nonnegative_int(value):
    value = int(value)
    if value < 0:
        raise argparse.ArgumentTypeError('must be nonnegative')
    return value


def nonnegative_seconds(value):
    value = float(value)
    if not math.isfinite(value) or value < 0:
        raise argparse.ArgumentTypeError('must be finite and nonnegative')
    return value


def residency_plan(mx, cache_limit_bytes, override_mib=None):
    """Bound process-local Metal residency to the final model and device budget.

    Call only after evaluating the final (optionally quantized) weights and
    clearing the temporary load buffers. This never raises a system wired limit.
    """
    if override_mib is not None and (not isinstance(override_mib, int) or override_mib < 0):
        raise ValueError('residency override must be a nonnegative integer')
    info = mx.device_info()
    recommended = info.get('max_recommended_working_set_size')
    memory_size = info.get('memory_size')
    active = int(mx.get_active_memory())
    metadata = {
        'wired_limit_mode': 'auto' if override_mib is None else 'explicit',
        'wired_limit_bytes': None,
        'wired_active_memory_bytes': active,
        'wired_headroom_bytes': WIRED_HEADROOM_BYTES,
        'wired_recommended_limit_bytes': recommended,
        'memory_size_bytes': memory_size,
    }
    if override_mib == 0:
        metadata.update(wired_limit_mode='disabled', wired_limit_bytes=0)
        return metadata
    if recommended is None or memory_size is None:
        if override_mib is not None:
            raise ValueError('explicit residency requires reported Metal memory limits')
        metadata['wired_limit_mode'] = 'unavailable'
        return metadata
    maximum = min(int(recommended), int(memory_size) - MIB)
    if maximum <= 0:
        raise ValueError('device has no safe Metal residency budget')
    if override_mib is not None:
        target = override_mib * MIB
        if target > maximum:
            raise ValueError('requested residency exceeds the safe device budget')
    else:
        target = min(maximum, active + cache_limit_bytes + WIRED_HEADROOM_BYTES)
    metadata['wired_limit_bytes'] = target
    return metadata


@contextmanager
def wired_residency(mx, cache_limit_bytes, override_mib=None):
    """Apply the plan before warmup and restore the old process limit on exit."""
    metadata = residency_plan(mx, cache_limit_bytes, override_mib)
    limit = metadata['wired_limit_bytes']
    previous = None
    if limit is not None:
        # The installed mlx-lm generation context also synchronizes before changing
        # residency: never resize sets while an async Metal evaluation is running.
        mx.synchronize()
        previous = mx.set_wired_limit(limit)
    try:
        yield metadata
    finally:
        if previous is not None:
            mx.synchronize()
            mx.set_wired_limit(previous)


def snapshot_revision(path):
    parts = Path(path).parts
    if 'snapshots' in parts:
        index = parts.index('snapshots') + 1
        if index < len(parts):
            return parts[index]
    return None


def resolve_checkpoint(checkpoint, revision=None):
    """Resolve pinned Hub weights without changing a local checkpoint in place."""
    path = Path(checkpoint).expanduser()
    if path.is_dir():
        return path.absolute()
    from huggingface_hub import snapshot_download
    if revision is None and checkpoint == DEFAULT_CHECKPOINT:
        revision = DEFAULT_CHECKPOINT_REVISION
    return Path(snapshot_download(checkpoint, revision=revision))


def pinned_overlay(checkpoint, base_revision, directory):
    """Override only the base path in a disposable checkpoint, leaving artifacts intact.

    Upstream records the base revision in provenance but does not read it when
    loading. Resolve that revision first, then let its unchanged MLX loader use
    the local snapshot instead of following Hub main.
    """
    config_file = checkpoint / 'strands_decider_config.json'
    if not config_file.exists():
        config_file = checkpoint / 'hobson_config.json'
    config = json.loads(config_file.read_text())
    base_name = config['base_model']
    provenance_file = checkpoint / 'provenance.json'
    provenance = json.loads(provenance_file.read_text()) if provenance_file.exists() else {}
    revision = base_revision or provenance.get('base_model_revision')
    if revision is None and base_name == DEFAULT_BASE_MODEL:
        revision = DEFAULT_BASE_REVISION
    base = resolve_checkpoint(base_name, revision)
    overlay = Path(directory)
    for source in checkpoint.iterdir():
        if source.name == config_file.name:
            continue
        (overlay / source.name).symlink_to(source, target_is_directory=source.is_dir())
    config['base_model'] = str(base)
    (overlay / config_file.name).write_text(json.dumps(config))
    head_file = next((checkpoint / name for name in ('head.safetensors', 'slot_head.pt')
                      if (checkpoint / name).exists()), None)
    return {
        'checkpoint': str(checkpoint),
        'checkpoint_revision': snapshot_revision(checkpoint),
        'base_model': base_name,
        'base_model_path': str(base),
        'base_model_revision': snapshot_revision(base),
        'head_sha256': hashlib.sha256(head_file.read_bytes()).hexdigest() if head_file else None,
    }


def default_warmup_body():
    # A generic upstream-schema smoke request, not a second copy of the P2 rubric.
    return {
        'state': 'Write a Python function that removes duplicates while preserving order.',
        'questions': {
            'request_type': {
                'type': 'choice',
                'instructions': 'What kind of help is being requested?',
                'criteria': {'code': 'write or explain source code',
                             'prose': 'draft or edit prose', 'fact': 'answer a factual question'},
            },
            'complexity': {
                'type': 'score', 'instructions': 'How demanding is the task?',
                'criteria': ['trivial', 'simple', 'moderate', 'complex', 'expert'],
            },
        },
    }


def warmup_profiles(body):
    """Warm short and bounded long states with the client's exact question schema."""
    short = copy.deepcopy(body)
    short['state'] = ('Latest request:\nWrite a Python function that removes duplicates '
                      'while preserving order.')
    long = copy.deepcopy(body)
    long['state'] = short['state'] + '\n\nEarlier conversation (newest first):\n' + (
        '[user] Process a list of identifiers, preserve their order, handle missing values, '
        'and return clear errors for invalid inputs.\n'
        '[assistant] Use a set for membership checks and append first occurrences to a list.\n'
    ) * 6
    profiles = []
    questions = short['questions']
    if len(questions) > 1:
        name = next((name for name, question in questions.items()
                     if question.get('type') == 'choice'), next(iter(questions)))
        type_only = copy.deepcopy(short)
        type_only['questions'] = {name: type_only['questions'][name]}
        profiles.append(('type_only', type_only))
    profiles.extend([('configured_short', short), ('configured_long', long)])
    return profiles


def run_warmup(engine, request_class, body):
    rows = []
    for name, payload in warmup_profiles(body):
        started = time.perf_counter()
        response = engine.evaluate(request_class.model_validate(payload)).model_dump()
        rows.append({'profile': name, 'latency_ms': (time.perf_counter() - started) * 1000,
                     'questions': len(payload['questions']),
                     'input_tokens': response.get('usage', {}).get('input_tokens')})
    return rows


class InferenceActivity:
    """Optional idle inference on HTTPServer's serial service-actions thread.

    Maintenance performs an actual short forward with the client's rubric. It
    has no response cache and never runs concurrently with a request handler.
    """
    def __init__(self, engine, request_class, body, idle_seconds=0):
        self.engine = engine
        self.request_class = request_class
        self.body = next(payload for name, payload in warmup_profiles(body)
                         if name == 'configured_short')
        self.idle_seconds = idle_seconds
        self.last_attempt = time.monotonic()
        self.ready = True
        self.idle_count = 0
        self.idle_failures = 0
        self.last_latency_ms = None

    def touch(self):
        self.last_attempt = time.monotonic()

    def service_actions(self):
        if self.idle_seconds <= 0 or time.monotonic() - self.last_attempt < self.idle_seconds:
            return
        self.touch()
        started = time.perf_counter()
        try:
            self.engine.evaluate(self.request_class.model_validate(self.body))
            self.ready = True
        except Exception as exc:
            self.ready = False
            self.idle_failures += 1
            print(f'idle inference failed ({type(exc).__name__})', file=sys.stderr, flush=True)
        finally:
            self.idle_count += 1
            self.last_latency_ms = (time.perf_counter() - started) * 1000
            self.touch()

    def metadata(self):
        return {
            'ready': self.ready,
            'idle_warmup_seconds': self.idle_seconds,
            'idle_warmup_count': self.idle_count,
            'idle_warmup_failures': self.idle_failures,
            'idle_warmup_last_latency_ms': self.last_latency_ms,
            'inference_last_attempt_monotonic': self.last_attempt,
        }


def request_deadline(headers):
    """Optional client epoch deadline, shared across queueing and inference."""
    value = headers.get('X-Request-Deadline-Unix-Ms')
    if value is None:
        return None
    value = float(value)
    if not math.isfinite(value) or value <= 0:
        raise ValueError('invalid deadline')
    return value / 1000


def make_handler(engine, request_class, metadata, activity=None):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            # BaseHTTPRequestHandler can include a malformed request line in errors.
            print('HTTP parser event', file=sys.stderr, flush=True)

        def log_request(self, code='-', size='-'):
            # Never log URLs, request bodies, or validation errors containing input.
            print(f'HTTP status={code}', file=sys.stderr, flush=True)

        def reply(self, status, payload):
            body = json.dumps(payload).encode()
            try:
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError, socket.timeout):
                pass  # The client deadline expired while inference was running.

        def do_GET(self):
            if self.path not in ('/health', '/ready'):
                return self.reply(404, {'error': 'not found'})
            ready = activity.ready if activity is not None else True
            status = 200 if ready or self.path == '/health' else 503
            health = {'status': 'ok' if ready else 'not_ready', 'ready': ready, **metadata}
            if activity is not None:
                health.update(activity.metadata())
            self.reply(status, health)

        def do_POST(self):
            if self.path != '/v1/systemone':
                return self.reply(404, {'error': 'not found'})
            if activity is not None:
                activity.touch()
            try:
                if self.headers.get('Transfer-Encoding'):
                    return self.reply(400, {'error': 'chunked bodies are unsupported'})
                size = int(self.headers.get('Content-Length', '0'))
                if not 0 < size <= MAX_BODY_BYTES:
                    return self.reply(413, {'error': f'body must be 1..{MAX_BODY_BYTES} bytes'})
                deadline = request_deadline(self.headers)
                if deadline is not None and time.time() >= deadline:
                    return self.reply(408, {'error': 'request deadline expired'})
                self.connection.settimeout(10)
                body = self.rfile.read(size)
                if len(body) != size:
                    return self.reply(400, {'error': 'incomplete body'})
                request = request_class.model_validate_json(body)
                if deadline is not None and time.time() >= deadline:
                    return self.reply(408, {'error': 'request deadline expired'})
                started = time.perf_counter()
                response = engine.evaluate(request).model_dump()
                if activity is not None:
                    activity.ready = True
                response['latency_ms'] = (time.perf_counter() - started) * 1000
                if deadline is not None and time.time() >= deadline:
                    return self.reply(408, {'error': 'request deadline expired'})
                self.reply(200, response)
            except (ValueError, TypeError):
                self.reply(422, {'error': 'invalid request or prompt exceeds model window'})
            except socket.timeout:
                self.reply(408, {'error': 'request body read timed out'})
            except Exception as exc:
                if activity is not None:
                    activity.ready = False
                print(f'inference failed ({type(exc).__name__})', file=sys.stderr, flush=True)
                self.reply(500, {'error': 'inference failed'})
            finally:
                if activity is not None:
                    activity.touch()
    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkpoint', default=DEFAULT_CHECKPOINT)
    parser.add_argument('--revision', help='Hub checkpoint revision; default v19 is pinned')
    parser.add_argument('--base-revision', help='Override the base revision recorded by the checkpoint')
    parser.add_argument('--port', type=positive_int, default=18099)
    parser.add_argument('--quantize', type=int, choices=[4, 8], help='Post-LoRA MLX quantization; a distinct model variant')
    parser.add_argument('--max-batch', type=positive_int, default=1)
    parser.add_argument('--prefix-cache', action='store_true', help='Share state across question batches larger than one')
    parser.add_argument('--cache-limit-mib', type=positive_int, default=64)
    parser.add_argument('--wired-limit-mib', type=nonnegative_int,
                        help='Process Metal residency budget; default bounds final-model bytes plus cache and 512 MiB, 0 disables')
    parser.add_argument('--idle-warmup-seconds', type=nonnegative_seconds, default=0,
                        help='Opt-in actual inference after this idle interval; default 0 disables maintenance')
    parser.add_argument('--warmup-body', type=Path, help='JSON client request supplying the exact inference rubric')
    args = parser.parse_args()
    if args.port > 65535:
        parser.error('--port must be in 1..65535')
    started = time.perf_counter()
    body = default_warmup_body()
    if args.warmup_body:
        if args.warmup_body.stat().st_size > MAX_BODY_BYTES:
            parser.error('--warmup-body exceeds the request size limit')
        body = json.loads(args.warmup_body.read_text())
    import torch
    from strands_decider.infer import EngineConfig
    from strands_decider.mlx_engine import load_mlx_engine
    from strands_decider.schema import SystemOneRequest

    SystemOneRequest.model_validate(body)
    torch.set_num_threads(2)
    checkpoint = resolve_checkpoint(args.checkpoint, args.revision)
    config = EngineConfig(device='mlx', use_prefix_cache=args.prefix_cache, strict_window=True,
                          max_batch=args.max_batch, model_name=args.checkpoint.rstrip('/').rsplit('/', 1)[-1])
    cache_limit = args.cache_limit_mib * 1024 * 1024
    with tempfile.TemporaryDirectory(prefix='nasiko-strands-pinned-') as directory:
        artifacts = pinned_overlay(checkpoint, args.base_revision, directory)
        engine = load_mlx_engine(directory, config, cache_limit_bytes=cache_limit)
        import mlx.core as mx
        if args.quantize:
            import mlx.nn as nn
            # The pinned upstream loader has already merged LoRA into this decoder.
            nn.quantize(engine._decoder, bits=args.quantize, group_size=64)
            engine.cfg.model_name += f'-q{args.quantize}'
        # Wire the final weights only. FP16 loading/LoRA transients must not inflate
        # the residency budget or occupy it ahead of the quantized model.
        mx.eval(engine._decoder.parameters())
        mx.clear_cache()
        load_ms = (time.perf_counter() - started) * 1000
        with wired_residency(mx, cache_limit, args.wired_limit_mib) as residency:
            warmup_started = time.perf_counter()
            warmup = run_warmup(engine, SystemOneRequest, body)
            warmup_ms = (time.perf_counter() - warmup_started) * 1000
            metadata = {
                **artifacts, **residency, 'model': engine.cfg.model_name, 'device': 'mlx',
                'prefix_cache': engine.cfg.use_prefix_cache, 'max_batch': engine.cfg.max_batch,
                'serial': True, 'quantization_bits': args.quantize,
                'cache_limit_bytes': cache_limit, 'max_length': engine.model.config.max_length,
                'load_ms': load_ms, 'warmup_ms': warmup_ms,
                'ready_ms': (time.perf_counter() - started) * 1000,
                'warmup': warmup, 'warmup_rubric': 'client' if args.warmup_body else 'generic_smoke',
                'deadline_header': 'X-Request-Deadline-Unix-Ms',
            }
            activity = InferenceActivity(engine, SystemOneRequest, body, args.idle_warmup_seconds)
            with HTTPServer(('127.0.0.1', args.port), make_handler(engine, SystemOneRequest, metadata, activity)) as server:
                # serve_forever calls this hook on the same thread, after handlers.
                server.service_actions = activity.service_actions
                print(f'Ready at http://127.0.0.1:{args.port} ' + json.dumps({**metadata, **activity.metadata()}), flush=True)
                server.serve_forever()


if __name__ == '__main__':
    main()
