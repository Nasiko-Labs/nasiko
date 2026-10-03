use nasiko_auth::Identity;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// Reusable Postgres test helper for `nasiko-auth` integration tests.
///
/// Sets up an isolated test database, runs all schema migrations,
/// and provides utility methods for provisioning users and agents.
pub struct TestDb {
    pub pool: PgPool,
    pub db_name: String,
    admin_pool: PgPool,
}

impl TestDb {
    /// Connect to Postgres admin URL, create an isolated database, and run migrations.
    pub async fn create() -> Self {
        let admin_url = std::env::var("TEST_PG_URL")
            .or_else(|_| std::env::var("TEST_PG_ADMIN_URL"))
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".into());

        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .expect("Failed to connect to Postgres admin URL. Ensure docker-compose.infra.yml is running.");

        let db_name = format!("nasiko_test_auth_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin_pool)
            .await
            .expect("Failed to create test database");

        let base = admin_url
            .rsplit_once('/')
            .map(|(b, _)| b)
            .unwrap_or(&admin_url);

        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&format!("{base}/{db_name}"))
            .await
            .expect("Failed to connect to created test database");

        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("Failed to run migrations on test database");

        Self {
            pool,
            db_name,
            admin_pool,
        }
    }

    /// Insert a test user into the `users` table and return its ID and `Identity`.
    pub async fn create_user(&self, username: &str) -> (Uuid, Identity) {
        let user_id = Uuid::new_v4();
        let email = format!("{username}-{}@test.local", Uuid::new_v4().simple());

        sqlx::query(
            "INSERT INTO users (id, username, email) VALUES ($1, $2, $3)",
        )
        .bind(user_id)
        .bind(username)
        .bind(&email)
        .execute(&self.pool)
        .await
        .expect("Failed to insert test user");

        (
            user_id,
            Identity {
                user_id: user_id.to_string(),
                username: username.to_string(),
                is_superuser: false,
            },
        )
    }

    /// Insert a test agent into the `agents` table and return its ID.
    pub async fn create_agent(&self, name: &str, owner_id: Uuid) -> Uuid {
        let agent_id = Uuid::new_v4();

        sqlx::query(
            "INSERT INTO agents (id, name, owner_id) VALUES ($1, $2, $3)",
        )
        .bind(agent_id)
        .bind(name)
        .bind(owner_id)
        .execute(&self.pool)
        .await
        .expect("Failed to insert test agent");

        agent_id
    }

    /// Terminate active connections, drop the test database, and close pools.
    pub async fn cleanup(self) {
        let _ = sqlx::query(&format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
            self.db_name
        ))
        .execute(&self.admin_pool)
        .await;

        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
            self.db_name
        ))
        .execute(&self.admin_pool)
        .await;

        self.pool.close().await;
        self.admin_pool.close().await;
    }
}
