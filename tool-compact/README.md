# nasiko-tool-compact

Compact tool definition format and decoder for LLM tool calling token reduction.

## Overview

Tool definitions cost prompt tokens. `nasiko-tool-compact` provides:
1. **Schema IR & Encoder**: Compacts OpenAI-shaped `ToolDef` schemas into a single-line notation per tool plus concise format instructions.
2. **Stream Decoder & Validator**: Parses streamed response chunks, recognizes `<<call name {json}>>` markers via a JSON-aware state machine, and validates decoded arguments against the original schema.
3. **Fail-Closed Guarantee**: Returns typed errors (`unknown_tool`, `invalid_arguments`) rather than guessing or silently mutating call payloads.
4. **Never-Worse Guarantee**: Compares prompt token sizes and automatically bypasses compaction when compact definitions do not yield meaningful savings (≥10% threshold).

## Measured Results (tiktoken-rs `o200k_base`)

| Metric / Dataset | Baseline Tokens | Compact Tokens | Reduction (%) |
|---|---|---|---|
| **2-Tool Set** | 184 | 159 | **13.59%** |
| **5-Tool Set** | 341 | 208 | **39.00%** |
| **Overall Dataset** | 525 | 367 | **30.10%** |

*Determinism*: 100% byte-identical output across repeated eval runs (`FC: no differences encountered`).

## Format Grammar (EBNF)

```ebnf
compact_block  = {tool_line LF} instructions LF example
tool_line      = name "(" params ")" [" - " description]
params         = param {"," param}
param          = field_name ["?"] ":" type
type           = "str" | "int" | "num" | "bool" | "datetime" | "date"
               | "[" type "]"                    (* array *)
               | enum_list                       (* string enum *)
               | "{" params "}"                  (* nested object *)
enum_list      = value {"|" value}
instructions   = 'To call a tool: <<call name {"arg":"val"}>>'
example        = "Example: <<call " name " " json_args ">>"
```

## Supported Schema Subset & Bypass List

### Supported
- Primitive types (`string`, `integer`, `number`, `boolean`)
- Formats: `date-time`, `date`
- String enums (clean alphanumeric values without quotes, whitespace, or `|`)
- Arrays with typed items
- Nested objects with required/optional properties

### Unsupported (Bypassed to Native JSON)
- Composition: `$ref`, `oneOf`, `anyOf`, `allOf`, `not`, `if/then/else`
- Complex properties: `patternProperties`, non-boolean `additionalProperties`
- Format-less constraint keywords: `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `minLength`, `maxLength`, `pattern`, `default`, `const`, `multipleOf`, `uniqueItems`, `minItems`, `maxItems`, `minProperties`, `maxProperties`
- Non-string enums or enums with special characters

## Known Limits & Failure Modes

1. **Small Toolsets**: For small toolsets (1-2 tools with minimal fields), fixed instruction overhead yields modest savings (~10-15%). Savings scale steeply to **39%+** on larger toolsets (5+ tools).
2. **Schema Bypasses**: Complex schemas utilizing unsupported constraint keywords automatically bypass compaction (preserved in native OpenAI JSON format) to preserve 100% schema fidelity.

## Running Evals

To evaluate token savings and decoder accuracy on datasets:

```bash
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

### Live LLM Mode

Optionally connect to an OpenAI-compatible endpoint:

```bash
PROVIDER_BASE_URL=https://api.openai.com/v1 MODEL=gpt-4o EVAL_SET=/tmp/eval.json OUT=/tmp/out.jsonl cargo run --release -p nasiko-llm-router --example compact_tools_eval
```
