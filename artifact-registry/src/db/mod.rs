pub mod queries;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use sqlx::PgPool;
use std::str::FromStr;

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = if let Ok(cert_path) = std::env::var("DB_SSL_ROOT_CERT") {
        let opts = PgConnectOptions::from_str(database_url)?
            .ssl_mode(PgSslMode::Require)
            .ssl_root_cert(&cert_path);
        PgPoolOptions::new()
            .max_connections(10)
            .connect_with(opts)
            .await?
    } else {
        PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?
    };
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
