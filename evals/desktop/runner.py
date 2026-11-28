"""Run trusted desktop-eval adapters and attach Nasiko's per-attempt usage.

Adapters own desktop isolation and communicate with the desktop through their
configured transport. The runner executes argv arrays directly on the host.
"""
import argparse
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
from urllib.error import URLError
from urllib.parse import urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener
from uuid import uuid4


def text(value):
    return isinstance(value, str) and bool(value.strip()) and "\x00" not in value


def command(argv):
    valid_args = isinstance(argv, list) and argv and all(
        isinstance(arg, str) and "\x00" not in arg for arg in argv
    )
    if not valid_args or not text(argv[0]):
        raise ValueError("adapter commands must be nonempty argv arrays")


def validate(manifest):
    if not isinstance(manifest, dict) or type(manifest.get("version")) is not int or manifest["version"] != 1:
        raise ValueError("expected desktop manifest version 1")
    tasks = manifest.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise ValueError("expected one or more tasks")
    ids = set()
    for task in tasks:
        if not isinstance(task, dict) or not text(task.get("id")) or not text(task.get("prompt")):
            raise ValueError("each task requires an id and prompt")
        if task["id"] in ids:
            raise ValueError(f"duplicate task id: {task['id']}")
        ids.add(task["id"])
        if "source" in task and not text(task["source"]):
            raise ValueError("source must be a nonempty string")
        for stage in ("reset", "grade"):
            command(task.get(stage))
    harnesses = manifest.get("harnesses")
    if not isinstance(harnesses, dict) or not harnesses:
        raise ValueError("expected named harness commands")
    for name, argv in harnesses.items():
        if not text(name):
            raise ValueError("empty harness name")
        command(argv)
    models = manifest.get("models")
    if not isinstance(models, list) or not models or not all(map(text, models)) or len(set(models)) != len(models):
        raise ValueError("expected a nonempty list of distinct model names")
    for key, default in (("trials", 1), ("timeout_seconds", 120)):
        value = manifest.get(key, default)
        if type(value) is not int or value < 1:
            raise ValueError(f"{key} must be a positive integer")


def invoke(argv, cwd, env, timeout, stdout=None):
    process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=stdout, start_new_session=os.name == "posix")
    try:
        return process.wait(timeout=timeout)
    finally:
        if process.poll() is None:
            if os.name == "posix":
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            else:
                process.kill()
            process.wait()


def screenshots(directory, output):
    frames = directory / "screenshots"
    if directory.is_symlink() or frames.is_symlink() or not frames.is_dir():
        raise ValueError("screenshot directory replaced or unavailable")
    result = []
    for path in sorted(frames.glob("*.png")):
        if path.is_symlink() or not path.is_file():
            raise ValueError("screenshot must be a regular file")
        with path.open("rb") as stream:
            if stream.read(8) != b"\x89PNG\r\n\x1a\n":
                raise ValueError("invalid screenshot PNG signature")
        result.append(path.relative_to(output).as_posix())
    return result


def attempt(manifest_dir, output, run_id, index, task, harness, argv, model, trial, timeout, server):
    slot = f"a{index:05d}"
    directory = output / slot
    (directory / "screenshots").mkdir(parents=True)
    tag = f"{run_id}:{slot}"
    env = dict(os.environ)
    env.pop("NASIKO_API_TOKEN", None)
    token = env.pop("NASIKO_ROUTER_TOKEN", None)
    env.pop("NASIKO_ROUTER_URL", None)
    env.update(DESKTOP_ATTEMPT_DIR=str(directory), DESKTOP_TASK_ID=task["id"],
               DESKTOP_PROMPT=task["prompt"], DESKTOP_HARNESS=harness,
               DESKTOP_MODEL=model, DESKTOP_TRIAL=str(trial), NASIKO_EVAL_ATTEMPT=tag)
    record = dict(id=slot, task=task["id"], harness=harness, model=model, trial=trial,
                  outcome="setup_error", resolved=None, seconds=0.0, screenshots=[],
                  router_tag=tag, router_usage=None, router_usd=None,
                  usage_status="unavailable" if server else "offline")
    try:
        if invoke(task["reset"], manifest_dir, env, timeout):
            return record
        record["outcome"] = "harness_error"
        harness_env = env.copy()
        if server:
            harness_env.update(NASIKO_ROUTER_URL=server + "/v1", NASIKO_ROUTER_TOKEN=token)
        started = time.monotonic()
        try:
            exit_code = invoke(argv, manifest_dir, harness_env, timeout)
            ended = "finished" if exit_code == 0 else "harness_error"
        except subprocess.TimeoutExpired:
            ended = "time_limit"
        finally:
            record["seconds"] = round(time.monotonic() - started, 3)
        record["outcome"] = "grader_error"
        record["harness_outcome"] = ended
        with tempfile.TemporaryFile() as grade_output:
            if invoke(task["grade"], manifest_dir, env, timeout, stdout=grade_output):
                return record
            if grade_output.tell() > 65536:
                raise ValueError("grader result exceeds 64 KiB")
            grade_output.seek(0)
            grade = json.load(grade_output)
            if not isinstance(grade, dict) or type(grade.get("resolved")) is not bool:
                raise ValueError("grader expected a boolean resolved field")
            record.update(outcome=ended, resolved=grade["resolved"])
    except (OSError, ValueError, subprocess.TimeoutExpired) as exc:
        print(f"{slot}: {record['outcome']} ({type(exc).__name__})", file=sys.stderr)
    finally:
        try:
            record["screenshots"] = screenshots(directory, output)
        except (OSError, ValueError) as exc:
            record["evidence_error"] = str(exc)
    return record


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def collect_usage(server, run_id, token):
    rows = {}
    params = {"run": run_id, "limit": 1000}
    opener = build_opener(NoRedirect())
    cursors = set()
    while True:
        url = server + "/api/evals/usage?" + urlencode(params)
        req = Request(url, headers={"Authorization": "Bearer " + token})
        with opener.open(req, timeout=20) as response:
            page = json.load(response)
        for row in page["data"]:
            tag = row["attempt"]
            if tag in rows or not tag.startswith(run_id + ":"):
                raise ValueError("unexpected attempt tag in router usage")
            rows[tag] = row
        cursor = page.get("next_cursor")
        if not cursor:
            return rows
        if cursor in cursors:
            raise ValueError("router usage cursor repeated")
        cursors.add(cursor)
        params["cursor"] = cursor


def attach_usage(report, server):
    try:
        rows = collect_usage(server, report["run_id"], os.environ["NASIKO_API_TOKEN"])
    except (OSError, URLError, ValueError, TypeError, KeyError) as exc:
        print(f"Nasiko usage lookup failed ({type(exc).__name__})", file=sys.stderr)
        for record in report["attempts"]:
            if record["router_usage"] is None:
                record["usage_status"] = "lookup_error"
        return
    for record in report["attempts"]:
        row = rows.get(record["router_tag"])
        record.update(router_usage=row, router_usd=None, usage_status="unavailable")
        if row is None:
            continue
        if row.get("models") != [record["model"]]:
            record["usage_status"] = "model_mismatch"
            continue
        usd = row.get("cost_usd")
        priced = row.get("worst_price_source") in ("Exact", "Family", "Static", "Ratio")
        valid_amount = type(usd) in (int, float) and math.isfinite(usd) and usd >= 0
        if not priced or not valid_amount:
            record["usage_status"] = "unpriced"
            continue
        record.update(router_usd=usd, usage_status="recorded")


def save(report, output):
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=output, delete=False) as f:
        json.dump(report, f, indent=2, allow_nan=False)
        f.write("\n")
    os.replace(f.name, output / "report.json")


def server_url(value):
    url = urlsplit(value)
    if not url.hostname or url.username or url.password or url.query or url.fragment:
        raise ValueError("expected a Nasiko server base URL")
    if url.scheme != "https" and not (url.scheme == "http" and url.hostname in ("127.0.0.1", "localhost", "::1")):
        raise ValueError("use HTTPS for a remote Nasiko server")
    return value.rstrip("/")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path, nargs="?")
    parser.add_argument("--output", required=True, type=Path, help="new results directory, or existing directory with --refresh-usage")
    parser.add_argument("--server", help="Nasiko server base URL; omit for an offline fixture")
    parser.add_argument("--refresh-usage", action="store_true", help="refresh a saved report without executing adapters")
    args = parser.parse_args()
    server = server_url(args.server) if args.server else None
    output = args.output.resolve()
    if server and not os.environ.get("NASIKO_API_TOKEN"):
        parser.error("--server requires NASIKO_API_TOKEN")
    if args.refresh_usage:
        if args.manifest or not server:
            parser.error("--refresh-usage requires --server and no manifest")
        report = json.loads((output / "report.json").read_text(encoding="utf-8"))
    else:
        if not args.manifest or (server and not os.environ.get("NASIKO_ROUTER_TOKEN")):
            parser.error("supply a manifest and, for --server, NASIKO_ROUTER_TOKEN")
        manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
        validate(manifest)
        output.mkdir(parents=True, exist_ok=False)
        report = {"version": 1, "run_id": "desktop-" + uuid4().hex, "tasks": [
            {k: t[k] for k in ("id", "prompt", "source") if k in t} for t in manifest["tasks"]], "attempts": []}
        save(report, output)
        for task in manifest["tasks"]:
            for harness, argv in manifest["harnesses"].items():
                for model in manifest["models"]:
                    for trial in range(1, manifest.get("trials", 1) + 1):
                        row = attempt(args.manifest.resolve().parent, output, report["run_id"],
                                      len(report["attempts"]) + 1, task, harness, argv, model, trial,
                                      manifest.get("timeout_seconds", 120), server)
                        report["attempts"].append(row)
                        save(report, output)
    if server:
        attach_usage(report, server)
        save(report, output)
    print(output / "report.json")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        sys.exit(str(error))
