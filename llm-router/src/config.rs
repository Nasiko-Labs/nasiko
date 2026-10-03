//! Classifier backend configuration for the standalone binary.
//!
//! Backend choice, model, endpoint, timeout and key are read **here**, never in the
//! library: `nasiko-llm-router` takes a ready-made [`ClassifierSettings`] and never touches
//! the environment for these. With nothing set, the backend is `regex` and behaviour is
//! identical to the router before the classifier trait existed.
//!
//! | Env var                     | Default | Meaning                                              |
//! |-----------------------------|---------|------------------------------------------------------|
//! | `CLASSIFIER_BACKEND`        | `regex` | `regex`, or `http` (alias `hosted`)                  |
//! | `CLASSIFIER_MODEL`          | empty   | model id sent in the request body                    |
//! | `CLASSIFIER_ENDPOINT`       | empty   | URL the `http` backend POSTs to                      |
//! | `CLASSIFIER_API_KEY`        | unset   | optional bearer token (never logged)                 |
//! | `CLASSIFIER_TIMEOUT_MS`     | `5000`  | per-call budget; on expiry regex is used             |
//! | `CLASSIFIER_MIN_CONFIDENCE` | `0.5`   | `http` verdicts below this fall back to regex        |

use nasiko_llm_router::ClassifierSettings;

/// Read classifier settings through `get` (the process environment in `main`, a map in
/// tests). Invalid numbers keep the default rather than failing startup.
pub fn classifier_settings_from(get: impl Fn(&str) -> Option<String>) -> ClassifierSettings {
    let d = ClassifierSettings::default();
    let non_empty = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    ClassifierSettings {
        backend: non_empty("CLASSIFIER_BACKEND").unwrap_or(d.backend),
        model: non_empty("CLASSIFIER_MODEL").unwrap_or(d.model),
        endpoint: non_empty("CLASSIFIER_ENDPOINT").unwrap_or(d.endpoint),
        api_key: non_empty("CLASSIFIER_API_KEY"),
        timeout_ms: non_empty("CLASSIFIER_TIMEOUT_MS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(d.timeout_ms),
        min_confidence: non_empty("CLASSIFIER_MIN_CONFIDENCE")
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| (0.0..=1.0).contains(v))
            .unwrap_or(d.min_confidence),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn from(pairs: &[(&str, &str)]) -> ClassifierSettings {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        classifier_settings_from(|k| m.get(k).cloned())
    }

    #[test]
    fn defaults_to_regex_with_5s_timeout() {
        let s = from(&[]);
        assert_eq!(s.backend, "regex");
        assert_eq!(s.timeout_ms, 5000);
        assert!(s.endpoint.is_empty() && s.model.is_empty() && s.api_key.is_none());
    }

    #[test]
    fn reads_http_backend_settings() {
        let s = from(&[
            ("CLASSIFIER_BACKEND", "http"),
            ("CLASSIFIER_MODEL", "m1"),
            ("CLASSIFIER_ENDPOINT", "http://localhost:9000/classify"),
            ("CLASSIFIER_TIMEOUT_MS", "250"),
            ("CLASSIFIER_MIN_CONFIDENCE", "0.7"),
        ]);
        assert_eq!(s.backend, "http");
        assert_eq!(s.model, "m1");
        assert_eq!(s.endpoint, "http://localhost:9000/classify");
        assert_eq!(s.timeout_ms, 250);
        assert!((s.min_confidence - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn invalid_numbers_keep_defaults() {
        let s = from(&[
            ("CLASSIFIER_TIMEOUT_MS", "soon"),
            ("CLASSIFIER_MIN_CONFIDENCE", "7"),
        ]);
        assert_eq!(s.timeout_ms, 5000);
        assert!((s.min_confidence - 0.5).abs() < f32::EPSILON);
    }
}
