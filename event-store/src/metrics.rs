//! OpenTelemetry metrics for tiering (docs/LAKEHOUSE_OPS.md "Observe").
//!
//! Same stack as the server (`global::meter`, exported by the server's OTLP
//! meter provider; Prometheus scrapes the collector). Names use `nasiko.tiering.*`.

use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::metrics::{Counter, Gauge, Histogram};

pub struct TieringMetrics {
    rows: Counter<u64>,
    batch_seconds: Histogram<f64>,
    failures: Counter<u64>,
    oldest_hot_age: Gauge<f64>,
    cold_read_seconds: Histogram<f64>,
    maintenance_seconds: Histogram<f64>,
}

pub fn metrics() -> &'static TieringMetrics {
    static M: OnceLock<TieringMetrics> = OnceLock::new();
    M.get_or_init(|| {
        let meter = global::meter("nasiko.tiering");
        TieringMetrics {
            rows: meter
                .u64_counter("nasiko.tiering.rows")
                .with_description("Rows moved from Postgres to Lance, by table")
                .build(),
            batch_seconds: meter
                .f64_histogram("nasiko.tiering.batch.duration")
                .with_unit("s")
                .with_description("Duration of one tiering batch, by table")
                .build(),
            failures: meter
                .u64_counter("nasiko.tiering.failures")
                .with_description("Failed tiering runs or maintenance steps, by stage")
                .build(),
            oldest_hot_age: meter
                .f64_gauge("nasiko.tiering.oldest_hot_age")
                .with_unit("s")
                .with_description("Age of the oldest hot row per tiered table (lag)")
                .build(),
            cold_read_seconds: meter
                .f64_histogram("nasiko.tiering.cold_read.duration")
                .with_unit("s")
                .with_description("Latency of Lance reads, by table and op")
                .build(),
            maintenance_seconds: meter
                .f64_histogram("nasiko.tiering.maintenance.duration")
                .with_unit("s")
                .with_description("Lance compact + cleanup + optimize per table")
                .build(),
        }
    })
}

impl TieringMetrics {
    pub fn batch(&self, table: &str, rows: u64, took: Duration) {
        let attrs = [KeyValue::new("table", table.to_owned())];
        self.rows.add(rows, &attrs);
        self.batch_seconds.record(took.as_secs_f64(), &attrs);
    }

    pub fn failure(&self, stage: &'static str) {
        self.failures.add(1, &[KeyValue::new("stage", stage)]);
    }

    pub fn oldest_hot(&self, table: &str, age: Duration) {
        self.oldest_hot_age.record(
            age.as_secs_f64(),
            &[KeyValue::new("table", table.to_owned())],
        );
    }

    pub fn cold_read(&self, table: &str, op: &'static str, took: Duration) {
        self.cold_read_seconds.record(
            took.as_secs_f64(),
            &[
                KeyValue::new("table", table.to_owned()),
                KeyValue::new("op", op),
            ],
        );
    }

    pub fn maintenance(&self, table: &str, took: Duration) {
        self.maintenance_seconds.record(
            took.as_secs_f64(),
            &[KeyValue::new("table", table.to_owned())],
        );
    }
}
