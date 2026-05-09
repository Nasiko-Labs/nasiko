#!/usr/bin/env python3
"""
Register Aegis with a running Nasiko instance.

Usage:
    python scripts/register_nasiko.py                           # defaults
    python scripts/register_nasiko.py --url http://nasiko:9100  # custom
    NASIKO_API_URL=http://nasiko:9100 python scripts/register_nasiko.py
"""

import argparse
import json
import os
import sys

try:
    import httpx
except ImportError:
    print("httpx is required: pip install httpx", file=sys.stderr)
    sys.exit(1)


def register(nasiko_url: str, agent_url: str, agentcard_path: str = "AgentCard.json"):
    """Upload the Aegis AgentCard to the Nasiko registry."""
    with open(agentcard_path) as f:
        card = json.load(f)

    card["url"] = agent_url

    print(f"Registering Aegis with Nasiko at {nasiko_url}...")
    print(f"  Agent name:  {card['name']}")
    print(f"  Agent URL:   {card['url']}")
    print(f"  Capabilities: {', '.join(card['capabilities'])}")

    try:
        resp = httpx.post(
            f"{nasiko_url}/api/v1/agents/register",
            json=card,
            headers={"Content-Type": "application/json"},
            timeout=10.0,
        )
        if resp.status_code in (200, 201):
            print(f"✓ Registered successfully!")
            print(f"  Response: {resp.json()}")
        else:
            print(f"✗ Registration failed: {resp.status_code} {resp.text}", file=sys.stderr)
            sys.exit(1)
    except httpx.ConnectError:
        print(f"✗ Cannot connect to Nasiko at {nasiko_url}", file=sys.stderr)
        print("  Make sure Nasiko is running: docker compose up -d", file=sys.stderr)
        sys.exit(1)


def main():
    parser = argparse.ArgumentParser(description="Register Aegis with Nasiko")
    parser.add_argument(
        "--url",
        default=os.environ.get("NASIKO_API_URL", "http://localhost:9100"),
        help="Nasiko API URL (default: $NASIKO_API_URL or http://localhost:9100)",
    )
    parser.add_argument(
        "--agent-url",
        default=os.environ.get("AEGIS_URL", "http://localhost:8000"),
        help="Aegis agent URL (default: $AEGIS_URL or http://localhost:8000)",
    )
    parser.add_argument(
        "--agentcard",
        default="AgentCard.json",
        help="Path to AgentCard.json (default: AgentCard.json)",
    )
    args = parser.parse_args()
    register(args.url, args.agent_url, args.agentcard)


if __name__ == "__main__":
    main()
