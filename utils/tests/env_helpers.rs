/// Tests for nasiko-utils env helper functions.
///
/// All tests manipulate process environment variables and must not run in
/// parallel. We use `#[serial]` from the `serial_test` crate for isolation.
use nasiko_utils::{env_bool, env_or, env_parse, required_env};
use serial_test::serial;

// ─── Helpers ────────────────────────────────────────────────────────────────

fn set(key: &str, val: &str) {
    // SAFETY: tests are serialised via #[serial]
    unsafe { std::env::set_var(key, val) };
}

fn unset(key: &str) {
    unsafe { std::env::remove_var(key) };
}

// ─── env_or ──────────────────────────────────────────────────────────────────

#[test]
#[serial]
fn env_or_returns_env_var_when_set() {
    set("TEST_ENV_OR_KEY", "hello");
    assert_eq!(env_or("TEST_ENV_OR_KEY", "default"), "hello");
    unset("TEST_ENV_OR_KEY");
}

#[test]
#[serial]
fn env_or_returns_default_when_not_set() {
    unset("TEST_ENV_OR_ABSENT");
    assert_eq!(env_or("TEST_ENV_OR_ABSENT", "my-default"), "my-default");
}

#[test]
#[serial]
fn env_or_returns_empty_string_when_var_is_empty() {
    set("TEST_ENV_OR_EMPTY", "");
    // env_or does not filter empty strings — it returns whatever std::env::var gives
    assert_eq!(env_or("TEST_ENV_OR_EMPTY", "fallback"), "");
    unset("TEST_ENV_OR_EMPTY");
}

#[test]
#[serial]
fn env_or_default_can_be_empty_string() {
    unset("TEST_ENV_OR_EMPTY_DEFAULT");
    assert_eq!(env_or("TEST_ENV_OR_EMPTY_DEFAULT", ""), "");
}

// ─── required_env ────────────────────────────────────────────────────────────

#[test]
#[serial]
fn required_env_returns_ok_when_set() {
    set("TEST_REQUIRED_KEY", "some-value");
    let result = required_env("TEST_REQUIRED_KEY");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "some-value");
    unset("TEST_REQUIRED_KEY");
}

#[test]
#[serial]
fn required_env_returns_err_when_not_set() {
    unset("TEST_REQUIRED_ABSENT");
    let result = required_env("TEST_REQUIRED_ABSENT");
    assert!(result.is_err());
}

#[test]
#[serial]
fn required_env_error_message_contains_key_name() {
    unset("TEST_REQUIRED_MSG");
    let err = required_env("TEST_REQUIRED_MSG").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("TEST_REQUIRED_MSG"),
        "Error message should contain the key name; got: {msg}"
    );
}

#[test]
#[serial]
fn required_env_returns_empty_string_when_var_is_empty() {
    // An empty env var IS set — required_env should return Ok("")
    set("TEST_REQUIRED_EMPTY", "");
    let result = required_env("TEST_REQUIRED_EMPTY");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "");
    unset("TEST_REQUIRED_EMPTY");
}

// ─── env_bool ────────────────────────────────────────────────────────────────

#[test]
#[serial]
fn env_bool_true_string_returns_true() {
    set("TEST_BOOL_TRUE", "true");
    assert!(env_bool("TEST_BOOL_TRUE", false));
    unset("TEST_BOOL_TRUE");
}

#[test]
#[serial]
fn env_bool_one_string_returns_true() {
    set("TEST_BOOL_ONE", "1");
    assert!(env_bool("TEST_BOOL_ONE", false));
    unset("TEST_BOOL_ONE");
}

#[test]
#[serial]
fn env_bool_false_string_returns_false() {
    set("TEST_BOOL_FALSE", "false");
    assert!(!env_bool("TEST_BOOL_FALSE", true));
    unset("TEST_BOOL_FALSE");
}

#[test]
#[serial]
fn env_bool_zero_string_returns_false() {
    set("TEST_BOOL_ZERO", "0");
    assert!(!env_bool("TEST_BOOL_ZERO", true));
    unset("TEST_BOOL_ZERO");
}

#[test]
#[serial]
fn env_bool_missing_returns_default_true() {
    unset("TEST_BOOL_MISSING_T");
    assert!(env_bool("TEST_BOOL_MISSING_T", true));
}

#[test]
#[serial]
fn env_bool_missing_returns_default_false() {
    unset("TEST_BOOL_MISSING_F");
    assert!(!env_bool("TEST_BOOL_MISSING_F", false));
}

#[test]
#[serial]
fn env_bool_garbage_value_returns_default_false() {
    set("TEST_BOOL_GARBAGE", "yes");
    // "yes" is not "true" or "1" → returns false (not matching either branch)
    assert!(!env_bool("TEST_BOOL_GARBAGE", false));
    unset("TEST_BOOL_GARBAGE");
}

#[test]
#[serial]
fn env_bool_garbage_value_returns_default_true() {
    set("TEST_BOOL_GARBAGE_T", "enabled");
    // garbage: the var IS set but is not "true" or "1", so map returns false, unwrap_or(true) is irrelevant
    // env_bool maps |v| v == "true" || v == "1" → false, then unwrap_or(default)
    // WAIT: `.map()` on Ok("enabled") returns Ok(false), then `.unwrap_or(default)` returns false
    // So even with default=true, a set-but-garbage var returns false.
    assert!(!env_bool("TEST_BOOL_GARBAGE_T", true));
    unset("TEST_BOOL_GARBAGE_T");
}

#[test]
#[serial]
fn env_bool_uppercase_true_returns_false() {
    // The impl uses strict equality — "TRUE" is not "true"
    set("TEST_BOOL_UPPER", "TRUE");
    assert!(!env_bool("TEST_BOOL_UPPER", false));
    unset("TEST_BOOL_UPPER");
}

// ─── env_parse ───────────────────────────────────────────────────────────────

#[test]
#[serial]
fn env_parse_u32_valid_number_is_parsed() {
    set("TEST_PARSE_U32", "42");
    assert_eq!(env_parse::<u32>("TEST_PARSE_U32", 0), 42);
    unset("TEST_PARSE_U32");
}

#[test]
#[serial]
fn env_parse_u32_invalid_falls_back_to_default() {
    set("TEST_PARSE_INVALID", "not-a-number");
    assert_eq!(env_parse::<u32>("TEST_PARSE_INVALID", 99), 99);
    unset("TEST_PARSE_INVALID");
}

#[test]
#[serial]
fn env_parse_u32_missing_falls_back_to_default() {
    unset("TEST_PARSE_MISSING");
    assert_eq!(env_parse::<u32>("TEST_PARSE_MISSING", 7), 7);
}

#[test]
#[serial]
fn env_parse_i32_negative_number_is_parsed() {
    set("TEST_PARSE_NEG", "-10");
    assert_eq!(env_parse::<i32>("TEST_PARSE_NEG", 0), -10);
    unset("TEST_PARSE_NEG");
}

#[test]
#[serial]
fn env_parse_usize_large_value_is_parsed() {
    set("TEST_PARSE_LARGE", "65535");
    assert_eq!(env_parse::<usize>("TEST_PARSE_LARGE", 0), 65535);
    unset("TEST_PARSE_LARGE");
}

#[test]
#[serial]
fn env_parse_u64_is_parsed() {
    set("TEST_PARSE_U64", "9999999999");
    assert_eq!(env_parse::<u64>("TEST_PARSE_U64", 0), 9_999_999_999);
    unset("TEST_PARSE_U64");
}

#[test]
#[serial]
fn env_parse_i64_is_parsed() {
    set("TEST_PARSE_I64", "100000");
    assert_eq!(env_parse::<i64>("TEST_PARSE_I64", 0), 100_000);
    unset("TEST_PARSE_I64");
}

#[test]
#[serial]
fn env_parse_zero_is_parsed_correctly() {
    set("TEST_PARSE_ZERO", "0");
    assert_eq!(env_parse::<u32>("TEST_PARSE_ZERO", 99), 0);
    unset("TEST_PARSE_ZERO");
}

#[test]
#[serial]
fn env_parse_empty_string_falls_back_to_default() {
    set("TEST_PARSE_EMPTY", "");
    assert_eq!(env_parse::<u32>("TEST_PARSE_EMPTY", 55), 55);
    unset("TEST_PARSE_EMPTY");
}

#[test]
#[serial]
fn env_parse_float_is_parsed() {
    set("TEST_PARSE_FLOAT", "4.25");
    let val = env_parse::<f64>("TEST_PARSE_FLOAT", 0.0);
    assert!((val - 4.25).abs() < 1e-9);
    unset("TEST_PARSE_FLOAT");
}

#[test]
#[serial]
fn env_parse_float_invalid_falls_back_to_default() {
    set("TEST_PARSE_FLOAT_BAD", "abc");
    let val = env_parse::<f64>("TEST_PARSE_FLOAT_BAD", 2.71);
    assert!((val - 2.71).abs() < 1e-9);
    unset("TEST_PARSE_FLOAT_BAD");
}
