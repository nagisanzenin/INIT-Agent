//! Owner-only experiment policy, loaded once before Root starts any zone process.
//! No product dollar cap lives here. This restricts a benchmark to its external
//! gateway and freezes admitted capabilities during control/transfer phases.
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub v: u32,
    pub run_id: String,
    pub phase: String,
    pub gateway_host: String,
    pub provider: String,
    pub model: String,
    pub request_max_tokens: u32,
}
#[derive(Clone, Debug)]
pub struct Policy { pub spec: Spec, pub fingerprint: String }
impl Policy {
    pub fn frozen(&self) -> bool { self.spec.phase != "learn" }
    pub fn view(&self) -> Value {
        let mut v = serde_json::to_value(&self.spec).expect("serializable policy");
        v["frozen"] = json!(self.frozen());
        v["policy_sha256"] = json!(self.fingerprint);
        v
    }
    pub fn request(&self, method: &str, provider: Option<&str>, dest: &str, path: &str, body: &[u8]) -> Result<(), String> {
        if method != "POST" || provider != Some(self.spec.provider.as_str()) || dest != self.spec.gateway_host
            || !matches!(path, "/chat/completions" | "/v1/chat/completions") {
            return Err("benchmark permits only its external guarded chat gateway".into());
        }
        let v: Value = serde_json::from_slice(body).map_err(|_| "invalid benchmark request")?;
        if v["model"].as_str() != Some(self.spec.model.as_str())
            || !v["max_tokens"].as_u64().is_some_and(|n| n > 0 && n <= u64::from(self.spec.request_max_tokens)) {
            return Err("benchmark model or explicit output limit mismatch".into());
        }
        Ok(())
    }
}
fn parse(raw: &[u8]) -> io::Result<Policy> {
    let spec: Spec = serde_json::from_slice(raw).map_err(|e| util::bad(format!("benchmark policy: {e}")))?;
    let port = spec.gateway_host.strip_prefix("127.0.0.1:").and_then(|p| p.parse::<u16>().ok());
    if spec.v != 1 || spec.run_id.is_empty() || spec.run_id.len() > 96
        || !spec.run_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || !matches!(spec.phase.as_str(), "genesis" | "learn" | "transfer")
        || port.is_none_or(|p| p == 0) || spec.provider != "deepseek" || spec.model != "deepseek-flash"
        || !(1..=4096).contains(&spec.request_max_tokens) {
        return Err(util::bad("invalid benchmark policy fields"));
    }
    Ok(Policy { spec, fingerprint: util::sha256_hex(raw) })
}
pub fn load(rh: &Path) -> io::Result<Option<Policy>> {
    let path = rh.join("benchmark.json");
    let mut f = match OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let m = f.metadata()?;
    // The Root home is owner-controlled; require the sidecar to have that same owner.
    if !m.is_file() || m.uid() != std::fs::metadata(rh)?.uid() || m.mode() & 0o077 != 0 || m.len() > 8192 {
        return Err(util::bad("benchmark policy must be an owner-only regular file <=8192 bytes"));
    }
    let mut raw = Vec::new();
    (&mut f).take(8193).read_to_end(&mut raw)?;
    if raw.len() > 8192 { return Err(util::bad("oversized benchmark policy")); }
    parse(&raw).map(Some)
}
pub fn deny_change(policy: Option<&Policy>) -> Result<(), crate::zone::Err3> {
    if policy.is_some_and(Policy::frozen) {
        Err(("root.benchmark_frozen", "owner benchmark policy freezes admitted capabilities".into(), Value::Null))
    } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn spec() -> Value { json!({"v":1,"run_id":"trial-1","phase":"genesis","gateway_host":"127.0.0.1:8123","provider":"deepseek","model":"deepseek-flash","request_max_tokens":4096}) }
    #[test]
    fn benchmark_policy_freeze_and_route_are_fail_closed() {
        let raw = serde_json::to_vec(&spec()).unwrap();
        let p = parse(&raw).unwrap();
        assert_eq!(p.view()["policy_sha256"], util::sha256_hex(&raw));
        assert!(deny_change(Some(&p)).is_err());
        let b = br#"{"model":"deepseek-flash","max_tokens":4096}"#;
        assert!(p.request("POST", Some("deepseek"), "127.0.0.1:8123", "/chat/completions", b).is_ok());
        for (method, provider, host, path) in [("GET", "deepseek", "127.0.0.1:8123", "/chat/completions"), ("POST", "other", "127.0.0.1:8123", "/chat/completions"), ("POST", "deepseek", "api.deepseek.com:443", "/chat/completions"), ("POST", "deepseek", "127.0.0.1:8123", "/responses")] {
            assert!(p.request(method, Some(provider), host, path, b).is_err());
        }
        assert!(p.request("POST", Some("deepseek"), "127.0.0.1:8123", "/chat/completions", br#"{"model":"deepseek-flash","max_tokens":4097}"#).is_err());
        let mut s = spec(); s["phase"] = json!("learn");
        assert!(deny_change(Some(&parse(&serde_json::to_vec(&s).unwrap()).unwrap())).is_ok());
        s["phase"] = json!("unknown"); assert!(parse(&serde_json::to_vec(&s).unwrap()).is_err());
        s = spec(); s["gateway_host"] = json!("evil.example:443"); assert!(parse(&serde_json::to_vec(&s).unwrap()).is_err());
    }
    #[test]
    fn benchmark_policy_rejects_insecure_sidecars_and_is_immutable_after_load() {
        let home = std::env::temp_dir().join(format!("init-benchmark-policy-{}", std::process::id()));
        std::fs::create_dir(&home).unwrap();
        let path = home.join("benchmark.json");
        assert!(load(&home).unwrap().is_none());
        std::fs::write(&path, serde_json::to_vec(&spec()).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&home).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let cached = load(&home).unwrap().unwrap();
        let mut changed = spec(); changed["phase"] = json!("learn");
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(cached.frozen());
        assert!(!load(&home).unwrap().unwrap().frozen());
        std::fs::remove_file(&path).unwrap();
        let target = home.join("policy-target");
        std::fs::write(&target, serde_json::to_vec(&spec()).unwrap()).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &path).unwrap();
        assert!(load(&home).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&target).unwrap();
        std::fs::remove_dir(&home).unwrap();
    }
}
