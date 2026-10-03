use nasiko_llm_router::GatewayConfig;

pub fn from_env() -> GatewayConfig {
    load(GatewayConfig::from_env(), |key| std::env::var(key).ok())
}

fn load(mut cfg: GatewayConfig, get: impl Fn(&str) -> Option<String>) -> GatewayConfig {
    cfg.classifier_backend = get("CLASSIFIER_BACKEND").unwrap_or(cfg.classifier_backend);
    cfg.classifier_endpoint = get("CLASSIFIER_ENDPOINT").unwrap_or(cfg.classifier_endpoint);
    cfg.classifier_model = get("CLASSIFIER_MODEL").unwrap_or_else(|| {
        if cfg.classifier_backend == "local" {
            "local-training-v1".into()
        } else {
            cfg.classifier_model
        }
    });
    cfg.classifier_timeout_ms = parse(get("CLASSIFIER_TIMEOUT_MS"), cfg.classifier_timeout_ms);
    cfg.classifier_min_confidence = parse(
        get("CLASSIFIER_MIN_CONFIDENCE"),
        cfg.classifier_min_confidence,
    );
    cfg.classifier_context_chars = parse(
        get("CLASSIFIER_CONTEXT_CHARS"),
        cfg.classifier_context_chars,
    )
    .min(24000);
    cfg.classifier_seed = parse(get("CLASSIFIER_SEED"), cfg.classifier_seed);
    cfg.cache_switch_enabled = match get("CACHE_SWITCH_ENABLED").as_deref() {
        Some("true" | "1") => true,
        Some("false" | "0") => false,
        _ => cfg.cache_switch_enabled,
    };
    cfg.cache_switch_margin = parse(get("CACHE_SWITCH_MARGIN"), cfg.cache_switch_margin);
    if !cfg.classifier_min_confidence.is_finite()
        || !(0.0..=1.0).contains(&cfg.classifier_min_confidence)
    {
        cfg.classifier_min_confidence = GatewayConfig::default().classifier_min_confidence;
    }
    if !cfg.cache_switch_margin.is_finite() || !(0.0..1.0).contains(&cfg.cache_switch_margin) {
        cfg.cache_switch_margin = GatewayConfig::default().cache_switch_margin;
    }
    cfg
}

fn parse<T: std::str::FromStr>(raw: Option<String>, default: T) -> T {
    raw.and_then(|value| value.parse().ok()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_probability_values_preserve_safe_defaults() {
        for value in ["NaN", "inf", "-0.1", "1.1"] {
            let cfg = load(GatewayConfig::default(), |key| match key {
                "CLASSIFIER_MIN_CONFIDENCE" | "CACHE_SWITCH_MARGIN" => Some(value.into()),
                _ => None,
            });
            assert_eq!(cfg.classifier_min_confidence, 0.6);
            assert_eq!(cfg.cache_switch_margin, 0.2);
        }
    }

    #[test]
    fn binary_loads_classifier_and_cache_overrides() {
        let cfg = load(GatewayConfig::default(), |key| match key {
            "CLASSIFIER_BACKEND" => Some("local".into()),
            "CLASSIFIER_ENDPOINT" => Some("http://localhost/decisions".into()),
            "CLASSIFIER_MODEL" => Some("test-model".into()),
            "CLASSIFIER_TIMEOUT_MS" => Some("50".into()),
            "CLASSIFIER_MIN_CONFIDENCE" => Some("0.8".into()),
            "CLASSIFIER_CONTEXT_CHARS" => Some("30000".into()),
            "CLASSIFIER_SEED" => Some("17".into()),
            "CACHE_SWITCH_ENABLED" => Some("1".into()),
            "CACHE_SWITCH_MARGIN" => Some("0.4".into()),
            _ => None,
        });
        assert_eq!(cfg.classifier_backend, "local");
        assert_eq!(cfg.classifier_endpoint, "http://localhost/decisions");
        assert_eq!(cfg.classifier_model, "test-model");
        assert_eq!(cfg.classifier_timeout_ms, 50);
        assert_eq!(cfg.classifier_min_confidence, 0.8);
        assert_eq!(cfg.classifier_context_chars, 24000);
        assert_eq!(cfg.classifier_seed, 17);
        assert!(cfg.cache_switch_enabled);
        assert_eq!(cfg.cache_switch_margin, 0.4);
    }

    #[test]
    fn invalid_values_preserve_regex_defaults() {
        let cfg = load(GatewayConfig::default(), |key| match key {
            "CLASSIFIER_TIMEOUT_MS"
            | "CLASSIFIER_MIN_CONFIDENCE"
            | "CLASSIFIER_CONTEXT_CHARS"
            | "CLASSIFIER_SEED"
            | "CACHE_SWITCH_ENABLED"
            | "CACHE_SWITCH_MARGIN" => Some("invalid".into()),
            _ => None,
        });
        assert_eq!(cfg.classifier_backend, "regex");
        assert_eq!(cfg.classifier_timeout_ms, 3000);
        assert_eq!(cfg.classifier_min_confidence, 0.6);
        assert_eq!(cfg.classifier_context_chars, 12000);
        assert_eq!(cfg.classifier_seed, 42);
        assert!(!cfg.cache_switch_enabled);
        assert_eq!(cfg.cache_switch_margin, 0.2);
    }

    #[test]
    fn local_backend_uses_embedded_model_label_without_override() {
        let cfg = load(GatewayConfig::default(), |key| {
            (key == "CLASSIFIER_BACKEND").then(|| "local".into())
        });
        assert_eq!(cfg.classifier_model, "local-training-v1");
    }
}
