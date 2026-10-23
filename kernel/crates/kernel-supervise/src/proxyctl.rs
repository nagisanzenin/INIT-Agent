//! The proxy seam (supervise.md §3.2; kernel/proxy.md Q-SV1). The proxy lives in its own lane
//! crate (`kernel-proxy`, tokio); the `init-kernel` binary links both and hands supervise a
//! [`ProxyCtl`], so this crate never depends on the proxy and a broken lane never blocks another
//! lane's tests (BUILD_PLAN.md "Shape").
//!
//! Supervise's duties: start the proxy with the boot block's `paths.egress_sock`, relay the
//! level and ceilings, label the live scope and give it each body's Root instance id
//! (`x-egress-body`, root/egress.md §4.1), and shut it down on `stop`/`prepare_swap`. It never
//! calls a panic API: an owner `/panic` never reaches the kernel (§3.8.1).

use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// `{sha, boot_reason, model, level}` of a scope (kernel/proxy.md §3.4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyLabel {
    pub sha: String,
    pub boot_reason: String,
    pub model: String,
    pub level: u8,
}

/// A proxy alert for `alerts.jsonl` and the Root (`source:"proxy"`).
pub type ProxyAlertSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// What supervise needs from the proxy.
pub trait ProxyCtl: Send + Sync {
    /// `Proxy::start`. `cfg` is the proxy's config subset (`[model]`, `[proxy]`, `[budget]`,
    /// `[traces]`, `[providers.*]`, `soft_day_fraction`). Errors are config errors (exit 78).
    fn start(&self, cfg: &Value, home: &Path, egress_sock: &Path, label: &ProxyLabel, alerts: ProxyAlertSink) -> Result<(), Vec<String>>;
    fn set_level(&self, level: u8);
    /// The `ceilings.get` result or a `ceilings` event, verbatim.
    fn set_ceilings(&self, ceilings: &Value);
    fn set_label(&self, scope: &str, label: &ProxyLabel);
    /// `x-egress-body` of a scope: the body's Root instance id.
    fn set_body(&self, scope: &str, instance: &str);
    fn shutdown(&self, grace: Duration);
    /// The scope's `/_init/stats` object (kernel/proxy.md §4.3), when a proxy runs. The owner's
    /// `state` frame reads `spend.task.today_musd` from the live scope (tui.md §9 Q2).
    fn stats(&self, _scope: &str) -> Option<Value> {
        None
    }
}

/// No proxy (tests, and `sv-kernel`): every call succeeds and does nothing.
pub struct NoProxy;

impl ProxyCtl for NoProxy {
    fn start(&self, _: &Value, _: &Path, _: &Path, _: &ProxyLabel, _: ProxyAlertSink) -> Result<(), Vec<String>> {
        Ok(())
    }
    fn set_level(&self, _: u8) {}
    fn set_ceilings(&self, _: &Value) {}
    fn set_label(&self, _: &str, _: &ProxyLabel) {}
    fn set_body(&self, _: &str, _: &str) {}
    fn shutdown(&self, _: Duration) {}
}

fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f).map(Value::Number).unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Value::Object(t.iter().map(|(k, v)| (k.clone(), toml_to_json(v))).collect()),
    }
}

/// The proxy's subset of `config.toml` as JSON (floats allowed here: this never reaches ICJ).
/// A missing file is `{}` (every section defaulted by the proxy).
pub fn proxy_config(path: &Path) -> Result<Value, Vec<String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Object(Default::default())),
        Err(e) => return Err(vec![format!("config.toml: {e}")]),
    };
    let t: toml::Table = toml::from_str(&text).map_err(|e| vec![format!("config.toml: {e}")])?;
    let mut out = serde_json::Map::new();
    for k in ["model", "proxy", "budget", "traces", "providers"] {
        if let Some(v) = t.get(k) {
            out.insert(k.to_string(), toml_to_json(v));
        }
    }
    if let Some(f) = t.get("evolution").and_then(|e| e.get("soft_day_fraction")) {
        out.insert("soft_day_fraction".into(), toml_to_json(f));
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    #[test]
    fn subset_extraction() {
        let d = crate::testutil::test_dir("proxycfg");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("config.toml");
        std::fs::write(&p, "schema_version = 2\n[model]\nprovider = \"mock\"\nid = \"m\"\n[proxy]\ntest_mode = true\n[supervise]\nsettle_s = 1\n[evolution]\nsoft_day_fraction = 0.8\n").unwrap();
        let v = super::proxy_config(&p).unwrap();
        assert_eq!(v["model"]["id"], "m");
        assert_eq!(v["proxy"]["test_mode"], true);
        assert!(v.get("supervise").is_none() && v.get("schema_version").is_none());
        assert_eq!(v["soft_day_fraction"].as_f64(), Some(0.8));
        assert_eq!(super::proxy_config(&d.join("missing.toml")).unwrap(), serde_json::json!({}));
        crate::testutil::remove_test_dir(&d);
    }
}
