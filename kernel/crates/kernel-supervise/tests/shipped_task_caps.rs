//! K-GATE release check: every task that ships with v0.1 has a cost cap at or above one model
//! request's reservation for the shipped default model. A lower cap makes G4 refuse to start
//! (`gate.g4.task_cost_cap`, kernel-gate `g4::task_cap_problem`), so every real promotion would fail.
//!
//! The floor is computed by the proxy's own `pricing::request_reservation_pusd` over the model block
//! that `scripts/install.sh` writes into a real install's `kernel/config.toml` (its template, with the
//! default model's prices and `reserve_output_tokens`). The tasks are what v0.1 ships or generates:
//! - the stand-in sealed S (12 tasks and the canary, FIX5: `root/.../fixtures/sealed/S-src/`);
//! - any task under `seed/` (the body checkout; none today);
//! - the kernel's `initctl task template` (init-kernel `serve.rs`);
//! - the Root's sealed-task default cap (init-root `sealed/task.rs`);
//! - gate.md's import default (`budget.task_usd` at L0).

use std::fs;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The `[providers.deepseek.*]` block install.sh writes for `model`, with the template filled in.
fn shipped_proxy_config(model: &str) -> String {
    let sh = read("scripts/install.sh");
    let start = sh.find("[providers.deepseek.schedule]").expect("install.sh: the schedule block");
    let end = sh[start..].find("prices.offpeak = {prices[1]}").map(|i| start + i + "prices.offpeak = {prices[1]}".len()).expect("install.sh: the model block");
    let line = sh.lines().find(|l| l.trim_start().starts_with("prices = {\"deepseek-flash\": ('{")).expect("install.sh: the TOML price table");
    let quoted = |l: &str, key: &str| -> (String, String) {
        let at = l.find(&format!("\"{key}\": (")).unwrap_or_else(|| panic!("install.sh: prices for {key}"));
        let parts: Vec<&str> = l[at..].split('\'').collect();
        (parts[1].to_string(), parts[3].to_string())
    };
    let next = sh.lines().skip_while(|l| *l != line).nth(1).unwrap_or("");
    let (peak, offpeak) = if line.contains(&format!("\"{model}\"")) { quoted(line, model) } else { quoted(next, model) };
    let block = sh[start..end].replace("{{", "{").replace("}}", "}").replace("{model}", model).replace("{prices[0]}", &peak).replace("{prices[1]}", &offpeak);
    format!("[model]\nprovider = \"deepseek\"\nid = \"{model}\"\n\n{block}\n")
}

/// `(where, cap)` for every shipped task cap.
fn shipped_caps() -> Vec<(String, u64)> {
    let mut v = Vec::new();
    // The stand-in sealed S.
    let s = repo().join("root/crates/init-root/tests/fixtures/sealed/S-src");
    for e in fs::read_dir(&s).unwrap().flatten() {
        let t = e.path().join("task.json");
        if t.is_file() {
            let j: serde_json::Value = serde_json::from_slice(&fs::read(&t).unwrap()).unwrap();
            v.push((format!("sealed S {}", e.file_name().to_string_lossy()), j["limits"]["max_cost_musd"].as_u64().unwrap_or(0)));
        }
    }
    // Any task in the seed tree.
    fn walk(d: &Path, out: &mut Vec<(String, u64)>) {
        for e in fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            let n = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() && !["target", "vendor", ".git"].contains(&n.as_str()) {
                walk(&p, out);
            } else if n == "task.json" {
                let j: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap_or_default();
                if let Some(c) = j["limits"]["max_cost_musd"].as_u64() {
                    out.push((format!("seed {}", p.display()), c));
                }
            }
        }
    }
    walk(&repo().join("seed"), &mut v);
    // `initctl task template`: every cap in init-kernel's template.
    let serve = read("kernel/crates/init-kernel/src/serve.rs");
    let caps = numbers_after(&serve, "\"max_cost_musd\": ");
    assert!(!caps.is_empty(), "serve.rs: the task template's cap");
    v.extend(caps.into_iter().map(|c| ("initctl task template".to_string(), c)));
    // The Root's default for a sealed task without limits.
    let root = read("root/crates/init-root/src/sealed/task.rs");
    let caps = numbers_after(&root, "[\"max_cost_musd\"], ");
    assert_eq!(caps.len(), 1, "sealed/task.rs: the default cap");
    v.push(("init-root sealed task default".into(), caps[0]));
    // gate.md's import default (budget.task_usd at L0).
    let g = read("docs/kernel/gate.md");
    let caps = numbers_after(&g, "the L0 default is $0.50, so ");
    assert_eq!(caps.len(), 1, "gate.md: the import default");
    v.push(("gate.md import default".into(), caps[0]));
    v
}

/// The integer (digits with `_` or `,` separators) after each occurrence of `key`.
fn numbers_after(text: &str, key: &str) -> Vec<u64> {
    text.match_indices(key)
        .filter_map(|(i, _)| {
            let n: String = text[i + key.len()..].chars().take_while(|c| c.is_ascii_digit() || *c == '_' || *c == ',').filter(char::is_ascii_digit).collect();
            n.parse().ok()
        })
        .collect()
}

#[test]
fn every_shipped_task_cap_clears_one_request_reservation() {
    let cfg: kernel_proxy::ProxyConfig = toml::from_str(&shipped_proxy_config("deepseek-flash")).expect("install.sh's proxy block parses");
    let k = cfg.compile().expect("install.sh's proxy block compiles");
    let m = k.model("deepseek", "deepseek-flash").expect("the default model is priced");
    let floor = kernel_proxy::money::pusd_to_musd(kernel_proxy::pricing::request_reservation_pusd(m));
    assert_eq!(m.reserve_output_tokens, 65_536, "the shipped reserve_output_tokens (the seed's max_tokens)");
    assert_eq!(floor, 78_643, "65,536 output tokens at deepseek-flash's peak $1.20/Mtok");
    let caps = shipped_caps();
    assert!(caps.iter().filter(|c| c.0.starts_with("sealed S")).count() >= 13, "the stand-in S: 12 tasks and the canary: {caps:?}");
    let low: Vec<&(String, u64)> = caps.iter().filter(|(_, c)| *c < floor).collect();
    assert!(low.is_empty(), "shipped task caps below one request's reservation ({floor} µUSD): {low:?}");
}
