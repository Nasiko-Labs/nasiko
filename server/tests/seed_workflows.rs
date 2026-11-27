/// Integration tests for seeded workflows (`SEED_WORKFLOWS`) and the public
/// workflow access they rely on.
///
/// Requires: `docker compose -f docker-compose.infra.yml up -d`
/// Run with:  `cargo test --test seed_workflows -- --test-threads=1`
mod common;

use nasiko_server::seed_workflows::{SeedOutcome, TEMPLATES, seed_workflows};
use serde_json::Value;
use serial_test::serial;
use uuid::Uuid;

const BOTH: [&str; 2] = ["inbox-morning-brief", "monthly-work-recap"];
const INBOX_AGENTS: [&str; 2] = ["scout", "distiller"];
const ALL_AGENTS: [&str; 3] = ["scout", "distiller", "herald"];

// ─── Helpers ───────────────────────────────────────────────────────────────

async fn insert_user(server: &common::TestServer, superuser: bool) -> Uuid {
    let id = Uuid::new_v4();
    let short = &id.to_string()[..8];
    sqlx::query("INSERT INTO users (id, username, email, is_superuser) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(format!("user_{short}"))
        .bind(format!("user_{short}@test.example"))
        .bind(superuser)
        .execute(&server.db)
        .await
        .expect("insert user");
    id
}

/// A seed agent row, as `SEED_AGENTS` registers it: the superuser's, public.
async fn insert_seed_agent(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, url, is_public, metadata)
         VALUES ($1, $2, $3, 'http://fake-agent.local/a2a', true, '{\"seed\": true}')
         RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(format!("localhost:5000/nasiko/{name}:1.0.0"))
    .fetch_one(&server.db)
    .await
    .expect("insert seed agent")
}

/// A server with a superuser and the given seed agents.
async fn setup(agents: &[&str]) -> (common::TestServer, Uuid) {
    let server = common::TestServer::start().await;
    let admin = insert_user(&server, true).await;
    for name in agents {
        insert_seed_agent(&server, admin, name).await;
    }
    (server, admin)
}

fn outcomes(report: &[(String, SeedOutcome)]) -> Vec<SeedOutcome> {
    report.iter().map(|(_, outcome)| outcome.clone()).collect()
}

async fn seeded_row(server: &common::TestServer, template: &str) -> (Uuid, Value) {
    let (id, maf_json): (Uuid, Value) =
        sqlx::query_as("SELECT id, maf_json FROM mafs WHERE seed_template = $1")
            .bind(template)
            .fetch_one(&server.db)
            .await
            .expect("seeded row");
    (id, maf_json)
}

fn step_agent_ids(maf_json: &Value) -> Vec<String> {
    maf_json["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|s| s["agent_id"].as_str().expect("agent_id").to_owned())
        .collect()
}

fn as_user(rb: reqwest::RequestBuilder, user: Uuid) -> reqwest::RequestBuilder {
    let id = user.to_string();
    common::as_member(rb, &id, &format!("user_{}", &id[..8]))
}

fn as_admin(rb: reqwest::RequestBuilder, user: Uuid) -> reqwest::RequestBuilder {
    let id = user.to_string();
    common::as_superuser(rb, &id, &format!("user_{}", &id[..8]))
}

async fn json(res: reqwest::Response) -> Value {
    res.json().await.expect("json body")
}

/// The items of a list response: `{"data": {"data": [...], "total": n}}`.
fn listed(body: &Value) -> &Vec<Value> {
    body["data"]["data"].as_array().expect("list items")
}

// ─── Seeding ───────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn a_restart_installs_nothing_twice() {
    let (server, admin) = setup(&ALL_AGENTS).await;

    let first = seed_workflows(&server.state, &BOTH).await;
    assert_eq!(
        outcomes(&first),
        [SeedOutcome::Installed, SeedOutcome::Installed]
    );
    let second = seed_workflows(&server.state, &BOTH).await;
    assert_eq!(
        outcomes(&second),
        [SeedOutcome::Unchanged, SeedOutcome::Unchanged]
    );

    let rows: Vec<(Uuid, bool, String)> = sqlx::query_as(
        "SELECT user_id, is_public, status FROM mafs WHERE seed_template IS NOT NULL",
    )
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "one row per template, however often seeded");
    for (owner, is_public, status) in rows {
        assert_eq!(owner, admin);
        assert!(is_public);
        assert_eq!(status, "active");
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn each_step_runs_on_its_seed_agent() {
    let (server, admin) = setup(&ALL_AGENTS).await;
    seed_workflows(&server.state, &["monthly-work-recap"]).await;

    let (_, maf_json) = seeded_row(&server, "monthly-work-recap").await;
    let names: Vec<&str> = maf_json["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["agent_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["scout", "distiller", "herald", "herald"]);

    let seeded = |name: &'static str| {
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM agents WHERE name = $1 AND owner_id = $2")
            .bind(name)
            .bind(admin)
            .fetch_one(&server.db)
    };
    let scout: Uuid = seeded("scout").await.unwrap();
    let herald: Uuid = seeded("herald").await.unwrap();
    let ids = step_agent_ids(&maf_json);
    assert_eq!(ids[0], scout.to_string());
    // One seed agent may run several steps.
    assert_eq!(ids[2], herald.to_string());
    assert_eq!(ids[3], herald.to_string());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_template_whose_agent_is_not_seeded_is_skipped_alone() {
    let (server, _) = setup(&INBOX_AGENTS).await;

    let report = seed_workflows(&server.state, &BOTH).await;
    assert_eq!(report[0].1, SeedOutcome::Installed);
    assert!(
        matches!(&report[1].1, SeedOutcome::Skipped(reason) if reason.contains("herald")),
        "Monthly Work Recap needs herald, which is not seeded: {:?}",
        report[1].1
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_unknown_template_is_skipped() {
    let (server, _) = setup(&ALL_AGENTS).await;

    let report = seed_workflows(&server.state, &["no-such-template"]).await;
    assert!(matches!(report[0].1, SeedOutcome::Skipped(_)));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mafs")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn nothing_is_seeded_without_a_superuser() {
    let server = common::TestServer::start().await;

    assert!(seed_workflows(&server.state, &BOTH).await.is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_newer_template_version_updates_the_same_row() {
    let (server, _) = setup(&ALL_AGENTS).await;
    seed_workflows(&server.state, &["monthly-work-recap"]).await;
    let (id, _) = seeded_row(&server, "monthly-work-recap").await;

    // As if the row was installed by an older template version.
    sqlx::query("UPDATE mafs SET seed_version = 0, name = 'old name' WHERE id = $1")
        .bind(id)
        .execute(&server.db)
        .await
        .unwrap();

    let report = seed_workflows(&server.state, &["monthly-work-recap"]).await;
    assert_eq!(outcomes(&report), [SeedOutcome::Updated]);
    let (name, version): (String, i32) =
        sqlx::query_as("SELECT name, seed_version FROM mafs WHERE id = $1")
            .bind(id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    let template = TEMPLATES
        .iter()
        .find(|t| t.id == "monthly-work-recap")
        .unwrap();
    assert_eq!(name, template.name);
    assert_eq!(version, template.version);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_reseeded_agent_is_repointed() {
    let (server, admin) = setup(&ALL_AGENTS).await;
    seed_workflows(&server.state, &["monthly-work-recap"]).await;
    let (id, _) = seeded_row(&server, "monthly-work-recap").await;

    // Scout is deleted and seeded again, under a new id.
    sqlx::query("UPDATE agents SET deleted_at = now() WHERE name = 'scout'")
        .execute(&server.db)
        .await
        .unwrap();
    let new_scout = insert_seed_agent(&server, admin, "scout").await;

    let report = seed_workflows(&server.state, &["monthly-work-recap"]).await;
    assert_eq!(outcomes(&report), [SeedOutcome::Updated]);
    let (same_id, maf_json) = seeded_row(&server, "monthly-work-recap").await;
    assert_eq!(same_id, id, "updated in place, not reinstalled");
    assert_eq!(step_agent_ids(&maf_json)[0], new_scout.to_string());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_deleted_seed_workflow_stays_deleted() {
    let (server, _) = setup(&ALL_AGENTS).await;
    seed_workflows(&server.state, &["inbox-morning-brief"]).await;
    sqlx::query("UPDATE mafs SET status = 'deleted' WHERE seed_template = 'inbox-morning-brief'")
        .execute(&server.db)
        .await
        .unwrap();

    let report = seed_workflows(&server.state, &["inbox-morning-brief"]).await;
    assert_eq!(outcomes(&report), [SeedOutcome::Unchanged]);
    let status: String =
        sqlx::query_scalar("SELECT status FROM mafs WHERE seed_template = 'inbox-morning-brief'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(status, "deleted");

    server.cleanup().await;
}

// ─── Public workflow access ────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn every_user_sees_and_runs_a_public_workflow_as_themselves() {
    let (server, admin) = setup(&ALL_AGENTS).await;
    seed_workflows(&server.state, &BOTH).await;
    let (id, _) = seeded_row(&server, "monthly-work-recap").await;
    let member = insert_user(&server, false).await;

    // Listed, and fetched, by a user who does not own it.
    let list = json(
        as_user(server.client.get(server.url("/api/maf/workflows")), member)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let items = listed(&list);
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|w| w["is_public"] == true));

    let res = as_user(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/{id}"))),
        member,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // Run by them, recorded under them.
    let res = as_user(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{id}/run"))),
        member,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202);
    let run = json(res).await;
    assert_eq!(run["data"]["execution_count"], 1);
    let exec_id: Uuid = run["data"]["execution_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let runner: Uuid = sqlx::query_scalar("SELECT user_id FROM maf_executions WHERE id = $1")
        .bind(exec_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(runner, member);

    // Their run is theirs: the owner's view counts none of it.
    let as_owner = json(
        as_admin(
            server
                .client
                .get(server.url(&format!("/api/maf/workflow/{id}"))),
            admin,
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(as_owner["data"]["execution_count"], 0);
    let owner_runs = json(
        as_admin(
            server
                .client
                .get(server.url(&format!("/api/maf/workflow/{id}/executions"))),
            admin,
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    let owner_runs = listed(&owner_runs);
    assert!(owner_runs.is_empty(), "the owner sees no one else's runs");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn only_the_owner_edits_or_deletes_a_public_workflow() {
    let (server, _) = setup(&ALL_AGENTS).await;
    seed_workflows(&server.state, &BOTH).await;
    let (id, _) = seeded_row(&server, "monthly-work-recap").await;
    let member = insert_user(&server, false).await;

    let res = as_user(
        server
            .client
            .put(server.url(&format!("/api/maf/workflow/{id}")))
            .json(&serde_json::json!({ "name": "mine now" })),
        member,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    let res = as_user(
        server
            .client
            .delete(server.url(&format!("/api/maf/workflow/{id}"))),
        member,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    let status: String = sqlx::query_scalar("SELECT status FROM mafs WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(status, "active");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn public_workflows_stay_out_of_other_users_drafts() {
    let (server, _) = setup(&ALL_AGENTS).await;
    seed_workflows(&server.state, &BOTH).await;
    let member = insert_user(&server, false).await;

    let drafts = json(
        as_user(
            server.client.get(server.url("/api/maf/workflow/drafts")),
            member,
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    let items = listed(&drafts);
    assert!(items.is_empty());

    server.cleanup().await;
}
