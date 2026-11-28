# Nasiko AutoEval

Run: `autoeval-20261128-150832-fa8f07`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.

Task kind: **real_repository**. Repository: https://github.com/pytest-dev/iniconfig.git.
Parent: `40d8c03a1553953350d787e45f1cb768e2c14e6a`. Reference fix: `d2dcb7289291e7e7019c183659814d42f7264b2d`.
Toolchain: python / pytest.

Validation: **verified**. 
Repeated clean runs: 2 old + 2 reference.
Fail-to-pass: 3; pass-to-pass: 51.

| Model | Resolved / attempted | Rate | Infrastructure failures | Attempt cost | Cost / attempted | Cost / resolved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| gemini-3.1-flash-lite | 1 / 1 | 100.0% | 0 | $0.003231 | $0.003231 | $0.003231 |
| gemini-3.5-flash-lite | 1 / 1 | 100.0% | 0 | $0.004903 | $0.004903 | $0.004903 |

Model preflight overhead (separate from attempts): $0.000027; known recorded portion: $0.000027.

Costs are Nasiko's recorded amounts. Missing, ambiguous, or estimated usage stays N/A; no locally estimated prices are substituted.
Infrastructure failures remain in the attempted-task denominator and are also shown separately.

## Attempt outcomes

- **gemini-3.1-flash-lite**: resolved; 7.36s; $0.003231. All expected tests passed.
- **gemini-3.5-flash-lite**: resolved; 5.44s; $0.004903. All expected tests passed.

## Grading evidence

Required regression identities:

- `testing.test_iniconfig::test_parse_utf8_bom_file`
- `testing.test_iniconfig::test_utf8_bom_file`
- `testing.test_iniconfig::test_utf8_bom_in_data_string`

Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.

This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.
