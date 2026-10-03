# Dataset Card — P2 classifier

## What is official vs ours?
Nasiko publishes a 10-case smoke/evaluation sample and keeps an approximately 200-case private scoring set. The participant brief explicitly instructs us to build our own labeled training and validation data.

## Files
- `LABELING_GUIDE.md`: our labeling rules.
- `request-classifier-train.json`: 364 labeled training examples.
- `request-classifier-validation.json`: 91 labeled validation examples.

The two splits combine the curated seed set, ambiguous/multi-intent hard negatives,
and controlled synthetic expansions. The team confirmed the synthetic labels and
approved these examples for model development. Source collections and model-fitting
scripts remain in the team's Python experimentation workspace; the labeled splits
needed to inspect and evaluate this Rust implementation are included here.

## Important limitation
The controlled expansion contains repeated task templates with surface variations. Its
labels are approved by the team, but a high score on it does not establish generalization
to independently authored requests or Nasiko's private set. The current collection is
development data, not a final blind test.

## Split and validation policy
The split is deterministic and family-grouped, with balanced request-type and
complexity coverage. Train and validation have all seven request types and no
overlapping IDs or families; normalized-query overlap was also checked before
exporting the fixtures.

Before production or a performance claim, collect more independently authored and
reviewed paraphrase, out-of-distribution, noisy/padded, context-paired, and regex-near-miss
cases. Keep a final blind family-held-out test set that is not used to choose rules or
model settings.
