# Security Policy

## Reporting a Vulnerability

**Please do not open a public issue for a security vulnerability.**

Report via either channel:

- **GitHub Private Vulnerability Reporting** on this repository (Security tab → "Report a vulnerability")
- Email: **security@nasiko.com**

Include the affected component, reproduction steps or a proof of concept, and your impact
assessment. We acknowledge within **2 business days** and send a triage verdict within **7 days**.

Good-faith security research against deployments you own or are authorized to test is welcomed
and will not result in legal action from us.

## Scope

In scope: the control-plane server, routing engine, MCP gateway, LLM router, embedded OCI
registry, auth, flow guard, secrets handling, CLI, coding-agent harness shim, web UI (including
the GenUI sandbox frame), and the example agents in this repository.

Out of scope: vulnerabilities in third-party dependencies without a demonstrated exploit path
through Nasiko code; denial-of-service against deployments you do not own; social engineering.

## Security Model (summary)

Nasiko is a **single control-plane process and the sole ingress** — there is no separate gateway.
Agents are never publicly exposed; every agent-to-agent call is proxied back through the server,
which enforces authentication, ACLs, rate limits, and anti-cascade flow limits at that one
chokepoint. Agents never hold real LLM API keys (the LLM router issues per-agent identity tokens)
and never see user session credentials (the proxy strips them before forwarding). Agent secrets
are AES-256-GCM encrypted at rest and decrypted only into container env at deploy time. The MCP
gateway authenticates agents with a deploy-time credential and binds user identity to the
request's `traceparent` via the flow record — both fail closed.

The full analysis: [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md). How we handle incidents:
[`docs/INCIDENT_RESPONSE.md`](docs/INCIDENT_RESPONSE.md).

## Supported Versions

Nasiko is pre-1.0 and rolling-release. **Only the latest `main` and the current release tag
receive security fixes.**

## Disclosure Policy

- We aim to confirm, fix, and release within **90 days** of a report; critical issues faster.
- We publish a GitHub Security Advisory with the fix release and credit reporters unless they
  ask otherwise.
- Please withhold public details until a fix is available.

## Operator Hardening Baseline

Running a Nasiko control plane? Minimum secure posture:

1. **Change the bootstrap admin password** (`ADMIN_PASSWORD`, default `changeme`) on first boot.
2. Set strong, unique `SECRETS_ENCRYPTION_KEY` (base64, 32 bytes) and `JWT_SECRET`.
3. Terminate TLS in front of the control plane; never expose it over plain HTTP beyond localhost.
4. Keep Postgres, Redis, and S3 off public networks — the control plane is the only public port.
5. Set `CORS_ALLOWED_ORIGINS` explicitly if browsers call the API cross-origin (default: empty).
6. Restrict `GIT_CLONE_ALLOWED_HOSTS` and `REGISTRY_IMPORT_ALLOWED_HOSTS` to hosts you trust.
7. Treat `BUILD_PUSH_TOKEN` as a cluster-wide root-equivalent credential; rotate on suspicion.
8. Review `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT` — captured prompt/completion
   content in Tempo/Loki is sensitive; apply access control and retention.
9. Restrict agent egress to what agents need (LLM router, MCP gateway, approved APIs).
10. Apply security releases promptly.
