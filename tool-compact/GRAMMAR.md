# Compact Tool Grammar (v1)

## 1. Tool definition line (encoder output)

    tool      = name "(" [ param { "," param } ] ")" [ " - " description ]
    param     = pname [ "?" ] ":" type
    type      = "str" | "int" | "num" | "bool" | "datetime"
              | "[" type "]"                       (array)
              | enum                               (string enum)
              | "{" [ param { "," param } ] "}"    (nested object)
    enum      = literal "|" literal { "|" literal }

- `?` after a parameter name means optional; no `?` means required.
- `datetime` means a string with `format: date-time`.
- Descriptions are kept when they disambiguate (shortened, never invented).

## 2. Call-format instruction (added once per request)

    To call a tool, emit: <<call name {json args}>>

## 3. Call syntax (model output)

    call = "<<call " name " " json_object ">>"

- `json_object` is one standard JSON object. The decoder reads it with a
  string-aware scan, so `>>` inside a JSON string does not end the call.
- Text before or after calls is allowed. Multiple calls are allowed.
- No `<<call` marker means a plain answer (zero calls, not an error).

## 4. Fail-closed rules (the decoder returns an error, never a guess)

| Condition                         | Error              |
|-----------------------------------|--------------------|
| name not in the tool list         | unknown_tool       |
| missing required field            | invalid_arguments  |
| wrong type / enum violation       | invalid_arguments  |
| unknown extra field               | invalid_arguments  |
| bad JSON, missing `>>`, truncated | malformed_call     |

## 5. Unsupported schema features (compaction is bypassed)

oneOf / anyOf / allOf, $ref, patternProperties, additionalProperties
schemas, and anything else the encoder cannot express losslessly.