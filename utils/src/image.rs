/// Strip registry host, tag, and digest from an OCI image reference, returning
/// just the repository path.
///
/// Follows Docker reference parsing rules:
/// - Digest (if present) is dropped first: `@sha256:...` → gone
/// - Tag is the part after the last `:` of the LAST path component only
/// - The first path component is a registry host when it contains `.` or `:` or equals `localhost`
///
/// Examples:
/// - `localhost:5000/nasiko/foo:1.0` → `nasiko/foo`
/// - `registry.io/u-abc.../agent:1.0` → `u-abc.../agent`
/// - `nasiko/agent:latest` → `nasiko/agent`
/// - `nasiko/agent@sha256:abc` → `nasiko/agent`
/// - `u-abc/agent` → `u-abc/agent`
pub fn strip_image_to_repo(image: &str) -> &str {
    // Drop digest first
    let without_digest = image.split('@').next().unwrap_or(image);

    // Split into segments on '/'
    let segments: Vec<&str> = without_digest.split('/').collect();

    if segments.is_empty() {
        return without_digest;
    }

    // Determine if the first segment is a registry host
    let first = segments[0];
    let has_registry = first.contains('.') || first.contains(':') || first == "localhost";

    // The repo path is everything after the registry (if present)
    let repo_path = if has_registry && segments.len() > 1 {
        &without_digest[first.len() + 1..]
    } else {
        without_digest
    };

    // Strip tag from the last component only - find the last '/' then the last ':' after it
    if let Some(last_slash_pos) = repo_path.rfind('/') {
        // Multi-segment repo: find ':' only in the last segment
        let last_segment = &repo_path[last_slash_pos + 1..];
        if let Some(colon_in_last) = last_segment.find(':') {
            let tag_start = last_slash_pos + 1 + colon_in_last;
            &repo_path[..tag_start]
        } else {
            repo_path
        }
    } else {
        // Single-segment repo: strip any trailing ':tag'
        repo_path.split(':').next().unwrap_or(repo_path)
    }
}

/// The platform repository an image names (host and tag stripped), or `None` when the image
/// lives on another registry. An image is a platform image when it has no registry host or its
/// host is exactly `platform_registry_host`; a different port is a different registry.
pub fn platform_repository_from_image<'a>(
    image: &'a str,
    platform_registry_host: &str,
) -> Option<&'a str> {
    let first = image.split('/').next().unwrap_or(image);
    let has_host =
        image.contains('/') && (first.contains('.') || first.contains(':') || first == "localhost");
    if has_host && !first.eq_ignore_ascii_case(platform_registry_host) {
        return None;
    }
    Some(strip_image_to_repo(image))
}

/// Parse an owner-namespaced repository path like `u-{hex32}/slug` into
/// `(owner_uuid, slug)`, or return `None` for legacy `nasiko/` repositories.
///
/// Rejects malformed namespace prefixes (wrong UUID length, uppercase hex).
/// Used for authorization: the caller's UUID must match the namespace owner.
pub fn parse_owner_namespace(repo: &str) -> Result<Option<(uuid::Uuid, &str)>, String> {
    let parts: Vec<&str> = repo.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(format!(
            "repository path '{}' has no namespace segment",
            repo
        ));
    }
    if let Some(hex) = parts[0].strip_prefix("u-") {
        if hex.len() != 32 {
            return Err(format!(
                "invalid owner namespace: '{}' must be exactly 32 hex digits",
                hex
            ));
        }
        if hex.chars().any(|c| c.is_ascii_uppercase()) {
            return Err(format!(
                "invalid owner namespace: '{}' must be lowercase hex",
                hex
            ));
        }
        let uuid = uuid::Uuid::parse_str(hex)
            .map_err(|_| format!("invalid owner namespace: '{hex}' is not a valid UUID"))?;
        Ok(Some((uuid, parts[1])))
    } else if repo.starts_with("nasiko/") {
        Ok(None)
    } else {
        Err(format!("unknown namespace in repository path '{repo}'"))
    }
}

/// Lowercase a display name into an OCI-safe repository component.
///
/// Replaces spaces with hyphens, filters to alphanumeric/hyphen/underscore,
/// trims leading/trailing hyphens, and falls back to "agent" if empty.
///
/// Must stay in sync across CLI and server - the CLI pushes to the slug
/// path, and the server builds with the same slug, so any divergence breaks
/// image resolution.
pub fn image_name_slug(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .replace(' ', "-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "agent".to_string()
    } else {
        slug.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_host_with_port_and_tag() {
        assert_eq!(
            strip_image_to_repo("localhost:5000/nasiko/foo:1.0"),
            "nasiko/foo"
        );
    }

    #[test]
    fn strip_host_with_port_no_tag() {
        assert_eq!(
            strip_image_to_repo("localhost:5000/nasiko/foo"),
            "nasiko/foo"
        );
    }

    #[test]
    fn strip_digest() {
        assert_eq!(
            strip_image_to_repo("nasiko/agent@sha256:abc123"),
            "nasiko/agent"
        );
    }

    #[test]
    fn strip_localhost_single_segment() {
        assert_eq!(strip_image_to_repo("localhost/x"), "x");
    }

    #[test]
    fn bare_repo_with_tag() {
        assert_eq!(strip_image_to_repo("nasiko/foo:1.0"), "nasiko/foo");
    }

    #[test]
    fn owner_namespace_with_tag() {
        assert_eq!(
            strip_image_to_repo("u-a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4/foo:1.0"),
            "u-a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4/foo"
        );
    }

    #[test]
    fn registry_with_dot_in_host() {
        assert_eq!(
            strip_image_to_repo("registry.io/u-abc/agent:1.0"),
            "u-abc/agent"
        );
    }

    #[test]
    fn colon_in_first_segment_not_last() {
        // registry.io:5000/repo/name:tag - the first ':' is in the host, not the tag
        assert_eq!(
            strip_image_to_repo("registry.io:5000/repo/name:tag"),
            "repo/name"
        );
    }

    #[test]
    fn slug_lowercase_and_spaces() {
        assert_eq!(image_name_slug("Code Reviewer"), "code-reviewer");
    }

    #[test]
    fn slug_preserves_hyphens_and_underscores() {
        assert_eq!(image_name_slug("my-agent_v2"), "my-agent_v2");
    }

    #[test]
    fn slug_strips_invalid_chars() {
        assert_eq!(image_name_slug("Agent@2024!"), "agent2024");
    }

    #[test]
    fn slug_empty_fallback() {
        assert_eq!(image_name_slug("!!!"), "agent");
    }

    #[test]
    fn slug_trims_leading_trailing_hyphens() {
        assert_eq!(image_name_slug("--agent--"), "agent");
    }

    #[test]
    fn platform_repository_unqualified_image() {
        assert_eq!(
            platform_repository_from_image("nasiko/agent:1.0", "registry.example.com"),
            Some("nasiko/agent")
        );
    }

    #[test]
    fn platform_repository_qualified_matching_host() {
        assert_eq!(
            platform_repository_from_image(
                "registry.example.com/nasiko/agent:1.0",
                "registry.example.com"
            ),
            Some("nasiko/agent")
        );
    }

    #[test]
    fn platform_repository_external_registry_returns_none() {
        assert_eq!(
            platform_repository_from_image(
                "external.example/nasiko/agent:1.0",
                "registry.example.com"
            ),
            None
        );
    }

    #[test]
    fn platform_repository_host_with_port() {
        assert_eq!(
            platform_repository_from_image("localhost:5000/nasiko/agent:1.0", "localhost:5000"),
            Some("nasiko/agent")
        );
        assert_eq!(
            platform_repository_from_image("localhost:5000/nasiko/agent:1.0", "localhost"),
            None
        );
    }

    #[test]
    fn platform_repository_digest_reference() {
        assert_eq!(
            platform_repository_from_image(
                "registry.example.com/u-abc123/agent@sha256:deadbeef",
                "registry.example.com"
            ),
            Some("u-abc123/agent")
        );
    }

    #[test]
    fn platform_repository_external_with_port() {
        assert_eq!(
            platform_repository_from_image(
                "external.io:5000/nasiko/agent:1.0",
                "registry.example.com"
            ),
            None
        );
    }
}
