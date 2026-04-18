"""
LLM Gateway management commands for the Nasiko CLI.

Wraps the LiteLLM proxy's REST API so operators can list, rotate, and revoke
per-agent virtual keys without opening the admin UI.
"""

import os
import sys

import requests
from rich.console import Console
from rich.table import Table

sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

console = Console()


def _gateway_url() -> str:
    return os.environ.get("LLM_GATEWAY_URL", "http://localhost:4001").rstrip("/")


def _master_key() -> str:
    key = os.environ.get("LLM_GATEWAY_MASTER_KEY")
    if not key:
        console.print(
            "[red]LLM_GATEWAY_MASTER_KEY is not set.[/] "
            "Export it or add it to your .env file."
        )
        raise SystemExit(1)
    return key


def _headers() -> dict:
    return {
        "Authorization": f"Bearer {_master_key()}",
        "Content-Type": "application/json",
    }


# ─────────────────────────────────────────────────────────────────────────────
# Commands
# ─────────────────────────────────────────────────────────────────────────────

def list_keys_command(format_type: str = "table") -> None:
    """List all virtual keys currently provisioned on the gateway."""
    try:
        resp = requests.get(
            f"{_gateway_url()}/key/list",
            headers=_headers(),
            timeout=15,
        )
        resp.raise_for_status()
        body = resp.json()
        keys = body.get("keys", body if isinstance(body, list) else [])

        if not keys:
            console.print("[yellow]No virtual keys found.[/]")
            return

        if format_type == "json":
            console.print_json(data=keys)
            return

        table = Table(title=f"Virtual Keys ({len(keys)})")
        table.add_column("Alias", style="cyan")
        table.add_column("Key (truncated)", style="green")
        table.add_column("Spend", style="yellow")
        table.add_column("Max Budget", style="magenta")
        table.add_column("Created", style="dim")

        for k in keys:
            if isinstance(k, str):
                table.add_row(
                    "—", f"{k[:10]}…{k[-4:]}" if len(k) > 14 else k, "—", "—", "—"
                )
                continue
            alias = k.get("key_alias") or "—"
            token = k.get("token") or k.get("key") or ""
            masked = f"{token[:10]}…{token[-4:]}" if len(token) > 14 else token
            table.add_row(
                alias,
                masked,
                str(k.get("spend", 0)),
                str(k.get("max_budget", "unlimited")),
                str(k.get("created_at", "—")),
            )

        console.print(table)

    except requests.exceptions.ConnectionError:
        console.print(
            f"[red]Cannot reach gateway at {_gateway_url()}.[/] "
            "Is `llm-gateway` running?"
        )
    except requests.exceptions.HTTPError as e:
        console.print(f"[red]HTTP {e.response.status_code}: {e.response.text}[/]")
    except Exception as e:
        console.print(f"[red]Error: {e}[/]")


def rotate_key_command(agent_name: str, max_budget: float = None) -> None:
    """Delete the current virtual key for an agent and mint a fresh one."""
    alias = f"nasiko-agent-{agent_name}"

    # Step 1 — look up existing key by alias and delete
    try:
        info = requests.get(
            f"{_gateway_url()}/key/info",
            params={"key_alias": alias},
            headers=_headers(),
            timeout=15,
        )
        if info.status_code == 200:
            token = info.json().get("info", {}).get("token") or info.json().get("token")
            if token:
                requests.post(
                    f"{_gateway_url()}/key/delete",
                    json={"keys": [token]},
                    headers=_headers(),
                    timeout=10,
                )
                console.print(f"[dim]Revoked old key for agent '{agent_name}'[/]")
    except Exception:
        # Not fatal — the alias may not exist yet
        pass

    # Step 2 — mint a fresh key
    payload = {
        "key_alias": alias,
        "metadata": {"agent_name": agent_name, "provisioned_by": "nasiko-cli"},
    }
    if max_budget is not None:
        payload["max_budget"] = max_budget

    try:
        resp = requests.post(
            f"{_gateway_url()}/key/generate",
            json=payload,
            headers=_headers(),
            timeout=15,
        )
        resp.raise_for_status()
        new_key = resp.json().get("key")
        console.print(f"[green]✓[/] Rotated virtual key for agent '{agent_name}'")
        console.print(f"  [bold]New key:[/] {new_key}")
        if max_budget is not None:
            console.print(f"  [bold]Budget cap:[/] ${max_budget}")
        console.print(
            "[yellow]Note:[/] agent must be redeployed to pick up the new key via env injection."
        )

    except requests.exceptions.HTTPError as e:
        console.print(f"[red]HTTP {e.response.status_code}: {e.response.text}[/]")
    except Exception as e:
        console.print(f"[red]Error: {e}[/]")


def revoke_key_command(agent_name: str) -> None:
    """Delete the virtual key for an agent (agent calls will 401 until redeployed)."""
    alias = f"nasiko-agent-{agent_name}"

    try:
        info = requests.get(
            f"{_gateway_url()}/key/info",
            params={"key_alias": alias},
            headers=_headers(),
            timeout=15,
        )
        if info.status_code != 200:
            console.print(f"[yellow]No virtual key found for agent '{agent_name}'.[/]")
            return

        token = info.json().get("info", {}).get("token") or info.json().get("token")
        if not token:
            console.print(f"[yellow]No token found for alias '{alias}'.[/]")
            return

        resp = requests.post(
            f"{_gateway_url()}/key/delete",
            json={"keys": [token]},
            headers=_headers(),
            timeout=10,
        )
        resp.raise_for_status()
        console.print(f"[green]✓[/] Revoked virtual key for agent '{agent_name}'")

    except requests.exceptions.HTTPError as e:
        console.print(f"[red]HTTP {e.response.status_code}: {e.response.text}[/]")
    except Exception as e:
        console.print(f"[red]Error: {e}[/]")


def gateway_health_command() -> None:
    """Show gateway readiness + configured models."""
    try:
        ready = requests.get(f"{_gateway_url()}/health/readiness", timeout=10).json()
        console.print(f"[green]✓[/] Gateway healthy at {_gateway_url()}")
        console.print(f"  DB: [bold]{ready.get('db', 'unknown')}[/]")
        console.print(f"  LiteLLM version: [bold]{ready.get('litellm_version', '?')}[/]")
        console.print(f"  Success callbacks: {ready.get('success_callbacks', [])}")

        models = requests.get(
            f"{_gateway_url()}/v1/models", headers=_headers(), timeout=10
        ).json()
        console.print("\n[bold]Configured models:[/]")
        for m in models.get("data", []):
            console.print(f"  • {m.get('id')}")

    except requests.exceptions.ConnectionError:
        console.print(f"[red]Cannot reach gateway at {_gateway_url()}[/]")
    except Exception as e:
        console.print(f"[red]Error: {e}[/]")
