pub fn legend_for_definitions() -> String {
    let mut parts = vec!["arg?=optional".to_string()];
    parts.push("!=no extra keys".to_string());
    parts.push("any=any JSON".to_string());
    parts.push("obj=any object".to_string());
    parts.push("datetime=ISO 8601 with offset".to_string());
    parts.push("date=YYYY-MM-DD".to_string());
    parts.push("time=HH:MM:SS with offset".to_string());
    parts.push("email=address".to_string());
    parts.push("uri=absolute URI".to_string());
    parts.push("uuid=UUID string".to_string());
    parts.join("; ")
}

pub fn prompt_for_definitions(definitions: &str) -> String {
    let legend = legend_for_definitions();
    format!(
        "Tools ({legend}):\n{definitions}\nCall a tool by writing <<call NAME {{JSON args}}>>, one per line. If none fits, answer normally."
    )
}
