import asyncio
from nasiko.phoenix_logger import register as register_phoenix
from agents.demo_agent import run as run_agent
from ui.dashboard import AegisDashboard


from rich.console import Console
from rich.panel import Panel

def main():
    register_phoenix()
    console = Console()

    console.print(Panel.fit(
        "[bold cyan]AEGIS FIREWALL - MODE SELECTION[/]\n\n"
        "1. [bold green]USER DEFINED[/] - Enter a custom task\n"
        "2. [bold yellow]DEMO MODE[/]     - Run scripted scenario",
        border_style="cyan"
    ))
    
    choice = console.input("\n[bold]Select mode (1/2) [default 2]: [/]").strip() or "2"
    
    initial_prompt = None
    if choice == "1":
        initial_prompt = console.input("[bold green]Enter your command for the agent: [/]")

    async def agent_runner(prompt=initial_prompt):
        # Small delay so the UI renders before the agent starts firing
        await asyncio.sleep(1.5)
        await run_agent(prompt=prompt)

    app = AegisDashboard(agent_runner=agent_runner)
    app.run()


if __name__ == "__main__":
    main()
