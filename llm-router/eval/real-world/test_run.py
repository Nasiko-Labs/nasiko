"""Stdlib-only runner tests; all generated data lives in temporary directories."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import run as runner
import score as scorer


class RunnerTests(unittest.TestCase):
    def test_defaults_and_explicit_endpoint_overrides(self):
        args = runner.parse_args(['--binary', '/binary', '--source-root', '/source'])
        self.assertEqual((args.repeats, args.output_name, args.timeout_ms), (2, 'results', 1500))
        self.assertEqual(args.strands_url, 'http://127.0.0.1:18099')
        self.assertEqual(args.laya_url, 'http://127.0.0.1:18098')
        self.assertEqual(args.backends, ['regex', 'laya', 'strands'])
        args = runner.parse_args([
            '--binary', '/binary', '--source-root', '/source', '--out', '/new',
            '--strands-url', 'http://127.0.0.1:18899', '--laya-url', 'http://127.0.0.1:18898',
        ])
        self.assertEqual(args.out, Path('/new'))
        self.assertEqual(args.strands_url, 'http://127.0.0.1:18899')
        self.assertEqual(args.laya_url, 'http://127.0.0.1:18898')

    def test_existing_runs_are_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            for name in ('results', 'offline-results'):
                output = Path(directory) / name
                output.mkdir()
                frozen = output / 'strands-run1.jsonl'
                frozen.write_text('frozen')
                with self.assertRaises(ValueError):
                    runner.ensure_fresh_output(output)
                self.assertEqual(frozen.read_text(), 'frozen')

    def test_alternative_backend_order_and_endpoint_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            source = root / 'source'
            source.mkdir()
            binary = root / 'binary'
            binary.write_bytes(b'frozen binary')
            data = root / 'sample-labelled.json'
            data.write_text(json.dumps({'examples': [{'id': 'a'}]}))
            (root / 'labels-frozen.sha256').write_text(runner.digest(data))
            selected = ['local', 'strands_raw', 'hybrid']
            invocations = []

            def fake_binary(command, **kwargs):
                env = kwargs['env']
                invocations.append(env['REQUEST_CLASSIFIER'])
                Path(env['OUT']).write_text('{"id":"a"}\n')

            with patch.object(runner, 'ROOT', root), \
                 patch.object(runner, 'get_health', return_value={'ready': True}) as health, \
                 patch.object(runner.subprocess, 'run', side_effect=fake_binary), \
                 patch.object(runner.subprocess, 'check_output', side_effect=lambda command, **kwargs:
                              'abc\n' if kwargs.get('text') else b'diff'), \
                 contextlib.redirect_stdout(io.StringIO()):
                runner.main(['--binary', str(binary), '--source-root', str(source),
                             '--out', str(root / 'selected'), '--backends', *selected,
                             '--strands-url', 'http://127.0.0.1:18899'])
            manifest = json.loads((root / 'selected/run-manifest.json').read_text())
            self.assertEqual(invocations, selected * 2)
            self.assertEqual(manifest['requested_backends'], selected)
            self.assertEqual(manifest['order'], selected)
            self.assertEqual(manifest['backend_urls'], {
                'strands_raw': 'http://127.0.0.1:18899', 'hybrid': 'http://127.0.0.1:18899'})
            self.assertEqual(manifest['timeouts_ms'], {'strands_raw': 1500, 'hybrid': 1500})
            health.assert_called_once_with('http://127.0.0.1:18899')

    def test_mock_run_records_provenance_and_runs_every_backend_twice(self):
        required_sources = {
            'llm-router/assets/local_classifier.json',
            'llm-router/assets/strands_calibration.json',
            'llm-router/src/routing/hybrid.rs',
            'llm-router/src/routing/strands.rs',
        }
        self.assertTrue(required_sources <= set(runner.SOURCE_FILES))
        self.assertEqual(len(runner.SOURCE_FILES), len(set(runner.SOURCE_FILES)))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            source = root / 'source'
            source.mkdir()
            binary = root / 'binary'
            binary.write_bytes(b'frozen binary')
            data = root / 'sample-labelled.json'
            data.write_text(json.dumps({'examples': [{'id': 'case-1'}, {'id': 'case-2'}]}))
            label_hash = runner.digest(data)
            (root / 'labels-frozen.sha256').write_text(label_hash)
            for relative in required_sources:
                file = source / relative
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text('{}' if file.suffix == '.json' else '// source snapshot fixture\n')
            artifact = source / 'llm-router/assets/strands_calibration.json'
            artifact.write_text(json.dumps({'type_temperature': 1, 'provenance': {'training_rows': 280}}))
            invocations = []

            def fake_binary(command, **kwargs):
                env = kwargs['env']
                invocations.append(env)
                Path(env['OUT']).write_text(''.join(
                    json.dumps({'id': identifier, 'source': env['REQUEST_CLASSIFIER']}) + '\n'
                    for identifier in ('case-1', 'case-2')
                ))

            def fake_git(command, **kwargs):
                return 'abc\n' if kwargs.get('text') else b'diff'

            output = root / 'optimized'
            with patch.object(runner, 'ROOT', root), \
                 patch.object(runner, 'get_health', return_value={'ready': True, 'model': 'q4'}), \
                 patch.object(runner.subprocess, 'run', side_effect=fake_binary), \
                 patch.object(runner.subprocess, 'check_output', side_effect=fake_git), \
                 contextlib.redirect_stdout(io.StringIO()):
                runner.main([
                    '--binary', str(binary), '--source-root', str(source), '--out', str(output),
                    '--strands-url', 'http://127.0.0.1:18899', '--laya-url', 'http://127.0.0.1:18898',
                ])
            manifest = json.loads((output / 'run-manifest.json').read_text())
            self.assertEqual(manifest['status'], 'complete')
            self.assertEqual(len(invocations), 6)
            self.assertEqual([row['backend'] for row in manifest['runs']], ['regex', 'laya', 'strands'] * 2)
            self.assertEqual([row['repeat'] for row in manifest['runs']], [1, 1, 1, 2, 2, 2])
            self.assertEqual(manifest['labels_sha256'], label_hash)
            self.assertEqual(manifest['binary_sha256'], runner.digest(binary))
            self.assertEqual(manifest['health']['strands']['model'], 'q4')
            self.assertEqual(manifest['calibration']['sources'][0]['sha256'], runner.digest(artifact))
            self.assertEqual(manifest['source_snapshot'], str(output / 'source-snapshot'))
            for relative in required_sources:
                copied = output / 'source-snapshot' / relative
                self.assertEqual(copied.read_bytes(), (source / relative).read_bytes())
                self.assertEqual(manifest['source_files_sha256'][relative], runner.digest(copied))
            self.assertTrue(all(env['STRANDS_URL'] == 'http://127.0.0.1:18899' for env in invocations))
            self.assertTrue(all(env['LAYA_URL'] == 'http://127.0.0.1:18898' for env in invocations))
            self.assertFalse((root / 'results').exists())
            self.assertFalse((root / 'offline-results').exists())

    def test_changed_frozen_labels_rejected_before_network_or_execution(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'sample-labelled.json').write_text('{}')
            (root / 'labels-frozen.sha256').write_text('incorrect')
            with patch.object(runner, 'ROOT', root), \
                 patch.object(runner, 'get_health') as health, \
                 patch.object(runner.subprocess, 'run') as binary:
                with self.assertRaises(ValueError):
                    runner.main([
                        '--binary', '/unused', '--source-root', str(root), '--out', str(root / 'out'),
                    ])
                health.assert_not_called()
                binary.assert_not_called()
            self.assertFalse((root / 'out').exists())

    def test_prediction_duplicates_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            predictions = Path(directory) / 'bad.jsonl'
            predictions.write_text('{"id":"a"}\n{"id":"a"}\n')
            with self.assertRaises(ValueError):
                runner.validate_predictions(predictions, ['a', 'b'])

    def test_backend_lists_reject_duplicates_and_keep_legacy_defaults(self):
        self.assertEqual(scorer.parse_args([]).backends, ['regex', 'laya', 'strands'])
        for parser, prefix in [(runner.parse_args, ['--binary', '/binary', '--source-root', '/source']),
                               (scorer.parse_args, [])]:
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                parser([*prefix, '--backends', 'strands', 'strands'])

    def test_dynamic_scorer_columns_admission_and_prediction_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / 'selected'
            output.mkdir()
            gold = [dict(id='a', request_type='writing', complexity=4,
                         acceptable_types=['writing'], ambiguous=False, user_turn=1, query='draft'),
                    dict(id='b', request_type='factual_lookup', complexity=1,
                         acceptable_types=['factual_lookup'], ambiguous=False, user_turn=2, query='fact')]
            labels = root / 'sample-labelled.json'
            labels.write_text(json.dumps({'examples': gold}))
            (root / 'labels-frozen.sha256').write_text(runner.digest(labels))
            selected = ['local', 'strands_raw', 'strands']
            runs = []
            for backend in selected:
                for repeat in (1, 2):
                    predictions = [dict(id=row['id'], request_type=row['request_type'],
                                        complexity=row['complexity'], confidence=.45 if row['id']=='a' else .8,
                                        latency_us=600, source='local' if backend=='local' else 'strands',
                                        fallback_reason=None, low_confidence=backend=='strands' and row['id']=='a')
                                   for row in gold]
                    predictions[1]['request_type'] = 'general'
                    path = output / f'{backend}-run{repeat}.jsonl'
                    path.write_text(''.join(json.dumps(row)+'\n' for row in predictions))
                    runs.append(dict(backend=backend, repeat=repeat, predictions_sha256=runner.digest(path)))
            manifest = dict(requested_backends=selected, order=selected, runs=runs,
                            binary_sha256='recorded-binary', labels_sha256=runner.digest(labels),
                            timeouts_ms={'strands_raw':1500,'strands':1500})
            (output / 'run-manifest.json').write_text(json.dumps(manifest))
            with patch.object(scorer, 'ROOT', root), contextlib.redirect_stdout(io.StringIO()):
                scorer.main(['--out', str(output), '--backends', *selected])
            summary = json.loads((output / 'summary.json').read_text())
            self.assertEqual(list(summary), selected)
            self.assertEqual(summary['strands']['below_confidence_floor'], 0)
            self.assertEqual(summary['strands']['low_confidence'], 1)
            self.assertEqual(summary['strands']['trusted_n'], 1)
            self.assertEqual(summary['strands']['confident_wrong_count'], 1)
            self.assertEqual(summary['strands_raw']['trusted_n'], 2)
            report = (output / 'report.md').read_text()
            self.assertIn('| Metric | Local | Strands raw 4-bit | Strands 4-bit |', report)
            self.assertIn('1500 ms deadline', report)
            self.assertIn('recorded-binary', report)
            self.assertEqual(summary['strands']['changed_classifications'], [])
            self.assertEqual(runner.digest(labels), manifest['labels_sha256'])
            changed = output / 'local-run1.jsonl'
            changed.write_text(changed.read_text()+'\n')
            with patch.object(scorer, 'ROOT', root), self.assertRaises(ValueError):
                scorer.main(['--out', str(output), '--backends', *selected])
            self.assertEqual((output / 'report.md').read_text(), report)


if __name__ == '__main__':
    unittest.main(verbosity=2)
