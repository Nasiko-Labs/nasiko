"""Pi coding-agent helpers used inside the Nasiko coder worker."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import threading
import urllib.error
import urllib.request


def openai_base(url: str) -> str:
    base = url.rstrip("/")
    return base if base.endswith("/v1") else base + "/v1"


def models_file(base_url: str, api_key: str, model: str) -> dict:
    return {
        "providers": {
            "agentkv": {
                "baseUrl": openai_base(base_url),
                "api": "openai-completions",
                "apiKey": api_key,
                "compat": {"supportsDeveloperRole": False, "supportsReasoningEffort": False},
                "models": [{
                    "id": model,
                    "reasoning": False,
                    "input": ["text"],
                    "contextWindow": 8192,
                    "maxTokens": 512,
                }],
            }
        }
    }


def config_dir(home: Path) -> Path:
    path = home / ".pi" / "agent"
    path.mkdir(parents=True, exist_ok=True)
    return path


def models_path(home: Path) -> Path:
    path = home / ".pi"
    path.mkdir(parents=True, exist_ok=True)
    return path / "models.json"


def write_runtime(home: Path, models: dict) -> Path:
    """Pi catalogs live in ~/.pi/agent/models.json; some builds also read ~/.pi/models.json."""
    text = json.dumps(models)
    spec = models_path(home)
    spec.write_text(text)
    (config_dir(home) / "models.json").write_text(text)
    settings = {
        "defaultProvider": "agentkv",
        "defaultModel": models["providers"]["agentkv"]["models"][0]["id"],
        "defaultThinkingLevel": "off",
        "defaultTools": ["read", "edit", "write"],
        "defaultProjectTrust": "always",
        "packages": [],
        "extensions": [],
        "enableAnalytics": False,
        "enableInstallTelemetry": False,
        "quietStartup": True,
    }
    settings_text = json.dumps(settings)
    (home / ".pi" / "settings.json").write_text(settings_text)
    (config_dir(home) / "settings.json").write_text(settings_text)
    return spec


def write_workspace(root: Path, source: str, system: str, user: str) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    (root / "module.py").write_text(source)
    (root / "SYSTEM.md").write_text(system)
    (root / "AGENTS.md").write_text(
        "Edit module.py only. Do not run tests, install packages, or call the network.\n\n" + user
    )
    return root / "module.py"


def extract_code(root: Path, fallback: str = "") -> str:
    path = root / "module.py"
    text = path.read_text() if path.exists() else fallback
    return text.strip()


def pi_command(model: str, prompt: str, api_key: str = "") -> list:
    # --offline skips Pi's startup git clone / npm self-update. Those paths
    # exited 1 in the Modal worker before any Qwen completion.
    command = ["pi", "--offline", "--approve", "--mode", "json", "--provider", "agentkv",
               "--model", model, "--thinking", "off", "--tools", "read,edit,write"]
    if api_key:
        command += ["--api-key", api_key]
    return command + ["-p", "--", prompt]


def pi_env(base: dict) -> dict:
    env = {**base, "PI_OFFLINE": "1", "PI_SKIP_VERSION_CHECK": "1", "PI_TELEMETRY": "0"}
    env.pop("OPENAI_BASE_URL", None)
    env["PATH"] = "/usr/local/bin:/usr/bin:/bin:" + env.get("PATH", "")
    return env


def merge_usage(parts):
    prompt = sum((item or {}).get("prompt_tokens") or 0 for item in parts)
    completion = sum((item or {}).get("completion_tokens") or 0 for item in parts)
    cached = sum(((item or {}).get("prompt_tokens_details") or {}).get("cached_tokens") or 0 for item in parts)
    if not parts:
        return None
    return {"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion,
            "prompt_tokens_details": {"cached_tokens": cached}}


def prepare_completion_body(body, request_id):
    payload = dict(body)
    payload["temperature"] = 0
    payload["stream"] = False
    payload.pop("stream_options", None)
    payload["chat_template_kwargs"] = {"enable_thinking": False}
    if payload.get("max_tokens") is not None:
        payload["max_tokens"] = max(1, min(int(payload["max_tokens"]), 512))
    payload["request_id"] = request_id
    return payload


def sse_wrap(payload: bytes) -> bytes:
    return b"data: " + payload + b"\n\ndata: [DONE]\n\n"


class _Capture(ThreadingHTTPServer):
    def __init__(self, upstream, headers, request_id, model="agentkv-qwen"):
        super().__init__(("127.0.0.1", 0), _Forward)
        self.upstream = openai_base(upstream)
        self.forward_headers = headers
        self.model = model
        self.capture = {"request_id": request_id, "usages": [], "ids": []}


class _Forward(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        path = self.path.split("?", 1)[0].rstrip("/")
        if path.endswith("/models") or "/models/" in path:
            model = {"id": self.server.model, "object": "model"}
            payload = json.dumps(
                model if "/models/" in path else {"object": "list", "data": [model]}
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        raw = json.loads(self.rfile.read(length) or b"{}")
        wanted_stream = bool(raw.get("stream"))
        body = prepare_completion_body(raw, self.server.capture["request_id"])
        path = self.path.split("?", 1)[0]
        target = "/chat/completions" if path.rstrip("/").endswith("/chat/completions") else path[2:] if path.startswith("/v1") else path
        request = urllib.request.Request(
            self.server.upstream + target,
            data=json.dumps(body).encode(),
            headers={**self.server.forward_headers, "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=180) as response:
                payload = response.read()
                status = response.status
        except urllib.error.HTTPError as exc:
            payload = exc.read()
            status = exc.code
        try:
            parsed = json.loads(payload)
            if parsed.get("usage"):
                self.server.capture["usages"].append(parsed["usage"])
            if parsed.get("id"):
                self.server.capture["ids"].append(parsed["id"])
        except json.JSONDecodeError:
            parsed = {}
        outgoing = sse_wrap(payload) if wanted_stream else payload
        self.send_response(status)
        self.send_header("Content-Type", "text/event-stream" if wanted_stream else "application/json")
        self.send_header("Content-Length", str(len(outgoing)))
        self.end_headers()
        self.wfile.write(outgoing)


def start_capture_proxy(upstream, headers, request_id, model="agentkv-qwen"):
    server = _Capture(upstream, headers, request_id, model)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address
    return server, f"http://{host}:{port}/v1"
