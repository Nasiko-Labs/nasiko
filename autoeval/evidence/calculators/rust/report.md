# Nasiko AutoEval

Run: `autoeval-20261128-151034-c12c2f`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.

Task kind: **local_fixture**. Repository: local fixture.
Parent: `307399c14c6d26997ef1e37178a155be18e5f075`. Reference fix: `43d754e7dd64f29598ba550eff896d3ead3dd88f`.
Toolchain: rust / cargo.

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

- `calculator::negative`
- `calculator::positive`

Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.

This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.
