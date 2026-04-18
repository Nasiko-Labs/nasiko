"""Tests for remote MCP server registration by URL (stretch requirement)."""

import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

# Make CLI modules importable
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "cli"))


def test_register_remote_mcp_unreachable_url_gives_clear_error(monkeypatch, capsys):
    """Unreachable remote URL must produce a clear registration-time validation error."""
    import requests as _requests

    def _raise_connection_error(*args, **kwargs):
        raise _requests.exceptions.ConnectionError("Connection refused")

    monkeypatch.setattr("cli.commands.registry.requests.get", _raise_connection_error)

    from cli.commands.registry import register_remote_mcp_command

    register_remote_mcp_command("http://192.0.2.1:9999", name="bad-server")

    output = capsys.readouterr().out
    assert "Could not connect" in output, "Must show connection error message"
    assert "192.0.2.1:9999" in output, "Must mention the unreachable URL"


def test_register_remote_mcp_timeout_gives_clear_error(monkeypatch, capsys):
    """Timeout to remote URL must produce a clear error."""
    import requests as _requests

    def _raise_timeout(*args, **kwargs):
        raise _requests.exceptions.Timeout("Timed out")

    monkeypatch.setattr("cli.commands.registry.requests.get", _raise_timeout)

    from cli.commands.registry import register_remote_mcp_command

    register_remote_mcp_command("http://slow-server.example.com:8080", name="slow")

    output = capsys.readouterr().out
    assert "timed out" in output.lower(), "Must show timeout error"


def test_register_remote_mcp_server_error_gives_clear_error(monkeypatch, capsys):
    """Remote server returning 500+ must produce a clear error."""
    mock_response = MagicMock()
    mock_response.status_code = 503

    monkeypatch.setattr(
        "cli.commands.registry.requests.get", lambda *a, **kw: mock_response
    )

    from cli.commands.registry import register_remote_mcp_command

    register_remote_mcp_command("http://broken.example.com", name="broken")

    output = capsys.readouterr().out
    assert "503" in output, "Must include the HTTP status code"


def test_register_remote_mcp_success_with_manifest(monkeypatch, capsys):
    """Successful registration with manifest discovery must create registry entry."""
    call_log = []

    # Health probe succeeds
    health_resp = MagicMock()
    health_resp.status_code = 200

    # Manifest discovery succeeds on /manifest
    manifest_resp = MagicMock()
    manifest_resp.status_code = 200
    manifest_resp.json.return_value = {
        "name": "remote-tools",
        "schemaVersion": "1.0",
        "tools": [{"name": "calculator", "description": "Math ops"}],
        "resources": [],
        "prompts": [],
    }

    def mock_get(url, **kwargs):
        if "/health" in url:
            return health_resp
        if "/manifest" in url:
            return manifest_resp
        not_found = MagicMock()
        not_found.status_code = 404
        return not_found

    monkeypatch.setattr("cli.commands.registry.requests.get", mock_get)

    # Mock the API client for upsert
    mock_upsert_resp = MagicMock()
    mock_upsert_resp.status_code = 200
    mock_upsert_resp.raise_for_status = MagicMock()

    mock_client = MagicMock()
    mock_client.put.return_value = mock_upsert_resp

    monkeypatch.setattr(
        "cli.commands.registry.get_api_client", lambda: mock_client
    )

    from cli.commands.registry import register_remote_mcp_command

    register_remote_mcp_command("http://remote-mcp.example.com:8080")

    output = capsys.readouterr().out
    assert "registered successfully" in output, "Must confirm success"
    assert "Discovered MCP manifest" in output, "Must show manifest discovery"

    # Verify upsert was called with correct data
    assert mock_client.put.called, "Must call API to upsert registry entry"
    call_args = mock_client.put.call_args
    data = call_args.kwargs.get("data") or call_args[1].get("data")
    assert data["artifact_type"] == "mcp_server"
    assert data["deployment_type"] == "remote"
    assert len(data["mcp_manifest"]["tools"]) == 1


def test_register_remote_mcp_no_manifest_creates_minimal_entry(monkeypatch, capsys):
    """If no manifest is discovered, a minimal entry must be created."""
    # Health probe succeeds
    health_resp = MagicMock()
    health_resp.status_code = 200

    # All manifest probes fail
    not_found = MagicMock()
    not_found.status_code = 404

    # JSON-RPC probe also fails
    rpc_fail = MagicMock()
    rpc_fail.status_code = 404

    def mock_get(url, **kwargs):
        if "/health" in url:
            return health_resp
        return not_found

    def mock_post(url, **kwargs):
        return rpc_fail

    monkeypatch.setattr("cli.commands.registry.requests.get", mock_get)
    monkeypatch.setattr("cli.commands.registry.requests.post", mock_post)

    mock_upsert_resp = MagicMock()
    mock_upsert_resp.status_code = 200
    mock_upsert_resp.raise_for_status = MagicMock()

    mock_client = MagicMock()
    mock_client.put.return_value = mock_upsert_resp

    monkeypatch.setattr(
        "cli.commands.registry.get_api_client", lambda: mock_client
    )

    from cli.commands.registry import register_remote_mcp_command

    register_remote_mcp_command("http://simple.example.com", name="simple-mcp")

    output = capsys.readouterr().out
    assert "No manifest discovered" in output, "Must warn about missing manifest"
    assert "registered successfully" in output, "Must still register"

    call_args = mock_client.put.call_args
    data = call_args.kwargs.get("data") or call_args[1].get("data")
    assert data["artifact_type"] == "mcp_server"
    assert data["mcp_manifest"]["transport"]["type"] == "http"
