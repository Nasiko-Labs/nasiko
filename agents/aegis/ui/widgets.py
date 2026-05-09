from textual.widgets import DataTable, Static
from textual.reactive import reactive
from rich.text import Text
from rich.panel import Panel
from rich.columns import Columns

LOGO = """\
[bold #00f2ff] █████╗ ███████╗ ██████╗ ██╗███████╗[/]
[bold #00d4ff]██╔══██╗██╔════╝██╔════╝ ██║██╔════╝[/]
[bold #00b6ff]███████║█████╗  ██║  ███╗██║███████╗[/]
[bold #0098ff]██╔══██║██╔══╝  ██║   ██║██║╚════██║[/]
[bold #007aff]██║  ██║███████╗╚██████╔╝██║███████║[/]
[bold #005cff]╚═╝  ╚═╝╚══════╝ ╚═════╝ ╚═╝╚══════╝[/] [dim]runtime firewall for AI agents[/]"""


class LogoBanner(Static):
    DEFAULT_CSS = "LogoBanner { padding: 0 2; height: 7; }"

    def render(self) -> str:
        return LOGO


DECISION_STYLE = {
    "ALLOW":   ("#00ff00", "✓"),
    "WARN":    ("#ffcc00", "⚠"),
    "BLOCK":   ("#ff3333", "✗"),
    "PENDING": ("#00ccff", "?"),
}


class FirewallTable(DataTable):
    """Live table of intercepted tool calls."""

    def on_mount(self) -> None:
        self.add_columns("Time", "Agent", "Tool", "Risk", "Decision", "Reason")
        self.cursor_type = "row"
        self.zebra_stripes = True

    def add_verdict(self, verdict) -> None:
        color, icon = DECISION_STYLE.get(verdict.decision.value, ("white", "·"))
        decision_text = Text(f"{icon} {verdict.decision.value}", style=f"bold {color}")
        risk_val = verdict.risk.score
        
        # Gradient risk color
        if risk_val < 0.3:
            risk_color = "#00ff00"
        elif risk_val < 0.6:
            risk_color = "#ffff00"
        elif risk_val < 0.8:
            risk_color = "#ff9900"
        else:
            risk_color = "#ff0000"
            
        self.add_row(
            verdict.call.timestamp.strftime("%H:%M:%S"),
            verdict.call.agent,
            Text(verdict.call.tool, style="cyan"),
            Text(f"{risk_val:.2f}", style=f"bold {risk_color}"),
            decision_text,
            Text(verdict.risk.reason[:50], style="dim"),
        )
        self.move_cursor(row=self.row_count - 1)


class StatsBar(Static):
    """Shows running counts of allow/warn/block with better styling."""

    allowed: reactive[int] = reactive(0)
    warned: reactive[int] = reactive(0)
    blocked: reactive[int] = reactive(0)

    def render(self) -> Columns:
        return Columns([
            Panel(f"[bold #00ff00]{self.allowed:02d}[/]\n[dim]ALLOW[/]", border_style="#00ff00", padding=(0, 2), title="[green]✓[/]"),
            Panel(f"[bold #ffcc00]{self.warned:02d}[/]\n[dim]WARN[/]", border_style="#ffcc00", padding=(0, 2), title="[yellow]⚠[/]"),
            Panel(f"[bold #ff3333]{self.blocked:02d}[/]\n[dim]BLOCK[/]", border_style="#ff3333", padding=(0, 2), title="[red]✗[/]"),
        ], padding=1)


class ApprovalBanner(Static):
    """Shown when a tool call needs human approval."""

    DEFAULT_CSS = "ApprovalBanner { background: #ff9900; color: black; padding: 0 1; margin: 1 0; }"

    def __init__(self, tool: str, call_id: str, **kwargs):
        super().__init__(**kwargs)
        self.tool = tool
        self.call_id = call_id

    def render(self) -> str:
        return (
            f"[bold]⚠ ACTION REQUIRED[/bold]  The agent wants to call [inverse] {self.tool} [/inverse]  "
            f"(id: [underline]{self.call_id}[/underline])\n"
            f"Press [bold]A[/bold] to [bold green]Approve[/] or [bold]D[/bold] to [bold red]Deny[/]"
        )
