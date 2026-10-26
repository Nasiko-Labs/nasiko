//! The tiny HTTP surface probes need, behind a trait so probes are tested
//! without a network.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Largest response body read. An agent card is a few KB; anything bigger is
/// not a card and is not worth buffering.
const MAX_BODY_BYTES: u64 = 256 * 1024;

/// Sent with every probe, so a developer reading their server's access log
/// can tell where the requests came from.
const USER_AGENT: &str = "nasiko-cli-agent-discovery";

/// Limit for the first request to a port. Loopback HTTP servers answer in
/// milliseconds and closed ports refuse at once; this only bounds ports that
/// accept a connection and never reply.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Limit for each later request to a port that has answered HTTP: it is a
/// live server, and some build a card on request (LangGraph, under load).
pub const ANSWERED_TIMEOUT: Duration = Duration::from_secs(3);

/// Total time spent on one port, however many cards it serves.
pub const ENDPOINT_BUDGET: Duration = Duration::from_secs(10);

/// What one request produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// A 2xx response whose body is JSON.
    Json(Value),
    /// Something answered over HTTP, but not with usable JSON (a 404, an HTML
    /// page, an oversized body). The endpoint is alive; other paths may work.
    Other,
    /// No HTTP answer at all: refused, timed out, or not speaking HTTP.
    /// Further requests to the same endpoint will not do better.
    Unreachable,
}

impl Reply {
    pub fn json(self) -> Option<Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::Other | Self::Unreachable => None,
        }
    }
}

/// JSON over HTTP, as framework adapters see it: one endpoint, timeouts
/// already decided ([`EndpointHttp`]).
pub trait CardHttp: Sync {
    fn get(&self, url: &str) -> Reply;
    fn post(&self, url: &str, body: &Value) -> Reply;
}

/// JSON over HTTP with a limit for the whole request.
pub trait Transport: Sync {
    fn get(&self, url: &str, timeout: Duration) -> Reply;
    fn post(&self, url: &str, body: &Value, timeout: Duration) -> Reply;
}

/// [`Transport`] over ureq: a capped body and no proxy.
pub struct UreqCardHttp {
    agent: ureq::Agent,
}

impl UreqCardHttp {
    /// Probes only ever target local addresses, so any proxy from the
    /// environment (`HTTP_PROXY`, `ALL_PROXY`) is ignored: routing them
    /// through a proxy would either fail or reach the proxy's localhost.
    pub fn new() -> Self {
        let config = ureq::config::Config::builder()
            .proxy(None)
            .user_agent(USER_AGENT)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Transport for UreqCardHttp {
    fn get(&self, url: &str, timeout: Duration) -> Reply {
        let request = self.agent.get(url).config().timeout_global(Some(timeout));
        to_reply(request.build().call())
    }

    fn post(&self, url: &str, body: &Value, timeout: Duration) -> Reply {
        let request = self.agent.post(url).config().timeout_global(Some(timeout));
        to_reply(request.build().send_json(body))
    }
}

fn to_reply(result: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Reply {
    match result {
        Ok(mut response) => response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_json()
            .map_or(Reply::Other, Reply::Json),
        // An HTTP status means a server answered; every other error means none did.
        Err(ureq::Error::StatusCode(_)) => Reply::Other,
        Err(_) => Reply::Unreachable,
    }
}

/// The requests to one endpoint, with their limits:
/// - after the first `Unreachable`, nothing more is sent, so a port that
///   swallows requests costs one [`PROBE_TIMEOUT`], not one per probe path;
/// - once the port has answered HTTP, requests get [`ANSWERED_TIMEOUT`];
/// - all of it stays within [`ENDPOINT_BUDGET`].
pub struct EndpointHttp<'a> {
    inner: &'a dyn Transport,
    deadline: Instant,
    answered: AtomicBool,
    unreachable: AtomicBool,
}

impl<'a> EndpointHttp<'a> {
    pub fn new(inner: &'a dyn Transport) -> Self {
        Self::with_budget(inner, ENDPOINT_BUDGET)
    }

    fn with_budget(inner: &'a dyn Transport, budget: Duration) -> Self {
        Self {
            inner,
            deadline: Instant::now() + budget,
            answered: AtomicBool::new(false),
            unreachable: AtomicBool::new(false),
        }
    }

    fn send(&self, request: impl FnOnce(Duration) -> Reply) -> Reply {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if self.unreachable.load(Ordering::Relaxed) || remaining.is_zero() {
            return Reply::Unreachable;
        }
        let limit = if self.answered.load(Ordering::Relaxed) {
            ANSWERED_TIMEOUT
        } else {
            PROBE_TIMEOUT
        };
        let reply = request(limit.min(remaining));
        match reply {
            Reply::Unreachable => self.unreachable.store(true, Ordering::Relaxed),
            Reply::Json(_) | Reply::Other => self.answered.store(true, Ordering::Relaxed),
        }
        reply
    }
}

impl CardHttp for EndpointHttp<'_> {
    fn get(&self, url: &str) -> Reply {
        self.send(|timeout| self.inner.get(url, timeout))
    }

    fn post(&self, url: &str, body: &Value) -> Reply {
        self.send(|timeout| self.inner.post(url, body, timeout))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    /// Canned replies keyed by URL. Unlisted URLs answer like a server with no
    /// such route (`Other`); listed ports in `unreachable` never answer.
    #[derive(Default)]
    pub struct FakeHttp {
        gets: HashMap<String, Value>,
        posts: HashMap<String, Value>,
        unreachable_ports: Vec<u16>,
        requests: AtomicUsize,
        post_requests: AtomicUsize,
        timeouts: Mutex<Vec<Duration>>,
    }

    impl FakeHttp {
        pub fn get(mut self, url: &str, response: Value) -> Self {
            self.gets.insert(url.to_string(), response);
            self
        }

        pub fn post(mut self, url: &str, response: Value) -> Self {
            self.posts.insert(url.to_string(), response);
            self
        }

        pub fn unreachable(mut self, port: u16) -> Self {
            self.unreachable_ports.push(port);
            self
        }

        pub fn request_count(&self) -> usize {
            self.requests.load(Ordering::Relaxed)
        }

        pub fn post_count(&self) -> usize {
            self.post_requests.load(Ordering::Relaxed)
        }

        /// The limit each request was sent with, in order.
        pub fn timeouts(&self) -> Vec<Duration> {
            self.timeouts.lock().unwrap().clone()
        }

        fn reply(&self, url: &str, table: &HashMap<String, Value>, timeout: Duration) -> Reply {
            self.requests.fetch_add(1, Ordering::Relaxed);
            self.timeouts.lock().unwrap().push(timeout);
            let dead = self
                .unreachable_ports
                .iter()
                .any(|port| url.contains(&format!(":{port}/")));
            if dead {
                return Reply::Unreachable;
            }
            table.get(url).cloned().map_or(Reply::Other, Reply::Json)
        }
    }

    impl Transport for FakeHttp {
        fn get(&self, url: &str, timeout: Duration) -> Reply {
            self.reply(url, &self.gets, timeout)
        }

        fn post(&self, url: &str, _body: &Value, timeout: Duration) -> Reply {
            self.post_requests.fetch_add(1, Ordering::Relaxed);
            self.reply(url, &self.posts, timeout)
        }
    }

    /// Adapter tests talk to the fake directly, as they would to one endpoint.
    impl CardHttp for FakeHttp {
        fn get(&self, url: &str) -> Reply {
            Transport::get(self, url, PROBE_TIMEOUT)
        }

        fn post(&self, url: &str, body: &Value) -> Reply {
            Transport::post(self, url, body, PROBE_TIMEOUT)
        }
    }

    #[test]
    fn an_unreachable_endpoint_is_asked_once() {
        let fake = FakeHttp::default().unreachable(41000);
        let endpoint = EndpointHttp::new(&fake);

        for path in ["/info", "/a", "/b"] {
            assert_eq!(
                endpoint.get(&format!("http://127.0.0.1:41000{path}")),
                Reply::Unreachable
            );
        }

        assert_eq!(fake.request_count(), 1);
    }

    #[test]
    fn a_live_endpoint_keeps_answering_after_a_miss() {
        let fake = FakeHttp::default().get("http://127.0.0.1:9000/b", serde_json::json!({}));
        let endpoint = EndpointHttp::new(&fake);

        assert_eq!(endpoint.get("http://127.0.0.1:9000/a"), Reply::Other);
        assert_eq!(
            endpoint.get("http://127.0.0.1:9000/b"),
            Reply::Json(serde_json::json!({}))
        );
    }

    #[test]
    fn a_port_that_answered_gets_the_longer_limit() {
        let fake = FakeHttp::default().get("http://127.0.0.1:2024/info", serde_json::json!({}));
        let endpoint = EndpointHttp::new(&fake);

        endpoint.get("http://127.0.0.1:2024/info");
        endpoint.get("http://127.0.0.1:2024/missing");
        endpoint.post("http://127.0.0.1:2024/search", &serde_json::json!({}));

        assert_eq!(
            fake.timeouts(),
            vec![PROBE_TIMEOUT, ANSWERED_TIMEOUT, ANSWERED_TIMEOUT]
        );
    }

    #[test]
    fn nothing_is_sent_once_the_endpoint_budget_is_spent() {
        let fake = FakeHttp::default().get("http://127.0.0.1:2024/info", serde_json::json!({}));
        let endpoint = EndpointHttp::with_budget(&fake, Duration::ZERO);

        assert_eq!(
            endpoint.get("http://127.0.0.1:2024/info"),
            Reply::Unreachable
        );
        assert_eq!(fake.request_count(), 0);
    }
}
