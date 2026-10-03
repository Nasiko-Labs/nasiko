# nasiko-tool-compact (CTP/1)

Deterministic compact tool definition codec and streaming parser for LLM tool calling.

## Purpose and Design

`nasiko-tool-compact` implements a deterministic translation layer between native JSON Schema tool definitions and a compact, token-efficient human-readable schema representation:

- **Compact Schema Protocol v1 (`ctp/1`)**: Encodes supported tool definitions into compact signatures with original names and descriptions preserved.
- **RFC 8259 Compliant Call Format**: Decodes `<<call NAME {JSON}>>` responses from LLMs.
- **Lossless Reconstruction**: `decode_tools()` reconstructs standard JSON Schemas from compact text alone without out-of-band state.
- **Strict Parsing & Limits**: Rejects duplicate JSON object keys (including unicode escapes like `"a"` vs `"\u0061"`), enforces max nesting depth, argument sizes, and calls per response.
- **Streaming & Atomic Release**: Accepts arbitrary byte chunks (splitting multibyte UTF-8, escape sequences, or call markers) and holds calls until the entire assistant turn validates completely.
