"""Run frozen P2 annotations against the Rust factory without replacing past runs.

The default backend URLs and --output-name behavior are retained. Use --out for a
new result directory and --strands-url for an optimized evaluation server. Existing
prediction/log/manifest files are never overwritten. Imports require stdlib only.
"""
import argparse
import hashlib
import json
import os
import platform
from pathlib import Path
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parent
BACKENDS = ('regex', 'laya', 'strands')
ALLOWED_BACKENDS = ('regex', 'local', 'laya', 'strands', 'strands_raw', 'hybrid')
SOURCE_FILES = (
    'llm-router/examples/classifier_eval.rs',
    'llm-router/src/lib.rs',
    'llm-router/src/config.rs',
    'llm-router/src/routing/mod.rs',
    'llm-router/src/handlers/chat.rs',
    'llm-router/src/routing/request_classifier.rs',
    'llm-router/src/routing/classifier.rs',
    'llm-router/src/routing/patterns.rs',
    'llm-router/src/routing/laya.rs',
    'llm-router/src/routing/strands.rs',
    'llm-router/src/routing/hybrid.rs',
    'llm-router/src/routing/local_classifier.rs',
    'llm-router/eval/strands/serve_local.py',
    'llm-router/eval/strands/calibrate.py',
    'llm-router/eval/strands/runtime-environment.json',
    'llm-router/eval/strands/calibration/native-warmup-body.json',
    'llm-router/eval/strands/calibration/warmup-body.json',
    'llm-router/assets/strands_calibration.json',
    'llm-router/assets/local_classifier.json',
)
CALIBRATION_SOURCES = (
    'llm-router/eval/strands/calibration/selection-manifest.json',
    'llm-router/eval/strands/calibration/inference-manifest.json',
    'llm-router/eval/strands/calibration/cv-report.json',
)


def positive_int(value):
    value = int(value)
    if value <= 0:
        raise argparse.ArgumentTypeError('must be positive')
    return value


def digest(path):
    result = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            result.update(chunk)
    return result.hexdigest()


def get_health(url):
    with urllib.request.urlopen(url.rstrip('/') + '/health', timeout=5) as response:
        return json.load(response)


def calibration_metadata(source, artifact):
    paths = [artifact, *(source / relative for relative in CALIBRATION_SOURCES)]
    rows = []
    for path in paths:
        if path.is_file():
            rows.append({'path': str(path.resolve()), 'sha256': digest(path),
                         'metadata': json.loads(path.read_text())})
    return {'artifact_path': str(artifact), 'artifact_present': artifact.is_file(),
            'sources': rows}


def ensure_fresh_output(output):
    """Protect earlier predictions and their provenance, including frozen pilots."""
    if output.exists():
        if not output.is_dir():
            raise ValueError(f'output is not a directory: {output}')
        protected = [path for path in output.iterdir()
                     if path.name == 'run-manifest.json'
                     or path.name == 'source-snapshot'
                     or path.name in {'report.md', 'summary.json'}
                     or path.suffix in {'.jsonl', '.log'}]
        if protected:
            raise ValueError(f'output already contains a run; choose a fresh --out directory: {output}')


def save_manifest(output, manifest):
    temporary = output / '.run-manifest.json.tmp'
    temporary.write_text(json.dumps(manifest, indent=2) + '\n')
    temporary.replace(output / 'run-manifest.json')


def snapshot_sources(source, output):
    files = {}
    for relative in SOURCE_FILES:
        original = source / relative
        if not original.is_file():
            continue
        destination = output / 'source-snapshot' / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(original.read_bytes())
        files[relative] = digest(destination)
    return files


def validate_predictions(path, expected_ids):
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    ids = [row['id'] for row in rows]
    if len(ids) != len(expected_ids) or set(ids) != set(expected_ids):
        raise ValueError(f'incomplete or duplicated predictions: {path.name}')
    return len(rows)


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--source-root', type=Path, required=True)
    parser.add_argument('--repeats', type=positive_int, default=2)
    parser.add_argument('--timeout-ms', type=positive_int, default=1500)
    parser.add_argument('--backends', nargs='+', choices=ALLOWED_BACKENDS,
                        default=list(BACKENDS), help='Backends in execution order; default: regex laya strands')
    destination = parser.add_mutually_exclusive_group()
    destination.add_argument('--output-name', default='results',
                             help='Result directory relative to the frozen dataset')
    destination.add_argument('--out', type=Path, help='New output directory (absolute or relative to cwd)')
    parser.add_argument('--laya-url', default='http://127.0.0.1:18098')
    parser.add_argument('--strands-url', default='http://127.0.0.1:18099')
    parser.add_argument('--calibration-artifact', type=Path,
                        help='Calibration JSON; default is source-root/llm-router/assets/strands_calibration.json')
    args = parser.parse_args(argv)
    if len(args.backends) != len(set(args.backends)):
        parser.error('--backends must not contain duplicates')
    return args


def main(argv=None):
    args = parse_args(argv)
    output = (args.out if args.out is not None else ROOT / args.output_name).expanduser().resolve()
    source = args.source_root.expanduser().resolve()
    binary = args.binary.expanduser().resolve()
    artifact = (args.calibration_artifact if args.calibration_artifact is not None
                else source / 'llm-router/assets/strands_calibration.json').expanduser().resolve()
    ensure_fresh_output(output)
    data = ROOT / 'sample-labelled.json'
    expected = (ROOT / 'labels-frozen.sha256').read_text().split()[0]
    if digest(data) != expected:
        raise ValueError('frozen labels changed; refusing evaluation')
    expected_ids = [row['id'] for row in json.loads(data.read_text())['examples']]
    if len(expected_ids) != len(set(expected_ids)):
        raise ValueError('frozen dataset contains duplicated IDs')
    binary_hash = digest(binary)
    urls = {'laya': args.laya_url.rstrip('/'), 'strands': args.strands_url.rstrip('/')}
    endpoint_kinds = {'laya' if backend == 'laya' else 'strands'
                      for backend in args.backends if backend not in {'regex', 'local'}}
    health = {kind: get_health(urls[kind]) for kind in sorted(endpoint_kinds)}
    backend_urls = {backend: urls['laya' if backend == 'laya' else 'strands']
                    for backend in args.backends if backend not in {'regex', 'local'}}
    manifest = {
        'schema_version': 2,
        'status': 'running',
        'labels_path': str(data), 'labels_sha256': expected,
        'binary_path': str(binary), 'binary_sha256': binary_hash,
        'source_root': str(source),
        'commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True).strip(),
        'tracked_diff_sha256': hashlib.sha256(
            subprocess.check_output(['git', 'diff', 'HEAD'], cwd=source)).hexdigest(),
        'hardware': platform.platform(),
        'timeouts_ms': {backend: args.timeout_ms for backend in backend_urls},
        'urls': urls, 'backend_urls': backend_urls, 'health': health,
        'calibration': calibration_metadata(source, artifact),
        'repeats': args.repeats, 'order': list(args.backends),
        'requested_backends': list(args.backends),
        'source_snapshot': str(output / 'source-snapshot'),
        'runs': [],
    }
    # Claim a new directory, or reuse an existing empty directory without replacing
    # any generated artifact. A second runner is rejected by exclusive manifest open.
    output.mkdir(parents=True, exist_ok=True)
    with (output / 'run-manifest.json').open('x') as file:
        file.write(json.dumps(manifest, indent=2) + '\n')
    try:
        manifest['source_files_sha256'] = snapshot_sources(source, output)
        save_manifest(output, manifest)
        for repeat in range(1, args.repeats + 1):
            for backend in args.backends:
                if digest(binary) != binary_hash or digest(data) != expected:
                    raise ValueError('binary or frozen labels changed during evaluation')
                name = f'{backend}-run{repeat}'
                predictions = output / f'{name}.jsonl'
                log_path = output / f'{name}.log'
                env = os.environ.copy()
                env.update(REQUEST_CLASSIFIER=backend,
                           LAYA_URL=urls['laya'], STRANDS_URL=urls['strands'],
                           LAYA_TIMEOUT_MS=str(args.timeout_ms), STRANDS_TIMEOUT_MS=str(args.timeout_ms),
                           EVAL_SET=str(data), OUT=str(predictions))
                started = time.perf_counter()
                with log_path.open('x') as log:
                    subprocess.run([str(binary)], cwd=source, env=env,
                                   stdout=log, stderr=log, check=True)
                if digest(binary) != binary_hash or digest(data) != expected:
                    raise ValueError('binary or frozen labels changed during evaluation')
                row = {'backend': backend, 'repeat': repeat,
                       'elapsed_s': time.perf_counter() - started,
                       'predictions': validate_predictions(predictions, expected_ids),
                       'predictions_sha256': digest(predictions), 'log_sha256': digest(log_path)}
                manifest['runs'].append(row)
                save_manifest(output, manifest)
                print(json.dumps(row), flush=True)
        manifest['status'] = 'complete'
        save_manifest(output, manifest)
    except Exception as error:
        manifest['status'] = 'failed'
        manifest['failure_type'] = type(error).__name__
        save_manifest(output, manifest)
        raise


if __name__ == '__main__':
    main()
