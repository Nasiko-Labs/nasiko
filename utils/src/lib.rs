/// Helper for required env vars — returns clear error message.
pub fn required_env(key: &str) -> anyhow::Result<String> {
    std::env::var(key).map_err(|_| anyhow::anyhow!("missing required env var: {}", key))
}

/// Helper for optional env var with default.
pub fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Helper for optional bool env var.
pub fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .map(|v| v == "true" || v == "1")
        .unwrap_or(default)
}

/// Helper for optional numeric env var.
pub fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Generates an env template string. Use in `Config::template()` implementations.
///
/// ```rust
/// use nasiko_utils::env_template;
/// let t = env_template! {
///     "CP_BIND" => "0.0.0.0:8080"; "Listen address",
///     "DATABASE_URL" => required; "Postgres connection string",
///     "OPENAI_API_KEY" => optional; "OpenAI key for routing",
/// };
/// ```
#[macro_export]
macro_rules! env_template {
    ($($key:literal => required; $desc:literal),* $(,)?) => {{
        let mut out = String::new();
        $(
            out.push_str(&format!("# {}\n{}=  # REQUIRED\n\n", $desc, $key));
        )*
        out
    }};
    ($($key:literal => $val:tt; $desc:literal),* $(,)?) => {{
        let mut out = String::new();
        $($crate::__template_entry!(out, $key, $val, $desc);)*
        out
    }};
}

#[macro_export]
#[doc(hidden)]
macro_rules! __template_entry {
    ($out:ident, $key:literal, required, $desc:literal) => {
        $out.push_str(&format!("# {}\n{}=  # REQUIRED\n\n", $desc, $key));
    };
    ($out:ident, $key:literal, optional, $desc:literal) => {
        $out.push_str(&format!("# {}\n# {}=\n\n", $desc, $key));
    };
    ($out:ident, $key:literal, $default:literal, $desc:literal) => {
        $out.push_str(&format!("# {}\n{}={}\n\n", $desc, $key, $default));
    };
}
