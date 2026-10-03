# Compact tool grammar

`encode_tools` writes this text. `decode_tools` reads it back. Nothing in the trailer names a real tool.

```
compact     = [tool_line LF]* emit LF example
emit        = "Emit <<call name {json}>>."
example     = "Example: <<call do_thing {\"k\":\"v\"}>>"
tool_line   = token [ "(" fields ")" ] [ " - " description ]
fields      = field *( ", " field )
field       = token [ "?" ] ":" type
type        = "str" | "int" | "number" | "bool" | "datetime"
            | "[" type "]"
            | "{" fields "}"
            | enum
enum        = enum_atom *( "|" enum_atom )
```

`LF` is `\n`. There is no trailing newline after `example`. No other whitespace appears, except inside `description` and inside quoted tokens.

`(...)` is present only when the tool has a parameters schema. No parameters omits the parentheses. An object schema with no fields is `name()`.

`?` marks a property that is not in `required`. Properties are sorted by name. Enum values keep their original order.

## Types

| Compact | JSON Schema |
| --- | --- |
| `str` | `{"type":"string"}` |
| `int` | `{"type":"integer"}` |
| `number` | `{"type":"number"}` |
| `bool` | `{"type":"boolean"}` |
| `datetime` | `{"type":"string","format":"date-time"}` |
| `[type]` | `{"type":"array","items": type}` |
| `{fields}` | `{"type":"object","properties":...,"required":[...]}` |
| `a\|b` | `{"type":"string","enum":["a","b"]}` |

A one-value enum whose text is a type keyword is quoted (`"str"`), so it is not read as the `str` type. In a longer enum, a keyword stays bare (`public|str`).

## Tokens

Tool names, field names and enum values use one token form.

- Bare: `^[A-Za-z_][A-Za-z0-9_]*$`
- Otherwise a JSON string, the bytes `serde_json` emits for that string (`"`, `\`, and control characters escaped).

```
say "hi"     ->  "say \"hi\""
a b          ->  "a b"
a|b          ->  "a|b"
c\d          ->  "c\\d"
```

## Descriptions

The tool `description` is the only description kept. Runs of whitespace become one space, then each `\` is written as `\\`. Field `description` values are omitted. `decode_tools` restores the collapsed tool description and does not restore field descriptions.

```
"Write  a\nnote."   ->  Write a note.
"path \ tmp"        ->  path \\ tmp
```

## Not encoded

Any schema keyword outside `type`, `properties`, `required`, `items`, `enum`, `format`, `description`, `additionalProperties` is `Error::Unsupported`. That includes `$ref`, `oneOf`, `anyOf`, `allOf`, `patternProperties`, `pattern`, and tuple `items` arrays.

`additionalProperties` on an object:

- `false` or absent: extra keys are `invalid_arguments`. Neither is written in the compact line.
- `true`: extra keys are allowed. Written as `...`.
- a schema this grammar can encode (`str`, `int`, objects, arrays, enums, …): extra keys must match it. Written as `...:type`, for example `...:str`.
- a schema this grammar cannot encode exactly (`pattern`, `$ref`, `oneOf`, …): `Error::Unsupported` at encode time. The caller sends the native tool.

Also unsupported: a non-object parameters schema, a non-string `enum`, an empty `enum`, `format` other than `date-time`, `format` combined with `enum`, a `required` name that is not a property, and duplicate `required` names.

`description` keys are allowed and dropped. They are not an error.

## Calls

Model output is decoded by `decode_calls` / `StreamDecoder`. Text outside a call is ignored. Zero calls is success.

```
call = "<<call" WS token WS json_object [WS] ">>"
```

`WS` is one or more Unicode whitespace characters. `token` is the same bare-or-JSON-string form as a tool name. `json_object` is one JSON object.

The closing `>>` is the two characters after the JSON value, not the first `>>` in the text. Inside the object, strings and nested `{}` / `[]` are tracked: `>>` inside a string does not end the call. A call that starts and never closes is `Error::InvalidArguments`, including when `>` and `>` arrive in different chunks. An unknown name is `Error::UnknownTool`. A known name whose JSON fails the tool schema (missing required field, extra key when `additionalProperties` is false or absent, extra key that fails an `additionalProperties` schema, wrong type, bad enum, bad date-time) is `Error::InvalidArguments`. The arguments string is the JSON object text unchanged.
