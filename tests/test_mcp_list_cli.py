"""Tests for the `nasiko mcp list` CLI command and MCP-specific filtering."""

import sys
from pathlib import Path

# Make CLI modules importable
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "cli"))


def test_list_mcp_servers_filters_by_artifact_type(monkeypatch, capsys):
    """list_mcp_servers_command must only show agents with artifact_type == mcp_server."""
    from unittest.mock import MagicMock

    # Simulate API response with mixed artifact types
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {
        "data": [
            {"name": "my-agent", "id": "a1", "artifact_type": "agent"},
            {"name": "my-mcp-server", "id": "m1", "artifact_type": "mcp_server",
             "mcp_manifest": {"tools": [{"name": "add"}], "resources": [], "prompts": []}},
            {"name": "another-agent", "id": "a2", "artifact_type": "agent"},
            {"name": "another-mcp", "id": "m2", "artifact_type": "mcp_server",
             "mcp_manifest": {"tools": [], "resources": [{"name": "r1"}], "prompts": []}},
        ],
        "message": "success",
    }

    mock_client = MagicMock()
    mock_client.get.return_value = mock_response
    mock_client.handle_response.return_value = mock_response.json()

    monkeypatch.setattr(
        "cli.commands.registry.get_api_client", lambda: mock_client
    )

    from cli.commands.registry import list_mcp_servers_command

    # Call with list format for easy string assertion
    list_mcp_servers_command(format_type="list", show_details=False)

    output = capsys.readouterr().out
    assert "my-mcp-server" in output or "another-mcp" in output, (
        "MCP servers must be shown"
    )
    assert "my-agent" not in output, "Regular agents must NOT appear in mcp list"
    assert "another-agent" not in output, "Regular agents must NOT appear in mcp list"


def test_list_mcp_servers_empty_registry(monkeypatch, capsys):
    """list_mcp_servers_command must handle an empty MCP registry gracefully."""
    from unittest.mock import MagicMock

    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {
        "data": [
            {"name": "my-agent", "id": "a1", "artifact_type": "agent"},
        ],
        "message": "success",
    }

    mock_client = MagicMock()
    mock_client.get.return_value = mock_response
    mock_client.handle_response.return_value = mock_response.json()

    monkeypatch.setattr(
        "cli.commands.registry.get_api_client", lambda: mock_client
    )

    from cli.commands.registry import list_mcp_servers_command

    list_mcp_servers_command(format_type="list")

    output = capsys.readouterr().out
    assert "No MCP servers found" in output, "Must show empty-state message"


def test_mcp_group_is_registered_in_cli():
    """The mcp command group must be importable and have expected commands."""
    from cli.groups.mcp_group import mcp_app

    # Typer stores commands in registered_groups/registered_commands
    command_names = [
        cmd.name or cmd.callback.__name__
        for cmd in mcp_app.registered_commands
    ]

    assert "list" in command_names, "nasiko mcp list must be registered"
    assert "get" in command_names, "nasiko mcp get must be registered"
    assert "register-remote" in command_names, "nasiko mcp register-remote must be registered"
