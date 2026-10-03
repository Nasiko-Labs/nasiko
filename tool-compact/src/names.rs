//! Gateway tool-name aliasing for Nasiko MCP catalogs.
//!
//! Nasiko serves MCP tools as `{uuid_hex16}__{suffix}`. Those names often start
//! with a digit and fail compact-identifier rules. When the suffix is a safe,
//! unique compact identifier we alias it for the model-facing prompt and keep
//! an exact reverse map for execution.
//!
//! Non-gateway names are unchanged. Digit-leading names that are *not* the
//! documented gateway pattern are rejected (fail closed — never strip
//! arbitrarily). Suffix collisions get a deterministic disambiguated alias.

use std::collections::HashMap;

use crate::error::CompactError;

/// Length of the hex connector prefix Nasiko prepends to MCP tool names.
const GATEWAY_PREFIX_LEN: usize = 16;

/// Parse `{uuid_hex16}__{suffix}` — returns `(prefix, suffix)` when the name
/// matches the documented Nasiko MCP gateway pattern.
pub fn parse_gateway_tool_name(name: &str) -> Option<(&str, &str)> {
    let (prefix, suffix) = name.split_once("__")?;
    if prefix.len() != GATEWAY_PREFIX_LEN || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    if suffix.is_empty() {
        return None;
    }
    Some((prefix, suffix))
}

/// Compact-identifier rules used for model-facing tool names.
pub fn is_valid_compact_ident(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
}

#[derive(Debug, Clone)]
struct Proposal {
    original: String,
    compact: String,
    /// Present when `original` matched the gateway pattern.
    gateway: Option<(String /*prefix*/, String /*suffix*/)>,
}

/// Assign model-facing compact aliases for a tool catalog.
///
/// Returns `(compact_name, original_name)` pairs in input order.
///
/// Fail closed (`InvalidToolName`) when a name cannot be safely normalized or
/// when collisions cannot be resolved to unique valid aliases.
pub fn assign_aliases(original_names: &[String]) -> Result<Vec<(String, String)>, CompactError> {
    let mut proposals = Vec::with_capacity(original_names.len());
    for original in original_names {
        proposals.push(propose(original)?);
    }

    disambiguate(&mut proposals)?;

    // Final uniqueness + validity check — never emit ambiguous aliases.
    let mut seen: HashMap<&str, &str> = HashMap::new();
    for p in &proposals {
        if !is_valid_compact_ident(&p.compact) {
            return Err(CompactError::InvalidToolName(p.original.clone()));
        }
        if let Some(prev) = seen.insert(p.compact.as_str(), p.original.as_str()) {
            return Err(CompactError::InvalidToolName(format!(
                "alias collision: {} and {} → {}",
                prev, p.original, p.compact
            )));
        }
    }

    Ok(proposals
        .into_iter()
        .map(|p| (p.compact, p.original))
        .collect())
}

fn propose(original: &str) -> Result<Proposal, CompactError> {
    // Any `__` name must be the documented gateway pattern. Near-misses
    // (short hex, non-hex 16-char prefix, empty suffix, …) fail closed —
    // never treat them as plain identities.
    if original.contains("__") {
        let Some((prefix, suffix)) = parse_gateway_tool_name(original) else {
            return Err(CompactError::InvalidToolName(original.into()));
        };
        if !is_valid_compact_ident(suffix) {
            return Err(CompactError::InvalidToolName(original.into()));
        }
        return Ok(Proposal {
            original: original.into(),
            compact: suffix.into(),
            gateway: Some((prefix.into(), suffix.into())),
        });
    }

    // Non-gateway: existing rules (including rejecting digit-leading names).
    if !is_valid_compact_ident(original) {
        return Err(CompactError::InvalidToolName(original.into()));
    }
    Ok(Proposal {
        original: original.into(),
        compact: original.into(),
        gateway: None,
    })
}

/// When multiple tools propose the same compact name, rename gateway tools to
/// `{suffix}_{prefix}` (still letter-leading, shorter than the full original).
/// Plain (non-gateway) tools keep their name; if two plains collide, fail closed.
fn disambiguate(proposals: &mut [Proposal]) -> Result<(), CompactError> {
    // Loop: after renaming gateways, check again (rare multi-way collisions).
    for _ in 0..proposals.len().saturating_add(1) {
        let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, p) in proposals.iter().enumerate() {
            groups.entry(p.compact.clone()).or_default().push(i);
        }

        let mut changed = false;
        for (_alias, idxs) in groups {
            if idxs.len() < 2 {
                continue;
            }

            let gateway_idxs: Vec<usize> = idxs
                .iter()
                .copied()
                .filter(|&i| proposals[i].gateway.is_some())
                .collect();
            let plain_idxs: Vec<usize> = idxs
                .iter()
                .copied()
                .filter(|&i| proposals[i].gateway.is_none())
                .collect();

            if plain_idxs.len() > 1 {
                // Two non-gateway tools share a name — not a gateway issue.
                return Err(CompactError::InvalidToolName(
                    proposals[plain_idxs[0]].original.clone(),
                ));
            }

            if gateway_idxs.is_empty() {
                return Err(CompactError::InvalidToolName(
                    proposals[idxs[0]].original.clone(),
                ));
            }

            // Rename every gateway tool in the collision group.
            // If a plain tool also claims the short name, it keeps it.
            for i in gateway_idxs {
                let Some((prefix, suffix)) = proposals[i]
                    .gateway
                    .as_ref()
                    .map(|(p, s)| (p.clone(), s.clone()))
                else {
                    return Err(CompactError::InvalidToolName(proposals[i].original.clone()));
                };
                let disambig = format!("{suffix}_{prefix}");
                if !is_valid_compact_ident(&disambig) {
                    return Err(CompactError::InvalidToolName(proposals[i].original.clone()));
                }
                if proposals[i].compact != disambig {
                    proposals[i].compact = disambig;
                    changed = true;
                }
            }
        }

        if !changed {
            return Ok(());
        }
    }

    Err(CompactError::InvalidToolName(
        "unresolvable alias collision".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gateway_pattern() {
        let (p, s) = parse_gateway_tool_name("1234567890abcdef__create_calendar_event").unwrap();
        assert_eq!(p, "1234567890abcdef");
        assert_eq!(s, "create_calendar_event");
    }

    #[test]
    fn rejects_short_or_non_hex_prefix() {
        assert!(parse_gateway_tool_name("123__create").is_none());
        assert!(parse_gateway_tool_name("gggggggggggggggg__create").is_none());
        assert!(parse_gateway_tool_name("1234567890abcdef__").is_none());
    }

    #[test]
    fn unique_suffix_aliases() {
        let names = vec![
            "1234567890abcdef__create_calendar_event".into(),
            "1234567890abcdef__list_events".into(),
        ];
        let aliases = assign_aliases(&names).unwrap();
        assert_eq!(
            aliases[0],
            (
                "create_calendar_event".into(),
                "1234567890abcdef__create_calendar_event".into()
            )
        );
        assert_eq!(
            aliases[1],
            ("list_events".into(), "1234567890abcdef__list_events".into())
        );
    }

    #[test]
    fn collision_disambiguates_with_prefix() {
        let names = vec![
            "1111111111111111__search".into(),
            "2222222222222222__search".into(),
        ];
        let aliases = assign_aliases(&names).unwrap();
        assert_eq!(aliases[0].0, "search_1111111111111111");
        assert_eq!(aliases[1].0, "search_2222222222222222");
        assert_ne!(aliases[0].0, aliases[1].0);
        // Never collapse both to bare `search`.
        assert!(!aliases.iter().any(|(c, _)| c == "search"));
    }

    #[test]
    fn plain_name_unchanged() {
        let names = vec!["create_calendar_event".into(), "send_email".into()];
        let aliases = assign_aliases(&names).unwrap();
        assert_eq!(aliases[0].0, "create_calendar_event");
        assert_eq!(aliases[0].1, "create_calendar_event");
        assert_eq!(aliases[1].0, "send_email");
    }

    #[test]
    fn digit_leading_non_gateway_rejected() {
        let names = vec!["9bad_tool".into()];
        assert!(matches!(
            assign_aliases(&names),
            Err(CompactError::InvalidToolName(_))
        ));
    }

    #[test]
    fn plain_keeps_name_when_gateway_suffix_collides() {
        let names = vec!["search".into(), "aaaaaaaaaaaaaaaa__search".into()];
        let aliases = assign_aliases(&names).unwrap();
        assert_eq!(aliases[0], ("search".into(), "search".into()));
        assert_eq!(
            aliases[1],
            (
                "search_aaaaaaaaaaaaaaaa".into(),
                "aaaaaaaaaaaaaaaa__search".into()
            )
        );
    }

    #[test]
    fn gateway_digit_leading_suffix_rejected() {
        let names = vec!["aaaaaaaaaaaaaaaa__9bad".into()];
        assert!(matches!(
            assign_aliases(&names),
            Err(CompactError::InvalidToolName(_))
        ));
    }

    #[test]
    fn near_miss_gateway_shapes_fail_closed() {
        for name in [
            "aaaaaaaaaaaaaaaa__",         // empty suffix
            "zzzzzzzzzzzzzzzz__create",   // non-hex 16-char prefix
            "12345678__create",           // short hex
            "abcdefabcdefabcd__bad-name", // invalid suffix chars
        ] {
            assert!(
                matches!(
                    assign_aliases(&[name.into()]),
                    Err(CompactError::InvalidToolName(_))
                ),
                "expected fail closed for {name}"
            );
        }
    }
}
