#!/usr/bin/env python3
"""Run the compiled release example and real fallback demos, then validate its outputs."""
import json
import os
import subprocess
from pathlib import Path
from summarize_eval import main as summarize

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / 'data/classifier'
REPORTS = ROOT / 'reports'
BINARY = ROOT.parent / 'target/release/examples/classifier_eval'


def run(name, backend, eval_set, **settings):
    env = os.environ.copy()
    # Caller overrides must not accidentally influence a baseline comparison.
    for key in ['CLASSIFIER_MODEL_PATH', 'CLASSIFIER_ENDPOINT', 'CLASSIFIER_TIMEOUT_MS']:
        env.pop(key, None)
    env.update(EVAL_SET=str(eval_set), OUT=str(REPORTS/f'{name}.jsonl'), CLASSIFIER_BACKEND=backend, **settings)
    with (REPORTS/f'{name}.log').open('w') as log:
        subprocess.run([str(BINARY)], env=env, stdout=log, stderr=log, check=True)


def main():
    if not BINARY.exists():
        raise SystemExit('First run cargo build --release -p nasiko-llm-router --example classifier_eval')
    REPORTS.mkdir(exist_ok=True)
    for split, source in [('public', DATA/'public-eval.json'), ('validation', DATA/'validation.json')]:
        for backend in ['regex', 'local']:
            run(f'{split}-{backend}', backend, source)
    run('missing-model', 'local', DATA/'public-eval.json', CLASSIFIER_MODEL_PATH=str(REPORTS/'intentionally-absent-model.json'))
    # An invalid endpoint tests hosted setup failure without depending on port availability.
    run('hosted-config-failure', 'hosted', DATA/'public-eval.json', CLASSIFIER_ENDPOINT='not-a-valid-url')
    run('unsupported-input', 'local', DATA/'fallback-demo.json')
    run('determinism-repeat', 'local', DATA/'validation.json')
    def predictions(name):
        return [{k:v for k,v in json.loads(line).items() if k != 'latency_us'} for line in (REPORTS/f'{name}.jsonl').read_text().splitlines()]
    assert predictions('validation-local') == predictions('determinism-repeat')
    for name in ['missing-model', 'hosted-config-failure', 'unsupported-input']:
        assert all(row['complexity'] == 1 and abs(row['confidence']-.4) < 1e-6 for row in predictions(name))
    summarize()


if __name__ == '__main__':
    main()
