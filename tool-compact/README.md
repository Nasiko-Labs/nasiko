# nasiko-tool-compact

Pure Rust codec for lossless compact tool definitions and validated streaming tool calls. The
crate performs no I/O, reads no environment variables, and never invents router call ids.

## Definition grammar

The top level is minified JSON. Each tool is a three-slot array:

```text
tools       = [tool, ...]
tool        = [name, description-or-null, schema-or-null]
```

Strings use normal JSON escaping. Tool and argument names are never aliased. Within schema
objects, common JSON Schema keys and primitive type names use one shared legend:

```text
t type                 p properties          r required
i items                e enum                d description
f format               a additionalProperties

o object   a array   s string   i integer   n number   b boolean   0 null
```

Other schema keys are encoded as `~` followed by their original name. Structural schema keywords
(`properties`, `items`, compositions, conditional schemas, and related object/array applicators)
are transformed recursively. Property names, enum values, defaults, examples, and other instance
values remain ordinary JSON. `decode_tools(encode_tools(tools))` reconstructs every supported
definition exactly, apart from JSON object key order, which is not semantically significant.

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

