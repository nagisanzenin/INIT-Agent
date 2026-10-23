//! EV-06 (the `trace:` reader and the failure-bearing predicate on a real trace store), EV-08
//! (proposal evidence rules), EV-09 (the INV/DOM path rule), EV-11 and EV-13 (kernel-built
//! variants: apply, conflict, duplicate, dependency), on scratch homes under
//! `kernel/target/test-homes/EVO-LITE/`.

use kernel_evo::mine::{self, MineCfg};
use kernel_evo::proposal::{self, Filer};
use kernel_evo::{trace, variant};
use kernel_gate::evo::trailers::GitTrailerParser;
use kernel_proxy::trace::{EndJob, RequestBody, StartJob, TraceWriter, TracesCfg};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn home(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/EVO-LITE").join(format!("{name}-{}", std::process::id()));
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn remove(d: &Path) {
    assert!(d.to_string_lossy().contains("/target/test-homes/EVO-LITE/"));
    let _ = std::fs::remove_dir_all(d);
}

fn req_id(n: u64) -> String {
    format!("{:012x}{:04x}", 1_760_918_400_000u64 + n * 1000, n)
}

/// One stored request: `messages` = `msgs`, with the start fields `extra`.
fn put(w: &TraceWriter, n: u64, msgs: &[Value], extra: Value) -> String {
    let id = req_id(n);
    let elems: Vec<String> = msgs.iter().map(|m| m.to_string()).collect();
    let mut bytes = b"{\"messages\":[".to_vec();
    let list_start = bytes.len() - 1;
    let mut ranges = Vec::new();
    for (i, e) in elems.iter().enumerate() {
        if i > 0 {
            bytes.push(b',');
        }
        ranges.push((bytes.len(), bytes.len() + e.len()));
        bytes.extend_from_slice(e.as_bytes());
    }
    bytes.push(b']');
    let list_end = bytes.len();
    bytes.extend_from_slice(b",\"model\":\"deepseek-flash\"}");
    let mut fields = Map::new();
    fields.insert("req_id".into(), json!(id));
    fields.insert("ts".into(), json!("2025-10-20T00:00:00.000Z"));
    fields.insert("scope".into(), json!("live"));
    fields.insert("vis".into(), json!("agent"));
    fields.insert("job_id".into(), Value::Null);
    if let Value::Object(x) = extra {
        fields.extend(x);
    }
    let session = fields.get("session").and_then(Value::as_str).unwrap_or("_none").to_string();
    w.start_record(StartJob { req_id: id.clone(), chain_key: format!("live\0{session}"), fields, body: Some(RequestBody { bytes: Arc::from(bytes), list: Some((list_start, list_end)), elements: ranges }) });
    let mut end = Map::new();
    end.insert("req_id".into(), json!(id));
    end.insert("ts".into(), json!("2025-10-20T00:00:01.000Z"));
    end.insert("status".into(), json!(200));
    w.end_record(EndJob { req_id: id.clone(), fields: end, response: None });
    id
}

fn turn(id: &str, cmd: &str, result: &str) -> Vec<Value> {
    vec![
        json!({"role": "system", "content": "s"}),
        json!({"role": "user", "content": "go"}),
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function", "function": {"name": "bash", "arguments": json!({"command": cmd}).to_string()}}]}),
        json!({"role": "tool", "tool_call_id": id, "content": result}),
    ]
}

/// Three failing requests (two sessions), a clean one, a kernel-visible one.
fn store(h: &Path) -> (String, String, String, String, String) {
    let root = h.join("vault/traces/v1");
    std::fs::create_dir_all(&root).unwrap();
    let w = TraceWriter::start(&root, &TracesCfg::default()).unwrap();
    let missing = "bash: line 1: cd: work/a1-calc: No such file or directory\n[exit_code=1 wall_time=0.01s]";
    let a = put(&w, 1, &turn("c1", "cd work/a1-calc && ls", missing), json!({"session": "20251020-000000-aaaaaa"}));
    let a2 = put(&w, 2, &turn("c2", "cd work/a1-calc && make", missing), json!({"session": "20251020-000000-aaaaaa"}));
    let b = put(&w, 3, &turn("c3", "cd work/a1-calc && ls", missing), json!({"session": "20251020-000001-bbbbbb"}));
    let clean = put(&w, 4, &turn("c4", "ls", "x\n[exit_code=0 wall_time=0.01s]"), json!({"session": "20251020-000001-bbbbbb"}));
    let hidden = put(&w, 5, &turn("c5", "cd work/a1-calc", missing), json!({"session": "20251020-000002-cccccc", "vis": "kernel"}));
    w.flush();
    w.shutdown();
    (a, a2, b, clean, hidden)
}

#[test]
fn ev06_trace_reader_and_predicate() {
    let h = home("ev06");
    let (a, _, _, clean, hidden) = store(&h);
    let root = h.join("vault/traces/v1");
    let v = trace::view(&root, &a).unwrap().expect("agent-visible");
    assert_eq!(v.session.as_deref(), Some("20251020-000000-aaaaaa"));
    assert!(trace::failure(&v).is_some_and(|w| w.starts_with("bash: exit 1")), "{:?}", trace::failure(&v));
    assert!(trace::failure(&trace::view(&root, &clean).unwrap().unwrap()).is_none(), "a clean request");
    assert!(trace::view(&root, &hidden).unwrap().is_none(), "vis:kernel is never read");
    let lookup = trace::lookup(root.clone());
    assert!(lookup(&a).unwrap() && lookup(&clean).unwrap());
    assert!(!lookup(&hidden).unwrap() && !lookup("0123456789abcdef").unwrap() && !lookup("nope").unwrap());
    remove(&h);
}

fn parser(h: &Path) -> GitTrailerParser {
    let e = h.join("cache/evo-empty");
    std::fs::create_dir_all(&e).unwrap();
    GitTrailerParser::new("/usr/bin/git", e)
}

fn draft(f: &str, repro: Value) -> Vec<u8> {
    json!({"v": 1, "scope": "inv", "frictions": [f], "hypothesis": "the model writes INIT_HOME-relative paths from work/",
           "change": "resolve the doubled work/ prefix", "repro": repro, "predicts": ["pass_rate >= parent+15%"],
           "subject": "fix(tools): resolve doubled work/ prefixes", "compensates_for": "none (test)", "tier_hint": "t2"})
    .to_string()
    .into_bytes()
}

#[test]
fn ev08_evidence_rules() {
    let h = home("ev08");
    let (a, a2, b, clean, hidden) = store(&h);
    let cfg = MineCfg { live_sessions: false, pool: false, ..MineCfg::for_home(&h) };
    let out = mine::mine(&h, &cfg).unwrap();
    let f = out.clusters.iter().find(|c| c.signature == "path.doubled_prefix").unwrap_or_else(|| panic!("{:#?}", out.clusters));
    assert!(f.eligible && f.counts.sessions == 2 && f.counts.instances == 3, "{f:#?}");
    assert!(f.refs.iter().all(|r| r.starts_with("trace:")));
    let codes = |repro: Value| -> Vec<String> {
        match proposal::propose(&h, &draft(&f.id, repro), &Filer::default(), &parser(&h)) {
            Ok((p, _)) => vec![format!("ok {}", p.id)],
            Err(e) => e.into_iter().map(|p| p.code).collect(),
        }
    };
    // Two event: refs only; one attested ref; two attested refs from one session.
    assert!(codes(json!(["event:20251020-000000-aaaaaa#0000000000000001", "event:20251020-000000-aaaaaa#0000000000000002"])).contains(&"evo.evidence_unattested".to_string()));
    assert!(codes(json!([format!("trace:{a}")])).contains(&"evo.evidence_missing".to_string()));
    assert!(codes(json!([format!("trace:{a}"), format!("trace:{a2}")])).contains(&"evo.evidence_missing".to_string()));
    // Not failure-bearing; not visible.
    assert!(codes(json!([format!("trace:{a}"), format!("trace:{clean}")])).contains(&"evo.not_failure".to_string()));
    assert!(codes(json!([format!("trace:{a}"), format!("trace:{hidden}")])).contains(&"evo.evidence_missing".to_string()));
    // Two sessions: filed; so is one trace plus the eligible friction.
    assert_eq!(codes(json!([format!("trace:{a}"), format!("trace:{b}")])), ["ok p-000001"]);
    assert_eq!(codes(json!([format!("trace:{a2}"), format!("friction:{}", f.id)])), ["ok p-000002"]);
    let p = proposal::load(&h, "p-000001").unwrap().unwrap();
    assert_eq!((p.state.as_str(), p.s_measurable), ("proposed", true));
    assert_eq!(mine::boot_summary(&h)["proposals_open"], 2);
    remove(&h);
}

#[test]
fn ev09_scope_rule() {
    let kind = |s: &str| if s == "domx" { "dom".to_string() } else { "inv".to_string() };
    let paths = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(variant::scope_problems("dom", &paths(&["src/tools/read.rs"]), &kind)[0].code, "evo.scope_paths");
    assert_eq!(variant::scope_problems("inv", &paths(&["skills/domx/SKILL.md"]), &kind)[0].code, "evo.scope_paths");
    assert!(variant::scope_problems("dom", &paths(&["skills/domx/SKILL.md", "self.json"]), &kind).is_empty());
    assert!(variant::scope_problems("inv", &paths(&["src/tools/read.rs", "skills/evolve/SKILL.md"]), &kind).is_empty());
    assert_eq!(variant::skill_kind("---\nname: x\nkind: dom\n---\nbody"), "dom");
    assert_eq!(variant::skill_kind("no frontmatter"), "inv");
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

#[test]
fn ev11_ev13_kernel_built_variants() {
    let h = home("ev11");
    let (a, _, b, _, _) = store(&h);
    let cfg = MineCfg { live_sessions: false, pool: false, ..MineCfg::for_home(&h) };
    let f = mine::mine(&h, &cfg).unwrap().clusters.into_iter().find(|c| c.signature == "path.doubled_prefix").unwrap();
    // Lineage with one commit on main.
    let src = h.join("cache/src");
    std::fs::create_dir_all(src.join("src")).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    std::fs::write(src.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(src.join("Cargo.lock"), "version = 4\n").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "seed"]);
    std::fs::create_dir_all(h.join("lineage")).unwrap();
    git(&h.join("lineage"), &["clone", "-q", "--bare", src.to_str().unwrap(), "body.git"]);
    let (p, _) = proposal::propose(&h, &draft(&f.id, json!([format!("trace:{a}"), format!("trace:{b}")])), &Filer::default(), &parser(&h)).unwrap_or_else(|e| panic!("{e:#?}"));
    let g = variant::Git::new("/usr/bin/git", &h);
    let patch = |body: &str| format!("diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1,2 @@\n pub fn a() {{}}\n+{body}\n");
    let v1 = variant::build(&h, &g, &p.id, 1, &patch("pub fn b() {}"), &parser(&h)).unwrap_or_else(|e| panic!("{e:#?}"));
    assert_eq!(v1.variant.git_ref, format!("refs/init/evo/{}/v1", p.id));
    assert_eq!(git(&h.join("lineage/body.git"), &["rev-parse", &v1.variant.git_ref]), v1.variant.sha);
    assert!(v1.variant.s_measurable, "src/ is t2");
    assert_eq!(git(&h.join("lineage/body.git"), &["log", "-1", "--format=%an", &v1.variant.sha]), "INIT Agent");
    // The same lines again; a patch that does not apply; vendor/ and a lockfile package.
    assert_eq!(variant::build(&h, &g, &p.id, 2, &patch("pub fn b() {}"), &parser(&h)).unwrap_err()[0].code, "evo.duplicate_variant");
    let conflict = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn zzz() {}\n+pub fn y() {}\n";
    assert_eq!(variant::build(&h, &g, &p.id, 2, conflict, &parser(&h)).unwrap_err()[0].code, "evo.patch_apply");
    let vendor = "diff --git a/vendor/x/lib.rs b/vendor/x/lib.rs\nnew file mode 100644\n--- /dev/null\n+++ b/vendor/x/lib.rs\n@@ -0,0 +1 @@\n+x\n";
    assert!(variant::build(&h, &g, &p.id, 2, vendor, &parser(&h)).unwrap_err().iter().any(|e| e.code == "evo.dep"));
    let lock = "diff --git a/Cargo.lock b/Cargo.lock\n--- a/Cargo.lock\n+++ b/Cargo.lock\n@@ -1 +1,6 @@\n version = 4\n+\n+[[package]]\n+name = \"evil\"\n+version = \"1.0.0\"\n+source = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    assert!(variant::build(&h, &g, &p.id, 2, lock, &parser(&h)).unwrap_err().iter().any(|e| e.code == "evo.dep"));
    // A variant ref is create-only.
    assert_eq!(variant::build(&h, &g, &p.id, 1, &patch("pub fn c() {}"), &parser(&h)).unwrap_err()[0].code, "evo.state");
    remove(&h);
}

/// ADR-011 decision 1 (X11's need): on the free path `predicts` is optional in `evo propose`; the
/// variant's message then has no `Predicts:` trailer and passes the free matrix only. The gated path
/// still wants 1 to 5 clauses, and a clause given on the free path must still parse.
#[test]
fn free_path_predicts_optional() {
    use kernel_gate::evo::ExtendMode::{Free, Gated};
    let h = home("free-predicts");
    let (a, _, b, _, _) = store(&h);
    let cfg = MineCfg { live_sessions: false, pool: false, ..MineCfg::for_home(&h) };
    let f = mine::mine(&h, &cfg).unwrap().clusters.into_iter().find(|c| c.signature == "path.doubled_prefix").unwrap();
    let src = h.join("cache/src");
    std::fs::create_dir_all(src.join("src")).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    std::fs::write(src.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(src.join("Cargo.lock"), "version = 4\n").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "seed"]);
    std::fs::create_dir_all(h.join("lineage")).unwrap();
    git(&h.join("lineage"), &["clone", "-q", "--bare", src.to_str().unwrap(), "body.git"]);
    let with = |predicts: Value| {
        let mut d: Value = serde_json::from_slice(&draft(&f.id, json!([format!("trace:{a}"), format!("trace:{b}")]))).unwrap();
        d["predicts"] = predicts;
        d.to_string().into_bytes()
    };
    let gated = proposal::propose_in(&h, &with(json!([])), &Filer::default(), &parser(&h), Gated).unwrap_err();
    assert!(gated.iter().any(|p| p.detail.contains("predicts: 1 to 5")), "{gated:#?}");
    let bad = proposal::propose_in(&h, &with(json!(["not a clause"])), &Filer::default(), &parser(&h), Free).unwrap_err();
    assert!(bad.iter().any(|p| p.detail.starts_with("predicts[0]")), "a clause given must parse: {bad:#?}");
    let six = proposal::propose_in(&h, &with(json!(vec!["pass_rate >= parent+1%"; 6])), &Filer::default(), &parser(&h), Free).unwrap_err();
    assert!(six.iter().any(|p| p.detail.contains("at most 5")), "{six:#?}");
    let (p, _) = proposal::propose_in(&h, &with(json!([])), &Filer::default(), &parser(&h), Free).unwrap_or_else(|e| panic!("{e:#?}"));
    assert!(p.predicts.is_empty());
    let msg = proposal::commit_message(&p, 1, &[]);
    assert!(!msg.contains("Predicts:"), "{msg}");
    let g = variant::Git::new("/usr/bin/git", &h);
    let patch = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1,2 @@\n pub fn a() {}\n+pub fn b() {}\n";
    let gated = variant::build_in(&h, &g, &p.id, 1, patch, &parser(&h), Gated).unwrap_err();
    assert!(gated.iter().any(|e| e.detail.contains("Predicts")), "the gated matrix still needs Predicts: {gated:#?}");
    let v = variant::build_in(&h, &g, &p.id, 1, patch, &parser(&h), Free).unwrap_or_else(|e| panic!("{e:#?}"));
    let body = git(&h.join("lineage/body.git"), &["log", "-1", "--format=%B", &v.variant.sha]);
    assert!(!body.contains("Predicts:") && body.contains("Why:"), "{body}");
    remove(&h);
}
