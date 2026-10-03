# ToolZip

Tool schemas repeat JSON Schema boilerplate on every model request. ToolZip
compiles a supported subset into readable function signatures, then decodes and
validates JSON calls without guessing values. This is the P1 Compact Tool Schemas
contribution. Provider integration is deliberately not installed: existing router
request/response behavior is unchanged. The router consumes this crate only as a
development dependency for examples.

## SCOPE → ZIP → GUARD

- **ToolScope**, optional: retain strong lexical matches, or keep the full set.
- **SchemaGuard preflight**: canonicalize supported schemas using ordered maps.
  One unsupported selected schema causes whole-request native fallback.
- **ToolZip**: render canonical schemas and the `<<call NAME JSON_OBJECT>>` syntax.
- **SchemaGuard after decoding**: reject unknown tools, malformed calls, missing
  fields, wrong types, enum violations and forbidden extra keys.

The library is pure and deterministic. It reads no files, environment variables,
clock, API keys or network. Its dependencies are serde, serde_json and thiserror.
It owns its types and never imports router types. Call IDs belong to integration
code, not this library.

## Supported schemas

Object, string, integer, number, boolean, homogeneous arrays, nested objects and
arrays, properties, required fields, descriptions, string format metadata and
type-compatible scalar enums. Integer checks use numeric integrality: `30` and
`30.0` pass; `30.5` and `"30"` fail. Numbers accept integral and floating values.
Numeric enum comparison preserves distinctions between large integral values.

`additionalProperties` absent/true permits unknown keys, including in nested
objects. False forbids them. Missing/null parameters mean strictly zero arguments:
only `{}` is accepted. A present unconstrained `{}` schema is unsupported, rather
than being treated as zero arguments. Descriptions, including empty descriptions,
are preserved. Formats are guidance metadata; V1 does not validate dates or URLs.

## Unsupported schemas and native fallback

The analyzer uses an allowlist. It rejects `$ref`, `$defs`, `definitions`, `oneOf`,
`anyOf`, `allOf`, `not`, `if`, `then`, `else`, `const`, `nullable`, type unions,
`dependentSchemas`, `dependentRequired`, `patternProperties`, `contains`,
`prefixItems`, `unevaluatedProperties`, `unevaluatedItems`, `propertyNames`, and
schema-valued `additionalProperties`. Unimplemented length, numeric, item-count
and pattern constraints also bypass, as do `title`, `default`, `examples`,
`deprecated`, `$comment` and all other unknown keywords/annotations.

Enums with null, arrays, objects, incompatible primitive values or no values are
rejected. Missing array items, malformed properties/required fields, undeclared or
duplicate required entries, duplicate tool names and non-function tool kinds are
rejected. Unknown tool-level AND function-level metadata (including `strict`) is
retained during deserialization and rejected rather than dropped.

Function names match `[A-Za-z_][A-Za-z0-9_.:-]*`. Property names use the same rule
without `:`. Unusual names cause fallback; no names are renamed. Format metadata
on types other than string is unsupported.

`encode_tools` returns a typed error with tool, schema path and keyword where
applicable. `optimize_tools` converts preflight failure into an explicit Native
plan with all original tool indices. The caller must actually send its original
native tools. There is no mixed native/compact representation.

## Model-visible grammar

```text
TOOLS
echo(text:str(Text to echo),mode?:str=public|private)! - Echo text
```

```text
<<call echo {"text":"Build >> deployed","mode":"public"}>>
```

Required properties have no suffix; `?` marks optional properties. Primitives are
`str`, `int`, `num`, `bool`; arrays are `[TYPE]`; nested objects are `{PROPERTIES}`.
`datetime` means string with format `date-time`. Other formats use
`str<"JSON-escaped format">`. `!` following an object or a function's parameters
forbids extra keys; its absence permits them.

Enums carry their primitive type: `str=public|private`, `int=1|2`, `num=1|2.5`,
`bool=true|false`. Single-value enums also retain the type. String enum members
use safe bare words or JSON quoting. This prevents losing the distinction between
an integer and a number enum. Schema descriptions use `(description)`; tool
descriptions follow ` - `. Nonempty text without parentheses, quotes, backslashes
or control characters is written verbatim. All other text uses a JSON string,
including empty descriptions: `str("")` or `str("text with (parentheses)")`.
Array/object/root-schema descriptions also survive. Leading/trailing whitespace,
Unicode, punctuation and every description word are preserved exactly.

The rendered schema keeps its `TOOLS` header. The evaluator supplies the call
syntax once in the surrounding system instruction, together with the fixed
reference date/timezone and ordinary-answer behavior. Notation explanations are
included only when the canonical schemas use optional fields, closed objects,
enums or descriptions. `decode_tools` also accepts the previous
`#"description"` annotations and trailing `CALL` instruction for compatibility.

`ping()` means absent/null parameters and only accepts `{}`. `ping(...)` is an
explicit empty open object; `ping()!` is an explicit empty closed object. These
extensions disambiguate meanings omitted by the draft shorthand. Arguments always
remain JSON; there is no compressed argument language.

`decode_tools` parses **the rendered text** and reconstructs schemas. No original
schema sidecar is stored. Compare `analyze_tools(original)` with
`analyze_tools(decode_tools(encode_tools(original)))`, rather than JSON key order.

## API

```rust
use nasiko_tool_compact::{ToolDef, FunctionDef, encode_tools, decode_calls,
    decode_tools, analyze_tools, StreamDecoder};
use serde_json::json;

# fn main() -> nasiko_tool_compact::Result<()> {
let tools = vec![ToolDef {
    kind: "function".into(),
    function: FunctionDef {
        name: "echo".into(), description: Some("Echo text".into()),
        parameters: Some(json!({"type":"object","required":["text"],
            "properties":{"text":{"type":"string"}},
            "additionalProperties":false})),
        extra: Default::default(),
    },
    extra: Default::default(),
}];
let compact = encode_tools(&tools)?;
assert_eq!(analyze_tools(&tools)?, analyze_tools(&decode_tools(&compact)?)?);
let calls = decode_calls("<<call echo {\"text\":\"hello\"}>>", &tools)?;
assert_eq!(calls.len(), 1);

let mut decoder = StreamDecoder::new(&tools)?;
decoder.push("<<ca")?;
decoder.push("ll echo {\"text\":\"hello >> world\"}>")?;
decoder.push(">")?;
let complete_response = decoder.finish()?;
assert_eq!(complete_response.len(), 1);
# Ok(())
# }
```

Other APIs: `render_calls`, `validate_call`, `select_tools(ScopeInput)` and
`optimize_tools(OptimizationContext)`. Policy outcomes expose input/selected
counts, compacted/bypassed counts, plan, confidence and typed bypass reasons.
Token counts are intentionally absent from the pure library.

## Streaming and fail-closed behavior

Batch decoding calls the same StreamDecoder implementation. Calls can be surrounded
by plain text, and multiple calls retain order. Plain answers return an empty vector.
The parser tracks marker prefixes, name boundaries, JSON nesting, string escapes
and closing markers. `>>` or braces inside a string do not terminate a call.
Duplicate JSON object keys are rejected, including in nested values.
Numeric literals that would round to a different decimal value in serde_json
also fail closed, so a large fraction cannot become a valid integer.

**Stage `push()` results; do not execute them yet.** They are validated but
provisional. A later malformed call invalidates the entire response. `finish()`
consumes the decoder and returns all calls only on whole-response success. Errors
are sticky; a poisoned stream cannot be recovered by adding valid text.

Limits: at most 4096 tools, 4096 properties per object and 4096 calls; schema and
JSON nesting are bounded to 64 levels; one call is limited to 1 MiB and a response
or reconstructed compact document to 16 MiB. A trailing partial marker is treated
conservatively as incomplete. Chunks must be valid Rust UTF-8 strings; split a
transport byte stream into valid UTF-8 before calling `push`.

## ToolScope

Normalization handles snake_case, kebab-case, camelCase, PascalCase and acronym
boundaries, then uses distinct alphanumeric tokens and a fixed stop-word list.
Scores: exact normalized full-name occurrence +24; each name token +8; each
required-property token +4; description token +2; optional-property token +1.

Eight tools or fewer always keep all. No score >=8 keeps all. Otherwise retain
every score >=8 plus at most one highest-scoring candidate in 4..7 (original-index
tie break). If the selection exceeds max(12, ceil(40% of tools)), keep all. Returned
indices preserve original order. Confidence is `min(1,max_score/24)`, a reporting
heuristic, not a calibrated probability or a guarantee of recall.

ToolScope is lexical and can miss paraphrased intent. It is optional and **never
called by the official evaluator**, even if environment variables request it.

## Evaluation and measured evidence

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
EVAL_SET=/tmp/compact-tools-eval.json \
  cargo run -p nasiko-llm-router --example compact_tools_tokens
cargo run -p nasiko-llm-router --example toolzip_demo
```

PowerShell: set `$env:EVAL_SET` and `$env:OUT`, then run the same Cargo command.
No CLI arguments are required. Offline evaluation does no HTTP or provider calls.
Decoder chunks are processed one at a time. Errors use `unknown_tool` and
`invalid_arguments`. Compact bodies preserve messages, add one instruction after
leading system messages, and omit native tools. Unsupported schemas, non-auto
tool choices, tool history and response-format constraints keep native requests.
Requests forbidding parallel tool calls also use native fallback.
Native and compact measurements share request construction, preserving case
options such as `max_tokens` and `top_p`, including all native fallback fields.
The evaluator uses the fixed challenge date `2026-10-02`, `Asia/Kolkata`, never
the machine clock.

Public sample: 3/3 expected call round trips and 5/5 decoder cases matched; two
offline release runs were byte-identical. Pinned `tiktoken-rs=0.12.1/o200k_base`
measures serialized **complete request bodies**, including instruction overhead:
656 native tokens versus 451 compact tokens, **31.25% reduction**. Per case:
253→164, 262→173 and 141→114. This exceeds the approximate 30% target on the small
public sample; it does not establish savings on unseen datasets. No descriptions
were summarized or removed. All public schemas retain equal canonical semantics.

Separate synthetic demo: 48 candidate tools, 2 retained for the explicit multi-tool
query; no-signal and ambiguous queries retain all 48. Native 5345 tokens; ZIP-only
2365 (**55.75% reduction**); SCOPE+ZIP 157 (**97.06% reduction**). These synthetic
selection savings are not official P1 scores or evidence of general relevance.

Live mode requires both `PROVIDER_BASE_URL` and `MODEL`; the base should be an
OpenAI-compatible `/v1` URL (a full `/chat/completions` URL also works). Optional
`PROVIDER_API_KEY` or `OPENAI_API_KEY` supplies bearer auth. Requests use temperature
0 and a 60-second timeout. Local mock-provider tests pass. Real-model adherence
not measured locally; organizer live evaluation remains authoritative. The
available local OpenAI credential returned HTTP 401 during model discovery, so
no real-model cases ran. Raw text and decoded calls/errors are reported. Native fallback
schemas outside this library's subset cannot be validated here; their round-trip
or live validation output is an explicit `invalid_arguments` error rather than a
claimed successful reconstruction. Such requests still preserve their native
tools for the provider; no unsafe compact call is accepted.

## Checks and limitations

```sh
cargo fmt --all -- --check
cargo check -p nasiko-tool-compact
cargo test -p nasiko-tool-compact
cargo check -p nasiko-llm-router --example compact_tools_eval
cargo test -p nasiko-llm-router --example compact_tools_eval
cargo test -p nasiko-llm-router
cargo check --workspace
cargo clippy --workspace
cargo clippy -p nasiko-tool-compact --all-targets -- -D warnings
cargo clippy -p nasiko-llm-router --all-targets -- -D warnings
```

V1 is a tested compiler/decoder and evaluator, not a provider-integrated production
gateway. No router flags or provider paths were changed. Provider streaming,
history translation, forced-choice compaction and full JSON Schema support are
not implemented. Description escaping protects grammar structure, not malicious
natural-language prompt injection. JSON numbers use serde_json's numeric model.
The baseline workspace has a Windows-only unused CLI `Stdio` import and an existing
`proc-macro-error2` future-incompatibility warning; ToolZip and router target checks
introduce no new warnings.
