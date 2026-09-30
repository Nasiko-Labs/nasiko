# Security Policy

## Reporting a Vulnerability

**Do not open a public GitHub issue for a security vulnerability, and do not
post secrets, credentials, tokens, or exploit details in any public channel
(issues, discussions, or Discord).**

Use GitHub's private reporting for this repository:

1. Go to the repository's **Security** tab.
2. Select **Report a vulnerability** to open a private security advisory.

This keeps the report visible only to the Nasiko maintainers until a fix is
ready.

GitHub's **Security → Report a vulnerability** mechanism is the preferred
reporting channel. If that mechanism is unavailable for your account, contact
the maintainers privately through an officially documented channel listed in
`README.md` without disclosing the vulnerability details publicly. General
bugs that are not security-sensitive should use the public bug-report issue
template instead.

## What to Include

Where it is safe to do so, include:

* A description of the vulnerability and its potential impact.
* Steps to reproduce or a minimal proof of concept.
* Affected version/commit and environment (OS, Docker/Compose or Rust setup).
* Any relevant logs or configuration, with all secrets, keys, tokens, and
  personal data redacted.

## Scope

This policy covers the code in this repository (`Nasiko-Labs/nasiko`). The
project is licensed under Apache-2.0 (see `LICENSE`).

## Response

Maintainers will review private reports, ask follow-up questions through the
private advisory thread, and coordinate a fix and disclosure. Public
recognition depends on the reporter's preference and maintainer confirmation.
