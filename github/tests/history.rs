//! The history reads replay evals use, against a mock GitHub API (no network, no token).

use mockito::Matcher;
use nasiko_github::{GitHubConfig, GitHubService};

fn service(api: &str) -> GitHubService {
    let cfg = GitHubConfig {
        client_id: "id".into(),
        client_secret: "secret".into(),
        callback_url: "https://example.com/cb".into(),
        oauth_state_secret: "state-secret-0123456789abcdef!!".into(),
        central_callback_url: None,
        clone_timeout_secs: 30,
        clone_max_size_bytes: 1024 * 1024,
    };
    GitHubService::with_base_urls(cfg, "https://github.invalid", api).expect("service")
}

fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().expect("timestamp")
}

#[tokio::test]
async fn commits_in_a_window_follow_pages() {
    let mut server = mockito::Server::new_async().await;
    let first: Vec<_> = (0..100)
        .map(|i| serde_json::json!({"sha": format!("a{i}")}))
        .collect();
    let page1 = server
        .mock("GET", "/repos/o/r/commits")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("sha".into(), "main".into()),
            Matcher::UrlEncoded("page".into(), "1".into()),
            Matcher::UrlEncoded("since".into(), "2026-11-01T10:00:00+00:00".into()),
        ]))
        .match_header("authorization", "Bearer tok")
        .with_body(serde_json::to_string(&first).unwrap())
        .create_async()
        .await;
    let page2 = server
        .mock("GET", "/repos/o/r/commits")
        .match_query(Matcher::UrlEncoded("page".into(), "2".into()))
        .with_body(r#"[{"sha":"b0"}]"#)
        .create_async()
        .await;
    let svc = service(&server.url());
    let shas = svc
        .commits_between(
            "tok",
            "o/r",
            Some("main"),
            at("2026-11-01T10:00:00Z"),
            at("2026-11-01T12:00:00Z"),
        )
        .await
        .expect("lists");
    assert_eq!(shas.len(), 101);
    assert_eq!(shas.last().map(String::as_str), Some("b0"));
    page1.assert_async().await;
    page2.assert_async().await;
}

#[tokio::test]
async fn a_gone_branch_has_no_commits() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/repos/o/r/commits")
        .match_query(Matcher::Any)
        .with_status(404)
        .create_async()
        .await;
    let shas = service(&server.url())
        .commits_between(
            "tok",
            "o/r",
            Some("gone"),
            at("2026-11-01T10:00:00Z"),
            at("2026-11-01T12:00:00Z"),
        )
        .await
        .expect("empty, not an error");
    assert!(shas.is_empty());
}

#[tokio::test]
async fn reads_a_commit_a_diff_files_and_the_root() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/repos/o/r/commits/c1")
        .with_body(
            r#"{"sha":"c1","parents":[{"sha":"p1"}],
                "commit":{"message":"fix\n\nCo-Authored-By: Claude","committer":{"date":"2026-11-01T10:30:00Z"}},
                "files":[{"filename":"src/lib.rs","status":"modified","patch":"@@ -1 +1 @@\n-a\n+b"}]}"#,
        )
        .create_async()
        .await;
    server
        .mock("GET", "/repos/o/r/compare/p1...c1")
        .match_header("accept", "application/vnd.github.diff")
        .with_body("diff --git a/src/lib.rs b/src/lib.rs\n")
        .create_async()
        .await;
    server
        .mock("GET", "/repos/o/r/contents/src/my%20lib.rs")
        .match_query(Matcher::UrlEncoded("ref".into(), "p1".into()))
        .match_header("accept", "application/vnd.github.raw")
        .with_body("fn a() {}\n")
        .create_async()
        .await;
    server
        .mock("GET", "/repos/o/r/contents/missing.rs")
        .match_query(Matcher::Any)
        .with_status(404)
        .create_async()
        .await;
    server
        .mock("GET", "/repos/o/r/contents")
        .match_query(Matcher::UrlEncoded("ref".into(), "p1".into()))
        .with_body(r#"[{"name":"Cargo.toml","type":"file"},{"name":"src","type":"dir"}]"#)
        .create_async()
        .await;
    server
        .mock("GET", "/repos/o/r")
        .with_body(r#"{"full_name":"o/r","default_branch":"main","private":true}"#)
        .create_async()
        .await;
    server
        .mock("GET", "/repos/o/hidden")
        .with_status(404)
        .create_async()
        .await;

    let svc = service(&server.url());
    let commit = svc.commit("tok", "o/r", "c1").await.expect("commit");
    assert_eq!(commit.parents[0].sha, "p1");
    assert_eq!(commit.added_lines(), vec!["b"]);
    assert!(
        svc.compare_diff("tok", "o/r", "p1", "c1")
            .await
            .expect("diff")
            .starts_with("diff --git")
    );
    assert_eq!(
        svc.file_at("tok", "o/r", "p1", "src/my lib.rs")
            .await
            .expect("file"),
        Some("fn a() {}\n".into())
    );
    assert_eq!(
        svc.file_at("tok", "o/r", "p1", "missing.rs")
            .await
            .expect("none"),
        None
    );
    assert_eq!(
        svc.root_files("tok", "o/r", "p1").await.expect("root"),
        vec!["Cargo.toml", "src"]
    );
    let repo = svc
        .repo("tok", "o/r")
        .await
        .expect("repo")
        .expect("readable");
    assert_eq!(repo.default_branch, "main");
    assert!(svc.repo("tok", "o/hidden").await.expect("answer").is_none());
}
