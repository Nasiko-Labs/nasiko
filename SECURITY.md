# Security Policy

Nasiko runs in the call path of agents, coding harnesses and model providers, and it handles
credentials, provider keys and user data. We take security reports seriously and appreciate the
work of everyone who helps keep Nasiko and its users safe.

## Reporting a vulnerability

**Do not report security vulnerabilities through public GitHub issues, pull requests, discussions or
Discord channels.**

Report them privately through GitHub's private vulnerability reporting:
[Report a vulnerability](https://github.com/Nasiko-Labs/nasiko/security/advisories/new).

If you cannot use GitHub, contact a maintainer privately on
[Discord](https://discord.com/invite/HmnfkTfjFv) and ask for a private channel. Do not include
vulnerability details in the first message.

Please include as much of the following as you can:

- the type of issue (for example authentication bypass, privilege escalation, injection, secret
  exposure, server-side request forgery);
- the affected component, endpoint or file, and the version or commit;
- the configuration needed to reproduce it;
- step-by-step reproduction instructions;
- a proof of concept, if you have one;
- the impact, and how an attacker could exploit it.

## What to expect

| Step | Target |
|---|---|
| Acknowledgement of your report | within 3 business days |
| Initial assessment and severity | within 10 business days |
| Status updates while we work on a fix | at least every 14 days |

We will keep you informed while we investigate, tell you whether we accept the report, and agree a
disclosure date with you. Once a fix is released, we publish a security advisory and credit you
unless you ask to remain anonymous.

## Supported versions

Security fixes are made on the `main` branch and included in the next release. We do not backport
fixes to older releases; upgrade to the latest release to receive them.

## Scope

In scope:

- the Nasiko server: API, authentication and authorization, agent proxy, LLM router, MCP gateway,
  embedded registry and web UI;
- the `nasiko` CLI;
- the example agents and Docker images published from this repository.

Out of scope:

- vulnerabilities in third-party dependencies that have no demonstrated impact on Nasiko (report
  those to the upstream project);
- issues that require a compromised host, a malicious administrator, or physical access;
- denial of service through volumetric traffic;
- findings from automated scanners without a demonstrated, reproducible impact;
- missing security headers or best-practice recommendations with no exploitable impact;
- social engineering of Nasiko maintainers or users.

## Safe harbor

We will not pursue legal action against anyone who, in good faith:

- reports a vulnerability through the channels above;
- tests only against their own installation or accounts they own;
- avoids privacy violations, data destruction, and degradation of service for others;
- gives us reasonable time to fix the issue before disclosing it publicly.

Thank you for helping keep Nasiko secure.
