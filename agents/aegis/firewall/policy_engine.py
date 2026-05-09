from pathlib import Path
import yaml
from .models import PolicyViolation, ToolCall, RiskResult

_DEFAULT_POLICY = {
    "blocked_tools": ["send_email", "delete_file"],
    "approval_required": ["shell_exec", "extract_secrets"],
    "allowed_domains": ["github.com", "arxiv.org"],
    "max_risk_score": 0.8,
}


class PolicyEngine:
    def __init__(self, policy_path: str = "config/policies.yaml"):
        path = Path(policy_path)
        if path.exists():
            with open(path) as f:
                self._policy = yaml.safe_load(f) or _DEFAULT_POLICY
        else:
            self._policy = _DEFAULT_POLICY

    def check(self, call: ToolCall, risk: RiskResult) -> PolicyViolation | None:
        if call.tool in self._policy.get("blocked_tools", []):
            return PolicyViolation("BLOCKED_TOOL", f"{call.tool} is explicitly blocked")

        if risk.score >= self._policy.get("max_risk_score", 0.8):
            return PolicyViolation("RISK_THRESHOLD", f"score {risk.score} >= {self._policy['max_risk_score']}")

        args_str = " ".join(str(v) for v in call.args.values()).lower()
        allowed = self._policy.get("allowed_domains", [])
        if "http" in args_str:
            if not any(d in args_str for d in allowed):
                return PolicyViolation("DOMAIN_NOT_ALLOWED", f"domain not in allowlist")

        return None

    def needs_approval(self, call: ToolCall) -> bool:
        return call.tool in self._policy.get("approval_required", [])
