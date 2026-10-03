# nasiko-tool-compact

Pure Rust codec for lossless compact tool definitions and validated streaming tool calls. The
crate performs no I/O, reads no environment variables, and never invents router call ids.

## Definition grammar

Model-visible definitions use familiar function signatures:

```text
tool        = name "(" argument *("," argument) ")" [description]
argument    = name ["?"] ":" schema [description]
description = json-string
schema      = primitive | "[" schema "]" | "{" arguments "}" | constraints
```

`?` marks optional arguments. Required arguments have no marker. A JSON string immediately after
a schema or signature describes that preceding item. Familiar primitive names (`str`,
`int`, `num`, `bool`, and `null`), nested object braces, and array brackets are written directly.
Enums, formats, `additionalProperties`, and other constraints stay inline:

```text
create_event(title:str,start:datetime,visibility?:"public"|"private")
```

Strings and less common constraint objects use normal JSON serialization. Every behavioral
constraint is model-visible. A private, deterministic reconstruction copy is retained in
`CompactTools`; it is never included in the prompt. `decode_tools(encode_tools(tools))`
reconstructs every supported definition exactly, including absent versus empty descriptions and
explicit schema fields.

## Call grammar

```text
call = "<<call " tool-name whitespace json-object ">>"
```

The closing marker is recognized only after the outer argument object closes and outside JSON
strings. Ordinary text may appear before, between, or after calls. A response with no call block
decodes to an empty call list. `StreamDecoder` tracks partial opening markers, JSON delimiters,
strings, escapes, and closing markers across arbitrary chunk boundaries. Calls are returned only
by `finish`; one invalid call rejects the whole batch.

Every call is validated against the original JSON Schema with `jsonschema` in offline mode.
Unknown tools, duplicate argument keys, missing required properties, invalid values, malformed
syntax, and truncated calls are errors. Values are never coerced or repaired.

## Supported and bypassed definitions

- Supported: required and optional object properties; primitive types; enums; nested objects;
  arrays and item schemas; `format`; boolean or schema-valued `additionalProperties`; standard
  numeric, string, array, object, composition, conditional, annotation, and content keywords.
- Preserved: absent versus empty descriptions, tool order, array order, required-array order, and
  enum order.
- Bypassed: `$ref`, `$dynamicRef`, and `$recursiveRef` at any depth; non-function tools; tool or
  function metadata outside the declared OpenAI-compatible fields; duplicate names; names that
  cannot be represented unambiguously by the call grammar; invalid JSON Schemas.

Callers should treat any encoding error as a request-level bypass and send the original native
tools unchanged.
