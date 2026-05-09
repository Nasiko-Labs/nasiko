from __future__ import annotations

import logging
import time
from dataclasses import dataclass
from typing import Iterable

import redis

logger = logging.getLogger(__name__)

TARGET_INDEX_KEY = "request-manager:targets"
TARGET_KEY_PREFIX = "request-manager:targets:"


@dataclass(frozen=True)
class AgentTargetRecord:
    agent_id: str
    public_path: str
    upstream_url: str
    target_revision: str
    source: str
    namespace: str
    updated_at: float

    def to_redis_hash(self) -> dict[str, str]:
        return {
            "agent_id": self.agent_id,
            "public_path": self.public_path,
            "upstream_url": self.upstream_url,
            "target_revision": self.target_revision,
            "source": self.source,
            "namespace": self.namespace,
            "updated_at": str(self.updated_at),
        }


def build_target_record(
    agent_id: str,
    host: str,
    port: int,
    public_path: str,
    namespace: str,
    source: str,
    target_revision: str,
    now: float | None = None,
) -> AgentTargetRecord:
    return AgentTargetRecord(
        agent_id=agent_id,
        public_path=public_path,
        upstream_url=f"http://{host}:{port}",
        target_revision=target_revision,
        source=source,
        namespace=namespace,
        updated_at=time.time() if now is None else now,
    )


class RedisTargetPublisher:
    def __init__(self, redis_url: str, socket_timeout: float = 1.0) -> None:
        self.client = redis.Redis.from_url(
            redis_url,
            decode_responses=True,
            socket_timeout=socket_timeout,
            socket_connect_timeout=socket_timeout,
        )

    def publish(self, records: Iterable[AgentTargetRecord]) -> None:
        records = list(records)
        pipeline = self.client.pipeline()
        active_ids = {record.agent_id for record in records}

        for record in records:
            pipeline.hset(
                f"{TARGET_KEY_PREFIX}{record.agent_id}",
                mapping=record.to_redis_hash(),
            )
            pipeline.sadd(TARGET_INDEX_KEY, record.agent_id)

        existing_ids = self.client.smembers(TARGET_INDEX_KEY)
        stale_ids = set(existing_ids) - active_ids
        for stale_id in stale_ids:
            pipeline.delete(f"{TARGET_KEY_PREFIX}{stale_id}")
            pipeline.srem(TARGET_INDEX_KEY, stale_id)

        pipeline.execute()
        logger.info("Published %s request-manager target records", len(records))
