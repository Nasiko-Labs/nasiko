# Nasiko AutoEval

Run: `autoeval-20261128-135835-414b60`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.

Task kind: **real_repository**. Repository: https://github.com/pytest-dev/iniconfig.git.
Parent: `40d8c03a1553953350d787e45f1cb768e2c14e6a`. Reference fix: `d2dcb7289291e7e7019c183659814d42f7264b2d`.
Toolchain: python / pytest.

Validation: **verified**. 
Repeated clean runs: 2 old + 0 reference.
Fail-to-pass: 3; pass-to-pass: 51.

| Model | Resolved / attempted | Rate | Infrastructure failures | Attempt cost | Cost / attempted | Cost / resolved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| gemini-3.8-flash | 0 / 1 | 0.0% | 1 | N/A | N/A | N/A |

Successful model preflight overhead (separate from attempts): $0.000000.

Costs are Nasiko's recorded amounts. Missing, ambiguous, or estimated usage stays N/A; no locally estimated prices are substituted.
Infrastructure failures remain in the attempted-task denominator and are also shown separately.

## Attempt outcomes

- **gemini-3.8-flash**: model_error; 240.18s; N/A. Model request failed: timed out
- Blocker: Preflight gemini:gemini-4-hydrogen: Model request HTTP 502: {"detail":"Upstream LLM error: provider returned 503: {\n  \"error\": {\n    \"code\": 503,\n    \"message\": \"This model is currently experiencing high demand. Spikes in demand are usually temporary. Please try again later.\",\n    \"status\": \"UNAVAILABLE\"\n  }\n}\n","error":{"message":"Upstream LLM error: provider returned 503: {\n  \"error\": {\n    \"code\": 503,\n    \"message\": \"This model is currently experiencing high demand. Spikes in demand are usually temporary. Please try again later.\",\n    \"status\": \"UNAVAILABLE\"\n  }\n}\n","type":"api_error"}}
- Blocker: Pricing gemini:gemini-3.8-flash: Nasiko marked this attempt's price/usage as estimated; correctness-only continuation explicitly enabled

## Grading evidence

Required regression identities:

- `testing.test_iniconfig::test_parse_utf8_bom_file`
- `testing.test_iniconfig::test_utf8_bom_file`
- `testing.test_iniconfig::test_utf8_bom_in_data_string`

Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.

This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.
