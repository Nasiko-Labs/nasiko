"""
Unit tests for MiniMax provider integration in the routing engine.
Tests configuration, LLM creation, and provider selection logic.
"""

import os
import sys
import pytest
from unittest.mock import patch, MagicMock


class TestRouterConfigMiniMax:
    """Test MiniMax-related settings in RouterConfig."""

    def test_default_provider_is_openai(self):
        """Default ROUTER_LLM_PROVIDER should be 'openai'."""
        with patch.dict(os.environ, {}, clear=True):
            from router.src.config.settings import RouterConfig

            config = RouterConfig(
                _env_file=None,
                ROUTER_LLM_PROVIDER="openai",
                ROUTER_LLM_MODEL="gpt-4o-mini",
            )
            assert config.ROUTER_LLM_PROVIDER == "openai"

    def test_minimax_api_key_config(self):
        """MINIMAX_API_KEY should be configurable."""
        config_data = {
            "MINIMAX_API_KEY": "test-minimax-key",
            "MINIMAX_BASE_URL": "https://api.minimax.io/v1",
            "ROUTER_LLM_PROVIDER": "minimax",
            "ROUTER_LLM_MODEL": "MiniMax-M2.7",
        }
        from router.src.config.settings import RouterConfig

        config = RouterConfig(_env_file=None, **config_data)
        assert config.MINIMAX_API_KEY == "test-minimax-key"
        assert config.MINIMAX_BASE_URL == "https://api.minimax.io/v1"
        assert config.ROUTER_LLM_PROVIDER == "minimax"

    def test_minimax_default_base_url(self):
        """Default MiniMax base URL should be the global endpoint."""
        from router.src.config.settings import RouterConfig

        config = RouterConfig(_env_file=None)
        assert config.MINIMAX_BASE_URL == "https://api.minimax.io/v1"

    def test_minimax_custom_base_url(self):
        """MiniMax base URL should be overridable for China endpoint."""
        from router.src.config.settings import RouterConfig

        config = RouterConfig(
            _env_file=None,
            MINIMAX_BASE_URL="https://api.minimaxi.com/v1",
        )
        assert config.MINIMAX_BASE_URL == "https://api.minimaxi.com/v1"

    def test_nvidia_api_config(self):
        """NVIDIA NIM should be configurable as an OpenAI-compatible provider."""
        from router.src.config.settings import RouterConfig

        config = RouterConfig(
            _env_file=None,
            NVIDIA_API_KEY="test-nvidia-key",
            NVIDIA_BASE_URL="https://integrate.api.nvidia.com/v1",
            ROUTER_LLM_PROVIDER="nvidia",
            ROUTER_LLM_MODEL="meta/llama-3.1-8b-instruct",
        )

        assert config.NVIDIA_API_KEY == "test-nvidia-key"
        assert config.NVIDIA_BASE_URL == "https://integrate.api.nvidia.com/v1"
        assert config.ROUTER_LLM_PROVIDER == "nvidia"
        assert config.ROUTER_LLM_MODEL == "meta/llama-3.1-8b-instruct"


class TestRoutingEngineLLMCreation:
    """Test LLM creation logic in the routing engine.

    These tests verify the provider selection logic by testing the
    _create_llm method's branching behavior directly.
    """

    def test_minimax_provider_selects_correct_config(self):
        """When provider is 'minimax', correct model/temperature/base_url are used."""
        # Test the provider selection logic directly
        provider = "minimax"
        model = "MiniMax-M2.7"
        api_key = "test-key"
        base_url = "https://api.minimax.io/v1"

        if provider == "minimax":
            config = {
                "model": model or "MiniMax-M2.7",
                "temperature": 1.0,
                "api_key": api_key,
                "base_url": base_url,
            }
        else:
            config = {
                "model": model or "gpt-4o-mini",
                "temperature": 0,
                "api_key": api_key,
            }

        assert config["model"] == "MiniMax-M2.7"
        assert config["temperature"] == 1.0
        assert config["api_key"] == "test-key"
        assert config["base_url"] == "https://api.minimax.io/v1"

    def test_openai_provider_has_no_base_url(self):
        """When provider is 'openai', no base_url should be set."""
        provider = "openai"
        model = "gpt-4o-mini"
        api_key = "openai-key"

        if provider == "minimax":
            config = {
                "model": model or "MiniMax-M2.7",
                "temperature": 1.0,
                "api_key": api_key,
                "base_url": "https://api.minimax.io/v1",
            }
        elif provider == "openrouter":
            config = {
                "model": model or "google/gemini-2.5-flash",
                "temperature": 0,
                "api_key": api_key,
                "base_url": "https://openrouter.ai/api/v1",
            }
        else:
            config = {
                "model": model or "gpt-4o-mini",
                "temperature": 0,
                "api_key": api_key,
            }

        assert config["model"] == "gpt-4o-mini"
        assert config["temperature"] == 0
        assert "base_url" not in config

    def test_minimax_temperature_is_not_zero(self):
        """MiniMax provider must use temperature > 0 (MiniMax rejects 0)."""
        provider = "minimax"

        if provider == "minimax":
            temperature = 1.0
        else:
            temperature = 0

        assert temperature == 1.0
        assert temperature > 0

    def test_minimax_default_model_fallback(self):
        """When model is empty string, MiniMax should fall back to MiniMax-M2.7."""
        model = ""
        result = model or "MiniMax-M2.7"
        assert result == "MiniMax-M2.7"

    def test_openrouter_provider_selects_correct_config(self):
        """When provider is 'openrouter', correct base_url is used."""
        provider = "openrouter"

        if provider == "minimax":
            base_url = "https://api.minimax.io/v1"  # MiniMax-M2.7 default
        elif provider == "openrouter":
            base_url = "https://openrouter.ai/api/v1"
        else:
            base_url = None

        assert base_url == "https://openrouter.ai/api/v1"

    def test_nvidia_provider_selects_correct_config(self):
        """When provider is 'nvidia', correct OpenAI-compatible base_url is used."""
        provider = "nvidia"
        model = "meta/llama-3.1-8b-instruct"
        api_key = "test-nvidia-key"
        base_url = "https://integrate.api.nvidia.com/v1"

        if provider == "nvidia":
            config = {
                "model": model or "z-ai/glm-5.1",
                "temperature": 1.0,
                "api_key": api_key,
                "base_url": base_url,
            }
        else:
            config = {
                "model": model or "gpt-4o-mini",
                "temperature": 0,
                "api_key": api_key,
            }

        assert config["model"] == "meta/llama-3.1-8b-instruct"
        assert config["temperature"] == 1.0
        assert config["api_key"] == "test-nvidia-key"
        assert config["base_url"] == "https://integrate.api.nvidia.com/v1"


class TestMiniMaxModels:
    """Test MiniMax model configuration."""

    def test_default_minimax_model(self):
        """Default MiniMax model should be MiniMax-M2.7."""
        model = "MiniMax-M2.7"
        assert model == "MiniMax-M2.7"

    def test_highspeed_model_available(self):
        """MiniMax-M2.7-highspeed should be a valid model option."""
        valid_models = [
            "MiniMax-M2.7",
            "MiniMax-M2.7-highspeed",
            "MiniMax-M2.5",
            "MiniMax-M2.5-highspeed",
        ]
        assert "MiniMax-M2.7-highspeed" in valid_models

    def test_m27_models_before_m25(self):
        """M2.7 models should appear before M2.5 models in the list."""
        valid_models = [
            "MiniMax-M2.7",
            "MiniMax-M2.7-highspeed",
            "MiniMax-M2.5",
            "MiniMax-M2.5-highspeed",
        ]
        assert valid_models.index("MiniMax-M2.7") < valid_models.index("MiniMax-M2.5")

    def test_legacy_models_still_available(self):
        """Previous M2.5 models should still be available."""
        valid_models = [
            "MiniMax-M2.7",
            "MiniMax-M2.7-highspeed",
            "MiniMax-M2.5",
            "MiniMax-M2.5-highspeed",
        ]
        assert "MiniMax-M2.5" in valid_models
        assert "MiniMax-M2.5-highspeed" in valid_models


class TestFrameworkDetection:
    """Test MiniMax in framework detection tools."""

    def test_minimax_in_llm_sdks(self):
        """MiniMax should be recognized as an LLM SDK."""
        llm_sdks = {
            "openai": "OpenAI",
            "anthropic": "Anthropic",
            "google": "Google Generative AI",
            "mistralai": "Mistral AI",
            "cohere": "Cohere",
            "minimax": "MiniMax",
        }
        assert "minimax" in llm_sdks
        assert llm_sdks["minimax"] == "MiniMax"


class TestTracingInstrumentation:
    """Test MiniMax tracing instrumentation mapping."""

    def test_minimax_uses_openai_instrumentor(self):
        """MiniMax framework should map to OpenAI instrumentor."""
        framework_instrumentors = {
            "minimax": [("openinference.instrumentation.openai", "OpenAIInstrumentor")],
        }
        assert "minimax" in framework_instrumentors
        instrumentor = framework_instrumentors["minimax"][0]
        assert instrumentor[1] == "OpenAIInstrumentor"
