"""Tests for Atlas Cloud provider configuration and LLM construction."""

import os
from unittest.mock import MagicMock, patch

from router.src.config.settings import RouterConfig
from router.src.core.routing_engine import RoutingEngine
from router.src.entities import RouterOutput


def test_atlascloud_settings_defaults():
    with patch.dict(os.environ, {}, clear=True):
        config = RouterConfig(_env_file=None)

    assert config.ATLASCLOUD_API_KEY is None
    assert config.ATLASCLOUD_BASE_URL == "https://api.atlascloud.ai/v1"


def test_atlascloud_settings_can_be_overridden():
    config = RouterConfig(
        _env_file=None,
        ATLASCLOUD_API_KEY="test-atlas-key",
        ATLASCLOUD_BASE_URL="https://atlas.example/v1",
    )

    assert config.ATLASCLOUD_API_KEY == "test-atlas-key"
    assert config.ATLASCLOUD_BASE_URL == "https://atlas.example/v1"


@patch("router.src.core.routing_engine.ChatOpenAI")
def test_atlascloud_provider_constructs_openai_compatible_client(mock_chat_openai):
    client = MagicMock()
    structured_client = MagicMock()
    mock_chat_openai.return_value = client
    client.with_structured_output.return_value = structured_client

    with (
        patch("router.src.core.routing_engine.settings.ROUTER_LLM_PROVIDER", "atlascloud"),
        patch("router.src.core.routing_engine.settings.ROUTER_LLM_MODEL", ""),
        patch(
            "router.src.core.routing_engine.settings.ATLASCLOUD_API_KEY",
            "test-atlas-key",
        ),
        patch(
            "router.src.core.routing_engine.settings.ATLASCLOUD_BASE_URL",
            "https://api.atlascloud.ai/v1",
        ),
    ):
        result = RoutingEngine.__new__(RoutingEngine)._create_llm()

    mock_chat_openai.assert_called_once_with(
        model="deepseek-ai/deepseek-v4-pro",
        temperature=0,
        api_key="test-atlas-key",
        base_url="https://api.atlascloud.ai/v1",
    )
    client.with_structured_output.assert_called_once_with(RouterOutput)
    assert result is structured_client
