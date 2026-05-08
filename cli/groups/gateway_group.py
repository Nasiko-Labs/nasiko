"""LLM Gateway command group — manage virtual keys and gateway health."""

import typer

gateway_app = typer.Typer(help="Manage the platform LLM gateway and virtual keys")
keys_app = typer.Typer(help="Virtual key management (mint / rotate / revoke)")
gateway_app.add_typer(keys_app, name="keys")


@gateway_app.command(name="health")
def health() -> None:
    """Show gateway readiness + configured provider models."""
    from commands.gateway import gateway_health_command

    gateway_health_command()


@keys_app.command(name="list")
def list_keys(
    format_type: str = typer.Option(
        "table", "--format", "-f", help="Output format: table, json"
    ),
) -> None:
    """List all virtual keys currently provisioned on the gateway."""
    from commands.gateway import list_keys_command

    list_keys_command(format_type)


@keys_app.command(name="rotate")
def rotate_key(
    agent_name: str = typer.Argument(..., help="Agent name to rotate the key for"),
    max_budget: float = typer.Option(
        None,
        "--max-budget",
        "-b",
        help="Optional USD budget cap for the new key (e.g. 10.0)",
    ),
) -> None:
    """Delete the current virtual key for an agent and mint a fresh one."""
    from commands.gateway import rotate_key_command

    rotate_key_command(agent_name, max_budget)


@keys_app.command(name="revoke")
def revoke_key(
    agent_name: str = typer.Argument(..., help="Agent name to revoke the key for"),
) -> None:
    """Delete the virtual key for an agent. Calls 401 until the agent is redeployed."""
    from commands.gateway import revoke_key_command

    revoke_key_command(agent_name)
