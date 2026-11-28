# Nasiko AutoEval

Run: `autoeval-20261128-140940-6141ab`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.

Task kind: **real_repository**. Repository: https://github.com/unjs/pathe.git.
Parent: `55a21be8a14b70ec67098d3f68f54a34f3796fd4`. Reference fix: `b52fcacc59717a1e4a8783d4bbfae7fa81b4db07`.
Toolchain: typescript / vitest.

Validation: **verified**. 
Repeated clean runs: 2 old + 2 reference.
Fail-to-pass: 10; pass-to-pass: 457.

| Model | Resolved / attempted | Rate | Infrastructure failures | Attempt cost | Cost / attempted | Cost / resolved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| gemini-3.1-flash-lite | 0 / 1 | 0.0% | 0 | N/A | N/A | N/A |
| gemini-3.5-flash-lite | 0 / 1 | 0.0% | 0 | N/A | N/A | N/A |

Model preflight overhead (separate from attempts): N/A; known recorded portion: $0.000000.

Costs are Nasiko's recorded amounts. Missing, ambiguous, or estimated usage stays N/A; no locally estimated prices are substituted.
Infrastructure failures remain in the attempted-task denominator and are also shown separately.

## Attempt outcomes

- **gemini-3.1-flash-lite**: unresolved; 18.87s; N/A. Required or existing tests did not pass: ["test/index.spec.ts::join join('\\\\\\\\.\\\\c:\\\\temp\\\\file','..\\\\path') should be '//./c:/temp/path'", "test/index.spec.ts::join join('\\\\\\\\.\\\\c:\\\\temp\\\\file','..\\\\path') should be '//./c:/temp/path' on Windows", "test/index.spec.ts::normalize normalize('\\\\\\\\.\\\\c:\\\\temp\\\\file\\\\..\\\\path') should be '//./c:/temp/path'", "test/index.spec.ts::normalize normalize('\\\\\\\\.\\\\c:\\\\temp\\\\file\\\\..\\\\path') should be '//./c:/temp/path' on Windows", "test/index.spec.ts::normalize normalize('\\\\\\\\.\\\\foo\\\\bar') should be '//./foo/bar'", "test/index.spec.ts::normalize normalize('\\\\\\\\.\\\\foo\\\\bar') should be '//./foo/bar' on Windows", "test/index.spec.ts::resolve resolve('//server/share/file','../path') should be '//server/share/path'", "test/index.spec.ts::resolve resolve('//server/share/file','../path') should be '//server/share/path' on Windows", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file') should be '//server/share/file'", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file') should be '//server/share/file' on Windows", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file','..\\\\path') should be '//server/share/path'", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file','..\\\\path') should be '//server/share/path' on Windows", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file','sub') should be '//server/share/file/sub'", "test/index.spec.ts::resolve resolve('\\\\\\\\server\\\\share\\\\file','sub') should be '//server/share/file/sub' on Windows", "test/index.spec.ts::resolve resolve('\\\\\\\\unc-server-name\\\\unc-path') should be '//unc-server-name/unc-path'", "test/index.spec.ts::resolve resolve('\\\\\\\\unc-server-name\\\\unc-path') should be '//unc-server-name/unc-path' on Windows"]
  Cost evidence gap: Nasiko marked this attempt's price/usage as estimated
  Nasiko ledger recorded $0.009575 using estimated pricing; excluded from verified-cost metrics. Trace: `direct:17333fda-7ea9-4faf-b59c-046336405c47`.
- **gemini-3.5-flash-lite**: unresolved; 12.34s; N/A. Candidate code prevented the verified suite from compiling/loading: Test suite failed before collecting test cases
  Cost evidence gap: Nasiko marked this attempt's price/usage as estimated
  Nasiko ledger recorded $0.009739 using estimated pricing; excluded from verified-cost metrics. Trace: `direct:0064728b-07cf-43a2-92ab-3d5e12f160c3`.
- Blocker: Pricing gemini:gemini-3.1-flash-lite: Nasiko marked this attempt's price/usage as estimated; correctness-only continuation explicitly enabled
- Blocker: Pricing gemini:gemini-3.5-flash-lite: Nasiko marked this attempt's price/usage as estimated; correctness-only continuation explicitly enabled

## Grading evidence

Required regression identities:

- `test/index.spec.ts::resolve resolve('//server/share/file','../path') should be '//server/share/path'`
- `test/index.spec.ts::resolve resolve('//server/share/file','../path') should be '//server/share/path' on Windows`
- `test/index.spec.ts::resolve resolve('\\\\server\\share\\file') should be '//server/share/file'`
- `test/index.spec.ts::resolve resolve('\\\\server\\share\\file') should be '//server/share/file' on Windows`
- `test/index.spec.ts::resolve resolve('\\\\server\\share\\file','..\\path') should be '//server/share/path'`
- `test/index.spec.ts::resolve resolve('\\\\server\\share\\file','..\\path') should be '//server/share/path' on Windows`
- `test/index.spec.ts::resolve resolve('\\\\server\\share\\file','sub') should be '//server/share/file/sub'`
- `test/index.spec.ts::resolve resolve('\\\\server\\share\\file','sub') should be '//server/share/file/sub' on Windows`
- `test/index.spec.ts::resolve resolve('\\\\unc-server-name\\unc-path') should be '//unc-server-name/unc-path'`
- `test/index.spec.ts::resolve resolve('\\\\unc-server-name\\unc-path') should be '//unc-server-name/unc-path' on Windows`

Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.

This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.
