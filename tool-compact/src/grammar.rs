//! Compact-tool grammar constants and field rendering helpers.
//!
//! Full prose grammar: `docs/compact-tools.md`.
//!
//! ```text
//! TOOL_LINE  := NAME '(' FIELDS ')' (' - ' DESCRIPTION)?
//! FIELDS     := FIELD (', ' FIELD)*
//! FIELD      := NAME ':' TYPE '!' (WS QUOTED_DESC)?   # required
//!             | NAME '?:' TYPE (WS QUOTED_DESC)?      # optional
//! QUOTED_DESC:= JSON string literal (shortened property description)
//! TYPE       := 'str' | 'int' | 'float' | 'bool' | 'datetime' | 'null'
//!             | 'object' | '[' TYPE ']' | '{' FIELDS '}'
//!             | ENUM_LIT ('|' ENUM_LIT)*
//! CALL       := '<<' NAME WS+ JSON_OBJECT '>>'
//! ```

/// Call-format instructions appended after every compacted tool list.
///
/// Kept minimal — this text is paid on every compacted request. The decoder is
/// state-aware (`>>` inside JSON strings), so the instruction does not restate
/// escaping rules.
pub const CALL_FORMAT: &str = "CALL <<name {json}>>";

/// Opening call marker (immediately followed by the tool name).
pub const CALL_OPEN: &str = "<<";

/// Closing call marker (checked only after a complete JSON object).
pub const CALL_CLOSE: &str = ">>";
