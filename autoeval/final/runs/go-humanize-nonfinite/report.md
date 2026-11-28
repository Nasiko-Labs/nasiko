# Nasiko AutoEval

Run: `autoeval-20261128-150902-12163b`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.

Task kind: **real_repository**. Repository: https://github.com/dustin/go-humanize.git.
Parent: `ac100271b177ad70370353d75f05ff5185a74ffc`. Reference fix: `1b638b2c0d9b358cbae4b51b74f136d9a6055d1a`.
Toolchain: go / go-test.

Validation: **verified**. 
Repeated clean runs: 2 old + 2 reference.
Fail-to-pass: 1; pass-to-pass: 68.

| Model | Resolved / attempted | Rate | Infrastructure failures | Attempt cost | Cost / attempted | Cost / resolved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| gemini-3.1-flash-lite | 1 / 1 | 100.0% | 0 | $0.006727 | $0.006727 | $0.006727 |
| gemini-3.5-flash-lite | 1 / 1 | 100.0% | 0 | $0.009653 | $0.009653 | $0.009653 |

Model preflight overhead (separate from attempts): $0.000027; known recorded portion: $0.000027.

Costs are Nasiko's recorded amounts. Missing, ambiguous, or estimated usage stays N/A; no locally estimated prices are substituted.
Infrastructure failures remain in the attempted-task denominator and are also shown separately.

## Attempt outcomes

- **gemini-3.1-flash-lite**: resolved; 20.67s; $0.006727. All expected tests passed.
- **gemini-3.5-flash-lite**: resolved; 18.71s; $0.009653. All expected tests passed.

## Grading evidence

Required regression identities:

- `github.com/dustin/go-humanize::TestComputeSINonFinite`

Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.

This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.
