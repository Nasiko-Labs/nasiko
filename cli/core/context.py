"""
Context storage for Nasiko CLI.

Persists the active cluster name to ~/.nasiko/context.json so commands
operate on the right cluster without requiring --cluster/-n every time.
"""

import json
from pathlib import Path
from typing import Optional

from setup.config import get_nasiko_home


def _context_path() -> Path:
    """Returns ~/.nasiko/context.json, creating the parent dir if needed."""
    return get_nasiko_home() / "context.json"


def read_context() -> dict:
    """
    Load context.json. Returns an empty dict if the file is missing or malformed.
    Never raises.
    """
    path = _context_path()
    if not path.exists():
        return {}
    try:
        with path.open("r") as f:
            return json.load(f)
    except Exception:
        return {}


def write_context(data: dict) -> None:
    """
    Overwrite context.json with the supplied dict.
    File is created with 0o600 permissions (owner read/write only).
    """
    path = _context_path()
    with path.open("w") as f:
        json.dump(data, f, indent=2)
    path.chmod(0o600)


def get_active_cluster() -> Optional[str]:
    """Return the active cluster name from context.json, or None."""
    return read_context().get("active_cluster")


def set_active_cluster(name: str) -> None:
    """Set (or update) the active_cluster key in context.json."""
    ctx = read_context()
    ctx["active_cluster"] = name
    write_context(ctx)
