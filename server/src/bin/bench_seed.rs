//! Seeds users + agents for control-plane load testing, then writes a JSON
//! manifest (pre-signed JWTs + agent IDs) so a load generator never needs to
//! call `/api/auth/login` (bcrypt-cost-12) in its hot path.
//!
//! Every row this tool creates is prefixed `bench_` (users) / `bench-agent-`
//! (agents) so `--reset` can safely delete only its own seed data.
//!
//! Usage:
//!   JWT_SECRET=... DATABASE_URL=... cargo run --bin bench_seed -- \
//!     --users 500 --agents 50 --sim-agent-url http://localhost:8000 --ee
use clap::Parser;
use nasiko_auth::Identity;
use nasiko_auth::jwt::encode_jwt;
use rand::seq::IndexedRandom;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(about = "Seed users + agents for control-plane benchmarking")]
struct Args {
    /// Number of simulated users to create.
    #[arg(long, default_value_t = 100)]
    users: u32,

    /// Number of simulated agents to create.
    #[arg(long, default_value_t = 20)]
    agents: u32,

    /// Endpoint every seeded agent's `url` column points at — the shared
    /// `simulated-agent` process (see `oss/agents/simulated-agent`).
    #[arg(long, default_value = "http://localhost:8000")]
    sim_agent_url: String,

    /// Also seed EE org hierarchy (departments/teams) and distribute seeded
    /// users across hierarchy roles. Requires an EE database (ee/migrations
    /// applied) — inserts into `departments`/`teams` and sets
    /// `users.department_id`/`team_id`.
    #[arg(long)]
    ee: bool,

    /// Number of departments/teams to spread seeded users across in --ee mode.
    #[arg(long, default_value_t = 5)]
    tenants: u32,

    /// Delete this tool's previously seeded rows (by `bench_`/`bench-agent-`
    /// prefix) before seeding.
    #[arg(long)]
    reset: bool,

    /// Where to write the manifest (tokens + agent IDs) for the load generator.
    #[arg(long, default_value = "bench-manifest.json")]
    manifest_out: String,

    /// JWT validity — long enough to outlast a benchmark run.
    #[arg(long, default_value_t = 24 * 60 * 60)]
    token_expiry_secs: u64,
}

#[derive(Serialize)]
struct ManifestUser {
    id: Uuid,
    username: String,
    token: String,
}

#[derive(Serialize)]
struct ManifestAgent {
    id: Uuid,
    name: String,
    url: String,
}

#[derive(Serialize)]
struct Manifest {
    users: Vec<ManifestUser>,
    agents: Vec<ManifestAgent>,
}

const EE_ROLES: &[&str] = &["team_member", "team_lead", "department_manager", "member"];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let args = Args::parse();

    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    if args.reset {
        reset(&pool, args.ee).await?;
    }

    let dept_team_ids = if args.ee {
        Some(seed_ee_hierarchy(&pool, args.tenants).await?)
    } else {
        None
    };

    let users = seed_users(&pool, &args, &jwt_secret, dept_team_ids.as_deref()).await?;
    let owner_id = users.first().map(|u| u.id).expect("--users must be >= 1");
    let agents = seed_agents(&pool, &args, owner_id).await?;

    let manifest = Manifest { users, agents };
    std::fs::write(&args.manifest_out, serde_json::to_string_pretty(&manifest)?)?;
    println!(
        "seeded {} users, {} agents -> {}",
        manifest.users.len(),
        manifest.agents.len(),
        args.manifest_out
    );
    Ok(())
}

/// Delete this tool's previously seeded rows, identified by the `bench_`
/// username prefix and `bench-agent-` name prefix — never touches real data.
async fn reset(pool: &sqlx::PgPool, ee: bool) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM agents WHERE name LIKE 'bench-agent-%'")
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM users WHERE username LIKE 'bench\\_%' ESCAPE '\\'")
        .execute(pool)
        .await?;
    if ee {
        sqlx::query("DELETE FROM teams WHERE name LIKE 'bench-team-%'")
            .execute(pool)
            .await?;
        sqlx::query("DELETE FROM departments WHERE name LIKE 'bench-dept-%'")
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// (department_id, team_id) pairs, one per tenant, for round-robin assignment.
async fn seed_ee_hierarchy(pool: &sqlx::PgPool, tenants: u32) -> anyhow::Result<Vec<(Uuid, Uuid)>> {
    let mut pairs = Vec::with_capacity(tenants as usize);
    for i in 0..tenants {
        let dept_id: Uuid =
            sqlx::query_scalar("INSERT INTO departments (name) VALUES ($1) RETURNING id")
                .bind(format!("bench-dept-{i}"))
                .fetch_one(pool)
                .await?;

        let team_id: Uuid = sqlx::query_scalar(
            "INSERT INTO teams (name, department_id) VALUES ($1, $2) RETURNING id",
        )
        .bind(format!("bench-team-{i}"))
        .bind(dept_id)
        .fetch_one(pool)
        .await?;

        pairs.push((dept_id, team_id));
    }
    Ok(pairs)
}

async fn seed_users(
    pool: &sqlx::PgPool,
    args: &Args,
    jwt_secret: &str,
    dept_team_ids: Option<&[(Uuid, Uuid)]>,
) -> anyhow::Result<Vec<ManifestUser>> {
    let mut rng = rand::rng();
    let mut out = Vec::with_capacity(args.users as usize);

    for i in 0..args.users {
        let username = format!("bench_user_{i}");
        let email = format!("{username}@bench.local");

        let (role, department_id, team_id): (&str, Option<Uuid>, Option<Uuid>) = match dept_team_ids
        {
            Some(pairs) if !pairs.is_empty() => {
                let (dept, team) = pairs[i as usize % pairs.len()];
                let role = EE_ROLES.choose(&mut rng).copied().unwrap_or("member");
                (role, Some(dept), Some(team))
            }
            _ => ("member", None, None),
        };

        let user_id: Uuid = if dept_team_ids.is_some() {
            sqlx::query_scalar(
                "INSERT INTO users (username, email, role, department_id, team_id)
                 VALUES ($1, $2, $3::user_role, $4, $5) RETURNING id",
            )
            .bind(&username)
            .bind(&email)
            .bind(role)
            .bind(department_id)
            .bind(team_id)
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_scalar(
                "INSERT INTO users (username, email, role) VALUES ($1, $2, 'member') RETURNING id",
            )
            .bind(&username)
            .bind(&email)
            .fetch_one(pool)
            .await?
        };

        let identity = Identity {
            user_id: user_id.to_string(),
            username: username.clone(),
            is_superuser: false,
        };
        let token = encode_jwt(jwt_secret, args.token_expiry_secs, &identity)?;

        out.push(ManifestUser {
            id: user_id,
            username,
            token,
        });
    }

    Ok(out)
}

async fn seed_agents(
    pool: &sqlx::PgPool,
    args: &Args,
    owner_id: Uuid,
) -> anyhow::Result<Vec<ManifestAgent>> {
    let mut out = Vec::with_capacity(args.agents as usize);

    for i in 0..args.agents {
        let name = format!("bench-agent-{i}");
        let agent_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agents (name, owner_id, url, status, is_public)
             VALUES ($1, $2, $3, 'running', true) RETURNING id",
        )
        .bind(&name)
        .bind(owner_id)
        .bind(&args.sim_agent_url)
        .fetch_one(pool)
        .await?;

        // Public grant so any seeded user can invoke this agent regardless of
        // ownership/ACL — matches the `chk_public_sentinel` constraint
        // (grant_type = 'public' requires grantee_id = '*').
        sqlx::query(
            "INSERT INTO agent_grants (agent_id, grant_type, grantee_id)
             VALUES ($1, 'public', '*')",
        )
        .bind(agent_id)
        .execute(pool)
        .await?;

        out.push(ManifestAgent {
            id: agent_id,
            name,
            url: args.sim_agent_url.clone(),
        });
    }

    Ok(out)
}
