import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "agents" / "python"))
import pi_coder


class PiHarnessTests(unittest.TestCase):
    def test_models_file_points_at_nasiko_injected_openai_url(self):
        spec = pi_coder.models_file("http://nasiko-router", "secret", "agentkv-qwen")
        provider = spec["providers"]["agentkv"]
        self.assertEqual(provider["baseUrl"], "http://nasiko-router/v1")
        self.assertEqual(provider["api"], "openai-completions")
        self.assertFalse(provider["compat"]["supportsDeveloperRole"])

    def test_workspace_keeps_source_and_stable_system_prefix(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pi_coder.write_workspace(root, "def clamp(v,lo,hi):\n    return v", "shared repo", "repair clamp")
            self.assertIn("def clamp", (root / "module.py").read_text())
            self.assertEqual((root / "SYSTEM.md").read_text(), "shared repo")
            self.assertIn("repair clamp", (root / "AGENTS.md").read_text())
            self.assertEqual(pi_coder.extract_code(root), "def clamp(v,lo,hi):\n    return v")

    def test_usage_merge_keeps_cached_tokens(self):
        merged = pi_coder.merge_usage([
            {"prompt_tokens": 10, "completion_tokens": 2, "prompt_tokens_details": {"cached_tokens": 8}},
            {"prompt_tokens": 12, "completion_tokens": 3, "prompt_tokens_details": {"cached_tokens": 10}},
        ])
        self.assertEqual(merged["prompt_tokens"], 22)
        self.assertEqual(merged["completion_tokens"], 5)
        self.assertEqual(merged["prompt_tokens_details"]["cached_tokens"], 18)

    def test_config_dir_is_pi_home_layout(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pi_coder.config_dir(Path(directory))
            self.assertEqual(path, Path(directory) / ".pi" / "agent")
            self.assertTrue(path.is_dir())

    def test_runtime_writes_catalog_where_pi_loads_it(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            spec = pi_coder.models_file("http://router", "secret", "agentkv-qwen")
            written = pi_coder.write_runtime(home, spec)
            self.assertEqual(written, home / ".pi" / "models.json")
            catalog = (home / ".pi" / "agent" / "models.json").read_text()
            self.assertEqual(json.loads(written.read_text()), json.loads(catalog))
            self.assertEqual(json.loads(catalog)["providers"]["agentkv"]["baseUrl"], "http://router/v1")

    def test_pi_command_selects_provider_and_passes_api_key(self):
        command = pi_coder.pi_command("agentkv-qwen", "repair clamp", "secret")
        self.assertEqual(command[:4], ["pi", "--offline", "--approve", "--mode"])
        self.assertIn("--provider", command)
        self.assertEqual(command[command.index("--provider") + 1], "agentkv")
        self.assertEqual(command[command.index("--model") + 1], "agentkv-qwen")
        self.assertIn("--api-key", command)
        self.assertEqual(command[command.index("--api-key") + 1], "secret")
        self.assertEqual(command[-2:], ["--", "repair clamp"])

    def test_runtime_disables_packages_and_install_telemetry(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            spec = pi_coder.models_file("http://router", "secret", "agentkv-qwen")
            pi_coder.write_runtime(home, spec)
            settings = json.loads((home / ".pi" / "agent" / "settings.json").read_text())
            self.assertEqual(settings["packages"], [])
            self.assertEqual(settings["extensions"], [])
            self.assertEqual(settings["defaultProjectTrust"], "always")
            self.assertFalse(settings["enableInstallTelemetry"])

    def test_pi_env_is_offline_and_drops_injected_openai_base(self):
        env = pi_coder.pi_env({"OPENAI_BASE_URL": "http://nasiko/v1", "PATH": "/custom", "HOME": "/tmp/pi"})
        self.assertEqual(env["PI_OFFLINE"], "1")
        self.assertEqual(env["PI_SKIP_VERSION_CHECK"], "1")
        self.assertEqual(env["PI_TELEMETRY"], "0")
        self.assertNotIn("OPENAI_BASE_URL", env)
        self.assertTrue(env["PATH"].startswith("/usr/local/bin:"))

    def test_proxy_forces_non_stream_and_wraps_sse_for_pi(self):
        body = pi_coder.prepare_completion_body(
            {"stream": True, "stream_options": {"include_usage": True}, "max_tokens": 4096},
            "agentkv-client")
        self.assertFalse(body["stream"])
        self.assertNotIn("stream_options", body)
        self.assertEqual(body["max_tokens"], 512)
        self.assertEqual(body["request_id"], "agentkv-client")
        wrapped = pi_coder.sse_wrap(b'{"id":"cmpl-1"}')
        self.assertEqual(wrapped, b'data: {"id":"cmpl-1"}\n\ndata: [DONE]\n\n')
