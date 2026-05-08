"""
litellm_setup.py — Typer sub-app for managing the Nasiko LLM gateway (LiteLLM).

Commands:
  init        Generate LITELLM_MASTER_KEY, LITELLM_SALT_KEY, LITELLM_POSTGRES_PASSWORD
              in .nasiko-local.env if absent (idempotent).
  mint        Mint a per-agent virtual key via the LiteLLM admin API.
  rotate      Create a new key, delete the old one, update MongoDB.
  revoke      Delete the key from LiteLLM and mark revoked in MongoDB.
  list-keys   Print all virtual key records from MongoDB (masked).
  info        Fetch key info from LiteLLM + MongoDB record.
"""

import asyncio
import base64
import logging
import os
import secrets
import sys
from pathlib import Path
from typing import Optional

import httpx
import typer
from rich.console import Console
from rich.table import Table

app = typer.Typer(help="Manage the Nasiko LLM gateway (LiteLLM virtual keys).")
console = Console()

logger = logging.getLogger("litellm_setup")
logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

DEFAULT_ENV_FILE = ".nasiko-local.env"


def _load_env_file(path: str) -> dict:
    """Read key=value pairs from an env file (ignores comments + blank lines)."""
    env = {}
    p = Path(path)
    if not p.exists():
        return env
    for line in p.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, _, value = line.partition("=")
            env[key.strip()] = value.strip()
    return env


def _append_env_var(path: str, key: str, value: str) -> None:
    """Append KEY=value to an env file if KEY is not already present."""
    p = Path(path)
    existing = _load_env_file(path)
    if key in existing:
        return
    with open(p, "a") as f:
        f.write(f"\n{key}={value}\n")


def _gateway_url() -> str:
    return os.getenv("LLM_GATEWAY_URL", "http://localhost:4100")


def _master_key() -> str:
    return os.getenv("LITELLM_MASTER_KEY", "")


def _auth_headers() -> dict:
    return {
        "Authorization": f"Bearer {_master_key()}",
        "Content-Type": "application/json",
    }


def _mongo_url() -> str:
    host = os.getenv("MONGO_NASIKO_HOST", "localhost")
    port = os.getenv("MONGO_NASIKO_PORT", "27017")
    user = os.getenv("MONGO_NASIKO_USER", "admin")
    password = os.getenv("MONGO_NASIKO_PASSWORD", "password")
    return f"mongodb://{user}:{password}@{host}:{port}"


def _mongo_db() -> str:
    return os.getenv("MONGO_NASIKO_DATABASE", "nasiko")


def _get_repo():
    """Lazy import of VirtualKeyRepository to avoid import errors at CLI load time."""
    # Allow running from repo root OR from within orchestrator/
    repo_root = Path(__file__).resolve().parent.parent.parent
    orchestrator_path = repo_root / "orchestrator"
    if str(orchestrator_path) not in sys.path:
        sys.path.insert(0, str(orchestrator_path))
    from virtual_keys_repository import VirtualKeyRepository  # type: ignore

    return VirtualKeyRepository


async def _repo_context():
    """Return (repo, client) for the current session."""
    VirtualKeyRepository = _get_repo()
    repo, client = VirtualKeyRepository.from_url(_mongo_url(), _mongo_db(), logger)
    return repo, client


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


@app.command("init")
def init(
    env_file: str = typer.Option(
        DEFAULT_ENV_FILE,
        "--env-file",
        "-e",
        help="Path to the .nasiko-local.env file to update.",
    ),
):
    """
    Generate gateway secrets and write them to the env file (idempotent).

    Creates LITELLM_MASTER_KEY (sk-prefixed random hex), LITELLM_SALT_KEY
    (base64 random 32 bytes), and LITELLM_POSTGRES_PASSWORD if absent.
    Safe to re-run — already-set values are preserved.
    """
    existing = _load_env_file(env_file)
    written = []

    if "LITELLM_MASTER_KEY" not in existing:
        master_key = "sk-" + secrets.token_hex(32)
        _append_env_var(env_file, "LITELLM_MASTER_KEY", master_key)
        written.append("LITELLM_MASTER_KEY")
        console.print(f"[green]Generated LITELLM_MASTER_KEY[/green]")
    else:
        console.print("[dim]LITELLM_MASTER_KEY already set — skipping[/dim]")

    if "LITELLM_SALT_KEY" not in existing:
        salt_key = base64.urlsafe_b64encode(secrets.token_bytes(32)).decode()
        _append_env_var(env_file, "LITELLM_SALT_KEY", salt_key)
        written.append("LITELLM_SALT_KEY")
        console.print(f"[green]Generated LITELLM_SALT_KEY[/green]")
    else:
        console.print("[dim]LITELLM_SALT_KEY already set — skipping[/dim]")

    if "LITELLM_POSTGRES_PASSWORD" not in existing:
        pg_password = secrets.token_hex(16)
        _append_env_var(env_file, "LITELLM_POSTGRES_PASSWORD", pg_password)
        written.append("LITELLM_POSTGRES_PASSWORD")
        console.print(f"[green]Generated LITELLM_POSTGRES_PASSWORD[/green]")
    else:
        console.print("[dim]LITELLM_POSTGRES_PASSWORD already set — skipping[/dim]")

    if written:
        console.print(
            f"\n[bold green]LLM Gateway initialized.[/bold green] "
            f"Written to [bold]{env_file}[/bold]: {', '.join(written)}\n"
            "Restart the gateway container to apply: "
            "docker compose ... restart llm-gateway"
        )
    else:
        console.print(
            "[bold green]All gateway secrets already present — nothing to write.[/bold green]"
        )


@app.command("mint")
def mint(
    agent_id: str = typer.Option(..., "--agent-id", help="Agent identifier."),
    owner_id: str = typer.Option("", "--owner-id", help="Owner/user identifier."),
    max_budget: float = typer.Option(
        5.0, "--max-budget", help="Maximum USD budget for this key."
    ),
):
    """
    Mint a virtual key for an agent and persist it to MongoDB.

    Calls POST /key/generate on the LiteLLM gateway, stores the result in the
    virtual_keys MongoDB collection, and prints the new key.
    """
    asyncio.run(_mint_async(agent_id, owner_id, max_budget))


async def _mint_async(agent_id: str, owner_id: str, max_budget: float):
    if not _master_key():
        console.print(
            "[red]LITELLM_MASTER_KEY is not set. Run 'nasiko-setup litellm init' first.[/red]"
        )
        raise typer.Exit(1)

    payload = {
        "key_alias": f"agent-{agent_id}",
        "metadata": {"agent_id": agent_id, "owner_id": owner_id or ""},
        "max_budget": max_budget,
        "models": ["default-model"],
    }
    url = f"{_gateway_url()}/key/generate"

    try:
        async with httpx.AsyncClient(timeout=15.0) as client:
            resp = await client.post(url, headers=_auth_headers(), json=payload)
            resp.raise_for_status()
    except httpx.HTTPStatusError as exc:
        console.print(
            f"[red]Gateway returned HTTP {exc.response.status_code}: {exc.response.text}[/red]"
        )
        raise typer.Exit(1)
    except Exception as exc:
        console.print(f"[red]Could not reach gateway at {_gateway_url()}: {exc}[/red]")
        raise typer.Exit(1)

    virtual_key = resp.json().get("key", "")
    if not virtual_key:
        console.print("[red]Gateway response missing 'key' field.[/red]")
        raise typer.Exit(1)

    repo, client = await _repo_context()
    try:
        await repo.save_key(
            agent_name=agent_id,
            owner_id=owner_id or "",
            virtual_key=virtual_key,
            key_alias=f"agent-{agent_id}",
        )
    finally:
        client.close()

    console.print(
        f"[bold green]Virtual key minted for agent '{agent_id}':[/bold green] {virtual_key}"
    )


@app.command("rotate")
def rotate(
    agent: str = typer.Option(..., "--agent", help="Agent identifier."),
):
    """
    Rotate the virtual key for an agent (create new, delete old, update MongoDB).

    If the old-key deletion fails, both keys are left active and a warning is
    printed — the new key is always returned.
    """
    asyncio.run(_rotate_async(agent))


async def _rotate_async(agent_id: str):
    if not _master_key():
        console.print("[red]LITELLM_MASTER_KEY is not set.[/red]")
        raise typer.Exit(1)

    repo, client = await _repo_context()
    try:
        old_key = await repo.get_active_key(agent_id)
    finally:
        client.close()

    # Mint new key
    payload = {
        "key_alias": f"agent-{agent_id}-rotated",
        "metadata": {"agent_id": agent_id, "rotated": True},
        "max_budget": 5.0,
        "models": ["default-model"],
    }
    try:
        async with httpx.AsyncClient(timeout=15.0) as http:
            resp = await http.post(
                f"{_gateway_url()}/key/generate",
                headers=_auth_headers(),
                json=payload,
            )
            resp.raise_for_status()
            new_key = resp.json().get("key", "")
    except Exception as exc:
        console.print(f"[red]Failed to mint new key: {exc}[/red]")
        raise typer.Exit(1)

    if not new_key:
        console.print("[red]Gateway response missing 'key' field.[/red]")
        raise typer.Exit(1)

    # Delete old key (best-effort)
    if old_key:
        try:
            async with httpx.AsyncClient(timeout=10.0) as http:
                del_resp = await http.post(
                    f"{_gateway_url()}/key/delete",
                    headers=_auth_headers(),
                    json={"keys": [old_key]},
                )
                del_resp.raise_for_status()
        except Exception as exc:
            console.print(
                f"[yellow]Warning: Could not delete old key for '{agent_id}': {exc}. "
                "Both keys remain active — revoke the old key manually.[/yellow]"
            )

    # Update MongoDB
    repo, client = await _repo_context()
    try:
        await repo.mark_rotated(agent_id, new_key)
    finally:
        client.close()

    console.print(
        f"[bold green]Key rotated for agent '{agent_id}':[/bold green] {new_key}\n"
        "[dim]Restart or re-deploy the agent container to inject the new key.[/dim]"
    )


@app.command("revoke")
def revoke(
    agent: str = typer.Option(..., "--agent", help="Agent identifier."),
):
    """
    Revoke the virtual key for an agent.

    Calls POST /key/delete on the gateway and marks the MongoDB record as revoked.
    Subsequent LLM calls from the agent will receive 401 Unauthorized.
    """
    asyncio.run(_revoke_async(agent))


async def _revoke_async(agent_id: str):
    if not _master_key():
        console.print("[red]LITELLM_MASTER_KEY is not set.[/red]")
        raise typer.Exit(1)

    repo, client = await _repo_context()
    try:
        key = await repo.get_active_key(agent_id)
    finally:
        client.close()

    if not key:
        console.print(
            f"[yellow]No active key found for agent '{agent_id}' in MongoDB.[/yellow]"
        )
        raise typer.Exit(1)

    # Delete from LiteLLM
    try:
        async with httpx.AsyncClient(timeout=10.0) as http:
            resp = await http.post(
                f"{_gateway_url()}/key/delete",
                headers=_auth_headers(),
                json={"keys": [key]},
            )
            resp.raise_for_status()
    except Exception as exc:
        console.print(f"[red]Gateway delete failed: {exc}[/red]")
        raise typer.Exit(1)

    # Mark revoked in MongoDB
    repo, client = await _repo_context()
    try:
        await repo.mark_revoked(agent_id, key)
    finally:
        client.close()

    console.print(
        f"[bold green]Key revoked for agent '{agent_id}'.[/bold green] "
        "The agent will receive 401 Unauthorized on its next LLM call."
    )


@app.command("list-keys")
def list_keys():
    """List all virtual key records from MongoDB (key values masked)."""
    asyncio.run(_list_keys_async())


async def _list_keys_async():
    repo, client = await _repo_context()
    try:
        records = await repo.list_all()
    finally:
        client.close()

    if not records:
        console.print("[dim]No virtual key records found in MongoDB.[/dim]")
        return

    table = Table(title="Virtual Keys", show_header=True, header_style="bold blue")
    table.add_column("Agent", style="cyan")
    table.add_column("Owner", style="white")
    table.add_column("Key (masked)", style="yellow")
    table.add_column("Status", style="green")
    table.add_column("Created At")
    table.add_column("Rotated At")

    for rec in records:
        table.add_row(
            rec.get("agent_name", ""),
            rec.get("owner_id", ""),
            rec.get("virtual_key", ""),
            rec.get("status", ""),
            str(rec.get("created_at", "")),
            str(rec.get("rotated_at", "") or "—"),
        )

    console.print(table)


@app.command("info")
def info(
    key: str = typer.Option(..., "--key", help="Virtual key value to look up."),
):
    """Fetch key info from the LiteLLM gateway and print JSON."""
    asyncio.run(_info_async(key))


async def _info_async(key: str):
    if not _master_key():
        console.print("[red]LITELLM_MASTER_KEY is not set.[/red]")
        raise typer.Exit(1)

    url = f"{_gateway_url()}/key/info"
    try:
        async with httpx.AsyncClient(timeout=10.0) as http:
            resp = await http.get(
                url,
                headers=_auth_headers(),
                params={"key": key},
            )
            resp.raise_for_status()
    except httpx.HTTPStatusError as exc:
        console.print(
            f"[red]Gateway returned HTTP {exc.response.status_code}: {exc.response.text}[/red]"
        )
        raise typer.Exit(1)
    except Exception as exc:
        console.print(f"[red]Could not reach gateway: {exc}[/red]")
        raise typer.Exit(1)

    import json

    console.print_json(json.dumps(resp.json()))
