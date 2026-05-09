import time

from target_publisher import AgentTargetRecord, build_target_record


def test_build_target_record_uses_internal_url_and_revision():
    record = build_target_record(
        agent_id="agent-a2a-demo",
        host="agent-a2a-demo",
        port=5000,
        public_path="/agents/agent-a2a-demo",
        namespace="docker-agents",
        source="docker",
        target_revision="container-123",
        now=123.4,
    )

    assert record.agent_id == "agent-a2a-demo"
    assert record.public_path == "/agents/agent-a2a-demo"
    assert record.upstream_url == "http://agent-a2a-demo:5000"
    assert record.target_revision == "container-123"
    assert record.source == "docker"
    assert record.updated_at == 123.4


def test_agent_target_record_serializes_for_redis_hash():
    record = AgentTargetRecord(
        agent_id="agent-a2a-demo",
        public_path="/agents/agent-a2a-demo",
        upstream_url="http://agent-a2a-demo:5000",
        target_revision="container-123",
        source="docker",
        namespace="docker-agents",
        updated_at=time.time(),
    )

    payload = record.to_redis_hash()

    assert payload["agent_id"] == "agent-a2a-demo"
    assert payload["upstream_url"] == "http://agent-a2a-demo:5000"
    assert "updated_at" in payload
