//! Credentials and personal data taken out of free text before it leaves the process: a log line
//! sent to an error tracker, say. Free text has no structure to allow-list, so this redacts by
//! value: URLs (credential query values, user:password, the credential path), email addresses,
//! `name=value` pairs whose name looks sensitive, and the word after `Bearer`.

const REDACTED: &str = "[REDACTED]";
const EMAIL: &str = "[email]";
/// The MCP gateway's credential-in-the-path route (`/api/mcp/s/{token}`).
const CREDENTIAL_PATH: &str = "/api/mcp/s/";

/// Name parts that mark a field, header or query parameter as secret or personal.
const SENSITIVE_NAME_PARTS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "key",
    "code",
    "state",
    "sig",
    "auth",
    "session",
    "credential",
    "jwt",
    "cookie",
    "email",
    "kubeconfig",
];

/// Whether a field or parameter called `name` may hold a secret or personal data.
pub fn is_sensitive_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SENSITIVE_NAME_PARTS.iter().any(|part| name.contains(part))
}

/// A string field of a structured log line, scrubbed in place: a field whose name looks secret or
/// personal keeps only a placeholder, any other has its value redacted like free text.
pub fn redact_field(name: &str, value: &mut String) {
    *value = if is_sensitive_name(name) {
        REDACTED.to_owned()
    } else {
        redact_text(value)
    };
}

/// `text` with every credential and email address it contains replaced; whitespace is kept.
pub fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after_bearer = false;
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end_matches(char::is_whitespace);
        let space = &piece[word.len()..];
        if word.is_empty() {
            out.push_str(piece);
            continue;
        }
        if after_bearer {
            out.push_str(REDACTED);
        } else {
            out.push_str(&redact_word(word));
        }
        out.push_str(space);
        after_bearer = word.eq_ignore_ascii_case("bearer");
    }
    out
}

/// One whitespace-free word, with trailing punctuation kept outside the redaction.
fn redact_word(word: &str) -> String {
    let core = word.trim_end_matches([',', ';', ')', ']', '}', '"', '\'', '.']);
    let tail = &word[core.len()..];
    let core = core.trim_start_matches(['(', '[', '{', '"', '\'']);
    let head = &word[..word.len() - tail.len() - core.len()];
    format!("{head}{}{tail}", redact_core(core))
}

fn redact_core(core: &str) -> String {
    if core.contains("://") || (core.starts_with('/') && core.contains('?')) {
        return redact_url(core);
    }
    if looks_like_email(core) {
        return EMAIL.to_owned();
    }
    if let Some((name, _)) = core.split_once('=')
        && is_sensitive_name(name)
    {
        return format!("{name}={REDACTED}");
    }
    core.to_owned()
}

fn looks_like_email(word: &str) -> bool {
    let Some((local, domain)) = word.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

/// A URL (absolute or a path) without user:password, the credential path's token or sensitive
/// query values. The fragment is dropped.
fn redact_url(url: &str) -> String {
    let url = url.split('#').next().unwrap_or_default();
    let (before_query, query) = match url.split_once('?') {
        Some((head, query)) => (head, Some(query)),
        None => (url, None),
    };
    let (scheme, rest) = match before_query.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, before_query),
    };
    let (authority, path) = match scheme {
        Some(_) => match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, ""),
        },
        None => ("", rest),
    };
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => format!("{REDACTED}@{host}"),
        None => authority.to_owned(),
    };
    let (path, query) = match path.find(CREDENTIAL_PATH) {
        Some(at) => (format!("{}{CREDENTIAL_PATH}{{token}}", &path[..at]), None),
        None => (path.to_owned(), query),
    };
    let mut out = match scheme {
        Some(scheme) => format!("{scheme}://{authority}{path}"),
        None => path,
    };
    if let Some(query) = query {
        out.push('?');
        out.push_str(&redact_query(query));
    }
    out
}

fn redact_query(query: &str) -> String {
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, _)) if is_sensitive_name(name) => format!("{name}={REDACTED}"),
            _ => pair.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unchanged() {
        let line = "backup scheduler: 3 clusters due, next pass in 300s";
        assert_eq!(redact_text(line), line);
    }

    #[test]
    fn emails_are_replaced() {
        assert_eq!(
            redact_text("invite sent to ada@acme.com, retrying (bob@x.io)"),
            "invite sent to [email], retrying ([email])"
        );
        assert_eq!(redact_text("user@localhost stays"), "user@localhost stays");
    }

    #[test]
    fn credential_query_values_and_userinfo_are_redacted() {
        assert_eq!(
            redact_text(
                "GET https://cp.example.com/api/auth/sso/session?token=jwt.v&page=2 failed"
            ),
            "GET https://cp.example.com/api/auth/sso/session?token=[REDACTED]&page=2 failed"
        );
        assert_eq!(
            redact_text("connect postgres://nasiko:hunter2@10.0.0.5:5432/db"),
            "connect postgres://[REDACTED]@10.0.0.5:5432/db"
        );
        assert_eq!(
            redact_text("callback /auth/github/callback?code=abc&state=xyz#frag"),
            "callback /auth/github/callback?code=[REDACTED]&state=[REDACTED]"
        );
    }

    #[test]
    fn the_credential_path_loses_its_token() {
        assert_eq!(
            redact_text("proxy https://ws.example.com/api/mcp/s/ngt_secret/tools?x=1"),
            "proxy https://ws.example.com/api/mcp/s/{token}"
        );
    }

    #[test]
    fn sensitive_pairs_and_bearer_tokens_are_redacted() {
        assert_eq!(
            redact_text("refused api_key=sk-123 region=us"),
            "refused api_key=[REDACTED] region=us"
        );
        assert_eq!(
            redact_text("header Authorization: Bearer eyJhbGci.x.y sent"),
            "header Authorization: Bearer [REDACTED] sent"
        );
    }

    #[test]
    fn fields_are_redacted_by_name_then_by_value() {
        let mut email = "ada@acme.com".to_owned();
        redact_field("user_email", &mut email);
        assert_eq!(email, REDACTED);
        let mut url = "/cb?code=abc&x=1".to_owned();
        redact_field("url", &mut url);
        assert_eq!(url, "/cb?code=[REDACTED]&x=1");
    }

    #[test]
    fn sensitive_names() {
        assert!(is_sensitive_name("user_email"));
        assert!(is_sensitive_name("X-API-Key"));
        assert!(is_sensitive_name("kubeconfig_path"));
        assert!(!is_sensitive_name("cluster_id"));
        assert!(!is_sensitive_name("status"));
    }
}
