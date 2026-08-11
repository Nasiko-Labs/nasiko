/// Parses `v` as a plain `x.y.z` version (e.g. `1.2.3`) — no pre-release or
/// build suffixes like `1.2.3-beta.1`, even though those are valid SemVer.
/// Rejecting them keeps free-form/junk strings (like `"latest"`) out of
/// agent version history. Shared by the CLI and server so the rule stays
/// the same on both sides.
pub fn parse_plain_version(v: &str) -> Option<semver::Version> {
    let parsed = semver::Version::parse(v).ok()?;
    if parsed.pre.is_empty() && parsed.build.is_empty() {
        Some(parsed)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_x_y_z() {
        assert!(parse_plain_version("1.2.3").is_some());
    }

    #[test]
    fn rejects_latest() {
        assert!(parse_plain_version("latest").is_none());
    }

    #[test]
    fn rejects_pre_release_and_build_metadata() {
        assert!(parse_plain_version("1.2.3-beta.1").is_none());
        assert!(parse_plain_version("1.2.3+build.5").is_none());
        // Valid full SemVer, but not a plain x.y.z version.
        assert!(parse_plain_version("0.10238.2893-let").is_none());
    }

    #[test]
    fn rejects_surrounding_whitespace() {
        assert!(parse_plain_version(" 1.2.3").is_none());
        assert!(parse_plain_version("1.2.3 ").is_none());
        assert!(parse_plain_version(" 1.2.3 ").is_none());
    }
}
