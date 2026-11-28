# Nasiko AutoEval — frozen benchmark

Dataset: `buildathon-final-v1`. 3 real tasks; 6 assigned model/task slots.

Membership was declared before final generation in planned.json. Only those runs are scored. Synthetic calculator demos, security probes, pricing diagnostics and development reruns are excluded.

| Model | Resolved / assigned | Rate | Infrastructure failures | Generation cost | Cost / assigned task | Cost / resolution | Cost coverage |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| gemini-3.1-flash-lite | 2 / 3 | 66.7% | 0 | $0.016999 | $0.005666 | $0.008500 | 3/3 |
| gemini-3.5-flash-lite | 2 / 3 | 66.7% | 0 | $0.025345 | $0.008448 | $0.012672 | 3/3 |

Costs are Nasiko ledger values at Exact, non-estimated rates; preflight is separate overhead. No local token-price estimate replaces a recorded amount.

| Task | Language / framework | Regression requirements | Existing passing requirements |
| --- | --- | ---: | ---: |
| `d2dcb7289291e7e7019c183659814d42f7264b2d` | python / pytest | 3 | 51 |
| `1b638b2c0d9b358cbae4b51b74f136d9a6055d1a` | go / go-test | 1 | 68 |
| `b52fcacc59717a1e4a8783d4bbfae7fa81b4db07` | typescript / vitest | 10 | 457 |

## Outcomes

- `d2dcb7289291` / gemini-3.1-flash-lite: **resolved**, $0.003231. All expected tests passed
- `d2dcb7289291` / gemini-3.5-flash-lite: **resolved**, $0.004903. All expected tests passed
- `1b638b2c0d9b` / gemini-3.1-flash-lite: **resolved**, $0.006727. All expected tests passed
- `1b638b2c0d9b` / gemini-3.5-flash-lite: **resolved**, $0.009653. All expected tests passed
- `b52fcacc5971` / gemini-3.1-flash-lite: **unresolved**, $0.007041. Required or existing tests did not pass: ["test/index.spec.ts::join join('\\\\\\\\.\\\\c:\\\\temp\\\\file','..\\\\path') should be '//./c:/temp/path'", "test/index.spec.ts::join join('\\\\\\\\.\\\\c:\\\\temp\\\\file','..\\\\path') should be '//./c:/temp/path' on Windows", "test/index.spec.ts::normalize normalize('\\\\\\\\.\\\\c:\\\\temp\\\\file\... (full identities in results.json)
- `b52fcacc5971` / gemini-3.5-flash-lite: **unresolved**, $0.010789. Candidate code prevented the verified suite from compiling/loading: Test suite failed before collecting test cases

Preflight overhead: $0.000081 known recorded; 0 failed/retried probes. Failed calls with no provider usage have unknown cost and are not silently treated as free.

## Trust and reproducibility

Reports/logs are captured outside the candidate container. The report/log overwrite plus forced-exit regression was reproduced before hardening and rejected afterward. Candidate code still runs inside test frameworks and may manipulate framework state or forge process output: **grading is not tamper-proof**. These are observed test outcomes for ordinary one-shot patches, not adversarially secure correctness proofs.

The sealed file checksums, pinned runtime digests, dependency inventories, repository Git bundles, exact SHAs, immutable test identities, prompts, candidates, raw captures and Nasiko usage records are retained. Public-task selection bias and model training overlap remain uncontrolled; no general model ranking follows from this sample.
