//! CLI for the `harness_sessions` scenario (the scenario itself lives in
//! `nasiko_bench_support::harness_sessions`). Run as
//! `nasiko-bench harness_sessions [flags]`; it needs the infra Postgres/Redis/S3 up and
//! creates and drops its own database, so it takes no manifest and no running server.

use clap::Parser;
use nasiko_bench_support::harness_sessions::{Params, run};

/// Load shape for the harness-session capacity scenario.
#[derive(Parser)]
#[command(name = "nasiko-bench harness_sessions")]
struct Args {
    /// Harness sessions to create (the design target is 10000).
    #[arg(long, default_value_t = 1_000)]
    sessions: usize,
    /// Users the sessions are spread over.
    #[arg(long, default_value_t = 100)]
    users: usize,
    /// Percent of sessions that send a turn each tick.
    #[arg(long, default_value_t = 2.0)]
    active_pct: f64,
    /// Number of ticks.
    #[arg(long, default_value_t = 10)]
    ticks: usize,
    /// Virtual seconds between ticks.
    #[arg(long, default_value_t = 120)]
    tick_secs: u64,
    /// Idle TTL in virtual seconds before the reaper may suspend a sandbox.
    #[arg(long, default_value_t = 300)]
    ttl_secs: u64,
    /// Seed for the active-session draw and the latency model.
    #[arg(long, default_value_t = 42)]
    seed: u64,
}

/// Runs the scenario with `args` (the arguments after the scenario name) and prints the report.
pub async fn main(args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    let args =
        Args::parse_from(std::iter::once("nasiko-bench harness_sessions".to_owned()).chain(args));
    let report = run(Params {
        sessions: args.sessions,
        users: args.users,
        active_pct: args.active_pct,
        ticks: args.ticks,
        tick_secs: args.tick_secs,
        idle_ttl_secs: args.ttl_secs,
        seed: args.seed,
        ..Params::default()
    })
    .await?;
    println!("{report}");
    Ok(())
}
