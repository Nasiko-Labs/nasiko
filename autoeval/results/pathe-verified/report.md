# Nasiko AutoEval

Run: `autoeval-20261128-140128-dccaa8`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.

Task kind: **real_repository**. Repository: https://github.com/unjs/pathe.git.
Parent: `55a21be8a14b70ec67098d3f68f54a34f3796fd4`. Reference fix: `b52fcacc59717a1e4a8783d4bbfae7fa81b4db07`.
Toolchain: typescript / vitest.

Validation: **rejected**. Validation infrastructure failure: Duplicate test identity: test/index.spec.ts::join join('some/nodejs/deep','../path') should be 'some/nodejs/path'
Repeated clean runs: 2 old + 2 reference.
Fail-to-pass: 0; pass-to-pass: 0.

| Model | Resolved / attempted | Rate | Infrastructure failures | Attempt cost | Cost / attempted | Cost / resolved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |

Model preflight overhead (separate from attempts): $0.000000; known recorded portion: $0.000000.

Costs are Nasiko's recorded amounts. Missing, ambiguous, or estimated usage stays N/A; no locally estimated prices are substituted.
Infrastructure failures remain in the attempted-task denominator and are also shown separately.

## Attempt outcomes

- Blocker: Validation infrastructure failure: Duplicate test identity: test/index.spec.ts::join join('some/nodejs/deep','../path') should be 'some/nodejs/path'

## Grading evidence

Required regression identities:


Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.

This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.
