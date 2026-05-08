import asyncio
import logging
from pathlib import Path

from app.service.agent_upload_service import AgentUploadService


LOGGER = logging.getLogger(__name__)


def _write_common_files(base: Path, *, include_main: bool = True, main_content: str = ""):
    (base / "Dockerfile").write_text("FROM python:3.12-slim\n", encoding="utf-8")
    (base / "docker-compose.yml").write_text(
        "services:\n  app:\n    container_name: app\n    build: .\n",
        encoding="utf-8",
    )
    (base / "src").mkdir(parents=True, exist_ok=True)
    if include_main:
        (base / "src" / "main.py").write_text(main_content, encoding="utf-8")


def test_mcp_upload_missing_main_py_has_clear_validation_error(tmp_path: Path):
    agent_dir = tmp_path / "missing-main-mcp"
    agent_dir.mkdir(parents=True, exist_ok=True)

    _write_common_files(agent_dir, include_main=False)
    # MCP markers present in non-entrypoint file so artifact is detected as MCP
    (agent_dir / "src" / "server.py").write_text(
        "from fastmcp import FastMCP\n"
        "mcp = FastMCP('demo')\n"
        "@mcp.tool()\n"
        "def ping():\n"
        "    return 'pong'\n",
        encoding="utf-8",
    )

    service = AgentUploadService(LOGGER)
    result = asyncio.run(service.validate_agent_structure(str(agent_dir)))

    assert result.is_valid is False
    joined = "\n".join(result.errors)
    assert "main.py entry point not found" in joined
    assert "MCP server entrypoint not found" in joined


def test_ambiguous_artifact_upload_has_clear_validation_error(tmp_path: Path):
    agent_dir = tmp_path / "ambiguous-agent"
    agent_dir.mkdir(parents=True, exist_ok=True)

    _write_common_files(
        agent_dir,
        include_main=True,
        main_content=(
            "import langchain\n"
            "from fastmcp import FastMCP\n"
            "mcp = FastMCP('demo')\n"
            "@mcp.tool()\n"
            "def ping():\n"
            "    return 'pong'\n"
        ),
    )

    service = AgentUploadService(LOGGER)
    result = asyncio.run(service.validate_agent_structure(str(agent_dir)))

    assert result.is_valid is False
    joined = "\n".join(result.errors)
    assert "Ambiguous artifact detected" in joined
    assert "Agent markers:" in joined
    assert "MCP markers:" in joined


def test_auto_generated_mcp_manifest_is_non_empty_and_correct(tmp_path: Path):
    agent_dir = tmp_path / "valid-mcp"
    agent_dir.mkdir(parents=True, exist_ok=True)

    _write_common_files(
        agent_dir,
        include_main=True,
        main_content=(
            "from fastmcp import FastMCP\n"
            "mcp = FastMCP('demo')\n"
            "@mcp.tool()\n"
            "def convert(amount: float, currency: str):\n"
            "    return f'{amount} {currency}'\n"
        ),
    )

    service = AgentUploadService(LOGGER)
    generated = asyncio.run(service._ensure_mcp_manifest_json(str(agent_dir), "valid-mcp"))
    manifest_path = agent_dir / "MCPManifest.json"

    assert generated is True
    assert manifest_path.exists()

    import json

    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest.get("transport", {}).get("type") == "stdio"
    assert manifest.get("preferredTransport") == "stdio"
    assert isinstance(manifest.get("tools"), list)
    assert len(manifest.get("tools")) > 0
    assert manifest["tools"][0].get("name")


def test_valid_stdio_mcp_upload_processes_and_sets_artifact_type(tmp_path: Path):
    source_dir = tmp_path / "upload-mcp"
    source_dir.mkdir(parents=True, exist_ok=True)

    _write_common_files(
        source_dir,
        include_main=True,
        main_content=(
            "from fastmcp import FastMCP\n"
            "mcp = FastMCP('demo')\n"
            "@mcp.tool()\n"
            "def hello(name: str):\n"
            "    return f'hello {name}'\n"
        ),
    )

    service = AgentUploadService(LOGGER)
    service.agents_directory = tmp_path / "agents-store"

    result = asyncio.run(service.process_directory_upload(str(source_dir), "upload-mcp"))

    assert result.success is True
    assert result.artifact_type == "mcp_server"
    assert result.status == "uploaded"
    # mcp upload generates MCP manifest (not AgentCard)
    copied_manifest = service.agents_directory / "upload-mcp" / result.version / "MCPManifest.json"
    assert copied_manifest.exists()
