"""
Unit tests for Atlas Cloud provider integration in the routing engine.
Tests configuration defaults and the provider-selection branch in
``RoutingEngine._create_llm``.
"""

import os
from unittest.mock import patch, MagicMock

import pytest


class TestRouterConfigAtlasCloud:
    """Test Atlas Cloud-related settings in RouterConfig."""

    def test_atlascloud_api_key_config(self):
        """ATLASCLOUD_API_KEY and provider selection should be configurable."""
        from router.src.config.settings import RouterConfig

        config = RouterConfig(
            _env_file=None,
            ATLASCLOUD_API_KEY="test-atlascloud-key",
            ROUTER_LLM_PROVIDER="atlascloud",
            ROUTER_LLM_MODEL="deepseek-ai/DeepSeek-V3-0324",
        )
        assert config.ATLASCLOUD_API_KEY == "test-atlascloud-key"
        assert config.ROUTER_LLM_PROVIDER == "atlascloud"
        assert config.ROUTER_LLM_MODEL == "deepseek-ai/DeepSeek-V3-0324"

    def test_atlascloud_default_base_url(self):
        """Default Atlas Cloud base URL should be the public OpenAI-compatible endpoint."""
        from router.src.config.settings import RouterConfig

        config = RouterConfig(_env_file=None)
        assert config.ATLASCLOUD_BASE_URL == "https://api.atlascloud.ai/v1"

    def test_atlascloud_base_url_override(self):
        """Atlas Cloud base URL should be overridable via env/config."""
        from router.src.config.settings import RouterConfig

        config = RouterConfig(
            _env_file=None,
            ATLASCLOUD_BASE_URL="https://gateway.example.com/v1",
        )
        assert config.ATLASCLOUD_BASE_URL == "https://gateway.example.com/v1"


class TestRoutingEngineAtlasCloud:
    """Test the Atlas Cloud branch of RoutingEngine._create_llm.

    ``ChatOpenAI`` is patched so we can assert exactly which kwargs the
    routing engine passes for the ``atlascloud`` provider without making
    any network calls.
    """

    def _build_engine_with_patches(self, config_kwargs):
        """Instantiate RoutingEngine with ChatOpenAI/embeddings patched.

        Returns the MagicMock standing in for ``ChatOpenAI`` so the caller
        can inspect the call arguments.
        """
        from router.src.config.settings import RouterConfig

        test_settings = RouterConfig(_env_file=None, **config_kwargs)

        with patch.dict(os.environ, {}, clear=True), patch(
            "router.src.core.routing_engine.settings", test_settings
        ), patch(
            "router.src.core.routing_engine.ChatOpenAI"
        ) as mock_chat_openai, patch(
            "router.src.core.routing_engine.OpenAIEmbeddings"
        ):
            # with_structured_output returns a wrapped runnable; keep it a mock
            mock_chat_openai.return_value.with_structured_output.return_value = (
                MagicMock()
            )
            from router.src.core.routing_engine import RoutingEngine

            RoutingEngine()

        return mock_chat_openai

    def test_atlascloud_provider_passes_expected_kwargs(self):
        """atlascloud provider must use the Atlas base URL, key and temperature 0."""
        mock_chat_openai = self._build_engine_with_patches(
            {
                "ROUTER_LLM_PROVIDER": "atlascloud",
                "ROUTER_LLM_MODEL": "deepseek-ai/DeepSeek-V3-0324",
                "ATLASCLOUD_API_KEY": "test-key",
            }
        )

        _, kwargs = mock_chat_openai.call_args
        assert kwargs["model"] == "deepseek-ai/DeepSeek-V3-0324"
        assert kwargs["base_url"] == "https://api.atlascloud.ai/v1"
        assert kwargs["api_key"] == "test-key"
        assert kwargs["temperature"] == 0
        # deepseek-v4-pro is a reasoning model; the branch must pass a
        # generous max_tokens so structured output is not truncated.
        assert kwargs["max_tokens"] >= 512

    def test_atlascloud_default_model_fallback(self):
        """Empty model should fall back to the default deepseek-ai/deepseek-v4-pro."""
        mock_chat_openai = self._build_engine_with_patches(
            {
                "ROUTER_LLM_PROVIDER": "atlascloud",
                "ROUTER_LLM_MODEL": "",
                "ATLASCLOUD_API_KEY": "test-key",
            }
        )

        _, kwargs = mock_chat_openai.call_args
        assert kwargs["model"] == "deepseek-ai/deepseek-v4-pro"
        assert kwargs["max_tokens"] >= 512

    def test_atlascloud_provider_is_case_insensitive(self):
        """Provider matching lower-cases the value, so 'AtlasCloud' must work."""
        mock_chat_openai = self._build_engine_with_patches(
            {
                "ROUTER_LLM_PROVIDER": "AtlasCloud",
                "ROUTER_LLM_MODEL": "deepseek-ai/DeepSeek-V3-0324",
                "ATLASCLOUD_API_KEY": "test-key",
            }
        )

        _, kwargs = mock_chat_openai.call_args
        assert kwargs["base_url"] == "https://api.atlascloud.ai/v1"

    def test_openai_default_has_no_base_url(self):
        """Sanity check: the default openai provider sets no base_url."""
        mock_chat_openai = self._build_engine_with_patches(
            {
                "ROUTER_LLM_PROVIDER": "openai",
                "ROUTER_LLM_MODEL": "gpt-4o-mini",
                "OPENAI_API_KEY": "openai-key",
            }
        )

        _, kwargs = mock_chat_openai.call_args
        assert "base_url" not in kwargs
        assert kwargs["model"] == "gpt-4o-mini"
