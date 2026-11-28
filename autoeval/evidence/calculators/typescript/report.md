# Nasiko AutoEval

Run: `autoeval-20261128-150922-823dbe`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.

Task kind: **local_fixture**. Repository: local fixture.
Parent: `f13222ea0bb6b811d88de97fac7a8e7139886dea`. Reference fix: `24df7bcb9176e6cb736e01533c2b3768248277a7`.
Toolchain: typescript / vitest.

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

- `tests/calculator.test.ts::negative`
- `tests/calculator.test.ts::positive`

Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.

This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.
