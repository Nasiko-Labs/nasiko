# nasiko-tool-compact

Lossless compact tool schemas with fail-closed decoder and streaming parser for LLM tool calling in Nasiko.

## Grammar (EBNF)

```ebnf
Call           ::= "<<" "call" WS ToolName WS JsonObject WS? ">>"
ToolName       ::= [a-zA-Z0-9_-]+
JsonObject     ::= "{" JsonMembers? "}"
JsonMembers    ::= JsonMember ("," JsonMember)*
JsonMember     ::= String WS? ":" WS? JsonValue
JsonValue      ::= String | Number | JsonObject | JsonArray | "true" | "false" | "null"
JsonArray      ::= "[" (JsonValue ("," JsonValue)*)? "]"
WS             ::= [ \t\r\n]+
String         ::= '"' ([^"\\\x00-\x1f] | Escape)* '"'
Escape         ::= '\' (["\\/bfnrt] | "u" [0-9a-fA-F]{4})
```

## Features

1. **Deterministic Schema Compaction**: Reduces verbose JSON Schema definitions to concise TypeScript-like function signatures while preserving every type, bound, default, format, and disambiguating description.
2. **Fail-Closed Validation**:
   - Unknown tools trigger `CompactError::UnknownTool`.
   - Missing required fields, type mismatches, enum violations, bound/format errors trigger `CompactError::InvalidArguments`.
   - Never coerces or guesses arguments.
3. **Robust JSON-Aware Scanner**: Handles nested braces, escaped quotes, and literal `>>` / `<<` inside string arguments without false terminates.
4. **Streaming Decoder (`StreamDecoder`)**: Incrementally decodes tool calls across arbitrary SSE chunk boundaries and emits non-call text as assistant responses.
5. **Lossless Round-Trip (`decode_tools`)**: Fully reconstructs non-description schema properties back into valid JSON Schema objects.
