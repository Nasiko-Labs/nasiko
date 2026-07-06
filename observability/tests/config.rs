//! Tests for TelemetryConfig construction and defaults.
//! No external services required.
//!
//! Tests that read/write env vars use a process-wide mutex to prevent races
//! when the test harness runs them in parallel.

use std::sync::Mutex;

use nasiko_observability::TelemetryConfig;

/// Serialise all env-touching tests to avoid data races on the process
/// environment (required in Rust 2024 where set_var/remove_var are unsafe).
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ─── Direct construction — no env vars needed ─────────────────────────────────

#[test]
fn telemetry_config_can_be_constructed_directly() {
    let cfg = TelemetryConfig {
        service_name: "direct-svc".to_owned(),
        otlp_endpoint: Some("http://localhost:4318".to_owned()),
        otlp_protocol: "http/protobuf".to_owned(),
        otlp_headers: None,
        sample_ratio: 0.1,
    };
    assert_eq!(cfg.service_name, "direct-svc");
    assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://localhost:4318"));
    assert_eq!(cfg.otlp_protocol, "http/protobuf");
    assert!(cfg.otlp_headers.is_none());
    assert!((cfg.sample_ratio - 0.1).abs() < f64::EPSILON);
}

#[test]
fn telemetry_config_direct_zero_sample_ratio() {
    let cfg = TelemetryConfig {
        service_name: "svc".to_owned(),
        otlp_endpoint: None,
        otlp_protocol: "grpc".to_owned(),
        otlp_headers: None,
        sample_ratio: 0.0,
    };
    assert!((cfg.sample_ratio - 0.0).abs() < f64::EPSILON);
    assert!(cfg.otlp_endpoint.is_none());
    assert!(cfg.otlp_headers.is_none());
}

// ─── Default values from from_env() when env vars are absent ─────────────────
//
// Each test acquires ENV_LOCK, clears the var, reads config, then restores.

#[test]
fn telemetry_config_default_service_name_is_nasiko() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_SERVICE_NAME").ok();
    // SAFETY: guarded by ENV_LOCK; no concurrent env access in test suite
    unsafe { std::env::remove_var("OTEL_SERVICE_NAME") };

    let cfg = TelemetryConfig::from_env();

    if let Some(v) = prev {
        unsafe { std::env::set_var("OTEL_SERVICE_NAME", v) };
    }
    assert_eq!(cfg.service_name, "nasiko");
}

#[test]
fn telemetry_config_default_protocol_is_grpc() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").ok();
    unsafe { std::env::remove_var("OTEL_EXPORTER_OTLP_PROTOCOL") };

    let cfg = TelemetryConfig::from_env();

    if let Some(v) = prev {
        unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", v) };
    }
    assert_eq!(cfg.otlp_protocol, "grpc");
}

#[test]
fn telemetry_config_default_sample_ratio_is_one() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_TRACES_SAMPLER_ARG").ok();
    unsafe { std::env::remove_var("OTEL_TRACES_SAMPLER_ARG") };

    let cfg = TelemetryConfig::from_env();

    if let Some(v) = prev {
        unsafe { std::env::set_var("OTEL_TRACES_SAMPLER_ARG", v) };
    }
    assert!((cfg.sample_ratio - 1.0).abs() < f64::EPSILON);
}

#[test]
fn telemetry_config_default_otlp_endpoint_is_none_when_unset() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    unsafe { std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT") };

    let cfg = TelemetryConfig::from_env();

    if let Some(v) = prev {
        unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", v) };
    }
    assert!(cfg.otlp_endpoint.is_none());
}

#[test]
fn telemetry_config_default_otlp_headers_is_none_when_unset() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_EXPORTER_OTLP_HEADERS").ok();
    unsafe { std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS") };

    let cfg = TelemetryConfig::from_env();

    if let Some(v) = prev {
        unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_HEADERS", v) };
    }
    assert!(cfg.otlp_headers.is_none());
}

// ─── Values picked up from environment ───────────────────────────────────────

#[test]
fn telemetry_config_reads_service_name_from_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_SERVICE_NAME").ok();
    // SAFETY: guarded by ENV_LOCK
    unsafe { std::env::set_var("OTEL_SERVICE_NAME", "my-custom-service") };
    let cfg = TelemetryConfig::from_env();
    match prev {
        Some(v) => unsafe { std::env::set_var("OTEL_SERVICE_NAME", v) },
        None => unsafe { std::env::remove_var("OTEL_SERVICE_NAME") },
    }
    assert_eq!(cfg.service_name, "my-custom-service");
}

#[test]
fn telemetry_config_reads_otlp_endpoint_from_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317") };
    let cfg = TelemetryConfig::from_env();
    match prev {
        Some(v) => unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", v) },
        None => unsafe { std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT") },
    }
    assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://collector:4317"));
}

#[test]
fn telemetry_config_reads_otlp_protocol_from_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").ok();
    unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf") };
    let cfg = TelemetryConfig::from_env();
    match prev {
        Some(v) => unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", v) },
        None => unsafe { std::env::remove_var("OTEL_EXPORTER_OTLP_PROTOCOL") },
    }
    assert_eq!(cfg.otlp_protocol, "http/protobuf");
}

#[test]
fn telemetry_config_reads_sample_ratio_from_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_TRACES_SAMPLER_ARG").ok();
    unsafe { std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "0.5") };
    let cfg = TelemetryConfig::from_env();
    match prev {
        Some(v) => unsafe { std::env::set_var("OTEL_TRACES_SAMPLER_ARG", v) },
        None => unsafe { std::env::remove_var("OTEL_TRACES_SAMPLER_ARG") },
    }
    assert!((cfg.sample_ratio - 0.5).abs() < f64::EPSILON);
}

#[test]
fn telemetry_config_falls_back_to_default_ratio_on_invalid_value() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_TRACES_SAMPLER_ARG").ok();
    unsafe { std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "not-a-number") };
    let cfg = TelemetryConfig::from_env();
    match prev {
        Some(v) => unsafe { std::env::set_var("OTEL_TRACES_SAMPLER_ARG", v) },
        None => unsafe { std::env::remove_var("OTEL_TRACES_SAMPLER_ARG") },
    }
    // Invalid value → default of 1.0
    assert!((cfg.sample_ratio - 1.0).abs() < f64::EPSILON);
}

#[test]
fn telemetry_config_reads_otlp_headers_from_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("OTEL_EXPORTER_OTLP_HEADERS").ok();
    unsafe {
        std::env::set_var("OTEL_EXPORTER_OTLP_HEADERS", "Authorization=Bearer token")
    };
    let cfg = TelemetryConfig::from_env();
    match prev {
        Some(v) => unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_HEADERS", v) },
        None => unsafe { std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS") },
    }
    assert_eq!(
        cfg.otlp_headers.as_deref(),
        Some("Authorization=Bearer token")
    );
}