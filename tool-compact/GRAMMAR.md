# Compact tool grammar

## Prompt block (encoder output)

```
block     = header LF { tool LF } footer
header    = "Tools (? = optional):"
tool      = name "(" [ props ] ")" [ LF "|" text ]
props     = prop { ", " prop }
prop      = ident [ "?" ] ":" type            ; "?" = not in `required`
type      = atom { "|" atom }                 ; a "|" list is an enum: every atom a literal
atom      = "str" | "int" | "num" | "bool"
          | "datetime" | "date" | "email"     ; string + format date-time | date | email
          | "[" type "]"                      ; array
          | "{" [ props ] "}"                 ; nested object
          | literal                           ; JSON string / number / true / false
footer    = "To call tools, write <<call name {\"arg\":value}>> for each call needed (several allowed)."
```

Required props come first (in `required` order), then optional ones, so output is deterministic.
The `|` line is the tool description plus `path: note` entries for argument descriptions that say
more than the argument name (`start: Start time, ISO 8601`); descriptions are whitespace-collapsed.

## Supported schema subset

`type` (string, integer, number, boolean, array, object — one string), `properties`, `required`,
`items`, `enum` (non-empty scalars), `format` (`date-time`, `date`, `email` on strings),
`description`, `title`/`$schema` (ignored), `additionalProperties: false`.

Anything else — `oneOf`/`anyOf`/`allOf`, `$ref`, `pattern`, `minimum`/`maximum`/length limits,
`default`, union types such as `["string","null"]`, free-form objects, property names outside
`[A-Za-z0-9_]`, tool names outside `[A-Za-z0-9_.-]`, nesting deeper than 6 — makes `encode_tools`
return `Error::Bypass`. The caller keeps the native `tools`. `Bypass` is also returned when the
compact text would not be smaller than the native JSON, or when there are no tools. `StreamDecoder::new`
applies the same check, so a decoder is never built for a schema it cannot fully verify.

## Call (model output, decoder input)

```
call = "<<call " name WS+ object WS* ">>"
name = 1*( ALPHA / DIGIT / "_" / "-" / "." )
object = a JSON object (RFC 8259); ends at its own closing "}"
```

* Text outside calls is ignored. No call at all is a normal, empty result.
* The JSON is parsed, not scanned for `>>`, so `>>` or `<<call` inside a string is just data.
* The `arguments` string returned is the model's own text for the object, unmodified.
* A trailing partial marker (`<`, `<<`, … `<<call`) is held back until it is known to be text or a call.

## Validation (fail closed)

Checked against the original schema; any failure returns an error and **no call**:

| Case | Error | Code |
|---|---|---|
| tool name not in the tool list (checked as soon as the name is read) | `UnknownTool` | `unknown_tool` |
| missing required, unknown key, wrong type, non-integer for `integer`, `null`, value not in `enum` (nested too) | `InvalidArguments` | `invalid_arguments` |
| bad JSON, non-object arguments, missing `>>`, name missing, call never terminated | `Malformed` | `invalid_arguments` |

`decode_calls` is all-or-nothing: one bad call fails the whole reply. `StreamDecoder` returns each
call once, as soon as its `>>` arrives and it validates; after any error it stays failed.
`finish()` turns a dangling partial marker into text and an unterminated call into an error.

Not checked: `format` (the value is only known to be a string). Dates are the model's to get right.
