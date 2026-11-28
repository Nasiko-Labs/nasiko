"""Run Codex or OpenCode with one desktop MCP server and a tagged Nasiko provider."""
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile


def launch(harness, model, prompt, mcp, root, env):
    tag = env["NASIKO_EVAL_ATTEMPT"]
    url = env["NASIKO_ROUTER_URL"]
    desktop = {key: env[key] for key in ("DESKTOP_ATTEMPT_DIR", "DISPLAY", "XAUTHORITY")
               if key in env}
    header = "x-nasiko-eval-attempt"
    if harness == "codex":
        args = ["codex", "exec", "--ephemeral", "--ignore-user-config", "--disable", "shell_tool",
                "--disable", "unified_exec", "--disable", "plugins", "--sandbox", "read-only",
                "--skip-git-repo-check", "--json"]
        for feature in ("view_image", "multi_agent", "multi_agent_v2", "goals", "apps", "hooks"):
            args.extend(["--disable", feature])
        settings = {
            "model_provider": "nasiko", "web_search": "disabled",
            "model_providers.nasiko.name": "Nasiko", "model_providers.nasiko.base_url": url,
            "model_providers.nasiko.wire_api": "responses",
            "model_providers.nasiko.env_key": "NASIKO_ROUTER_TOKEN",
            f"model_providers.nasiko.http_headers.{header}": tag,
            "mcp_servers.desktop.command": mcp,
        }
        for name, value in desktop.items():
            settings[f"mcp_servers.desktop.env.{name}"] = value
        for tool in ("observe", "click", "type_text", "key"):
            settings[f"mcp_servers.desktop.tools.{tool}.approval_mode"] = "approve"
        for key, value in settings.items():
            args.extend(["-c", f"{key}={json.dumps(value, ensure_ascii=False)}"])
        return args + ["-m", model, "--", prompt]

    config = {
        "enabled_providers": ["nasiko"],
        "provider": {"nasiko": {
            "npm": "@ai-sdk/openai-compatible", "name": "Nasiko",
            "options": {"baseURL": url, "apiKey": "{env:NASIKO_ROUTER_TOKEN}",
                        "headers": {header: tag}},
            "models": {model: {"name": model, "modalities": {
                "input": ["text", "image"], "output": ["text"]}}},
        }},
        "mcp": {"desktop": {"type": "local", "command": [mcp], "environment": desktop}},
        "permission": {"*": "deny", "desktop_*": "allow"},
        "small_model": f"nasiko/{model}",
    }
    path = root / "opencode.json"
    path.write_text(json.dumps(config), encoding="utf-8")
    env["OPENCODE_CONFIG"] = str(path)
    return ["opencode", "run", "--pure", "--format", "json", "--model", f"nasiko/{model}",
            "--", prompt]


# Provider messages can echo credentials, headers, prompts or response bodies. Persist
# only a recognized diagnosis and structured status, never arbitrary client text.
ERROR_KINDS = {
    "authentication failed": ("unauthorized", "authentication", "invalid api key", "invalid_api_key"),
    "access denied": ("forbidden", "permission denied"),
    "rate limited": ("rate limit", "rate_limit", "too many requests"),
    "quota exhausted": ("insufficient_quota", "quota exceeded"),
    "model unavailable": ("model_not_found", "model not found", "unknown model"),
    "context limit exceeded": ("context_length_exceeded", "context window"),
    "request timed out": ("timed out", "timeout"),
    "connection failed": ("econnrefused", "connection refused", "connection reset"),
}


def client_diagnostic(event):
    error = event.get("error")
    error = error if isinstance(error, dict) else {}
    data = error.get("data")
    data = data if isinstance(data, dict) else {}
    fields = [event.get("message"), event.get("error"), error.get("message"),
              error.get("code"), data.get("message"), data.get("code")]
    description = " ".join(value for value in fields if isinstance(value, str)).lower()
    reason = next((kind for kind, hints in ERROR_KINDS.items()
                   if any(hint in description for hint in hints)), "unclassified client failure")
    entry = {"event": event["type"], "reason": reason}
    status = data.get("statusCode", error.get("statusCode"))
    if type(status) is int and 400 <= status <= 599:
        entry["http_status"] = status
    else:
        match = re.search(r"\b(?:http(?: status)?|status(?: code)?)\s*[:=]?\s*([45]\d\d)\b", description)
        if match:
            entry["http_status"] = int(match[1])
    return entry


def log_failure(directory, entry):
    with (directory / "harness-errors.jsonl").open("a", encoding="utf-8") as log:
        print(json.dumps(entry), file=log)


def main():
    harness = os.environ["DESKTOP_HARNESS"]
    if harness not in ("codex", "opencode"):
        sys.exit("Expected codex or opencode as the harness name")
    mcp = Path(os.environ["NASIKO_DESKTOP_MCP"])
    if not mcp.is_absolute() or not mcp.is_file() or not os.access(mcp, os.X_OK):
        sys.exit("NASIKO_DESKTOP_MCP must name an absolute executable stdio MCP server")
    if not os.environ.get("NASIKO_ROUTER_TOKEN") or not os.environ.get("NASIKO_ROUTER_URL"):
        sys.exit("Configure the eval router token and server before running a harness")
    keys = ("PATH", "LANG", "LC_ALL", "TMPDIR", "DISPLAY", "XAUTHORITY", "SSL_CERT_FILE",
            "DESKTOP_ATTEMPT_DIR", "NASIKO_ROUTER_TOKEN", "NASIKO_ROUTER_URL", "NASIKO_EVAL_ATTEMPT")
    env = {key: os.environ[key] for key in keys if key in os.environ}
    with tempfile.TemporaryDirectory(prefix="nasiko-desktop-") as temp:
        root = Path(temp)
        for key in ("HOME", "CODEX_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"):
            directory = root / key.lower()
            directory.mkdir()
            env[key] = str(directory)
        prompt = "Operate only the provided desktop through its MCP tools. " + os.environ["DESKTOP_PROMPT"]
        args = launch(harness, os.environ["DESKTOP_MODEL"], prompt, str(mcp), root, env)
        directory = Path(env["DESKTOP_ATTEMPT_DIR"])
        failed = False
        with subprocess.Popen(args, cwd=root, env=env, stdin=subprocess.DEVNULL,
                              stdout=subprocess.PIPE, text=True) as proc:
            for line in proc.stdout:
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                if isinstance(event, dict) and event.get("type") in ("error", "turn.failed"):
                    log_failure(directory, client_diagnostic(event))
                    failed = True
        if proc.returncode:
            log_failure(directory, {"event": "client_exit", "exit_code": proc.returncode})
        return proc.returncode or int(failed)


if __name__ == "__main__":
    sys.exit(main())
