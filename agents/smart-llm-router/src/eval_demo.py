#!/usr/bin/env python3
"""Run the Smart LLM Router evaluation criteria against ROUTER_BASE_URL.

Prefer:  python -m src.eval_demo   (from agents/smart-llm-router with PYTHONPATH=.)
Also:    python -m src.eval_demo   after `pip install -e .`
"""

from __future__ import annotations

import asyncio
import os
import sys
import uuid
from pathlib import Path

from dotenv import load_dotenv

# Allow `python src/eval_demo.py` by putting the agent root on sys.path and
# re-executing as a package module (relative imports require a package context).
if __package__ is None:
    root = Path(__file__).resolve().parents[1]
    sys.path.insert(0, str(root))
    load_dotenv(override=True)
    raise SystemExit(asyncio.run(__import__("src.eval_demo", fromlist=["main"]).main()))

load_dotenv(override=True)

from .agent import SmartLlmRouterAgent
from .classify import parse_classifier_output
from .format_out import format_error
from .router_client import RouterError, RouterSuccess, router_base_url
from .session_store import STORE
from .tier import decide_tier


def _ok(name: str, cond: bool, detail: str = "") -> bool:
    mark = "PASS" if cond else "FAIL"
    suffix = f" — {detail}" if detail else ""
    print(f"[{mark}] {name}{suffix}")
    return cond


async def main() -> int:
    base = router_base_url()
    print(f"ROUTER_BASE_URL={base}")
    print()

    agent = SmartLlmRouterAgent()
    session = str(uuid.uuid4())
    STORE.reset(session)
    passed = 0
    total = 6

    # 1. Trivial → cheap (+ groq when stub / live cascade)
    t1 = await agent.run_turn("hi", session_id=session, source="eval")
    c1 = (
        t1.ok
        and t1.requested_tier == "cheap"
        and isinstance(t1.router, RouterSuccess)
        and float(t1.router.usage.get("cost_usd") or 0) < 0.001
    )
    # Prefer groq; accept stub/cascade provider that still used cheap tier.
    provider_ok = isinstance(t1.router, RouterSuccess) and (
        t1.router.provider == "groq" or t1.router.tier_used == "cheap"
    )
    detail_cost = None
    if isinstance(t1.router, RouterSuccess):
        detail_cost = t1.router.usage.get("cost_usd")
    if _ok(
        "1. trivial 'hi' → cheap, low cost",
        c1 and provider_ok,
        f"tier={t1.requested_tier} provider={getattr(t1.router, 'provider', None)} cost={detail_cost}",
    ):
        passed += 1
    if isinstance(t1.router, RouterSuccess) and t1.router.provider != "groq":
        print(
            "      note: expected provider=groq on stub/live cheap cascade; "
            f"got {t1.router.provider!r} (still cheap-tier OK for cost demo)"
        )

    # 2. Proof → premium
    t2 = await agent.run_turn(
        "Prove the Pythagorean theorem.",
        session_id=session,
        source="eval",
    )
    c2 = (
        t2.ok
        and t2.requested_tier == "premium"
        and isinstance(t2.router, RouterSuccess)
        and t2.router.provider in {"openai", "anthropic", "gemini", "nvidia"}
    )
    if _ok(
        "2. Pythagorean proof → premium",
        c2,
        f"tier={t2.requested_tier} provider={getattr(t2.router, 'provider', None)}",
    ):
        passed += 1

    # 3. Malformed classifier → chat/3 → balanced, still works
    bad = parse_classifier_output("NOT JSON at all {{{")
    assert bad.task_type == "chat" and bad.complexity == 3
    tier = decide_tier(bad.task_type, bad.complexity)
    t3 = await agent.run_turn(
        "tell me a fun fact",
        session_id=session,
        force_classification=bad,
        source="eval",
    )
    c3 = t3.ok and tier == "balanced" and isinstance(t3.router, RouterSuccess)
    if _ok(
        "3. malformed classifier → chat/3/balanced still works",
        c3,
        f"tier={tier} ok={t3.ok}",
    ):
        passed += 1

    # 4. Router error surfaces cleanly (empty body → 400 from Nasiko)
    import httpx

    async with httpx.AsyncClient(timeout=10.0) as http:
        resp = await http.post(
            f"{base}/v1/route",
            headers={"Content-Type": "application/json"},
            json={},
        )
        data = resp.json()
    err_obj = data.get("error") or {}
    formatted = format_error(
        RouterError(
            status=resp.status_code,
            code=str(err_obj.get("code") or "unknown"),
            message=str(err_obj.get("message") or "error"),
            provider=err_obj.get("provider"),
            raw=data,
        )
    )
    c4 = (
        resp.status_code == 400
        and err_obj.get("code") == "bad_request"
        and "bad_request" in formatted
        and "Traceback" not in formatted
    )
    if _ok(
        "4. router 4xx → clear error.code/message (no stack)",
        c4,
        formatted.strip().splitlines()[0] if formatted else "",
    ):
        passed += 1

    # 5. Session savings increment
    totals = t3.totals
    c5 = (
        totals is not None
        and totals.running_calls >= 3
        and totals.running_saved_usd >= 0
        and (t1.totals is None or totals.running_calls > (t1.totals.running_calls if t1.totals else 0))
    )
    # Stronger: calls increased across turns in same session
    c5 = (
        t1.totals is not None
        and t2.totals is not None
        and t3.totals is not None
        and t1.totals.running_calls == 1
        and t2.totals.running_calls == 2
        and t3.totals.running_calls == 3
        and t3.totals.running_saved_usd >= t2.totals.running_saved_usd
    )
    if _ok(
        "5. session savings increment across turns",
        c5,
        f"calls={getattr(t3.totals, 'running_calls', None)} "
        f"saved=${getattr(t3.totals, 'running_saved_usd', 0):.6f}",
    ):
        passed += 1

    # 6. Base URL is env-only
    c6 = os.getenv("ROUTER_BASE_URL", "http://localhost:8080").rstrip("/") == base
    if _ok("6. base URL is one env var (ROUTER_BASE_URL)", c6, base):
        passed += 1

    print()
    print(f"{passed}/{total} checks passed")
    if t1.ok:
        print("\n--- sample reply (hi) ---\n")
        print(t1.text)
    return 0 if passed == total else 1


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
