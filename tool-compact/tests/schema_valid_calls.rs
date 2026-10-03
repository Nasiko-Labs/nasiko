//! `cargo test -p nasiko-tool-compact schema_valid_calls_only`
//!
//! The same property is also a scenario in `tests/features/decode.feature`.

#[path = "common/sample.rs"]
mod sample;

#[test]
fn schema_valid_calls_only() {
    sample::reject_invalid_success();
}
