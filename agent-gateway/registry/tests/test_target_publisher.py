import sys
import time
from pathlib import Path

REGISTRY_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REGISTRY_DIR))

from target_publisher import AgentTargetRecord, RedisTargetPublisher, build_target_record


class FakePipeline:
    def __init__(self):
        self.commands = []

    def hset(self, key, mapping):
        self.commands.append(("hset", key, mapping))

    def sadd(self, key, member):
        self.commands.append(("sadd", key, member))

    def delete(self, key):
        self.commands.append(("delete", key))

    def srem(self, key, member):
        self.commands.append(("srem", key, member))

    def execute(self):
        self.commands.append(("execute",))


class FakeRedisClient:
    def __init__(self, existing_ids):
        self.existing_ids = existing_ids
        self.pipeline_instance = FakePipeline()
        self.smembers_called = False

    def pipeline(self):
        return self.pipeline_instance

    def smembers(self, key):
        self.smembers_called = True
        return self.existing_ids


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


def test_empty_publish_does_not_delete_existing_targets():
    publisher = RedisTargetPublisher.__new__(RedisTargetPublisher)
    client = FakeRedisClient(existing_ids={"agent-a2a-demo"})
    publisher.client = client

    publisher.publish([])

    assert client.smembers_called is False
    assert client.pipeline_instance.commands == [("execute",)]
