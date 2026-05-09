"""
AgentCard parser for Nasiko agents.

Reads a Nasiko ``AgentCard.json`` and extracts structured metadata that
Aegis uses to auto-populate firewall policies (blocked_tools,
approval_required) based on the agent's declared capabilities.

High-risk skills are identified by matching skill names or tags against
known dangerous patterns (delete, write, exec, email, secret, etc.).
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

# Patterns that mark a skill as high-risk
_HIGH_RISK_PATTERNS = re.compile(
    r"(delete|remove|drop|write|exec|run|email|send|secret|credential|extract|exfil)",
    re.IGNORECASE,
)

# Tags that elevate risk
_HIGH_RISK_TAGS = {"exec", "write", "delete", "email", "secret", "admin", "dangerous"}


def parse_agentcard(path: str | Path) -> dict[str, Any]:
    """
    Parse a Nasiko AgentCard.json and return structured metadata.

    Returns::

        {
            "name": "github-agent",
            "description": "...",
            "url": "http://...",
            "capabilities": ["search_code", "delete_branch", ...],
            "tags": ["exec", "write"],
            "skills": ["search_code", "delete_branch"],
            "high_risk_skills": ["delete_branch"],
            "endpoints": {"/analyze": "...", ...},
        }
    """
    path = Path(path)
    if not path.exists():
        raise FileNotFoundError(f"AgentCard not found: {path}")

    with open(path) as f:
        card = json.load(f)

    name = card.get("name", "unknown")
    description = card.get("description", "")
    capabilities = card.get("capabilities", [])
    tags = set(card.get("tags", []))
    endpoints = card.get("endpoints", {})
    examples = card.get("examples", [])

    # Extract skills from capabilities + endpoint names
    skills = list(capabilities)
    for ep in endpoints:
        skill_name = ep.strip("/").replace("/", "_")
        if skill_name and skill_name not in skills:
            skills.append(skill_name)

    # Identify high-risk skills
    high_risk = []
    for skill in skills:
        if _HIGH_RISK_PATTERNS.search(skill):
            high_risk.append(skill)

    # Also flag skills if agent tags include high-risk categories
    if tags & _HIGH_RISK_TAGS:
        for skill in skills:
            if skill not in high_risk:
                high_risk.append(skill)

    return {
        "name": name,
        "description": description,
        "url": card.get("url", ""),
        "capabilities": capabilities,
        "tags": list(tags),
        "skills": skills,
        "high_risk_skills": high_risk,
        "endpoints": endpoints,
        "examples": examples,
    }


def agentcard_to_policy_overrides(card_meta: dict[str, Any]) -> dict[str, list[str]]:
    """
    Convert parsed AgentCard metadata into policy overrides that can
    be merged into Aegis's PolicyEngine at runtime.

    Returns::

        {
            "approval_required": ["delete_branch", "exec_command"],
            "blocked_tools": [],  # nothing auto-blocked, but could be
        }
    """
    return {
        "approval_required": list(card_meta.get("high_risk_skills", [])),
        "blocked_tools": [],
    }
