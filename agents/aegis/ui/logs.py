from textual.widgets import RichLog
from firewall.models import FirewallVerdict, Decision


ICONS = {Decision.ALLOW: "✓", Decision.WARN: "⚠", Decision.BLOCK: "✗", Decision.PENDING: "?"}
COLORS = {Decision.ALLOW: "#00ff00", Decision.WARN: "#ffcc00", Decision.BLOCK: "#ff3333", Decision.PENDING: "#00ccff"}


class EventLog(RichLog):
    """Scrolling log of firewall events."""

    def log_verdict(self, verdict: FirewallVerdict) -> None:
        d = verdict.decision
        icon, color = ICONS[d], COLORS[d]
        ts = verdict.call.timestamp.strftime("%H:%M:%S")
        
        # Tool call text
        tool_text = f"[cyan]{verdict.call.tool:<18}[/cyan]"
        
        # Decision status
        status_text = f"[bold {color}]{icon} {d.value:7}[/bold {color}]"
        
        # Risk score with color
        score = verdict.risk.score
        score_color = "#00ff00" if score < 0.3 else ("#ffff00" if score < 0.6 else ("#ff9900" if score < 0.8 else "#ff0000"))
        risk_text = f"risk=[{score_color}]{score:.2f}[/]"
        
        line = f"[dim]{ts}[/dim] {status_text} {tool_text} {risk_text}"
        
        if verdict.violation:
            line += f"  [#ff00ff dim]({verdict.violation.rule})[/#ff00ff dim]"
            
        self.write(line)
