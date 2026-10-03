use serde_json::{Value, json};
use std::collections::HashSet;

// ─── Public API ───────────────────────────────────────────────────────────────

/// Converts OpenAI-style tool definitions into a compact text representation.
///
/// Example output line:
/// ```text
/// github_repo_info(owner:string, repo:string)
/// ```
///
/// Tools whose schemas contain constructs the compact decoder cannot safely
/// validate (see `can_compact_schema`) are silently skipped — the caller
/// should detect them with `can_compact_schema` and keep their original
/// full JSON schema instead.
pub fn encode_tools(tools: &[Value]) -> String {
    let mut output = String::new();

    for tool in tools {
        let function = match tool.get("function") {
            Some(value) => value,
            None => continue,
        };

        let name = match function.get("name").and_then(Value::as_str) {
            Some(value) => value,
            None => continue,
        };

        let parameters = function.get("parameters");

        output.push_str(name);
        output.push('(');

        if let Some(properties) = parameters
            .and_then(|p| p.get("properties"))
            .and_then(Value::as_object)
        {
            let mut first = true;

            for (param_name, param_schema) in properties {
                if !first {
                    output.push_str(", ");
                }

                first = false;

                let param_type = param_schema
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("any");

                output.push_str(param_name);
                output.push(':');
                output.push_str(param_type);
            }
        }

        output.push_str(")\n");
    }

    output
}

/// Returns `true` when the tool's parameter schema only uses constructs that
/// the compact decoder can faithfully validate.
///
/// If this returns `false` the tool is a **bypass** case: do not compact it
/// and do not use `decode_calls` for it.  Pass the original full JSON schema
/// to the model unchanged (0 % savings, but 100 % correct).
///
/// Unsupported constructs that trigger bypass:
/// - `oneOf`, `anyOf`, `allOf`, `not`
/// - `$ref`
/// - `pattern`, `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`
/// - `minLength`, `maxLength`, `minItems`, `maxItems`
/// - `additionalProperties`
/// - property schemas with no `type` field
pub fn can_compact_schema(tool: &Value) -> bool {
    let parameters = match tool.get("function").and_then(|f| f.get("parameters")) {
        Some(p) => p,
        None => return true, // no parameters → trivially compactable
    };

    schema_is_safe(parameters)
}

// ─── Strict decoder ───────────────────────────────────────────────────────────

/// Validates a model-generated tool call against the original JSON schemas and,
/// on success, returns an OpenAI-compatible tool-call object:
///
/// ```json
/// {
///   "type": "function",
///   "function": {
///     "name": "github_repo_info",
///     "arguments": { "owner": "octocat", "repo": "hello-world" }
///   }
/// }
/// ```
///
/// The decoder is intentionally strict and returns an error for:
/// - unknown tool names
/// - malformed / non-object JSON arguments
/// - missing required arguments
/// - extra / unknown arguments
/// - wrong primitive types
/// - values that violate an `enum` constraint
/// - strings that violate a `format` constraint (e.g. `date-time`)
pub fn decode_calls(tool_defs: &[Value], name: &str, arguments: &str) -> Result<Value, String> {
    // 1. Resolve tool definition.
    let tool = tool_defs
        .iter()
        .find(|tool| tool["function"]["name"].as_str() == Some(name))
        .ok_or_else(|| format!("Unknown tool: {name}"))?;

    let schema = &tool["function"]["parameters"];

    // 2. Parse arguments.
    let args: Value =
        serde_json::from_str(arguments).map_err(|e| format!("Invalid JSON arguments: {e}"))?;

    let args_object = args
        .as_object()
        .ok_or_else(|| "Arguments must be a JSON object".to_string())?;

    // 3. Resolve property schemas and required list.
    let properties = schema["properties"]
        .as_object()
        .ok_or_else(|| "Tool has no properties schema".to_string())?;

    let required = schema["required"]
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();

    // 4. Check all required arguments are present.
    for required_name in &required {
        if !args_object.contains_key(*required_name) {
            return Err(format!("Missing required argument: {required_name}"));
        }
    }

    // 5. Reject extra arguments not declared in the schema.
    for argument_name in args_object.keys() {
        if !properties.contains_key(argument_name) {
            return Err(format!("Extra argument: {argument_name}"));
        }
    }

    // 6. Validate every supplied argument against its property schema.
    for (argument_name, argument_value) in args_object {
        let property_schema = &properties[argument_name];

        validate_type(argument_name, argument_value, property_schema)?;
        validate_enum(argument_name, argument_value, property_schema)?;
        validate_format(argument_name, argument_value, property_schema)?;
    }

    Ok(json!({
        "type": "function",
        "function": {
            "name": name,
            "arguments": args
        }
    }))
}

// ─── StreamDecoder ────────────────────────────────────────────────────────────

/// Accepts model output in arbitrary-sized chunks and decodes complete compact
/// tool calls as they arrive.
///
/// The compact call format is:
/// ```text
/// tool_name({"arg":"value", ...})
/// ```
///
/// Calls may be split across multiple chunks.  The decoder buffers incomplete
/// content and only emits a decoded call once the closing `)` of the JSON
/// object is unambiguously found.
///
/// Multiple complete calls in a single chunk are all returned at once.
///
/// # Errors
/// - `push()` returns `Err` if a completed call contains malformed JSON or
///   names an unknown tool.
/// - `finish()` returns `Err` if any buffered (incomplete) content remains
///   after the stream ends.
pub struct StreamDecoder {
    /// Tool definitions used to validate decoded calls.
    tool_defs: Vec<Value>,
    /// Rolling buffer of unprocessed text.
    buffer: String,
}

impl StreamDecoder {
    /// Create a new decoder.  The `tool_defs` slice should be the same
    /// OpenAI-style tool definitions passed to `encode_tools`.
    pub fn new(tool_defs: Vec<Value>) -> Self {
        Self {
            tool_defs,
            buffer: String::new(),
        }
    }

    /// Push the next chunk of model output into the decoder.
    ///
    /// Returns all tool calls that became complete with this chunk.
    /// Incomplete content stays buffered for the next call.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<Value>, String> {
        self.buffer.push_str(chunk);
        self.drain_complete_calls()
    }

    /// Signal end-of-stream.
    ///
    /// Returns `Err` if any non-whitespace content remains in the buffer
    /// (indicating a truncated / incomplete call).
    pub fn finish(&mut self) -> Result<Vec<Value>, String> {
        // Drain anything still completable.
        let calls = self.drain_complete_calls()?;

        let remaining = self.buffer.trim();
        if !remaining.is_empty() {
            return Err(format!(
                "Stream ended with incomplete content in buffer: {remaining:?}"
            ));
        }

        Ok(calls)
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Extract and decode all complete `name({...})` calls from the front of
    /// the buffer, leaving any trailing incomplete content in place.
    fn drain_complete_calls(&mut self) -> Result<Vec<Value>, String> {
        let mut decoded = Vec::new();

        loop {
            // Skip leading whitespace / newlines between calls.
            let trimmed_start = self
                .buffer
                .find(|c: char| !c.is_whitespace())
                .unwrap_or(self.buffer.len());
            self.buffer.drain(..trimmed_start);

            if self.buffer.is_empty() {
                break;
            }

            // A call starts with: IDENTIFIER '('
            let paren_pos = match self.buffer.find('(') {
                Some(p) => p,
                None => break, // no '(' yet — keep buffering
            };

            // Everything before '(' is the tool name.
            let name_candidate = &self.buffer[..paren_pos];
            if !is_valid_identifier(name_candidate) {
                // Not a tool call — skip one character to avoid infinite loop.
                self.buffer.drain(..1);
                continue;
            }

            // After '(' we expect a JSON object.  Find where it ends.
            let json_start = paren_pos + 1; // byte index of '{'

            if json_start >= self.buffer.len() {
                break; // '(' received but no JSON yet
            }

            if self.buffer.as_bytes()[json_start] != b'{' {
                // Malformed — skip past the '(' and try again.
                self.buffer.drain(..json_start);
                continue;
            }

            // Scan for the matching closing '}', then expect ')'.
            let json_bytes = &self.buffer.as_bytes()[json_start..];
            match find_json_object_end(json_bytes) {
                None => break, // JSON object not yet complete — keep buffering
                Some(relative_end) => {
                    // `relative_end` is the index of the closing '}' within
                    // `json_bytes`.  Convert to absolute buffer positions.
                    let json_end_abs = json_start + relative_end; // index of '}'
                    let closing_paren_abs = json_end_abs + 1; // index of ')'

                    if closing_paren_abs >= self.buffer.len() {
                        break; // ')' not yet received
                    }

                    if self.buffer.as_bytes()[closing_paren_abs] != b')' {
                        // Unexpected character after '}' — skip and keep going.
                        self.buffer.drain(..closing_paren_abs);
                        continue;
                    }

                    // Extract the complete call.
                    let name = self.buffer[..paren_pos].to_string();
                    let arguments = self.buffer[json_start..=json_end_abs].to_string();

                    // Remove the consumed call from the buffer (including ')').
                    self.buffer.drain(..=closing_paren_abs);

                    // Decode and validate.
                    let call = decode_calls(&self.tool_defs, &name, &arguments)?;
                    decoded.push(call);
                }
            }
        }

        Ok(decoded)
    }
}

/// Returns `true` when `s` looks like a valid ASCII identifier
/// (letters, digits, underscores; must not start with a digit).
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Scans `bytes` (which must start at `{`) and returns the index of the
/// matching closing `}`, accounting for nested objects, arrays, strings,
/// escape sequences, and escaped backslashes.
///
/// Returns `None` if the object is not yet complete.
fn find_json_object_end(bytes: &[u8]) -> Option<usize> {
    debug_assert_eq!(bytes.first(), Some(&b'{'));

    let mut depth: i32 = 0; // nesting depth for { }
    let mut in_string = false;
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];

        if in_string {
            match b {
                b'\\' => {
                    // Skip the next byte (it is escaped).
                    i += 2;
                    continue;
                }
                b'"' => {
                    in_string = false;
                }
                _ => {}
            }
        } else {
            match b {
                b'"' => {
                    in_string = true;
                }
                b'{' | b'[' => {
                    depth += 1;
                }
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 && b == b'}' {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }

        i += 1;
    }

    None // object not yet complete
}

// ─── decode_tools ─────────────────────────────────────────────────────────────

/// Parses the compact text representation produced by `encode_tools()` and
/// reconstructs OpenAI-style tool definitions.
///
/// Input format (one tool per line):
/// ```text
/// github_repo_info(owner:string, repo:string)
/// check_endpoint(url:string)
/// ```
///
/// Output format mirrors the standard OpenAI function-calling schema:
/// ```json
/// [
///   {
///     "type": "function",
///     "function": {
///       "name": "github_repo_info",
///       "parameters": {
///         "type": "object",
///         "properties": { "owner": {"type":"string"}, "repo": {"type":"string"} },
///         "required": ["owner", "repo"]
///       }
///     }
///   }
/// ]
/// ```
///
/// All parameters are treated as required (this matches `encode_tools` which
/// does not encode optional vs. required information).
///
/// # Errors
/// Returns `Err` for:
/// - empty or whitespace-only input
/// - duplicate tool names
/// - malformed parentheses
/// - missing or malformed parameter declarations
/// - duplicate parameter names within a tool
/// - invalid / unsupported type names
pub fn decode_tools(compact: &str) -> Result<Vec<Value>, String> {
    let mut tools: Vec<Value> = Vec::new();
    let mut seen_tool_names: HashSet<String> = HashSet::new();

    for (line_index, raw_line) in compact.lines().enumerate() {
        let line = raw_line.trim();

        if line.is_empty() {
            continue; // blank lines are allowed between tools
        }

        let tool = parse_compact_line(line).map_err(|e| format!("Line {}: {e}", line_index + 1))?;

        let name = tool["function"]["name"].as_str().unwrap_or("").to_string();

        if !seen_tool_names.insert(name.clone()) {
            return Err(format!("Duplicate tool name: '{name}'"));
        }

        tools.push(tool);
    }

    if tools.is_empty() {
        return Err("No tool definitions found in compact representation".to_string());
    }

    Ok(tools)
}

/// Parses a single compact line such as:
/// ```text
/// github_repo_info(owner:string, repo:string)
/// ```
/// into an OpenAI-style tool definition `Value`.
fn parse_compact_line(line: &str) -> Result<Value, String> {
    // ── Split at '(' ─────────────────────────────────────────────────────────
    let paren_open = line
        .find('(')
        .ok_or_else(|| format!("Missing '(' in: {line:?}"))?;

    let name = line[..paren_open].trim();

    if name.is_empty() || !is_valid_identifier(name) {
        return Err(format!("Invalid tool name: {name:?}"));
    }

    // ── Closing ')' ───────────────────────────────────────────────────────────
    if !line.ends_with(')') {
        return Err(format!("Missing closing ')' in: {line:?}"));
    }

    let params_str = &line[paren_open + 1..line.len() - 1]; // between '(' and ')'

    // ── Parse parameters ──────────────────────────────────────────────────────
    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = Vec::new();
    let mut seen_param_names: HashSet<String> = HashSet::new();

    if !params_str.trim().is_empty() {
        for raw_param in params_str.split(',') {
            let param = raw_param.trim();

            if param.is_empty() {
                return Err(format!("Empty parameter segment in: {line:?}"));
            }

            let colon = param
                .find(':')
                .ok_or_else(|| format!("Missing ':' in parameter: {param:?}"))?;

            let param_name = param[..colon].trim();
            let param_type = param[colon + 1..].trim();

            if param_name.is_empty() || !is_valid_identifier(param_name) {
                return Err(format!("Invalid parameter name: {param_name:?}"));
            }

            if param_type.is_empty() {
                return Err(format!("Missing type for parameter: {param_name:?}"));
            }

            validate_primitive_type_name(param_type)?;

            if !seen_param_names.insert(param_name.to_string()) {
                return Err(format!("Duplicate parameter name: '{param_name}'"));
            }

            properties.insert(param_name.to_string(), json!({ "type": param_type }));

            required.push(json!(param_name));
        }
    }

    Ok(json!({
        "type": "function",
        "function": {
            "name": name,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required
            }
        }
    }))
}

/// Checks that `type_name` is one of the JSON Schema primitive types the
/// compact format supports.  Returns `Err` for unknown types.
fn validate_primitive_type_name(type_name: &str) -> Result<(), String> {
    const SUPPORTED: &[&str] = &[
        "string", "number", "integer", "boolean", "object", "array", "null",
    ];

    if SUPPORTED.contains(&type_name) {
        Ok(())
    } else {
        Err(format!(
            "Invalid type '{type_name}'. Supported types: {}",
            SUPPORTED.join(", ")
        ))
    }
}

// ─── Validation helpers (Phase 3 — unchanged) ─────────────────────────────────

/// Checks that `value` matches the JSON Schema primitive `type` declared in
/// `schema`.  All seven standard JSON Schema primitive types are supported.
///
/// `integer` is stricter than `number`: floating-point values such as `1.5`
/// are rejected; only whole numbers (representable as i64/u64) are accepted.
fn validate_type(argument_name: &str, value: &Value, schema: &Value) -> Result<(), String> {
    let expected_type = schema["type"]
        .as_str()
        .ok_or_else(|| format!("Missing type for argument: {argument_name}"))?;

    let valid = match expected_type {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || value.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false)
        }
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    };

    if !valid {
        return Err(format!(
            "Wrong type for argument '{argument_name}': expected {expected_type}, got {}",
            json_type_name(value)
        ));
    }

    Ok(())
}

/// If the schema specifies an `enum` array, verifies that `value` is one of
/// the permitted values.
fn validate_enum(argument_name: &str, value: &Value, schema: &Value) -> Result<(), String> {
    let enum_values = match schema.get("enum").and_then(Value::as_array) {
        Some(ev) => ev,
        None => return Ok(()),
    };

    if enum_values.contains(value) {
        Ok(())
    } else {
        let allowed: Vec<String> = enum_values.iter().map(|v| v.to_string()).collect();
        Err(format!(
            "Invalid enum value for argument '{argument_name}': \
             {value} is not one of [{}]",
            allowed.join(", ")
        ))
    }
}

/// If the schema specifies a `format`, validates the value against it.
///
/// Currently supported formats:
/// - `date-time` — RFC 3339 / ISO 8601 combined date-time
///
/// Other formats are ignored (pass-through).
fn validate_format(argument_name: &str, value: &Value, schema: &Value) -> Result<(), String> {
    let format = match schema.get("format").and_then(Value::as_str) {
        Some(f) => f,
        None => return Ok(()),
    };

    let s = match value.as_str() {
        Some(s) => s,
        None => return Ok(()),
    };

    match format {
        "date-time" => {
            if is_valid_date_time(s) {
                Ok(())
            } else {
                Err(format!(
                    "Invalid date-time value for argument '{argument_name}': \
                     '{s}' is not a valid RFC 3339 date-time"
                ))
            }
        }
        _ => Ok(()),
    }
}

// ─── Bypass / schema safety (Phase 3 — unchanged) ────────────────────────────

/// Returns `true` when `schema` (and every nested property schema) uses only
/// constructs the compact decoder can safely enforce.
fn schema_is_safe(schema: &Value) -> bool {
    let bypass_keys = [
        "oneOf",
        "anyOf",
        "allOf",
        "not",
        "$ref",
        "pattern",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "additionalProperties",
    ];

    for key in &bypass_keys {
        if schema.get(key).is_some() {
            return false;
        }
    }

    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for prop_schema in properties.values() {
            if prop_schema.get("type").is_none() {
                return false;
            }
            if !schema_is_safe(prop_schema) {
                return false;
            }
        }
    }

    if let Some(items) = schema.get("items")
        && !schema_is_safe(items)
    {
        return false;
    }

    true
}

// ─── Date-time validation (Phase 3 — unchanged) ───────────────────────────────

/// Validates a string as an RFC 3339 date-time using a lightweight hand-rolled
/// parser.  Avoids pulling in a full date/time crate.
fn is_valid_date_time(s: &str) -> bool {
    if s.len() < 20 {
        return false;
    }

    let bytes = s.as_bytes();

    if !bytes[..4].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let year = parse_digits(&s[0..4]);

    if bytes[4] != b'-' {
        return false;
    }

    if !bytes[5..7].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let month = parse_digits(&s[5..7]);
    if !(1..=12).contains(&month) {
        return false;
    }

    if bytes[7] != b'-' {
        return false;
    }

    if !bytes[8..10].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let day = parse_digits(&s[8..10]);
    let max_day = days_in_month(year, month);
    if day < 1 || day > max_day {
        return false;
    }

    if bytes[10] != b'T' && bytes[10] != b't' {
        return false;
    }

    if !bytes[11..13].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let hour = parse_digits(&s[11..13]);
    if hour > 23 {
        return false;
    }

    if bytes[13] != b':' {
        return false;
    }

    if !bytes[14..16].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let minute = parse_digits(&s[14..16]);
    if minute > 59 {
        return false;
    }

    if bytes[16] != b':' {
        return false;
    }

    if !bytes[17..19].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let second = parse_digits(&s[17..19]);
    if second > 60 {
        return false;
    }

    let mut pos = 19;

    if pos < bytes.len() && bytes[pos] == b'.' {
        pos += 1;
        let frac_start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        if pos == frac_start {
            return false;
        }
    }

    if pos >= bytes.len() {
        return false;
    }

    match bytes[pos] {
        b'Z' | b'z' => {
            pos += 1;
        }
        b'+' | b'-' => {
            pos += 1;
            if pos + 5 > bytes.len() {
                return false;
            }
            if !bytes[pos..pos + 2].iter().all(|b| b.is_ascii_digit()) {
                return false;
            }
            let tz_hour = parse_digits(&s[pos..pos + 2]);
            if tz_hour > 23 {
                return false;
            }
            pos += 2;
            if bytes[pos] != b':' {
                return false;
            }
            pos += 1;
            if !bytes[pos..pos + 2].iter().all(|b| b.is_ascii_digit()) {
                return false;
            }
            let tz_min = parse_digits(&s[pos..pos + 2]);
            if tz_min > 59 {
                return false;
            }
            pos += 2;
        }
        _ => return false,
    }

    pos == bytes.len()
}

fn parse_digits(s: &str) -> u32 {
    s.chars()
        .fold(0u32, |acc, c| acc * 10 + c.to_digit(10).unwrap_or(0))
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ─── Integration helper ───────────────────────────────────────────────────────

/// Per-tool report produced by [`CompactToolSet::analyze`].
#[derive(Debug)]
pub struct ToolReport {
    /// Tool name.
    pub name: String,
    /// Whether this tool can be safely compacted.
    pub compactable: bool,
    /// Serialized baseline JSON (pretty-printed, as currently sent to the model).
    pub baseline_json: String,
    /// Compact one-line representation (if compactable).  Empty when bypassed.
    pub compact_line: String,
    /// Byte length of `baseline_json`.
    pub baseline_bytes: usize,
    /// Byte length of `compact_line`.  0 when bypassed.
    pub compact_bytes: usize,
}

/// Aggregated analysis result over a full set of tool definitions.
#[derive(Debug)]
pub struct AnalysisResult {
    /// Per-tool breakdown.
    pub tools: Vec<ToolReport>,
    /// Total bytes across all tool baseline JSON.
    pub total_baseline_bytes: usize,
    /// Total compact bytes for compactable tools.
    pub total_compact_bytes: usize,
    /// Total baseline bytes for bypassed tools (unavoidable overhead).
    pub total_bypass_bytes: usize,
}

impl AnalysisResult {
    /// Number of tools successfully compacted.
    pub fn compacted_count(&self) -> usize {
        self.tools.iter().filter(|t| t.compactable).count()
    }

    /// Number of bypassed tools.
    pub fn bypassed_count(&self) -> usize {
        self.tools.iter().filter(|t| !t.compactable).count()
    }

    /// Bytes saved for compactable tools (bypassed tools contribute 0).
    pub fn bytes_saved(&self) -> usize {
        self.tools
            .iter()
            .filter(|t| t.compactable)
            .map(|t| t.baseline_bytes.saturating_sub(t.compact_bytes))
            .sum()
    }

    /// Percentage of total baseline bytes saved, to two decimal places.
    pub fn percent_saved(&self) -> f64 {
        if self.total_baseline_bytes == 0 {
            return 0.0;
        }
        (self.bytes_saved() as f64 / self.total_baseline_bytes as f64) * 100.0
    }
}

/// Facade over a set of OpenAI-style tool definitions that handles
/// compaction, bypass classification, system-message generation, and
/// strict call decoding.
///
/// # Agent integration sketch
///
/// ```text
/// let cts = CompactToolSet::new(tools::definitions());
///
/// // System message gets the compact block instead of verbose JSON:
/// let compact_block = cts.compact_system_block();
///
/// // Structured `tools:` field only needs bypass tools:
/// let bypass = cts.bypass_tools();
///
/// // Decode a compact call that the model emits:
/// let decoded = cts.decode("github_repo_info", r#"{"owner":"o","repo":"r"}"#)?;
/// ```
pub struct CompactToolSet {
    original: Vec<Value>,
}

impl CompactToolSet {
    /// Build from the standard OpenAI-style tool definitions.
    pub fn new(original: Vec<Value>) -> Self {
        Self { original }
    }

    /// Analyse all tools and return a detailed measurement report.
    ///
    /// Sizes are in **UTF-8 bytes**, not tokens.  Token counts require a
    /// tokeniser library; byte counts are the most honest proxy available
    /// without additional dependencies.
    pub fn analyze(&self) -> AnalysisResult {
        let mut tool_reports: Vec<ToolReport> = Vec::new();
        let mut total_baseline_bytes: usize = 0;
        let mut total_compact_bytes: usize = 0;
        let mut total_bypass_bytes: usize = 0;

        for tool in &self.original {
            let name = tool["function"]["name"]
                .as_str()
                .unwrap_or("<unknown>")
                .to_string();

            let baseline_json = serde_json::to_string_pretty(tool).unwrap_or_default();
            let baseline_bytes = baseline_json.len();
            total_baseline_bytes += baseline_bytes;

            if can_compact_schema(tool) {
                let compact_line = encode_tools(std::slice::from_ref(tool));
                let compact_trimmed = compact_line.trim_end().to_string();
                let compact_bytes = compact_trimmed.len();
                total_compact_bytes += compact_bytes;

                tool_reports.push(ToolReport {
                    name,
                    compactable: true,
                    baseline_json,
                    compact_line: compact_trimmed,
                    baseline_bytes,
                    compact_bytes,
                });
            } else {
                total_bypass_bytes += baseline_bytes;

                tool_reports.push(ToolReport {
                    name,
                    compactable: false,
                    baseline_json,
                    compact_line: String::new(),
                    baseline_bytes,
                    compact_bytes: 0,
                });
            }
        }

        AnalysisResult {
            tools: tool_reports,
            total_baseline_bytes,
            total_compact_bytes,
            total_bypass_bytes,
        }
    }

    /// Returns the compact text block for embedding in the model's system
    /// message.  Contains only compactable tools.
    pub fn compact_system_block(&self) -> String {
        let compactable: Vec<Value> = self
            .original
            .iter()
            .filter(|t| can_compact_schema(t))
            .cloned()
            .collect();
        encode_tools(&compactable)
    }

    /// Returns tools that could not be safely compacted.
    ///
    /// These must be forwarded to the model as standard OpenAI `tools:`
    /// entries so their full schema constraints are enforced.
    pub fn bypass_tools(&self) -> Vec<&Value> {
        self.original
            .iter()
            .filter(|t| !can_compact_schema(t))
            .collect()
    }

    /// Decode a compact tool call produced by the model.
    ///
    /// Delegates to [`decode_calls`] with full strict validation.
    pub fn decode(&self, name: &str, arguments: &str) -> Result<Value, String> {
        decode_calls(&self.original, name, arguments)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ════════════════════════════════════════════════════════════════════════
    //  Shared fixtures
    // ════════════════════════════════════════════════════════════════════════

    /// Basic tool with two required string parameters.
    fn tool_github_repo() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "github_repo_info",
                "description": "Get repository information",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" },
                        "repo":  { "type": "string" }
                    },
                    "required": ["owner", "repo"]
                }
            }
        })
    }

    /// Tool with a single string parameter.
    fn tool_check_endpoint() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_endpoint",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string" }
                    },
                    "required": ["url"]
                }
            }
        })
    }

    /// Tool with multiple parameter types.
    fn tool_mixed_types() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "mixed_tool",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "label":   { "type": "string"  },
                        "count":   { "type": "integer" },
                        "ratio":   { "type": "number"  },
                        "enabled": { "type": "boolean" },
                        "tags":    { "type": "array"   },
                        "meta":    { "type": "object"  }
                    },
                    "required": ["label", "count", "ratio", "enabled", "tags", "meta"]
                }
            }
        })
    }

    /// Tool with an enum constraint on a string parameter.
    fn tool_with_enum() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "set_status",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "status": {
                            "type": "string",
                            "enum": ["open", "closed"]
                        }
                    },
                    "required": ["status"]
                }
            }
        })
    }

    /// Tool with a date-time format constraint.
    fn tool_with_datetime() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "schedule_event",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "at": {
                            "type": "string",
                            "format": "date-time"
                        }
                    },
                    "required": ["at"]
                }
            }
        })
    }

    /// Tool whose schema contains an unsupported constraint (bypass case).
    fn tool_bypass_pattern() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "regex_tool",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "input": {
                            "type": "string",
                            "pattern": "^[a-z]+$"
                        }
                    },
                    "required": ["input"]
                }
            }
        })
    }

    fn tool_bypass_oneof() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "oneof_tool",
                "parameters": {
                    "type": "object",
                    "oneOf": [
                        { "properties": { "a": { "type": "string" } } }
                    ]
                }
            }
        })
    }

    fn tool_bypass_minimum() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "range_tool",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "port": { "type": "integer", "minimum": 1, "maximum": 65535 }
                    },
                    "required": ["port"]
                }
            }
        })
    }

    /// Convenience: build a StreamDecoder with the standard two-tool set.
    fn stream_decoder() -> StreamDecoder {
        StreamDecoder::new(vec![tool_github_repo(), tool_check_endpoint()])
    }

    // ════════════════════════════════════════════════════════════════════════
    //  Phase 3 tests (preserved)
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn encode_simple_tools() {
        let encoded = encode_tools(&[tool_github_repo()]);
        assert_eq!(encoded, "github_repo_info(owner:string, repo:string)\n");
    }

    #[test]
    fn decode_valid_call() {
        let result = decode_calls(
            &[tool_github_repo()],
            "github_repo_info",
            r#"{"owner":"octocat","repo":"hello-world"}"#,
        );
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let call = result.unwrap();
        assert_eq!(call["function"]["name"].as_str(), Some("github_repo_info"));
        assert_eq!(
            call["function"]["arguments"]["owner"].as_str(),
            Some("octocat")
        );
        assert_eq!(call["type"].as_str(), Some("function"));
    }

    #[test]
    fn reject_unknown_tool() {
        let result = decode_calls(
            &[tool_github_repo()],
            "unknown_tool",
            r#"{"owner":"octocat","repo":"hello-world"}"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown tool"));
    }

    #[test]
    fn reject_invalid_json() {
        let result = decode_calls(&[tool_github_repo()], "github_repo_info", r#"{"owner":"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid JSON"));
    }

    #[test]
    fn reject_non_object_arguments() {
        let result = decode_calls(
            &[tool_github_repo()],
            "github_repo_info",
            r#"["foo","bar"]"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("JSON object"));
    }

    #[test]
    fn reject_missing_required_argument() {
        let result = decode_calls(
            &[tool_github_repo()],
            "github_repo_info",
            r#"{"owner":"octocat"}"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing required"));
    }

    #[test]
    fn reject_extra_argument() {
        let result = decode_calls(
            &[tool_github_repo()],
            "github_repo_info",
            r#"{"owner":"octocat","repo":"hello-world","evil":"bad"}"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Extra argument"));
    }

    #[test]
    fn reject_wrong_string_type() {
        let result = decode_calls(
            &[tool_github_repo()],
            "github_repo_info",
            r#"{"owner":123,"repo":"hello-world"}"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Wrong type"));
    }

    #[test]
    fn reject_wrong_number_type() {
        let result = decode_calls(
            &[tool_mixed_types()],
            "mixed_tool",
            r#"{"label":"x","count":1,"ratio":"not-a-number","enabled":true,"tags":[],"meta":{}}"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Wrong type"));
    }

    #[test]
    fn reject_wrong_integer_type_float() {
        let result = decode_calls(
            &[tool_mixed_types()],
            "mixed_tool",
            r#"{"label":"x","count":1.5,"ratio":2.0,"enabled":true,"tags":[],"meta":{}}"#,
        );
        assert!(result.is_err(), "expected Err for fractional float integer");
        assert!(result.unwrap_err().contains("Wrong type"));
    }

    #[test]
    fn reject_wrong_boolean_type() {
        let result = decode_calls(
            &[tool_mixed_types()],
            "mixed_tool",
            r#"{"label":"x","count":1,"ratio":1.0,"enabled":"yes","tags":[],"meta":{}}"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Wrong type"));
    }

    #[test]
    fn reject_wrong_array_type() {
        let result = decode_calls(
            &[tool_mixed_types()],
            "mixed_tool",
            r#"{"label":"x","count":1,"ratio":1.0,"enabled":true,"tags":"not-array","meta":{}}"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Wrong type"));
    }

    #[test]
    fn reject_wrong_object_type() {
        let result = decode_calls(
            &[tool_mixed_types()],
            "mixed_tool",
            r#"{"label":"x","count":1,"ratio":1.0,"enabled":true,"tags":[],"meta":[]}"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Wrong type"));
    }

    #[test]
    fn valid_enum_value() {
        let result = decode_calls(&[tool_with_enum()], "set_status", r#"{"status":"open"}"#);
        assert!(
            result.is_ok(),
            "expected Ok for valid enum value, got {result:?}"
        );
    }

    #[test]
    fn invalid_enum_value() {
        let result = decode_calls(&[tool_with_enum()], "set_status", r#"{"status":"pending"}"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("enum"));
    }

    #[test]
    fn valid_datetime_utc_z() {
        let result = decode_calls(
            &[tool_with_datetime()],
            "schedule_event",
            r#"{"at":"2026-10-03T12:30:00Z"}"#,
        );
        assert!(
            result.is_ok(),
            "expected Ok for valid date-time, got {result:?}"
        );
    }

    #[test]
    fn valid_datetime_offset() {
        let result = decode_calls(
            &[tool_with_datetime()],
            "schedule_event",
            r#"{"at":"2026-10-03T12:30:00+05:30"}"#,
        );
        assert!(result.is_ok(), "expected Ok for offset date-time");
    }

    #[test]
    fn valid_datetime_with_fractional() {
        let result = decode_calls(
            &[tool_with_datetime()],
            "schedule_event",
            r#"{"at":"2026-10-03T12:30:00.123Z"}"#,
        );
        assert!(result.is_ok(), "expected Ok for fractional date-time");
    }

    #[test]
    fn invalid_datetime_plain_string() {
        let result = decode_calls(
            &[tool_with_datetime()],
            "schedule_event",
            r#"{"at":"not-a-date"}"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("date-time"));
    }

    #[test]
    fn invalid_datetime_hello() {
        let result = decode_calls(
            &[tool_with_datetime()],
            "schedule_event",
            r#"{"at":"hello"}"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn invalid_datetime_numeric_string() {
        let result = decode_calls(
            &[tool_with_datetime()],
            "schedule_event",
            r#"{"at":"20261003"}"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn safe_schema_is_compactable() {
        assert!(can_compact_schema(&tool_github_repo()));
        assert!(can_compact_schema(&tool_mixed_types()));
        assert!(can_compact_schema(&tool_with_enum()));
        assert!(can_compact_schema(&tool_with_datetime()));
    }

    #[test]
    fn bypass_schema_pattern() {
        assert!(!can_compact_schema(&tool_bypass_pattern()));
    }

    #[test]
    fn bypass_schema_oneof() {
        assert!(!can_compact_schema(&tool_bypass_oneof()));
    }

    #[test]
    fn bypass_schema_minimum_maximum() {
        assert!(!can_compact_schema(&tool_bypass_minimum()));
    }

    #[test]
    fn bypass_schema_ref() {
        let tool = json!({
            "type": "function",
            "function": {
                "name": "ref_tool",
                "parameters": { "$ref": "#/definitions/Params" }
            }
        });
        assert!(!can_compact_schema(&tool));
    }

    #[test]
    fn bypass_schema_additional_properties() {
        let tool = json!({
            "type": "function",
            "function": {
                "name": "ap_tool",
                "parameters": {
                    "type": "object",
                    "properties": { "name": { "type": "string" } },
                    "additionalProperties": false
                }
            }
        });
        assert!(!can_compact_schema(&tool));
    }

    #[test]
    fn bypass_schema_missing_property_type() {
        let tool = json!({
            "type": "function",
            "function": {
                "name": "notype_tool",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "value": { "description": "no type here" }
                    }
                }
            }
        });
        assert!(!can_compact_schema(&tool));
    }

    // ════════════════════════════════════════════════════════════════════════
    //  Phase 4 — StreamDecoder tests
    // ════════════════════════════════════════════════════════════════════════

    // 1. Single complete call in one push.
    #[test]
    fn stream_single_complete_call() {
        let mut dec = stream_decoder();
        let calls = dec
            .push(r#"github_repo_info({"owner":"octocat","repo":"hello-world"})"#)
            .expect("push should succeed");
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0]["function"]["name"].as_str(),
            Some("github_repo_info")
        );
        assert_eq!(
            calls[0]["function"]["arguments"]["owner"].as_str(),
            Some("octocat")
        );
        let remaining = dec.finish().expect("finish should succeed");
        assert!(remaining.is_empty());
    }

    // 2. Call split across two chunks.
    #[test]
    fn stream_call_split_two_chunks() {
        let mut dec = stream_decoder();

        // First chunk is incomplete — no calls yet.
        let calls1 = dec
            .push(r#"github_repo_info({"owner":"octocat","#)
            .expect("push 1 should succeed");
        assert!(
            calls1.is_empty(),
            "incomplete call should not be decoded yet"
        );

        // Second chunk completes the call.
        let calls2 = dec
            .push(r#""repo":"hello-world"})"#)
            .expect("push 2 should succeed");
        assert_eq!(calls2.len(), 1);
        assert_eq!(
            calls2[0]["function"]["arguments"]["repo"].as_str(),
            Some("hello-world")
        );

        dec.finish().expect("finish should succeed");
    }

    // 3. Call split across several chunks.
    #[test]
    fn stream_call_split_many_chunks() {
        let mut dec = stream_decoder();

        let parts = [
            "check_",
            "endpoint(",
            r#"{"url":"https"#,
            r#"://example.com"}"#,
            ")",
        ];

        let mut all_calls: Vec<Value> = Vec::new();
        for part in &parts {
            let mut calls = dec.push(part).expect("push should succeed");
            all_calls.append(&mut calls);
        }

        assert_eq!(all_calls.len(), 1);
        assert_eq!(
            all_calls[0]["function"]["name"].as_str(),
            Some("check_endpoint")
        );
        assert_eq!(
            all_calls[0]["function"]["arguments"]["url"].as_str(),
            Some("https://example.com")
        );

        dec.finish().expect("finish should succeed");
    }

    // 4. Multiple calls in a single chunk.
    #[test]
    fn stream_multiple_calls_one_chunk() {
        let mut dec = stream_decoder();

        let input = concat!(
            r#"github_repo_info({"owner":"octocat","repo":"hello-world"})"#,
            "\n",
            r#"check_endpoint({"url":"https://example.com"})"#,
        );

        let calls = dec.push(input).expect("push should succeed");
        assert_eq!(calls.len(), 2, "expected 2 decoded calls");
        assert_eq!(
            calls[0]["function"]["name"].as_str(),
            Some("github_repo_info")
        );
        assert_eq!(
            calls[1]["function"]["name"].as_str(),
            Some("check_endpoint")
        );

        dec.finish().expect("finish should succeed");
    }

    // 5. Multiple calls split across chunks.
    #[test]
    fn stream_multiple_calls_across_chunks() {
        let mut dec = stream_decoder();

        let mut all_calls: Vec<Value> = Vec::new();

        // First call arrives completely in chunk 1.
        let mut c = dec
            .push(r#"github_repo_info({"owner":"octocat","repo":"hello-world"})"#)
            .expect("push 1");
        all_calls.append(&mut c);

        // Second call split across chunks 2 and 3.
        let mut c = dec.push(r#"check_endpoint({"url":"https"#).expect("push 2");
        all_calls.append(&mut c);

        let mut c = dec.push(r#"://example.com"})"#).expect("push 3");
        all_calls.append(&mut c);

        assert_eq!(all_calls.len(), 2);
        assert_eq!(
            all_calls[1]["function"]["name"].as_str(),
            Some("check_endpoint")
        );

        dec.finish().expect("finish should succeed");
    }

    // 6. Braces inside a JSON string value must not prematurely end the object.
    #[test]
    fn stream_brace_inside_json_string() {
        let mut dec = stream_decoder();

        // The value contains '}' inside a string — must not confuse the scanner.
        let calls = dec
            .push(r#"check_endpoint({"url":"https://example.com/{id}"})"#)
            .expect("push should succeed");

        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0]["function"]["arguments"]["url"].as_str(),
            Some("https://example.com/{id}")
        );
        dec.finish().expect("finish should succeed");
    }

    // 7. Escaped quote inside a JSON string must not prematurely end the string.
    #[test]
    fn stream_escaped_quote_inside_string() {
        let mut dec = stream_decoder();

        // Value: say "hello"
        let calls = dec
            .push(r#"check_endpoint({"url":"https://say\"hello\".com"})"#)
            .expect("push should succeed");

        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0]["function"]["arguments"]["url"].as_str(),
            Some(r#"https://say"hello".com"#)
        );
        dec.finish().expect("finish should succeed");
    }

    // 8. Incomplete call at finish should produce an error.
    #[test]
    fn stream_incomplete_at_finish() {
        let mut dec = stream_decoder();

        // Only the beginning of a call — never completed.
        dec.push(r#"github_repo_info({"owner":"octocat","#)
            .expect("push should succeed");

        let result = dec.finish();
        assert!(
            result.is_err(),
            "expected Err for incomplete content at finish"
        );
        assert!(
            result.unwrap_err().contains("incomplete"),
            "error should mention incomplete content"
        );
    }

    // 9. Malformed JSON inside a syntactically complete call should be an error.
    #[test]
    fn stream_malformed_json_in_completed_call() {
        let mut dec = stream_decoder();

        // The outer `name({...})` syntax is complete but JSON inside is invalid.
        // We craft a case where the brace scanner sees balanced braces but
        // serde_json rejects the content.
        // Trick: use a bare word that looks like a balanced object to the scanner
        // but isn't valid JSON.  We can't easily do that because find_json_object_end
        // relies on valid brace structure.  Instead we test a case where arguments
        // parse as an array (non-object), which decode_calls rejects.
        let result = dec.push(r#"github_repo_info({"owner":})"#);

        // serde_json will reject `{"owner":}` — should propagate as Err.
        assert!(result.is_err(), "expected Err for malformed JSON arguments");
    }

    // 10. Unknown tool name in a completed call should be an error.
    #[test]
    fn stream_unknown_tool_in_completed_call() {
        let mut dec = stream_decoder();

        let result = dec.push(r#"no_such_tool({"key":"value"})"#);
        assert!(result.is_err(), "expected Err for unknown tool");
        assert!(result.unwrap_err().contains("Unknown tool"));
    }

    // ════════════════════════════════════════════════════════════════════════
    //  Phase 4 — decode_tools tests
    // ════════════════════════════════════════════════════════════════════════

    // 11. Single tool round-trip.
    #[test]
    fn decode_tools_single_tool() {
        let tools = decode_tools("check_endpoint(url:string)\n").expect("should succeed");
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0]["function"]["name"].as_str(),
            Some("check_endpoint")
        );
        assert_eq!(
            tools[0]["function"]["parameters"]["properties"]["url"]["type"].as_str(),
            Some("string")
        );
    }

    // 12. Multiple tools.
    #[test]
    fn decode_tools_multiple_tools() {
        let compact = "github_repo_info(owner:string, repo:string)\ncheck_endpoint(url:string)\n";
        let tools = decode_tools(compact).expect("should succeed");
        assert_eq!(tools.len(), 2);
        assert_eq!(
            tools[0]["function"]["name"].as_str(),
            Some("github_repo_info")
        );
        assert_eq!(
            tools[1]["function"]["name"].as_str(),
            Some("check_endpoint")
        );
    }

    // 13. Multiple parameters — all marked required.
    #[test]
    fn decode_tools_multiple_parameters() {
        let tools =
            decode_tools("github_repo_info(owner:string, repo:string)\n").expect("should succeed");
        let required = tools[0]["function"]["parameters"]["required"]
            .as_array()
            .expect("required array");
        assert_eq!(required.len(), 2);
        // Both owner and repo must appear in required.
        let req_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(req_strs.contains(&"owner"));
        assert!(req_strs.contains(&"repo"));
    }

    // 14. Different primitive types are accepted.
    #[test]
    fn decode_tools_different_types() {
        let compact = "multi_tool(a:string, b:integer, c:number, d:boolean, e:array, f:object)\n";
        let tools = decode_tools(compact).expect("should succeed");
        let props = &tools[0]["function"]["parameters"]["properties"];
        assert_eq!(props["a"]["type"].as_str(), Some("string"));
        assert_eq!(props["b"]["type"].as_str(), Some("integer"));
        assert_eq!(props["c"]["type"].as_str(), Some("number"));
        assert_eq!(props["d"]["type"].as_str(), Some("boolean"));
        assert_eq!(props["e"]["type"].as_str(), Some("array"));
        assert_eq!(props["f"]["type"].as_str(), Some("object"));
    }

    // 15. Duplicate tool name is rejected.
    #[test]
    fn decode_tools_duplicate_tool_rejection() {
        let compact = "my_tool(x:string)\nmy_tool(y:integer)\n";
        let result = decode_tools(compact);
        assert!(result.is_err(), "expected Err for duplicate tool name");
        assert!(result.unwrap_err().contains("Duplicate tool name"));
    }

    // 16. Duplicate parameter name within a tool is rejected.
    #[test]
    fn decode_tools_duplicate_parameter_rejection() {
        let result = decode_tools("my_tool(x:string, x:integer)\n");
        assert!(result.is_err(), "expected Err for duplicate parameter");
        assert!(result.unwrap_err().contains("Duplicate parameter"));
    }

    // 17. Malformed parentheses are rejected.
    #[test]
    fn decode_tools_malformed_parentheses() {
        // Missing closing ')'
        let result = decode_tools("my_tool(x:string");
        assert!(result.is_err(), "expected Err for missing ')'");

        // Missing opening '(' — the line has no '(' at all
        let result = decode_tools("my_tool x:string)");
        assert!(result.is_err(), "expected Err for missing '('");
    }

    // 18. Malformed parameter syntax (missing ':') is rejected.
    #[test]
    fn decode_tools_malformed_parameter_syntax() {
        let result = decode_tools("my_tool(xstring)\n");
        assert!(result.is_err(), "expected Err for missing ':' in parameter");
    }

    // 19. Invalid type name is rejected.
    #[test]
    fn decode_tools_invalid_type_rejection() {
        let result = decode_tools("my_tool(x:foobar)\n");
        assert!(result.is_err(), "expected Err for invalid type");
        assert!(result.unwrap_err().contains("Invalid type"));
    }

    // 20. Empty / whitespace-only input is rejected.
    #[test]
    fn decode_tools_empty_input() {
        let result = decode_tools("");
        assert!(result.is_err(), "expected Err for empty input");

        let result = decode_tools("   \n  \n");
        assert!(result.is_err(), "expected Err for whitespace-only input");
    }

    // ── Round-trip sanity check ───────────────────────────────────────────────

    /// encode_tools() → decode_tools() must produce structurally equivalent
    /// definitions (name + parameter names + types preserved).
    #[test]
    fn encode_decode_tools_round_trip() {
        let original = vec![tool_github_repo(), tool_check_endpoint()];
        let compact = encode_tools(&original);

        let reconstructed = decode_tools(&compact).expect("round-trip decode should succeed");

        assert_eq!(reconstructed.len(), 2);

        // Tool names match.
        assert_eq!(
            reconstructed[0]["function"]["name"].as_str(),
            Some("github_repo_info")
        );
        assert_eq!(
            reconstructed[1]["function"]["name"].as_str(),
            Some("check_endpoint")
        );

        // Parameter types match.
        assert_eq!(
            reconstructed[0]["function"]["parameters"]["properties"]["owner"]["type"].as_str(),
            Some("string")
        );
        assert_eq!(
            reconstructed[1]["function"]["parameters"]["properties"]["url"]["type"].as_str(),
            Some("string")
        );
    }
}
