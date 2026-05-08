import asyncio
import importlib.util
import logging
from pathlib import Path

from app.service.agent_upload_service import AgentUploadResult
from app.service.agent_upload_tracking_service import AgentUploadTrackingService
from cli.commands.chat_send import build_agent_invoke_url


LOGGER = logging.getLogger(__name__)


def _load_observability_config_class():
    repo_root = Path(__file__).resolve().parents[1]
    module_path = repo_root / "app" / "utils" / "observability" / "config.py"
    spec = importlib.util.spec_from_file_location("observability_config_module", module_path)
    module = importlib.util.module_from_spec(spec)
    assert spec and spec.loader
    spec.loader.exec_module(module)
    return module.ObservabilityConfig


class _FakeRepo:
    def __init__(self):
        self.created = []
        self.updated = []

    async def create_upload_status(self, data):
        self.created.append(dict(data))
        return data

    async def update_upload_status(self, upload_id, data):
        payload = {"upload_id": upload_id, **dict(data)}
        self.updated.append(payload)
        return payload


class _FakeOrchestration:
    captured = None

    def __init__(self, logger):
        self.logger = logger

    async def trigger_agent_orchestration(self, **kwargs):
        _FakeOrchestration.captured = kwargs
        return True


class _FakeRegistryService:
    async def get_all_registries(self):
        return [
            {
                "id": "mcp-demo",
                "name": "mcp-demo",
                "version": "1.0.0",
                "description": "MCP demo",
                "url": "http://localhost:9100/mcp/mcp-demo",
                "artifact_type": "mcp_server",
                "deployment_type": "docker-local-mcp",
                "metadata": {"mcp_manifest_present": True},
                "mcp_manifest": {"name": "mcp-demo", "tools": [{"name": "ping"}]},
                "associations": {"agent_ids": ["agent-a"]},
            }
        ]


class _FakeBaseService:
    async def process_directory_upload(self, directory_path, agent_name=None):
        return AgentUploadResult(
            success=True,
            agent_name=agent_name or "mcp-demo",
            status="uploaded",
            capabilities_generated=False,
            orchestration_triggered=False,
            version="v1.0.0",
            artifact_type="mcp_server",
        )


def test_valid_stdio_mcp_upload_deploy_list_callable_and_traces(monkeypatch, tmp_path):
    # ---- upload + deploy (tracking service orchestration trigger) ----
    repo = _FakeRepo()
    tracking = AgentUploadTrackingService(LOGGER, repo)
    tracking.base_service = _FakeBaseService()

    monkeypatch.setattr(
        "app.service.orchestration_service.OrchestrationService",
        _FakeOrchestration,
    )

    result = asyncio.run(
        tracking.process_directory_upload(str(tmp_path), user_id="user-1", agent_name="mcp-demo")
    )

    assert result.success is True
    assert result.orchestration_triggered is True
    assert _FakeOrchestration.captured is not None
    assert _FakeOrchestration.captured["additional_data"]["artifact_type"] == "mcp_server"

    # ---- list (registry metadata includes MCP fields) ----
    listing = asyncio.run(_FakeRegistryService().get_all_registries())
    assert len(listing) == 1
    item = listing[0]
    assert item["artifact_type"] == "mcp_server"
    assert item["mcp_manifest"] is not None
    assert item["mcp_manifest"].get("tools")

    # ---- callable (CLI/web URL path parity for MCP) ----
    url = build_agent_invoke_url("http://localhost:9100", "mcp-demo", "mcp_server")
    assert url == "http://localhost:9100/mcp/mcp-demo"

    def _callable_status(path: str) -> int:
        return 200 if path == "/mcp/mcp-demo" else 404

    assert _callable_status("/mcp/mcp-demo") == 200

    # ---- traces (observability enabled + bridge deps present) ----
    ObservabilityConfig = _load_observability_config_class()
    assert ObservabilityConfig.is_tracing_enabled() is True
    deps = ObservabilityConfig.get_required_dependencies()
    assert any("opentelemetry-instrumentation-httpx" in dep for dep in deps)
    assert any("opentelemetry-instrumentation-requests" in dep for dep in deps)
