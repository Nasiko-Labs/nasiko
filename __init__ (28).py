"""Pydantic v2 models for the MCP Bridge."""

from __future__ import annotations

from datetime import datetime
from typing import Literal

from pydantic import BaseModel


class BridgeConfig(BaseModel):
    """Configuration snapshot persisted to /tmp/nasiko/{artifact_id}/bridge.json.

    Every field is required — no Optional fields.  The model is fully
    JSON-serializable via ``model.model_dump_json()``.
    """

    artifact_id: str
    port: int  # dynamically assigned, 8100-8200
    entry_point: str  # absolute path to agent's main .py
    pid: int  # PID of spawned subprocess
    kong_service_id: str  # UUID returned by Kong Admin API
    kong_route_id: str  # UUID returned by Kong Admin API
    status: Literal["starting", "ready", "failed"]
    created_at: datetime
    bridge_json_path: str  # /tmp/nasiko/{artifact_id}/bridge.json
