# Grammar and schema support

## Call format (what the model writes)

```
call        = "<<call" SP name SP json-object ">>"
name        = [A-Za-z_][A-Za-z0-9_.-]*
json-object = one RFC 8259 object; no duplicate keys; nothing after it except ">>"
```

- `SP` is exactly one space. `>>` follows the closing `}` immediately.
- Strings may contain `>>`, `}`, escapes and Unicode; `>>` inside a string never closes the call.
- Text before, between and after calls is ignored (surrounding prose is supported).
- A reply with no call is a plain answer: zero calls, not an error.
- A partial opener at the very end of the text (fewer characters than `<<call `, for example `<<ca`) is plain text.
- Maximum arguments size: 64 KiB. Maximum name length: 256 bytes.

## Tolerance: deliberately narrow

Nothing is repaired or guessed. These are rejected (`invalid_arguments`), not fixed:
a missing or extra `}`, a missing `>` of the closer, whitespace before `>>`, a newline or tab or second space after the name,
trailing commas, single quotes, unquoted keys, duplicate keys, non-object arguments, truncated calls, wrong types, missing
required fields, enum violations. An unknown tool name is `unknown_tool`. Decoding is all or nothing: one bad call fails the whole text.

## Tool signature (what the model reads)

```
name(req:T, opt?:T, ...) - tool description
  prop: description of a property
  obj.field: description of a nested property
  list[].field: description of a property inside an array of objects
```

Properties: required ones first in `required` order, then optional ones alphabetically (deterministic). `?` marks optional.

| JSON Schema | Compact | Round trip through `decode_tools` |
|---|---|---|
| `string` | `str` | yes |
| `string` + `format: date-time` | `datetime` | yes |
| `integer`, `number`, `boolean` | `int`, `num`, `bool` | yes |
| `enum` of simple strings (`[A-Za-z0-9_.:/+-]+`) | `a\|b\|c` | yes |
| `array` + `items` | `[T]` | yes |
| nested `object` with `properties` | `{k:T,k2?:T}` | yes |
| `required` | no `?` | yes |
| property `description` (single line) | `path: text` note lines | yes, at any depth |
| tool `description` (single line) | ` - text` | yes |

## Unsupported: the tool is bypassed with a reason code

`compacted:false` plus `bypass:[{tool, reason}]` in the eval output. Reason is `unsupported:<keyword>` for any schema keyword outside the table:
`$ref`, `$defs`, `oneOf`, `anyOf`, `allOf`, `const`, `pattern`, `minimum`, `maximum`, `minLength`, `maxLength`, `additionalProperties`,
`default`, `title`, union `type` lists, and so on. Also: `unsupported:enum`/`enum-value` (non-string enums, values outside the simple charset, a lone value spelled like a type word),
`unsupported:property-name` (names outside `[A-Za-z_][A-Za-z0-9_-]*`), `unsupported:description-newline`, `unsupported:items`, `unsupported:properties`, `unsupported:required`.

**Bypass scope is the whole request.** If any requested tool is bypassed, every tool is sent natively; native and compact tools are never mixed, because model behaviour on a mixed request is unknown.

The eval also sends the request natively when compaction would not reduce the request's token count: `bypass:[{tool:"*", reason:"no_saving"}]`. Compaction is never knowingly larger than native.

## What validation enforces (decode side, against the original schema)

`type` (including type lists), `properties`, `required`, `enum`, `items`, `format: date-time` (RFC 3339), `additionalProperties: false`.
Integers must be written as integers (`30`, not `30.0`). Other keywords are not enforced; tools that use them are bypassed by the encoder.
