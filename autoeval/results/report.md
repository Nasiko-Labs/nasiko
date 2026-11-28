# Nasiko AutoEval — measured results

One implementation; repository language and framework are inputs. These are one-shot production-file attempts, not interactive coding-agent replay.

**3 verified real tasks; 7 model attempts.** Synthetic adapter fixtures are excluded from these totals.

| Repository | Toolchain | Fix commit | Regression tests | Existing tests |
| --- | --- | --- | ---: | ---: |
| https://github.com/dustin/go-humanize.git | go / go-test | `1b638b2c0d9b358cbae4b51b74f136d9a6055d1a` | 1 | 68 |
| https://github.com/pytest-dev/iniconfig.git | python / pytest | `d2dcb7289291e7e7019c183659814d42f7264b2d` | 3 | 51 |
| https://github.com/unjs/pathe.git | typescript / vitest | `b52fcacc59717a1e4a8783d4bbfae7fa81b4db07` | 10 | 457 |

Each accepted task failed on old code and passed on reference code twice in fresh, offline containers. Exact identities and all four results are in its source run.

| Model | Resolved / attempts | Resolution rate | Infrastructure failures | Cost / attempt | Cost / resolution | Cost coverage |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| gemini-3.1-flash-lite | 2 / 3 | 66.7% | 0 | N/A | N/A | 0/3 |
| gemini-3.5-flash-lite | 1 / 3 | 33.3% | 0 | N/A | N/A | 0/3 |
| gemini-3.8-flash | 0 / 1 | 0.0% | 1 | N/A | N/A | 0/1 |

Costs require attributable, non-estimated Nasiko ledger records. Missing or estimated rates remain N/A. Preflight probes and their failures are separate from model-attempt denominators.

## Attempts and limitations

- `1b638b2c0d9b` / **gemini-3.1-flash-lite**: resolved, 16.93s, N/A. All expected tests passed. Cost gap: Nasiko marked this attempt's price/usage as estimated
- `1b638b2c0d9b` / **gemini-3.5-flash-lite**: resolved, 15.89s, N/A. All expected tests passed. Cost gap: Nasiko marked this attempt's price/usage as estimated
- `d2dcb7289291` / **gemini-3.1-flash-lite**: resolved, 6.23s, N/A. All expected tests passed. Cost gap: Nasiko marked this attempt's price/usage as estimated
- `d2dcb7289291` / **gemini-3.5-flash-lite**: unresolved, 6.25s, N/A. Required or existing tests did not pass: ['testing.test_iniconfig::test_parse_utf8_bom_file', 'testing.test_iniconfig::test_utf8_bom_file'] Cost gap: Nasiko marked this attempt's price/usage as estimated
- `d2dcb7289291` / **gemini-3.8-flash**: model_error, 240.18s, N/A. Model request failed: timed out
- `b52fcacc5971` / **gemini-3.1-flash-lite**: unresolved, 18.87s, N/A. Required or existing tests did not pass: ["test/index.spec.ts::join join('\\\\\\\\.\\\\c:\\\\temp\\\\file','..\\\\path') should be '//./c:/temp/path'", "test/index.spec.ts::join join('\\\\\\\\.\\\\c:\\\\temp\\\\file','..\\\\path') should be '//./c:/temp/path' on Windows", "test/index.spec.ts::normalize normalize('\\\\\\\\.\\\\c:\\\\temp\\\\file\\\\..\\\\path') should be '//./c:/temp/path'", "test/index.spec.ts::normalize normalize('\\\\\\\\.\\\\c:\\\\temp\\\\file\\\\..\\\\path') should be '//./c:/temp/path' on Windows", "test/index.spec.ts::normalize normalize('\\\\\\\\.\\\\foo\\\\bar') should be '//./foo/bar'", "test/index.spec.ts::normalize normalize('\\\\\\\\.\\\\foo\\\\bar') should be '//./foo/bar' on Windows", "test/index.spec.ts::resolve resolve('//server/share/file','../path') should be '//server/share/path'", "test/index.spec.ts::resolve resolve('//server/share/file','../path') should be '//server/share/path' on Windows", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file') should be '//server/share/file'", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file') should be '//server/share/file' on Windows", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file','..\\\\path') should be '//server/share/path'", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file','..\\\\path') should be '//server/share/path' on Windows", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file','sub') should be '//server/share/file/sub'", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file','sub') should be '//server/share/file/sub' on Windows", "test/index.spec.ts::resolve resolve('\\\\\\\\unc-server-name\\\\unc-path') should be '//unc-server-name/unc-path'", "test/index.spec.ts::resolve resolve('\\\\\\\\unc-server-name\\\\unc-path') should be '//unc-server-name/unc-path' on Windows"] Cost gap: Nasiko marked this attempt's price/usage as estimated
- `b52fcacc5971` / **gemini-3.5-flash-lite**: unresolved, 12.34s, N/A. Candidate code prevented the verified suite from compiling/loading: Test suite failed before collecting test cases Cost gap: Nasiko marked this attempt's price/usage as estimated

### Recorded ledger amounts with unverified pricing

These are actual Nasiko records, explicitly marked estimated by Nasiko. They are audit evidence, not verified spend or a basis for a cost ranking.

| Task | Model | Nasiko recorded (estimated USD) | Trace |
| --- | --- | ---: | --- |
| `1b638b2c0d9b` | gemini-3.1-flash-lite | $0.010448 | `direct:068ef6e1-4573-4066-b666-6f097bf07fe4` |
| `1b638b2c0d9b` | gemini-3.5-flash-lite | $0.010555 | `direct:8945fd94-8a69-4ccd-85dc-4210c41047b5` |
| `d2dcb7289291` | gemini-3.1-flash-lite | $0.004234 | `direct:025f70bc-88ba-4d2b-8e2d-0c30bc4069d5` |
| `d2dcb7289291` | gemini-3.5-flash-lite | $0.005442 | `direct:95640bba-f7dd-4ac3-91b5-30ca0c020014` |
| `b52fcacc5971` | gemini-3.1-flash-lite | $0.009575 | `direct:17333fda-7ea9-4faf-b59c-046336405c47` |
| `b52fcacc5971` | gemini-3.5-flash-lite | $0.009739 | `direct:0064728b-07cf-43a2-92ab-3d5e12f160c3` |

Preflight: 7 text responses; 2 unavailable/error results. Older prebuilt servers use the documented read-only Postgres ledger bridge because /api/evals/* is absent.

Verified adapter fixture runs (synthetic, excluded from benchmark): cargo, go-test, jest, pytest, vitest

This is a small, selected sample of public commits. Training-data overlap, independent issue review and broad repository compatibility are unmeasured. A model ranking is not supported.

Reproduction commands and grading boundaries: [README](../README.md).
