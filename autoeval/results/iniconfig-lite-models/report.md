# Nasiko AutoEval

Run: `autoeval-20261128-140610-27f253`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.

Task kind: **real_repository**. Repository: https://github.com/pytest-dev/iniconfig.git.
Parent: `40d8c03a1553953350d787e45f1cb768e2c14e6a`. Reference fix: `d2dcb7289291e7e7019c183659814d42f7264b2d`.
Toolchain: python / pytest.

Validation: **verified**. 
Repeated clean runs: 2 old + 2 reference.
Fail-to-pass: 3; pass-to-pass: 51.

| Model | Resolved / attempted | Rate | Infrastructure failures | Attempt cost | Cost / attempted | Cost / resolved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| gemini-3.1-flash-lite | 1 / 1 | 100.0% | 0 | N/A | N/A | N/A |
| gemini-3.5-flash-lite | 0 / 1 | 0.0% | 0 | N/A | N/A | N/A |

Model preflight overhead (separate from attempts): N/A; known recorded portion: $0.000000.

Costs are Nasiko's recorded amounts. Missing, ambiguous, or estimated usage stays N/A; no locally estimated prices are substituted.
Infrastructure failures remain in the attempted-task denominator and are also shown separately.

## Attempt outcomes

- **gemini-3.1-flash-lite**: resolved; 6.23s; N/A. All expected tests passed.
  Cost evidence gap: Nasiko marked this attempt's price/usage as estimated
- **gemini-3.5-flash-lite**: unresolved; 6.25s; N/A. Required or existing tests did not pass: ['testing.test_iniconfig::test_parse_utf8_bom_file', 'testing.test_iniconfig::test_utf8_bom_file']
  Cost evidence gap: Nasiko marked this attempt's price/usage as estimated
- Blocker: Pricing gemini:gemini-3.1-flash-lite: Nasiko marked this attempt's price/usage as estimated; correctness-only continuation explicitly enabled
- Blocker: Pricing gemini:gemini-3.5-flash-lite: Nasiko marked this attempt's price/usage as estimated; correctness-only continuation explicitly enabled

## Grading evidence

Required regression identities:

- `testing.test_iniconfig::test_parse_utf8_bom_file`
- `testing.test_iniconfig::test_utf8_bom_file`
- `testing.test_iniconfig::test_utf8_bom_in_data_string`

Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.

This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.
