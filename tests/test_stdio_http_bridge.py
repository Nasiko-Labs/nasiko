"""Tests for the stdio-to-HTTP bridge injection and template correctness."""

import sys
import textwrap
import tempfile
from pathlib import Path

# Orchestrator lives in its own directory
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "orchestrator"))


def test_bridge_template_is_valid_python():
    """The bridge template must be parsable Python (no syntax errors)."""
    from instrumentation_injector import InstrumentationInjector

    injector = InstrumentationInjector()
    template = injector.stdio_http_bridge_template
    assert template, "Bridge template must not be empty"
    compile(template, "<bridge_template>", "exec")  # SyntaxError on failure


def test_bridge_template_contains_required_components():
    """Bridge template must contain health endpoint, JSON-RPC handling, and entrypoint discovery."""
    from instrumentation_injector import InstrumentationInjector

    injector = InstrumentationInjector()
    template = injector.stdio_http_bridge_template

    # Health endpoint for Kong
    assert "/health" in template, "Bridge must expose /health for Kong health checks"
    # JSON-RPC error codes
    assert "-32700" in template, "Bridge must handle JSON parse errors (-32700)"
    assert "-32603" in template, "Bridge must handle internal errors (-32603)"
    # Entrypoint discovery
    assert "src/main.py" in template, "Bridge must search for src/main.py"
    # Port configuration
    assert "MCP_BRIDGE_PORT" in template, "Bridge must read MCP_BRIDGE_PORT env var"
    # Subprocess management
    assert "create_subprocess_exec" in template, "Bridge must use asyncio subprocess"
    # Signal handling for graceful shutdown
    assert "SIGTERM" in template, "Bridge must handle SIGTERM for graceful shutdown"


def test_inject_stdio_http_bridge_writes_file():
    """inject_stdio_http_bridge must write the bridge file to the build context root."""
    from instrumentation_injector import InstrumentationInjector

    injector = InstrumentationInjector()

    with tempfile.TemporaryDirectory() as tmp:
        agent_path = Path(tmp) / "my-mcp-server"
        agent_path.mkdir()

        result = injector.inject_stdio_http_bridge(agent_path, "my-mcp-server")

        assert result is True
        bridge_file = agent_path / "mcp_stdio_http_bridge.py"
        assert bridge_file.exists(), "Bridge file must be created in agent directory"
        content = bridge_file.read_text()
        assert len(content) > 100, "Bridge file must have substantial content"
        assert "if __name__" in content, "Bridge must have __main__ guard"


def test_bridge_not_injected_for_regular_agents():
    """The bridge injection method must only be called explicitly — not for regular agents."""
    from instrumentation_injector import InstrumentationInjector

    injector = InstrumentationInjector()

    with tempfile.TemporaryDirectory() as tmp:
        agent_path = Path(tmp) / "my-agent"
        agent_path.mkdir()
        src_dir = agent_path / "src"
        src_dir.mkdir()
        (src_dir / "main.py").write_text("print('hello')")

        # Regular langtrace injection does NOT write bridge
        injector.inject_langtrace_config(agent_path, "my-agent", artifact_type="agent")

        bridge_file = agent_path / "mcp_stdio_http_bridge.py"
        assert not bridge_file.exists(), "Bridge must NOT be injected for regular agents"


def test_docker_cmd_override_for_mcp_servers():
    """The Redis stream listener must append bridge CMD override for MCP servers."""
    # Simulate the docker_cmd building logic from redis_stream_listener.py
    def build_docker_cmd(image_tag, artifact_type):
        docker_cmd = ["docker", "run", "-d", "--name", "test-container"]
        docker_cmd.append(image_tag)
        if artifact_type == "mcp_server":
            docker_cmd.extend(["python", "mcp_stdio_http_bridge.py"])
        return docker_cmd

    agent_cmd = build_docker_cmd("my-agent_instrumented", "agent")
    assert agent_cmd[-1] == "my-agent_instrumented", "Agent CMD must not be overridden"

    mcp_cmd = build_docker_cmd("my-mcp_instrumented", "mcp_server")
    assert mcp_cmd[-1] == "mcp_stdio_http_bridge.py", "MCP CMD must use bridge"
    assert mcp_cmd[-2] == "python", "MCP CMD must run Python"
    assert mcp_cmd[-3] == "my-mcp_instrumented", "Image tag must precede CMD override"
