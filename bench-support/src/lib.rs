//! Shared harness for benchmarking the control-plane server APIs (both
//! `nasiko-server` and `nasiko-server-ee`) without Docker/Kubernetes or real
//! LLM cost.
//!
//! Two consumers:
//! - `cargo bench` targets (`oss/server/benches`, `ee/server/benches`) use
//!   every module in-process: sim agent + mock LLM + `SimulatedRuntime` +
//!   the real server, all inside one criterion binary.
//! - The Goose load generator (`oss/bench`) runs against a separately
//!   started, long-lived real server — for that workflow, `mock_llm` also
//!   ships as a standalone binary (`src/bin/mock_llm.rs`) a developer points
//!   `OPENAI_BASE_URL` at when starting the server with `AGENT_RUNTIME=simulated`.

pub mod config;
pub mod mock_llm;
pub mod seed;
pub mod server;
pub mod sim_agent;
