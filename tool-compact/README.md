# Compact function tools

`nasiko-tool-compact` is a pure Rust library. It does not read files, environment
variables, provider credentials, or network data. The router converts its own tool
types at the boundary.

## Declaration and call grammar

```text
create_event(title!:str~"Event title",start!:str@date-time,visibility?:str{"public","private"})~"Create an event."
<<call create_event {"title":"Review","start":"2026-10-05T15:00:00+05:30"}>>
```

`!` and `?` mark required and optional fields. Types are `str`, `int`, `num`,
`bool`, `null`, `[type]`, and `obj{fields}`. `obj!{fields}` retains
`additionalProperties: false`; `obj+{fields}` retains explicit `true`.
`|null` retains a nullable type. `@format` retains a string format hint.
`type{value,value}` retains an enum with JSON values. Descriptions use
`~"JSON-escaped text"`; a root parameter description uses `^"text"`.
The renderer normalizes description whitespace but preserves its words.
Calls always carry a JSON object. The parser tracks JSON strings, escapes, and
object depth before accepting the final `>>`, so `>>` inside a string is safe.

`encode_tools` rejects schemas it cannot express without losing meaning.
`decode_tools` reconstructs supported definitions by parsing the compact text,
without a hidden schema copy. `decode_calls` and `StreamDecoder` validate
arguments against the original JSON Schema. Unknown names, malformed calls,
missing required fields, type errors, enum errors, and forbidden extra fields
return errors instead of guessed calls.

The supported JSON Schema subset is object properties and required fields,
primitive types, arrays with `items`, nested objects, enums, descriptions,
string format hints, boolean `additionalProperties`, and a two-member nullable
type union. Validation treats format as an annotation, not a format assertion.
Keywords such as `$ref`, `oneOf`, `anyOf`, `allOf`, conditional constraints,
`patternProperties`, and numeric/string length constraints cause native-tool
bypass in the router. Non-function tools and forced tool choices also bypass.

The stream decoder accepts arbitrary chunk boundaries. A completed marker is
emitted only after its name and arguments validate. A truncated marker at
`finish()` is an error.
