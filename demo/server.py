#!/usr/bin/env python3
"""Local demo server for the Nasiko hackathon evals (P1 compact-tools, P2 classifier).

Pure Python standard library — no pip installs. It shells out to the cargo examples,
reads their JSONL output, joins it against the eval set's ground truth, scores each case,
and serves a single-page dashboard at http://localhost:8765.

Run:
    python3 demo/server.py
then open the printed URL. Nothing here is part of the PR; it's a local demo harness.
"""

import json
import os
import subprocess
import sys
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEMO_DIR = os.path.join(REPO_ROOT, "demo")
P1_EVAL = os.path.join(REPO_ROOT, "eval-data", "compact-tools-eval.json")
P2_EVAL = os.path.join(REPO_ROOT, "eval-data", "classifier-eval.json")
P1_OUT = "/tmp/nasiko_p1_out.jsonl"
P2_OUT = "/tmp/nasiko_p2_out.jsonl"
PORT = 8765

# Live gateway playground — runs one real agent task so P1 and P2 act on the same call.
GATEWAY = "http://localhost:8080/v1/chat/completions"
OWNER = "091ecc7c-b852-4b0d-a2d1-906d2015f258"
PLAYGROUND_AGENT = "19fb9132-8adf-4b14-8bf6-16dbea6fa340"  # General-Agent: un-pinned, so P2 fires
AGENT_JWT_SECRET = "dev-agent-secret-change-me"
AGENT_DEFAULT_MODEL = "openai.gpt-oss-20b"


def cargo_env():
    env = os.environ.copy()
    cargo_bin = os.path.expanduser("~/.cargo/bin")
    env["PATH"] = cargo_bin + os.pathsep + env.get("PATH", "")
    return env


def run_example(example, extra_env):
    """Run a cargo example; return (ok, stderr_text)."""
    env = cargo_env()
    env.update(extra_env)
    cmd = [
        "cargo", "run", "--release", "-q",
        "-p", "nasiko-llm-router", "--example", example,
    ]
    proc = subprocess.run(cmd, cwd=REPO_ROOT, env=env, capture_output=True, text=True)
    return proc.returncode == 0, (proc.stderr or "") + (proc.stdout or "")


def read_jsonl(path):
    rows = {}
    order = []
    with open(path) as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            obj = json.loads(line)
            rows[obj["id"]] = obj
            order.append(obj["id"])
    return rows, order


# ---------------------------------------------------------------------------
# P1 scoring
# ---------------------------------------------------------------------------

def match_call(expected, actual, free_fields):
    if expected.get("name") != actual.get("name"):
        return False
    ea = expected.get("arguments", {}) or {}
    aa = actual.get("arguments", {}) or {}
    for key, val in ea.items():
        if key in free_fields:
            if key not in aa or type(aa[key]) is not type(val):
                return False
        else:
            if aa.get(key) != val:
                return False
    return True


def match_call_list(expected, actual, free_fields):
    if len(expected) != len(actual):
        return False
    return all(match_call(e, a, free_fields) for e, a in zip(expected, actual))


def run_p1():
    ok, log = run_example("compact_tools_eval", {"EVAL_SET": P1_EVAL, "OUT": P1_OUT, "EMIT_TOKENS": "1"})
    if not ok:
        return {"ok": False, "log": log}

    eval_set = json.load(open(P1_EVAL))
    out_rows, _ = read_jsonl(P1_OUT)

    # token reduction line from stderr: "token reduction 31.9% (baseline 668 -> compact 455 ...)"
    token = None
    for part in log.split("\n"):
        if "token reduction" in part:
            token = part.strip()

    cases = []
    passed = 0
    base_tok_total = 0
    comp_tok_total = 0
    for case in eval_set.get("cases", []):
        cid = case["id"]
        out = out_rows.get(cid, {})
        expected = case.get("expected", [])
        roundtrip = out.get("roundtrip_calls", [])
        free_fields = (case.get("match", {}) or {}).get("free_text_fields", [])
        ok_case = match_call_list(expected, roundtrip, free_fields)
        passed += ok_case
        base_tok_total += out.get("baseline_tokens", 0)
        comp_tok_total += out.get("compact_tokens", 0)
        cases.append({
            "id": cid,
            "query": (case.get("messages", [{}])[0] or {}).get("content", ""),
            "expected": expected,
            "rendered": out.get("rendered_calls", ""),
            "roundtrip": roundtrip,
            "pass": ok_case,
            "baseline_request": out.get("baseline_request"),
            "compact_request": out.get("compact_request"),
            "baseline_tokens": out.get("baseline_tokens"),
            "compact_tokens": out.get("compact_tokens"),
            "saved_pct": out.get("saved_pct"),
        })

    decoder = []
    dpassed = 0
    for dc in eval_set.get("decoder_cases", []):
        did = dc["id"]
        out = out_rows.get(did, {})
        decoded = out.get("decoded", {})
        expected = dc.get("expected", {})
        if "error" in expected:
            ok_dc = decoded.get("error") == expected["error"]
        else:
            ok_dc = match_call_list(expected.get("calls", []), decoded.get("calls", []), [])
        dpassed += ok_dc
        decoder.append({
            "id": did,
            "note": dc.get("note", ""),
            "chunks": dc.get("chunks", []),
            "expected": expected,
            "decoded": decoded,
            "pass": ok_dc,
        })

    return {
        "ok": True,
        "token": token,
        "token_totals": {
            "baseline": base_tok_total, "compact": comp_tok_total,
            "reduction": round(100.0 * (1 - comp_tok_total / base_tok_total), 1) if base_tok_total else 0,
        },
        "cases": cases,
        "decoder": decoder,
        "summary": {
            "cases_passed": passed, "cases_total": len(cases),
            "decoder_passed": dpassed, "decoder_total": len(decoder),
        },
    }


# ---------------------------------------------------------------------------
# P2 scoring
# ---------------------------------------------------------------------------

def run_p2(backend):
    ok, log = run_example("classifier_eval", {
        "EVAL_SET": P2_EVAL, "OUT": P2_OUT, "CLASSIFIER_BACKEND": backend,
    })
    if not ok:
        return {"ok": False, "log": log}

    eval_set = json.load(open(P2_EVAL))
    out_rows, _ = read_jsonl(P2_OUT)

    rows = []
    correct = 0
    within1 = 0
    lat = []
    for ex in eval_set.get("examples", []):
        eid = ex["id"]
        out = out_rows.get(eid, {})
        pred_type = out.get("request_type")
        exp_type = ex.get("request_type")
        type_ok = pred_type == exp_type
        correct += type_ok
        pred_c = out.get("complexity")
        exp_c = ex.get("complexity")
        c_ok = pred_c is not None and exp_c is not None and abs(pred_c - exp_c) <= 1
        within1 += c_ok
        lat.append(out.get("latency_us", 0))
        rows.append({
            "id": eid,
            "query": ex.get("query", ""),
            "pred_type": pred_type, "exp_type": exp_type, "type_ok": type_ok,
            "pred_complexity": pred_c, "exp_complexity": exp_c, "c_ok": c_ok,
            "confidence": out.get("confidence"),
            "latency_us": out.get("latency_us"),
        })

    lat_sorted = sorted(lat)
    p50 = lat_sorted[len(lat_sorted) // 2] if lat_sorted else 0
    p95 = lat_sorted[min(len(lat_sorted) - 1, int(len(lat_sorted) * 0.95))] if lat_sorted else 0
    n = len(rows) or 1
    return {
        "ok": True,
        "backend": backend,
        "rows": rows,
        "summary": {
            "type_acc": round(100.0 * correct / n, 1),
            "complexity_within1": round(100.0 * within1 / n, 1),
            "p50_us": p50, "p95_us": p95, "total": len(rows),
        },
    }


def run_p2_compare():
    """Run both backends and merge rows side by side so the UI can show before/after."""
    base = run_p2("regex")
    new = run_p2("heuristic")
    if not base.get("ok") or not new.get("ok"):
        return base if not base.get("ok") else new

    new_by_id = {r["id"]: r for r in new["rows"]}
    rows = []
    for b in base["rows"]:
        h = new_by_id.get(b["id"], {})
        rows.append({
            "id": b["id"],
            "query": b["query"],
            "exp_type": b["exp_type"],
            "exp_complexity": b["exp_complexity"],
            "regex_type": b["pred_type"], "regex_c": b["pred_complexity"],
            "regex_conf": b["confidence"], "regex_type_ok": b["type_ok"],
            "heur_type": h.get("pred_type"), "heur_c": h.get("pred_complexity"),
            "heur_conf": h.get("confidence"), "heur_type_ok": h.get("type_ok"),
            "changed": b["pred_complexity"] != h.get("pred_complexity")
            or b["confidence"] != h.get("confidence")
            or b["pred_type"] != h.get("pred_type"),
        })
    return {
        "ok": True,
        "mode": "compare",
        "rows": rows,
        "regex": base["summary"],
        "heuristic": new["summary"],
    }


# ---------------------------------------------------------------------------
# Live playground — type your own input, run it through the real binaries
# ---------------------------------------------------------------------------

def _run_classifier(eval_path, backend, out_path):
    ok, log = run_example("classifier_eval", {
        "EVAL_SET": eval_path, "OUT": out_path, "CLASSIFIER_BACKEND": backend,
    })
    if not ok:
        raise RuntimeError(log)
    rows, _ = read_jsonl(out_path)
    return rows


def classify_live(query, context):
    """Run one typed query through both classifier backends (the real Rust trait)."""
    eval_set = {"labels": [], "examples": [{
        "id": "live", "query": query, "context": context or "",
        "request_type": "general", "complexity": 3,
    }]}
    path = "/tmp/nasiko_live_p2.json"
    json.dump(eval_set, open(path, "w"))
    reg = _run_classifier(path, "regex", "/tmp/nasiko_live_p2r.jsonl").get("live", {})
    heu = _run_classifier(path, "heuristic", "/tmp/nasiko_live_p2h.jsonl").get("live", {})
    return {"ok": True, "query": query, "context": context, "regex": reg, "heuristic": heu}


def p1_live(query, call_text):
    """Transform a typed user message (native vs compact request) and/or decode a typed
    compact `<<call ...>>` string — both through the real compact_tools_eval binary."""
    tools_full = json.load(open(P1_EVAL)).get("tools", [])
    names = [t["function"]["name"] for t in tools_full]
    eval_set = {
        "schema_version": "live", "tools": tools_full,
        "cases": ([{"id": "live", "tools": names,
                    "messages": [{"role": "user", "content": query}], "expected": []}]
                  if query else []),
        "decoder_cases": ([{"id": "dlive", "tools": names, "chunks": [call_text]}]
                          if call_text else []),
    }
    path = "/tmp/nasiko_live_p1.json"
    json.dump(eval_set, open(path, "w"))
    ok, log = run_example("compact_tools_eval", {
        "EVAL_SET": path, "OUT": "/tmp/nasiko_live_p1.jsonl", "EMIT_TOKENS": "1",
    })
    if not ok:
        raise RuntimeError(log)
    rows, _ = read_jsonl("/tmp/nasiko_live_p1.jsonl")
    case = rows.get("live", {})
    dec = rows.get("dlive", {})
    return {
        "ok": True, "query": query,
        "tools": [compact_sig(t) for t in tools_full],
        "baseline_request": case.get("baseline_request"),
        "compact_request": case.get("compact_request"),
        "baseline_tokens": case.get("baseline_tokens"),
        "compact_tokens": case.get("compact_tokens"),
        "saved_pct": case.get("saved_pct"),
        "decoded": dec.get("decoded"),
    }


def compact_sig(tool):
    """A short human label of a native tool for the UI's tool list."""
    fn = tool.get("function", tool)
    props = (fn.get("parameters", {}) or {}).get("properties", {}) or {}
    req = set((fn.get("parameters", {}) or {}).get("required", []) or [])
    args = ", ".join(k + ("" if k in req else "?") for k in props)
    return f"{fn.get('name')}({args})"


# ---------------------------------------------------------------------------
# Live app — read what P1/P2 actually did inside the running Nasiko server
# ---------------------------------------------------------------------------

def psql_rows(sql):
    """Run a read-only query against the real app DB via psql (stdlib subprocess)."""
    env = os.environ.copy()
    env["PGPASSWORD"] = "nasiko"
    env["PATH"] = "/opt/homebrew/opt/postgresql@17/bin:" + env.get("PATH", "")
    proc = subprocess.run(
        ["psql", "-U", "nasiko", "-h", "localhost", "-d", "nasiko_dev", "-tAF,", "-c", sql],
        env=env, capture_output=True, text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or "psql failed")
    return [line for line in proc.stdout.strip().split("\n") if line]


def live_app_stats():
    """P1 savings recorded by the real router in token_usage.metadata.compact."""
    try:
        row = psql_rows(
            "SELECT count(*) FILTER (WHERE metadata->'compact'->>'applied'='true'), "
            "coalesce(sum((metadata->'compact'->>'saved_bytes')::int),0), "
            "coalesce(sum((metadata->'compact'->>'native_bytes')::int),0), "
            "coalesce(sum((metadata->'compact'->>'compact_bytes')::int),0), "
            "count(*) FROM token_usage WHERE total_tokens>0;"
        )[0].split(",")
        applied, saved, native, compact, total = (int(x) for x in row)
        return {"ok": True, "p1": {
            "applied_calls": applied, "total_calls": total,
            "saved_bytes": saved, "native_bytes": native, "compact_bytes": compact,
            "reduction_pct": round(100.0 * saved / native, 1) if native else 0,
        }}
    except Exception as exc:  # noqa: BLE001 - live strip is best-effort
        return {"ok": False, "log": str(exc)}


def run_summary():
    """One bundle for the simple landing page: real eval numbers + live-app savings."""
    p1 = run_p1()
    p2 = run_p2_compare()
    return {
        "ok": p1.get("ok", False) and p2.get("ok", False),
        "p1": {
            "tokens": p1.get("token_totals"),
            "cases": p1.get("summary"),
            "sample": [
                {"id": c["id"], "baseline": c["baseline_tokens"],
                 "compact": c["compact_tokens"], "saved_pct": c["saved_pct"]}
                for c in p1.get("cases", []) if c.get("baseline_tokens")
            ],
        },
        "p2": {
            "regex": p2.get("regex"), "heuristic": p2.get("heuristic"),
            "rows": [r for r in p2.get("rows", []) if r.get("changed")][:6]
                    or p2.get("rows", [])[:6],
        },
        "live": live_app_stats(),
    }


# ---------------------------------------------------------------------------
# Playground — run one real agent task through the live gateway, annotated P1/P2
# ---------------------------------------------------------------------------

def mint_jwt(agent):
    env = cargo_env()
    env["AGENT_JWT_SECRET"] = AGENT_JWT_SECRET
    proc = subprocess.run(
        ["cargo", "run", "--release", "-q", "-p", "nasiko-llm-router",
         "--example", "mint_token", "--", agent, OWNER],
        cwd=REPO_ROOT, env=env, capture_output=True, text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or "mint_token failed")
    return proc.stdout.strip().splitlines()[-1]


def psql_exec(sql):
    env = os.environ.copy()
    env["PGPASSWORD"] = "nasiko"
    env["PATH"] = "/opt/homebrew/opt/postgresql@17/bin:" + env.get("PATH", "")
    subprocess.run(
        ["psql", "-U", "nasiko", "-h", "localhost", "-d", "nasiko_dev", "-q", "-c", sql],
        env=env, capture_output=True, text=True,
    )


def run_agent_task(query):
    """Send one task to the real gateway with tools attached (so P1 compacts them) through an
    un-pinned agent (so P2 classifies + routes), then read back what each layer did for this call."""
    if not query.strip():
        return {"ok": False, "log": "type a task first"}
    tools = json.load(open(P1_EVAL)).get("tools", [])
    jwt = mint_jwt(PLAYGROUND_AGENT)
    last = "gateway did not return 200"
    for _ in range(3):
        trace, span = os.urandom(16).hex(), os.urandom(8).hex()
        ctx = "pg-" + os.urandom(3).hex()
        psql_exec(
            f"INSERT INTO flows(flow_id,user_id,status,metadata,created_at) "
            f"VALUES ('{trace}','{OWNER}','running','{{\"context_id\":\"{ctx}\",\"mode\":\"continue\"}}',now()); "
            f"INSERT INTO flow_participants(flow_id,agent_id) VALUES ('{trace}','{PLAYGROUND_AGENT}');"
        )
        payload = {"model": "gpt-4o",
                   "messages": [{"role": "user", "content": query}],
                   "tools": tools}
        req = urllib.request.Request(
            GATEWAY, data=json.dumps(payload).encode(),
            headers={"Authorization": "Bearer " + jwt, "Content-Type": "application/json",
                     "traceparent": f"00-{trace}-{span}-01"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=90) as resp:
                body = json.loads(resp.read().decode())
        except Exception as exc:  # noqa: BLE001 - transient upstream 502s are retried
            last = str(exc)
            continue

        choice = (body.get("choices") or [{}])[0]
        msg = choice.get("message", {}) or {}
        tool_calls = msg.get("tool_calls")
        output = msg.get("content") or msg.get("reasoning") or ""
        if not output and tool_calls:
            output = "→ tool call:\n" + json.dumps(tool_calls, indent=2)
        if not output:
            output = json.dumps(choice, indent=2)

        time.sleep(1)
        try:
            rows = psql_rows(
                "SELECT json_build_object('model',model,'input_tokens',input_tokens,"
                "'compact',metadata->'compact')::text FROM token_usage "
                f"WHERE session_id='{trace}' ORDER BY created_at DESC LIMIT 1;"
            )
            info = json.loads(rows[0]) if rows else {}
        except Exception:  # noqa: BLE001
            info = {}

        p2cls = classify_live(query, "")
        routed = info.get("model") or AGENT_DEFAULT_MODEL
        return {
            "ok": True, "query": query, "output": output,
            "usage": body.get("usage"),
            "p1": info.get("compact"),
            "p2": {
                "routed_model": routed,
                "default_model": AGENT_DEFAULT_MODEL,
                "overridden": routed != AGENT_DEFAULT_MODEL,
                "heuristic": p2cls.get("heuristic"),
                "regex": p2cls.get("regex"),
            },
        }
    return {"ok": False, "log": last + " (transient upstream — try again)"}


class Handler(BaseHTTPRequestHandler):
    def _send(self, code, body, ctype="application/json"):
        data = body.encode() if isinstance(body, str) else body
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass  # quiet

    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path in ("/", "/index.html", "/simple.html"):
            with open(os.path.join(DEMO_DIR, "simple.html"), "rb") as fh:
                return self._send(200, fh.read(), "text/html; charset=utf-8")
        if parsed.path in ("/advanced", "/advanced.html"):
            with open(os.path.join(DEMO_DIR, "index.html"), "rb") as fh:
                return self._send(200, fh.read(), "text/html; charset=utf-8")
        if parsed.path == "/api/summary":
            try:
                return self._send(200, json.dumps(run_summary()))
            except Exception as exc:  # noqa: BLE001
                return self._send(200, json.dumps({"ok": False, "log": str(exc)}))
        if parsed.path == "/api/run_task":
            query = parse_qs(parsed.query).get("q", [""])[0]
            try:
                return self._send(200, json.dumps(run_agent_task(query)))
            except Exception as exc:  # noqa: BLE001
                return self._send(200, json.dumps({"ok": False, "log": str(exc)}))
        if parsed.path == "/api/p1":
            try:
                return self._send(200, json.dumps(run_p1()))
            except Exception as exc:  # noqa: BLE001 - surface any failure to the UI
                return self._send(200, json.dumps({"ok": False, "log": str(exc)}))
        if parsed.path == "/api/p2":
            backend = (parse_qs(parsed.query).get("backend", ["regex"])[0])
            try:
                if backend == "compare":
                    return self._send(200, json.dumps(run_p2_compare()))
                if backend not in ("regex", "heuristic"):
                    backend = "regex"
                return self._send(200, json.dumps(run_p2(backend)))
            except Exception as exc:  # noqa: BLE001
                return self._send(200, json.dumps({"ok": False, "log": str(exc)}))
        if parsed.path == "/api/classify":
            qs = parse_qs(parsed.query)
            query = qs.get("q", [""])[0]
            context = qs.get("ctx", [""])[0]
            try:
                return self._send(200, json.dumps(classify_live(query, context)))
            except Exception as exc:  # noqa: BLE001
                return self._send(200, json.dumps({"ok": False, "log": str(exc)}))
        if parsed.path == "/api/p1live":
            qs = parse_qs(parsed.query)
            query = qs.get("q", [""])[0]
            call_text = qs.get("call", [""])[0]
            try:
                return self._send(200, json.dumps(p1_live(query, call_text)))
            except Exception as exc:  # noqa: BLE001
                return self._send(200, json.dumps({"ok": False, "log": str(exc)}))
        return self._send(404, json.dumps({"error": "not found"}))


def main():
    if not os.path.exists(P1_EVAL) or not os.path.exists(P2_EVAL):
        print("Eval sets missing under eval-data/. Download them first (see PR notes).", file=sys.stderr)
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    url = f"http://localhost:{PORT}"
    print(f"\n  Nasiko eval demo running at {url}")
    print("  Open it in your browser. Ctrl+C to stop.\n")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n  stopped.")


if __name__ == "__main__":
    main()
