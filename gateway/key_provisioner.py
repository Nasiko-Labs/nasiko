"""
LiteLLM Virtual Key Provisioner for Nasiko

Generates a virtual key for agents via the LiteLLM /key/generate API
and stores it in Redis for the orchestrator to pick up.

Usage:
    python gateway/key_provisioner.py [--gateway-url URL] [--master-key KEY]
"""

import os
import sys
import json
import time
import argparse
import urllib.request
import urllib.error


def wait_for_gateway(gateway_url: str, timeout: int = 120) -> bool:
    """Wait for the LiteLLM gateway to become healthy."""
    health_url = f"{gateway_url}/health/liveliness"
    start = time.time()
    while time.time() - start < timeout:
        try:
            req = urllib.request.Request(health_url)
            with urllib.request.urlopen(req, timeout=5) as resp:
                if resp.status == 200:
                    print(f"✅ Gateway is healthy at {gateway_url}")
                    return True
        except (urllib.error.URLError, ConnectionError, OSError):
            pass
        print(f"⏳ Waiting for gateway at {gateway_url}...")
        time.sleep(3)
    print(f"❌ Gateway did not become healthy within {timeout}s")
    return False


def generate_virtual_key(gateway_url: str, master_key: str) -> str | None:
    """Generate a virtual key using LiteLLM's /key/generate API."""
    url = f"{gateway_url}/key/generate"
    payload = json.dumps({
        "key_alias": "nasiko-agents-key",
        "max_budget": 100,
        "models": [],  # Allow all models
        "duration": None,  # No expiry
        "metadata": {
            "purpose": "Shared key for Nasiko platform agents",
            "created_by": "key_provisioner",
        },
    }).encode("utf-8")

    req = urllib.request.Request(
        url,
        data=payload,
        headers={
            "Authorization": f"Bearer {master_key}",
            "Content-Type": "application/json",
        },
        method="POST",
    )

    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            key = data.get("key")
            if key:
                print(f"✅ Virtual key generated: {key[:12]}...")
                return key
            else:
                print(f"⚠️  Unexpected response: {data}")
                return None
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        # If key alias already exists, try to retrieve it
        if e.code == 400 and "already exists" in body.lower():
            print("ℹ️  Key alias 'nasiko-agents-key' already exists, reusing.")
            return retrieve_existing_key(gateway_url, master_key)
        print(f"❌ HTTP error generating key: {e.code} — {body}")
        return None
    except Exception as e:
        print(f"❌ Error generating key: {e}")
        return None


def retrieve_existing_key(gateway_url: str, master_key: str) -> str | None:
    """Retrieve info about existing keys."""
    url = f"{gateway_url}/key/info"
    req = urllib.request.Request(
        url,
        data=json.dumps({"key_alias": "nasiko-agents-key"}).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {master_key}",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            token = data.get("info", {}).get("token") or data.get("key")
            if token:
                print(f"✅ Retrieved existing key: {token[:12]}...")
                return token
    except Exception as e:
        print(f"⚠️  Could not retrieve existing key: {e}")
    return None


def store_in_redis(key: str, redis_host: str = "localhost", redis_port: int = 6379) -> bool:
    """Store the virtual key in Redis for the orchestrator to read."""
    try:
        import redis as redis_lib
        r = redis_lib.Redis(host=redis_host, port=redis_port, decode_responses=True)
        r.set("litellm:agent_key", key, ex=31536000)  # 1 year TTL
        print(f"✅ Key stored in Redis (litellm:agent_key)")
        return True
    except ImportError:
        print("⚠️  redis package not installed, skipping Redis storage")
        return False
    except Exception as e:
        print(f"❌ Failed to store key in Redis: {e}")
        return False


def store_to_file(key: str, filepath: str = "gateway/agent_virtual_key.json") -> bool:
    """Store the virtual key to a local file as backup."""
    try:
        data = {
            "key": key,
            "key_alias": "nasiko-agents-key",
            "note": "Auto-generated virtual key for Nasiko agents. Set LITELLM_API_KEY to this value.",
        }
        with open(filepath, "w") as f:
            json.dump(data, f, indent=2)
        print(f"✅ Key saved to {filepath}")
        return True
    except Exception as e:
        print(f"❌ Failed to save key to file: {e}")
        return False


def main():
    parser = argparse.ArgumentParser(description="LiteLLM Virtual Key Provisioner")
    parser.add_argument(
        "--gateway-url",
        default=os.getenv("LITELLM_GATEWAY_URL", "http://localhost:4000"),
        help="LiteLLM gateway URL (default: http://localhost:4000)",
    )
    parser.add_argument(
        "--master-key",
        default=os.getenv("LITELLM_MASTER_KEY", "sk-nasiko-master-key"),
        help="LiteLLM master key",
    )
    parser.add_argument(
        "--redis-host",
        default=os.getenv("REDIS_HOST", "localhost"),
        help="Redis host (default: localhost)",
    )
    parser.add_argument(
        "--redis-port",
        type=int,
        default=int(os.getenv("REDIS_PORT", "6379")),
        help="Redis port (default: 6379)",
    )
    parser.add_argument(
        "--wait",
        action="store_true",
        default=True,
        help="Wait for gateway to be healthy before generating key",
    )
    args = parser.parse_args()

    print("=" * 60)
    print("  Nasiko LLM Gateway — Virtual Key Provisioner")
    print("=" * 60)

    if args.wait:
        if not wait_for_gateway(args.gateway_url):
            sys.exit(1)

    key = generate_virtual_key(args.gateway_url, args.master_key)
    if not key:
        print("❌ Failed to generate virtual key. Using master key as fallback.")
        key = args.master_key

    # Store in Redis
    store_in_redis(key, args.redis_host, args.redis_port)

    # Store to file as backup
    store_to_file(key)

    print()
    print("=" * 60)
    print(f"  LITELLM_API_KEY={key[:20]}...")
    print("  Set this in .nasiko-local.env or it will be read from Redis")
    print("=" * 60)


if __name__ == "__main__":
    main()
