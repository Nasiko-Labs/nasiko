import asyncio
import logging
from pathlib import Path

from app.service.agent_upload_service import AgentUploadService
from app.service.agentcard_service import AgentCardService


def write_minimal_project(root: Path, main_source: str):
    (root / "src").mkdir()
    (root / "src" / "main.py").write_text(main_source)
    (root / "Dockerfile").write_text("FROM python:3.11-slim\n")
    (root / "docker-compose.yml").write_text(
        "services:\n"
        "  weather-agent:\n"
        "    container_name: weather-agent\n"
        "    build: .\n"
    )


def test_valid_fastmcp_server_detection_and_manifest_fallback(tmp_path):
    write_minimal_project(
        tmp_path,
        """
from mcp.server.fastmcp import FastMCP
mcp = FastMCP("weather-mcp")

@mcp.tool()
def get_weather(location: str) -> str:
    \"\"\"Returns weather for a city.\"\"\"
    return f"Weather for {location} is Sunny"

if __name__ == "__main__":
    mcp.run()
""",
    )

    service = AgentUploadService(logging.getLogger("test"))
    validation = asyncio.run(service.validate_agent_structure(str(tmp_path)))

    assert validation.is_valid
    assert validation.artifact_type == "mcp_server"

    card_service = AgentCardService(logging.getLogger("test"), openai_api_key="")
    assert card_service._generate_and_save_mcp_manifest_fallback(
        str(tmp_path), "weather-agent"
    )

    manifest = (tmp_path / "McpServerManifest.json").read_text()
    assert '"artifact_type": "mcp_server"' in manifest
    assert '"name": "get_weather"' in manifest
    assert '"location"' in manifest


def test_missing_main_py_upload_validation_fails(tmp_path):
    (tmp_path / "Dockerfile").write_text("FROM python:3.11-slim\n")
    (tmp_path / "docker-compose.yml").write_text(
        "services:\n  weather-agent:\n    container_name: weather-agent\n"
    )

    service = AgentUploadService(logging.getLogger("test"))
    validation = asyncio.run(service.validate_agent_structure(str(tmp_path)))

    assert not validation.is_valid
    assert any("main.py entry point not found" in error for error in validation.errors)


def test_ambiguous_agent_and_mcp_detection_fails(tmp_path):
    write_minimal_project(
        tmp_path,
        """
from mcp.server.fastmcp import FastMCP
from langchain.tools import tool

mcp = FastMCP("ambiguous")

@mcp.tool()
def echo(value: str) -> str:
    return value
""",
    )

    service = AgentUploadService(logging.getLogger("test"))
    validation = asyncio.run(service.validate_agent_structure(str(tmp_path)))

    assert not validation.is_valid
    assert validation.artifact_type == "mcp_server"
    assert any("Ambiguous artifact" in error for error in validation.errors)
