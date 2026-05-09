#!/usr/bin/env python3
from __future__ import annotations

import json
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        self._json({"name": "agent-demo-request-layer", "status": "ok"})

    def do_POST(self) -> None:
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length)
        try:
            payload = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            payload = {}

        text = ""
        for part in payload.get("params", {}).get("message", {}).get("parts", []):
            if isinstance(part, dict) and part.get("kind") == "text":
                text += part.get("text", "")

        time.sleep(1.2)
        self._json(
            {
                "jsonrpc": "2.0",
                "id": payload.get("id"),
                "result": {
                    "status": "completed",
                    "message": {
                        "role": "agent",
                        "parts": [{"kind": "text", "text": f"demo response for: {text}"}],
                    },
                },
            }
        )

    def log_message(self, format: str, *args) -> None:
        return

    def _json(self, body: dict) -> None:
        encoded = json.dumps(body).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 5000), Handler).serve_forever()
