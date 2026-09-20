//! Standalone mock OpenAI-compatible LLM server for load-testing the real
//! server binary (`nasiko-server`/`nasiko-server-ee`) via Goose without real
//! LLM cost or network dependency.
//!
//! Usage: start this, then point the server at it —
//!   OPENAI_BASE_URL=http://127.0.0.1:<port> OPENAI_API_KEY=dummy AGENT_RUNTIME=simulated \
//!     cargo run -p nasiko-server-ee
//!
//! Prints its bound address on stdout and blocks until interrupted (Ctrl-C).

#[tokio::main]
async fn main() {
    let handle = nasiko_bench_support::mock_llm::spawn_mock_llm().await;
    println!("mock-llm listening on {}", handle.base_url);
    let _ = tokio::signal::ctrl_c().await;
}
