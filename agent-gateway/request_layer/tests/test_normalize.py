"""Unit tests for the L0 query normalizer."""

from request_layer.src.normalize import normalize


def test_normalize_handles_none() -> None:
    assert normalize(None) == ""


def test_normalize_strips_whitespace_and_lowercases() -> None:
    assert normalize("   Hello   World  ") == "hello world"


def test_normalize_collapses_internal_whitespace() -> None:
    assert normalize("hello\t\tworld\n\nagain") == "hello world again"


def test_normalize_strips_trailing_punctuation() -> None:
    assert normalize("hello world!!!") == "hello world"
    assert normalize("hello!?!?!") == "hello"


def test_normalize_collapses_punctuation_runs() -> None:
    assert normalize("wait...... what??!!") == "wait. what"


def test_normalize_json_sorts_keys() -> None:
    a = normalize('{"b": 1, "a": 2}')
    b = normalize('{"a": 2, "b": 1}')
    assert a == b


def test_normalize_json_drops_default_values() -> None:
    full = normalize('{"text": "hello", "options": {}}')
    minimal = normalize('{"text": "hello"}')
    assert full == minimal


def test_normalize_json_recursive_lowercase() -> None:
    a = normalize('{"text": "Hello World"}')
    b = normalize('{"text": "hello world"}')
    assert a == b


def test_normalize_bytes_decodes_utf8() -> None:
    assert normalize(b'{"text": "Hello"}') == normalize('{"text": "hello"}')


def test_normalize_invalid_json_falls_back_to_text() -> None:
    out = normalize("hello world! this is not json")
    assert out == "hello world! this is not json"


def test_normalize_is_idempotent() -> None:
    once = normalize('{"text": "  Hello   World  ", "options": null}')
    twice = normalize(once)
    # Twice will be normalized as a plain string (since the canonical form
    # is no longer JSON), but the canonical-form-of-canonical-form should
    # be byte-identical to the first canonical form.
    assert once == twice
