#!/usr/bin/env python3
"""Local AutoEval UI. Reuses the unmodified CLI; no credentials reach the browser."""

import argparse
import collections
import difflib
import json
import os
import re
import signal
import subprocess
import sys
import threading
import uuid
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))


def read_json(path, default=None):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError):
        return default


def evaluate_gate(results, baseline, candidate):
    """An executable no-regression policy; never changes a deployed model."""
    base = baseline.split(":", 1)[-1]
    proposed = candidate.split(":", 1)[-1]
    reasons = []
    if base == proposed:
        return {"decision": "INCONCLUSIVE", "reasons": ["Choose two different models."],
                "baseline": base, "candidate": proposed}
    attempts = results.get("attempts", [])
    by_model = {}
    for name in (base, proposed):
        rows = [a for a in attempts if a.get("model") == name]
        keys = [a.get("task") for a in rows]
        if not rows or len(set(keys)) != len(keys) or None in keys:
            reasons.append("Missing or duplicate task results for " + name)
        by_model[name] = {a.get("task"): a for a in rows}
    if set(by_model[base]) != set(by_model[proposed]):
        reasons.append("Models were not evaluated on the same tasks.")
    for name, rows in by_model.items():
        for task, row in rows.items():
            if row.get("outcome") not in {"resolved", "unresolved", "invalid_patch"}:
                reasons.append(f"{name}: incomplete/infrastructure outcome for {str(task)[:12]}")
    if results.get("validation", {}).get("status") != "verified" and results.get("run_kind") != "frozen_benchmark":
        reasons.append("Task validation is not verified.")
    if reasons:
        return {"decision": "INCONCLUSIVE", "reasons": reasons,
                "baseline": base, "candidate": proposed}
    successes = [task for task, row in by_model[base].items() if row["outcome"] == "resolved"]
    regressions = [task for task in successes if by_model[proposed][task]["outcome"] != "resolved"]
    if regressions:
        decision = "BLOCK"
        reasons = ["Candidate loses a task the baseline solved: " + str(t)[:12] for t in regressions]
    elif not successes:
        decision = "INCONCLUSIVE"
        reasons = ["Baseline solved no tasks; this run cannot establish no-regression quality."]
    else:
        decision = "PASS"
        reasons = [f"Candidate retained all {len(successes)} baseline-resolved task(s).",
                   "This policy covers the evaluated tasks only; it does not establish general model quality."]
    return {"decision": decision, "reasons": reasons, "baseline": base, "candidate": proposed}


class App:
    def __init__(self, server_url, postgres_container):
        self.server_url = server_url
        self.postgres_container = postgres_container
        self.jobs = {}
        self.lock = threading.RLock()
        self.prepare_lock = threading.Lock()
        from nasiko_autoeval.nasiko import read_env
        values = read_env(ROOT.parent / ".env")
        self.secrets = [str(v) for k, v in {**values, **os.environ}.items()
                        if any(s in k.upper() for s in ("KEY", "SECRET", "PASSWORD", "TOKEN")) and len(str(v)) > 5]
        self.manifest = read_json(ROOT / "benchmark.json", {})

    def redact(self, value):
        text = str(value)
        for secret in self.secrets:
            text = text.replace(secret, "[redacted]")
        return re.sub(r"(?i)(authorization:\s*bearer\s+|[?&]key=)[^\s\"&]+", r"\1[redacted]", text)

    def projects(self):
        return ([{"id": "calculator:" + name, "label": "Calculator / " + name.title(), "kind": "synthetic"}
                 for name in ("python", "javascript", "typescript", "go", "rust")]
                + [{"id": "real:" + item["id"], "label": "Real task / " + item["id"], "kind": "real_repository"}
                   for item in self.manifest.get("tasks", [])]
                + [{"id": "custom", "label": "My local repository", "kind": "custom"}])

    def project(self, data):
        selector = str(data.get("project_id", "calculator:python"))
        if selector.startswith("calculator:"):
            name = selector.split(":", 1)[1]
            if name not in {"python", "javascript", "typescript", "go", "rust"}:
                raise ValueError("Unknown calculator.")
            from nasiko_autoeval.examples import prepare_example
            with self.prepare_lock:
                item = prepare_example(name)
            config = ROOT / "examples" / "calculators" / name / "execution.json"
            return {"label": "Calculator / " + name, "kind": "synthetic", "repo": item["repository"],
                    "commit": item["fix_sha"], "issue": (ROOT / "examples/calculators/issue.txt").read_text(),
                    "config": str(config) if config.exists() else None, "max_tokens": 4096}
        if selector.startswith("real:"):
            entry = next((x for x in self.manifest.get("tasks", []) if "real:" + x["id"] == selector), None)
            if not entry:
                raise ValueError("Unknown real task.")
            from nasiko_autoeval.benchmark import _repository
            with self.prepare_lock:
                bundle_root = ROOT / "final/inputs"
                repo = _repository(entry, bundle_root if (bundle_root / entry["id"] / "source.bundle").exists() else None)
            config = ROOT / entry["config_file"] if entry.get("config_file") else None
            return {"label": entry["id"], "kind": "real_repository", "repo": str(repo),
                    "commit": entry["commit"], "issue": (ROOT / entry["issue_file"]).read_text(),
                    "config": str(config) if config else None, "max_tokens": entry["max_output_tokens"]}
        if selector != "custom":
            raise ValueError("Unknown project.")
        repo = Path(str(data.get("repo", ""))).expanduser().resolve()
        if not (repo / ".git").exists():
            raise ValueError("Choose an existing local Git checkout, with a .git directory.")
        commit = str(data.get("commit", "")).strip()
        issue = str(data.get("issue", "")).strip()
        if not commit or commit.startswith("-") or not issue:
            raise ValueError("A fix commit and issue description are required.")
        return {"label": repo.name, "kind": "custom", "repo": str(repo), "commit": commit,
                "issue": issue, "config": None, "max_tokens": 4096}

    def inspect(self, data):
        from nasiko_autoeval.tasks import build_task
        import tempfile
        project = self.project(data)
        overrides = read_json(project["config"]) if project["config"] else None
        with tempfile.TemporaryDirectory(prefix="autoeval-ui-inspect-") as temp:
            task, plan, *_ = build_task(project["repo"], project["commit"], temp,
                                         overrides=overrides, issue=project["issue"])
        return {**project, "plans": [plan.to_dict()], "separation": {
            "production_changes": task["production_changes"], "test_changes": task["test_changes"]}}

    def start(self, data):
        models = data.get("models", [])
        if not isinstance(models, list) or len(models) != 2 or not all(isinstance(m, str) and m.strip() and not m.startswith("-") for m in models):
            raise ValueError("Enter two model configurations or provider:model selectors.")
        if models[0] == models[1]:
            raise ValueError("Choose two different models.")
        with self.lock:
            if any(j["state"] == "running" for j in self.jobs.values()):
                raise ValueError("A comparison is already running. Wait for it to finish.")
            job_id = datetime.now().strftime("%Y%m%d-%H%M%S-") + uuid.uuid4().hex[:6]
            job = {"job_id": job_id, "state": "running", "stage": "preparing", "logs": collections.deque(maxlen=500),
                   "exit_code": None, "process": None, "models": models, "gate": None,
                   "project": {"label": data.get("project_id", "project"), "kind": "pending"},
                   "directory": ROOT / "runs" / ("web-" + job_id)}
            self.jobs[job_id] = job
        threading.Thread(target=self._run, args=(job, dict(data)), daemon=True).start()
        return {"job_id": job_id}

    def _run(self, job, data):
        try:
            project = self.project(data)
            job["project"] = project
            directory = job["directory"]
            directory.mkdir(parents=True, exist_ok=False)
            issue_file = directory / "ui-issue.txt"
            issue_file.write_text(project["issue"])
            command = [sys.executable, "-u", str(ROOT / "run.py"), "compare", "--repo", project["repo"],
                       "--commit", project["commit"], "--issue-file", str(issue_file),
                       "--model", job["models"][0], "--model", job["models"][1],
                       "--max-output-tokens", str(project["max_tokens"]), "--preflight-attempts", "3",
                       "--server-url", self.server_url, "--run", str(directory)]
            if self.postgres_container:
                command += ["--postgres-container", self.postgres_container]
            if project["config"]:
                command += ["--config", project["config"]]
            job["logs"].append("Starting a fresh CLI comparison. Preflight and generation make paid calls through Nasiko.")
            process = subprocess.Popen(command, cwd=ROOT.parent, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                       text=True, bufsize=1, start_new_session=True)
            job["process"] = process
            for line in process.stdout:
                line = self.redact(line.rstrip())[:12000]
                with self.lock:
                    job["logs"].append(line)
                    if line.startswith(("Preflight:", "Availability preflight")):
                        job["stage"] = "preflight"
                    elif line.startswith("Preparing"):
                        job["stage"] = "preparing"
                    elif line.startswith("Offline validation"):
                        job["stage"] = "validating"
                    elif line.startswith("One-shot attempt:"):
                        job["stage"] = "generating"
                    elif line.lstrip().startswith(("resolved:", "unresolved:", "invalid_patch:", "infrastructure_error:")):
                        job["stage"] = "grading"
            code = process.wait()
            with self.lock:
                job["exit_code"] = code
                job["state"] = "finished" if code == 0 else "failed"
                job["stage"] = "completed" if code == 0 else "failed"
        except Exception as exc:
            with self.lock:
                job["logs"].append(self.redact(exc))
                job["state"] = "failed"
                job["stage"] = "failed"

    def status(self, job_id):
        with self.lock:
            job = self.jobs[job_id]
            status = {k: job[k] for k in ("job_id", "state", "stage", "exit_code", "project", "gate")}
            status["logs"] = list(job["logs"])
        directory = job["directory"]
        status["results"] = read_json(directory / "results.json")
        status["evidence"] = []
        originals = {}
        try:
            prompt = (directory / "prompt.txt").read_text().split("\n", 1)[1]
            originals = {f["path"]: f["content"] for f in json.loads(prompt)["source"]}
        except (OSError, ValueError, KeyError, IndexError):
            pass
        for path in sorted(directory.glob("model-*.json")):
            record = read_json(path, {})
            response, usage = record.get("response", {}), record.get("usage") or {}
            index = int(path.stem.split("-")[-1])
            candidate = read_json(directory / f"candidate-{index}.json", {})
            files = []
            for file in candidate.get("files", []):
                content = file.get("content", "")
                diff = "".join(difflib.unified_diff(originals.get(file["path"], "").splitlines(True),
                                                  content.splitlines(True), fromfile="before/" + file["path"],
                                                  tofile="candidate/" + file["path"]))
                files.append({**file, "diff": diff})
            status["evidence"].append({"index": index, "model": response.get("model"),
                "response_id": response.get("id"), "trace_id": usage.get("trace_id"),
                "input_tokens": usage.get("input_tokens"), "output_tokens": usage.get("output_tokens"),
                "generation_seconds": record.get("duration_seconds"), "cost_usd": record.get("cost_usd"),
                "pricing_source": (usage.get("pricing") or {}).get("source"),
                "cost_estimated": (usage.get("pricing") or {}).get("estimated"), "files": files})
        return json.loads(self.redact(json.dumps(status)))

    def gate(self, data):
        job = self.jobs[str(data["job_id"])]
        if job["state"] != "finished":
            raise ValueError("Wait for a completed comparison before checking the gate.")
        results = read_json(job["directory"] / "results.json", {})
        decision = evaluate_gate(results, str(data["baseline"]), str(data["candidate"]))
        job["gate"] = decision
        return decision

    def stop(self):
        for job in self.jobs.values():
            process = job.get("process")
            if process and process.poll() is None:
                os.killpg(process.pid, signal.SIGINT)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def send(self, status, value, content_type="application/json"):
        payload = json.dumps(value).encode() if content_type == "application/json" else value.encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type + "; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.end_headers()
        self.wfile.write(payload)

    def local(self):
        host = self.headers.get("Host", "")
        allowed = {f"127.0.0.1:{self.server.server_port}", f"localhost:{self.server.server_port}"}
        origin = self.headers.get("Origin")
        return host in allowed and (origin is None or origin in {"http://" + h for h in allowed})

    def do_GET(self):
        try:
            if not self.local():
                return self.send(403, {"error": "Local browser access only."})
            route = urlsplit(self.path)
            if route.path == "/":
                return self.send(200, Path(__file__).with_name("index.html").read_text(), "text/html")
            if route.path == "/api/bootstrap":
                return self.send(200, {"projects": self.server.app.projects(),
                    "default_models": self.server.app.manifest.get("models", [])[:2],
                    "nasiko_url": self.server.app.server_url})
            query = parse_qs(route.query)
            job_id = query.get("job_id", [""])[0]
            if route.path == "/api/status":
                return self.send(200, self.server.app.status(job_id))
            if route.path == "/api/report":
                job = self.server.app.jobs[job_id]
                report = (job["directory"] / "report.md").read_text()
                return self.send(200, self.server.app.redact(report), "text/plain")
            self.send(404, {"error": "Unknown endpoint."})
        except (ValueError, OSError, KeyError) as exc:
            self.send(400, {"error": self.server.app.redact(exc)})

    def do_POST(self):
        try:
            if not self.local():
                return self.send(403, {"error": "Local browser access only."})
            if self.headers.get_content_type() != "application/json":
                return self.send(415, {"error": "Send JSON."})
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 65536:
                raise ValueError("Request too large or empty.")
            data = json.loads(self.rfile.read(length))
            routes = {"/api/inspect": self.server.app.inspect, "/api/run": self.server.app.start,
                      "/api/gate": self.server.app.gate}
            action = routes.get(self.path)
            if action is None:
                return self.send(404, {"error": "Unknown endpoint."})
            self.send(200, action(data))
        except Exception as exc:
            self.send(400, {"error": self.server.app.redact(exc)})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8081)
    parser.add_argument("--server-url", default="http://localhost:8080")
    parser.add_argument("--postgres-container", default="nasiko-postgres-1")
    parser.add_argument("--gate", type=Path, help="Evaluate results.json as a pipeline gate; exits 0 only on PASS")
    parser.add_argument("--baseline")
    parser.add_argument("--candidate")
    args = parser.parse_args()
    if args.gate:
        if not args.baseline or not args.candidate:
            parser.error("--gate needs --baseline and --candidate")
        decision = evaluate_gate(json.loads(args.gate.read_text()), args.baseline, args.candidate)
        print(json.dumps(decision, indent=2))
        return {"PASS": 0, "BLOCK": 1, "INCONCLUSIVE": 2}[decision["decision"]]
    if not (ROOT / "run.py").exists():
        parser.error("Copy the web folder to your Nasiko repository's autoeval/web/ directory first.")
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    server.app = App(args.server_url, args.postgres_container)
    print(f"AutoEval Live: http://127.0.0.1:{args.port}\nNasiko: {args.server_url}\nKeep this terminal open. Ctrl+C stops the UI.", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.app.stop()
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
