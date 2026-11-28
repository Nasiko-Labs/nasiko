#!/usr/bin/env python3
"""Test Gemini generation and Nasiko's provider probe without printing secrets.

Run from any directory: python3 scripts/check-gemini.py
Makes two small model requests; does not save a provider or change configuration.
"""

import argparse
import json
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request


def read_env(path):
    values = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        name, value = line.removeprefix("export ").split("=", 1)
        values[name.strip()] = value.strip().strip("\"'")
    return values


def post(url, body, headers=None):
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", **(headers or {})},
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read().decode(errors="replace")
        raise RuntimeError(f"HTTP {error.code}: {detail}") from None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", type=Path, default=Path(__file__).resolve().parents[1] / ".env")
    parser.add_argument("--server-url", default="http://localhost:8080")
    parser.add_argument("--model", help="Override the model in .env for this test only")
    args = parser.parse_args()
    secrets = []
    failures = 0
    try:
        cfg = read_env(args.env)
        secrets = [cfg.get(name, "") for name in
                   ("GEMINI_API_KEY", "ADMIN_PASSWORD", "JWT_SECRET", "AGENT_JWT_SECRET")]
        key = cfg.get("GEMINI_API_KEY", "")
        model = args.model or cfg.get("DEFAULT_MODEL") or cfg.get("OPENAI_MODEL")
        if not key or not model:
            raise RuntimeError("Set GEMINI_API_KEY and DEFAULT_MODEL in .env first.")
        print(f"Testing model: {model}", flush=True)

        def report_failure(label, error):
            message = str(error)
            for secret in secrets:
                if secret:
                    message = message.replace(secret, "[redacted]")
            print(f"FAIL {label}: {message[:1400]}", flush=True)

        try:
            base = cfg.get("GOOGLE_GEMINI_BASE_URL") or "https://generativelanguage.googleapis.com/v1beta"
            result = post(
                f"{base.rstrip('/')}/models/{urllib.parse.quote(model, safe='')}:generateContent",
                {
                    "contents": [{"parts": [{"text": "Reply with exactly API_OK."}]}],
                    "generationConfig": {"maxOutputTokens": 256},
                },
                {"x-goog-api-key": key},
            )
            candidates = result.get("candidates", [])
            answer = " ".join(
                part.get("text", "")
                for candidate in candidates
                for part in candidate.get("content", {}).get("parts", [])
                if not part.get("thought")
            ).strip()
            if not answer:
                reasons = [candidate.get("finishReason", "unknown") for candidate in candidates]
                raise RuntimeError(f"No text returned; finish reasons: {reasons}. Try a larger token budget or another available model.")
            print(f"PASS Google native Gemini: {answer[:160]}", flush=True)
        except (RuntimeError, OSError, ValueError) as error:
            report_failure("Google native Gemini", error)
            failures += 1

        try:
            server = args.server_url.rstrip("/")
            login = post(server + "/api/auth/login", {
                "username": cfg["ADMIN_USERNAME"], "password": cfg["ADMIN_PASSWORD"],
            })
            token = login["token"]
            secrets.append(token)
            result = post(server + "/api/custom-providers/test", {
                "base_url": "https://generativelanguage.googleapis.com/v1beta/openai/",
                "kind": "openai", "api_key": key, "model": model,
            }, {"Authorization": "Bearer " + token})
            data = result.get("data", result)
            if not data.get("chat_ok"):
                raise RuntimeError(data.get("chat_error") or result.get("message") or "Provider probe failed")
            print("PASS Nasiko provider test: chat_ok=true", flush=True)
        except (RuntimeError, OSError, ValueError, KeyError) as error:
            report_failure("Nasiko provider test", error)
            failures += 1
        return 1 if failures else 0
    except (RuntimeError, OSError, ValueError) as error:
        message = str(error)
        for secret in secrets:
            if secret:
                message = message.replace(secret, "[redacted]")
        print("FAIL: " + message[:1400], flush=True)
        return 1


if __name__ == "__main__":
    sys.exit(main())
