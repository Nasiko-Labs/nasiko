//! Local TCP endpoints that might serve an A2A agent.
//!
//! Each [`EndpointSource`] lists endpoints one way (listening sockets, Docker
//! published ports). [`collect`] runs them in parallel and merges them, so
//! every endpoint is probed once and nothing the platform runs, or that is
//! known not to speak HTTP, is probed at all.

use std::collections::HashSet;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Prefix of containers the platform's own `DockerRuntime` deploys. Those are
/// already registered agents, not discoveries (`oss/runtime/src/stats.rs`).
const PLATFORM_CONTAINER_PREFIX: &str = "nasiko-agent-";

/// Longest an external listing command (`lsof`, `docker ps`) may run. A
/// half-started Docker daemon can block `docker ps` indefinitely.
pub(super) const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

/// Ports of well-known services that do not speak HTTP. Probing them gains
/// nothing and is not harmless: Redis logs an HTTP request as a "Possible
/// SECURITY ATTACK" and Postgres, MySQL and MongoDB log protocol errors.
const NON_HTTP_PORTS: &[u16] = &[
    22, 25, 53, 110, 143, 389, 445, 465, 587, 993, 995, 1433, 1521, 2181, 3306, 4222, 5432, 5672,
    6379, 6380, 7687, 9042, 9092, 11211, 26379, 27017,
];

/// Commands of the same services, for when they run on a non-default port.
/// Prefixes, because `lsof` truncates command names (`redis-ser`).
const NON_HTTP_COMMANDS: &[&str] = &[
    "postgres",
    "redis-ser",
    "mysqld",
    "mariadbd",
    "mongod",
    "memcached",
    "sshd",
];

/// A local address and port worth probing, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEndpoint {
    /// Host to connect to: `127.0.0.1`, `[::1]`, or the specific address the
    /// socket is bound to.
    pub host: String,
    pub port: u16,
    pub origin: EndpointOrigin,
}

impl LocalEndpoint {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    /// What makes two endpoints the same socket: the port, and the address
    /// unless it is a loopback one. A wildcard bind is probed on its family's
    /// loopback, and a server listening on both families (`0.0.0.0` and `::`)
    /// is one process, so `127.0.0.1` and `[::1]` count as one address and it
    /// is asked once. Processes bound to different specific addresses (a LAN
    /// IP and loopback) on one port are different servers, probed apart.
    fn address(&self) -> (u16, Option<&str>) {
        let loopback = matches!(self.host.as_str(), "127.0.0.1" | "[::1]");
        (self.port, (!loopback).then_some(self.host.as_str()))
    }
}

/// What owns an endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointOrigin {
    /// A process listening directly on the host.
    /// `pid` is known when `lsof` listed the socket (always for the user's own processes).
    Process { pid: Option<u32>, command: String },
    /// A Docker container publishing the port.
    Docker { container: String },
}

/// One way of listing local endpoints. A source with nothing to look at
/// (Docker not installed or not running) returns an empty list; one that
/// should have worked and did not returns `Err` with a sentence saying what
/// was skipped and why, which the CLI prints as a warning. Discovery reports
/// what it can see either way.
pub trait EndpointSource: Send + Sync {
    fn endpoints(&self) -> Result<Vec<LocalEndpoint>, String>;
}

/// Endpoints worth probing, and the sources that could not be read.
#[derive(Debug, Default)]
pub struct EndpointScan {
    pub endpoints: Vec<LocalEndpoint>,
    pub warnings: Vec<String>,
}

/// Sources used by `nasiko agents discover`. Docker comes first so a port it
/// publishes is labelled with its container rather than the Docker proxy
/// process that holds the socket on the host.
pub fn default_sources() -> Vec<Box<dyn EndpointSource>> {
    vec![Box::new(DockerPublishedPorts), Box::new(ListeningSockets)]
}

/// Run every source in parallel and merge the results; the first source wins
/// for an [address](LocalEndpoint::address). Dropped: `excluded_ports`, ports published by platform-deployed
/// containers, and endpoints of known non-HTTP services.
pub fn collect(sources: &[Box<dyn EndpointSource>], excluded_ports: &[u16]) -> EndpointScan {
    let results: Vec<Result<Vec<LocalEndpoint>, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = sources
            .iter()
            .map(|source| scope.spawn(|| source.endpoints()))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or_else(|_| Ok(Vec::new())))
            .collect()
    });
    let mut all = Vec::new();
    let mut warnings = Vec::new();
    for result in results {
        match result {
            Ok(endpoints) => all.extend(endpoints),
            Err(warning) => warnings.push(warning),
        }
    }

    let mut skipped: HashSet<u16> = excluded_ports.iter().copied().collect();
    skipped.extend(NON_HTTP_PORTS);
    skipped.extend(
        all.iter()
            .filter(|e| is_platform_managed(e))
            .map(|e| e.port),
    );

    let mut seen = HashSet::new();
    let endpoints = all
        .into_iter()
        .filter(|endpoint| !skipped.contains(&endpoint.port))
        .filter(|endpoint| !is_non_http_service(endpoint))
        .filter(|endpoint| seen.insert(owned(endpoint.address())))
        .collect();
    EndpointScan {
        endpoints,
        warnings,
    }
}

fn is_platform_managed(endpoint: &LocalEndpoint) -> bool {
    matches!(
        &endpoint.origin,
        EndpointOrigin::Docker { container } if container.starts_with(PLATFORM_CONTAINER_PREFIX)
    )
}

fn is_non_http_service(endpoint: &LocalEndpoint) -> bool {
    matches!(
        &endpoint.origin,
        EndpointOrigin::Process { command, .. }
            if NON_HTTP_COMMANDS.iter().any(|name| command.starts_with(name))
    )
}

/// The host to probe for a socket bound to `bound`. A wildcard bind is
/// reached through the loopback of its family; a specific address (a LAN IP
/// on a multi-homed laptop) only through itself.
fn probe_host(bound: IpAddr) -> String {
    match bound {
        IpAddr::V4(v4) if v4.is_unspecified() || v4.is_loopback() => {
            Ipv4Addr::LOCALHOST.to_string()
        }
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => probe_host(IpAddr::V4(v4)),
            None if v6.is_unspecified() || v6.is_loopback() => format!("[{}]", Ipv6Addr::LOCALHOST),
            None => format!("[{v6}]"),
        },
    }
}

/// Parse a socket address host as printed by `lsof` or `docker ps`
/// (`*`, `0.0.0.0`, `[::1]`, `::`). `*` is the IPv4 wildcard.
fn parse_bound_host(host: &str) -> Option<IpAddr> {
    if host == "*" {
        return Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()
}

/// Split `host:port` at the last colon, so bracketed IPv6 hosts survive.
fn split_host_port(address: &str) -> Option<(String, u16)> {
    let (host, port) = address.rsplit_once(':')?;
    Some((probe_host(parse_bound_host(host)?), port.parse().ok()?))
}

/// How running an external listing command went.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CommandOutcome {
    /// Exited successfully; its stdout.
    Output(String),
    /// Not installed, or could not be started.
    Missing,
    /// Ran and exited unsuccessfully (e.g. Docker installed but not running);
    /// its stderr, trimmed.
    Failed(String),
    /// Still running at the deadline; it was killed.
    TimedOut,
}

/// Run `program args`, killing it if it outlives `timeout`.
pub(super) fn run_command(program: &str, args: &[&str], timeout: Duration) -> CommandOutcome {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return CommandOutcome::Missing;
    };
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return CommandOutcome::Failed(String::new());
    };
    let stdout = read_in_background(stdout);
    let stderr = read_in_background(stderr);

    let deadline = Instant::now() + timeout;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status.success()),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    match finished {
        None => CommandOutcome::TimedOut,
        Some(true) => match stdout.recv_timeout(remaining) {
            Ok(Some(text)) => CommandOutcome::Output(text),
            Ok(None) | Err(RecvTimeoutError::Disconnected) => CommandOutcome::Failed(String::new()),
            Err(RecvTimeoutError::Timeout) => CommandOutcome::TimedOut,
        },
        Some(false) => {
            let error = stderr.recv_timeout(remaining).ok().flatten();
            CommandOutcome::Failed(error.unwrap_or_default().trim().to_string())
        }
    }
}

/// Read `pipe` to the end on its own thread and hand the text over on a
/// channel, so waiting for it can be bounded by the deadline: a grandchild the
/// command spawned (a shell wrapper's `sleep`, a CLI plugin) can keep the pipe
/// open long after the command itself is gone. The thread is never joined; it
/// ends when the pipe closes or the CLI exits.
fn read_in_background(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<Option<String>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = sender.send(pipe.read_to_string(&mut text).ok().map(|_| text));
    });
    receiver
}

fn timed_out(what: &str, command: &str) -> String {
    format!(
        "{what} skipped: `{command}` did not finish within {}s",
        COMMAND_TIMEOUT.as_secs()
    )
}

// ─── Listening sockets ───────────────────────────────────────────────────────

/// TCP sockets in LISTEN state on this host. On Linux `/proc/net/tcp` lists
/// every user's sockets, including root's (Docker's port forwarder) and other
/// services' accounts; `lsof` without root sees only the current user's, but
/// names their processes. Both are read and merged. Elsewhere (macOS) `lsof`
/// is the only source.
pub struct ListeningSockets;

impl EndpointSource for ListeningSockets {
    fn endpoints(&self) -> Result<Vec<LocalEndpoint>, String> {
        let lsof = run_command(
            "lsof",
            &["-nP", "-w", "-iTCP", "-sTCP:LISTEN", "-Fpcn"],
            COMMAND_TIMEOUT,
        );
        let named = match &lsof {
            CommandOutcome::Output(output) => Some(parse_lsof(output)),
            _ => None,
        };
        match (named, proc_net_listening()) {
            (named, Some(all)) => Ok(with_names(named.unwrap_or_default(), all)),
            (Some(named), None) => Ok(named),
            (None, None) => Err(match lsof {
                CommandOutcome::TimedOut => timed_out("Local listening ports", "lsof"),
                _ => "Local listening ports skipped: `lsof` is not available".to_string(),
            }),
        }
    }
}

/// `named` (from `lsof`, with process names) plus every socket in `all` (from
/// `/proc`) at an [address](LocalEndpoint::address) `named` does not cover.
fn with_names(named: Vec<LocalEndpoint>, all: Vec<LocalEndpoint>) -> Vec<LocalEndpoint> {
    let covered: HashSet<_> = named
        .iter()
        .map(|endpoint| owned(endpoint.address()))
        .collect();
    let unnamed = all
        .into_iter()
        .filter(|endpoint| !covered.contains(&owned(endpoint.address())));
    named.into_iter().chain(unnamed).collect()
}

fn owned((port, host): (u16, Option<&str>)) -> (u16, Option<String>) {
    (port, host.map(str::to_string))
}

/// Listening sockets from `/proc/net/tcp{,6}`; `None` where those files do
/// not exist (any OS but Linux).
fn proc_net_listening() -> Option<Vec<LocalEndpoint>> {
    let tables: Vec<String> = ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect();
    if tables.is_empty() {
        return None;
    }
    let mut seen = HashSet::new();
    let endpoints = tables
        .iter()
        .flat_map(|table| parse_proc_net_tcp(table))
        .map(|(host, port)| LocalEndpoint {
            host,
            port,
            origin: EndpointOrigin::Process {
                pid: None,
                command: "-".into(),
            },
        })
        .filter(|endpoint| seen.insert(owned(endpoint.address())))
        .collect();
    Some(endpoints)
}

/// `(probe host, port)` of each LISTEN row (state `0A`) in a
/// `/proc/net/tcp{,6}` table, whose local address is `HEXIP:HEXPORT`.
fn parse_proc_net_tcp(table: &str) -> Vec<(String, u16)> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let local = fields.nth(1)?;
            if fields.nth(1)? != "0A" {
                return None;
            }
            let (ip_hex, port_hex) = local.split_once(':')?;
            let port = u16::from_str_radix(port_hex, 16).ok()?;
            Some((probe_host(parse_proc_ip(ip_hex)?), port))
        })
        .collect()
}

/// `/proc/net/tcp` prints an address as 32-bit words in host (little-endian)
/// order: `0100007F` is 127.0.0.1.
fn parse_proc_ip(hex: &str) -> Option<IpAddr> {
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    let words: Vec<u8> = bytes
        .chunks(4)
        .flat_map(|word| word.iter().rev().copied())
        .collect();
    match words.len() {
        4 => Some(IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(words).ok()?))),
        16 => Some(IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(words).ok()?,
        ))),
        _ => None,
    }
}

/// Listening endpoints from `lsof -Fpcn` field output: `p<pid>` starts a
/// process, `c<command>` names it, each `n<addr>:<port>` is one socket.
fn parse_lsof(output: &str) -> Vec<LocalEndpoint> {
    let mut endpoints = Vec::new();
    let mut seen = HashSet::new();
    let mut pid = None;
    let mut command = String::from("-");
    for line in output.lines() {
        let (tag, value) = line.split_at(line.len().min(1));
        match tag {
            "p" => {
                pid = value.parse().ok();
                command = "-".into();
            }
            "c" => command = value.to_string(),
            "n" => {
                let Some((host, port)) = split_host_port(value) else {
                    continue;
                };
                if seen.insert(port) {
                    endpoints.push(LocalEndpoint {
                        host,
                        port,
                        origin: EndpointOrigin::Process {
                            pid,
                            command: command.clone(),
                        },
                    });
                }
            }
            _ => {}
        }
    }
    endpoints
}

// ─── Docker ──────────────────────────────────────────────────────────────────

/// Host ports published by running Docker containers.
pub struct DockerPublishedPorts;

impl EndpointSource for DockerPublishedPorts {
    /// Docker not installed, or installed but not running, is normal and
    /// silent. A `docker ps` that hangs, or that the user may not run (not in
    /// the `docker` group), is a warning: containers are being missed.
    fn endpoints(&self) -> Result<Vec<LocalEndpoint>, String> {
        if which::which("docker").is_err() {
            return Ok(Vec::new());
        }
        docker_endpoints(run_command(
            "docker",
            &["ps", "--format", "{{.Names}}\t{{.Ports}}"],
            COMMAND_TIMEOUT,
        ))
    }
}

fn docker_endpoints(docker_ps: CommandOutcome) -> Result<Vec<LocalEndpoint>, String> {
    match docker_ps {
        CommandOutcome::Output(output) => Ok(parse_docker_ps(&output)),
        CommandOutcome::TimedOut => Err(timed_out("Docker containers", "docker ps")),
        CommandOutcome::Failed(error) if error.to_lowercase().contains("permission denied") => Err(
            "Docker containers skipped: permission denied talking to the Docker daemon \
             (is your user in the `docker` group?)"
                .to_string(),
        ),
        CommandOutcome::Missing | CommandOutcome::Failed(_) => Ok(Vec::new()),
    }
}

/// Most ports probed from one published range. Agents publish one port; a
/// wide range (a media server's RTP ports) is not worth hundreds of probes.
const MAX_PUBLISHED_RANGE: u16 = 32;

/// The probe host and ports of one published address as `docker ps` prints
/// it: `0.0.0.0:8080`, `[::]:8080`, or a range `0.0.0.0:8000-8002`.
fn published_ports(published: &str) -> Option<(String, std::ops::RangeInclusive<u16>)> {
    let (host, ports) = published.rsplit_once(':')?;
    let host = probe_host(parse_bound_host(host)?);
    let (first, last): (u16, u16) = match ports.split_once('-') {
        Some((first, last)) => (first.parse().ok()?, last.parse().ok()?),
        None => {
            let port = ports.parse().ok()?;
            (port, port)
        }
    };
    let last = last.min(first.saturating_add(MAX_PUBLISHED_RANGE - 1));
    (first <= last).then_some((host, first..=last))
}

/// Endpoints from `docker ps --format '{{.Names}}\t{{.Ports}}'`. A port entry
/// looks like `0.0.0.0:8080->8000/tcp`; only published TCP host ports count.
fn parse_docker_ps(output: &str) -> Vec<LocalEndpoint> {
    let mut endpoints = Vec::new();
    let mut seen = HashSet::new();
    for line in output.lines() {
        let Some((container, ports)) = line.split_once('\t') else {
            continue;
        };
        for mapping in ports.split(", ") {
            let Some((published, target)) = mapping.split_once("->") else {
                continue;
            };
            if !target.ends_with("/tcp") {
                continue;
            }
            let Some((host, ports)) = published_ports(published) else {
                continue;
            };
            for port in ports.filter(|port| seen.insert(*port)) {
                endpoints.push(LocalEndpoint {
                    host: host.clone(),
                    port,
                    origin: EndpointOrigin::Docker {
                        container: container.to_string(),
                    },
                });
            }
        }
    }
    endpoints
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Vec<LocalEndpoint>);

    impl EndpointSource for Fixed {
        fn endpoints(&self) -> Result<Vec<LocalEndpoint>, String> {
            Ok(self.0.clone())
        }
    }

    struct Broken;

    impl EndpointSource for Broken {
        fn endpoints(&self) -> Result<Vec<LocalEndpoint>, String> {
            Err("Docker containers skipped: `docker ps` did not finish within 3s".into())
        }
    }

    fn process(port: u16, command: &str) -> LocalEndpoint {
        LocalEndpoint {
            host: "127.0.0.1".into(),
            port,
            origin: EndpointOrigin::Process {
                pid: None,
                command: command.into(),
            },
        }
    }

    fn docker(port: u16, container: &str) -> LocalEndpoint {
        LocalEndpoint {
            host: "127.0.0.1".into(),
            port,
            origin: EndpointOrigin::Docker {
                container: container.into(),
            },
        }
    }

    #[test]
    fn wildcard_and_loopback_binds_are_probed_on_their_family_loopback() {
        let host = |s: &str| probe_host(parse_bound_host(s).unwrap());
        assert_eq!(host("*"), "127.0.0.1");
        assert_eq!(host("0.0.0.0"), "127.0.0.1");
        assert_eq!(host("127.0.0.1"), "127.0.0.1");
        assert_eq!(host("[::1]"), "[::1]");
        assert_eq!(host("[::]"), "[::1]");
        assert_eq!(host("[::ffff:127.0.0.1]"), "127.0.0.1");
        assert_eq!(host("192.168.1.20"), "192.168.1.20");
    }

    #[test]
    fn proc_net_tcp_yields_listening_sockets_with_their_address() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue\n\
           0: 00000000:07E8 00000000:0000 0A 00000000:00000000\n\
           1: 0100007F:1F90 0100007F:D2A4 01 00000000:00000000\n\
           2: 1401A8C0:2329 00000000:0000 0A 00000000:00000000\n\
           3: 00000000000000000000000001000000:2328 00000000000000000000000000000000:0000 0A 0\n";
        assert_eq!(
            parse_proc_net_tcp(table),
            vec![
                ("127.0.0.1".to_string(), 2024),
                ("192.168.1.20".to_string(), 9001),
                ("[::1]".to_string(), 9000),
            ]
        );
    }

    #[test]
    fn lsof_output_maps_sockets_to_their_process_and_address() {
        let output = "p101\ncpython3.12\nn127.0.0.1:2024\nn[::1]:2024\np202\ncnode\nn[::1]:3000\n";
        let endpoints = parse_lsof(output);
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].base_url(), "http://127.0.0.1:2024");
        assert_eq!(
            endpoints[0].origin,
            EndpointOrigin::Process {
                pid: Some(101),
                command: "python3.12".into()
            }
        );
        // An IPv6-only listener is probed over IPv6, not missed on 127.0.0.1.
        assert_eq!(endpoints[1].base_url(), "http://[::1]:3000");
    }

    #[test]
    fn macos_lsof_output_with_file_descriptor_lines_parses_the_same() {
        // Captured on macOS 26: `lsof -Fpcn` adds an `f<fd>` line before each
        // socket, and prints a wildcard bind as `*`.
        let output = "p4242\ncPython\nf4\nn127.0.0.1:2024\n\
                      p4343\ncPython\nf3\nn[::1]:8767\n\
                      p5151\nccom.docker.backend\nf112\nn*:8090\n";

        let endpoints = parse_lsof(output);

        let found: Vec<(String, &str)> = endpoints
            .iter()
            .map(|endpoint| match &endpoint.origin {
                EndpointOrigin::Process { command, .. } => (endpoint.base_url(), command.as_str()),
                EndpointOrigin::Docker { .. } => panic!("lsof yields processes only"),
            })
            .collect();
        assert_eq!(
            found,
            vec![
                ("http://127.0.0.1:2024".to_string(), "Python"),
                ("http://[::1]:8767".to_string(), "Python"),
                ("http://127.0.0.1:8090".to_string(), "com.docker.backend"),
            ]
        );
    }

    #[test]
    fn docker_ps_yields_published_tcp_host_ports() {
        let output = "crew-api\t0.0.0.0:8080->8000/tcp, [::]:8080->8000/tcp\n\
                      metrics\t0.0.0.0:9125->9125/udp\n\
                      worker\t8000/tcp\n";
        assert_eq!(parse_docker_ps(output), vec![docker(8080, "crew-api")]);
    }

    #[test]
    fn collect_prefers_the_first_source_and_drops_duplicates() {
        let sources: Vec<Box<dyn EndpointSource>> = vec![
            Box::new(Fixed(vec![docker(8080, "crew-api")])),
            Box::new(Fixed(vec![process(8080, "-"), process(2024, "python")])),
        ];
        assert_eq!(
            collect(&sources, &[]).endpoints,
            vec![docker(8080, "crew-api"), process(2024, "python")]
        );
    }

    fn process_at(host: &str, port: u16, command: &str) -> LocalEndpoint {
        LocalEndpoint {
            host: host.into(),
            ..process(port, command)
        }
    }

    #[test]
    fn two_processes_on_one_port_at_different_addresses_are_both_kept() {
        let sources: Vec<Box<dyn EndpointSource>> = vec![Box::new(Fixed(vec![
            process(8000, "python3"),
            process_at("192.168.1.20", 8000, "node"),
        ]))];
        assert_eq!(
            collect(&sources, &[]).endpoints,
            vec![
                process(8000, "python3"),
                process_at("192.168.1.20", 8000, "node")
            ]
        );
    }

    #[test]
    fn a_server_on_both_loopbacks_is_probed_once() {
        let sources: Vec<Box<dyn EndpointSource>> = vec![Box::new(Fixed(vec![
            process(3000, "node"),
            process_at("[::1]", 3000, "node"),
        ]))];
        assert_eq!(
            collect(&sources, &[]).endpoints,
            vec![process(3000, "node")]
        );
    }

    #[test]
    fn collect_skips_excluded_platform_and_non_http_endpoints() {
        let sources: Vec<Box<dyn EndpointSource>> = vec![
            Box::new(Fixed(vec![docker(32768, "nasiko-agent-446ee8d3")])),
            Box::new(Fixed(vec![
                process(32768, "-"),
                process(8443, "nasiko-server"),
                process(6379, "redis-ser"),
                process(16379, "redis-ser"),
                process(2024, "python"),
            ])),
        ];
        assert_eq!(
            collect(&sources, &[8443]).endpoints,
            vec![process(2024, "python")]
        );
    }

    #[test]
    fn a_source_that_fails_is_reported_and_the_others_still_count() {
        let sources: Vec<Box<dyn EndpointSource>> = vec![
            Box::new(Broken),
            Box::new(Fixed(vec![process(2024, "python")])),
        ];
        let scan = collect(&sources, &[]);
        assert_eq!(scan.endpoints, vec![process(2024, "python")]);
        assert_eq!(
            scan.warnings,
            vec!["Docker containers skipped: `docker ps` did not finish within 3s"]
        );
    }

    #[test]
    fn a_missing_command_yields_no_output() {
        let timeout = Duration::from_secs(1);
        assert_eq!(
            run_command("nasiko-no-such-command", &[], timeout),
            CommandOutcome::Missing
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_command_is_killed_at_the_deadline() {
        let started = Instant::now();
        assert_eq!(
            run_command("sleep", &["5"], Duration::from_millis(100)),
            CommandOutcome::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn the_deadline_holds_when_a_grandchild_keeps_the_pipe_open() {
        let started = Instant::now();
        assert_eq!(
            run_command("sh", &["-c", "sleep 5"], Duration::from_millis(100)),
            CommandOutcome::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn a_finished_command_returns_its_stdout() {
        let output = run_command("echo", &["ok"], Duration::from_secs(2));
        assert_eq!(output, CommandOutcome::Output("ok\n".into()));
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_exits_unsuccessfully_is_a_failure_not_output() {
        let output = run_command("false", &[], Duration::from_secs(2));
        assert_eq!(output, CommandOutcome::Failed(String::new()));
    }

    // ─── Against the real machine ───────────────────────────────────────────
    // `lsof` (or `/proc/net/tcp` without it) listing real sockets.

    #[cfg(unix)]
    #[test]
    fn a_listening_socket_on_this_machine_is_listed() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let endpoints = ListeningSockets.endpoints().expect("listing should work");

        let ours = endpoints.iter().find(|endpoint| endpoint.port == port);
        assert_eq!(
            ours.map(LocalEndpoint::base_url),
            Some(format!("http://127.0.0.1:{port}")),
            "port {port} not in {endpoints:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_ipv6_only_listener_is_probed_over_ipv6() {
        let Ok(listener) = std::net::TcpListener::bind("[::1]:0") else {
            return; // No IPv6 loopback on this machine.
        };
        let port = listener.local_addr().unwrap().port();

        let endpoints = ListeningSockets.endpoints().expect("listing should work");

        let ours = endpoints.iter().find(|endpoint| endpoint.port == port);
        assert_eq!(
            ours.map(LocalEndpoint::base_url),
            Some(format!("http://[::1]:{port}"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_child_left_holding_the_output_cannot_stretch_the_deadline() {
        // The command exits at once, but a background child keeps its stdout
        // open (a CLI plugin, a wrapper's helper).
        let started = Instant::now();

        let outcome = run_command(
            "sh",
            &["-c", "echo partial; sleep 5 &"],
            Duration::from_millis(200),
        );

        assert_eq!(outcome, CommandOutcome::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn docker_port_ranges_are_expanded_up_to_a_cap() {
        let output = "crew-api\t0.0.0.0:8000-8002->8000-8002/tcp\n\
                      media\t0.0.0.0:10000-20000->10000-20000/tcp\n";

        let endpoints = parse_docker_ps(output);

        let crew: Vec<u16> = endpoints
            .iter()
            .filter(|endpoint| {
                endpoint.origin
                    == EndpointOrigin::Docker {
                        container: "crew-api".into(),
                    }
            })
            .map(|endpoint| endpoint.port)
            .collect();
        assert_eq!(crew, vec![8000, 8001, 8002]);
        assert_eq!(endpoints.len(), 3 + MAX_PUBLISHED_RANGE as usize);
    }

    #[test]
    fn docker_denied_is_a_warning_but_docker_stopped_is_silent() {
        let denied = CommandOutcome::Failed(
            "permission denied while trying to connect to the Docker daemon socket".into(),
        );
        let stopped = CommandOutcome::Failed(
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?".into(),
        );

        assert!(
            docker_endpoints(denied)
                .unwrap_err()
                .contains("docker` group")
        );
        assert_eq!(docker_endpoints(stopped), Ok(Vec::new()));
    }

    #[test]
    fn sockets_lsof_cannot_see_are_still_probed() {
        // `lsof` named this user's agent; `/proc` also lists root's Docker
        // forwarder and the agent again.
        let named = vec![process(8000, "python3")];
        let all = vec![
            process(8000, "-"),
            process(8090, "-"),
            process_at("10.0.0.5", 8000, "-"),
        ];

        assert_eq!(
            with_names(named, all),
            vec![
                process(8000, "python3"),
                process(8090, "-"),
                process_at("10.0.0.5", 8000, "-")
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_command_reports_its_error_output() {
        assert_eq!(
            run_command("sh", &["-c", "echo denied >&2; exit 1"], COMMAND_TIMEOUT),
            CommandOutcome::Failed("denied".into())
        );
    }
}
