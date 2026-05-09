from .models import RiskResult, ToolCall

_DANGEROUS_KEYWORDS = [
    "aws_secret", "api_key", "password", "token", "credential",
    "rm -rf", "drop table", "exfil", "extract_secret",
]

_HIGH_RISK_TOOLS = {
    "send_email": 0.85,
    "delete_file": 0.90,
    "shell_exec": 0.75,
    "extract_secrets": 0.95,
}

_SUSPICIOUS_DOMAINS = ["pastebin.com", "ngrok.io", "requestbin.com"]


def score(call: ToolCall) -> RiskResult:
    base = _HIGH_RISK_TOOLS.get(call.tool, 0.1)
    reasons = []

    args_str = " ".join(str(v) for v in call.args.values()).lower()

    for kw in _DANGEROUS_KEYWORDS:
        if kw in args_str:
            base = min(1.0, base + 0.25)
            reasons.append(f"dangerous keyword: {kw}")

    for domain in _SUSPICIOUS_DOMAINS:
        if domain in args_str:
            base = min(1.0, base + 0.3)
            reasons.append(f"suspicious domain: {domain}")

    reason = "; ".join(reasons) if reasons else f"tool={call.tool}"
    return RiskResult(score=round(base, 2), reason=reason)
