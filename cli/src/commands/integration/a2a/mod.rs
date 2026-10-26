//! Discovery of A2A agents running on this machine.
//!
//! Discovery is read-only and framework-agnostic. It works in three steps:
//!
//! 1. [`endpoints`] lists local TCP endpoints that could serve an agent
//!    (listening sockets, Docker published ports).
//! 2. Every [`Framework`] adapter is asked whether an A2A agent card is served
//!    at each endpoint. The standard well-known card covers most frameworks
//!    (CrewAI, Google ADK, a2a-sdk servers); LangGraph needs its own adapter
//!    because its server keys cards by assistant.
//! 3. Each valid card becomes one [`DiscoveredAgent`].
//!
//! Supporting another framework whose card is not at the standard path means
//! adding one adapter module under [`frameworks`], the same way coding agents
//! are added under `integration/agents/`; nothing else changes.
//!
//! Cost to the machine: a few loopback GETs per listening port (one POST
//! more on a confirmed LangGraph server), no writes, nothing sent off the
//! host. A port that does not answer costs one timeout, after which it is
//! asked nothing more.

mod card;
mod endpoints;
mod frameworks;
mod http;

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use card::check_card;

pub use card::CardIssue;
pub use endpoints::LocalEndpoint;
pub use frameworks::{FoundCard, Framework};
pub use http::{CardHttp, Transport};

/// Endpoints probed at once. Bounded so a machine with hundreds of listening
/// sockets does not spawn hundreds of threads.
const MAX_PARALLEL_PROBES: usize = 64;

/// One A2A agent found on this machine.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredAgent {
    /// The card's `name`, as served.
    pub name: String,
    /// Where the card was fetched from, on the address this machine reaches
    /// the agent at.
    pub card_url: String,
    /// The version the catalog would store from this card, or every reason
    /// it could not store the card ([`card::check_card`]).
    pub card: Result<String, Vec<CardIssue>>,
}

impl DiscoveredAgent {
    /// The version the catalog would store.
    pub fn version(&self) -> Option<&str> {
        self.card.as_deref().ok()
    }
}

/// What a discovery run found, and what it could not look at.
#[derive(Debug, Default)]
pub struct Discovery {
    pub agents: Vec<DiscoveredAgent>,
    /// One line per source that could not be read (a hung `docker ps`, no
    /// `lsof`). The agents it would have found are missing from `agents`.
    pub warnings: Vec<String>,
}

/// Find every A2A agent running on this machine. `excluded_ports` are never
/// probed — the caller passes the local control plane's port so the platform
/// is not reported as an agent.
pub fn discover_local(excluded_ports: &[u16]) -> Discovery {
    let scan = endpoints::collect(&endpoints::default_sources(), excluded_ports);
    let http = http::UreqCardHttp::new();
    Discovery {
        agents: discover_at(&scan.endpoints, Framework::ALL, &http),
        warnings: scan.warnings,
    }
}

/// Probe `endpoints` with every adapter in `frameworks` on a pool of up to [`MAX_PARALLEL_PROBES`]
/// workers. Each worker takes the next endpoint as soon as it is free, so one
/// slow endpoint delays only itself. Results keep the order of `endpoints`.
pub fn discover_at(
    endpoints: &[LocalEndpoint],
    frameworks: &[Framework],
    http: &dyn Transport,
) -> Vec<DiscoveredAgent> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Vec<DiscoveredAgent>)>> = Mutex::new(Vec::new());
    let workers = endpoints.len().min(MAX_PARALLEL_PROBES);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(endpoint) = endpoints.get(index) else {
                        break;
                    };
                    let found = probe_endpoint(endpoint, frameworks, http);
                    if !found.is_empty() {
                        results
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push((index, found));
                    }
                }
            });
        }
    });

    let mut results = results
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().flat_map(|(_, found)| found).collect()
}

/// Run every adapter against one endpoint, through one [`http::EndpointHttp`]
/// that sets each request's limit. When two adapters find the same card (a
/// LangGraph server can serve one assistant's card at both paths) it is
/// reported once, with the first adapter's card URL. Different cards that share
/// a name are different agents and are all reported.
fn probe_endpoint(
    endpoint: &LocalEndpoint,
    frameworks: &[Framework],
    http: &dyn Transport,
) -> Vec<DiscoveredAgent> {
    let base = endpoint.base_url();
    let http = http::EndpointHttp::new(http);
    let mut seen_cards = HashSet::new();
    frameworks
        .iter()
        .flat_map(|framework| framework.probe(&base, &http))
        .filter(|found| seen_cards.insert(found.card.to_string()))
        .filter_map(to_discovered)
        .collect()
}

fn to_discovered(found: FoundCard) -> Option<DiscoveredAgent> {
    let name = found.card.get("name")?.as_str()?.trim().to_string();
    Some(DiscoveredAgent {
        name,
        card: check_card(&found.card),
        card_url: found.card_url,
    })
}

#[cfg(test)]
mod tests {
    use super::endpoints::EndpointOrigin;
    use super::http::PROBE_TIMEOUT;
    use super::http::tests::FakeHttp;
    use super::*;
    use serde_json::{Value, json};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::time::Instant;

    fn process_endpoint(port: u16) -> LocalEndpoint {
        LocalEndpoint {
            host: "127.0.0.1".into(),
            port,
            origin: EndpointOrigin::Process {
                command: "python".into(),
            },
        }
    }

    #[test]
    fn a_standard_card_is_discovered_on_any_framework() {
        let http = FakeHttp::default().get(
            "http://127.0.0.1:9000/.well-known/agent-card.json",
            json!({"name": "researcher", "version": "1.2.0", "url": "http://127.0.0.1:9000/"}),
        );

        let found = discover_at(&[process_endpoint(9000)], Framework::ALL, &http);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "researcher");
        assert_eq!(found[0].version(), Some("1.2.0"));
        assert_eq!(
            found[0].card_url,
            "http://127.0.0.1:9000/.well-known/agent-card.json"
        );
    }

    #[test]
    fn an_endpoint_without_a_card_yields_nothing() {
        let found = discover_at(
            &[process_endpoint(8000)],
            Framework::ALL,
            &FakeHttp::default(),
        );
        assert!(found.is_empty());
    }

    /// The load discovery puts on an unrelated local web server. Raising it
    /// (a new probe, a new path) should be a deliberate choice.
    #[test]
    fn a_web_server_without_a_card_gets_three_reads_and_no_writes() {
        let http = FakeHttp::default();

        discover_at(&[process_endpoint(8000)], Framework::ALL, &http);

        assert_eq!(http.request_count(), 3);
        assert_eq!(http.post_count(), 0);
    }

    #[test]
    fn an_unresponsive_endpoint_is_asked_only_once() {
        let http = FakeHttp::default().unreachable(41000);

        let found = discover_at(&[process_endpoint(41000)], Framework::ALL, &http);

        assert!(found.is_empty());
        assert_eq!(http.request_count(), 1);
    }

    #[test]
    fn the_same_agent_found_by_two_probes_is_reported_once() {
        let card = json!({"name": "planner", "url": "http://127.0.0.1:2024/a2a/a1"});
        let http = FakeHttp::default()
            .get(
                "http://127.0.0.1:2024/info",
                json!({"version": "0.4.21", "langgraph_py_version": "1.0.0", "flags": {}}),
            )
            .post(
                "http://127.0.0.1:2024/assistants/search",
                json!([{"assistant_id": "a1", "graph_id": "planner"}]),
            )
            .get(
                "http://127.0.0.1:2024/.well-known/agent-card.json?assistant_id=a1",
                card.clone(),
            )
            // DEFAULT_A2A_ASSISTANT_ID set: the bare card path serves the same agent.
            .get("http://127.0.0.1:2024/.well-known/agent-card.json", card);

        let found = discover_at(&[process_endpoint(2024)], Framework::ALL, &http);

        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].card_url,
            "http://127.0.0.1:2024/.well-known/agent-card.json?assistant_id=a1"
        );
    }

    #[test]
    fn different_agents_with_the_same_name_are_all_reported() {
        let untitled = |id: &str| json!({"name": "Untitled", "url": format!("http://127.0.0.1:2024/a2a/{id}")});
        let http = FakeHttp::default()
            .get(
                "http://127.0.0.1:2024/info",
                json!({"version": "0.16.4", "langgraph_py_version": "1.0.0", "flags": {}}),
            )
            .post(
                "http://127.0.0.1:2024/assistants/search",
                json!([{"assistant_id": "a1"}, {"assistant_id": "a2"}]),
            )
            .get(
                "http://127.0.0.1:2024/.well-known/agent-card.json?assistant_id=a1",
                untitled("a1"),
            )
            .get(
                "http://127.0.0.1:2024/.well-known/agent-card.json?assistant_id=a2",
                untitled("a2"),
            );

        let found = discover_at(&[process_endpoint(2024)], Framework::ALL, &http);

        assert_eq!(found.len(), 2);
    }

    #[test]
    fn results_keep_endpoint_order_whatever_order_workers_finish_in() {
        let ports: Vec<u16> = (9000..9000 + (MAX_PARALLEL_PROBES as u16) * 2 + 3).collect();
        let http =
            ports
                .iter()
                .filter(|port| *port % 3 != 0)
                .fold(FakeHttp::default(), |http, port| {
                    http.get(
                        &format!("http://127.0.0.1:{port}/.well-known/agent-card.json"),
                        json!({"name": format!("agent-{port}"), "url": "http://x/"}),
                    )
                });
        let endpoints: Vec<_> = ports.iter().copied().map(process_endpoint).collect();

        let found = discover_at(&endpoints, Framework::ALL, &http);

        let found_names: Vec<&str> = found.iter().map(|agent| agent.name.as_str()).collect();
        let expected: Vec<String> = ports
            .into_iter()
            .filter(|port| port % 3 != 0)
            .map(|port| format!("agent-{port}"))
            .collect();
        assert_eq!(found_names, expected);
    }

    #[test]
    fn no_endpoints_means_no_work() {
        let http = FakeHttp::default();
        assert!(discover_at(&[], Framework::ALL, &http).is_empty());
        assert_eq!(http.request_count(), 0);
    }

    #[test]
    fn an_agent_whose_card_the_catalog_cannot_store_is_reported_with_the_reasons() {
        let http = FakeHttp::default().get(
            "http://127.0.0.1:9000/.well-known/agent-card.json",
            json!({"name": "broken", "url": "http://127.0.0.1:9000/", "skills": [{"id": "s", "name": "S"}]}),
        );

        let found = discover_at(&[process_endpoint(9000)], Framework::ALL, &http);

        assert_eq!(found.len(), 1);
        let issues = found[0].card.as_ref().expect_err("card should be invalid");
        assert_eq!(issues[0].to_string(), "skills[0].description: is required");
        assert_eq!(found[0].version(), None);
    }

    #[test]
    fn the_card_url_uses_the_probed_port_not_the_cards_own() {
        // A container serving on 8000 inside, published on 8080 outside.
        let http = FakeHttp::default().get(
            "http://127.0.0.1:8080/.well-known/agent-card.json",
            json!({"name": "dockerised", "url": "http://0.0.0.0:8000/rpc", "skills": []}),
        );

        let found = discover_at(&[process_endpoint(8080)], Framework::ALL, &http);

        assert_eq!(
            found[0].card_url,
            "http://127.0.0.1:8080/.well-known/agent-card.json"
        );
        assert!(found[0].card.is_ok());
    }

    #[test]
    fn a_langgraph_agent_is_found_by_its_assistant_card() {
        let card = json!({
            "name": "planner",
            "supportedInterfaces": [{"url": "http://127.0.0.1:2024/a2a/a1", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}],
            "skills": [{"id": "a1-main", "name": "planner", "description": "plans"}]
        });
        let http = FakeHttp::default()
            .get(
                "http://127.0.0.1:2024/info",
                json!({"version": "0.16.4", "langgraph_py_version": "1.2.16", "flags": {}}),
            )
            .post(
                "http://127.0.0.1:2024/assistants/search",
                json!([{"assistant_id": "a1"}]),
            )
            .get(
                "http://127.0.0.1:2024/.well-known/agent-card.json?assistant_id=a1",
                card,
            );

        let found = discover_at(&[process_endpoint(2024)], Framework::ALL, &http);

        assert_eq!(
            found[0].card_url,
            "http://127.0.0.1:2024/.well-known/agent-card.json?assistant_id=a1"
        );
        assert!(found[0].card.is_ok());
    }

    // ─── Over real HTTP ─────────────────────────────────────────────────────
    // The tests above use a fake client. These probe loopback servers through
    // the client `discover_local` uses, so the socket, timeout and body
    // handling are the real ones.

    fn real_http() -> http::UreqCardHttp {
        http::UreqCardHttp::new()
    }

    fn endpoint_of(server: &mockito::Server) -> LocalEndpoint {
        process_endpoint(server.socket_address().port())
    }

    fn json_body(value: Value) -> String {
        value.to_string()
    }

    #[test]
    fn finds_a_standard_card_over_http() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/.well-known/agent-card.json")
            .with_header("content-type", "application/json")
            .with_body(json_body(json!({
                "name": "crew-researcher", "version": "v1.2.0", "url": "http://127.0.0.1:1/",
                "skills": [{"id": "r", "name": "Researcher", "description": "Find facts"}]
            })))
            .create();

        let found = discover_at(&[endpoint_of(&server)], Framework::ALL, &real_http());

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "crew-researcher");
        assert_eq!(found[0].version(), Some("1.2.0"));
        assert_eq!(
            found[0].card_url,
            format!("{}/.well-known/agent-card.json", server.url())
        );
    }

    #[test]
    fn finds_every_langgraph_assistant_over_http() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/info")
            .with_body(json_body(
                json!({"version": "0.16.4", "langgraph_py_version": "1.2.16", "flags": {}}),
            ))
            .create();
        let search = server
            .mock("POST", "/assistants/search")
            .with_body(json_body(
                json!([{"assistant_id": "a1"}, {"assistant_id": "a2"}]),
            ))
            .expect(1)
            .create();
        for (id, name) in [("a1", "planner"), ("a2", "writer")] {
            server
                .mock("GET", "/.well-known/agent-card.json")
                .match_query(mockito::Matcher::UrlEncoded(
                    "assistant_id".into(),
                    id.into(),
                ))
                .with_body(json_body(json!({
                    "name": name,
                    "supportedInterfaces": [{"url": format!("http://127.0.0.1:1/a2a/{id}"), "protocolBinding": "JSONRPC"}],
                    "version": "0.16.4"
                })))
                .create();
        }

        let found = discover_at(&[endpoint_of(&server)], Framework::ALL, &real_http());

        let names: Vec<&str> = found.iter().map(|agent| agent.name.as_str()).collect();
        assert_eq!(names, vec!["planner", "writer"]);
        search.assert();
    }

    #[test]
    fn an_ordinary_web_server_gets_no_writes_and_yields_nothing() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", mockito::Matcher::Any)
            .with_header("content-type", "text/html")
            .with_body("<html>not an agent</html>")
            .create();
        let no_post = server
            .mock("POST", mockito::Matcher::Any)
            .expect(0)
            .create();

        let found = discover_at(&[endpoint_of(&server)], Framework::ALL, &real_http());

        assert!(found.is_empty());
        no_post.assert();
    }

    #[test]
    fn a_port_that_never_answers_costs_one_timeout() {
        // Accepts connections and never replies, like a hung service.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(Mutex::new(Vec::<TcpStream>::new()));
        let held = Arc::clone(&accepted);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                held.lock().unwrap().push(stream);
            }
        });

        let started = Instant::now();
        let found = discover_at(&[process_endpoint(port)], Framework::ALL, &real_http());
        let elapsed = started.elapsed();

        assert!(found.is_empty());
        assert!(
            elapsed >= PROBE_TIMEOUT && elapsed < PROBE_TIMEOUT * 3,
            "took {elapsed:?}"
        );
        assert_eq!(accepted.lock().unwrap().len(), 1, "probed more than once");
    }

    #[test]
    fn a_closed_port_is_skipped_without_waiting() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        let started = Instant::now();
        let found = discover_at(&[process_endpoint(port)], Framework::ALL, &real_http());

        assert!(found.is_empty());
        assert!(
            started.elapsed() < PROBE_TIMEOUT,
            "took {:?}",
            started.elapsed()
        );
    }
}
