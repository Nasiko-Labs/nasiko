//! DB-backed [`PricingSource`] over the `model_pricing` table, with a
//! short-lived in-memory cache and [`StaticPricing`] fallback.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sqlx::PgPool;
use tokio::sync::RwLock;

use crate::pricing::{PricePer1M, PricingSource, StaticPricing};

const CACHE_TTL: Duration = Duration::from_secs(300);

pub struct DbPricing {
    db: PgPool,
    cache: RwLock<HashMap<String, (Option<PricePer1M>, Instant)>>,
}

impl DbPricing {
    pub fn new(db: PgPool) -> Self {
        Self {
            db,
            cache: RwLock::new(HashMap::new()),
        }
    }

    async fn lookup_db(&self, model: &str) -> Option<PricePer1M> {
        #[derive(sqlx::FromRow)]
        struct Row {
            input_price_per_1m: rust_decimal::Decimal,
            output_price_per_1m: rust_decimal::Decimal,
        }

        let row = sqlx::query_as::<_, Row>(
            r#"SELECT input_price_per_1m, output_price_per_1m
               FROM model_pricing
               WHERE model = $1
                 AND effective_from <= now()
                 AND (effective_until IS NULL OR effective_until > now())
               ORDER BY effective_from DESC
               LIMIT 1"#,
        )
        .bind(model)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| tracing::warn!(model, error = %e, "model_pricing lookup failed"))
        .ok()
        .flatten()?;

        use rust_decimal::prelude::ToPrimitive;
        Some((
            row.input_price_per_1m.to_f64()?,
            row.output_price_per_1m.to_f64()?,
        ))
    }
}

#[async_trait]
impl PricingSource for DbPricing {
    async fn price_per_1m(&self, model: &str) -> Option<PricePer1M> {
        if let Some((price, at)) = self.cache.read().await.get(model)
            && at.elapsed() < CACHE_TTL
        {
            return *price;
        }

        let price = match self.lookup_db(model).await {
            Some(p) => Some(p),
            None => StaticPricing.price_per_1m(model).await,
        };

        self.cache
            .write()
            .await
            .insert(model.to_string(), (price, Instant::now()));
        price
    }
}
