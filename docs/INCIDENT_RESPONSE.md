# Incident Response Plan (OSS Edition)

**Companion:** [`THREAT_MODEL.md`](THREAT_MODEL.md) — scenario IDs map to its threat rows (T#).
**Framework:** NIST SP 800-61. **Applies to:** this repository, its maintainers, and reference
deployments of the OSS control plane. Operators running their own deployment should adapt §2.1
(preparation) and §3 (playbooks) to their environment.

## 0. Roles & contacts

| Role | Who | Responsibility |
|---|---|---|
| Incident Commander (IC) | First responding maintainer until relieved | Declares the incident, runs the timeline, delegates |
| Tech Lead | Nasiko development lead | Technical decisions, containment execution |
| Comms | IC designates | All external messaging — **no one else communicates externally** |

- Vulnerability intake: GitHub Private Vulnerability Reporting, or a private Discord channel with a
  maintainer (see [`SECURITY.md`](../SECURITY.md)).
- Declare an incident when in doubt — a stood-down false alarm costs an hour; a late declaration
  costs a breach.

## 1. Severity

| Sev | Definition | Examples | Response |
|---|---|---|---|
| **SEV-1** | User data exposed, auth bypass in the wild, platform-wide compromise | T2 exploited, Postgres dump taken | All hands, immediate containment, advisory with the fix |
| **SEV-2** | Single-deployment compromise, credential leak, bounded exposure | T1, T8, leaked `BUILD_PUSH_TOKEN` | Same-day containment, notify affected parties |
| **SEV-3** | Availability/cost incidents, exploitation blocked by controls | T6 cascade storm, T11 login DoS | Fix in the next release cycle, monitor |
| **SEV-4** | Near-misses, control gaps found in review | Missing dispatch-path participant insert | Track as a hardening task |

## 2. Phases

### 2.1 Preparation (maintainers: do these now)

- [ ] GitHub PVR enabled and triaged; private Discord requests answered.
- [ ] Maintainers have break-glass access to reference deployments, CI, and the package/registry accounts.
- [ ] Alerts wired on reference deployments: `GuardUnavailable` spikes, LLM-router 403 storms, OCI 401 bursts.
- [ ] Postgres backups with point-in-time recovery; restore tested quarterly.
- [ ] This plan + the threat model reviewed after every SEV-1/2 and every 6 months.

### 2.2 Detection & analysis

Sources: alerts, user reports, vulnerability reports, GitHub advisories, Tempo/Loki anomalies.

1. **Declare** (IC) with a severity guess — refine later.
2. **Start the timeline** — every action timestamped, one shared doc.
3. **Triage:** real or noise? Which trust boundary? Which scenario (§3)?
4. **Scope:** which deployments / agents / users affected? Since when?
5. **Preserve evidence** before cleanup: logs, DB rows (`flows`, `agent_gateway_tokens`,
   `token_usage`), container images.

### 2.3 Containment — platform levers (narrowest first)

| Lever | Action | Kills |
|---|---|---|
| Revoke one agent's gateway credential | `DELETE /api/agents/{id}/mcp-token` (immediate; a redeploy also rotates it, but the old credential keeps working for a 15-minute grace) | That agent's MCP and LLM-router access and its calls to peer agents |
| Complete a flow | Mark the `flows` row completed | All MCP/LLM calls bound to it (both fail closed) |
| Scale agent to 0 / destroy | `nasiko rm` | The agent container |
| Rotate `BUILD_PUSH_TOKEN` | Env change + restart | Registry push access cluster-wide |
| Block at the edge | Firewall / reverse-proxy rule | The network path |
| **Nuclear:** stop the control plane | Stop `nasiko-server` | Everything — sole ingress; last resort |

### 2.4 Eradication

- Rotate every credential in the blast radius: `JWT_SECRET`, `AGENT_JWT_SECRET` (signs agents'
  LLM-router JWTs), `MCP_IDENTITY_SIGNING_KEY`, `SECRETS_ENCRYPTION_KEY` (arrange the re-encryption of
  stored secrets), `BUILD_PUSH_TOKEN`, DB passwords, OAuth client secrets, LLM provider keys;
  revoke agents' gateway credentials (`DELETE /api/agents/{id}/mcp-token`) and redeploy them to
  mint new ones — a redeploy alone leaves the old credential valid for 15 minutes.
- Patch the hole, deploy, and verify the exploit path now fails.

### 2.5 Recovery

- Restore in dependency order: Postgres/Redis/S3 → control plane → agents.
- Elevated alerting for ≥ 48h; watch for re-exploitation before declaring recovery.

### 2.6 Post-incident

- Blameless review within 5 business days: timeline, root cause, what detected it (or didn't).
- Update the threat model — every incident is a row that was missing or under-ranked.
- For code-affecting incidents: GitHub Security Advisory + release notes, crediting the reporter.

## 3. Scenario playbooks

### S1 — Compromised/malicious agent (T1, T6)
Detect: flow-guard rejections, 403 storms, anomalous FinOps token usage.
Contain: complete the flow → revoke the agent's gateway credential
(`DELETE /api/agents/{id}/mcp-token`) → scale to 0.
Scope for notification: the participant check confines blast radius to users whose flows routed
through the agent — pull that list from `flow_participants`.

### S2 — Auth bypass / session compromise (T2, T9, T11)
Contain: if systemic, rotate `JWT_SECRET` (logs everyone out).
Eradicate: patch the middleware gap; add a route-inventory regression test.

### S3 — SSRF / data-store exposure (T3, T4)
Contain: block the egress path; revoke the registering user's access.
Eradicate: extend the SSRF guard to the new call site; rotate anything reachable (metadata
credentials, DB).

### S4 — Secrets or prompt content leaked via observability (T7)
Contain: restrict Tempo/Loki access; purge exposed spans where possible.
Eradicate: rotate any credential found in span data; reconsider
`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT`.

### S5 — Supply chain: malicious PR or dependency (T10)
Contain: revert in the public repo; yank affected release artifacts.
Eradicate: audit what the code did while live; if it ran in any deployment, treat as S1/S2.

### S6 — Sign-in / registry takeover (T16, T17, T19)
Contain: disable the affected sign-in provider or route; revoke sessions of adopted accounts;
freeze pushes to the affected repositories.
Eradicate: fix the linking/claim rule; redeploy agents from known-good digests.

### S7 — Tokens in logs (T21)
Contain: restrict log/trace access.
Eradicate: rotate tokens found in logs; ship the fix as a security release.

## 4. Communication

- **Security releases:** GitHub Security Advisory + release notes, crediting the reporter unless
  they ask otherwise.
- **Operators of affected deployments:** what happened, what is affected, what they must do
  (rotate X, upgrade to Y), and the follow-up timeline.
- **Regulatory:** if personal data of EU residents is involved, assess GDPR 72-hour notification
  immediately — do not wait for full root cause.

*Last reviewed: 2026-11-12.*
