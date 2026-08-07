use std::io::IsTerminal;

use anyhow::{Result, bail};
use dialoguer::{Confirm, Input};
use nasiko_utils::version::parse_plain_version;

/// Used when there's no version to start from (first-ever deploy).
const FIRST_VERSION: &str = "0.1.0";

/// The version to use, plus whether it's okay to overwrite an existing one.
/// `overwrite` must be forwarded to the server as `allow_overwrite`.
#[derive(Debug)]
pub struct VersionDecision {
    pub version: String,
    pub overwrite: bool,
}

/// The CLI flags (`--version`, `--overwrite`, `--yes`) that control
/// [`resolve_deploy_version`].
#[derive(Debug, Clone, Copy, Default)]
pub struct VersionFlags<'a> {
    pub version: Option<&'a str>,
    pub overwrite: bool,
    pub yes: bool,
}

/// What we already know about an agent, so [`resolve_deploy_version`] can
/// decide the next version.
///
/// - `card_version`: the version in `AgentCard.json`, if any.
/// - `current_deployed_version`: the version currently live, if any.
/// - `used_versions`: every version this agent has ever had.
///
/// Pass `None`/`&[]` when a value doesn't apply (e.g. `nasiko build` never
/// checks the server, so it has no deployed version or history).
#[derive(Debug, Clone, Copy)]
pub struct VersionContext<'a> {
    pub card_version: Option<&'a str>,
    pub current_deployed_version: Option<&'a str>,
    pub used_versions: &'a [String],
}

/// Picks the version to build/push/deploy under.
///
/// Rules, in order:
/// 1. `--version` wins if given (must be a plain `x.y.z`, e.g. `1.2.3`).
/// 2. Otherwise, the AgentCard version is the priority — used as-is if it's
///    a valid, unused `x.y.z`.
/// 3. If the card version is invalid or already used (e.g. `"latest"`, or a
///    version this agent has had before), it doesn't count as a real
///    choice: we suggest the next unused patch bump and ask the user to
///    confirm or pick another version. If they type in that same old
///    version, they can either overwrite it (replace its content) or back
///    out and run `nasiko rollback` to it instead.
///
/// In CI (no terminal), the same choice must come from an explicit
/// `--version`, or `--yes` to accept the suggested bump — never silently.
pub fn resolve_deploy_version(
    context: VersionContext<'_>,
    flags: VersionFlags,
) -> Result<VersionDecision> {
    let VersionContext {
        card_version,
        current_deployed_version,
        used_versions,
    } = context;

    if let Some(v) = flags.version {
        if parse_plain_version(v).is_none() {
            bail!("--version {v} is not a valid version — expected x.y.z, e.g. 1.2.3");
        }
        if is_used(v, used_versions) {
            if !flags.overwrite {
                bail!(
                    "version {v} already exists in this agent's history. To go back to it, run \
                     `nasiko rollback --version {v}` instead — or re-run with --overwrite to \
                     replace its content."
                );
            }
            return Ok(VersionDecision {
                version: v.to_string(),
                overwrite: true,
            });
        }
        return Ok(VersionDecision {
            version: v.to_string(),
            overwrite: false,
        });
    }

    let interactive = std::io::stdin().is_terminal();
    // A card version only counts if it's valid AND not already used before.
    let card_version =
        card_version.filter(|v| parse_plain_version(v).is_some() && !is_used(v, used_versions));

    match (card_version, current_deployed_version) {
        (None, deployed) => {
            let suggested = suggest_unused_version(deployed, used_versions);
            if !interactive && !flags.yes {
                bail!(
                    "AgentCard.json has no usable \"version\" field (missing, invalid, or \
                     already used for this agent), and this isn't an interactive terminal. \
                     Pass --version explicitly, add a fresh x.y.z \"version\" field to \
                     AgentCard.json, or re-run with --yes to use the suggested {suggested}."
                );
            }
            prompt_for_version(None, &suggested, used_versions, flags.yes)
        }
        // Redeploying the same version that's already live — always ask
        // (or require --yes in CI) instead of silently reusing it.
        (Some(cv), Some(dv)) if cv == dv => {
            let suggested = suggest_unused_version(Some(cv), used_versions);
            if !interactive && !flags.yes {
                bail!(
                    "version {cv} is already deployed for this agent. Bump the \
                     version in AgentCard.json, pass --version, or re-run with \
                     --yes to auto-bump to {suggested}."
                );
            }
            prompt_for_version(Some(cv), &suggested, used_versions, flags.yes)
        }
        // A fresh, unused version is already in the card — use it, no prompt.
        (Some(cv), _) => Ok(VersionDecision {
            version: cv.to_string(),
            overwrite: false,
        }),
    }
}

fn is_used(v: &str, used_versions: &[String]) -> bool {
    used_versions.iter().any(|u| u == v)
}

/// Bumps `base`'s patch number (or starts from [`FIRST_VERSION`]) until it
/// finds a version not already in `used_versions`.
fn suggest_unused_version(base: Option<&str>, used_versions: &[String]) -> String {
    let mut candidate = base
        .filter(|v| parse_plain_version(v).is_some())
        .map(bump_patch)
        .unwrap_or_else(|| FIRST_VERSION.to_string());
    while is_used(&candidate, used_versions) {
        candidate = bump_patch(&candidate);
    }
    candidate
}

fn prompt_for_version(
    current: Option<&str>,
    suggested: &str,
    used_versions: &[String],
    assume_yes: bool,
) -> Result<VersionDecision> {
    if assume_yes {
        if let Some(cv) = current {
            eprintln!("  ! version {cv} already deployed — auto-bumping to {suggested} (--yes)");
        }
        // `suggested` is always unused, so no need to ask about overwriting.
        return Ok(VersionDecision {
            version: suggested.to_string(),
            overwrite: false,
        });
    }
    let prompt = match current {
        Some(cv) => format!("Version {cv} is already deployed. Enter a new version"),
        None => "No usable version set in AgentCard.json. Enter a version".to_string(),
    };
    loop {
        let input: String = Input::new()
            .with_prompt(&prompt)
            .default(suggested.to_string())
            .validate_with(|s: &String| -> Result<(), &'static str> { validate_version_input(s) })
            .interact_text()?;
        let input = input.trim().to_string();

        if is_used(&input, used_versions) {
            let confirmed = Confirm::new()
                .with_prompt(format!(
                    "version {input} already exists in this agent's history — overwrite its \
                     content anyway? (or choose No to enter a different version)"
                ))
                .default(false)
                .interact()?;
            if confirmed {
                return Ok(VersionDecision {
                    version: input,
                    overwrite: true,
                });
            }
            // Declined — ask again instead of failing.
            continue;
        }

        return Ok(VersionDecision {
            version: input,
            overwrite: false,
        });
    }
}

/// Checks that the typed input is a valid `x.y.z` version (syntax only —
/// the "already used?" check happens separately in [`prompt_for_version`]).
fn validate_version_input(s: &str) -> Result<(), &'static str> {
    if parse_plain_version(s).is_some() {
        Ok(())
    } else {
        Err("must be a version in x.y.z format, e.g. 1.2.3")
    }
}

fn bump_patch(v: &str) -> String {
    parse_plain_version(v)
        .map(|mut sv| {
            sv.patch += 1;
            sv.to_string()
        })
        .unwrap_or_else(|| FIRST_VERSION.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn used(versions: &[&str]) -> Vec<String> {
        versions.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn validate_version_input_rejects_latest() {
        assert!(validate_version_input("latest").is_err());
    }

    #[test]
    fn validate_version_input_rejects_empty() {
        assert!(validate_version_input("").is_err());
        assert!(validate_version_input("   ").is_err());
    }

    #[test]
    fn validate_version_input_rejects_free_form_text() {
        assert!(validate_version_input("bjjnjn").is_err());
        assert!(validate_version_input("v1.0.0").is_err());
        assert!(validate_version_input("1.0").is_err());
    }

    #[test]
    fn validate_version_input_accepts_a_real_version() {
        assert!(validate_version_input("1.2.3").is_ok());
    }

    #[test]
    fn validate_version_input_rejects_pre_release_and_build_metadata() {
        assert!(validate_version_input("1.2.3-beta.1").is_err());
        assert!(validate_version_input("1.2.3+build.5").is_err());
        // Valid full SemVer, but not a plain x.y.z version.
        assert!(validate_version_input("0.10238.2893-let").is_err());
    }

    #[test]
    fn suggest_unused_version_skips_past_used_patch_numbers() {
        // 0.1.1 and 0.1.2 already used -> bumping from 0.1.0 must land on 0.1.3.
        let history = used(&["0.1.0", "0.1.1", "0.1.2"]);
        let suggested = suggest_unused_version(Some("0.1.0"), &history);
        assert_eq!(suggested, "0.1.3");
    }

    fn context<'a>(
        card_version: Option<&'a str>,
        current_deployed_version: Option<&'a str>,
        used_versions: &'a [String],
    ) -> VersionContext<'a> {
        VersionContext {
            card_version,
            current_deployed_version,
            used_versions,
        }
    }

    fn flags(version: Option<&str>, overwrite: bool, yes: bool) -> VersionFlags<'_> {
        VersionFlags {
            version,
            overwrite,
            yes,
        }
    }

    #[test]
    fn explicit_flag_wins_over_everything() {
        let d = resolve_deploy_version(
            context(Some("1.0.0"), Some("1.0.0"), &used(&[])),
            flags(Some("9.9.9"), false, false),
        )
        .unwrap();
        assert_eq!(d.version, "9.9.9");
        assert!(!d.overwrite);
    }

    #[test]
    fn explicit_flag_rejects_non_plain_semver() {
        let err = resolve_deploy_version(
            context(None, None, &used(&[])),
            flags(Some("latest"), false, false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("expected x.y.z"));
    }

    #[test]
    fn explicit_flag_rejects_a_reused_version_without_overwrite_flag() {
        let history = used(&["1.0.0", "1.1.0"]);
        let err = resolve_deploy_version(
            context(None, None, &history),
            flags(Some("1.0.0"), false, false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("nasiko rollback"));
        assert!(err.to_string().contains("--overwrite"));
    }

    #[test]
    fn explicit_flag_with_overwrite_flag_accepts_a_reused_version() {
        let history = used(&["1.0.0", "1.1.0"]);
        let d = resolve_deploy_version(
            context(None, None, &history),
            flags(Some("1.0.0"), true, false),
        )
        .unwrap();
        assert_eq!(d.version, "1.0.0");
        assert!(d.overwrite);
    }

    #[test]
    fn new_card_version_is_trusted_without_prompting() {
        // current_deployed_version differs from card_version -> no prompt path.
        let d = resolve_deploy_version(
            context(Some("1.2.0"), Some("1.1.0"), &used(&[])),
            flags(None, false, false),
        )
        .unwrap();
        assert_eq!(d.version, "1.2.0");
        assert!(!d.overwrite);
    }

    #[test]
    fn card_version_reused_from_history_is_treated_as_missing() {
        // "0.1.3" looks valid but was already used before -> don't trust it.
        let history = used(&["0.1.0", "0.1.1", "0.1.2", "0.1.3", "2.0.0"]);
        let err = resolve_deploy_version(
            context(Some("0.1.3"), Some("2.0.0"), &history),
            flags(None, false, false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no usable"));
    }

    #[test]
    fn non_plain_semver_card_version_is_treated_as_missing() {
        let err = resolve_deploy_version(
            context(Some("latest"), Some("1.0.0"), &used(&[])),
            flags(None, false, false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no usable"));
    }

    #[test]
    fn brand_new_agent_trusts_card_version() {
        let d = resolve_deploy_version(
            context(Some("0.1.0"), None, &used(&[])),
            flags(None, false, false),
        )
        .unwrap();
        assert_eq!(d.version, "0.1.0");
    }

    #[test]
    fn same_version_non_interactive_without_yes_errors() {
        // stdin in test runs is not a terminal, so this exercises the non-interactive branch.
        let err = resolve_deploy_version(
            context(Some("1.0.0"), Some("1.0.0"), &used(&[])),
            flags(None, false, false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("already deployed"));
    }

    #[test]
    fn same_version_with_yes_auto_bumps_patch() {
        let d = resolve_deploy_version(
            context(Some("1.0.0"), Some("1.0.0"), &used(&[])),
            flags(None, false, true),
        )
        .unwrap();
        assert_eq!(d.version, "1.0.1");
        assert!(!d.overwrite);
    }

    #[test]
    fn same_version_with_yes_skips_past_used_bumps() {
        let history = used(&["1.0.0", "1.0.1", "1.0.2"]);
        let d = resolve_deploy_version(
            context(Some("1.0.0"), Some("1.0.0"), &history),
            flags(None, false, true),
        )
        .unwrap();
        assert_eq!(d.version, "1.0.3");
    }

    #[test]
    fn missing_card_version_non_interactive_without_yes_errors() {
        let err =
            resolve_deploy_version(context(None, None, &used(&[])), flags(None, false, false))
                .unwrap_err();
        assert!(err.to_string().contains("no usable"));
    }

    #[test]
    fn missing_card_version_and_no_deployed_version_with_yes_uses_first_version() {
        let d = resolve_deploy_version(context(None, None, &used(&[])), flags(None, false, true))
            .unwrap();
        assert_eq!(d.version, FIRST_VERSION);
    }

    #[test]
    fn missing_card_version_with_deployed_version_and_yes_bumps_patch() {
        let d = resolve_deploy_version(
            context(None, Some("2.0.0"), &used(&[])),
            flags(None, false, true),
        )
        .unwrap();
        assert_eq!(d.version, "2.0.1");
    }

    #[test]
    fn missing_card_version_with_non_semver_deployed_version_falls_back_to_first_version() {
        // Can't patch-bump a garbage version like "bjjnjn" -> fall back instead.
        let d = resolve_deploy_version(
            context(None, Some("bjjnjn"), &used(&[])),
            flags(None, false, true),
        )
        .unwrap();
        assert_eq!(d.version, FIRST_VERSION);
    }
}
