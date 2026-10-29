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
//!
//! Everything a discovered agent or a source said is made [`printable`]
//! before it leaves this module: any local process can serve a card, and a
//! card name is printed to the user's terminal.

mod card;
pub mod connections;
mod endpoints;
pub mod env_file;
mod frameworks;
mod http;
pub mod process;

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use card::check_card;

pub use card::CardIssue;
pub use endpoints::{EndpointOrigin, LocalEndpoint};
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
    /// What serves it: a process (with its pid when known) or a Docker container.
    pub origin: EndpointOrigin,
    /// The card exactly as served; what registration sends to the control plane.
    pub raw_card: serde_json::Value,
    /// Which copy of the agent this is, so two copies with one card name are two agents: a
    /// hash of the folder or container serving it, stable across restarts and port changes
    /// ([`identify_copies`]). `None` when neither could be read and the name is unique here.
    pub instance: Option<String>,
}

impl DiscoveredAgent {
    /// The version the catalog would store.
    pub fn version(&self) -> Option<&str> {
        self.card.as_deref().ok()
    }
}

/// This machine's name as the control plane records discovered agents under it: the
/// hostname, without a trailing `.local`. `None` when it cannot be read, so nothing is
/// registered under a made-up identity.
pub fn machine_name() -> Option<String> {
    let endpoints::CommandOutcome::Output(name) =
        endpoints::run_command("hostname", &[], endpoints::COMMAND_TIMEOUT)
    else {
        return None;
    };
    let name = name.trim();
    let name = name.strip_suffix(".local").unwrap_or(name);
    (!name.is_empty()).then(|| name.to_string())
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
    let mut discovery = discover_at(&scan.endpoints, Framework::ALL, &http);
    identify_copies(&mut discovery.agents, process::working_folder);
    // A source's warning can quote a command's stderr.
    let source_warnings = scan.warnings.iter().map(|w| printable_lines(w));
    discovery.warnings.splice(0..0, source_warnings);
    discovery
}

/// Probe `endpoints` with every adapter in `frameworks` on a pool of up to [`MAX_PARALLEL_PROBES`]
/// workers. Each worker takes the next endpoint as soon as it is free, so one
/// slow endpoint delays only itself. Results keep the order of `endpoints`,
/// with a warning per endpoint that stopped answering part-way.
pub fn discover_at(
    endpoints: &[LocalEndpoint],
    frameworks: &[Framework],
    http: &dyn Transport,
) -> Discovery {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Probed)>> = Mutex::new(Vec::new());
    let workers = endpoints.len().min(MAX_PARALLEL_PROBES);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(endpoint) = endpoints.get(index) else {
                        break;
                    };
                    let probed = probe_endpoint(endpoint, frameworks, http);
                    if !probed.agents.is_empty() || probed.gave_up {
                        results
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push((index, probed));
                    }
                }
            });
        }
    });

    let mut results = results
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    results.sort_by_key(|(index, _)| *index);
    let mut discovery = Discovery::default();
    for (index, probed) in results {
        if probed.gave_up {
            discovery.warnings.push(gave_up_warning(&endpoints[index]));
        }
        discovery.agents.extend(probed.agents);
    }
    discovery
}

/// What probing one endpoint found, and whether it stopped answering before
/// every adapter was done with it.
struct Probed {
    agents: Vec<DiscoveredAgent>,
    gave_up: bool,
}

fn gave_up_warning(endpoint: &LocalEndpoint) -> String {
    format!(
        "{}:{} stopped answering part-way (a request hung, or it took over {} s); \
         agents it serves may be missing",
        endpoint.host,
        endpoint.port,
        http::ENDPOINT_BUDGET.as_secs()
    )
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
) -> Probed {
    let base = endpoint.base_url();
    let http = http::EndpointHttp::new(http);
    let mut seen_cards = HashSet::new();
    let agents = frameworks
        .iter()
        .flat_map(|framework| framework.probe(&base, &http))
        .filter(|found| seen_cards.insert(found.card.to_string()))
        .filter_map(|found| to_discovered(found, &endpoint.origin))
        .collect();
    Probed {
        agents,
        gave_up: http.gave_up(),
    }
}

fn to_discovered(found: FoundCard, origin: &EndpointOrigin) -> Option<DiscoveredAgent> {
    let name = printable(found.card.get("name")?.as_str()?.trim());
    Some(DiscoveredAgent {
        name,
        card: check_card(&found.card).map(|version| printable(&version)),
        card_url: printable(&found.card_url),
        origin: origin.clone(),
        // Sent to the server as JSON, never printed.
        raw_card: found.card,
        instance: None,
    })
}

/// Set each agent's [`DiscoveredAgent::instance`]: a hash of what serves it, its container or
/// its process's working folder (`folder_of`, asked once per process), which survives restarts
/// and port changes. An agent this cannot tell apart from another of its name (its folder
/// unreadable, or one folder serving the name twice) is told apart by its card URL instead,
/// which a restart on another port changes. A lone agent with nothing to hash has none.
pub fn identify_copies(
    agents: &mut [DiscoveredAgent],
    folder_of: impl Fn(u32) -> Option<std::path::PathBuf>,
) {
    let mut folders = std::collections::HashMap::new();
    let served_from: Vec<Option<String>> = agents
        .iter()
        .map(|agent| match &agent.origin {
            EndpointOrigin::Docker { container } => Some(format!("container:{container}")),
            EndpointOrigin::Process { pid: Some(pid), .. } => folders
                .entry(*pid)
                .or_insert_with(|| folder_of(*pid))
                .as_ref()
                .map(|folder| format!("folder:{}", folder.display())),
            EndpointOrigin::Process { pid: None, .. } => None,
        })
        .collect();
    let keys: Vec<(&str, Option<&str>)> = agents
        .iter()
        .zip(&served_from)
        .map(|(agent, from)| (agent.name.as_str(), from.as_deref()))
        .collect();
    let instances: Vec<Option<String>> = agents
        .iter()
        .zip(&keys)
        .map(|(agent, key)| {
            let twins = keys.iter().filter(|other| *other == key).count();
            let namesakes = keys.iter().filter(|other| other.0 == key.0).count();
            match key.1 {
                Some(from) if twins == 1 => Some(short_hash(from)),
                None if namesakes == 1 => None,
                from => Some(short_hash(&format!(
                    "{}\nurl:{}",
                    from.unwrap_or_default(),
                    agent.card_url
                ))),
            }
        })
        .collect();
    for (agent, instance) in agents.iter_mut().zip(instances) {
        agent.instance = instance;
    }
}

/// The first 16 hex digits of the SHA-256 of `text`: the folder never leaves the machine.
fn short_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))[..16].to_string()
}

/// `text` with every character that could act on a terminal replaced by `?`:
/// control characters (ESC, which starts colour, cursor and hyperlink
/// sequences; CR; tab; newline) and the bidirectional overrides that reorder
/// what is displayed. Other text, any script, is kept as is.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if is_unsafe_to_print(c) { '?' } else { c })
        .collect()
}

/// [`printable`], line by line: multi-line text such as a command's stderr
/// keeps its line breaks.
fn printable_lines(text: &str) -> String {
    text.lines().map(printable).collect::<Vec<_>>().join("\n")
}

fn is_unsafe_to_print(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
mod tests {
    use super::http::PROBE_TIMEOUT;
    use super::http::tests::FakeHttp;
    use super::*;
    use serde_json::{Value, json};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::time::Instant;

    /// The agents [`discover_at`] finds, for tests that do not look at warnings.
    fn agents_at(
        endpoints: &[LocalEndpoint],
        frameworks: &[Framework],
        http: &dyn Transport,
    ) -> Vec<DiscoveredAgent> {
        discover_at(endpoints, frameworks, http).agents
    }

    fn process_endpoint(port: u16) -> LocalEndpoint {
        LocalEndpoint {
            host: "127.0.0.1".into(),
            port,
            origin: EndpointOrigin::Process {
                pid: Some(42),
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

        let found = agents_at(&[process_endpoint(9000)], Framework::ALL, &http);

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
        let found = agents_at(
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

        agents_at(&[process_endpoint(8000)], Framework::ALL, &http);

        assert_eq!(http.request_count(), 3);
        assert_eq!(http.post_count(), 0);
    }

    #[test]
    fn an_unresponsive_endpoint_is_asked_only_once() {
        let http = FakeHttp::default().unreachable(41000);

        let found = agents_at(&[process_endpoint(41000)], Framework::ALL, &http);

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

        let found = agents_at(&[process_endpoint(2024)], Framework::ALL, &http);

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

        let found = agents_at(&[process_endpoint(2024)], Framework::ALL, &http);

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

        let found = agents_at(&endpoints, Framework::ALL, &http);

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
        assert!(agents_at(&[], Framework::ALL, &http).is_empty());
        assert_eq!(http.request_count(), 0);
    }

    #[test]
    fn an_agent_whose_card_the_catalog_cannot_store_is_reported_with_the_reasons() {
        let http = FakeHttp::default().get(
            "http://127.0.0.1:9000/.well-known/agent-card.json",
            json!({"name": "broken", "url": "http://127.0.0.1:9000/", "skills": [{"id": "s", "name": "S"}]}),
        );

        let found = agents_at(&[process_endpoint(9000)], Framework::ALL, &http);

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

        let found = agents_at(&[process_endpoint(8080)], Framework::ALL, &http);

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

        let found = agents_at(&[process_endpoint(2024)], Framework::ALL, &http);

        assert_eq!(
            found[0].card_url,
            "http://127.0.0.1:2024/.well-known/agent-card.json?assistant_id=a1"
        );
        assert!(found[0].card.is_ok());
    }

    #[test]
    fn a_langgraph_server_that_stops_answering_part_way_is_reported() {
        let base = "http://127.0.0.1:2024";
        let card = |name: &str| json!({"name": name, "url": format!("{base}/a2a/{name}")});
        let http = FakeHttp::default()
            .get(
                &format!("{base}/info"),
                json!({"version": "0.16.4", "langgraph_py_version": "1.2.16", "flags": {}}),
            )
            .post(
                &format!("{base}/assistants/search"),
                json!([{"assistant_id": "a1"}, {"assistant_id": "a2"}, {"assistant_id": "a3"}]),
            )
            .get(
                &format!("{base}/.well-known/agent-card.json?assistant_id=a1"),
                card("planner"),
            )
            .hangs(&format!(
                "{base}/.well-known/agent-card.json?assistant_id=a2"
            ))
            .get(
                &format!("{base}/.well-known/agent-card.json?assistant_id=a3"),
                card("writer"),
            );

        let discovery = discover_at(&[process_endpoint(2024)], Framework::ALL, &http);

        let names: Vec<&str> = discovery.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["planner"]);
        assert_eq!(discovery.warnings.len(), 1, "{:?}", discovery.warnings);
        assert!(
            discovery.warnings[0].starts_with("127.0.0.1:2024 stopped answering part-way"),
            "{:?}",
            discovery.warnings
        );
    }

    #[test]
    fn a_complete_probe_has_no_warnings() {
        let http = FakeHttp::default().get(
            "http://127.0.0.1:8000/.well-known/agent-card.json",
            json!({"name": "crew", "url": "http://127.0.0.1:8000/"}),
        );

        let discovery = discover_at(&[process_endpoint(8000)], Framework::ALL, &http);

        assert_eq!(discovery.agents.len(), 1);
        assert!(discovery.warnings.is_empty());
    }

    #[test]
    fn terminal_control_sequences_from_a_card_are_never_printed() {
        let http = FakeHttp::default().get(
            "http://127.0.0.1:8000/.well-known/agent-card.json",
            json!({
                // Clear the screen, then a hyperlink: both start with ESC.
                "name": "evil\u{1b}[2J\u{1b}]8;;http://x\u{7}agent\u{202E}",
                "version": "1.0\u{1b}[31m",
                "url": "http://127.0.0.1:8000/"
            }),
        );

        let found = agents_at(&[process_endpoint(8000)], Framework::ALL, &http);

        assert_eq!(found[0].name, "evil?[2J?]8;;http://x?agent?");
        assert_eq!(found[0].version(), Some("1.0?[31m"));
    }

    #[test]
    fn printable_keeps_ordinary_text_in_any_script() {
        assert_eq!(printable("café 规划 planner-2"), "café 规划 planner-2");
        assert_eq!(printable("tab\there"), "tab?here");
        assert_eq!(
            printable_lines("permission denied\nsee \u{1b}[1mdocs"),
            "permission denied\nsee ?[1mdocs"
        );
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

        let found = agents_at(&[endpoint_of(&server)], Framework::ALL, &real_http());

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

        let found = agents_at(&[endpoint_of(&server)], Framework::ALL, &real_http());

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

        let found = agents_at(&[endpoint_of(&server)], Framework::ALL, &real_http());

        assert!(found.is_empty());
        no_post.assert();
    }

    #[test]
    fn a_redirect_is_not_followed_and_yields_nothing() {
        // Stands in for another host: a login page a local app redirects to.
        let mut elsewhere = mockito::Server::new();
        let card = json!({
            "name": "elsewhere", "url": "http://127.0.0.1:1/",
            "skills": [{"id": "r", "name": "R", "description": "d"}]
        });
        let never_contacted = elsewhere
            .mock("GET", mockito::Matcher::Any)
            .with_body(json_body(card.clone()))
            .expect(0)
            .create();
        let mut server = mockito::Server::new();
        server
            .mock("GET", mockito::Matcher::Any)
            .with_status(302)
            .with_header("location", &format!("{}/login", elsewhere.url()))
            // A card-shaped body on the redirect itself is not a card either.
            .with_body(json_body(card))
            .create();

        let found = agents_at(&[endpoint_of(&server)], Framework::ALL, &real_http());

        assert!(found.is_empty(), "{found:?}");
        never_contacted.assert();
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
        let found = agents_at(&[process_endpoint(port)], Framework::ALL, &real_http());
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
        let found = agents_at(&[process_endpoint(port)], Framework::ALL, &real_http());

        assert!(found.is_empty());
        assert!(
            started.elapsed() < PROBE_TIMEOUT,
            "took {:?}",
            started.elapsed()
        );
    }

    fn served(name: &str, port: u16, origin: EndpointOrigin) -> DiscoveredAgent {
        DiscoveredAgent {
            name: name.into(),
            card_url: format!("http://127.0.0.1:{port}/card"),
            card: Ok("1.0.0".into()),
            origin,
            raw_card: serde_json::json!({"name": name}),
            instance: None,
        }
    }

    fn process(pid: u32) -> EndpointOrigin {
        EndpointOrigin::Process {
            pid: Some(pid),
            command: "python".into(),
        }
    }

    fn folders(pid: u32) -> Option<std::path::PathBuf> {
        match pid {
            1 => Some("/work/app".into()),
            2 => Some("/work/app-copy".into()),
            3 => Some("/work/app".into()),
            _ => None,
        }
    }

    fn instances(agents: &mut [DiscoveredAgent]) -> Vec<Option<String>> {
        identify_copies(agents, folders);
        agents.iter().map(|agent| agent.instance.clone()).collect()
    }

    #[test]
    fn a_copy_is_identified_by_its_folder_whatever_port_it_is_on() {
        let first = instances(&mut [served("planner", 2024, process(1))]);
        let restarted = instances(&mut [served("planner", 46303, process(1))]);
        assert_eq!(first, restarted);
        assert_eq!(first[0].as_ref().map(String::len), Some(16));

        let mut two = [
            served("planner", 2024, process(1)),
            served("writer", 2024, process(1)),
            served("planner", 2025, process(2)),
            served(
                "planner",
                2026,
                EndpointOrigin::Docker {
                    container: "p".into(),
                },
            ),
        ];
        let ids = instances(&mut two);
        assert_eq!(ids[0], first[0]);
        assert_eq!(ids[0], ids[1], "one folder, two agent names");
        assert_ne!(ids[0], ids[2]);
        assert_ne!(ids[2], ids[3]);
    }

    #[test]
    fn copies_a_folder_cannot_tell_apart_are_told_apart_by_card_url() {
        // One folder serving the same name twice, and a copy whose folder is unreadable.
        let mut agents = [
            served("planner", 2024, process(1)),
            served("planner", 2025, process(3)),
            served("planner", 2026, process(9)),
        ];
        let ids = instances(&mut agents);
        assert!(ids.iter().all(Option::is_some));
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);

        // Alone, an agent with nothing to hash has no copy id: its name is enough.
        assert_eq!(
            instances(&mut [served("planner", 2026, process(9))]),
            [None]
        );
    }
}
