import asyncio
import json
from pathlib import Path

from app.service.agent_upload_service import AgentUploadService
from app.service.service import Service


class _NoopLogger:
    def info(self, msg, *args, **kwargs):
        return None

    def warning(self, msg, *args, **kwargs):
        return None

    def error(self, msg, *args, **kwargs):
        return None

    def debug(self, msg, *args, **kwargs):
        return None


class _AssociationRepo:
    def __init__(self, registry_doc):
        self.registry_doc = registry_doc

    async def get_registry_by_agent_id(self, agent_id):
        if self.registry_doc.get("id") == agent_id:
            return self.registry_doc
        return None

    async def update_registry(self, db_id, update_data):
        _ = db_id
        self.registry_doc.update(update_data)
        return self.registry_doc


def _write_file(path: Path, content: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def _create_valid_mcp_server(base_dir: Path):
    _write_file(
        base_dir / "Dockerfile",
        "FROM python:3.12-slim\nWORKDIR /app\nCOPY . .\nCMD [\"python\", \"src/main.py\"]\n",
    )
    _write_file(
        base_dir / "docker-compose.yml",
        "services:\n  weather-server:\n    build: .\n    container_name: weather-server\n",
    )
    _write_file(
        base_dir / "src" / "main.py",
        '''
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("weather-server")


@mcp.tool()
def get_weather(city: str) -> str:
    """Get weather for a city."""
    return f"Sunny in {city}"


@mcp.resource("weather://cities")
def list_cities() -> str:
    """List available cities."""
    return "Bengaluru, London"


@mcp.prompt()
def weather_prompt(topic: str) -> str:
    """Prompt template for weather questions."""
    return f"You are a weather assistant for {topic}."
'''.strip()
        + "\n",
    )


def test_upload_valid_mcp_server_generates_manifest(tmp_path):
    source = tmp_path / "weather-mcp"
    _create_valid_mcp_server(source)

    upload_service = AgentUploadService(_NoopLogger())
    upload_service.agents_directory = tmp_path / "agents"

    result = asyncio.run(
        upload_service.process_directory_upload(str(source), agent_name="weather-mcp")
    )

    assert result.success is True
    assert result.is_mcp is True
    assert result.manifest is not None
    assert len(result.manifest.get("tools", [])) >= 1
    assert len(result.manifest.get("resources", [])) >= 1
    assert len(result.manifest.get("prompts", [])) >= 1

    manifest_path = (
        upload_service.agents_directory
        / "weather-mcp"
        / result.version
        / "McpServerManifest.json"
    )
    assert manifest_path.exists()

    manifest_on_disk = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest_on_disk.get("name") == "weather-mcp"
    assert len(manifest_on_disk.get("tools", [])) >= 1


def test_upload_missing_main_py_returns_clear_error(tmp_path):
    source = tmp_path / "broken-mcp"
    _write_file(
        source / "Dockerfile",
        "FROM python:3.12-slim\nWORKDIR /app\nCOPY . .\n",
    )
    _write_file(
        source / "docker-compose.yml",
        "services:\n  broken-mcp:\n    build: .\n    container_name: broken-mcp\n",
    )

    upload_service = AgentUploadService(_NoopLogger())
    upload_service.agents_directory = tmp_path / "agents"

    result = asyncio.run(
        upload_service.process_directory_upload(str(source), agent_name="broken-mcp")
    )

    assert result.success is False
    assert result.status == "validation_failed"
    assert any("main.py entry point not found" in err for err in result.validation_errors)


def test_ambiguous_agent_and_mcp_detection_fails_loudly(tmp_path):
    source = tmp_path / "ambiguous"
    _write_file(
        source / "Dockerfile",
        "FROM python:3.12-slim\nWORKDIR /app\nCOPY . .\n",
    )
    _write_file(
        source / "docker-compose.yml",
        "services:\n  ambiguous:\n    build: .\n    container_name: ambiguous\n",
    )
    _write_file(
        source / "src" / "main.py",
        '''
from langchain import PromptTemplate
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("ambiguous")


@mcp.tool()
def run_tool(query: str) -> str:
    return PromptTemplate.from_template("{q}").format(q=query)
'''.strip()
        + "\n",
    )

    upload_service = AgentUploadService(_NoopLogger())
    upload_service.agents_directory = tmp_path / "agents"

    result = asyncio.run(
        upload_service.process_directory_upload(str(source), agent_name="ambiguous")
    )

    assert result.success is False
    assert result.status == "detection_failed"
    assert any("Ambiguous artifact" in err for err in result.validation_errors)


def test_mcp_discovery_and_association_flow(tmp_path):
    agents_root = tmp_path / "agents"

    weather = agents_root / "weather-server"
    math = agents_root / "math-server"
    _create_valid_mcp_server(weather)
    _create_valid_mcp_server(math)

    weather_manifest = {
        "name": "weather-server",
        "version": "1.0.0",
        "tools": [{"name": "get_weather", "description": "..."}],
        "resources": [{"name": "list_cities", "description": "..."}],
        "prompts": [{"name": "weather_prompt", "description": "..."}],
    }
    math_manifest = {
        "name": "math-server",
        "version": "1.0.0",
        "tools": [{"name": "add", "description": "..."}],
        "resources": [],
        "prompts": [],
    }

    _write_file(
        weather / "McpServerManifest.json", json.dumps(weather_manifest, indent=2)
    )
    _write_file(math / "McpServerManifest.json", json.dumps(math_manifest, indent=2))

    repo = _AssociationRepo(
        {
            "_id": "db-1",
            "id": "agent-1",
            "associated_mcp_servers": ["weather-server"],
            "mcp_bridge_urls": {
                "weather-server": "http://localhost:9100/router/mcp/weather-server/tool"
            },
        }
    )

    service = Service(repo=repo, logger=_NoopLogger())
    service._agents_directory = lambda: agents_root

    servers = asyncio.run(service.list_mcp_servers())
    server_ids = {server["server_id"] for server in servers}
    assert "weather-server" in server_ids
    assert "math-server" in server_ids

    assoc = asyncio.run(
        service.associate_agent_with_mcp(
            agent_id="agent-1",
            mcp_server_ids=["math-server"],
            replace=False,
        )
    )
    assert set(assoc["associated_mcp_servers"]) == {"weather-server", "math-server"}

    assoc_replaced = asyncio.run(
        service.associate_agent_with_mcp(
            agent_id="agent-1",
            mcp_server_ids=["math-server"],
            replace=True,
        )
    )
    assert assoc_replaced["associated_mcp_servers"] == ["math-server"]
