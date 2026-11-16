"""Original reader for Nasiko's authenticated flow API.

Polling establishes lifecycle visibility only. It cannot manufacture engine prefixes,
measure cache savings, or recover intermediate transitions missed between polls.
"""
import asyncio
import hashlib
import json
import re
from urllib.parse import quote

import httpx

from agentkv.control import Event, Store


class NasikoBridge:
    def __init__(self, store: Store, *, owner: str, base_url: str, token: str,
                 transport: httpx.AsyncBaseTransport | None = None):
        if not owner or not token:
            raise ValueError("trusted Nasiko owner and token required")
        self.store, self.owner = store, owner
        self.lock = asyncio.Lock()
        self.client = httpx.AsyncClient(base_url=base_url.rstrip("/"),
            headers={"Authorization": "Bearer " + token}, timeout=3,
            follow_redirects=False, transport=transport)

    async def close(self):
        await self.client.aclose()

    async def sync(self, flow_id: str) -> bool:
        async with self.lock:
            return await self._sync(flow_id)

    async def _sync(self, flow_id: str) -> bool:
        if not re.fullmatch(r"[A-Za-z0-9_.:-]{1,128}", flow_id):
            raise ValueError("invalid flow ID")
        response = await self.client.get("/api/flows/" + quote(flow_id, safe=""))
        response.raise_for_status()
        payload = response.json()
        flow = payload["flow"]
        if flow["user_id"] != self.owner or flow["flow_id"] != flow_id:
            raise ValueError("Nasiko flow identity mismatch")
        status = flow["status"]
        if status not in {"running", "active", "completed", "failed"}:
            raise ValueError("unsupported Nasiko lifecycle status")
        steps = payload.get("steps", [])
        latest = max(steps, key=lambda step: step["step_order"]) if steps else None
        agent = (latest or {}).get("agent_id") or flow.get("root_agent_id") or "nasiko"
        evidence = str((latest or {}).get("output_summary") or "")[:2000]
        signature = hashlib.sha256(json.dumps([
            flow_id, status, (latest or {}).get("id"), (latest or {}).get("status"), evidence
        ], separators=(",", ":")).encode()).hexdigest()
        event_id = "nasiko:" + signature
        current = self.store.snapshot(self.owner, flow_id)
        if current and (current[0].event_id == event_id or current[0].kind in {"completed", "failed"}):
            return False
        if current is None:
            self.store.ingest(self.owner, Event(event_id="nasiko-start:" + hashlib.sha256(flow_id.encode()).hexdigest(),
                flow_id=flow_id, revision=1, kind="started", agent=agent))
            current = self.store.snapshot(self.owner, flow_id)
        kind = status if status in {"completed", "failed"} else "transition"
        return self.store.ingest(self.owner, Event(event_id=event_id, flow_id=flow_id,
            revision=current[0].revision + 1, kind=kind, agent=agent, evidence=evidence))
