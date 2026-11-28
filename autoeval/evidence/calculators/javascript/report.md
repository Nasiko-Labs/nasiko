# Nasiko AutoEval

Run: `autoeval-20261128-150842-ee2184`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.

Task kind: **local_fixture**. Repository: local fixture.
Parent: `fa41707e6b235a83dc1187fae09dead960792f13`. Reference fix: `f8c5d1516534163252aa4758ddc3d86ecb3c1ae2`.
Toolchain: javascript / jest.

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

- `tests/calculator.test.js::negative`
- `tests/calculator.test.js::positive`

Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.

This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.
