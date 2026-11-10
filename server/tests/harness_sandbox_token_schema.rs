//! Schema guard for migrations `0082_harness_sandbox_token.sql` (`chat_sessions.sandbox_token_jti`
//! text and `sandbox_token_enc` bytea) and `0088_chat_sessions_sandbox_key.sql`
//! (`chat_sessions.sandbox_key` text and its partial index): the columns exist, are nullable, and
//! default to NULL.
//!
//! Run with infra up (`just infra`):
//!   `cargo test -p nasiko-server --test harness_sandbox_token_schema`
//! Override the admin DSN with TEST_PG_ADMIN_URL if Postgres isn't on :5432.

use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn admin_url() -> String {
    std::env::var("TEST_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".to_string())
}

#[tokio::test]
async fn sandbox_token_jti_column_is_in_place() {
    let admin_dsn = admin_url();
    let admin = match PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_dsn)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SKIP: cannot reach Postgres at {admin_dsn}: {e}");
            return;
        }
    };
    let db_name = format!("nasiko_test_sbxtok_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin)
        .await
        .expect("create test database");
    let base = admin_dsn
        .rsplit_once('/')
        .map(|(b, _)| b)
        .unwrap_or(&admin_dsn);
    let result = run_checks(&format!("{base}/{db_name}")).await;
    let _ = sqlx::query(&format!(
        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
    ))
    .execute(&admin)
    .await;
    result.expect("0080 sandbox token column is in place");
}

async fn run_checks(db_url: &str) -> Result<(), String> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(db_url)
        .await
        .map_err(|e| format!("connect test db: {e}"))?;
    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;

    for (column, want) in [
        ("sandbox_token_jti", "text"),
        ("sandbox_token_enc", "bytea"),
    ] {
        let (data_type, nullable): (String, String) = sqlx::query_as(
            "SELECT data_type, is_nullable FROM information_schema.columns \
             WHERE table_name = 'chat_sessions' AND column_name = $1",
        )
        .bind(column)
        .fetch_one(&pool)
        .await
        .map_err(|e| format!("{column} missing: {e}"))?;
        if (data_type.as_str(), nullable.as_str()) != (want, "YES") {
            return Err(format!(
                "{column}: unexpected shape {data_type} nullable={nullable}"
            ));
        }
    }

    // 0086: the sandbox key column and its partial index.
    let (data_type, nullable): (String, String) = sqlx::query_as(
        "SELECT data_type, is_nullable FROM information_schema.columns \
         WHERE table_name = 'chat_sessions' AND column_name = 'sandbox_key'",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| format!("sandbox_key missing: {e}"))?;
    if (data_type.as_str(), nullable.as_str()) != ("text", "YES") {
        return Err(format!(
            "sandbox_key: unexpected shape {data_type} nullable={nullable}"
        ));
    }
    let indexed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_indexes WHERE indexname = 'idx_chat_sessions_sandbox_key'",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| format!("read indexes: {e}"))?;
    if indexed != 1 {
        return Err("idx_chat_sessions_sandbox_key is missing".into());
    }

    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email) VALUES ('sbxtok', 'sbxtok@test.local') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| format!("insert user: {e}"))?;
    sqlx::query("INSERT INTO chat_sessions (session_id, user_id, title) VALUES ('s1', $1, 't')")
        .bind(user)
        .execute(&pool)
        .await
        .map_err(|e| format!("insert session: {e}"))?;
    let (jti, enc): (Option<String>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT sandbox_token_jti, sandbox_token_enc FROM chat_sessions WHERE session_id = 's1'",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| format!("read token columns: {e}"))?;
    if jti.is_some() || enc.is_some() {
        return Err("the sandbox token columns should default to NULL".into());
    }
    Ok(())
}
