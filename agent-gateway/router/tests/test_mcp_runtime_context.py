import base64
from io import BytesIO

from router.src.core.agent_registry import AgentRegistry
from router.src.entities import UserRequest
from router.src.utils.payload_utils import construct_payload


def test_extract_mcp_context_returns_none_without_associations():
    registry = AgentRegistry()

    assert registry.extract_mcp_context(None) is None
    assert registry.extract_mcp_context({"name": "agent-a"}) is None


def test_extract_mcp_context_builds_server_entries():
    registry = AgentRegistry()

    agent_card = {
        "name": "agent-a",
        "associated_mcp_servers": ["weather-server", "math-server"],
        "mcp_bridge_urls": {
            "weather-server": "http://localhost:9100/router/mcp/weather-server/tool",
            "math-server": "http://localhost:9100/router/mcp/math-server/tool",
        },
    }

    context = registry.extract_mcp_context(agent_card)

    assert context is not None
    assert context["associated_server_ids"] == ["weather-server", "math-server"]
    assert context["bridge_urls"]["weather-server"].endswith(
        "/router/mcp/weather-server/tool"
    )
    assert context["servers"][0]["id"] == "weather-server"


def test_construct_payload_includes_mcp_context_metadata_and_files():
    request = UserRequest(session_id="session-1", query="Find weather", route="agent-a")
    files = [
        (
            "files",
            (
                "report.txt",
                BytesIO(b"hello-from-router"),
                "text/plain",
            ),
        )
    ]

    mcp_context = {
        "associated_server_ids": ["weather-server"],
        "bridge_urls": {
            "weather-server": "http://localhost:9100/router/mcp/weather-server/tool"
        },
        "servers": [
            {
                "id": "weather-server",
                "url": "http://localhost:9100/router/mcp/weather-server/tool",
            }
        ],
        "transport": "http_bridge",
    }

    payload = construct_payload(request, files, "http://agent.local", mcp_context=mcp_context)

    assert payload["params"]["metadata"]["route"] == "agent-a"
    assert payload["params"]["metadata"]["mcp"] == mcp_context

    file_part = payload["params"]["message"]["parts"][1]
    assert file_part["kind"] == "file"
    assert file_part["file"]["name"] == "report.txt"
    decoded = base64.b64decode(file_part["file"]["bytes"])  # noqa: S324
    assert decoded == b"hello-from-router"
