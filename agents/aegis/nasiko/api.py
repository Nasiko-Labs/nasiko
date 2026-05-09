"""
Aegis FastAPI service — exposes the firewall as a Nasiko-compatible agent.

Deploy this as a Nasiko agent and every other agent on the platform can
call Aegis to evaluate tool calls before executing them.

For tools in ``approval_required``, the API returns a PENDING verdict
immediately with an approval URL.  The caller can then POST to
``/approve/{call_id}`` or ``/deny/{call_id}`` to resolve it, or let
the 30-second timeout auto-deny.

Endpoints:
    POST /evaluate          — Evaluate a single tool call
    POST /batch-evaluate    — Evaluate multiple tool calls
    POST /approve/{call_id} — Approve a pending tool call
    POST /deny/{call_id}    — Deny a pending tool call
    GET  /policies          — Return current firewall policies
    POST /policies/reload   — Hot-reload policies from disk
    POST /agentcard/load    — Load an AgentCard and auto-configure policies
    GET  /health            — Health check
    GET  /traces            — Return recent verdicts from current session
"""

from __future__ import annotations

import asyncio
import uuid
from typing import Any

from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel, Field

from firewall.models import ToolCall, Decision
from firewall.firewall import firewall
from firewall.risk_engine import score
from firewall.policy_engine import PolicyEngine
from traces.storage import trace_store
from nasiko.phoenix_logger import register as register_phoenix
from nasiko.agentcard_parser import parse_agentcard, agentcard_to_policy_overrides


# ---------------------------------------------------------------------------
# Pydantic schemas
# ---------------------------------------------------------------------------

class EvaluateRequest(BaseModel):
    tool: str = Field(..., description="Name of the tool being called")
    args: dict[str, Any] = Field(default_factory=dict, description="Tool arguments")
    agent: str = Field(default="unknown", description="Name of the calling agent")


class EvaluateResponse(BaseModel):
    call_id: str
    tool: str
    agent: str
    risk_score: float
    decision: str
    reason: str
    violation: str | None = None
    violation_detail: str | None = None
    approval_url: str | None = None


class BatchEvaluateRequest(BaseModel):
    calls: list[EvaluateRequest]


class PolicyResponse(BaseModel):
    blocked_tools: list[str]
    approval_required: list[str]
    allowed_domains: list[str]
    max_risk_score: float


class AgentCardRequest(BaseModel):
    path: str = Field(..., description="Path to AgentCard.json")


class TraceEntry(BaseModel):
    call_id: str
    tool: str
    agent: str
    risk_score: float
    decision: str
    reason: str
    timestamp: str


# ---------------------------------------------------------------------------
# App
# ---------------------------------------------------------------------------

app = FastAPI(
    title="Aegis — Runtime Firewall for AI Agents",
    description=(
        "Intercepts tool calls from Nasiko agents, scores risk, enforces "
        "YAML-defined policies, and blocks dangerous actions before they execute."
    ),
    version="1.0.0",
    docs_url="/docs",
    redoc_url="/redoc",
)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.on_event("startup")
async def _startup():
    register_phoenix()


# ---------------------------------------------------------------------------
# Core evaluation — non-blocking for approval_required tools
# ---------------------------------------------------------------------------

def _evaluate_sync(req: EvaluateRequest) -> EvaluateResponse:
    """
    Evaluate a tool call without blocking on approval.

    Instead of suspending the request for 30s waiting for human input,
    the API returns PENDING immediately so the caller can use the
    /approve or /deny endpoints.
    """
    call = ToolCall(
        tool=req.tool,
        args=req.args,
        agent=req.agent,
        call_id=str(uuid.uuid4())[:8],
    )

    # Score risk
    risk = score(call)

    # Check policy
    policy = firewall._policy
    violation = policy.check(call, risk)

    if violation:
        decision = Decision.BLOCK
    elif policy.needs_approval(call):
        # Don't block — return PENDING with approval URL
        decision = Decision.PENDING
        # Register the approval event so /approve and /deny work
        ev = asyncio.Event()
        firewall._approval_events[call.call_id] = ev
    elif risk.score >= 0.5:
        decision = Decision.WARN
    else:
        decision = Decision.ALLOW

    from firewall.models import FirewallVerdict
    verdict = FirewallVerdict(call, risk, decision, violation)
    trace_store.record(verdict)

    approval_url = None
    if decision == Decision.PENDING:
        approval_url = f"/approve/{call.call_id}"

    return EvaluateResponse(
        call_id=call.call_id,
        tool=call.tool,
        agent=call.agent,
        risk_score=risk.score,
        decision=decision.value,
        reason=risk.reason,
        violation=violation.rule if violation else None,
        violation_detail=violation.detail if violation else None,
        approval_url=approval_url,
    )


# ---------------------------------------------------------------------------
# Endpoints
# ---------------------------------------------------------------------------

@app.post("/evaluate", response_model=EvaluateResponse)
async def evaluate_tool_call(req: EvaluateRequest):
    """
    Evaluate a single tool call through the Aegis firewall.

    If the tool requires approval, returns PENDING with an ``approval_url``
    instead of blocking.  POST to ``/approve/{call_id}`` or
    ``/deny/{call_id}`` to resolve.
    """
    return _evaluate_sync(req)


@app.post("/batch-evaluate", response_model=list[EvaluateResponse])
async def batch_evaluate(req: BatchEvaluateRequest):
    """Evaluate multiple tool calls in one request."""
    return [_evaluate_sync(item) for item in req.calls]


@app.post("/approve/{call_id}")
async def approve_call(call_id: str):
    """Approve a pending tool call."""
    if call_id not in firewall._approval_events:
        raise HTTPException(status_code=404, detail=f"No pending call with id {call_id}")
    firewall.resolve_approval(call_id, True)
    return {"status": "approved", "call_id": call_id}


@app.post("/deny/{call_id}")
async def deny_call(call_id: str):
    """Deny a pending tool call."""
    if call_id not in firewall._approval_events:
        raise HTTPException(status_code=404, detail=f"No pending call with id {call_id}")
    firewall.resolve_approval(call_id, False)
    return {"status": "denied", "call_id": call_id}


@app.get("/policies", response_model=PolicyResponse)
async def get_policies():
    """Return current firewall policy configuration."""
    pe = PolicyEngine()
    p = pe._policy
    return PolicyResponse(
        blocked_tools=p.get("blocked_tools", []),
        approval_required=p.get("approval_required", []),
        allowed_domains=p.get("allowed_domains", []),
        max_risk_score=p.get("max_risk_score", 0.8),
    )


@app.post("/policies/reload")
async def reload_policies():
    """Hot-reload policies from config/policies.yaml."""
    try:
        firewall._policy = PolicyEngine()
        return {"status": "ok", "message": "Policies reloaded from disk"}
    except Exception as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/agentcard/load")
async def load_agentcard(req: AgentCardRequest):
    """
    Parse an AgentCard.json and auto-configure Aegis policies based
    on the agent's declared capabilities.
    """
    try:
        meta = parse_agentcard(req.path)
        overrides = agentcard_to_policy_overrides(meta)
        return {
            "status": "ok",
            "agent": meta["name"],
            "skills": meta["skills"],
            "high_risk_skills": meta["high_risk_skills"],
            "policy_overrides": overrides,
        }
    except FileNotFoundError as e:
        raise HTTPException(status_code=404, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.get("/traces", response_model=list[TraceEntry])
async def get_traces():
    """Return all verdicts from the current session."""
    return [
        TraceEntry(
            call_id=ev.call_id,
            tool=ev.tool,
            agent=ev.agent,
            risk_score=ev.risk_score,
            decision=ev.decision,
            reason=ev.reason,
            timestamp=ev.timestamp.isoformat(),
        )
        for ev in trace_store.all()
    ]


@app.get("/health")
async def health_check():
    """Health check endpoint for Nasiko service discovery."""
    return {
        "status": "healthy",
        "service": "aegis-firewall",
        "version": "1.0.0",
    }
