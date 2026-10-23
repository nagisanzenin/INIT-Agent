//! Proxy-generated errors and the typed 429 cap format (kernel/proxy.md §4.2).

use serde_json::{Map, Value, json};
use std::fmt;

/// An error response the proxy generates itself.
#[derive(Clone, Debug, PartialEq)]
pub struct ProxyError {
    pub status: u16,
    pub code: String,
    pub message: String,
    /// `init_cap_exceeded` (a kernel cap or a mapped Root money refusal) or `init_proxy_error`.
    pub cap_type: bool,
    /// `x-init-cap`.
    pub cap: Option<String>,
    /// The top-level `init` object (ICJ integers).
    pub init: Map<String, Value>,
    /// `retry-after` seconds.
    pub retry_after_s: Option<u64>,
}

impl ProxyError {
    /// A non-cap proxy error (`type: init_proxy_error`).
    pub fn new(status: u16, code: &str, message: impl Into<String>) -> ProxyError {
        let mut init = Map::new();
        init.insert("code".into(), json!(code));
        init.insert("retryable".into(), json!(false));
        ProxyError {
            status,
            code: code.to_string(),
            message: message.into(),
            cap_type: false,
            cap: None,
            init,
            retry_after_s: None,
        }
    }

    /// A cap refusal: HTTP 429, `type: init_cap_exceeded`, `x-init-cap: <cap>`.
    pub fn cap(
        code: &str,
        cap: &str,
        init: Map<String, Value>,
        retry_after_s: Option<u64>,
        message: String,
    ) -> ProxyError {
        ProxyError {
            status: 429,
            code: code.to_string(),
            message,
            cap_type: true,
            cap: Some(cap.to_string()),
            init,
            retry_after_s,
        }
    }

    /// The error's JSON body, shaped by the route family (§4.2).
    pub fn body(&self, anthropic_shaped: bool) -> Value {
        let ty = if self.cap_type {
            "init_cap_exceeded"
        } else {
            "init_proxy_error"
        };
        if anthropic_shaped {
            json!({"type": "error", "error": {"type": ty, "message": self.message}, "init": Value::Object(self.init.clone())})
        } else {
            json!({"error": {"code": self.code, "message": self.message, "param": null, "type": ty},
                   "init": Value::Object(self.init.clone())})
        }
    }

    /// Status, headers and body bytes.
    pub fn render(&self, anthropic_shaped: bool) -> (u16, Vec<(&'static str, String)>, Vec<u8>) {
        let mut h = vec![
            ("content-type", "application/json".to_string()),
            ("x-init-error", self.code.clone()),
        ];
        if let Some(c) = &self.cap {
            h.push(("x-init-cap", c.clone()));
        }
        if let Some(s) = self.retry_after_s {
            h.push(("retry-after", s.to_string()));
        }
        let body = serde_json::to_vec(&self.body(anthropic_shaped)).unwrap_or_default();
        (self.status, h, body)
    }
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.status, self.code, self.message)
    }
}

impl std::error::Error for ProxyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let mut init = Map::new();
        init.insert("cap".into(), json!("day"));
        init.insert("limit_musd".into(), json!(5_000_000));
        let e = ProxyError::cap(
            "budget.day",
            "day",
            init,
            Some(38_521),
            "daily budget reached".into(),
        );
        let (st, h, b) = e.render(false);
        assert_eq!(st, 429);
        assert!(h.contains(&("x-init-cap", "day".into())));
        assert!(h.contains(&("retry-after", "38521".into())));
        let v: Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["error"]["type"], "init_cap_exceeded");
        assert_eq!(v["error"]["code"], "budget.day");
        assert_eq!(v["init"]["limit_musd"], 5_000_000);
        let a = e.body(true);
        assert_eq!(a["type"], "error");
        assert_eq!(a["error"]["type"], "init_cap_exceeded");
        assert_eq!(a["init"]["cap"], "day");
        let p = ProxyError::new(403, "route.not_allowed", "x");
        assert_eq!(p.body(false)["error"]["type"], "init_proxy_error");
        assert!(!p.render(false).1.iter().any(|(k, _)| *k == "x-init-cap"));
    }
}
