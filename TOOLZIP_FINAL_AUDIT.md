# ToolZip Final Audit

## 1. Executive Status

Submission readiness: **GO**.

Independent source, manifest, test, architecture and evaluator review completed on 2026-10-03 and repeated against the committed audit implementation. Required P1 behavior passes locally. The initial audit fixed an evaluator completeness defect, added two core tests and one evaluator regression, and corrected stale design documentation. The repeated audit reproduced and fixed native live-argument framing injection with one focused regression. No router runtime integration or grammar optimization was added.

This is a local engineering acceptance decision. Real-model adherence is **NOT VERIFIED LOCALLY** and remains an explicit optional/live evaluation limitation.

## 2. Repository

- Repository: `YellankiKaushik/Nasiko-Build-a-thon`; upstream `Nasiko-Labs/nasiko`.
- Local checkout: `Nasiko-Build-a-thon-repo` within the Nasiko Build-a-thon workspace.
- Branch: `compact-tools`.
- Initial audit starting commit: `e63af8490fe96d3d7037830c473393daa1568551`; repeated audit starting commit: `2c651e5158e47d32472639fcbdaf46d856bca185`.
- Final audit changes are in the single child commit containing this report; use `git rev-parse HEAD` for its exact ID. A report cannot embed its own Git commit ID.
- Upstream fetched: `796211c2c3b383086294a4e744d13b937a12aea7`; merge base `70b4e74c169444b9accebbd599d9822d56d66f03`.
- The repeated audit started with 8 local commits and upstream had 1 additional commit. Each audit adds one justified fix commit without rewriting history.
- `git merge-tree --write-tree compact-tools upstream/main` succeeded without conflicts. No merge/rebase was performed.
- Starting and final delivered working tree: CLEAN. Verification covers the source and documentation changes before the audit commit; origin and PR head are checked after the normal push.
- Existing PR: [Nasiko-Labs/nasiko #228](https://github.com/Nasiko-Labs/nasiko/pull/228). Update the existing branch; do not create or merge another PR.

## 3. Requirement Matrix

Paths are relative to the repository. PASS refers to observed implementation/test behavior, not inferred live-model quality.

| Requirement | Implementation location | Test/evidence | Status |
|---|---|---|---|
| Pure library workspace crate | `tool-compact/Cargo.toml`, root manifest | cargo check; normal dependency tree | PASS |
| Owned native ToolDef/ToolCall | `src/types.rs` | types metadata retention | PASS |
| `encode_tools` | `src/lib.rs`, schema analyze/render | encode exact output, unsupported cases | PASS |
| `decode_calls` | `src/lib.rs`, decode stream/parser | batch/stream equivalence, public cases | PASS |
| `StreamDecoder` | `src/decode/stream.rs` | every character split, staged finalization | PASS |
| `decode_tools` / semantic reconstruction | schema parse + canonical AST | roundtrip tests, public AST equality | PASS |
| Required fields | schema analyze/validate | missing required rejection | PASS |
| Optional fields | Property.required + renderer `?` | encode/validation/roundtrip | PASS |
| Primitive types, no coercion | schema validate | integer/number/string/boolean tests | PASS |
| Scalar enums | schema analyze/render/validate | scalar/numeric enum tests | PASS |
| Nested objects | recursive SchemaNode | nested semantic/validation tests | PASS |
| Homogeneous/nested arrays | SchemaKind::Array | recursive arrays and invalid items | PASS |
| Exact descriptions | schema render/parse | hostile text and generated roundtrip | PASS |
| String formats preserved | SchemaNode.format | metadata equality/escaping; not constraint validation | PASS |
| Boolean additionalProperties | schema analyze/validate | absent/true/false tests | PASS |
| Missing/null parameters | CanonicalTool.parameters=None | empty object accepted; other args rejected | PASS |
| Unsupported keyword/annotation rejection | recursive allowlists | unsupported and adversarial tests | PASS |
| Unsupported-schema whole-native fallback | request.rs / report.rs | preserved tools/messages/options and all-original policy | PASS |
| Unknown tool rejection | StreamDecoder + validate_call | public decoder case + adversarial tests | PASS |
| Invalid arguments rejection | strict parser + schema validate | types/enums/nesting/required tests | PASS |
| Opening/name/JSON/closing splits | seven-state StreamDecoder | all valid UTF-8 boundaries, public chunk case | PASS |
| `>>` / braces inside strings | string/escape state | adversarial and Unicode stream tests | PASS |
| Multiple and repeated valid calls | StreamDecoder ordered vector | stream repeated calls; ct-002 | PASS |
| Plain response → no calls | Searching state | stream test; ct-003 | PASS |
| Whole-response atomic finalization | sticky failure + consuming finish | valid followed by malformed tests | PASS |
| Duplicate JSON keys / lossy numbers | decode/parser.rs | adversarial and live/native tests | PASS |
| Parser resource bounds | analyze/parse/stream modules | depth/call-size + new response/count boundary test | PASS |
| Generic dataset resolution | examples/compact_tools/dataset.rs | unseen-name regression; source ID scan empty | PASS |
| One JSONL record per supplied valid case | evaluator orchestration | public 8 records; unseen native-name plus subsequent case | PASS |
| EVAL_SET / OUT contract | evaluator main | fresh release runs to two output files | PASS |
| Stable unknown_tool / invalid_arguments labels | evaluator error_label | public decoder outputs and regression | PASS |
| Offline no-key/no-network default | LiveConfig::from_env + main | MODEL/base unset; two local release runs | PASS |
| Determinism | canonical order + fixed instruction | binary identical output files | PASS |
| Fixed 2026-10-02 Asia/Kolkata reference | request.rs and live.rs | source and request assertions | PASS |
| Full-body o200k_base measurement | tokens.rs + shared request.rs | pinned 0.12.1; 656/451 | PASS |
| Approximate public >=30% savings target | complete-body metric | 31.25% | PASS |
| ToolScope excluded from official evaluator | evaluator/request/token paths | source search and no selection invocation | PASS |
| Router defaults unchanged | no llm-router/src diff; dev dependency only | 379 router tests pass | PASS |
| Optional lexical ToolScope | scope modules | scope/policy tests, synthetic demo | PASS |
| Optional live adapter | live.rs | hermetic HTTP mock + native parsing tests | PASS |
| Native argument framing integrity | live.rs native_calls | injected call text rejected; valid JSON and delimiter strings retained | PASS |
| Real-model adherence | optional provider example | no successful external model run | PARTIAL |
| Production router integration | intentionally skipped stretch | zero production router changes | NOT APPLICABLE |

## 4. Architecture Compliance

**ToolZip: PASS.** Owns no provider request state. Supported native schemas become canonical signatures; inverse parsing operates on visible grammar alone.

**SchemaGuard: PASS.** One supported AST supplies preflight and recursive post-output validation. All unknown schema semantics are rejected rather than dropped. No coercion, repair or inferred parameter values.

**StreamDecoder: PASS.** Shared batch/stream engine tracks strings, escaping and nesting. Outputs from push are provisional; consuming finish commits the whole ordered response. A later error invalidates successful finalization.

**ToolScope: PASS (optional).** Deterministic lexical scores, conservative full-set fallback and original ordering. Explicit pure policy reports Native/CompactAll/SelectAndCompact; uncertainty and schema errors do not fabricate safety. Not invoked by official evaluation.

The canonical AST is publicly exported intentionally for schema equality and request-legend inspection. Internal parser/renderer/state implementation stays private. No API refactor was required.

## 5. Dependency Purity

`cargo tree -p nasiko-tool-compact --edges normal` shows only direct `serde`, `serde_json` and `thiserror` plus their normal support/proc-macro dependencies. No router, reqwest, Tokio network, provider or configuration dependency is reachable through this crate's normal graph.

The router references ToolZip under `[dev-dependencies]` only. The dependency direction is example/router-development code → pure library.

Searching `tool-compact/src` for `std::env`, `std::fs`, `File::`, `OpenOptions`, `reqwest`, `TcpStream`, `UdpSocket`, `tokio::net`, `Command::` and `nasiko_llm_router` found **NONE**. Provider-specific executable code: **NONE**. The word OpenAI appears only in the owned-type documentation (“OpenAI-shaped”).

Cargo.lock diff is 303 lines. Comparing baseline name/version pairs found 21 added and one removed entry, with **no replacements of existing registry package versions**. Additions include the new crate, pinned tokenizer/regex dependencies, five already-declared workspace crates and feature-expanded native TLS/zstd/shellexpand/toml_writer dependencies. Baseline manifests already contained requirements such as CLI libc, zip and toml_edit that its lock graph incompletely reflected. The obsolete workspace nasiko-oidc entry was removed; A2A source spelling gained its manifest's pinned rev while retaining the same commit. This is workspace graph reconciliation plus actual new dev dependencies, not unexplained package upgrades. The lockfile was retained.

## 6. Schema Support

Supported: explicitly typed object/string/integer/number/boolean, homogeneous arrays, nested arrays/objects, properties, required, items, exact descriptions, string-format metadata, scalar enums and boolean additionalProperties.

Absent/true additionalProperties allows and preserves extras. False rejects them. Schema-valued additionalProperties is unsupported. Missing/null parameters means strictly zero arguments: `{}` is valid and arbitrary other argument shapes are invalid.

Integer `30` and `30.0` pass; `30.5` and `"30"` fail. Number accepts integral/floating values. Numeric enum validation avoids collapsing distinct large integers. Formats retain metadata without full format constraint checking.

Unsupported: `$ref/$defs/definitions`, combinators, conditional schemas, const, nullable/unions, contains/prefixItems, patternProperties/dependentSchemas, schema-valued extras, object/array enums, missing types, constraints such as minimum/pattern/default and every other unknown annotation. Duplicate names, invalid identifiers and malformed schemas fail before compaction. Unsupported schemas yield actual whole-native requests in the example/policy; they cannot be validated by the subset decoder.

## 7. Decoder Safety

Evidence: six stream tests, six adversarial tests, four recursive validation tests and public decoder cases.

Seven states cover search, marker separation, tool name, JSON start/body and both ending characters. Matching nesting and string/escape tracking handle JSON-internal delimiter text. Strict parsing rejects duplicate object keys and numeric literal rounding. Unknown functions, invalid enums/types/required fields and malformed/incomplete calls fail the whole response. Repeated VALID calls are allowed.

`push` can return newly completed calls before later failure; these are explicitly provisional. `finish(self)` returns the whole response only after complete success. Caller execution must wait for finish. Sticky failures clear the internal accumulated list and cannot recover on later chunks.

Bounds: schema depth 64, JSON nesting 64, 4096 tools, 4096 properties/object, 1 MiB/call, 16 MiB/response, 4096 calls and 16 MiB grammar input. The added boundary test accepts exactly 4096 calls and rejects the next one; a response at 16 MiB is accepted and one extra byte poisons it. Caller-owned Values and encoder output are not covered by a universal allocation ceiling.

All chunk inputs are valid UTF-8 strings. Every character-boundary split and one-character feeding pass, including Unicode, escaped quote/backslash, nested arrays/objects and `>>` in a string. Byte transport decoding is the caller's job. Partial markers at end conservatively return IncompleteCall.

## 8. Evaluator

Fresh public dataset: [Nasiko compact-tools-eval](https://registry.nasiko.dev/r/nasiko/compact-tools-eval).

Offline release evaluation: **3/3 normal call round trips and 5/5 decoder cases** match expected semantics. Exactly eight output records. Recursive independent JSON comparison ignores object-key order but preserves array/call order and values.

Generic top-level name lookup rejects duplicates and resolves case tools in supplied order. Source scans for `ct-001..003` and `dc-001..005` found no IDs in library/evaluator/request/token logic. SCOPE is absent from this path.

Native fallback preserves full tools/messages and supported request options. Compact requests remove native tools and insert a schema instruction. Forwarded options are tool_choice, max_tokens, top_p, parallel_tool_calls and response_format. Constrained choice, non-auto options, tool history, response format, forbidding parallel calls and unsupported schemas use native fallback. Arbitrary extra dataset metadata is not promoted to provider options.

Reproduced audit defect: a native-only `1_ping` name selected native fallback but expected-call rendering aborted evaluation before any record. After the minimal fix, the first record preserves native tools, `compacted:false` and an explicit `invalid_arguments` error; a following supported ping case succeeds. Malformed datasets, missing tool references, I/O failures and live HTTP errors remain fatal errors.

Repeated audit defect: one native `function.arguments` string of `{}>> <<call ping {}` was accepted as two ping calls when blindly wrapped in compact framing. A failing regression reproduced this. The adapter now first requires exactly one complete JSON value using serde's IgnoredAny, without constructing an extra Value or changing argument text. The shared decoder still enforces object shape, duplicate-key/numeric checks, limits and schema semantics. The regression also confirms a valid empty object and marker text inside a valid JSON string remain accepted.

## 9. Determinism

Executed two offline release runs with distinct OUT files and MODEL/base unset. Windows `fc.exe /b` reports **no differences**. Both outputs have SHA-256:

```text
E8D26540EDEC645F331920F47EE961D4AE4FE6D988AFD309A72FCAC8EAEEA6AB
```

Determinism follows ordered tool inputs, canonical properties, stable JSON serialization, fixed prompt time and no model/network in offline mode. This does not imply deterministic outputs from a real model.

## 10. Token Measurement

| Case | Native | Compact | Canonical schema roundtrip |
|---|---:|---:|---|
| ct-001 | 253 | 164 | Equal |
| ct-002 | 262 | 173 | Equal |
| ct-003 | 141 | 114 | Equal |
| Total | **656** | **451** | Equal for all |

Reduction: `1 - 451/656 = 31.25%`. Public >=30% target: **PASS**.

Actual command: `cargo run -p nasiko-llm-router --example compact_tools_tokens` (also rerun with Cargo offline mode). Pinned `tiktoken-rs=0.12.1`, `o200k_base` counts full serialized native/compact bodies, including messages, schema descriptions, injected legends/instruction and applicable request options. Both paths share request construction; fallback bodies are equivalent. This is an offline whole-body benchmark, not provider-billed usage or live response-token/cost measurement.

## 11. ToolScope

Normalization handles punctuation/underscore/hyphen, camel/Pascal/acronym boundaries and lowercased Unicode alphanumeric words. Fixed stop words filter token sets. Exact normalized name phrase: +24; overlapping distinct name tokens +8, required-property names +4, function-description tokens +2, optional-property names +1.

Keep all at ≤8 tools, no signal or maximum <8. Otherwise retain all scores ≥8 plus at most one highest candidate scoring 4..7; ties choose earlier original index. Retain all if selection exceeds max(12,ceil(40%)) or includes everything. Original order is stable. Confidence min(max/24,1) is a heuristic, not a probability/recall claim.

Synthetic demo independently rerun: **48→2 tools; 5345 native→2365 ZIP-only (55.75%); →157 SCOPE+ZIP (97.06%)**. No-signal and broad record queries each retain 48. Decoder emissions [0,0,0,1]; schema equality and rejection/fallback checks true.

These are favorable custom fixture results, **not official P1 scores**. Official evaluator never calls selection; no tool reduction affects the 656→451 comparison.

## 12. Tests

The complete requested suite was rerun after the code and documentation changes. Windows Rust/cargo 1.99.0 and Visual Studio 2022 C++ Build Tools were used.

| Exact command | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo check -p nasiko-tool-compact` | PASS |
| `cargo test -p nasiko-tool-compact` | PASS: 34 integration tests + 1 doctest |
| `cargo check -p nasiko-llm-router --example compact_tools_eval` | PASS |
| `cargo test -p nasiko-llm-router --example compact_tools_eval` | PASS: 9 tests including hermetic live mock and native framing regression |
| `cargo test -p nasiko-llm-router` | PASS: 379 passed; 1 library and 6 infrastructure tests ignored |
| `cargo check --workspace` | PASS |
| `cargo clippy --workspace` | PASS |
| `cargo clippy -p nasiko-tool-compact --all-targets -- -D warnings` | PASS |
| `cargo clippy -p nasiko-llm-router --all-targets -- -D warnings` | PASS |

Existing baseline Windows CLI Stdio unused-import and proc-macro-error2 future-incompatibility warnings remain on broad workspace checks. They are not introduced by ToolZip. Affected targets pass strict Clippy. No unrelated warning cleanup was performed.

Additional executed checks: two offline release evaluator runs, binary comparison, expected-result comparison, token example, synthetic demo and the unseen native-name two-case reproduction.

| Core test file | Count | Requirement evidence |
|---|---:|---|
| types | 1 | Owned metadata retention |
| encode | 3 | Exact grammar, zero/open/closed distinction, escaping |
| unsupported | 5 | Allowlists, shape/duplicate/depth failures |
| validation | 4 | Recursive types/required/enums/extras, numerics |
| stream | 6 | Every split, string framing, atomicity, incomplete and resource boundaries |
| roundtrip | 4 | Rendered-only reconstruction, exact hostile descriptions, legacy forms |
| adversarial | 6 | Generated schemas/chunks, large/deep/malformed calls, numeric loss |
| scope | 3 | Exact scores, ordering and uncertainty fallback |
| policy | 2 | Explicit opt-in and all-original native fallback |

Logs and datasets are in the OS temporary directory under `toolzip-audit-*` and `toolzip-reaudit-*`, outside the commit.

## 13. Security

**PASS for the audited contribution.** No live credential was found in its changed files or new commit content. A full-HEAD credential-pattern scan found five paths that also match unchanged baseline files: github integration-test documentation, MCP aggregator test fixture, secret-encryption test vector and two UI tests. These are baseline matches, not ToolZip additions; they were not silently counted as a clean whole-repository scan. No real .env, private-key or credential file was added; tracked .env.example files remain baseline placeholders.

Executable provider auth obtains optional keys only from environment in the live example. The request body's fields do not include credentials, no auth header is stored in JSONL, and no key value was printed or embedded in docs. This audit did not make paid provider calls.

Production crate forbids unsafe and denies Clippy unwrap_used, expect_used and panic. Searches found no reachable `unwrap()`, Option/Result `expect()` or `panic!` in model-input production paths. Grammar parser `self.expect` is a custom fallible cursor method returning Result, not the panicking standard method. Test-only unwraps are assertion setup.

Description encoding blocks structural delimiter escape while preserving exact text. Strict parsing, nesting/size/count bounds and atomic finish constrain malformed-output handling. These measures do not replace tool execution authorization, natural-language prompt-injection defenses or caller-level input allocation budgets. A pattern scan/source audit is scoped evidence, not a universal secret-discovery or vulnerability proof.

Native live argument text is validated as one complete JSON value before compact framing, preventing malformed argument text from manufacturing additional validated calls. The guard does not coerce, reserialize or repair text; duplicate-key and numeric protections still run in the shared decoder.

## 14. Git / PR Scope

Reviewed every changed contribution file: manifests/lockfile, all production modules, nine test files, evaluator helpers/examples and documentation. The large architecture copy was read in full and checked against actual code.

At initial audit start: 39 changed files, 7374 insertions, 34 deletions against the upstream merge base. At repeated audit start: 41 files, 8163 insertions, 34 deletions. The follow-up changes stay within the live evaluator example and existing documentation. No production `llm-router/src` diff, no unrelated runtime behavior change, no logs/credentials/temp fixtures in the contribution.

Upstream's extra commit concerns classification tests/export paths; merge-tree found no conflicts. No rebase, force push or merge was needed. Existing PR #228 remains the submission; final normal push and refreshed body record this audit's counts/limitations. Its description distinguishes public and synthetic results and does not claim verified real-model adherence.

## 15. Documentation Consistency

Active README/demo/PR metrics are 656→451 /31.25%, and synthetic 5345→2365→157 /55.75% /97.06%. Searches for obsolete 527/19.66%/48.48%/96.67% found no stale active metric claims. Current grammar uses required `title:str` and optional `title?:str`, not an obsolete required-field marker.

Architecture status and old target grammar/EBNF/checklist/demo are explicitly historical. Corrections align description escaping, mathematically integral numbers, consuming finish and repeated valid calls with actual code. Resolved tokenizer and live auth environment choices are recorded. The implemented appendix, README and walkthrough are the current semantic contract. The large original design still includes illustrative integration sketches and future work; those are not claimed as shipped production behavior.

Public type comments no longer imply that CompactTools contains the call instruction or that manually constructed ToolCall values are already validated. New guides explain current code, exact APIs, tests, limitations and demonstration/judge questions, including ordered Windows demo commands.

## 16. Real-Model Adherence

**NOT VERIFIED LOCALLY.**

**MOCK LIVE ADAPTER: PASS.** A hermetic local HTTP mock verifies configured model, temperature 0, endpoint post, raw output and compact-call reconstruction. Native result tests reject unknown names, duplicate arguments and injected framing while preserving valid arguments. Auth handling and endpoint/environment configuration were inspected in source; the three adapter tests do not establish external-provider compatibility for every environment.

No successful real-provider/model request was performed in this audit. Earlier implementation notes record HTTP 401 from the available credential; that prior failure is not a successful adherence test and was not retried as a paid model run. Offline reconstruction and mocks must not be presented as model compliance.

## 17. Known Limitations

1. Real-model adherence and unseen-dataset token savings are unverified.
2. Production router integration and provider streaming/history translation are skipped.
3. Only an explicit JSON Schema subset is compiled; formats are metadata, unknown keywords fail.
4. Native-only schemas/names remain native but may return explicit compact validation errors.
5. SCOPE is lexical, can miss paraphrases and provides no calibrated confidence/recall guarantee.
6. Public measurement covers three normal cases, not a broad workload; synthetic selection is favorable by construction.
7. Limits protect documented parser paths; encoder output and caller-owned/dataset allocations lack a universal byte budget.
8. A trailing partial marker in plain text is conservatively incomplete.
9. Escaping protects grammar structure, not model obedience to malicious descriptions or tool-execution authorization.
10. Malformed datasets and live I/O/provider failures can stop the evaluator rather than generate success-shaped case output.
11. Calls from push remain provisional; an integration executing them early violates atomic safety.
12. Broad workspace checks retain baseline warnings and seven existing router tests require infrastructure.

## 18. Changes Made During Audit

The initial audit is preserved in commit 2c651e51. This repeated audit adds only the native live-argument fix/regression, current evidence updates and small walkthrough formatting corrections.

- Fixed one reproducible evaluator defect: expected-call rendering failures now become stable per-case validation errors instead of aborting native-fallback evaluation.
- Added the smallest evaluator regression for a native-only identifier.
- Fixed reproduced native live-argument framing injection by requiring complete JSON before adding call markers; added a focused negative/positive regression.
- Added positive legacy hash-annotation/CALL-footer semantic reconstruction coverage.
- Added inclusive 16 MiB response and 4096-call boundaries with sticky rejection above limits.
- Corrected public type comments and stale architecture status/grammar/escaping/integer/finish/duplicate-call/resolved-question text.
- Added this useful acceptance report and the detailed project walkthrough, with diagrams, APIs, modules, demo scripts and 28 judge questions.
- Refreshed existing PR counts and audit notes; no new feature, runtime integration, grammar redesign or unrelated cleanup.

## 19. Remaining Blockers

**NONE** for required local P1 submission acceptance.

Live-model adherence, production integration and ToolScope recall evaluation remain optional limitations and are labeled honestly. They do not invalidate the measured offline/core contract.

## 20. Final Verdict

**GO FOR SUBMISSION.**

The required compiler/decoder/evaluator path passes the complete local verification suite, public 3/3 round trips and 5/5 decoder cases, exact offline determinism, supported-schema reconstruction and the public 31.25% full-body token target. Dependency purity and unchanged production router behavior are demonstrated. The audits repaired concrete per-case completeness and native-argument framing defects without widening scope. Submit the existing PR with the explicit live/model, subset and optional-selection limitations above.
