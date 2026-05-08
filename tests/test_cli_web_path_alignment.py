from cli.commands.chat_send import build_agent_invoke_url


def test_cli_chat_invoke_flow_equals_web_agent_path():
    assert (
        build_agent_invoke_url("http://localhost:9100", "agent-abc", "agent")
        == "http://localhost:9100/agents/agent-abc"
    )


def test_cli_chat_invoke_flow_equals_web_mcp_path():
    assert (
        build_agent_invoke_url("http://localhost:9100", "mcp-abc", "mcp_server")
        == "http://localhost:9100/mcp/mcp-abc"
    )
