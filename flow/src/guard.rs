use redis::AsyncCommands;

use crate::context::FlowContext;

#[derive(Debug, Clone)]
pub struct FlowConfig {
    pub max_depth: u32,
    pub max_fan_out: u32,
    pub max_flow_tokens: u64,
    pub flow_timeout_secs: u64,
    pub flow_state_ttl_secs: u64,
}

impl Default for FlowConfig {
    fn default() -> Self {
        Self {
            max_depth: 5,
            max_fan_out: 20,
            max_flow_tokens: 100_000,
            flow_timeout_secs: 120,
            flow_state_ttl_secs: 300,
        }
    }
}

impl FlowConfig {
    pub fn from_env() -> Self {
        Self {
            max_depth: parse_env("NASIKO_FLOW_MAX_DEPTH", 5),
            max_fan_out: parse_env("NASIKO_FLOW_MAX_FAN_OUT", 20),
            max_flow_tokens: parse_env("NASIKO_FLOW_MAX_TOKENS", 100_000),
            flow_timeout_secs: parse_env("NASIKO_FLOW_TIMEOUT_SECS", 120),
            flow_state_ttl_secs: parse_env("NASIKO_FLOW_STATE_TTL_SECS", 300),
        }
    }
}

fn parse_env<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[derive(Debug, Clone)]
pub enum FlowRejection {
    MaxDepthExceeded { depth: u32, max: u32 },
    CycleDetected { agent_id: String, chain: Vec<String> },
    MaxFanOutExceeded { invocations: u32, max: u32 },
    TokenBudgetExhausted { used: u64, max: u64 },
    FlowTimeout { elapsed_secs: u64, max: u64 },
    /// Redis (the sole backing store for depth/cycle/fan-out/token state) was
    /// unreachable. Fails CLOSED rather than silently disabling every limit —
    /// the threat model is explicitly unvetted agent/build code, so a Redis
    /// outage must not become an unbounded-recursion/fan-out DoS window.
    GuardUnavailable,
}

impl std::fmt::Display for FlowRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxDepthExceeded { depth, max } => {
                write!(f, "max call depth exceeded: {depth}/{max}")
            }
            Self::CycleDetected { agent_id, chain } => {
                write!(f, "cycle detected: agent {agent_id} already in chain {chain:?}")
            }
            Self::MaxFanOutExceeded { invocations, max } => {
                write!(f, "max fan-out exceeded: {invocations}/{max} invocations")
            }
            Self::TokenBudgetExhausted { used, max } => {
                write!(f, "flow token budget exhausted: {used}/{max}")
            }
            Self::FlowTimeout { elapsed_secs, max } => {
                write!(f, "flow timeout: {elapsed_secs}s/{max}s")
            }
            Self::GuardUnavailable => {
                write!(f, "flow guard unavailable (redis unreachable) — failing closed")
            }
        }
    }
}

/// Flow guard that enforces cascade limits using Redis for all state.
#[derive(Clone)]
pub struct FlowGuard {
    config: FlowConfig,
    redis: redis::Client,
}

impl FlowGuard {
    pub fn new(redis: redis::Client, config: FlowConfig) -> Self {
        Self { config, redis }
    }

    pub fn config(&self) -> &FlowConfig {
        &self.config
    }

    pub async fn init_flow(&self, ctx: &FlowContext, root_agent: &str) {
        let key = ctx.redis_key();
        let Some(mut conn) = self.redis.get_multiplexed_async_connection().await.ok() else {
            return;
        };

        let now = chrono::Utc::now().to_rfc3339();
        let _: () = redis::cmd("HSET")
            .arg(&key)
            .arg("root_agent")
            .arg(root_agent)
            .arg("depth")
            .arg(0u32)
            .arg("call_chain")
            .arg(root_agent)
            .arg("total_invocations")
            .arg(0u32)
            .arg("total_tokens_used")
            .arg(0u64)
            .arg("started_at")
            .arg(&now)
            .query_async(&mut conn)
            .await
            .unwrap_or(());

        let _: () = conn
            .expire(&key, self.config.flow_state_ttl_secs as i64)
            .await
            .unwrap_or(());
    }

    pub async fn check(
        &self,
        ctx: &FlowContext,
        target_agent_id: &str,
    ) -> Result<(), FlowRejection> {
        let key = ctx.redis_key();
        let Some(mut conn) = self.redis.get_multiplexed_async_connection().await.ok() else {
            return Err(FlowRejection::GuardUnavailable);
        };

        let depth: u32 = conn.hget(&key, "depth").await.unwrap_or(0);
        let call_chain_str: String = conn.hget(&key, "call_chain").await.unwrap_or_default();
        let total_invocations: u32 = conn.hget(&key, "total_invocations").await.unwrap_or(0);
        let total_tokens: u64 = conn.hget(&key, "total_tokens_used").await.unwrap_or(0);
        let started_at: Option<String> = conn.hget(&key, "started_at").await.unwrap_or(None);

        let call_chain: Vec<&str> = call_chain_str
            .split(',')
            .filter(|s| !s.is_empty())
            .collect();

        if depth >= self.config.max_depth {
            return Err(FlowRejection::MaxDepthExceeded {
                depth,
                max: self.config.max_depth,
            });
        }

        if call_chain.contains(&target_agent_id) {
            return Err(FlowRejection::CycleDetected {
                agent_id: target_agent_id.to_string(),
                chain: call_chain.iter().map(|s| s.to_string()).collect(),
            });
        }

        if total_invocations >= self.config.max_fan_out {
            return Err(FlowRejection::MaxFanOutExceeded {
                invocations: total_invocations,
                max: self.config.max_fan_out,
            });
        }

        if total_tokens >= self.config.max_flow_tokens {
            return Err(FlowRejection::TokenBudgetExhausted {
                used: total_tokens,
                max: self.config.max_flow_tokens,
            });
        }

        if let Some(started_str) = started_at
            && let Ok(started) = chrono::DateTime::parse_from_rfc3339(&started_str)
        {
            let elapsed = chrono::Utc::now()
                .signed_duration_since(started)
                .num_seconds() as u64;
            if elapsed > self.config.flow_timeout_secs {
                return Err(FlowRejection::FlowTimeout {
                    elapsed_secs: elapsed,
                    max: self.config.flow_timeout_secs,
                });
            }
        }

        Ok(())
    }

    pub async fn record_invocation(
        &self,
        ctx: &FlowContext,
        target_agent_id: &str,
    ) -> Result<(), FlowRejection> {
        let key = ctx.redis_key();
        let Some(mut conn) = self.redis.get_multiplexed_async_connection().await.ok() else {
            return Err(FlowRejection::GuardUnavailable);
        };

        let invocations: u32 = redis::cmd("HINCRBY")
            .arg(&key)
            .arg("total_invocations")
            .arg(1)
            .query_async(&mut conn)
            .await
            .unwrap_or(1);

        if invocations > self.config.max_fan_out {
            return Err(FlowRejection::MaxFanOutExceeded {
                invocations,
                max: self.config.max_fan_out,
            });
        }

        let depth: u32 = redis::cmd("HINCRBY")
            .arg(&key)
            .arg("depth")
            .arg(1)
            .query_async(&mut conn)
            .await
            .unwrap_or(1);

        let chain: String = conn.hget(&key, "call_chain").await.unwrap_or_default();
        let new_chain = if chain.is_empty() {
            target_agent_id.to_string()
        } else {
            format!("{},{}", chain, target_agent_id)
        };
        let _: () = conn.hset(&key, "call_chain", &new_chain).await.unwrap_or(());

        let _: () = conn
            .expire(&key, self.config.flow_state_ttl_secs as i64)
            .await
            .unwrap_or(());

        if depth > self.config.max_depth {
            return Err(FlowRejection::MaxDepthExceeded {
                depth,
                max: self.config.max_depth,
            });
        }

        Ok(())
    }

    pub async fn record_tokens(
        &self,
        ctx: &FlowContext,
        tokens_used: u64,
    ) -> Result<u64, FlowRejection> {
        let key = ctx.redis_key();
        let Some(mut conn) = self.redis.get_multiplexed_async_connection().await.ok() else {
            return Err(FlowRejection::GuardUnavailable);
        };

        let total_used: u64 = redis::cmd("HINCRBY")
            .arg(&key)
            .arg("total_tokens_used")
            .arg(tokens_used)
            .query_async(&mut conn)
            .await
            .unwrap_or(tokens_used);

        if total_used > self.config.max_flow_tokens {
            return Err(FlowRejection::TokenBudgetExhausted {
                used: total_used,
                max: self.config.max_flow_tokens,
            });
        }

        Ok(self.config.max_flow_tokens.saturating_sub(total_used))
    }

    pub async fn record_return(&self, ctx: &FlowContext) {
        let key = ctx.redis_key();
        let Some(mut conn) = self.redis.get_multiplexed_async_connection().await.ok() else {
            return;
        };

        let _: () = redis::cmd("HINCRBY")
            .arg(&key)
            .arg("depth")
            .arg(-1i32)
            .query_async(&mut conn)
            .await
            .unwrap_or(());

        let chain: String = conn.hget(&key, "call_chain").await.unwrap_or_default();
        if let Some(pos) = chain.rfind(',') {
            let shortened = &chain[..pos];
            let _: () = conn.hset(&key, "call_chain", shortened).await.unwrap_or(());
        } else {
            let _: () = conn.hset(&key, "call_chain", "").await.unwrap_or(());
        }
    }
}
