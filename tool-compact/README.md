# Compact tools (ct1)

A pure Rust library for reversible tool schemas and validated model calls. No I/O,
environment access, provider dependencies, or router dependencies.

## Public API

- encode_tools / decode_tools: original ToolDef schemas to/from CompactTools.definitions.
- CompactTools::catalog: model-facing signatures without the ct1 storage version line.
- render_calls: validated expected calls to compact text, for offline evaluation.
- decode_calls / decode_response: compact text to ToolCall objects and preserved ordinary text.
- StreamDecoder: new(tools), push(chunk), finish() or finish_response().

The host assigns standard call IDs and serializes arguments as the OpenAI JSON string.
All calls are withheld until the whole response validates. Any error rejects the
entire response; earlier valid calls are never returned as a partial success.

## Grammar

Definitions start with the literal version line ct1. Each tool occupies one line:

    NAME(PROPERTY_LIST)[!] [SCHEMA_MODIFIERS] [ - JSON_STRING_DESCRIPTION ] newline
    NAME:obj[!] [SCHEMA_MODIFIERS] [ - JSON_STRING_DESCRIPTION ] newline

Schema grammar:

    SCHEMA = BASE [ = JSON_ARRAY_ENUM ] [ # JSON_STRING_DESCRIPTION ] [ @ JSON_OBJECT_CONSTRAINTS ]
    BASE = str [ / date | / date-time ] | int | num | bool | null | obj [ ! ] | array
         | [ SCHEMA ]
         | { PROPERTY_LIST } [ ! ]
    PROPERTY = IDENTIFIER_OR_JSON_STRING [ ? ] : SCHEMA

Bare identifiers contain ASCII letters, digits, underscore, hyphen, or dot. Other
property names use JSON strings. An absent ? means required. The trailing ! records
the presence of the original required keyword, distinguishing omitted required from
required: []. Required properties are emitted in original required-array order,
then optional properties in deterministic key order. The encoder preserves descriptions,
enum order, schema annotations, and explicit versus absent properties/items.
There is no hidden original-schema sidecar in CompactTools.

Example:

    ct1
    calendar(title:str#"Event title",start:str/date-time,visibility?:str=["public","private"])! - "Create an event."

Model call grammar:

    CALL = "<<" [ "call " ] NAME ASCII_WHITESPACE? JSON_OBJECT ASCII_WHITESPACE? ">>"
         | "[TOOL_CALLS]" NAME ASCII_WHITESPACE? JSON_OBJECT
         | "[TOOL_CALLS]" NAME "(" ASCII_WHITESPACE? JSON_OBJECT ASCII_WHITESPACE? ")"

Multiple calls may be separated by any ordinary text. Ordinary text before, between,
and after calls is preserved. The prefixes << and [TOOL_CALLS] are reserved for calls, including in
code fences; do not include a literal call example in an ordinary answer.
The decoder tracks JSON nesting, strings, and backslash escapes, so >> and <<call
inside string arguments are data, as is [TOOL_CALLS]. The native textual aliases
are explicit grammar forms observed from Mistral; argument JSON must still be valid.
Stray closing delimiters after a native frame are rejected. Key=value arguments
and non-JSON function-call syntax are unsupported; the parser never repairs them. Chunks may split anywhere at valid UTF-8 boundaries,
including within names, escaped sequences, or the closing marker.
Truncated markers, JSON, and delimiters are errors, not plain answers.

## Supported schema subset

- Explicit single type: object, array, string, integer, number, boolean, null.
- Object: properties, required, additionalProperties (boolean or supported schema),
  minProperties, maxProperties.
- Array: items (one supported schema), minItems, maxItems, uniqueItems.
- String: minLength, maxLength, format: date or date-time.
- Number/integer: minimum, maximum, exclusiveMinimum, exclusiveMaximum.
- All types: enum, const, description, title, default, examples, deprecated,
  readOnly, writeOnly. Defaults are annotations; they are never inserted into calls.

Unknown keywords, references (including remote references), boolean schemas,
type unions, allOf/anyOf/oneOf, pattern, multipleOf, tuple arrays, and unsupported
formats cause unsupported_schema. The router bypasses compaction for the complete
request. Required names must reference distinct declared properties. This conservative
subset avoids silently weakening any constraint.

Numeric bounds use exact integer comparisons where possible. Mixed floating-point
comparisons beyond the exact f64 integer range are rejected rather than rounded.
JSON Schema numeric equality treats 1 and 1.0 identically for enum/const/uniqueItems.

Limits: 1 MiB input/definition text, 32 JSON nesting levels, 128 tools, 64 calls,
and 4096 array/enum/annotation-array items. Calls above limits are rejected. UTF-8
strings are counted by Unicode scalar values for minLength/maxLength.

Errors have stable codes: unsupported_schema, invalid_tools, unknown_tool,
invalid_arguments, malformed_output, limit_exceeded. Duplicate JSON argument keys
are invalid_arguments; no type coercion, JSON repair, or guessed values are allowed.

## Tests

    cargo test -p nasiko-tool-compact

Unit and property tests cover schema reversibility, escaping, arbitrary stream
partitions, malformed output, limits, nested validation, numeric precision, and
atomic failure after a valid call.
