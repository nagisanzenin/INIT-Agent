//! Route allowlists and the routing algorithm (kernel/proxy.md §3.5).

use crate::errors::ProxyError;
use crate::tap::Family;

/// A built-in route table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Deepseek,
    Anthropic,
    Openai,
}

/// One allowlisted (method, upstream path) pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub method: String,
    /// Upstream path without the leading `/`.
    pub path: String,
    pub family: Family,
    pub metered: bool,
}

fn r(method: &str, path: &str, family: Family, metered: bool) -> Route {
    Route {
        method: method.into(),
        path: path.into(),
        family,
        metered,
    }
}

impl Preset {
    pub fn parse(s: &str) -> Option<Preset> {
        match s {
            "deepseek" => Some(Preset::Deepseek),
            "anthropic" => Some(Preset::Anthropic),
            "openai" => Some(Preset::Openai),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Preset::Deepseek => "deepseek",
            Preset::Anthropic => "anthropic",
            Preset::Openai => "openai",
        }
    }
    /// proxy.md §3.5 "Built-in route tables".
    pub fn routes(self) -> Vec<Route> {
        use Family::*;
        match self {
            Preset::Deepseek => vec![
                r("POST", "chat/completions", OpenAiChat, true),
                r("POST", "beta/chat/completions", OpenAiChat, true),
                r("POST", "responses", OpenAiResponses, true),
                r("POST", "anthropic/v1/messages", Anthropic, true),
                r("GET", "models", None, false),
                r("GET", "user/balance", None, false),
            ],
            Preset::Anthropic => vec![
                r("POST", "v1/messages", Anthropic, true),
                r("POST", "v1/messages/count_tokens", None, false),
                r("GET", "v1/models", None, false),
            ],
            Preset::Openai => vec![
                r("POST", "v1/responses", OpenAiResponses, true),
                r("POST", "v1/chat/completions", OpenAiChat, true),
                r("GET", "v1/models", None, false),
            ],
        }
    }
    /// Which error shape a `none`-family route of this preset uses (§4.2).
    pub fn anthropic_shaped(self) -> bool {
        self == Preset::Anthropic
    }
}

/// `..`, `//`, `%2e`, `%2f` (case-insensitive) anywhere in the path.
pub fn bad_path(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    l.contains("..") || l.contains("//") || l.contains("%2e") || l.contains("%2f")
}

fn valid_provider_segment(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// Where a request goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Routed {
    /// `GET /_init/*`, answered by the proxy.
    Local(String),
    Upstream {
        provider: String,
        /// Upstream path without the leading `/` (e.g. `chat/completions`).
        rest: String,
        /// `/<provider>/<rest>[?query]` on the egress socket.
        egress_path: String,
        route: Route,
    },
}

/// The provider facts `route` needs.
pub trait RouteTable {
    /// `(enabled in kernel config, routes)` of a configured provider.
    fn provider(&self, id: &str) -> Option<(bool, &[Route])>;
    /// Does the Root forward this provider?
    fn root_forwards(&self, id: &str) -> bool;
}

/// proxy.md §3.5 "Routing algorithm". `path` is the request path without the query.
pub fn route(
    method: &str,
    path: &str,
    query: Option<&str>,
    t: &dyn RouteTable,
) -> Result<Routed, ProxyError> {
    if let Some(rest) = path.strip_prefix("/_init/") {
        return Ok(Routed::Local(rest.to_string()));
    }
    let trimmed = path.trim_start_matches('/');
    let Some((pid, rest)) = trimmed.split_once('/') else {
        return Err(ProxyError::new(
            404,
            "route.unknown",
            "no route: expected /<provider_id>/<path>",
        ));
    };
    if !valid_provider_segment(pid) {
        return Err(ProxyError::new(
            404,
            "route.unknown",
            "no route: bad provider id",
        ));
    }
    let Some((enabled, routes)) = t.provider(pid) else {
        return Err(ProxyError::new(
            404,
            "route.unknown_provider",
            format!("unknown provider {pid:?}"),
        ));
    };
    if !enabled || !t.root_forwards(pid) {
        return Err(ProxyError::new(
            503,
            "provider.disabled",
            format!("provider {pid:?} is disabled or not forwarded by the Root"),
        ));
    }
    if bad_path(rest) {
        return Err(ProxyError::new(
            400,
            "route.bad_path",
            "path contains .., //, %2e or %2f",
        ));
    }
    let Some(route) = routes.iter().find(|r| r.method == method && r.path == rest) else {
        return Err(ProxyError::new(
            403,
            "route.not_allowed",
            format!("{method} {rest} is not an allowed route of {pid:?}"),
        ));
    };
    let q = query.filter(|q| !q.is_empty());
    if q.is_some() && method == "POST" {
        return Err(ProxyError::new(
            400,
            "route.query_not_allowed",
            "a query string is not allowed on POST routes",
        ));
    }
    let egress_path = match q {
        Some(q) => format!("/{pid}/{rest}?{q}"),
        None => format!("/{pid}/{rest}"),
    };
    Ok(Routed::Upstream {
        provider: pid.to_string(),
        rest: rest.to_string(),
        egress_path,
        route: route.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct T(BTreeMap<String, (bool, Vec<Route>)>, Vec<String>);
    impl RouteTable for T {
        fn provider(&self, id: &str) -> Option<(bool, &[Route])> {
            self.0.get(id).map(|(e, r)| (*e, r.as_slice()))
        }
        fn root_forwards(&self, id: &str) -> bool {
            self.1.iter().any(|x| x == id)
        }
    }

    fn table() -> T {
        let mut m = BTreeMap::new();
        m.insert("deepseek".to_string(), (true, Preset::Deepseek.routes()));
        m.insert("anthropic".to_string(), (true, Preset::Anthropic.routes()));
        m.insert("openai".to_string(), (true, Preset::Openai.routes()));
        m.insert("off".to_string(), (false, Preset::Deepseek.routes()));
        T(
            m,
            vec![
                "deepseek".into(),
                "anthropic".into(),
                "openai".into(),
                "off".into(),
            ],
        )
    }

    fn code(r: Result<Routed, ProxyError>) -> (u16, String) {
        let e = r.unwrap_err();
        (e.status, e.code)
    }

    /// P-01.
    #[test]
    fn p01_routing_table() {
        let t = table();
        for p in [Preset::Deepseek, Preset::Anthropic, Preset::Openai] {
            for rt in p.routes() {
                let path = format!("/{}/{}", p.as_str(), rt.path);
                match route(&rt.method, &path, None, &t).unwrap() {
                    Routed::Upstream {
                        egress_path, route, ..
                    } => {
                        assert_eq!(egress_path, path);
                        assert_eq!(route, rt);
                    }
                    other => panic!("{other:?}"),
                }
            }
        }
        assert_eq!(
            route("GET", "/_init/stats", None, &t).unwrap(),
            Routed::Local("stats".into())
        );
        assert_eq!(
            code(route("POST", "/nope/chat/completions", None, &t)),
            (404, "route.unknown_provider".into())
        );
        assert_eq!(
            code(route("POST", "/deepseek", None, &t)),
            (404, "route.unknown".into())
        );
        assert_eq!(
            code(route("GET", "/deepseek/chat/completions", None, &t)),
            (403, "route.not_allowed".into())
        );
        assert_eq!(
            code(route("POST", "/deepseek/v1/files", None, &t)),
            (403, "route.not_allowed".into())
        );
        assert_eq!(
            code(route("POST", "/deepseek/../chat/completions", None, &t)),
            (400, "route.bad_path".into())
        );
        assert_eq!(
            code(route("POST", "/deepseek/%2E%2e/x", None, &t)),
            (400, "route.bad_path".into())
        );
        assert_eq!(
            code(route("POST", "/deepseek/a//b", None, &t)),
            (400, "route.bad_path".into())
        );
        assert_eq!(
            code(route("POST", "/deepseek/chat/completions", Some("a=1"), &t)),
            (400, "route.query_not_allowed".into())
        );
        assert_eq!(
            code(route("POST", "/off/chat/completions", None, &t)),
            (503, "provider.disabled".into())
        );
        match route("GET", "/deepseek/models", Some("x=1"), &t).unwrap() {
            Routed::Upstream { egress_path, .. } => assert_eq!(egress_path, "/deepseek/models?x=1"),
            other => panic!("{other:?}"),
        }
        let mut t2 = table();
        t2.1.retain(|x| x != "openai");
        assert_eq!(
            code(route("POST", "/openai/v1/responses", None, &t2)),
            (503, "provider.disabled".into())
        );
    }
}
