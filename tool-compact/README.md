# nasiko-tool-compact

`nasiko-tool-compact` is a pure, deterministic codec for rendering OpenAI-style
function schemas compactly and recovering validated calls from model text. It has
no IO, environment reads, router dependency, or provider dependency.

## Grammar

The encoder sends a compact tool inventory followed by this instruction:

```text
To call a tool emit exactly: <<call tool_name {"json":"arguments"}>>
```

Calls use this grammar:

```text
call := "<<call" hws name hws json-object ws ">>"
hws  := one or more whitespace characters
ws   := zero or more whitespace characters
```

The decoder finds the JSON object with a JSON parser rather than scanning for
`>>`; therefore `>>` inside a JSON string is safe. Text before, between, and
after calls is ignored. No marker means a plain answer and decodes to no calls.

Every recovered call is checked against the original schema. Unknown tools,
malformed markers, missing required properties, invalid types, invalid enum
values, and forbidden additional properties return an error; they never become a
best-effort call. `StreamDecoder` buffers chunks and performs that same parse at
`finish`, so split markers are handled identically to non-streaming text.

Supported schema subset: `type`, `description`, `format`, `enum`, `properties`,
`required`, `items`, and boolean `additionalProperties`, including nested object
and array schemas. Combinators, references, numeric/string bounds, pattern
constraints, non-boolean `additionalProperties`, tuple arrays, nullable union
types, and unknown validation keywords are deliberately rejected by `encode_tools`.
Callers should bypass compaction when that happens.
