"""Unit tests for the AgentCard parser and policy inference."""

import pytest

from request_layer.src.agentcard import parse_agentcard
from request_layer.src.cache.policy import infer_policy
from request_layer.src.config import Settings


@pytest.fixture
def settings() -> Settings:
    return Settings()


def test_parse_a2a_translator_card() -> None:
    card = {
        "name": "translator",
        "url": "http://agent-translator:8000",
        "capabilities": {"streaming": True, "pushNotifications": False},
        "skills": [
            {
                "id": "translate-text",
                "tags": ["translation", "language"],
                "examples": ["translate hello to french"],
            }
        ],
    }
    manifest = parse_agentcard(card)
    assert manifest is not None
    assert manifest.name == "translator"
    assert manifest.endpoint_url == "http://agent-translator:8000"
    assert "translation" in manifest.tags
    assert "translate-text" in manifest.capabilities
    assert manifest.examples == ["translate hello to french"]


def test_parse_returns_none_without_name() -> None:
    assert parse_agentcard({"url": "http://x"}) is None


def test_parse_returns_none_without_url() -> None:
    assert parse_agentcard({"name": "translator"}) is None


def test_parse_supports_list_capabilities() -> None:
    card = {
        "name": "weather",
        "url": "http://weather:8000",
        "capabilities": ["weather", "forecast"],
    }
    manifest = parse_agentcard(card)
    assert manifest is not None
    assert manifest.capabilities == {"weather", "forecast"}


def test_policy_infer_translation_long_ttl(settings) -> None:
    card = {
        "name": "translator",
        "url": "http://x",
        "skills": [{"id": "translate-text", "tags": ["translation"]}],
    }
    manifest = parse_agentcard(card)
    policy = infer_policy(manifest, settings)
    assert policy.cache_ttl_seconds == 86400
    assert policy.semantic_threshold == 0.92


def test_policy_infer_realtime_short_ttl(settings) -> None:
    card = {
        "name": "weather",
        "url": "http://x",
        "skills": [{"id": "lookup", "tags": ["weather", "realtime"]}],
    }
    manifest = parse_agentcard(card)
    policy = infer_policy(manifest, settings)
    assert policy.cache_ttl_seconds == 300
    assert policy.semantic_threshold == 0.97


def test_policy_default_for_unknown_capability(settings) -> None:
    card = {
        "name": "novel-agent",
        "url": "http://x",
        "skills": [{"id": "do-stuff", "tags": ["misc"]}],
    }
    manifest = parse_agentcard(card)
    policy = infer_policy(manifest, settings)
    assert policy.cache_ttl_seconds == settings.request_layer_default_ttl_seconds
    assert policy.semantic_threshold == settings.request_layer_semantic_threshold


def test_expensive_tag_raises_cost_cap(settings) -> None:
    card = {
        "name": "gpu-heavy",
        "url": "http://x",
        "skills": [{"id": "render", "tags": ["expensive"]}],
    }
    manifest = parse_agentcard(card)
    policy = infer_policy(manifest, settings)
    assert policy.cost_cap_usd_per_min == 5.0
