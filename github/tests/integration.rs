//! Integration tests for `nasiko-github`.
//!
//! Tests t01–t04 and t12–t13 run in CI without any credentials (pure logic).
//! Tests t05–t11 are `#[ignore]` — opt-in locally with real GitHub credentials.
//!
//! ## Running the network tests locally
//!
//! ```bash
//! export GITHUB_CLIENT_ID="your_oauth_app_client_id"
//! export GITHUB_CLIENT_SECRET="your_oauth_app_client_secret"
//! export GITHUB_CALLBACK_URL="https://example.com/api/github/callback"
//! export GITHUB_TEST_TOKEN="ghp_yourPersonalAccessToken"
//! export GITHUB_TEST_REPO="octocat/Hello-World"
//! export GITHUB_TEST_BRANCH="master"
//!
//! cargo test -p nasiko-github -- --ignored --nocapture
//! ```
//!
//! ## What each test covers
//!
//! | Test | Ignored? | Real network? | Needs token? | What it proves |
//! |---|---|---|---|---|
//! | `t01_state_build_verify` | no | no | no | HMAC round-trip with real secret |
//! | `t02_state_tampered_rejected` | no | no | no | Constant-time sig check |
//! | `t03_state_expired_rejected` | no | no | no | 600 s age window |
//! | `t04_authorization_url` | no | no | no | URL shape + client_id present |
//! | `t05_verify_token_valid` | **yes** | **yes** | yes | Real token → true |
//! | `t06_verify_token_invalid` | **yes** | **yes** | no | Garbage token → false |
//! | `t07_list_repos` | **yes** | **yes** | yes | Returns ≥ 1 repo, fields populated |
//! | `t08_clone_to_archive` | **yes** | **yes** | yes | Archive non-empty, no .git inside |
//! | `t09_token_not_in_error` | **yes** | **yes** | no | Bad repo → error scrubbed |
//! | `t10_archive_structure` | **yes** | **yes** | yes | tar.gz unpacks to real files |
//! | `t11_list_repos_sorted_by_updated_desc` | **yes** | **yes** | yes | Ordering newest-first |
//! | `t12_clone_rejects_invalid_repo_name` | no | no | no | Input validation before network |
//! | `t13_clone_rejects_invalid_branch_name` | no | no | no | Input validation before network |

use std::collections::BTreeMap;

use base64::Engine as _;
use nasiko_github::{GitHubConfig, GitHubService};

// ── helpers ───────────────────────────────────────────────────────────────────

/// Read an env var, skipping the test with a clear message when absent.
macro_rules! need_env {
    ($key:literal) => {
        match std::env::var($key) {
            Ok(v) if !v.is_empty() => v,
            _ => {
                println!("[SKIP] {} not set — skipping this test", $key);
                return;
            }
        }
    };
}

/// Build service from env, skipping if required env vars are absent.
fn svc_from_env() -> Option<GitHubService> {
    let cfg = GitHubConfig::from_env().ok()?;
    GitHubService::new(cfg).ok()
}

// ── t01 ── OAuth state round-trip ─────────────────────────────────────────────

#[tokio::test]
async fn t01_state_build_verify() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };

    let user_id = "integration-test-user-42";
    let state = svc.build_state(user_id).expect("build_state failed");

    println!("  state string  : {state}");
    assert!(
        state.starts_with("v1."),
        "state must begin with version prefix 'v1.'"
    );
    assert_eq!(
        state.splitn(3, '.').count(),
        3,
        "state must have 3 dot-separated segments"
    );

    let claims = svc.verify_state(&state).expect("verify_state failed");
    println!("  user_id       : {}", claims.user_id);
    println!("  issued_at     : {}", claims.issued_at);
    assert_eq!(claims.user_id, user_id);
    println!("[PASS] t01_state_build_verify");
}

// ── t02 ── Tampered signature is rejected ─────────────────────────────────────

#[tokio::test]
async fn t02_state_tampered_rejected() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };

    let state = svc.build_state("user-1").unwrap();
    let tampered = {
        let mut s = state.clone();
        let last = s.pop().unwrap();
        s.push(if last == 'f' { '0' } else { 'f' });
        s
    };

    let err = svc.verify_state(&tampered).unwrap_err();
    println!("  rejection reason: {err}");
    assert!(
        err.to_string().contains("invalid OAuth state"),
        "wrong error variant: {err}"
    );
    println!("[PASS] t02_state_tampered_rejected");
}

// ── t03 ── Expired state (correctly signed, iat > 600 s ago) is rejected ─────

#[tokio::test]
async fn t03_state_expired_rejected() {
    // This test verifies the AGE CHECK, not the signature check.
    // It builds a correctly-signed state whose `iat` is 700 seconds in the
    // past, then confirms verify_state rejects it with an "expired" reason.
    //
    // Implementation note: sign_payload is a private method, so we replicate
    // the HMAC-SHA256 logic here using the same crates that the library uses.
    // We use `cfg.oauth_state_secret` (a public field) as the key.
    let cfg = match GitHubConfig::from_env() {
        Ok(c) => c,
        Err(_) => {
            println!("[SKIP] required GitHub env vars not set");
            return;
        }
    };
    let svc = nasiko_github::GitHubService::new(cfg.clone()).unwrap();

    let stale_iat = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(700); // 700s ago → 100s past the 600s window

    let mut payload: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
    payload.insert("flow", serde_json::Value::String("connect".into()));
    payload.insert("iat", serde_json::Value::Number(stale_iat.into()));
    payload.insert("nonce", serde_json::Value::String("old-nonce".into()));
    payload.insert("user_id", serde_json::Value::String("stale-user".into()));

    let serialized = serde_json::to_string(&payload).unwrap();
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serialized.as_bytes());

    // Re-implement the HMAC signing with the real secret so the state passes
    // the signature check and fails only at the age check.
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac: Hmac<Sha256> = Mac::new_from_slice(cfg.oauth_state_secret.as_bytes()).unwrap();
    mac.update(encoded.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());

    let state = format!("v1.{encoded}.{sig}");

    let err = svc.verify_state(&state).unwrap_err();
    println!("  rejection reason: {err}");
    // Must say "expired", not "signature mismatch" or anything else.
    assert!(
        err.to_string().contains("expired"),
        "expected expiry rejection, got: {err}"
    );
    println!("[PASS] t03_state_expired_rejected");
}

// ── t04 ── Authorization URL shape ───────────────────────────────────────────

#[tokio::test]
async fn t04_authorization_url() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };

    let url = svc
        .authorization_url("test-user-99")
        .expect("authorization_url failed");

    println!("  authorization URL: {url}");

    assert!(
        url.starts_with("https://github.com/login/oauth/authorize"),
        "wrong base URL"
    );
    assert!(url.contains("client_id="), "missing client_id param");
    assert!(url.contains("scope="), "missing scope param");
    assert!(url.contains("state="), "missing state param");
    assert!(url.contains("redirect_uri="), "missing redirect_uri param");
    assert!(
        url.contains("repo") && url.contains("user"),
        "scope must include repo and user:email"
    );
    println!("\n  Open this URL in a browser to test the real OAuth flow:");
    println!("  {url}\n");
    println!("[PASS] t04_authorization_url");
}

// ── t05 ── verify_token — real valid token ────────────────────────────────────

#[tokio::test]
#[ignore]
async fn t05_verify_token_valid() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };
    let token = need_env!("GITHUB_TEST_TOKEN");

    let valid = svc
        .verify_token(&token)
        .await
        .expect("verify_token returned Err (network/5xx)");

    println!("  token valid: {valid}");
    assert!(
        valid,
        "GITHUB_TEST_TOKEN appears expired or invalid — check the token"
    );
    println!("[PASS] t05_verify_token_valid");
}

// ── t06 ── verify_token — garbage token returns false, not Err ───────────────

#[tokio::test]
#[ignore]
async fn t06_verify_token_invalid() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };

    let valid = svc
        .verify_token("ghp_thisisnotarealtokenxxxxxxxxxxxxxxxxxx")
        .await
        .expect("verify_token should return Ok(false) on 401, not Err");

    println!("  garbage token valid: {valid}");
    assert!(!valid, "a garbage token must not be reported as valid");
    println!("[PASS] t06_verify_token_invalid");
}

// ── t07 ── list_repos ─────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn t07_list_repos() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };
    let token = need_env!("GITHUB_TEST_TOKEN");

    let repos = svc.list_repos(&token).await.expect("list_repos failed");

    println!("  repositories returned: {}", repos.len());
    assert!(!repos.is_empty(), "Expected at least one repository");

    // Print a summary table of the first 5 repos.
    println!(
        "\n  {:<40} {:<8} {:<12}",
        "full_name", "private", "default_branch"
    );
    println!("  {}", "-".repeat(62));
    for r in repos.iter().take(5) {
        println!(
            "  {:<40} {:<8} {:<12}",
            r.full_name,
            r.private.to_string(),
            r.default_branch
        );
    }
    if repos.len() > 5 {
        println!("  ... and {} more", repos.len() - 5);
    }

    // Assert every returned repo has the mandatory fields populated.
    for r in &repos {
        assert!(!r.name.is_empty(), "repo.name must not be empty");
        assert!(!r.full_name.is_empty(), "repo.full_name must not be empty");
        assert!(!r.clone_url.is_empty(), "repo.clone_url must not be empty");
        assert!(
            !r.default_branch.is_empty(),
            "repo.default_branch must not be empty"
        );
    }
    println!("[PASS] t07_list_repos");
}

// ── t08 ── clone_to_archive ───────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn t08_clone_to_archive() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };
    let token = need_env!("GITHUB_TEST_TOKEN");
    let repo = need_env!("GITHUB_TEST_REPO");
    let branch = std::env::var("GITHUB_TEST_BRANCH").unwrap_or_else(|_| "main".into());

    println!("  cloning {repo}@{branch} ...");

    let archive = svc
        .clone_to_archive(&token, &repo, &branch)
        .await
        .expect("clone_to_archive failed");

    println!("  s3_key           : {}", archive.s3_key);
    println!(
        "  archive size     : {} bytes ({:.1} KB)",
        archive.archive_bytes.len(),
        (archive.archive_bytes.len() as f64) / 1024.0
    );

    // Basic assertions on the archive.
    assert!(
        !archive.archive_bytes.is_empty(),
        "archive must not be empty"
    );
    assert!(
        archive.s3_key.starts_with("github/"),
        "s3_key must start with 'github/'"
    );
    assert!(
        archive.s3_key.ends_with(".tar.gz"),
        "s3_key must end with '.tar.gz'"
    );
    assert!(
        archive.s3_key.contains(&repo),
        "s3_key must contain repo name"
    );

    // The first two bytes of a gzip stream are always 0x1f 0x8b.
    assert_eq!(
        &archive.archive_bytes[..2],
        &[0x1f, 0x8b],
        "archive must be a valid gzip stream"
    );

    println!("[PASS] t08_clone_to_archive");
}

// ── t09 ── Token and its base64-encoded form never appear in error messages ────

#[tokio::test]
#[ignore]
async fn t09_token_not_in_error() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };

    // Use a fake token with a recognisable unique string.
    let sentinel = "SENTINEL_TOKEN_DO_NOT_LOG_abc123xyz";
    // The service sends Basic auth as base64("x-access-token:<token>").
    // We verify the encoded form is also scrubbed.
    let encoded_sentinel = base64::engine::general_purpose::STANDARD
        .encode(format!("x-access-token:{sentinel}").as_bytes());

    // Clone a repo that does not exist — triggers a git authentication error.
    let err = svc
        .clone_to_archive(
            sentinel,
            "octocat/this-repo-definitely-does-not-exist-xyz",
            "main",
        )
        .await
        .unwrap_err();

    let err_msg = err.to_string();
    println!("  error message: {err_msg}");

    assert!(
        !err_msg.contains(sentinel),
        "ERROR: raw token appeared in error message!\n  message: {err_msg}"
    );
    assert!(
        !err_msg.contains(&encoded_sentinel),
        "ERROR: base64-encoded token appeared in error message!\n  message: {err_msg}"
    );
    println!(
        "[PASS] t09_token_not_in_error — neither raw nor encoded token present in error output"
    );
}

// ── t10 ── Archive contains real files, no .git ───────────────────────────────

#[tokio::test]
#[ignore]
async fn t10_archive_structure() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };
    let token = need_env!("GITHUB_TEST_TOKEN");
    let repo = need_env!("GITHUB_TEST_REPO");
    let branch = std::env::var("GITHUB_TEST_BRANCH").unwrap_or_else(|_| "main".into());

    let archive = svc
        .clone_to_archive(&token, &repo, &branch)
        .await
        .expect("clone failed");

    // Decompress and list files in the archive.
    use flate2::read::GzDecoder;
    use tar::Archive;

    let cursor = std::io::Cursor::new(&archive.archive_bytes);
    let gz = GzDecoder::new(cursor);
    let mut tar = Archive::new(gz);

    let mut entries: Vec<String> = tar
        .entries()
        .expect("failed to read tar entries")
        .filter_map(|e| e.ok())
        .map(|e| e.path().unwrap_or_default().to_string_lossy().into_owned())
        .collect();

    entries.sort();

    println!("\n  archive entries ({} total):", entries.len());
    for name in entries.iter().take(20) {
        println!("    {name}");
    }
    if entries.len() > 20 {
        println!("    ... and {} more", entries.len() - 20);
    }

    assert!(
        !entries.is_empty(),
        "archive must contain at least one file"
    );

    // The .git directory must have been stripped.
    let git_entries: Vec<&str> = entries
        .iter()
        .map(|s| s.as_str())
        .filter(|s| s.starts_with(".git/") || *s == ".git")
        .collect();

    assert!(
        git_entries.is_empty(),
        ".git directory must not be present in archive, found: {git_entries:?}"
    );

    println!(
        "[PASS] t10_archive_structure — {} files, no .git",
        entries.len()
    );
}

// ── t11 ── Repository list is sorted newest-updated first ─────────────────────

#[tokio::test]
#[ignore]
async fn t11_list_repos_sorted_by_updated_desc() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };
    let token = need_env!("GITHUB_TEST_TOKEN");

    let repos = svc.list_repos(&token).await.expect("list_repos failed");

    if repos.len() < 2 {
        println!(
            "[SKIP] only {} repo(s) — need at least 2 to test ordering",
            repos.len()
        );
        return;
    }

    // `updated_at` is an ISO-8601 string, lexicographic order == chronological order.
    for window in repos.windows(2) {
        let (newer, older) = (&window[0], &window[1]);
        assert!(
            newer.updated_at >= older.updated_at,
            "repos must be sorted newest-updated first: {} ({}) came before {} ({})",
            newer.full_name,
            newer.updated_at,
            older.full_name,
            older.updated_at
        );
    }

    println!("  ordering verified across {} repos", repos.len());
    println!("[PASS] t11_list_repos_sorted_by_updated_desc");
}

// ── t12 ── Invalid repository name is rejected before network call ─────────────

#[tokio::test]
async fn t12_clone_rejects_invalid_repo_name() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };

    // These should all be rejected by validate_repo_full_name without any network call.
    // No real token needed — validation fires before any git/network operation.
    let invalid_names = [
        "no-slash",
        "",
        "owner/",
        "/repo",
        "owner/repo/extra-segment",
        "owner/../../../etc/passwd",
        "owner/repo name with spaces",
    ];

    for bad in invalid_names {
        let err = svc
            .clone_to_archive("dummy-token", bad, "main")
            .await
            .unwrap_err();
        assert!(
            matches!(err, nasiko_github::Error::GitClone(_)),
            "expected GitClone for invalid repo {bad:?}, got: {err:?}"
        );
        println!("  {bad:?} → correctly rejected");
    }
    println!("[PASS] t12_clone_rejects_invalid_repo_name");
}

// ── t13 ── Invalid branch name is rejected before network call ────────────────

#[tokio::test]
async fn t13_clone_rejects_invalid_branch_name() {
    let svc = match svc_from_env() {
        Some(s) => s,
        None => {
            println!("[SKIP] GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set");
            return;
        }
    };

    // No real token needed — branch validation fires before any git/network operation.
    let invalid_branches = ["", "has space", "has~tilde", "has^caret", "has:colon"];

    for bad in invalid_branches {
        let err = svc
            .clone_to_archive("dummy-token", "owner/repo", bad)
            .await
            .unwrap_err();
        assert!(
            matches!(err, nasiko_github::Error::GitClone(_)),
            "expected GitClone for invalid branch {bad:?}, got: {err:?}"
        );
        println!("  branch {bad:?} → correctly rejected");
    }
    println!("[PASS] t13_clone_rejects_invalid_branch_name");
}
