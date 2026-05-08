import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import yaml


REPO_ROOT = Path(__file__).resolve().parents[3]
COMPOSE_FILE = REPO_ROOT / "docker-compose.local.yml"
TRANSLATOR_MAIN = REPO_ROOT / "agents/a2a-translator/src/__main__.py"
TRANSLATOR_EXECUTOR = REPO_ROOT / "agents/a2a-translator/src/openai_agent_executor.py"
TRANSLATOR_COMPOSE = REPO_ROOT / "agents/a2a-translator/docker-compose.yml"
LITELLM_CONFIG = REPO_ROOT / "orchestrator/litellm_config.yaml"


class TestLLMGatewayIntegration(unittest.TestCase):
    """Integration-oriented checks for the LiteLLM gateway path."""

    @classmethod
    def setUpClass(cls):
        with COMPOSE_FILE.open("r", encoding="utf-8") as f:
            cls.compose = yaml.safe_load(f)

    def test_gateway_service_is_part_of_platform_boot(self):
        services = self.compose.get("services", {})
        self.assertIn("llm-gateway", services)
        gateway = services["llm-gateway"]
        self.assertIn("app-network", gateway.get("networks", []))
        self.assertIn("agents-net", gateway.get("networks", []))

    def test_redis_listener_injects_gateway_runtime_env(self):
        listener_env = self.compose["services"]["nasiko-redis-listener"]["environment"]
        self.assertIn("LLM_GATEWAY_URL", listener_env)
        self.assertIn("LLM_VIRTUAL_KEY", listener_env)
        self.assertIn("LLM_GATEWAY_MODEL", listener_env)

    def test_sample_agent_is_gateway_first_without_provider_keys_in_compose(self):
        compose_data = yaml.safe_load(TRANSLATOR_COMPOSE.read_text(encoding="utf-8"))
        env_list = compose_data["services"]["a2a-translator"]["environment"]

        env_as_text = "\n".join(env_list)
        self.assertIn("LLM_GATEWAY_URL", env_as_text)
        self.assertIn("LLM_VIRTUAL_KEY", env_as_text)
        self.assertNotIn("OPENAI_API_KEY", env_as_text)
        self.assertNotIn("ANTHROPIC_API_KEY", env_as_text)

    def test_agent_runtime_keeps_legacy_provider_fallback_path(self):
        text = TRANSLATOR_MAIN.read_text(encoding="utf-8")
        self.assertIn("elif os.getenv(\"OPENROUTER_API_KEY\")", text)
        self.assertIn("elif os.getenv(\"MINIMAX_API_KEY\")", text)
        self.assertIn("os.getenv(\"OPENROUTER_API_KEY\")", text)
        self.assertIn("os.getenv(\"OPENAI_API_KEY\")", text)

    def test_provider_rotation_is_config_driven(self):
        config = yaml.safe_load(LITELLM_CONFIG.read_text(encoding="utf-8"))
        models = {entry["model_name"]: entry["litellm_params"]["model"] for entry in config["model_list"]}
        self.assertIn("platform-default", models)
        self.assertIn("anthropic-default", models)
        self.assertIn("${LITELLM_PROVIDER_MODEL", models["platform-default"])
        self.assertIn("${LITELLM_ANTHROPIC_MODEL", models["anthropic-default"])

    def test_gateway_calls_preserve_trace_context_headers(self):
        text = TRANSLATOR_EXECUTOR.read_text(encoding="utf-8")
        self.assertIn("inject(headers)", text)
        self.assertIn("extra_headers=self._build_gateway_headers() if self.use_gateway else None", text)

    def test_live_gateway_healthcheck_optional(self):
        if os.getenv("NASIKO_RUN_DOCKER_INTEGRATION") != "1":
            self.skipTest("Set NASIKO_RUN_DOCKER_INTEGRATION=1 to run docker-compose integration checks")

        with tempfile.NamedTemporaryFile(mode="w", suffix=".env", delete=False) as env_file:
            env_file.write("OPENAI_API_KEY=dummy\n")
            env_file.write("LITELLM_MASTER_KEY=nasiko-dev-virtual-key\n")
            env_path = env_file.name

        try:
            subprocess.run(
                [
                    "docker",
                    "compose",
                    "--env-file",
                    env_path,
                    "-f",
                    str(COMPOSE_FILE),
                    "up",
                    "-d",
                    "llm-gateway",
                ],
                check=True,
                cwd=REPO_ROOT,
            )

            result = subprocess.run(
                [
                    "docker",
                    "compose",
                    "--env-file",
                    env_path,
                    "-f",
                    str(COMPOSE_FILE),
                    "exec",
                    "-T",
                    "llm-gateway",
                    "curl",
                    "-sSf",
                    "http://localhost:4000/health",
                ],
                check=False,
                cwd=REPO_ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, msg=result.stderr or result.stdout)
        finally:
            subprocess.run(
                [
                    "docker",
                    "compose",
                    "--env-file",
                    env_path,
                    "-f",
                    str(COMPOSE_FILE),
                    "down",
                ],
                cwd=REPO_ROOT,
                check=False,
            )
            Path(env_path).unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
