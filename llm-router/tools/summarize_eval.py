#!/usr/bin/env python3
"""Validate the JSONL contract and summarize stored evaluation evidence."""
import json
import math
import re
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / 'data/classifier'
REPORTS = ROOT / 'reports'
KEYS = {'id', 'request_type', 'complexity', 'confidence', 'latency_us'}
LABELS = {'code_generation', 'code_understanding', 'technical_design', 'analytical_reasoning', 'writing', 'factual_lookup', 'general'}


def rows(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def summarize(expected, actual, log):
    assert len(actual) == len(expected)
    assert [x['id'] for x in actual] == [x['id'] for x in expected]
    for item in actual:
        assert set(item) == KEYS
        assert item['request_type'] in LABELS
        assert type(item['complexity']) is int and 1 <= item['complexity'] <= 5
        assert math.isfinite(item['confidence']) and 0 <= item['confidence'] <= 1
        assert type(item['latency_us']) is int and item['latency_us'] >= 0
    latency = sorted(x['latency_us'] for x in actual)
    misses = [dict(id=e['id'], expected_type=e['request_type'], actual_type=a['request_type'], expected_complexity=e['complexity'], actual_complexity=a['complexity'], confidence=a['confidence']) for e, a in zip(expected, actual) if e['request_type'] != a['request_type'] or e['complexity'] != a['complexity']]
    matched = re.search(r'fallbacks=(\d+) decisions=(\d+)', log)
    assert matched and int(matched[2]) == len(actual)
    count = len(actual)
    return dict(examples=count, request_type_correct=sum(e['request_type'] == a['request_type'] for e,a in zip(expected,actual)), complexity_correct=sum(e['complexity'] == a['complexity'] for e,a in zip(expected,actual)), joint_correct=count-len(misses), complexity_mae=statistics.mean(abs(e['complexity']-a['complexity']) for e,a in zip(expected,actual)), fallbacks=int(matched[1]), fallback_rate=int(matched[1])/count, p50_latency_us=statistics.median(latency), p95_latency_us=latency[math.ceil(.95*count)-1], mean_latency_us=statistics.mean(latency), confident_wrong_at_0_8=sum(a['confidence'] >= .8 and (e['request_type'] != a['request_type'] or e['complexity'] != a['complexity']) for e,a in zip(expected,actual)), external_service_cost_usd_per_decision=0.0, cost_note='Regex/local have no paid API calls. CPU/runner billing is not known; this is not a claim of zero total compute cost.', mismatches=misses)


def main():
    result = {}
    for split, path in [('public', DATA/'public-eval.json'), ('validation', DATA/'validation.json')]:
        expected = json.loads(path.read_text())['examples']
        result[split] = {}
        for backend in ['regex', 'local']:
            result[split][backend] = summarize(expected, rows(REPORTS/f'{split}-{backend}.jsonl'), (REPORTS/f'{split}-{backend}.log').read_text())
            print(split, backend, json.dumps({k:v for k,v in result[split][backend].items() if k not in ['mismatches','cost_note']}))
    (REPORTS/'metrics.json').write_text(json.dumps(result,indent=2)+'\n')


if __name__ == '__main__':
    main()
