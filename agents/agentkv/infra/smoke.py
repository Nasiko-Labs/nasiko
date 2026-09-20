"""Short CPU-only verification of Modal and the server-side Jev credential.

Run: modal run infra/smoke.py
No endpoint is deployed and no key or provider response body is printed.
"""
import modal

app = modal.App("agentkv-smoke")
image = modal.Image.debian_slim(python_version="3.11")


@app.function(
    image=image,
    secrets=[modal.Secret.from_name("agentkv-jev", required_keys=["TYPESAFE_API_KEY"])],
    cpu=0.25,
    memory=256,
    timeout=60,
    retries=0,
    max_containers=1,
)
def check_jev() -> dict:
    import json
    import os
    import urllib.error
    import urllib.request

    request = urllib.request.Request(
        "https://api.typesafe.ai/v1/models",
        headers={"Authorization": "Bearer " + os.environ["TYPESAFE_API_KEY"]},
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            payload = json.load(response)
            models = payload.get("models", [])
            if not isinstance(models, list) or not models:
                return {"ok": False, "reason": "unexpected_models_response"}
            return {"ok": True, "credential_source": "Modal Secret", "model_count": len(models)}
    except urllib.error.HTTPError as error:
        return {"ok": False, "http_status": error.code}
    except (urllib.error.URLError, TimeoutError, ValueError):
        return {"ok": False, "reason": "provider_connection_or_response_error"}


@app.local_entrypoint()
def main():
    result = check_jev.remote()
    print(result)
    if not result["ok"]:
        raise SystemExit(1)
