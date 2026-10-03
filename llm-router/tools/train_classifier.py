#!/usr/bin/env python3
"""Train fixed CPU linear heads; validation and public data never enter fitting."""
import json
import re
from pathlib import Path

import numpy as np
from sklearn.linear_model import LogisticRegression

ROOT = Path(__file__).resolve().parents[1]
DIM = 2048
LABELS = ['code_generation', 'code_understanding', 'technical_design', 'analytical_reasoning', 'writing', 'factual_lookup', 'general']


def bucket(feature):
    value = 2166136261
    for byte in feature.encode('ascii'):
        value = ((value ^ byte) * 16777619) & 0xffffffff
    return value % DIM


def features(query, context):
    result = np.zeros(DIM)
    for text, scale in [(query, 1.0), (context or '', .25)]:
        words = re.findall(r'[a-z0-9_]+', text.lower())
        for word in words:
            result[bucket('w:' + word)] += scale
        for first, second in zip(words, words[1:]):
            result[bucket('b:' + first + ' ' + second)] += scale
        normalized = ' '.join(words)
        for i in range(max(0, len(normalized) - 3)):
            result[bucket('c:' + normalized[i:i + 4])] += .1 * scale
    norm = np.linalg.norm(result)
    return result / norm if norm else result


def audit_splits(train, validation):
    def tokens(row):
        return set(re.findall(r'[a-z0-9_]+', row['query'].lower()))
    maximum, pair = 0, None
    for a in train:
        for b in validation:
            x, y = tokens(a), tokens(b)
            score = len(x & y) / max(1, len(x | y))
            if score > maximum:
                maximum, pair = score, [a['id'], b['id']]
            if a['query'].strip().lower() == b['query'].strip().lower() or score >= .65:
                raise ValueError(f"Cross-split near duplicate: {a['id']}, {b['id']}")
    return dict(threshold=.65, max_query_token_jaccard=maximum, closest_pair=pair)


def main():
    data = ROOT / 'data/classifier'
    train = json.loads((data / 'train.json').read_text())['examples']
    validation = json.loads((data / 'validation.json').read_text())['examples']
    audit = audit_splits(train, validation)
    x = np.array([features(row['query'], row.get('context')) for row in train])
    heads = []
    for target in ['request_type', 'complexity']:
        y = [LABELS.index(row[target]) if target == 'request_type' else row[target] - 1 for row in train]
        head = LogisticRegression(C=8, max_iter=2000, solver='lbfgs', random_state=0)
        head.fit(x, y)
        heads.append(dict(weights=head.coef_.tolist(), bias=head.intercept_.tolist()))
    model = dict(version=1, dimension=DIM, labels=LABELS, type_head=heads[0], complexity_head=heads[1], confidence_cap=.8, temperature=2.0, train_examples=len(train))
    (data / 'model.json').write_text(json.dumps(model, separators=(',', ':')) + '\n')
    golden = []
    for row in train[:4]:
        vector = features(row['query'], row.get('context'))
        predictions = []
        for head in heads:
            logits = (np.array(head['weights']) @ vector + np.array(head['bias'])) / 2.0
            choice = int(np.argmax(logits))
            probability = float(1.0 / np.exp(logits - logits[choice]).sum())
            predictions.append((choice, probability))
        golden.append(dict(query=row['query'], context=row.get('context'), request_type=LABELS[predictions[0][0]], complexity=predictions[1][0]+1, confidence=min(predictions[0][1], predictions[1][1], .8)))
    (data / 'golden.json').write_text(json.dumps(golden, indent=2) + '\n')
    audit['train_examples'], audit['validation_examples'] = len(train), len(validation)
    audit['method'] = 'Case-insensitive exact query check and query token-set Jaccard; manually authored validation uses different tasks. This heuristic cannot prove semantic independence.'
    (data / 'split-audit.json').write_text(json.dumps(audit, indent=2) + '\n')
    print(json.dumps(audit, indent=2))


if __name__ == '__main__':
    main()
