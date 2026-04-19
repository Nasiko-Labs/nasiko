import asyncio
import textwrap
from pathlib import Path

from app.service.mcp_manifest_generator import MCPManifestGenerator
from orchestrator.mcp_bridge_service import MCPBridgeService


def _write_file(path: Path, content: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def test_example_manifest_counts_not_duplicated():
    manifest = asyncio.run(
        MCPManifestGenerator().generate_manifest(
            "examples/mcp-weather-server",
            "weather-server",
        )
    )

    assert manifest is not None
    assert len(manifest.get("tools", [])) == 3
    assert len(manifest.get("resources", [])) == 2
    assert len(manifest.get("prompts", [])) == 1

    tool_names = {item["name"] for item in manifest.get("tools", [])}
    assert tool_names == {
        "get_weather",
        "forecast_weather",
        "alert_weather_changes",
    }


def test_async_decorated_tool_is_discovered(tmp_path):
    server_dir = tmp_path / "async-server"
    _write_file(
        server_dir / "src" / "main.py",
        textwrap.dedent(
            '''
            from mcp.server.fastmcp import FastMCP

            mcp = FastMCP("async-server")

            @mcp.tool()
            async def async_ping(message: str) -> str:
                """Async ping tool."""
                return message
            '''
        ).strip()
        + "\n",
    )

    manifest = asyncio.run(
        MCPManifestGenerator().generate_manifest(str(server_dir), "async-server")
    )

    assert manifest is not None
    tools = manifest.get("tools", [])
    assert len(tools) == 1
    assert tools[0]["name"] == "async_ping"
    assert "message" in tools[0].get("parameters", {})


def test_bridge_skips_non_json_stdout_noise(tmp_path):
    server_dir = tmp_path / "noisy-mcp"

    _write_file(
        server_dir / "Dockerfile",
        "FROM python:3.12-slim\nWORKDIR /app\nCOPY . .\nCMD [\"python\", \"src/main.py\"]\n",
    )
    _write_file(
        server_dir / "docker-compose.yml",
        "services:\n  noisy-mcp:\n    build: .\n",
    )
    _write_file(
        server_dir / "src" / "main.py",
        textwrap.dedent(
            '''
            import json
            import sys

            while True:
                line = sys.stdin.readline()
                if not line:
                    break

                req = json.loads(line)
                method = req.get("method")
                req_id = req.get("id")

                if method == "initialize":
                    print("NOISE initialize", flush=True)
                    print(
                        json.dumps(
                            {
                                "jsonrpc": "2.0",
                                "id": req_id,
                                "result": {
                                    "protocolVersion": "2024-11-05",
                                    "capabilities": {},
                                    "serverInfo": {"name": "noisy", "version": "1.0.0"},
                                },
                            }
                        ),
                        flush=True,
                    )
                elif method == "notifications/initialized":
                    print("NOISE initialized", flush=True)
                elif method == "tools/call":
                    print("NOISE tool call", flush=True)
                    args = req.get("params", {}).get("arguments", {})
                    print(
                        json.dumps(
                            {
                                "jsonrpc": "2.0",
                                "id": req_id,
                                "result": {"echo": args.get("message"), "ok": True},
                            }
                        ),
                        flush=True,
                    )
                elif req_id is not None:
                    print(
                        json.dumps(
                            {
                                "jsonrpc": "2.0",
                                "id": req_id,
                                "error": {"code": -32601, "message": "Method not found"},
                            }
                        ),
                        flush=True,
                    )
            '''
        ).strip()
        + "\n",
    )

    bridge = MCPBridgeService(str(server_dir))
    started = asyncio.run(bridge.start())
    assert started is True

    try:
        response = asyncio.run(bridge.call_tool("echo", {"message": "hello"}))
        assert response.success is True
        assert response.result == {"echo": "hello", "ok": True}
    finally:
        asyncio.run(bridge.stop())
