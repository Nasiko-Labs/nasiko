use std::io::IsTerminal;

use anyhow::{Result, bail};
use dialoguer::Input;
use nasiko_utils::version::parse_plain_version;

/// Used when there's no version to start from (first-ever deploy).
const FIRST_VERSION: &str = "0.1.0";

/// The version to use for this build/push/deploy.
#[derive(Debug)]
pub struct VersionDecision {
    pub version: String,
}

/// The CLI flags (`--version`, `--yes`) that control [`resolve_deploy_version`].
#[derive(Debug, Clone, Copy, Default)]
pub struct VersionFlags<'a> {
    pub version: Option<&'a str>,
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
/// 1. `--version` wins if given (must be plain `x.y.z`).
/// 2. Otherwise use the AgentCard version, if it's valid and unused.
/// 3. Otherwise suggest the next unused patch bump and ask the user.
///
/// Versions are immutable: a collision is always a hard error (or a
/// re-prompt) with a suggested next version — never an overwrite. Use
/// `nasiko rollback` to go back to an old version.
///
/// Non-interactively, the version must come from `--version` or `--yes`.
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
            let suggested = suggest_unused_version(Some(v), used_versions);
            bail!(
                "version {v} already exists in this agent's history and versions are \
                 immutable. Suggested next version: {suggested}. To go back to it, run \
                 `nasiko rollback --version {v}` instead."
            );
        }
        return Ok(VersionDecision {
            version: v.to_string(),
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
        // Card version is unused but not ahead of what's deployed (e.g. a
        // stale AgentCard.json) — don't trust it, treat like no card version.
        (Some(cv), Some(dv)) if !is_ahead(cv, dv) => {
            let suggested = suggest_unused_version(Some(dv), used_versions);
            if !interactive && !flags.yes {
                bail!(
                    "AgentCard.json's version ({cv}) doesn't reflect what's actually deployed \
                     ({dv}) and isn't ahead of it. Bump the version in AgentCard.json, pass \
                     --version, or re-run with --yes to use the suggested {suggested}."
                );
            }
            prompt_for_version(Some(dv), &suggested, used_versions, flags.yes)
        }
        // A fresh version, ahead of (or with nothing yet) deployed — use it,
        // no prompt.
        (Some(cv), _) => Ok(VersionDecision {
            version: cv.to_string(),
        }),
    }
}

/// Resolves the version for an already-built `image:tag`, as opposed to a
/// source directory. Shared by `deploy_from_image` and `push_from_image`.
///
/// An explicit `:tag` is treated like `--version`: used as-is, or a hard
/// error naming the exact artifact if it collides with history. A bare
/// `image` (no `:tag`) falls back to the normal suggest/prompt logic.
///
/// `command` is the verb to print in the suggested next steps ("deploy" or
/// "push").
pub fn resolve_image_deploy_version(
    image: &str,
    image_tag_version: &str,
    flags: VersionFlags,
    current_deployed_version: Option<&str>,
    used_versions: &[String],
    command: &str,
) -> Result<VersionDecision> {
    if crate::util::image_has_explicit_tag(image) && is_used(image_tag_version, used_versions) {
        let (name, _) = crate::util::parse_image_name_and_tag(image);
        let suggested = suggest_unused_version(Some(image_tag_version), used_versions);
        bail!(
            "{image} already exists and versions are immutable.\n\n\
             Suggested next version: {suggested}\n\n\
             Build the new version first:\n  nasiko build --version {suggested}\n\n\
             Then {command} the exact artifact:\n  nasiko {command} {name}:{suggested}"
        );
    }
    let explicit_tag = crate::util::image_has_explicit_tag(image).then_some(image_tag_version);
    if let (Some(v), Some(t)) = (flags.version, explicit_tag)
        && v != t
    {
        let (name, _) = crate::util::parse_image_name_and_tag(image);
        bail!(
            "{image} names version {t}, but --version {v} was also given — they must match. \
             Drop --version, or {command} {name}:{v} instead."
        );
    }
    let flags = VersionFlags {
        version: flags.version.or(explicit_tag),
        ..flags
    };
    let context = VersionContext {
        card_version: None,
        current_deployed_version,
        used_versions,
    };
    resolve_deploy_version(context, flags)
}

/// Whether `candidate` is a real version bump past `baseline`. Uncomparable
/// (non-semver) inputs default to `true` so we don't second-guess them.
fn is_ahead(candidate: &str, baseline: &str) -> bool {
    match (
        parse_plain_version(candidate),
        parse_plain_version(baseline),
    ) {
        (Some(c), Some(b)) => c > b,
        _ => true,
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
        // `suggested` is always unused, so nothing further to check.
        return Ok(VersionDecision {
            version: suggested.to_string(),
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
            // Versions are immutable — no overwrite option, just tell them
            // and ask again.
            println!(
                "  ! version {input} already exists in this agent's history and versions are \
                 immutable — pick a different version."
            );
            continue;
        }

        return Ok(VersionDecision { version: input });
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
    fn is_ahead_true_for_a_real_bump() {
        assert!(is_ahead("1.2.0", "1.1.0"));
    }

    #[test]
    fn is_ahead_false_for_equal_or_behind() {
        assert!(!is_ahead("1.0.0", "1.0.0"));
        assert!(!is_ahead("0.1.0", "1.0.0"));
        assert!(!is_ahead("0.9.9", "1.0.0"));
    }

    #[test]
    fn is_ahead_true_when_either_side_is_not_plain_semver() {
        // Can't compare -> don't second-guess an otherwise-valid card version.
        assert!(is_ahead("1.0.0", "latest"));
        assert!(is_ahead("latest", "1.0.0"));
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

    fn flags(version: Option<&str>, yes: bool) -> VersionFlags<'_> {
        VersionFlags { version, yes }
    }

    #[test]
    fn explicit_flag_wins_over_everything() {
        let d = resolve_deploy_version(
            context(Some("1.0.0"), Some("1.0.0"), &used(&[])),
            flags(Some("9.9.9"), false),
        )
        .unwrap();
        assert_eq!(d.version, "9.9.9");
    }

    #[test]
    fn explicit_flag_rejects_non_plain_semver() {
        let err = resolve_deploy_version(
            context(None, None, &used(&[])),
            flags(Some("latest"), false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("expected x.y.z"));
    }

    #[test]
    fn explicit_flag_rejects_a_reused_version_with_no_overwrite_option() {
        let history = used(&["1.0.0", "1.1.0"]);
        let err =
            resolve_deploy_version(context(None, None, &history), flags(Some("1.0.0"), false))
                .unwrap_err();
        assert!(err.to_string().contains("immutable"));
        assert!(err.to_string().contains("nasiko rollback"));
        assert!(!err.to_string().contains("overwrite"));
    }

    #[test]
    fn new_card_version_is_trusted_without_prompting() {
        // current_deployed_version differs from card_version, but the card
        // version is AHEAD of it (a real bump) -> no prompt path.
        let d = resolve_deploy_version(
            context(Some("1.2.0"), Some("1.1.0"), &used(&[])),
            flags(None, false),
        )
        .unwrap();
        assert_eq!(d.version, "1.2.0");
    }

    #[test]
    fn stale_card_version_behind_deployed_is_not_trusted_and_suggests_next_from_deployed() {
        // Card says 0.1.0 but platform is at 1.0.0 -> bump from 1.0.0, not the stale value.
        let d = resolve_deploy_version(
            context(Some("0.1.0"), Some("1.0.0"), &used(&[])),
            flags(None, true),
        )
        .unwrap();
        assert_eq!(d.version, "1.0.1");
    }

    #[test]
    fn stale_card_version_non_interactive_without_yes_errors() {
        let err = resolve_deploy_version(
            context(Some("0.1.0"), Some("1.0.0"), &used(&[])),
            flags(None, false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("1.0.0"));
        assert!(err.to_string().contains("--yes"));
    }

    #[test]
    fn card_version_equal_to_a_lower_major_is_still_stale_not_ahead() {
        // "0.9.9" < "1.0.0" numerically despite looking close — must not be
        // trusted just because it differs from the deployed value.
        let d = resolve_deploy_version(
            context(Some("0.9.9"), Some("1.0.0"), &used(&[])),
            flags(None, true),
        )
        .unwrap();
        assert_eq!(d.version, "1.0.1");
    }

    #[test]
    fn card_version_reused_from_history_is_treated_as_missing() {
        // "0.1.3" looks valid but was already used before -> don't trust it.
        let history = used(&["0.1.0", "0.1.1", "0.1.2", "0.1.3", "2.0.0"]);
        let err = resolve_deploy_version(
            context(Some("0.1.3"), Some("2.0.0"), &history),
            flags(None, false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no usable"));
    }

    #[test]
    fn non_plain_semver_card_version_is_treated_as_missing() {
        let err = resolve_deploy_version(
            context(Some("latest"), Some("1.0.0"), &used(&[])),
            flags(None, false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no usable"));
    }

    #[test]
    fn brand_new_agent_trusts_card_version() {
        let d =
            resolve_deploy_version(context(Some("0.1.0"), None, &used(&[])), flags(None, false))
                .unwrap();
        assert_eq!(d.version, "0.1.0");
    }

    #[test]
    fn same_version_non_interactive_without_yes_errors() {
        // stdin in test runs is not a terminal, so this exercises the non-interactive branch.
        let err = resolve_deploy_version(
            context(Some("1.0.0"), Some("1.0.0"), &used(&[])),
            flags(None, false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("already deployed"));
    }

    #[test]
    fn same_version_with_yes_auto_bumps_patch() {
        let d = resolve_deploy_version(
            context(Some("1.0.0"), Some("1.0.0"), &used(&[])),
            flags(None, true),
        )
        .unwrap();
        assert_eq!(d.version, "1.0.1");
    }

    #[test]
    fn same_version_with_yes_skips_past_used_bumps() {
        let history = used(&["1.0.0", "1.0.1", "1.0.2"]);
        let d = resolve_deploy_version(
            context(Some("1.0.0"), Some("1.0.0"), &history),
            flags(None, true),
        )
        .unwrap();
        assert_eq!(d.version, "1.0.3");
    }

    #[test]
    fn missing_card_version_non_interactive_without_yes_errors() {
        let err = resolve_deploy_version(context(None, None, &used(&[])), flags(None, false))
            .unwrap_err();
        assert!(err.to_string().contains("no usable"));
    }

    #[test]
    fn missing_card_version_and_no_deployed_version_with_yes_uses_first_version() {
        let d = resolve_deploy_version(context(None, None, &used(&[])), flags(None, true)).unwrap();
        assert_eq!(d.version, FIRST_VERSION);
    }

    #[test]
    fn missing_card_version_with_deployed_version_and_yes_bumps_patch() {
        let d = resolve_deploy_version(context(None, Some("2.0.0"), &used(&[])), flags(None, true))
            .unwrap();
        assert_eq!(d.version, "2.0.1");
    }

    #[test]
    fn missing_card_version_with_non_semver_deployed_version_falls_back_to_first_version() {
        // Can't patch-bump a garbage version like "bjjnjn" -> fall back instead.
        let d =
            resolve_deploy_version(context(None, Some("bjjnjn"), &used(&[])), flags(None, true))
                .unwrap();
        assert_eq!(d.version, FIRST_VERSION);
    }

    // ─── resolve_image_deploy_version ────────────────────────────────────────

    #[test]
    fn image_tag_and_matching_version_flag_is_fine() {
        let d = resolve_image_deploy_version(
            "agent:1.0.1",
            "1.0.1",
            flags(Some("1.0.1"), false),
            None,
            &used(&[]),
            "deploy",
        )
        .unwrap();
        assert_eq!(d.version, "1.0.1");
    }

    #[test]
    fn image_tag_and_mismatched_version_flag_is_rejected() {
        // `nasiko deploy agent:1.0.1 --version 2.0.0` must not silently ship
        // the bytes tagged 1.0.1 under the label 2.0.0 — that's exactly the
        // artifact/version decoupling immutable versions are meant to prevent.
        let err = resolve_image_deploy_version(
            "agent:1.0.1",
            "1.0.1",
            flags(Some("2.0.0"), false),
            None,
            &used(&[]),
            "deploy",
        )
        .unwrap_err();
        assert!(err.to_string().contains("1.0.1"));
        assert!(err.to_string().contains("2.0.0"));
        assert!(err.to_string().contains("must match"));
    }

    #[test]
    fn bare_image_with_version_flag_uses_the_flag() {
        // No explicit tag on the image -> nothing to conflict with.
        let d = resolve_image_deploy_version(
            "agent",
            "latest",
            flags(Some("2.0.0"), false),
            None,
            &used(&[]),
            "deploy",
        )
        .unwrap();
        assert_eq!(d.version, "2.0.0");
    }
}
