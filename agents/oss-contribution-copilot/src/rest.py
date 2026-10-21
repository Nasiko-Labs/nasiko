"""Thin REST face over the copilot, for chat front-ends such as DronaHQ.

Four endpoints plus a human-readable detail page. Every response is the
small projection from Copilot.projection(), never raw thread state.
"""

from __future__ import annotations

import hmac
import html
import json
import time

from starlette.requests import Request
from starlette.responses import HTMLResponse, JSONResponse
from starlette.routing import Route

from copilot import Copilot, GateError, PolicyError


def _error(status: int, msg: str) -> JSONResponse:
    return JSONResponse({"error": msg}, status_code=status)


async def _json(request: Request) -> dict:
    try:
        body = await request.json()
    except (json.JSONDecodeError, ValueError):
        return {}
    return body if isinstance(body, dict) else {}


def build_routes(cp: Copilot) -> list[Route]:
    async def contribute(request: Request):
        body = await _json(request)
        try:
            return JSONResponse(cp.contribute(str(body.get("repo", ""))))
        except PolicyError as e:
            return _error(403, str(e))

    async def status(request: Request):
        tid = request.query_params.get("thread_id", "")
        try:
            return JSONResponse(cp.status(tid))
        except KeyError as e:
            return _error(404, str(e))

    async def approvals(_: Request):
        return JSONResponse({"approvals": cp.approvals()})

    async def approve(request: Request):
        key = cp.cfg.approver_key
        if key and not hmac.compare_digest(request.headers.get("x-approver-key", ""), key):
            return _error(401, "approval requires a valid X-Approver-Key header")
        body = await _json(request)
        try:
            return JSONResponse(cp.decide(
                gate_id=str(body.get("gate_id", "")).upper(),
                decision=str(body.get("decision", "")),
                note=str(body.get("note", "")),
                thread_id=str(body.get("thread_id", "")),
                approver=request.headers.get("x-approver", "human"),
            ))
        except KeyError as e:
            return _error(404, str(e))
        except GateError as e:
            return _error(409, str(e))

    async def health(_: Request):
        return JSONResponse({"ok": True, "threads": len(cp.store.threads)})

    async def work(request: Request):
        try:
            t = cp.get(request.path_params["thread_id"])
        except KeyError:
            return HTMLResponse("<h1>Not found</h1>", status_code=404)
        return HTMLResponse(_render(cp, t))

    return [
        Route("/a2a/contribute", contribute, methods=["POST"]),
        Route("/a2a/status", status, methods=["GET"]),
        Route("/a2a/approvals", approvals, methods=["GET"]),
        Route("/a2a/approve", approve, methods=["POST"]),
        Route("/health", health, methods=["GET"]),
        Route("/work/{thread_id}", work, methods=["GET"]),
    ]


def _render(cp: Copilot, t: dict) -> str:
    e = html.escape
    p = cp.projection(t)
    gate = p["gate"]
    events = "".join(
        f"<tr><td>{time.strftime('%H:%M:%S', time.gmtime(ev['ts']))}</td><td>{e(ev['actor'])}</td>"
        f"<td><b>{e(ev['type'])}</b></td><td>{e(ev['detail'])}</td></tr>"
        for ev in t["events"]
    )
    files = "".join(
        f"<details><summary><code>{e(f['path'])}</code> ({len(f['content'])} bytes)</summary>"
        f"<pre>{e(f['content'])}</pre></details>"
        for f in t.get("files", [])
    )
    prop = t.get("proposal") or {}
    pr = f'<p>Pull request: <a href="{e(p["pr_url"])}">{e(p["pr_url"])}</a></p>' if p["pr_url"] else ""
    gate_html = (f"<p class=gate>Waiting on a human: <b>{e(gate['id'])}</b> — {e(gate['question'])}</p>"
                 if gate else "")
    return f"""<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width">
<title>{e(t['id'])}</title>
<style>body{{font:15px system-ui;max-width:900px;margin:24px auto;padding:0 16px}}
table{{border-collapse:collapse;width:100%}}td{{border-bottom:1px solid #ddd;padding:4px 6px;vertical-align:top}}
pre{{background:#f5f5f5;padding:10px;overflow:auto;max-height:420px}}.gate{{background:#fff4d6;padding:10px}}</style>
<h1>{e(t['repo'])}</h1>
<p><b>{e(p['phase'])}</b> — {e(p['headline'])} · cost ${p['cost_usd']:.4f}</p>
{gate_html}{pr}
<h2>Proposal</h2><p><b>{e(prop.get('title', '—'))}</b><br>{e(prop.get('why', ''))}</p>
<h2>Files</h2>{files or '<p>—</p>'}
<h2>Pull request body</h2><pre>{e(t.get('pr_body', '—'))}</pre>
<h2>Audit log</h2><table>{events}</table>"""
