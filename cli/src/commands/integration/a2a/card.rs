//! Whether the catalog could store an A2A agent card discovery found, and the
//! version it would store.
//!
//! The rules are where storing a card fails on the server: the deploy-time
//! card fetch writes card fields into `agents` columns, and the skills decode
//! into the catalog's `Skill` and are inserted into `agent_skills`
//! (`VARCHAR(255)` id and name). Cards of both protocol generations are
//! accepted: A2A 0.3 (top-level `url`) and 1.0 (`supportedInterfaces[]`).

use std::fmt;

use nasiko_types::a2a::extract_transport_path;
use serde_json::{Map, Value};

/// Version the catalog stores when a card declares none (`catalog/import.rs`).
const DEFAULT_VERSION: &str = "1.0.0";
/// `agent_skills.skill_key` and `agent_skills.name` are `VARCHAR(255)`.
const MAX_SKILL_FIELD_CHARS: usize = 255;

/// One reason a card cannot be stored, at a JSON path (`skills[0].id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardIssue {
    pub field: String,
    pub message: String,
}

impl fmt::Display for CardIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// The version the catalog would store for `card`, or every reason it could
/// not store the card at all.
pub fn check_card(card: &Value) -> Result<String, Vec<CardIssue>> {
    let Some(object) = card.as_object() else {
        return Err(vec![issue("card", "is not a JSON object")]);
    };
    let mut errors = Vec::new();
    check_name(object, &mut errors);
    if extract_transport_path(card).is_none() {
        errors.push(issue(
            "url",
            "no usable endpoint: needs `supportedInterfaces[].url` (A2A 1.0) or `url` \
             (A2A 0.3) as an http(s) URL or an absolute path",
        ));
    }
    for field in [
        "description",
        "version",
        "protocolVersion",
        "documentationUrl",
        "iconUrl",
    ] {
        expect_type(
            object.get(field),
            field,
            Value::is_string,
            "a string",
            &mut errors,
        );
    }
    for field in ["provider", "capabilities", "securitySchemes"] {
        expect_type(
            object.get(field),
            field,
            Value::is_object,
            "an object",
            &mut errors,
        );
    }
    expect_type(
        object.get("security"),
        "security",
        Value::is_array,
        "an array",
        &mut errors,
    );
    for field in ["defaultInputModes", "defaultOutputModes"] {
        check_string_list(object.get(field), field, &mut errors);
    }
    check_skills(object.get("skills"), &mut errors);

    if errors.is_empty() {
        Ok(version(object))
    } else {
        Err(errors)
    }
}

/// The card's version with one leading `v` stripped, as the import stores it;
/// [`DEFAULT_VERSION`] when it declares none.
fn version(card: &Map<String, Value>) -> String {
    match card.get("version").and_then(Value::as_str).map(str::trim) {
        Some(version) if !version.is_empty() => {
            version.strip_prefix('v').unwrap_or(version).to_string()
        }
        _ => DEFAULT_VERSION.to_string(),
    }
}

fn check_name(card: &Map<String, Value>, errors: &mut Vec<CardIssue>) {
    let name = card.get("name").and_then(Value::as_str).map(str::trim);
    if name.is_none_or(str::is_empty) {
        errors.push(issue("name", "is required and must be a non-empty string"));
    }
}

/// Each skill needs string `id`, `name` and `description`; `id` and `name`
/// non-empty and within `VARCHAR(255)`; `tags` and modes lists of strings.
/// The catalog's `Skill` gives `tags` and `examples` `#[serde(default)]`, which
/// fills an absent field but rejects `null`, and `description` is a plain
/// `String`, so `null` is an error for those three though absent lists are not.
fn check_skills(skills: Option<&Value>, errors: &mut Vec<CardIssue>) {
    let skills = match skills {
        None | Some(Value::Null) => return,
        Some(Value::Array(skills)) => skills,
        Some(_) => return errors.push(issue("skills", "must be an array")),
    };
    for (index, skill) in skills.iter().enumerate() {
        let path = format!("skills[{index}]");
        let Some(skill) = skill.as_object() else {
            errors.push(issue(path, "must be an object"));
            continue;
        };
        for field in ["id", "name"] {
            check_bounded_string(skill.get(field), &format!("{path}.{field}"), errors);
        }
        let description = format!("{path}.description");
        match skill.get("description") {
            None => errors.push(issue(description, "is required")),
            Some(Value::String(_)) => {}
            Some(_) => errors.push(issue(description, "must be a string")),
        }
        expect_list_if_present(
            skill.get("tags"),
            &format!("{path}.tags"),
            is_string_list,
            "an array of strings",
            errors,
        );
        expect_list_if_present(
            skill.get("examples"),
            &format!("{path}.examples"),
            Value::is_array,
            "an array",
            errors,
        );
        // Not decoded by the catalog, so `null` is harmless there.
        for field in ["inputModes", "outputModes"] {
            check_string_list(skill.get(field), &format!("{path}.{field}"), errors);
        }
    }
}

fn check_bounded_string(value: Option<&Value>, path: &str, errors: &mut Vec<CardIssue>) {
    match value {
        None => errors.push(issue(path, "is required")),
        Some(Value::String(value)) if value.trim().is_empty() => {
            errors.push(issue(path, "must not be empty"));
        }
        Some(Value::String(value)) if value.chars().count() > MAX_SKILL_FIELD_CHARS => {
            errors.push(issue(
                path,
                format!("longer than {MAX_SKILL_FIELD_CHARS} characters"),
            ));
        }
        Some(Value::String(_)) => {}
        Some(_) => errors.push(issue(path, "must be a string")),
    }
}

/// An optional field: absent or `null` is fine, any other type than
/// `expected` is an error.
fn expect_type(
    value: Option<&Value>,
    path: &str,
    is_expected: fn(&Value) -> bool,
    expected: &str,
    errors: &mut Vec<CardIssue>,
) {
    if value.is_some_and(|value| !value.is_null() && !is_expected(value)) {
        errors.push(issue(path, format!("must be {expected}")));
    }
}

/// A list field decoded with `#[serde(default)]`: absent is fine, anything
/// else, `null` included, must be `expected`.
fn expect_list_if_present(
    value: Option<&Value>,
    path: &str,
    is_expected: fn(&Value) -> bool,
    expected: &str,
    errors: &mut Vec<CardIssue>,
) {
    if value.is_some_and(|value| !is_expected(value)) {
        errors.push(issue(path, format!("must be {expected}")));
    }
}

/// An optional list of strings: absent or `null` is fine.
fn check_string_list(value: Option<&Value>, path: &str, errors: &mut Vec<CardIssue>) {
    expect_type(value, path, is_string_list, "an array of strings", errors);
}

fn is_string_list(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_string))
}

fn issue(field: impl Into<String>, message: impl Into<String>) -> CardIssue {
    CardIssue {
        field: field.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The card CrewAI 1.15 (`A2AServerConfig` + a2a-sdk 0.3) actually serves.
    fn crewai_card() -> Value {
        json!({
            "additionalInterfaces": [],
            "capabilities": {"pushNotifications": false, "streaming": true},
            "defaultInputModes": ["text/plain", "application/json"],
            "defaultOutputModes": ["text/plain", "application/json"],
            "description": "Find facts Careful analyst",
            "name": "crew-researcher",
            "preferredTransport": "JSONRPC",
            "protocolVersion": "0.3.0",
            "security": [],
            "securitySchemes": {},
            "skills": [{"description": "Find facts", "id": "researcher", "name": "Researcher", "tags": ["researcher"]}],
            "supportsAuthenticatedExtendedCard": false,
            "url": "http://127.0.0.1:8765/",
            "version": "1.0.0"
        })
    }

    /// The card LangGraph Agent Server 0.16 actually serves (extensions trimmed).
    fn langgraph_card() -> Value {
        json!({
            "name": "planner",
            "description": "planner assistant",
            "supportedInterfaces": [{"url": "http://127.0.0.1:2024/a2a/2bdf", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}],
            "capabilities": {"streaming": true, "pushNotifications": false, "stateTransitionHistory": false},
            "defaultInputModes": ["application/json", "text/plain"],
            "defaultOutputModes": ["application/json", "text/plain"],
            "skills": [{
                "id": "2bdf-main", "name": "planner Capabilities", "description": "planner assistant",
                "tags": ["assistant", "langgraph"], "examples": [],
                "inputModes": ["application/json", "text/plain"], "outputModes": ["application/json", "text/plain"],
                "metadata": {"inputSchema": {"required": ["messages"]}}
            }],
            "version": "0.16.4"
        })
    }

    fn errors_of(card: Value) -> Vec<String> {
        check_card(&card)
            .expect_err("card should be rejected")
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn real_crewai_and_langgraph_cards_are_storable() {
        assert_eq!(check_card(&crewai_card()), Ok("1.0.0".to_string()));
        assert_eq!(check_card(&langgraph_card()), Ok("0.16.4".to_string()));
    }

    #[test]
    fn a_minimal_card_is_storable_with_the_default_version() {
        let card = json!({"name": "Minimal Agent", "url": "/"});
        assert_eq!(check_card(&card), Ok(DEFAULT_VERSION.to_string()));
    }

    #[test]
    fn versions_are_normalized_like_the_import() {
        let card = json!({"name": "a", "url": "http://h/", "version": "v3.1"});
        assert_eq!(check_card(&card), Ok("3.1".to_string()));
    }

    #[test]
    fn a_card_without_a_name_or_endpoint_is_rejected() {
        assert_eq!(
            errors_of(json!({"name": " "})),
            vec![
                "name: is required and must be a non-empty string".to_string(),
                "url: no usable endpoint: needs `supportedInterfaces[].url` (A2A 1.0) or `url` (A2A 0.3) as an http(s) URL or an absolute path".to_string(),
            ]
        );
        assert_eq!(
            errors_of(json!("not a card")),
            vec!["card: is not a JSON object"]
        );
    }

    #[test]
    fn skills_the_catalog_cannot_decode_are_rejected_with_their_path() {
        let card = json!({
            "name": "a", "url": "http://h/",
            "skills": [
                {"id": "ok", "name": "Ok", "description": "fine"},
                {"id": "no-description", "name": "N"},
                {"id": 7, "name": "N", "description": "d"},
                {"id": "t", "name": "T", "description": "d", "tags": ["x", 1]},
                {"id": "x".repeat(256), "name": "Long", "description": "d"},
                "not an object"
            ]
        });
        assert_eq!(
            errors_of(card),
            vec![
                "skills[1].description: is required",
                "skills[2].id: must be a string",
                "skills[3].tags: must be an array of strings",
                "skills[4].id: longer than 255 characters",
                "skills[5]: must be an object",
            ]
        );
    }

    /// `null` where the catalog's `Skill` has a `#[serde(default)]` list or a
    /// plain `String` fails its decode, so it fails here too; absent lists,
    /// and `null` in fields the catalog ignores, do not.
    #[test]
    fn null_is_rejected_where_the_catalog_would_reject_it() {
        let card = json!({
            "name": "a", "url": "http://h/",
            "skills": [
                {"id": "a", "name": "A", "description": null},
                {"id": "b", "name": "B", "description": "d", "tags": null},
                {"id": "c", "name": "C", "description": "d", "examples": null},
                {"id": "d", "name": "D", "description": "d",
                 "inputModes": null, "outputModes": null},
                {"id": "e", "name": "E", "description": "d"}
            ]
        });
        assert_eq!(
            errors_of(card),
            vec![
                "skills[0].description: must be a string",
                "skills[1].tags: must be an array of strings",
                "skills[2].examples: must be an array",
            ]
        );
    }

    #[test]
    fn wrongly_typed_card_fields_are_rejected() {
        let card = json!({
            "name": "a", "url": "http://h/",
            "capabilities": "streaming",
            "defaultInputModes": "text/plain",
            "securitySchemes": [],
            "skills": {}
        });
        let errors = errors_of(card);
        assert!(errors.contains(&"capabilities: must be an object".to_string()));
        assert!(errors.contains(&"defaultInputModes: must be an array of strings".to_string()));
        assert!(errors.contains(&"securitySchemes: must be an object".to_string()));
        assert!(errors.contains(&"skills: must be an array".to_string()));
    }
}
