"""Governed contribution pipeline.

ingest -> propose (G1) -> build files (G2) -> publish (G3) -> pull request

Every public write sits behind a human gate, writes are limited to an
allow-list of upstream repositories (and the caller's own fork of them),
and every LLM call is metered against a per-thread budget.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import time
import uuid
from pathlib import Path
from typing import Any

import httpx

logger = logging.getLogger(__name__)

GITHUB_API = "https://api.github.com"
REPO_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
# USD per 1M tokens (input, output). Unknown models fall back to gpt-4o-mini pricing.
PRICES = {
    "gpt-4o-mini": (0.15, 0.60),
    "gpt-4o": (2.50, 10.00),
    "gpt-4.1-mini": (0.40, 1.60),
    "gpt-4.1": (2.00, 8.00),
}
GATES = {
    "G1": "Approve this contribution idea?",
    "G2": "Approve these exact files for commit?",
    "G3": "Publish: push a branch to your fork and open the pull request?",
}
CHOICES = ["approve", "reject"]
ALLOWED_PREFIXES = ("agents/", "docs/")
SELF_TARGET = "agents/oss-contribution-copilot"
SELF_FILES = (
    "AgentCard.json",
    "Dockerfile",
    "README.md",
    "pyproject.toml",
    "docker-compose.yml",
    ".env.example",
    "src/__init__.py",
    "src/__main__.py",
    "src/agent_executor.py",
    "src/copilot.py",
    "src/rest.py",
    "src/telemetry.py",
)


class GateError(Exception):
    """A decision that does not match the thread's open gate."""


class PolicyError(Exception):
    """A request the safety policy refuses (allow-list, budget, path jail)."""


def _env_list(name: str, default: str) -> list[str]:
    return [r.strip() for r in os.getenv(name, default).split(",") if r.strip()]


class Config:
    def __init__(self) -> None:
        self.github_token = os.getenv("GITHUB_TOKEN", "")
        self.writable_repos = {r.lower() for r in _env_list("WRITABLE_REPOS", "Nasiko-Labs/nasiko")}
        self.openai_key = os.getenv("OPENAI_API_KEY", "")
        self.openai_base = os.getenv("OPENAI_BASE_URL") or None
        self.model = os.getenv("MODEL", "gpt-4o-mini")
        self.public_url = os.getenv("PUBLIC_URL", "http://localhost:8001").rstrip("/")
        self.approver_key = os.getenv("APPROVER_KEY", "")
        self.budget_usd = float(os.getenv("MAX_COST_USD", "0.50"))
        self.mode = os.getenv("CONTRIBUTION_MODE", "auto")  # auto | self | docs
        self.data_dir = Path(os.getenv("DATA_DIR", "./data"))
        self.bundle_dir = Path(os.getenv("SELF_BUNDLE_DIR", Path(__file__).resolve().parent.parent))


class Store:
    """Threads in memory, persisted to one JSON file so a restart loses nothing."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.threads: dict[str, dict[str, Any]] = {}
        if path.exists():
            try:
                self.threads = json.loads(path.read_text())
            except (OSError, json.JSONDecodeError):
                logger.warning("could not read %s; starting empty", path)

    def save(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps(self.threads, indent=2))
        tmp.replace(self.path)


class Copilot:
    def __init__(self, cfg: Config | None = None) -> None:
        self.cfg = cfg or Config()
        self.store = Store(self.cfg.data_dir / "threads.json")
        self._tasks: set[asyncio.Task] = set()

    # ---------------------------------------------------------------- views
    def projection(self, t: dict[str, Any]) -> dict[str, Any]:
        """The only shape that crosses the wire: small, stable, chat-friendly."""
        gate = t.get("gate")
        if gate:
            nxt = "waiting_on_human"
        elif t["phase"] in ("pr_open",):
            nxt = "done"
        elif t["phase"] in ("rejected", "failed"):
            nxt = "stopped"
        else:
            nxt = "working"
        return {
            "thread_id": t["id"],
            "repo": t["repo"],
            "phase": t["phase"],
            "headline": t["headline"],
            "gate": gate,
            "next": nxt,
            "cost_usd": round(t["cost_usd"], 4),
            "pr_url": t.get("pr_url"),
            "detail_url": f"{self.cfg.public_url}/work/{t['id']}",
        }

    def get(self, thread_id: str) -> dict[str, Any]:
        t = self.store.threads.get(thread_id)
        if not t:
            raise KeyError(f"unknown thread_id {thread_id!r}")
        return t

    def status(self, thread_id: str) -> dict[str, Any]:
        return self.projection(self.get(thread_id))

    def approvals(self) -> list[dict[str, Any]]:
        return [self.projection(t) for t in self.store.threads.values() if t.get("gate")]

    # ------------------------------------------------------------ lifecycle
    def contribute(self, repo: str) -> dict[str, Any]:
        repo = (repo or "").strip().removeprefix("https://github.com/").strip("/")
        if not REPO_RE.match(repo):
            raise PolicyError("repo must look like owner/name")
        if repo.lower() not in self.cfg.writable_repos:
            raise PolicyError(
                f"{repo} is not on the writable allow-list. "
                "Add it to WRITABLE_REPOS (one repo per entry, no wildcards) and restart."
            )
        tid = "t_" + uuid.uuid4().hex[:12]
        t = {
            "id": tid,
            "repo": repo,
            "phase": "ingest",
            "headline": "Reading the repository",
            "gate": None,
            "cost_usd": 0.0,
            "created": time.time(),
            "events": [],
            "proposal": None,
            "files": [],
            "pr_url": None,
        }
        self.store.threads[tid] = t
        self._log(t, "started", f"contribution requested for {repo}")
        self._spawn(self._ingest_and_propose(t), t)
        return self.projection(t)

    def decide(self, gate_id: str, decision: str, note: str = "", thread_id: str = "",
               approver: str = "human") -> dict[str, Any]:
        decision = (decision or "").strip().lower()
        if decision not in CHOICES:
            raise GateError("decision must be exactly 'approve' or 'reject'")
        t = self._thread_for_gate(gate_id, thread_id)
        gate = t["gate"]
        # A decision lands only on the gate it names.
        if not gate or gate["id"] != gate_id:
            raise GateError(f"thread {t['id']} has no open gate {gate_id}")
        t["gate"] = None
        self._log(t, f"{gate_id}:{decision}", note or "", actor=approver)
        if decision == "reject":
            self._set(t, "rejected", f"Stopped: a human rejected {gate_id}")
            return self.projection(t)
        next_step = {"G1": self._build_files, "G2": self._open_g3, "G3": self._publish}[gate_id]
        self._set(t, t["phase"], f"{gate_id} approved — continuing")
        self._spawn(next_step(t), t)
        return self.projection(t)

    def _thread_for_gate(self, gate_id: str, thread_id: str) -> dict[str, Any]:
        if thread_id:
            return self.get(thread_id)
        open_ = [t for t in self.store.threads.values() if (t.get("gate") or {}).get("id") == gate_id]
        if len(open_) != 1:
            raise GateError(
                f"{len(open_)} threads have gate {gate_id} open; pass thread_id to choose one"
            )
        return open_[0]

    # ---------------------------------------------------------------- steps
    async def _ingest_and_propose(self, t: dict[str, Any]) -> None:
        async with self._gh() as gh:
            repo = (await gh.get(f"/repos/{t['repo']}")).raise_for_status().json()
            t["default_branch"] = repo["default_branch"]
            tree = await self._tree(gh, t["repo"], repo["default_branch"])
        paths = {p["path"] for p in tree}
        t["repo_paths_count"] = len(paths)
        self._log(t, "ingested", f"{len(paths)} paths on {repo['default_branch']}")

        self._set(t, "triage", "Choosing a contribution")
        proposal = self._pick_proposal(t["repo"], paths)
        t["proposal"] = proposal
        self._log(t, "proposed", proposal["title"])
        self._open_gate(t, "G1", f"Proposal: {proposal['title']}")

    def _pick_proposal(self, repo: str, paths: set[str]) -> dict[str, Any]:
        mode = self.cfg.mode
        self_ok = repo.lower() == "nasiko-labs/nasiko" and f"{SELF_TARGET}/README.md" not in paths
        if mode in ("auto", "self") and self_ok and (self.cfg.bundle_dir / "README.md").exists():
            return {
                "kind": "self",
                "title": "Add oss-contribution-copilot example agent",
                "why": "A Python A2A agent that ships governed open-source PRs; fits the "
                       "house pattern in agents/ and touches no Rust.",
                "target": SELF_TARGET,
            }
        agents = sorted({p.split("/")[1] for p in paths if p.startswith("agents/") and p.count("/") >= 2})
        missing = [a for a in agents if f"agents/{a}/README.md" not in paths]
        if not missing:
            raise PolicyError("no safe contribution found (every example agent already has a README)")
        a = missing[0]
        return {
            "kind": "docs",
            "title": f"docs(agents): add README for {a}",
            "why": f"agents/{a} has no README; new contributors cannot tell what it does or how to run it.",
            "target": f"agents/{a}",
        }

    async def _build_files(self, t: dict[str, Any]) -> None:
        self._set(t, "build", "Preparing files")
        p = t["proposal"]
        if p["kind"] == "self":
            files = self._self_files(p["target"])
        else:
            files = [await self._write_readme(t, p["target"])]
        async with self._gh() as gh:
            existing = {x["path"] for x in await self._tree(gh, t["repo"], t["default_branch"])}
        for f in files:
            self._jail(f["path"], existing)
        t["files"] = files
        t["pr_title"] = p["title"] if p["kind"] == "docs" else "feat(agents): add oss-contribution-copilot example agent"
        t["pr_body"] = self._pr_body(t)
        self._log(t, "built", f"{len(files)} file(s), {sum(len(f['content']) for f in files)} bytes")
        self._open_gate(t, "G2", f"{len(files)} new file(s) ready for review")

    async def _open_g3(self, t: dict[str, Any]) -> None:
        if not self.cfg.github_token:
            self._fail(t, "GITHUB_TOKEN is not set; cannot publish")
            return
        self._open_gate(t, "G3", f"Ready to open PR: {t['pr_title']}")

    async def _publish(self, t: dict[str, Any]) -> None:
        self._set(t, "publish", "Pushing branch to fork and opening PR")
        up = t["repo"]
        async with self._gh(auth=True) as gh:
            login = (await gh.get("/user")).raise_for_status().json()["login"]
            fork = await self._ensure_fork(gh, up, login)
            base_ref = (await gh.get(f"/repos/{up}/git/ref/heads/{t['default_branch']}")).raise_for_status().json()
            base_sha = base_ref["object"]["sha"]
            await gh.post(f"/repos/{fork}/merge-upstream", json={"branch": t["default_branch"]})
            base_tree = (await gh.get(f"/repos/{up}/git/commits/{base_sha}")).raise_for_status().json()["tree"]["sha"]
            tree_items = []
            for f in t["files"]:
                blob = (await gh.post(f"/repos/{fork}/git/blobs",
                                      json={"content": f["content"], "encoding": "utf-8"})).raise_for_status().json()
                tree_items.append({"path": f["path"], "mode": "100644", "type": "blob", "sha": blob["sha"]})
            tree = (await gh.post(f"/repos/{fork}/git/trees",
                                  json={"base_tree": base_tree, "tree": tree_items})).raise_for_status().json()
            commit = (await gh.post(f"/repos/{fork}/git/commits", json={
                "message": t["pr_title"], "tree": tree["sha"], "parents": [base_sha],
            })).raise_for_status().json()
            branch = f"copilot/{t['id']}"
            (await gh.post(f"/repos/{fork}/git/refs",
                           json={"ref": f"refs/heads/{branch}", "sha": commit["sha"]})).raise_for_status()
            self._log(t, "pushed", f"{fork}@{branch} {commit['sha'][:7]}")
            t["pr_body"] = self._pr_body(t)  # refresh cost line
            pr = (await gh.post(f"/repos/{up}/pulls", json={
                "title": t["pr_title"], "head": f"{login}:{branch}", "base": t["default_branch"],
                "body": t["pr_body"], "maintainer_can_modify": True,
            })).raise_for_status().json()
        t["pr_url"] = pr["html_url"]
        self._log(t, "pr_opened", pr["html_url"])
        self._set(t, "pr_open", f"PR opened: {pr['html_url']}")

    # -------------------------------------------------------------- helpers
    async def _ensure_fork(self, gh: httpx.AsyncClient, upstream: str, login: str) -> str:
        if upstream.lower() not in self.cfg.writable_repos:
            raise PolicyError(f"{upstream} is not on the writable allow-list")
        r = (await gh.post(f"/repos/{upstream}/forks")).raise_for_status().json()
        fork = r["full_name"]
        for _ in range(30):
            info = await gh.get(f"/repos/{fork}")
            if info.status_code == 200:
                parent = (info.json().get("parent") or {}).get("full_name", "")
                # The fork is writable only because its parent is allow-listed.
                if info.json()["owner"]["login"] != login or parent.lower() != upstream.lower():
                    raise PolicyError(f"{fork} is not {login}'s fork of {upstream}")
                return fork
            await asyncio.sleep(2)
        raise RuntimeError(f"fork {fork} did not become ready")

    def _self_files(self, target: str) -> list[dict[str, str]]:
        out = []
        for rel in SELF_FILES:
            src = self.cfg.bundle_dir / rel
            if src.is_file():
                out.append({"path": f"{target}/{rel}", "content": src.read_text()})
        if not out:
            raise PolicyError(f"bundle dir {self.cfg.bundle_dir} has none of the agent's files")
        return out

    async def _write_readme(self, t: dict[str, Any], target: str) -> dict[str, str]:
        async with self._gh() as gh:
            tree = await self._tree(gh, t["repo"], t["default_branch"])
            srcs = [x["path"] for x in tree if x["path"].startswith(target + "/") and x.get("size", 0) < 20_000]
            snippets = []
            for path in srcs[:8]:
                r = await gh.get(f"https://raw.githubusercontent.com/{t['repo']}/{t['default_branch']}/{path}")
                if r.status_code == 200:
                    snippets.append(f"--- {path}\n{r.text[:4000]}")
        name = target.split("/")[-1]
        if not self.cfg.openai_key:
            body = f"# {name}\n\nExample A2A agent for Nasiko.\n\n## Files\n\n" + "\n".join(f"- `{s}`" for s in srcs)
            return {"path": f"{target}/README.md", "content": body + "\n"}
        prompt = (
            "Write a concise, accurate README.md for this example agent in the Nasiko repo. "
            "Sections: what it does, how to run (Docker), environment variables, A2A endpoint. "
            "Use only facts visible in the files. Output markdown only.\n\n" + "\n\n".join(snippets)
        )
        text = await self._llm(t, prompt)
        return {"path": f"{target}/README.md", "content": text.strip() + "\n"}

    async def _llm(self, t: dict[str, Any], prompt: str) -> str:
        if t["cost_usd"] >= self.cfg.budget_usd:
            raise PolicyError(f"budget ${self.cfg.budget_usd:.2f} exhausted")
        from openai import AsyncOpenAI

        client = AsyncOpenAI(api_key=self.cfg.openai_key, base_url=self.cfg.openai_base)
        resp = await client.chat.completions.create(
            model=self.cfg.model, messages=[{"role": "user", "content": prompt}], temperature=0.2,
        )
        u = resp.usage
        pin, pout = PRICES.get(self.cfg.model, PRICES["gpt-4o-mini"])
        cost = (u.prompt_tokens * pin + u.completion_tokens * pout) / 1_000_000 if u else 0.0
        t["cost_usd"] += cost
        self._log(t, "llm", f"{self.cfg.model} in={getattr(u, 'prompt_tokens', 0)} "
                            f"out={getattr(u, 'completion_tokens', 0)} ${cost:.4f}")
        return resp.choices[0].message.content or ""

    def _jail(self, path: str, existing: set[str]) -> None:
        if path.startswith("/") or ".." in path.split("/") or not path.startswith(ALLOWED_PREFIXES):
            raise PolicyError(f"path {path!r} is outside the allowed directories")
        if path in existing:
            raise PolicyError(f"{path} already exists upstream; this agent only adds new files")

    def _pr_body(self, t: dict[str, Any]) -> str:
        p = t["proposal"]
        files = "\n".join(f"- `{f['path']}`" for f in t["files"])
        gates = "\n".join(f"- {e['type']} by {e['actor']} at {time.strftime('%H:%M:%S', time.gmtime(e['ts']))} UTC"
                          for e in t["events"] if e["type"].startswith("G"))
        return (
            f"## What\n{p['why']}\n\n## Files\n{files}\n\n"
            "## How this PR was made\n"
            "Opened by the **OSS Contribution Copilot**, an A2A agent, under human approval gates:\n"
            "G1 (idea), G2 (exact files), G3 (public write). No gate can be passed by the agent itself.\n\n"
            f"Gate log:\n{gates or '- (pending)'}\n\n"
            f"LLM cost for this contribution: **${t['cost_usd']:.4f}**\n"
        )

    async def _tree(self, gh: httpx.AsyncClient, repo: str, branch: str) -> list[dict[str, Any]]:
        r = (await gh.get(f"/repos/{repo}/git/trees/{branch}", params={"recursive": "1"})).raise_for_status()
        return [x for x in r.json()["tree"] if x["type"] == "blob"]

    def _gh(self, auth: bool = False) -> httpx.AsyncClient:
        headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28",
                   "User-Agent": "oss-contribution-copilot"}
        if self.cfg.github_token:
            headers["Authorization"] = f"Bearer {self.cfg.github_token}"
        elif auth:
            raise PolicyError("GITHUB_TOKEN is required for this step")
        return httpx.AsyncClient(base_url=GITHUB_API, headers=headers, timeout=30, follow_redirects=True)

    def _open_gate(self, t: dict[str, Any], gid: str, headline: str) -> None:
        t["gate"] = {"id": gid, "question": GATES[gid], "choices": CHOICES}
        self._set(t, {"G1": "proposal", "G2": "review", "G3": "ready_to_publish"}[gid], headline)

    def _set(self, t: dict[str, Any], phase: str, headline: str) -> None:
        t["phase"], t["headline"] = phase, headline
        self.store.save()

    def _fail(self, t: dict[str, Any], msg: str) -> None:
        t["gate"] = None
        self._log(t, "failed", msg)
        self._set(t, "failed", f"Failed: {msg}")

    def _log(self, t: dict[str, Any], typ: str, detail: str, actor: str = "copilot") -> None:
        t["events"].append({"ts": time.time(), "type": typ, "detail": detail, "actor": actor})
        self.store.save()

    def _spawn(self, coro, t: dict[str, Any]) -> None:

        async def guarded():
            try:
                await coro
            except Exception as e:  # surface every failure on the thread, never swallow it
                logger.exception("step failed")
                detail = e.response.text[:300] if isinstance(e, httpx.HTTPStatusError) else str(e)
                self._fail(t, f"{type(e).__name__}: {detail}")

        task = asyncio.get_running_loop().create_task(guarded())
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)
