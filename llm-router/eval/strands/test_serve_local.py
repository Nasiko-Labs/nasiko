"""Run with python -B test_serve_local.py; no model, MLX, or sockets are needed."""
import copy
from contextlib import nullcontext
import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import Mock, patch

import serve_local


class FakeRequest:
    @classmethod
    def model_validate(cls, payload):
        if not isinstance(payload, dict) or not isinstance(payload.get('questions'), dict):
            raise ValueError('PRIVATE input in schema error')
        return payload

    @classmethod
    def model_validate_json(cls, body):
        return cls.model_validate(json.loads(body))


class FakeResponse:
    def model_dump(self):
        return {'answers': {}, 'usage': {'input_tokens': 42}}


class FakeEngine:
    def __init__(self):
        self.calls = []
        self.error = None
        self.model = types.SimpleNamespace(config=types.SimpleNamespace(max_length=4096))
        self._decoder = types.SimpleNamespace(parameters=lambda: ())

    def evaluate(self, request):
        if self.error:
            raise self.error
        self.calls.append(request)
        return FakeResponse()


class HandlerTests(unittest.TestCase):
    def setUp(self):
        self.engine = FakeEngine()
        self.handler_class = serve_local.make_handler(self.engine, FakeRequest, {'model': 'test'})

    def handler(self, payload=None, headers=None):
        # Exercise handlers without opening a port or invoking the HTTP constructor.
        handler = object.__new__(self.handler_class)
        handler.path = '/v1/systemone'
        body = json.dumps(payload or serve_local.default_warmup_body()).encode()
        handler.headers = {'Content-Length': str(len(body)), **(headers or {})}
        handler.rfile = io.BytesIO(body)
        handler.connection = types.SimpleNamespace(settimeout=lambda value: None)
        handler.responses = []
        handler.reply = lambda code, payload: handler.responses.append((code, payload))
        return handler

    def test_expired_queued_deadline_skips_engine(self):
        handler = self.handler(headers={'X-Request-Deadline-Unix-Ms': '9000'})
        with patch.object(serve_local.time, 'time', return_value=10):
            handler.do_POST()
        self.assertEqual(handler.responses[0][0], 408)
        self.assertEqual(self.engine.calls, [])

    def test_deadline_expiring_during_body_read_skips_engine(self):
        handler = self.handler(headers={'X-Request-Deadline-Unix-Ms': '10000'})
        with patch.object(serve_local.time, 'time', side_effect=[9, 11]):
            handler.do_POST()
        self.assertEqual(handler.responses[0][0], 408)
        self.assertEqual(self.engine.calls, [])

    def test_inference_after_deadline_returns_timeout(self):
        handler = self.handler(headers={'X-Request-Deadline-Unix-Ms': '10000'})
        with patch.object(serve_local.time, 'time', side_effect=[9, 9, 11]):
            handler.do_POST()
        self.assertEqual(handler.responses[0][0], 408)
        self.assertEqual(len(self.engine.calls), 1)

    def test_invalid_deadline_rejected_without_echoing_input(self):
        for value in ('nan', 'inf', '-1', 'PRIVATE input'):
            with self.subTest(value=value):
                handler = self.handler(headers={'X-Request-Deadline-Unix-Ms': value})
                handler.do_POST()
                self.assertEqual(handler.responses[0][0], 422)
                self.assertNotIn(value, str(handler.responses))
        self.assertEqual(self.engine.calls, [])

    def test_inference_success_and_ready_metadata(self):
        handler = self.handler()
        handler.do_POST()
        self.assertEqual(handler.responses[0][0], 200)
        self.assertIn('latency_ms', handler.responses[0][1])
        for endpoint in ('/ready', '/health'):
            handler.path = endpoint
            handler.do_GET()
            self.assertEqual(handler.responses[-1],
                             (200, {'status': 'ok', 'ready': True, 'model': 'test'}))

    def test_body_limits_and_unsupported_transfer_encoding_skip_engine(self):
        cases = [({'Content-Length': str(serve_local.MAX_BODY_BYTES + 1)}, 413),
                 ({'Content-Length': '0'}, 413),
                 ({'Content-Length': 'invalid'}, 422),
                 ({'Transfer-Encoding': 'chunked'}, 400)]
        for headers, expected_status in cases:
            with self.subTest(headers=headers):
                handler = self.handler(headers=headers)
                handler.do_POST()
                self.assertEqual(handler.responses[0][0], expected_status)
        self.assertEqual(self.engine.calls, [])

    def test_incomplete_body_skips_engine(self):
        handler = self.handler()
        handler.headers['Content-Length'] = str(int(handler.headers['Content-Length']) + 1)
        handler.do_POST()
        self.assertEqual(handler.responses[0][0], 400)
        self.assertEqual(self.engine.calls, [])

    def test_inference_error_does_not_log_or_return_content(self):
        self.engine.error = RuntimeError('PRIVATE request text')
        handler = self.handler()
        captured = io.StringIO()
        with patch.object(serve_local.sys, 'stderr', captured):
            handler.do_POST()
        self.assertEqual(handler.responses[0][0], 500)
        self.assertNotIn('PRIVATE', captured.getvalue() + str(handler.responses))

    def test_schema_error_does_not_return_content(self):
        handler = self.handler()
        body = b'{"state":"PRIVATE input"}'
        handler.rfile = io.BytesIO(body)
        handler.headers['Content-Length'] = str(len(body))
        handler.do_POST()
        self.assertEqual(handler.responses[0][0], 422)
        self.assertNotIn('PRIVATE', str(handler.responses))

    def test_parser_errors_do_not_log_request_line(self):
        handler = self.handler()
        captured = io.StringIO()
        with patch.object(serve_local.sys, 'stderr', captured):
            handler.log_error('malformed line %s', 'PRIVATE content')
        self.assertNotIn('PRIVATE', captured.getvalue())


class WarmupTests(unittest.TestCase):
    def setUp(self):
        self.engine = FakeEngine()

    def test_warmup_preserves_client_body_and_exact_rubric(self):
        body = serve_local.default_warmup_body()
        original = copy.deepcopy(body)
        profiles = serve_local.run_warmup(self.engine, FakeRequest, body)
        self.assertEqual(body, original)
        self.assertEqual([row['questions'] for row in profiles], [1, 2, 2])
        self.assertEqual(self.engine.calls[-1]['questions'], body['questions'])
        self.assertLess(len(self.engine.calls[-1]['state']), 2000)
        self.assertGreater(len(self.engine.calls[-1]['state']),
                           len(self.engine.calls[-2]['state']))
        self.assertEqual([row['input_tokens'] for row in profiles], [42, 42, 42])

    def test_warmup_failure_aborts(self):
        self.engine.error = RuntimeError('failed')
        with self.assertRaises(RuntimeError):
            serve_local.run_warmup(self.engine, FakeRequest, serve_local.default_warmup_body())

    def test_type_only_client_keeps_type_only_warmup(self):
        body = serve_local.default_warmup_body()
        body['questions'].pop('complexity')
        rows = serve_local.run_warmup(self.engine, FakeRequest, body)
        self.assertEqual([row['questions'] for row in rows], [1, 1])
        self.assertEqual(self.engine.calls[-1]['questions'], body['questions'])

    def test_failed_warmup_does_not_bind_or_announce_ready(self):
        # Model-related imports are mocked, so main's startup ordering is tested
        # without depending on torch, strands_decider, or available hardware.
        modules = {
            'torch': types.ModuleType('torch'),
            'strands_decider': types.ModuleType('strands_decider'),
            'strands_decider.infer': types.ModuleType('strands_decider.infer'),
            'strands_decider.mlx_engine': types.ModuleType('strands_decider.mlx_engine'),
            'strands_decider.schema': types.ModuleType('strands_decider.schema'),
            'mlx': types.ModuleType('mlx'),
            'mlx.core': types.ModuleType('mlx.core'),
        }
        modules['torch'].set_num_threads = Mock()
        modules['strands_decider.infer'].EngineConfig = lambda **kwargs: types.SimpleNamespace(**kwargs)
        modules['strands_decider.mlx_engine'].load_mlx_engine = Mock(return_value=self.engine)
        modules['strands_decider.schema'].SystemOneRequest = FakeRequest
        modules['mlx'].core = modules['mlx.core']
        modules['mlx.core'].eval = Mock()
        modules['mlx.core'].clear_cache = Mock()
        captured = io.StringIO()
        with patch.dict(sys.modules, modules), \
             patch.object(sys, 'argv', ['serve_local.py']), \
             patch.object(serve_local, 'resolve_checkpoint', return_value=Path('/mock')), \
             patch.object(serve_local, 'pinned_overlay', return_value={}), \
             patch.object(serve_local, 'wired_residency', return_value=nullcontext({})), \
             patch.object(serve_local, 'run_warmup', side_effect=RuntimeError('failed')), \
             patch.object(serve_local, 'HTTPServer') as server, \
             patch.object(serve_local.sys, 'stdout', captured):
            with self.assertRaises(RuntimeError):
                serve_local.main()
            server.assert_not_called()
        self.assertNotIn('Ready at', captured.getvalue())


class ResidencyTests(unittest.TestCase):
    def mlx(self, active_mib=1000, recommended_mib=4096, memory_mib=8192, previous=0):
        return types.SimpleNamespace(
            device_info=Mock(return_value={
                'max_recommended_working_set_size': recommended_mib * serve_local.MIB,
                'memory_size': memory_mib * serve_local.MIB,
            }),
            get_active_memory=Mock(return_value=active_mib * serve_local.MIB),
            synchronize=Mock(),
            set_wired_limit=Mock(return_value=previous),
        )

    def test_auto_budget_uses_final_active_memory_cache_and_bounded_headroom(self):
        mx = self.mlx()
        with serve_local.wired_residency(mx, 128 * serve_local.MIB) as metadata:
            self.assertEqual(metadata['wired_limit_bytes'], 1640 * serve_local.MIB)
            self.assertEqual(metadata['wired_active_memory_bytes'], 1000 * serve_local.MIB)
            self.assertEqual(metadata['wired_limit_mode'], 'auto')
            mx.set_wired_limit.assert_called_once_with(1640 * serve_local.MIB)
        self.assertEqual([call.args[0] for call in mx.set_wired_limit.call_args_list],
                         [1640 * serve_local.MIB, 0])
        self.assertEqual(mx.synchronize.call_count, 2)

    def test_auto_budget_obeys_device_recommendation(self):
        mx = self.mlx(active_mib=3500, recommended_mib=4096)
        plan = serve_local.residency_plan(mx, 128 * serve_local.MIB)
        self.assertEqual(plan['wired_limit_bytes'], 4096 * serve_local.MIB)

    def test_auto_budget_remains_strictly_below_physical_memory(self):
        mx = self.mlx(active_mib=3500, recommended_mib=5000, memory_mib=4096)
        plan = serve_local.residency_plan(mx, 128 * serve_local.MIB)
        self.assertEqual(plan['wired_limit_bytes'], 4095 * serve_local.MIB)

    def test_explicit_budget_is_process_local_and_restored_on_failure(self):
        mx = self.mlx(previous=64 * serve_local.MIB)
        with self.assertRaises(RuntimeError):
            with serve_local.wired_residency(mx, 128 * serve_local.MIB, 2048) as metadata:
                self.assertEqual(metadata['wired_limit_mode'], 'explicit')
                raise RuntimeError('warmup failed')
        self.assertEqual([call.args[0] for call in mx.set_wired_limit.call_args_list],
                         [2048 * serve_local.MIB, 64 * serve_local.MIB])

    def test_unsafe_explicit_budget_rejected_before_mutating_mlx(self):
        for override in (4097, 8192, -1):
            with self.subTest(override=override):
                mx = self.mlx()
                with self.assertRaises(ValueError):
                    with serve_local.wired_residency(mx, 128 * serve_local.MIB, override):
                        self.fail('unsafe budget was accepted')
                mx.set_wired_limit.assert_not_called()

    def test_zero_disables_residency_and_restores_previous_limit(self):
        mx = self.mlx(previous=32 * serve_local.MIB)
        with serve_local.wired_residency(mx, 128 * serve_local.MIB, 0) as metadata:
            self.assertEqual(metadata['wired_limit_mode'], 'disabled')
            self.assertEqual(metadata['wired_limit_bytes'], 0)
        self.assertEqual([call.args[0] for call in mx.set_wired_limit.call_args_list],
                         [0, 32 * serve_local.MIB])

    def test_missing_device_metadata_skips_auto_but_rejects_explicit_budget(self):
        mx = self.mlx()
        mx.device_info.return_value = {}
        with serve_local.wired_residency(mx, 128 * serve_local.MIB) as metadata:
            self.assertEqual(metadata['wired_limit_mode'], 'unavailable')
            self.assertIsNone(metadata['wired_limit_bytes'])
        mx.set_wired_limit.assert_not_called()
        with self.assertRaises(ValueError):
            serve_local.residency_plan(mx, 128 * serve_local.MIB, 2048)


class IdleWarmupTests(unittest.TestCase):
    def setUp(self):
        self.engine = FakeEngine()
        self.body = serve_local.default_warmup_body()

    def handler(self, activity, endpoint):
        handler = object.__new__(serve_local.make_handler(
            self.engine, FakeRequest, {'model': 'test'}, activity))
        handler.path = endpoint
        body = json.dumps(self.body).encode()
        handler.headers = {'Content-Length': str(len(body))}
        handler.rfile = io.BytesIO(body)
        handler.connection = types.SimpleNamespace(settimeout=lambda value: None)
        handler.responses = []
        handler.reply = lambda code, payload: handler.responses.append((code, payload))
        return handler

    def test_idle_inference_is_disabled_by_default(self):
        activity = serve_local.InferenceActivity(self.engine, FakeRequest, self.body)
        with patch.object(serve_local.time, 'monotonic', return_value=1000000):
            activity.service_actions()
        self.assertEqual(self.engine.calls, [])
        self.assertEqual(activity.idle_count, 0)

    def test_idle_cadence_uses_real_inference_and_preserves_client_rubric(self):
        with patch.object(serve_local.time, 'monotonic', return_value=0) as clock:
            activity = serve_local.InferenceActivity(self.engine, FakeRequest, self.body, 5)
            clock.return_value = 4.99
            activity.service_actions()
            self.assertEqual(self.engine.calls, [])
            clock.return_value = 5
            activity.service_actions()
            self.assertEqual(len(self.engine.calls), 1)
            self.assertEqual(self.engine.calls[0]['questions'], self.body['questions'])
            self.assertEqual(activity.idle_count, 1)
            clock.return_value = 9.99
            activity.service_actions()
            self.assertEqual(len(self.engine.calls), 1)
            clock.return_value = 10
            activity.service_actions()
            self.assertEqual(len(self.engine.calls), 2)

    def test_health_reads_do_not_reset_inference_idle_time(self):
        with patch.object(serve_local.time, 'monotonic', return_value=0) as clock:
            activity = serve_local.InferenceActivity(self.engine, FakeRequest, self.body, 5)
            clock.return_value = 5
            handler = self.handler(activity, '/health')
            handler.do_GET()
            self.assertEqual(activity.last_attempt, 0)
            activity.service_actions()
            self.assertEqual(activity.idle_count, 1)

    def test_active_requests_and_expired_deadlines_postpone_idle_inference(self):
        with patch.object(serve_local.time, 'monotonic', return_value=0) as clock:
            activity = serve_local.InferenceActivity(self.engine, FakeRequest, self.body, 5)
            clock.return_value = 4
            handler = self.handler(activity, '/v1/systemone')
            handler.do_POST()
            self.assertEqual(len(self.engine.calls), 1)
            clock.return_value = 8
            activity.service_actions()
            self.assertEqual(activity.idle_count, 0)
            handler = self.handler(activity, '/v1/systemone')
            handler.headers['X-Request-Deadline-Unix-Ms'] = '9000'
            with patch.object(serve_local.time, 'time', return_value=10):
                handler.do_POST()
            self.assertEqual(handler.responses[0][0], 408)
            self.assertEqual(len(self.engine.calls), 1)
            self.assertEqual(activity.last_attempt, 8)
            clock.return_value = 12
            activity.service_actions()
            self.assertEqual(activity.idle_count, 0)

    def test_maintenance_failure_marks_not_ready_until_success_without_leaking_input(self):
        self.engine.error = RuntimeError('PRIVATE request text')
        captured = io.StringIO()
        with patch.object(serve_local.time, 'monotonic', return_value=0) as clock:
            activity = serve_local.InferenceActivity(self.engine, FakeRequest, self.body, 5)
            clock.return_value = 5
            with patch.object(serve_local.sys, 'stderr', captured):
                activity.service_actions()
            self.assertFalse(activity.ready)
            self.assertEqual(activity.idle_failures, 1)
            handler = self.handler(activity, '/ready')
            handler.do_GET()
            self.assertEqual(handler.responses[0][0], 503)
            self.assertNotIn('PRIVATE', captured.getvalue() + str(handler.responses))
            self.engine.error = None
            clock.return_value = 10
            activity.service_actions()
            handler.do_GET()
            self.assertEqual(handler.responses[-1][0], 200)
            self.assertTrue(activity.ready)
            self.assertEqual(activity.idle_count, 2)

    def test_real_inference_restores_readiness_after_failed_idle_pass(self):
        activity = serve_local.InferenceActivity(self.engine, FakeRequest, self.body, 5)
        activity.ready = False
        handler = self.handler(activity, '/v1/systemone')
        handler.do_POST()
        self.assertEqual(handler.responses[0][0], 200)
        self.assertTrue(activity.ready)

    def test_failed_idle_pass_keeps_process_context_and_restores_residency_on_exit(self):
        self.engine.error = RuntimeError('failed')
        mx = types.SimpleNamespace(
            device_info=Mock(return_value={'max_recommended_working_set_size': 4096 * serve_local.MIB,
                                          'memory_size': 8192 * serve_local.MIB}),
            get_active_memory=Mock(return_value=1000 * serve_local.MIB),
            synchronize=Mock(), set_wired_limit=Mock(return_value=32 * serve_local.MIB),
        )
        with patch.object(serve_local.time, 'monotonic', return_value=0) as clock, \
             patch.object(serve_local.sys, 'stderr', io.StringIO()):
            activity = serve_local.InferenceActivity(self.engine, FakeRequest, self.body, 5)
            with serve_local.wired_residency(mx, 128 * serve_local.MIB):
                clock.return_value = 5
                activity.service_actions()
                self.assertFalse(activity.ready)
                self.assertEqual(mx.set_wired_limit.call_count, 1)
            self.assertEqual(mx.set_wired_limit.call_args.args, (32 * serve_local.MIB,))


class ArtifactPinTests(unittest.TestCase):
    def test_pinned_overlay_preserves_original_checkpoint(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            checkpoint = directory / 'snapshots' / 'checkpoint-revision'
            checkpoint.mkdir(parents=True)
            base = directory / 'base' / 'snapshots' / 'base-revision'
            base.mkdir(parents=True)
            config = checkpoint / 'hobson_config.json'
            original = json.dumps({'base_model': str(base)})
            config.write_text(original)
            (checkpoint / 'head.safetensors').write_bytes(b'head')
            overlay = directory / 'overlay'
            overlay.mkdir()
            metadata = serve_local.pinned_overlay(checkpoint, None, overlay)
            self.assertEqual(config.read_text(), original)
            self.assertEqual(json.loads((overlay / config.name).read_text())['base_model'], str(base))
            self.assertTrue((overlay / 'head.safetensors').is_symlink())
            self.assertEqual(metadata['checkpoint_revision'], 'checkpoint-revision')
            self.assertEqual(metadata['base_model_revision'], 'base-revision')
            self.assertEqual(metadata['head_sha256'], hashlib.sha256(b'head').hexdigest())

    def test_default_hub_checkpoint_uses_fixed_revision(self):
        hub = types.ModuleType('huggingface_hub')
        hub.snapshot_download = Mock(return_value='/cache/snapshots/revision')
        with patch.dict(sys.modules, {'huggingface_hub': hub}):
            serve_local.resolve_checkpoint(serve_local.DEFAULT_CHECKPOINT)
        hub.snapshot_download.assert_called_once_with(
            serve_local.DEFAULT_CHECKPOINT, revision=serve_local.DEFAULT_CHECKPOINT_REVISION)

    def test_base_provenance_revision_is_used(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            checkpoint = directory / 'checkpoint'
            checkpoint.mkdir()
            (checkpoint / 'strands_decider_config.json').write_text(json.dumps({'base_model': 'org/base'}))
            (checkpoint / 'provenance.json').write_text(json.dumps({'base_model_revision': 'original'}))
            overlay = directory / 'overlay'
            overlay.mkdir()
            with patch.object(serve_local, 'resolve_checkpoint', return_value=directory / 'base') as resolve:
                serve_local.pinned_overlay(checkpoint, None, overlay)
            resolve.assert_called_once_with('org/base', 'original')

    def test_explicit_base_revision_overrides_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            checkpoint = directory / 'checkpoint'
            checkpoint.mkdir()
            (checkpoint / 'strands_decider_config.json').write_text(json.dumps({'base_model': 'org/base'}))
            (checkpoint / 'provenance.json').write_text(json.dumps({'base_model_revision': 'original'}))
            overlay = directory / 'overlay'
            overlay.mkdir()
            with patch.object(serve_local, 'resolve_checkpoint', return_value=directory / 'base') as resolve:
                serve_local.pinned_overlay(checkpoint, 'explicit', overlay)
            resolve.assert_called_once_with('org/base', 'explicit')


if __name__ == '__main__':
    unittest.main(verbosity=2)
