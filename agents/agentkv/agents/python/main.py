"""Original minimal A2A agents, deployed and called through Nasiko."""
import ast
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid
import telemetry
import pi_coder
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROLE = os.environ.get("AGENTKV_ROLE", "coder")


def handle_task(payload):
    if ROLE == "tester":
        code = payload["code"]
        try:
            tree = ast.parse(code)
        except SyntaxError as exc:
            return {"passed": False, "error": "SyntaxError: " + str(exc)}
        forbidden = {"open", "exec", "eval", "compile", "getattr", "setattr", "globals", "locals", "vars", "input", "breakpoint"}
        for node in ast.walk(tree):
            if isinstance(node, (ast.Import, ast.ImportFrom)) or (isinstance(node, ast.Name) and (node.id in forbidden or node.id.startswith("__"))) or (isinstance(node, ast.Attribute) and node.attr.startswith("_")):
                return {"passed": False, "error": "Patch exceeds pure-function task contract"}
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory, "check.py")
            source.write_text(code + "\n" + payload["tests"])
            start = time.perf_counter()
            try:
                result = subprocess.run([sys.executable, "-I", "-S", str(source)],
                    cwd=directory, env={}, capture_output=True, text=True, timeout=3)
                return {"passed": result.returncode == 0, "seconds": time.perf_counter()-start,
                        "output": (result.stdout + result.stderr)[-2000:]}
            except subprocess.TimeoutExpired:
                return {"passed": False, "error": "test_timeout"}
    if ROLE == "coder" and os.environ.get("AGENTKV_HARNESS") == "pi" and int(payload.get("max_tokens", 512)) > 1:
        return run_pi_coder(payload)
    base = os.environ["OPENAI_BASE_URL"].rstrip("/")
    if not base.endswith("/v1"):
        base += "/v1"
    body = {"model": os.environ.get("AGENTKV_MODEL", "agentkv-qwen"),
            "messages": payload["messages"], "temperature": 0, "max_tokens": max(1,min(int(payload.get("max_tokens",512)),512)),
            "chat_template_kwargs": {"enable_thinking": False},
            "request_id": payload["request_id"]}
    request = urllib.request.Request(base + "/chat/completions", data=json.dumps(body).encode(),
        headers={**telemetry.outgoing(), "Content-Type": "application/json", "Authorization": "Bearer " + os.environ["OPENAI_API_KEY"]})
    start = time.perf_counter()
    with telemetry.span('chat agentkv-qwen', **{'gen_ai.operation.name':'chat','gen_ai.request.model':'agentkv-qwen'}) as current:
        with urllib.request.urlopen(request, timeout=180) as response:
            answer = json.load(response)
        if current:
            usage=answer.get('usage') or {}
            current.set_attribute('gen_ai.usage.input_tokens',usage.get('prompt_tokens',0))
            current.set_attribute('gen_ai.usage.output_tokens',usage.get('completion_tokens',0))
            if 'cached_tokens' in (usage.get('prompt_tokens_details') or {}):
                current.set_attribute('agentkv.cached_tokens',usage['prompt_tokens_details']['cached_tokens'])
    content = answer["choices"][0]["message"]["content"] or ""
    code = re.search(r"```(?:python)?\s*\n(.*?)```", content, re.S)
    return {"content": content, "code": code.group(1).strip() if code else content.strip(),
            "usage": answer.get("usage"), "inference_seconds": time.perf_counter()-start,
            "request_id": answer.get("id"), "role": ROLE}


def run_pi_coder(payload):
    model = os.environ.get("AGENTKV_MODEL", "agentkv-qwen")
    messages = payload.get("messages") or []
    system = next((item["content"] for item in messages if item.get("role") == "system"), "")
    user = next((item["content"] for item in messages if item.get("role") == "user"), payload.get("goal", ""))
    request_id = payload.get("request_id") or ("agentkv-" + uuid.uuid4().hex)
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory, "work")
        home = Path(directory, "pi-home")
        pi_coder.write_workspace(root, payload.get("source") or "", system, user)
        proxy, local_url = pi_coder.start_capture_proxy(
            os.environ["OPENAI_BASE_URL"],
            {**telemetry.outgoing(), "Authorization": "Bearer " + os.environ["OPENAI_API_KEY"]},
            request_id, model)
        try:
            pi_coder.write_runtime(home, pi_coder.models_file(local_url, os.environ["OPENAI_API_KEY"], model))
            env = pi_coder.pi_env({**os.environ, "HOME": str(home)})
            start = time.perf_counter()
            result = subprocess.run(pi_coder.pi_command(model, user, os.environ["OPENAI_API_KEY"]),
                                    cwd=root, env=env, capture_output=True, text=True, timeout=300)
            code = pi_coder.extract_code(root)
            usage = pi_coder.merge_usage(proxy.capture["usages"])
            return {"content": (result.stdout or "")[-4000:], "code": code,
                    "usage": usage, "inference_seconds": time.perf_counter() - start,
                    "request_id": (proxy.capture["ids"] or [request_id])[-1], "role": ROLE,
                    "harness": "pi", "returncode": result.returncode,
                    "stderr": (result.stderr or "")[-1000:]}
        finally:
            proxy.shutdown()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def send(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        if self.path in {"/.well-known/agent-card.json", "/.well-known/agent.json"}:
            return self.send(200, json.loads(Path("AgentCard.json").read_text()))
        self.send(200, {"status": "ok", "role": ROLE})
    def do_POST(self):
        rpc = {}
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 200000:
                return self.send(413, {"error": "request too large"})
            rpc = json.loads(self.rfile.read(length))
            message = rpc["params"]["message"]
            payload = next(part["data"] for part in message["parts"] if part.get("kind") == "data")
            with telemetry.span('invoke_agent '+ROLE, self.headers,
                    **{'gen_ai.operation.name':'invoke_agent','gen_ai.agent.name':ROLE}) as current:
                result = handle_task(payload)
                if current and ROLE=='tester': current.set_attribute('agentkv.test_passed',result['passed'])
            self.send(200, {"jsonrpc": "2.0", "id": rpc.get("id"), "result": {
                "kind": "message", "role": "agent", "messageId": str(uuid.uuid4()),
                "contextId": message.get("contextId"), "parts": [{"kind": "data", "data": result}]}})
        except Exception as exc:
            self.send(200, {"jsonrpc": "2.0", "id": rpc.get("id"),
                "error": {"code": -32603, "message": type(exc).__name__ + (" HTTP "+str(exc.code) if isinstance(exc,urllib.error.HTTPError) else "")}})


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8000), Handler).serve_forever()
