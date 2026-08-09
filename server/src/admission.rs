//! Corporate-only login admission for multi-tenant mode.
//!
//! In `MULTI_TENANT_MODE` a workspace belongs to a company, so a login must
//! carry a corporate identity. This is the single gate both SSO callbacks run
//! (GitHub in [`crate::github`], OIDC/Google in `nasiko-server-ee`'s `oidc`)
//! after they resolve a verified email — kept here, pure and shared, so the two
//! paths can never drift apart.
//!
//! Outside multi-tenant mode the gate is inert: a single-tenant control plane
//! keeps admitting every identity exactly as it did before.

/// Consumer email providers. A verified email whose domain is one of these is
/// treated as personal, not corporate. Deliberately not exhaustive — it only
/// needs to catch the mainstream consumer providers; any other domain is
/// assumed to be a company's. (Post-V1 a stronger signal — GitHub org
/// membership, verified domains — supersedes this heuristic; see
/// `docs/MULTITENANT.md` §12.)
const PERSONAL_EMAIL_DOMAINS: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "outlook.com",
    "hotmail.com",
    "live.com",
    "msn.com",
    "yahoo.com",
    "yahoo.co.uk",
    "ymail.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "aol.com",
    "proton.me",
    "protonmail.com",
    "pm.me",
    "gmx.com",
    "gmx.net",
    "zoho.com",
    "yandex.com",
    "mail.com",
    "fastmail.com",
    "hey.com",
    "duck.com",
];

/// Whether a domain is a known personal-email provider (case-insensitive).
pub fn is_personal_domain(domain: &str) -> bool {
    let d = domain.trim().to_ascii_lowercase();
    PERSONAL_EMAIL_DOMAINS.contains(&d.as_str())
}

/// The outcome of the admission check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// The login may proceed.
    Allow,
    /// Corporate-only mode rejected a personal / unverifiable identity. The
    /// caller must reject *before* creating any user — nothing is provisioned.
    RejectPersonal,
}

/// Decide whether a login may proceed under this control plane's admission
/// policy, given what the SSO provider told us.
///
/// - `multi_tenant_mode` / `allow_personal_emails` come from the CP config.
/// - `verified_email` is the address the provider asserted verified (`None`
///   when it gave us nothing trustworthy).
/// - `google_hd` is Google's hosted-domain claim (`Some` only for a managed
///   Workspace account) — an unconditional corporate signal when present.
///
/// Rules, in order:
/// 1. Not multi-tenant → always `Allow` (single-tenant behavior is untouched).
/// 2. `allow_personal_emails` on → always `Allow`.
/// 3. Corporate-only: allow a Google `hd`, or a verified email on a
///    non-personal domain; reject everything else — including a login with no
///    verified email at all, since there is no corporate identity to place it
///    under.
pub fn check(
    multi_tenant_mode: bool,
    allow_personal_emails: bool,
    verified_email: Option<&str>,
    google_hd: Option<&str>,
) -> Admission {
    if !multi_tenant_mode || allow_personal_emails {
        return Admission::Allow;
    }
    if google_hd.is_some_and(|hd| !hd.trim().is_empty()) {
        return Admission::Allow;
    }
    match verified_email.and_then(email_domain) {
        Some(domain) if !is_personal_domain(&domain) => Admission::Allow,
        _ => Admission::RejectPersonal,
    }
}

/// Lower-cased domain part of an email address, or `None` if it has no `@` or an
/// empty domain.
fn email_domain(email: &str) -> Option<String> {
    email
        .rsplit_once('@')
        .map(|(_, domain)| domain.trim().to_ascii_lowercase())
        .filter(|d| !d.is_empty())
}
