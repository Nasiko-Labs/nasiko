pub mod display;
pub mod image;
pub mod logs_level;
pub mod net;
pub mod redact;
pub mod source_files;
pub mod term;
pub mod version;
pub mod zip;

/// A required env var. Set but empty (or only whitespace) counts as missing: an empty
/// `DATABASE_URL` would otherwise fail later with a driver error that does not name the variable.
pub fn required_env(key: &str) -> anyhow::Result<String> {
    nonempty_env(key).ok_or_else(|| anyhow::anyhow!("missing required env var: {key}"))
}

/// Every one of `keys` that is unset or empty, reported together, so a first start names all of
/// them instead of one per restart.
pub fn require_all(keys: &[&str]) -> anyhow::Result<()> {
    let missing: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|key| nonempty_env(key).is_none())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "{}",
        missing
            .iter()
            .map(|key| format!("missing required env var: {key}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// Helper for optional env var with default.
pub fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// An optional bool env var: `true`/`1` and `false`/`0` (any case), with empty meaning `false` as
/// before. Anything else keeps the default and says so, rather than silently reading as `false`.
pub fn env_bool(key: &str, default: bool) -> bool {
    let Ok(raw) = std::env::var(key) else {
        return default;
    };
    match parse_bool(&raw) {
        Some(value) => value,
        None => {
            tracing::warn!(
                "{key}={raw:?} is not a valid boolean (true, false, 1 or 0); using default {default}"
            );
            default
        }
    }
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" | "" => Some(false),
        _ => None,
    }
}

/// An optional numeric (or other parsed) env var. Unset or empty takes the default; a value that
/// does not parse takes it too, with a warning naming the key, the value and the default.
pub fn env_parse<T: std::str::FromStr + std::fmt::Display>(key: &str, default: T) -> T {
    let Some(raw) = nonempty_env(key) else {
        return default;
    };
    match raw.trim().parse() {
        Ok(value) => value,
        Err(_) => {
            tracing::warn!(
                "{key}={raw:?} is not a valid {}; using default {default}",
                std::any::type_name::<T>()
            );
            default
        }
    }
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

#[cfg(test)]
mod env_tests {
    use super::parse_bool;

    #[test]
    fn booleans_accept_both_spellings_in_any_case() {
        assert_eq!(parse_bool("TRUE"), Some(true));
        assert_eq!(parse_bool(" 1 "), Some(true));
        assert_eq!(parse_bool("False"), Some(false));
        assert_eq!(parse_bool("0"), Some(false));
    }

    #[test]
    fn an_empty_boolean_still_reads_as_false() {
        assert_eq!(parse_bool(""), Some(false));
    }

    #[test]
    fn an_unrecognised_boolean_is_not_guessed() {
        assert_eq!(parse_bool("yes"), None);
        assert_eq!(parse_bool("on"), None);
    }
}
