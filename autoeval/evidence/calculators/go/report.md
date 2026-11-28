# Nasiko AutoEval

Run: `autoeval-20261128-150943-3bed3a`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.

Task kind: **local_fixture**. Repository: local fixture.
Parent: `c4b1a2dc43632956156a336b07551d8e0fe40b42`. Reference fix: `5340a20524758b6d1a1ad51d851e1f751dfd6fee`.
Toolchain: go / go-test.

Validation: **verified**. 
Repeated clean runs: 2 old + 2 reference.
Fail-to-pass: 2; pass-to-pass: 1.

| Model | Resolved / attempted | Rate | Infrastructure failures | Attempt cost | Cost / attempted | Cost / resolved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |

Model preflight overhead (separate from attempts): $0.000000; known recorded portion: $0.000000.

Costs are Nasiko's recorded amounts. Missing, ambiguous, or estimated usage stays N/A; no locally estimated prices are substituted.
Infrastructure failures remain in the attempted-task denominator and are also shown separately.

## Attempt outcomes


## Grading evidence

Required regression identities:

- `example.invalid/synthetic-calculator::TestNegative`
- `example.invalid/synthetic-calculator::TestPositive`

Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.

This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.
