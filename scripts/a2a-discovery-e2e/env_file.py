"""Helpers for the MCP snippet checks: load an env file the way the agent's framework does at
startup, and read a tool result's text whatever shape the client library returns it in."""

import json
import os


def load(path: str) -> None:
    """Load `path`'s active `KEY=VALUE` lines into `os.environ`."""
    with open(path) as lines:
        for line in lines:
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                key, value = line.split("=", 1)
                os.environ[key.removeprefix("export ").strip()] = value.strip()


def text(result) -> str:
    """A tool result as text: a string, MCP content blocks, or a (content, artifact) pair."""
    if isinstance(result, tuple):
        result = result[0]
    if isinstance(result, str):
        try:
            blocks = json.loads(result)
        except ValueError:
            return result
        if isinstance(blocks, list):
            result = blocks
        else:
            return result
    if isinstance(result, list):
        return "".join(
            block.get("text", "") if isinstance(block, dict) else getattr(block, "text", str(block))
            for block in result
        )
    return str(result)
