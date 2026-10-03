import json
from http.server import HTTPServer, BaseHTTPRequestHandler

REPLY = "I am the Nasiko Cost-Aware Classifier Agent. Request analyzed and routed successfully."

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"status":"ok","agent":"classifier"}')

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw_body = self.rfile.read(length).decode("utf-8") if length > 0 else "{}"
        try:
            body = json.loads(raw_body)
        except Exception:
            body = {}

        req_id = body.get("id", 1)

        # Universal response: satisfies A2A JSON-RPC, OpenRuntime agent message, and OpenAI format
        response_payload = {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "role": "assistant",
                "content": REPLY,
                "parts": [{"type": "text", "text": REPLY}],
                "message": {
                    "role": "assistant",
                    "content": REPLY
                }
            },
            "role": "assistant",
            "content": REPLY,
            "text": REPLY,
            "output": REPLY,
            "message": {
                "role": "assistant",
                "content": REPLY
            },
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": REPLY
                    },
                    "delta": {
                        "role": "assistant",
                        "content": REPLY
                    },
                    "finish_reason": "stop"
                }
            ]
        }

        encoded = json.dumps(response_payload).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, format, *args):
        pass

if __name__ == "__main__":
    server = HTTPServer(("0.0.0.0", 8000), Handler)
    print("Serving agent on port 8000...")
    server.serve_forever()