import asyncio
from textual.app import App, ComposeResult
from textual.widgets import Header, Footer, Static
from textual.containers import Vertical, Horizontal
from textual.binding import Binding

from firewall.models import FirewallVerdict, Decision
from firewall.firewall import firewall
from events.event_bus import bus
from ui.widgets import FirewallTable, StatsBar, ApprovalBanner, LogoBanner
from ui.logs import EventLog
from textual.widgets import Input


CSS = """
Screen {
    layout: vertical;
    background: #0a0b11;
}

#header-row {
    height: 9;
    layout: horizontal;
    background: #161925;
    border-bottom: double #00ccff;
    padding: 1 2;
    margin-bottom: 1;
}

#logo {
    width: 1fr;
}

#stats {
    width: auto;
    content-align: right middle;
}

#approval-area {
    height: auto;
    min-height: 0;
}

#top {
    height: 1fr;
    layout: horizontal;
}

#table-pane {
    width: 2fr;
    border: tall #00ccff;
    background: #10121d;
    margin: 0 1;
}

#table-pane-title {
    background: #00ccff;
    color: #0a0b11;
    padding: 0 1;
    text-style: bold;
    width: 100%;
}

#log-pane {
    width: 1fr;
    border: tall #ff00ff;
    background: #10121d;
    margin: 0 1;
}

#log-pane-title {
    background: #ff00ff;
    color: #0a0b11;
    padding: 0 1;
    text-style: bold;
    width: 100%;
}

#user-query {
    margin: 1 1;
    border: tall #00ccff;
    background: #161925;
    color: #00f2ff;
}

Footer {
    background: #161925;
    color: #00ccff;
}
"""


class AegisDashboard(App):
    CSS = CSS
    TITLE = "Aegis"
    BINDINGS = [
        Binding("a", "approve", "Approve"),
        Binding("d", "deny", "Deny"),
        Binding("q", "quit", "Quit"),
    ]

    def __init__(self, agent_runner=None):
        super().__init__()
        self._agent_runner = agent_runner
        self._pending_call_id: str | None = None

    def compose(self) -> ComposeResult:
        with Horizontal(id="header-row"):
            yield LogoBanner(id="logo")
            yield StatsBar(id="stats")
        yield Static("", id="approval-area")
        with Horizontal(id="top"):
            with Vertical(id="table-pane"):
                yield Static("⚡ Tool Calls", id="table-pane-title")
                yield FirewallTable(id="table")
            with Vertical(id="log-pane"):
                yield Static("📋 Event Log", id="log-pane-title")
                yield EventLog(id="log", highlight=True, markup=True)
        yield Footer()

    def on_mount(self) -> None:
        bus.subscribe("firewall_verdict", self._on_verdict)
        bus.subscribe("approval_request", self._on_approval_request)
        bus.subscribe("user_query", self._on_user_query)
        if self._agent_runner:
            self.run_worker(self._agent_runner(), exclusive=False)

    async def _on_user_query(self, query: str) -> None:
        if self._agent_runner:
            # Run the agent with the new query in a background worker
            self.run_worker(self._agent_runner(prompt=query), exclusive=False)

    async def _on_verdict(self, verdict: FirewallVerdict) -> None:
        self._update_ui(verdict)

    def _update_ui(self, verdict: FirewallVerdict) -> None:
        table: FirewallTable = self.query_one("#table")
        log: EventLog = self.query_one("#log")
        stats: StatsBar = self.query_one("#stats")

        table.add_verdict(verdict)
        log.log_verdict(verdict)

        if verdict.decision == Decision.ALLOW:
            stats.allowed += 1
        elif verdict.decision == Decision.WARN:
            stats.warned += 1
        elif verdict.decision == Decision.BLOCK:
            stats.blocked += 1

    async def _on_approval_request(self, verdict: FirewallVerdict) -> None:
        self._show_approval(verdict)

    def _show_approval(self, verdict: FirewallVerdict) -> None:
        self._pending_call_id = verdict.call.call_id
        area = self.query_one("#approval-area")
        area.remove_children()
        area.mount(ApprovalBanner(verdict.call.tool, verdict.call.call_id))

    def _clear_approval(self) -> None:
        self._pending_call_id = None
        area = self.query_one("#approval-area")
        area.remove_children()

    def action_approve(self) -> None:
        if self._pending_call_id:
            firewall.resolve_approval(self._pending_call_id, True)
            self._clear_approval()

    def action_deny(self) -> None:
        if self._pending_call_id:
            firewall.resolve_approval(self._pending_call_id, False)
            self._clear_approval()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        query = event.value
        if query == "clear":
            self.clear()
        else:
            firewall.handle_user_input(query)