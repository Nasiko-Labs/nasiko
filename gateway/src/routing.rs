use crate::config::RouteConfig;

/// Resolves a request path to an upstream backend based on configured routes.
pub struct Router {
    routes: Vec<ResolvedRoute>,
}

#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub path_prefix: String,
    pub upstream_host: String,
    pub upstream_port: u16,
    pub strip_prefix: bool,
    pub require_auth: bool,
    pub required_role: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RouteMatch {
    pub upstream_host: String,
    pub upstream_port: u16,
    pub rewritten_path: String,
    pub require_auth: bool,
    pub required_role: Option<String>,
}

impl Router {
    pub fn new(routes: &[RouteConfig]) -> Self {
        let resolved = routes
            .iter()
            .map(|r| {
                let (host, port) = parse_upstream(&r.upstream);
                ResolvedRoute {
                    path_prefix: r.path_prefix.clone(),
                    upstream_host: host,
                    upstream_port: port,
                    strip_prefix: r.strip_prefix,
                    require_auth: r.require_auth,
                    required_role: r.required_role.clone(),
                }
            })
            .collect();
        Self { routes: resolved }
    }

    /// Find the matching route for a given path. Returns None if no route matches.
    pub fn resolve(&self, path: &str) -> Option<RouteMatch> {
        // Longest-prefix match
        let mut best: Option<&ResolvedRoute> = None;
        for route in &self.routes {
            if path.starts_with(&route.path_prefix)
                && (best.is_none()
                    || route.path_prefix.len() > best.unwrap().path_prefix.len())
                {
                    best = Some(route);
                }
        }

        best.map(|route| {
            let rewritten_path = if route.strip_prefix {
                let stripped = path.strip_prefix(&route.path_prefix).unwrap_or(path);
                if stripped.starts_with('/') {
                    stripped.to_string()
                } else {
                    format!("/{}", stripped)
                }
            } else {
                path.to_string()
            };

            RouteMatch {
                upstream_host: route.upstream_host.clone(),
                upstream_port: route.upstream_port,
                rewritten_path,
                require_auth: route.require_auth,
                required_role: route.required_role.clone(),
            }
        })
    }
}

fn parse_upstream(upstream: &str) -> (String, u16) {
    if let Some((host, port_str)) = upstream.rsplit_once(':') {
        let port = port_str.parse().unwrap_or(80);
        (host.to_string(), port)
    } else {
        (upstream.to_string(), 80)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RouteConfig;

    fn routes() -> Vec<RouteConfig> {
        vec![
            RouteConfig {
                path_prefix: "/api/".into(),
                upstream: "cp-service:8080".into(),
                strip_prefix: false,
                require_auth: true,
                required_role: None,
            },
            RouteConfig {
                path_prefix: "/api/admin/".into(),
                upstream: "admin-service:9090".into(),
                strip_prefix: true,
                require_auth: true,
                required_role: Some("admin".into()),
            },
            RouteConfig {
                path_prefix: "/health".into(),
                upstream: "cp-service:8080".into(),
                strip_prefix: false,
                require_auth: false,
                required_role: None,
            },
            RouteConfig {
                path_prefix: "/.well-known/".into(),
                upstream: "cp-service:8080".into(),
                strip_prefix: false,
                require_auth: false,
                required_role: None,
            },
        ]
    }

    #[test]
    fn matches_longest_prefix() {
        let router = Router::new(&routes());

        let m = router.resolve("/api/admin/users").unwrap();
        assert_eq!(m.upstream_host, "admin-service");
        assert_eq!(m.upstream_port, 9090);
        assert_eq!(m.required_role, Some("admin".into()));
    }

    #[test]
    fn strips_prefix_when_configured() {
        let router = Router::new(&routes());

        let m = router.resolve("/api/admin/users").unwrap();
        assert_eq!(m.rewritten_path, "/users");
    }

    #[test]
    fn preserves_path_when_no_strip() {
        let router = Router::new(&routes());

        let m = router.resolve("/api/containers").unwrap();
        assert_eq!(m.rewritten_path, "/api/containers");
        assert_eq!(m.upstream_host, "cp-service");
        assert_eq!(m.upstream_port, 8080);
    }

    #[test]
    fn matches_health_endpoint() {
        let router = Router::new(&routes());

        let m = router.resolve("/health").unwrap();
        assert!(!m.require_auth);
    }

    #[test]
    fn no_match_returns_none() {
        let router = Router::new(&routes());

        let m = router.resolve("/unknown/path");
        assert!(m.is_none());
    }

    #[test]
    fn well_known_no_auth() {
        let router = Router::new(&routes());

        let m = router.resolve("/.well-known/agent-card.json").unwrap();
        assert!(!m.require_auth);
    }

    #[test]
    fn parses_upstream_with_port() {
        let (host, port) = parse_upstream("backend:3000");
        assert_eq!(host, "backend");
        assert_eq!(port, 3000);
    }

    #[test]
    fn parses_upstream_without_port() {
        let (host, port) = parse_upstream("backend");
        assert_eq!(host, "backend");
        assert_eq!(port, 80);
    }
}
