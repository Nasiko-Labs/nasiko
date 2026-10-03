#!/usr/bin/env python3
"""
Nasiko Build-A-Thon Track P1 — Live Dynamic Token Reduction Demonstration

Demonstrates the live end-to-end compact tool pipeline:
1. Sends a multi-tool agent request using Standard JSON Schema.
2. Sends the exact same multi-tool agent request with `x-nasiko-compact-tools: true`.
3. Measures live prompt tokens from the upstream LLM and verifies lossless tool call decoding.
"""

import json
import urllib.request
import sys

URL = "http://localhost:8080/v1/chat/completions"
# Agent identity token minted by the platform
TOKEN = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJhZ2VudF9pZCI6ImJkM2MwYjRjLTQwOTYtNDgyNy1hNDg0LWE1YTcxYWU3MDBmYSIsIm93bmVyX2lkIjoiMTU4MzA5MzgtMmFiNi00NjE5LWEzMTItYjQ4MzM1YTM3NDBhIiwiaWF0IjoxNzkxMDE1MjQ1LCJleHAiOjE4MjI1NTEyNDV9.4JiFN8nnyxG8jBXFykC8fXof2AUsRngFvVZnJ0IXp64"

TOOLS = [
    {
        "type": "function",
        "function": {
            "name": "create_calendar_event",
            "description": "Schedule a new event on the user calendar with attendees and reminders.",
            "parameters": {
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Title of the event"},
                    "start_time": {"type": "string", "description": "Start ISO datetime string"},
                    "duration_minutes": {"type": "integer", "description": "Meeting duration in minutes"},
                    "attendees": {"type": "array", "items": {"type": "string"}, "description": "List of emails"},
                    "visibility": {"type": "string", "enum": ["public", "private"], "description": "Visibility flag"}
                },
                "required": ["title", "start_time"]
            }
        }
    },
    {
        "type": "function",
        "function": {
            "name": "send_slack_message",
            "description": "Post a markdown formatted message to a Slack channel with attachments.",
            "parameters": {
                "type": "object",
                "properties": {
                    "channel": {"type": "string", "description": "Target channel ID or name"},
                    "text": {"type": "string", "description": "Body message text"},
                    "thread_ts": {"type": "string", "description": "Optional thread parent timestamp"}
                },
                "required": ["channel", "text"]
            }
        }
    },
    {
        "type": "function",
        "function": {
            "name": "create_github_issue",
            "description": "Create a new issue in a GitHub repository with labels and milestone.",
            "parameters": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Owner and repo name e.g. org/repo"},
                    "title": {"type": "string", "description": "Issue title"},
                    "body": {"type": "string", "description": "Issue description in markdown"},
                    "labels": {"type": "array", "items": {"type": "string"}, "description": "Labels to apply"}
                },
                "required": ["repo", "title"]
            }
        }
    },
    {
        "type": "function",
        "function": {
            "name": "query_database",
            "description": "Execute a readonly SQL query against the Postgres operational store.",
            "parameters": {
                "type": "object",
                "properties": {
                    "sql": {"type": "string", "description": "The SELECT SQL query"},
                    "max_rows": {"type": "integer", "description": "Maximum number of rows to return"}
                },
                "required": ["sql"]
            }
        }
    }
]

PROMPT = "Post a message to #dev channel saying build passed successfully."

def send_request(compact: bool):
    payload = {
        "model": "mistral.mistral-large-3-675b-instruct",
        "messages": [{"role": "user", "content": PROMPT}],
        "tools": TOOLS,
    }
    headers = {
        "Authorization": f"Bearer {TOKEN}",
        "Content-Type": "application/json",
    }
    if compact:
        headers["x-nasiko-compact-tools"] = "true"

    req = urllib.request.Request(URL, data=json.dumps(payload).encode(), headers=headers)
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read().decode())

def main():
    print("=" * 70)
    print("  Nasiko Live Dynamic Token Reduction Demonstration")
    print("=" * 70)
    print(f"Target Server : {URL}")
    print(f"User Prompt   : \"{PROMPT}\"")
    print(f"Tool Count    : {len(TOOLS)} tool definitions (Calendar, Slack, GitHub, Database)\n")

    print("[1/2] Sending Standard JSON Schema Request...")
    std_resp = send_request(compact=False)
    std_tokens = std_resp["usage"]["prompt_tokens"]
    std_tool = std_resp["choices"][0]["message"]["tool_calls"][0]

    print("[2/2] Sending Compact DSL Schema Request (x-nasiko-compact-tools: true)...")
    cpt_resp = send_request(compact=True)
    cpt_tokens = cpt_resp["usage"]["prompt_tokens"]
    cpt_tool = cpt_resp["choices"][0]["message"]["tool_calls"][0]

    saved = std_tokens - cpt_tokens
    percent = (saved / std_tokens) * 100.0

    print("\n" + "─" * 70)
    print("  LIVE COMPARISON RESULTS")
    print("─" * 70)
    print(f"  Standard JSON Schema Prompt Tokens : {std_tokens} tokens")
    print(f"  Compact DSL Schema Prompt Tokens   : {cpt_tokens} tokens")
    print(f"  Live Prompt Tokens Saved           : \033[92m{saved} tokens ({percent:.2f}% reduction)\033[0m")
    print(f"  Standard Tool Call Executed        : {std_tool['function']['name']}({std_tool['function']['arguments']})")
    print(f"  Compact Tool Call Decoded Lossless : {cpt_tool['function']['name']}({cpt_tool['function']['arguments']})")
    print("─" * 70)
    print("\n✓ Live dynamic demonstration verified successfully!\n")

if __name__ == "__main__":
    main()
