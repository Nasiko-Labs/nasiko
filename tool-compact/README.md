# Compact tools

`nasiko-tool-compact` is a pure Rust library for reversible tool definitions and
validated compact tool calls. It does not read configuration, perform I/O, execute
tools, assign call IDs, or depend on the router. There are no model or tokenizer
dependencies. The router owns transport, opt-in policy, IDs, and API conversion.

```rust
use nasiko_tool_compact::{ToolDef, encode_tools, decode_tools, decode_calls};
use serde_json::json;

let tools = vec![ToolDef {
    name: "lookup".into(),
    description: Some("Look up a document by its ID.".into()),
    parameters: Some(json!({
        "type":"object",
        "properties":{"id":{"type":"string"}},
        "required":["id"]
    })),
}];
let compact = encode_tools(&tools)?;
assert_eq!(decode_tools(&compact)?, tools);
let calls = decode_calls(r#"<<call lookup {"id":"doc-123"}>>"#, &tools)?;
assert_eq!(calls[0].arguments["id"], "doc-123");
# Ok::<(), nasiko_tool_compact::CompactError>(())
```

## Design invariants

- All supported schema information, including every description, survives. The
  compact text itself is sufficient for `decode_tools`; there is no hidden copy
  of the original schema.
- Encoding is deterministic. Required properties appear in original `required`
  order, followed by optional properties in sorted order. This preserves array
  order while avoiding duplicate names in a separate required-field list.
- Decoding validates against the original schema. It does not coerce types,
  insert defaults, repair JSON, invent missing arguments, or drop extra fields.
- Calls are released atomically at `finish()`. A valid first call followed by an
  invalid second call returns an error for the whole response.
- A failed stream stays failed. Empty output and ordinary text yield no calls.
- Duplicate JSON keys, including nested keys, are rejected. Last-key-wins parsing
  would silently alter the model's arguments.
- Validators compile once per decoder. Streaming scans each byte once, with
  bounded buffers; one-byte chunks do not repeatedly reparse accumulated JSON.
- Schema errors cause the router to bypass before dispatch. Invalid model output
  causes an error after dispatch. These are different decisions, not a retry loop.

## Grammar, version 1

The generated definition text uses no insignificant whitespace. JSON strings
provide escaping; embedded quotes, line breaks, delimiters, and Unicode are data.

```ebnf
definitions = [ tool, { LF, tool } ];
tool        = name, "(", (schema | "-"), ")", ["@", json-string];
schema      = base, ["=", json-array], ["@", json-string], ["~", json-object]
            | "true" | "false";
base        = "{", [field, {",", field}], "}"
            | "[", schema, "]"
            | "string" | "integer" | "number" | "boolean" | "null"
            | "object" | "array" | "any";
field       = (name | json-string), ["?"], ":", schema;
name        = 1 * (ASCII-letter | ASCII-digit | "_" | "-");
call        = "<<call ", tool-name, whitespace, json-object, [whitespace], ">>";
output      = { text | call };
```

`?` marks optional properties. An unmarked property is required. `{...}` means
an object with `properties`; `[T]` means an array with `items: T`. `any` means no
base `type` constraint; a type union is preserved in the `~` metadata. `=` retains
the original enum array. `@` retains the original description string. `~` retains
remaining JSON Schema keywords verbatim, including numeric bounds, formats,
`additionalProperties`, defaults, and examples. A metadata key may not overwrite
a structural key already represented by the base. `-` means omitted parameters,
which permits only an empty argument object. Boolean schemas cannot have suffixes.

The output protocol reserves `<<` outside a call's JSON string. A malformed or
incomplete reserved marker fails closed. Inside JSON strings, `>>`, `<<call`,
escaped quotes, backslashes, and newlines have their usual JSON meaning. Tool names
are 1–64 ASCII characters from `name`. Text surrounding valid calls is preserved.
`push_bytes` accepts boundaries inside UTF-8 code points; invalid completed UTF-8
is rejected. `push` is a convenience for already decoded provider text deltas.

## Supported schemas

The explicitly supported subset uses Draft 2020-12 validation:

- Boolean schemas; primitive types and type unions.
- `properties`, `required`, `additionalProperties` (boolean or schema).
- `items` (schema), `minItems`, `maxItems`, `uniqueItems`.
- `minProperties`, `maxProperties`, `minLength`, `maxLength`.
- `enum`, `const`, `minimum`, `maximum`, `exclusiveMinimum`,
  `exclusiveMaximum`, `multipleOf`.
- `title`, `description`, `default`, `examples` (preserved; defaults are never applied).
- Validated formats: `date-time`, `date`, `time`, `email`, `hostname`, `ipv4`,
  `ipv6`, `uuid`, `uri`, `uri-reference`.

Omitted `additionalProperties` continues to allow extra properties, as JSON Schema
requires. Omitted required fields are optional, including when a `default` exists.

All other keywords are rejected before compaction: references (including local
`$ref`), explicit `$schema` dialects, composition (`anyOf`/`oneOf`/`allOf`),
conditionals, pattern constraints, tuple schemas, custom formats and custom
annotations. The host retains native definitions for these cases. Patterns and
references need their own complexity/dialect design before enabling them.

`jsonschema` has **all default resolver features disabled**. The support check also
rejects every reference before compilation, so another crate enabling dependency
features cannot induce reference I/O here. Errors expose categories, not argument
values. Dependency pins are local to the new crate to respect the submission's
root-manifest scope; established dependencies inherit workspace versions.

## Limits

Default output limit: 1 MiB; one argument object: 256 KiB; calls: 128; JSON nesting:
64. These can be supplied through `Limits`. Tool catalogs are limited to 256 tools,
1 MiB of serialized definitions, and 32 schema nesting levels. Exceeding any limit
returns an error. The router never emits an incomplete call to reduce buffering.

## Verification

```sh
cargo test -p nasiko-tool-compact
cargo clippy -p nasiko-tool-compact --all-targets -- -D warnings
```

Tests exercise schema reconstruction, every byte split through escaped Unicode
calls, multiple calls, plain answers, type/enum/nested/format violations, duplicate
keys, truncation, permanent error state, and resource limits. Property tests cover
arbitrary strings, property names, descriptions, chunk sizes, and malformed bytes.
