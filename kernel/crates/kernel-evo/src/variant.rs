//! Kernel-built variants (evolution-v2 §3.5.1, D-E6): `initctl evo variant <p> <k> <patch>`
//! applies the worker's unified diff to lineage `main` in a kernel-owned scratch index
//! (`cache/evo/<p>/index`, never an agent-writable tree, K-F3), writes the tree, commits it with
//! `commit-tree` as `INIT Agent <agent@init.local>` with the proposal's message, and creates
//! `refs/init/evo/<p>/v<k>` (kernel-only, create-only, never `main`).
//!
//! Refusals: `evo.state`, `evo.patch_apply`, `evo.duplicate_variant` (changed-line Jaccard ≥ 0.8
//! with an earlier variant), `evo.scope_paths` (the INV/DOM path rule), `evo.dep` (vendor/ or a
//! lockfile dependency change: forbidden for the engine at L0).

use crate::mine::{self, Cluster};
use crate::proposal::{self, codes, Problem, Proposal, Variant};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A patch is at most 200 KiB (§3.5.1).
pub const MAX_PATCH_BYTES: usize = 200 * 1024;
/// Diversity threshold (Harness² `tree.diversity`).
pub const JACCARD_MAX_PERMILLE: u64 = 800;
/// supervise.md §3.11.2: every kernel git command has a 120 s timeout.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// The `-c` options of the hardened prefix `$G` (supervise.md §3.11.2).
const HARDENED_CONFIG: [&str; 12] = [
    "core.hooksPath=/dev/null",
    "core.fsmonitor=false",
    "core.attributesFile=/dev/null",
    "protocol.allow=never",
    "protocol.file.allow=always",
    "transfer.fsckObjects=true",
    "maintenance.auto=false",
    "gc.auto=0",
    "core.logAllRefUpdates=true",
    "user.useConfigOnly=true",
    "user.name=INIT Kernel",
    "user.email=kernel@init.local",
];

/// The hardened `$G` on `lineage/body.git`.
#[derive(Clone, Debug)]
pub struct Git {
    pub git: PathBuf,
    pub git_dir: PathBuf,
    pub git_home: PathBuf,
}

impl Git {
    /// The standard layout under `$INIT_HOME`.
    pub fn new(git: impl Into<PathBuf>, home: &Path) -> Git {
        Git { git: git.into(), git_dir: home.join("lineage/body.git"), git_home: home.join("cache/git-home") }
    }

    fn cmd(&self, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut c = Command::new(&self.git);
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.git_home)
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_REPLACE_OBJECTS", "1");
        for (k, v) in env {
            c.env(k, v);
        }
        for kv in HARDENED_CONFIG {
            c.arg("-c").arg(kv);
        }
        c.arg(format!("--git-dir={}", self.git_dir.display())).args(args).current_dir(&self.git_dir);
        c
    }

    /// Runs `$G args` with `stdin`; `(status ok, stdout, stderr)`.
    pub fn run(&self, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> io::Result<(bool, String, String)> {
        let _ = std::fs::create_dir_all(&self.git_home);
        let mut c = self.cmd(args, env);
        c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn()?;
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let data = data.to_vec();
            std::thread::spawn(move || {
                let _ = pipe.write_all(&data);
            });
        }
        let mut so = child.stdout.take();
        let mut se = child.stderr.take();
        let t_out = std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(p) = so.as_mut() {
                let _ = p.read_to_end(&mut b);
            }
            b
        });
        let t_err = std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(p) = se.as_mut() {
                let _ = p.read_to_end(&mut b);
            }
            b
        });
        let deadline = Instant::now() + GIT_TIMEOUT;
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break s;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(io::ErrorKind::TimedOut, format!("git {} timed out", args.join(" "))));
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let out = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
        let err = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
        Ok((status.success(), out, err))
    }

    fn ok(&self, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<String, Problem> {
        match self.run(args, env, stdin) {
            Ok((true, out, _)) => Ok(out.trim().to_string()),
            Ok((false, _, err)) => Err(Problem::new(codes::IO, format!("git {}: {}", args.first().unwrap_or(&""), err.trim()))),
            Err(e) => Err(Problem::new(codes::IO, format!("git {}: {e}", args.first().unwrap_or(&"")))),
        }
    }

    /// `rev-parse --verify -q <rev>^{commit}`.
    pub fn commit_of(&self, rev: &str) -> Option<String> {
        match self.run(&["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")], &[], None) {
            Ok((true, out, _)) => Some(out.trim().to_string()),
            _ => None,
        }
    }

    fn blob(&self, rev: &str, path: &str) -> Option<String> {
        match self.run(&["show", &format!("{rev}:{path}")], &[], None) {
            Ok((true, out, _)) => Some(out),
            _ => None,
        }
    }
}

/// The paths a unified diff touches (both sides of renames), from `diff --git` / `---` / `+++`.
pub fn patch_paths(patch: &str) -> Result<Vec<String>, String> {
    let mut out = BTreeSet::new();
    for l in patch.lines() {
        let p = l.strip_prefix("+++ ").or_else(|| l.strip_prefix("--- "));
        let Some(p) = p else { continue };
        let p = p.split('\t').next().unwrap_or(p).trim();
        if p == "/dev/null" {
            continue;
        }
        let p = p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p);
        if p.starts_with('/') || p.split('/').any(|c| c == ".." || c == ".git") || p.is_empty() {
            return Err(format!("unsafe path '{p}' in the patch"));
        }
        out.insert(p.to_string());
    }
    Ok(out.into_iter().collect())
}

/// The changed lines (`+`/`-` lines without the file headers), for the diversity check.
pub fn changed_lines(patch: &str) -> BTreeSet<String> {
    patch
        .lines()
        .filter(|l| (l.starts_with('+') && !l.starts_with("+++")) || (l.starts_with('-') && !l.starts_with("---")))
        .map(str::to_string)
        .collect()
}

/// Jaccard similarity in permille.
pub fn jaccard_permille(a: &BTreeSet<String>, b: &BTreeSet<String>) -> u64 {
    let union = a.union(b).count() as u64;
    if union == 0 {
        return 1000;
    }
    a.intersection(b).count() as u64 * 1000 / union
}

/// The `kind:` of a skill's SKILL.md frontmatter (`inv` when absent; selfmap.md §10).
pub fn skill_kind(text: &str) -> String {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return "inv".into();
    }
    for l in lines {
        let l = l.trim();
        if l == "---" {
            break;
        }
        if let Some(v) = l.strip_prefix("kind:") {
            return v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
        }
    }
    "inv".into()
}

/// The INV/DOM path rule (§3.4.7 "Scope rule"). `kind_of(skill)` reads a skill's `kind` (on the
/// candidate tree, so a new skill's own frontmatter counts).
pub fn scope_problems(scope: &str, paths: &[String], kind_of: &dyn Fn(&str) -> String) -> Vec<Problem> {
    let mut out = Vec::new();
    for p in paths.iter().filter(|p| p.as_str() != "self.json") {
        let skill = p.strip_prefix("skills/").and_then(|r| r.split('/').next()).filter(|s| !s.is_empty());
        match (scope, skill) {
            ("dom", None) => out.push(Problem::new(codes::SCOPE_PATHS, format!("{p}: a dom proposal changes only skills/<name>/** of kind:dom skills"))),
            ("dom", Some(s)) if kind_of(s) != "dom" => out.push(Problem::new(codes::SCOPE_PATHS, format!("{p}: skill {s} is not kind: dom"))),
            ("inv", Some(s)) if kind_of(s) == "dom" => out.push(Problem::new(codes::SCOPE_PATHS, format!("{p}: an inv proposal may not change the kind: dom skill {s}"))),
            _ => {}
        }
    }
    out
}

/// The result of `evo variant`.
#[derive(Clone, Debug)]
pub struct Built {
    pub proposal: Proposal,
    pub variant: Variant,
    /// The kernel record (§4.7 `proposal`).
    pub record: Value,
}

/// `evo variant <p> <k> <patch>` (the gated path's message rules; [`build_in`]).
pub fn build(home: &Path, git: &Git, id: &str, k: u32, patch: &str, parser: &dyn kernel_gate::evo::trailers::TrailerParse) -> Result<Built, Vec<Problem>> {
    build_in(home, git, id, k, patch, parser, kernel_gate::evo::ExtendMode::Gated)
}

/// [`build`] with the variant's commit message dry-run under `mode` (ADR-011: on the free path a
/// proposal without `predicts` writes no `Predicts:` trailer, which only the free matrix accepts).
pub fn build_in(home: &Path, git: &Git, id: &str, k: u32, patch: &str, parser: &dyn kernel_gate::evo::trailers::TrailerParse, mode: kernel_gate::evo::ExtendMode) -> Result<Built, Vec<Problem>> {
    let one = |p: Problem| vec![p];
    let mut p = proposal::load(home, id).map_err(|e| one(Problem::new(codes::IO, e.to_string())))?.ok_or_else(|| one(Problem::new(codes::STATE, format!("{id}: no such proposal"))))?;
    if p.state != "proposed" && p.state != "variants" {
        return Err(one(Problem::new(codes::STATE, format!("{id} is {}; variants are added while it is proposed", p.state))));
    }
    if k == 0 || k > p.k {
        return Err(one(Problem::new(codes::STATE, format!("k must be 1..={} for {id}", p.k))));
    }
    let r = format!("refs/init/evo/{id}/v{k}");
    if p.variants.iter().any(|v| v.k == k) || git.commit_of(&r).is_some() {
        return Err(one(Problem::new(codes::STATE, format!("variant {k} of {id} exists ({r} is create-only)"))));
    }
    if patch.len() > MAX_PATCH_BYTES {
        return Err(one(Problem::new(codes::PATCH_APPLY, format!("the patch is {} bytes; at most {MAX_PATCH_BYTES}", patch.len()))));
    }
    let paths = patch_paths(patch).map_err(|e| one(Problem::new(codes::PATCH_APPLY, e)))?;
    if paths.is_empty() {
        return Err(one(Problem::new(codes::PATCH_APPLY, "the patch changes nothing")));
    }
    let mut probs = Vec::new();
    if paths.iter().any(|x| x.starts_with("vendor/") || x.starts_with("ext/vendor/")) {
        probs.push(Problem::new(codes::DEP, "a variant may not touch vendor/ (dependency changes are not allowed for the engine at L0)"));
    }
    // Diversity against the earlier variants' patches.
    let cache = home.join("cache/evo").join(id);
    std::fs::create_dir_all(&cache).map_err(|e| one(Problem::new(codes::IO, e.to_string())))?;
    let lines = changed_lines(patch);
    for v in &p.variants {
        if let Ok(prev) = std::fs::read_to_string(cache.join(format!("v{}.patch", v.k))) {
            let j = jaccard_permille(&lines, &changed_lines(&prev));
            if j >= JACCARD_MAX_PERMILLE {
                probs.push(Problem::new(codes::DUPLICATE_VARIANT, format!("changed lines are {j}‰ similar to variant {} (limit {JACCARD_MAX_PERMILLE}‰); submit a real alternative or fewer variants", v.k)));
            }
        }
    }
    let main = git.commit_of("refs/heads/main").ok_or_else(|| one(Problem::new(codes::IO, "lineage has no refs/heads/main")))?;
    // Apply in the kernel's scratch index.
    let index = cache.join("index");
    let _ = std::fs::remove_file(&index);
    let idx = index.to_string_lossy().into_owned();
    let env = [("GIT_INDEX_FILE", idx.as_str())];
    git.ok(&["read-tree", &main], &env, None).map_err(one)?;
    let pfile = cache.join(format!("v{k}.patch.try"));
    std::fs::write(&pfile, patch).map_err(|e| one(Problem::new(codes::IO, e.to_string())))?;
    let pf = pfile.to_string_lossy().into_owned();
    match git.run(&["apply", "--cached", "--whitespace=nowarn", &pf], &env, None) {
        Ok((true, _, _)) => {}
        Ok((false, _, err)) => probs.push(Problem::new(codes::PATCH_APPLY, format!("the patch does not apply to main {}: {}", &main[..12], crate::classify::cap(err.trim(), 3500)))),
        Err(e) => probs.push(Problem::new(codes::IO, format!("git apply: {e}"))),
    }
    if !probs.is_empty() {
        let _ = std::fs::remove_file(&pfile);
        let _ = std::fs::remove_file(&index);
        return Err(probs);
    }
    let tree = git.ok(&["write-tree"], &env, None).map_err(one)?;
    let _ = std::fs::remove_file(&index);
    // Scope rule on the candidate tree.
    let kind_of = |skill: &str| git.blob(&tree, &format!("skills/{skill}/SKILL.md")).or_else(|| git.blob(&main, &format!("skills/{skill}/SKILL.md"))).map(|t| skill_kind(&t)).unwrap_or_else(|| "inv".into());
    probs.extend(scope_problems(&p.scope, &paths, &kind_of));
    // The gate's own scope decision (gate.md §3.1), for `dep` and S-measurability.
    let numstat = git.ok(&["diff", "--numstat", "--no-renames", &main, &tree], &[], None).map_err(one)?;
    let mut changes = Vec::new();
    for l in numstat.lines() {
        let mut it = l.splitn(3, '\t');
        let (a, d, path) = (it.next().unwrap_or("-"), it.next().unwrap_or("-"), it.next().unwrap_or(""));
        let old_lines = git.blob(&main, path).map(|b| b.lines().count() as u64).unwrap_or(0);
        changes.push(kernel_gate::scope::FileChange { path: path.to_string(), added: a.parse().ok(), deleted: d.parse().ok(), old_lines });
    }
    let (bp, bc, ep, ec) = (git.blob(&main, "Cargo.lock"), git.blob(&tree, "Cargo.lock"), git.blob(&main, "ext/Cargo.lock"), git.blob(&tree, "ext/Cargo.lock"));
    let locks = kernel_gate::scope::Lockfiles { body_parent: bp.as_deref(), body_candidate: bc.as_deref(), ext_parent: ep.as_deref(), ext_candidate: ec.as_deref() };
    let scope_name = match kernel_gate::scope::classify(&changes, &locks) {
        Ok((d, dep)) => {
            let name = d.scope.as_str().to_string();
            if dep || name == "dep" {
                probs.push(Problem::new(codes::DEP, "the variant changes a dependency (lockfile); not allowed for the engine at L0"));
            }
            name
        }
        Err(e) => {
            probs.push(Problem::new(codes::DEP, format!("lockfile: {}", e.0)));
            "dep".into()
        }
    };
    if !probs.is_empty() {
        let _ = std::fs::remove_file(&pfile);
        return Err(probs);
    }
    // The message, dry-run through the validator's message checks.
    let index_now: Vec<Cluster> = mine::read_index(home);
    let msg = proposal::commit_message(&p, k, &index_now);
    if let Err(errs) = kernel_gate::evo::validate_message_in(parser, &msg, mode) {
        let _ = std::fs::remove_file(&pfile);
        return Err(errs.into_iter().map(|e| Problem::new(codes::SCHEMA, format!("commit message: {}: {}", e.code, e.detail))).collect());
    }
    let ident = [
        ("GIT_AUTHOR_NAME", "INIT Agent"),
        ("GIT_AUTHOR_EMAIL", "agent@init.local"),
        ("GIT_COMMITTER_NAME", "INIT Agent"),
        ("GIT_COMMITTER_EMAIL", "agent@init.local"),
    ];
    let sha = git.ok(&["commit-tree", &tree, "-p", &main, "-F", "-"], &ident, Some(msg.as_bytes())).map_err(one)?;
    // Create-only: the old value must be absent (40 zeros).
    git.ok(&["update-ref", "-m", &format!("init: evo variant {id} v{k}"), &r, &sha, "0000000000000000000000000000000000000000"], &[], None).map_err(one)?;
    let _ = std::fs::rename(&pfile, cache.join(format!("v{k}.patch")));
    let stat = git.ok(&["diff", "--shortstat", &main, &sha], &[], None).unwrap_or_default();
    let s_measurable = scope_name == "t2";
    let variant = Variant { k, git_ref: r.clone(), sha: sha.clone(), paths: paths.clone(), diff_stat: stat.clone(), s_measurable, screen: None };
    p.variants.push(variant.clone());
    p.variants.sort_by_key(|v| v.k);
    let record = p.transition("variants", json!({"k": k, "ref": r, "sha": sha, "scope": scope_name, "s_measurable": s_measurable, "diff_stat": stat}));
    proposal::store(home, &p).map_err(|e| one(Problem::new(codes::IO, e.to_string())))?;
    Ok(Built { proposal: p, variant, record })
}
