"""Nasiko HTTP contracts plus a read-only ledger bridge for older server images."""

import fcntl
import json
import os
import re
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

from .schema import EvalError


def read_env(path):
    values = {}
    if path and Path(path).exists():
        for line in Path(path).read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                key, value = line.removeprefix("export ").split("=", 1)
                values[key.strip()] = value.strip().strip("\"'")
    return values


class NasikoClient:
    def __init__(self, base, env_path, postgres_container=None):
        self.base = base.rstrip("/")
        self.values = {**read_env(env_path), **os.environ}
        self.secrets = [
            v
            for k, v in self.values.items()
            if any(s in k.upper() for s in ("KEY", "SECRET", "PASSWORD", "TOKEN"))
            and len(v) > 5
        ]
        self.token = self.values.get("NASIKO_TOKEN")
        self.postgres_container = postgres_container
        self.agent = None
        self.previous_binding = None
        self.lock = None
        self.modern = False
        self.owner_id = None
        self.preflight = []

    def redact(self, value):
        for secret in self.secrets:
            value = value.replace(secret, "[redacted]")
        return re.sub(r"([?&]key=)[^&\s\"]+", r"\1[redacted]", value)

    def request(self, path, body=None, method=None, token=None):
        headers = {"Content-Type": "application/json"}
        credential = token or self.token
        if credential:
            headers["Authorization"] = "Bearer " + credential
        request = urllib.request.Request(
            self.base + path,
            data=json.dumps(body).encode() if body is not None else None,
            headers=headers,
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=240) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            raise EvalError(
                self.redact(
                    f"Nasiko HTTP {error.code} at {path}: "
                    + error.read().decode(errors="replace")[:1200]
                )
            ) from None
        except (OSError, ValueError) as error:
            raise EvalError(
                self.redact(f"Nasiko request failed at {path}: {error}")
            ) from None

    def login(self):
        if not self.token:
            username = self.values.get("NASIKO_USERNAME") or self.values.get(
                "ADMIN_USERNAME"
            )
            password = self.values.get("NASIKO_PASSWORD") or self.values.get(
                "ADMIN_PASSWORD"
            )
            if not username or not password:
                raise EvalError(
                    "Supply NASIKO_TOKEN or admin credentials via --env-file; never as CLI arguments"
                )
            self.token = self.request(
                "/api/auth/login", {"username": username, "password": password}
            )["token"]
        self.secrets.append(self.token)
        return self

    def catalog(self):
        return self.request("/api/llm-router/providers")["data"]

    def configurations(self):
        return self.request("/api/llm-configs")["data"]

    def resolve_model(self, selector):
        configs = self.configurations()
        found = next((c for c in configs if selector in (c["id"], c["name"])), None)
        if found:
            if not found.get("pinned") or found.get("fallback_models"):
                raise EvalError(
                    f"Config {selector} must be pinned and have no fallback models"
                )
            return found
        if ":" not in selector:
            raise EvalError(f"Unknown Nasiko model config: {selector}")
        provider, model = selector.split(":", 1)
        priced = [
            entry
            for group in self.catalog()
            if group["provider"] == provider
            for entry in group["models"]
            if entry.get("pricing_available")
            and (entry["model"] == model or entry["model"].startswith(model + "-lte-"))
        ]
        if not priced:
            raise EvalError(f"No Nasiko price-book entry found for {selector}")
        name = "autoeval-" + re.sub(r"[^a-zA-Z0-9_-]", "-", selector)
        found = next((c for c in configs if c["name"] == name), None)
        if found:
            return found
        return self.request(
            "/api/llm-configs",
            {
                "name": name,
                "provider": provider,
                "model": model,
                "pinned": True,
                "pinned_model": model,
                "temperature": 0,
                "fallback_models": [],
            },
        )["data"]

    def __enter__(self):
        self.login()
        self.lock = open(
            Path(tempfile.gettempdir()) / "nasiko-autoeval-router.lock", "a+"
        )
        try:
            fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            self.lock.close()
            raise EvalError(
                "Another AutoEval process is using the serial router identity"
            ) from None
        try:
            try:
                self.request("/api/evals/usage?run=autoeval-capability-check")
                self.modern = True
            except EvalError as error:
                if "HTTP 404" not in str(error):
                    raise
            if not self.modern and not self.postgres_container:
                raise EvalError(
                    "This server lacks /api/evals/usage; supply --postgres-container for the read-only ledger bridge"
                )
            integration = "eval" if self.modern else "opencode"
            data = self.request(
                "/api/agents/coding-integrations", {"integration_id": integration}
            )
            self.agent = data.get("data", data)
            self.owner_id = str(uuid.UUID(self.agent["owner_id"]))
            str(uuid.UUID(self.agent["id"]))
            self.previous_binding = self.request(
                f"/api/agents/{self.agent['id']}/llm-config"
            )["data"]
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *_):
        try:
            if self.previous_binding is not None:
                previous = self.previous_binding
                self.request(
                    f"/api/agents/{self.agent['id']}/llm-config",
                    {
                        "llm_config_id": previous.get("llm_config_id"),
                        "pinned_model": previous.get("pinned_model"),
                    },
                    "PATCH",
                )
        finally:
            if self.lock:
                self.lock.close()
                self.lock = None

    def _ledger_rows(self):
        agent = str(uuid.UUID(self.agent["id"]))
        owner = str(uuid.UUID(self.owner_id))
        query = (
            "SELECT COALESCE(json_agg(row_to_json(q)), '[]') FROM (SELECT trace_id, model, provider, "
            "router_input_tokens AS input_tokens, router_output_tokens AS output_tokens, "
            "router_cache_read_tokens AS cache_read_tokens, router_cache_creation_tokens AS cache_creation_tokens, "
            "router_cost_usd AS cost_usd, router_error_count AS failed_calls, router_estimated_input_tokens, "
            "cost_estimated, metadata->'pricing' AS pricing FROM trace_usage "
            f"WHERE agent_id='{agent}'::uuid AND user_id='{owner}'::uuid) q;"
        )
        result = subprocess.run(
            [
                "docker",
                "exec",
                self.postgres_container,
                "psql",
                "-X",
                "-U",
                "nasiko",
                "-d",
                "nasiko_dev",
                "-v",
                "ON_ERROR_STOP=1",
                "-At",
                "-c",
                "BEGIN READ ONLY; " + query + " COMMIT;",
            ],
            check=False,
            capture_output=True,
            timeout=30,
        )
        if result.returncode:
            raise EvalError(
                "Read-only usage ledger bridge failed: "
                + self.redact(result.stderr.decode())[:600]
            )
        line = next(
            (
                line
                for line in result.stdout.decode().splitlines()
                if line.startswith("[")
            ),
            None,
        )
        if line is None:
            raise EvalError("Usage ledger bridge returned no JSON")
        return json.loads(line)

    def _usage(self, tag, before, response):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.modern:
                run = urllib.parse.quote(tag.rsplit(":", 1)[0], safe="")
                rows = self.request(f"/api/evals/usage?run={run}&limit=1000")["data"]
                row = next((r for r in rows if r["attempt"] == tag), None)
                if row:
                    return {**row, "source": "GET /api/evals/usage"}
            else:
                rows = [r for r in self._ledger_rows() if r["trace_id"] not in before]
                if rows:
                    if len(rows) != 1:
                        raise EvalError(
                            "Ambiguous ledger attribution: multiple calls used the shared identity"
                        )
                    row = rows[0]
                    usage = response.get("usage", {})
                    reported_prompt = usage.get("prompt_tokens")
                    prompt_total = (
                        row["input_tokens"]
                        + row["cache_read_tokens"]
                        + row["cache_creation_tokens"]
                    )
                    if (
                        reported_prompt != prompt_total
                        or usage.get("completion_tokens") != row["output_tokens"]
                    ):
                        raise EvalError(
                            "Ledger tokens disagree with the routed response; attribution unverified"
                        )
                    return {
                        **row,
                        "calls": 1,
                        "source": "read-only trace_usage.router_* snapshot",
                        "attribution": "one new identity row; response-token cross-check",
                    }
            time.sleep(0.5)
        raise EvalError("Nasiko usage did not become available; cost is unknown")

    def complete(self, config, prompt, tag, max_tokens):
        model = config.get("pinned_model") or config["model"]
        self.request(
            f"/api/agents/{self.agent['id']}/llm-config",
            {"llm_config_id": config["id"], "pinned_model": model},
            "PATCH",
        )
        credential = self.request(f"/api/agents/{self.agent['id']}/llm-token", {})[
            "data"
        ]["token"]
        self.secrets.append(credential)
        before = (
            {r["trace_id"] for r in self._ledger_rows()} if not self.modern else set()
        )
        headers = {
            "Authorization": "Bearer " + credential,
            "Content-Type": "application/json",
            "x-nasiko-eval-attempt": tag,
        }
        body = {
            "model": "router",
            "temperature": 0,
            "stream": False,
            "max_tokens": max_tokens,
            "messages": [
                {
                    "role": "system",
                    "content": "Follow the task instructions. Return only the requested output.",
                },
                {"role": "user", "content": prompt},
            ],
        }
        started = time.monotonic()
        request = urllib.request.Request(
            self.base + "/v1/chat/completions",
            data=json.dumps(body).encode(),
            headers=headers,
        )
        try:
            with urllib.request.urlopen(request, timeout=240) as result:
                response = json.load(result)
        except urllib.error.HTTPError as error:
            raise EvalError(
                self.redact(
                    f"Model request HTTP {error.code}: " + error.read().decode()[:1000]
                )
            ) from None
        except (OSError, ValueError) as error:
            raise EvalError(self.redact(f"Model request failed: {error}")) from None
        duration = time.monotonic() - started
        record = {
            "duration_seconds": duration,
            "response": response,
            "usage": None,
            "cost_usd": None,
            "cost_error": None,
        }
        try:
            record["usage"] = self._usage(tag, before, response)
            record["cost_usd"] = verified_cost(record["usage"], model)
        except EvalError as error:
            record["cost_error"] = str(error)
        return record

    def probe(self, config, tag, allow_cost_gaps=False):
        record = self.complete(config, "Reply with exactly ROUTER_OK.", tag, 1024)
        text = (
            record["response"].get("choices", [{}])[0].get("message", {}).get("content")
        )
        if not text or record["cost_usd"] is None and not allow_cost_gaps:
            raise EvalError(
                "Model preflight needs a text response and attributable, non-estimated Nasiko cost: "
                + str(record["cost_error"])
            )
        return {
            "config": config["name"],
            "model": config.get("pinned_model") or config["model"],
            **record,
        }


def verified_cost(usage, model):
    served = usage.get("model") or usage.get("models", [None])[0]
    if served != model or "models" in usage and usage["models"] != [model]:
        raise EvalError(f"Requested model {model} but Nasiko served {served}")
    pricing = usage.get("pricing") or {}
    if "pricing" in usage:
        provenance_known = (
            pricing.get("source") in {"Exact", "Family"}
            and pricing.get("estimated") is False
        )
    else:
        provenance_known = (
            usage.get("worst_price_source") in {"Exact", "Family"}
            and usage.get("estimated_calls") == 0
        )
    estimated = (
        not provenance_known
        or usage.get("estimated_calls", 0)
        or usage.get("cost_estimated")
        or usage.get("router_estimated_input_tokens")
        or pricing.get("estimated", False)
    )
    if estimated:
        raise EvalError("Nasiko marked this attempt's price/usage as estimated")
    if usage["input_tokens"] + usage["output_tokens"] <= 0:
        raise EvalError("Nasiko recorded no provider token usage")
    value = usage.get("cost_usd")
    import math

    if not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
        raise EvalError("Nasiko returned no valid cost")
    return value
