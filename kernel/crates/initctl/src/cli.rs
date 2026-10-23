//! Argument parsing: `initctl [--sock PATH] [--json] [--quiet] [--timeout SECS] <command> [args]`
//! (kernel/initctl.md §4.5) → one validated [`Command`].

use crate::gitsnap;
use init_proto::errors::exit;
use init_proto::initctl::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Global options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Opts {
    pub sock: Option<PathBuf>,
    pub json: bool,
    pub quiet: bool,
    pub timeout: Option<u64>,
}

/// What to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    Run(Opts, Command),
    Help,
    Version,
}

/// A local failure before anything is sent: exit code plus message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliError {
    pub code: u8,
    pub message: String,
}

impl CliError {
    pub fn usage(m: impl Into<String>) -> CliError {
        CliError { code: exit::USAGE, message: m.into() }
    }
}

pub const USAGE: &str = "\
usage: initctl [--sock PATH] [--json] [--quiet] [--timeout SECS] <command> [args]

  status | ping | level
  reload [--wait]
  gate --quick [<path>|<ref>] [--no-wait]   default: the git toplevel of the current directory
  promote <evo/id> [--wait]
  promote --sha <40 hex> [--wait]             L2: adopt a qualified member SHA
  rollback [--reason TEXT] [--wait]
  vendor <crate>@<version> [--no-wait] [--workspace body|ext]
  task template | task submit <dir> [--no-wait] | task list | task show <family>   (L1)
  budget set <key>=<value>...                 (L1)
  model set <provider>/<id>                   (L1)
  fleet list
  fleet spawn [--parent SHA] [--agent NAME] --prompt-file FILE [--share-musd-day N] (L2; L0 on the free path)
  fleet send <bid> --text TEXT                (L2)
  fleet stop (<bid> | --all) [--reason TEXT]
  kernel propose <patch-file> --title TEXT [--rationale-file FILE]   (alias: propose-kernel)
  kernel promote <kevo/id> [--wait]           (L3)
  kernel status                               (L3)
  wait (<job> | --last [--cmd NAME]) [--timeout SECS]
  job status <job> | job wait <job> [--timeout SECS] | job list [--all] | job cancel <job>
  evo frictions [--all]                       the friction index: top-10 digest, or every cluster
  evo propose <draft.json>                    file a proposal (skills/evolve/SKILL.md)
  evo variant <p-id> <k> <patch>              a kernel-built variant commit from a unified diff
  evo screen <p-id> [--wait]                  reject-only screen of the variants, est, go/no-go
  evo calibrate [--repeats R] [--limit N] [--wait]  the LKG on the visible pool: seeds the pool cache and the miner
  evo status [<p-id>] | evo abandon <p-id> <reason>
  grant <kind> <value> [--hours H] --why TEXT   ask the owner for a grant (ADR-011 §7.4; at most 3 per 24 h)

stdout: one JSON document (the result, or {\"error\": ...}); progress on stderr.
exit: 0 ok, 1 failed, 2 timed out, 64 usage, 69 unavailable, 70 internal, 75 temporary,
      76 protocol, 77 not permitted.";

/// Flags that take a value.
const VALUE_FLAGS: &[&str] = &[
    "--sock",
    "--repeats",
    "--limit",
    "--timeout",
    "--reason",
    "--sha",
    "--parent",
    "--prompt-file",
    "--share-musd-day",
    "--text",
    "--title",
    "--rationale-file",
    "--cmd",
    "--workspace",
    "--hours",
    "--why",
];
/// Flags without a value.
const BOOL_FLAGS: &[&str] = &["--json", "--quiet", "--wait", "--no-wait", "--quick", "--last", "--all", "--help", "--version"];
const GLOBAL_FLAGS: &[&str] = &["--sock", "--timeout", "--json", "--quiet"];

#[derive(Debug, Default)]
struct Toks {
    pos: Vec<String>,
    flags: BTreeMap<String, Option<String>>,
}

fn tokenize(argv: &[String]) -> Result<Toks, CliError> {
    let mut t = Toks::default();
    let mut it = argv.iter();
    let mut only_pos = false;
    while let Some(a) = it.next() {
        if only_pos || !a.starts_with("--") || a == "--" {
            if a == "--" && !only_pos {
                only_pos = true;
                continue;
            }
            t.pos.push(a.clone());
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (a.clone(), None),
        };
        if VALUE_FLAGS.contains(&name.as_str()) {
            let v = match inline {
                Some(v) => v,
                None => it.next().cloned().ok_or_else(|| CliError::usage(format!("{name} needs a value")))?,
            };
            if t.flags.insert(name.clone(), Some(v)).is_some() {
                return Err(CliError::usage(format!("{name} given twice")));
            }
        } else if BOOL_FLAGS.contains(&name.as_str()) {
            if inline.is_some() {
                return Err(CliError::usage(format!("{name} takes no value")));
            }
            t.flags.insert(name, None);
        } else {
            return Err(CliError::usage(format!("unknown option {name}")));
        }
    }
    Ok(t)
}

impl Toks {
    fn has(&self, f: &str) -> bool {
        self.flags.contains_key(f)
    }

    fn val(&self, f: &str) -> Option<&str> {
        self.flags.get(f).and_then(|v| v.as_deref())
    }

    /// Fail on any flag outside `allowed` (globals are always allowed).
    fn only(&self, allowed: &[&str]) -> Result<(), CliError> {
        for f in self.flags.keys() {
            if !GLOBAL_FLAGS.contains(&f.as_str()) && !allowed.contains(&f.as_str()) {
                return Err(CliError::usage(format!("option {f} does not apply to this command")));
            }
        }
        Ok(())
    }

    /// Exactly `n` positionals after the command words.
    fn args(&self, skip: usize, n: usize, what: &str) -> Result<Vec<String>, CliError> {
        let rest = &self.pos[skip.min(self.pos.len())..];
        if rest.len() != n {
            return Err(CliError::usage(format!("expected {what}")));
        }
        Ok(rest.to_vec())
    }

    /// `--wait` / `--no-wait` → `Some(true|false)`; neither → None (server default).
    fn wait(&self) -> Result<Option<bool>, CliError> {
        match (self.has("--wait"), self.has("--no-wait")) {
            (true, true) => Err(CliError::usage("--wait and --no-wait conflict")),
            (true, false) => Ok(Some(true)),
            (false, true) => Ok(Some(false)),
            (false, false) => Ok(None),
        }
    }
}

fn parse_u64(flag: &str, v: &str) -> Result<u64, CliError> {
    v.parse::<u64>().map_err(|_| CliError::usage(format!("{flag} needs a non-negative integer, got `{v}`")))
}

fn read_capped(flag: &str, path: &Path, max: usize) -> Result<String, CliError> {
    let bytes = std::fs::read(path).map_err(|e| CliError::usage(format!("{flag} {}: {e}", path.display())))?;
    if bytes.len() > max {
        return Err(CliError::usage(format!("{flag} {}: {} bytes; the cap is {max}", path.display(), bytes.len())));
    }
    String::from_utf8(bytes).map_err(|_| CliError::usage(format!("{flag} {}: not UTF-8", path.display())))
}

/// Make a path absolute against `cwd` without resolving symlinks (the kernel checks those with
/// `O_NOFOLLOW`).
fn absolute(cwd: &Path, p: &str) -> String {
    let p = Path::new(p);
    let joined = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    std::path::absolute(&joined).unwrap_or(joined).to_string_lossy().into_owned()
}

/// Parse argv (without the program name).
pub fn parse(argv: &[String], cwd: &Path) -> Result<Parsed, CliError> {
    let t = tokenize(argv)?;
    if t.has("--help") || t.pos.first().map(String::as_str) == Some("help") {
        return Ok(Parsed::Help);
    }
    if t.has("--version") {
        return Ok(Parsed::Version);
    }
    let opts = Opts {
        sock: t.val("--sock").map(PathBuf::from),
        json: t.has("--json"),
        quiet: t.has("--quiet"),
        timeout: t.val("--timeout").map(|v| parse_u64("--timeout", v)).transpose()?,
    };
    let word = |i: usize| t.pos.get(i).map(String::as_str);
    let cmd = match (word(0), word(1)) {
        (None, _) => return Err(CliError::usage("missing command")),
        (Some("status"), _) => {
            t.only(&[])?;
            t.args(1, 0, "no arguments")?;
            Command::Status
        }
        (Some("ping"), _) => {
            t.only(&[])?;
            t.args(1, 0, "no arguments")?;
            Command::Ping
        }
        (Some("level"), _) => {
            t.only(&[])?;
            t.args(1, 0, "no arguments")?;
            Command::Level
        }
        (Some("reload"), _) => {
            t.only(&["--wait", "--no-wait"])?;
            t.args(1, 0, "no arguments")?;
            Command::Reload(ReloadArgs { wait: t.wait()? })
        }
        (Some("gate"), _) => {
            t.only(&["--quick", "--wait", "--no-wait"])?;
            if !t.has("--quick") {
                return Err(CliError::usage("only quick gates run from initctl (gate --quick); a full gate runs through promote"));
            }
            let target = match t.pos.len() {
                1 => None,
                2 => Some(t.pos[1].clone()),
                _ => return Err(CliError::usage("expected at most one <path> or <ref>")),
            };
            let (repo, rev) = gitsnap::resolve(cwd, target.as_deref())?;
            Command::Gate(GateArgs { quick: true, repo, rev, wait: t.wait()? })
        }
        (Some("promote"), _) => {
            t.only(&["--sha", "--wait"])?;
            match t.val("--sha") {
                Some(sha) => {
                    t.args(1, 0, "no <branch> together with --sha")?;
                    Command::Promote(PromoteArgs { git_ref: None, sha: Some(sha.to_string()), wait: t.wait()? })
                }
                None => {
                    let a = t.args(1, 1, "promote <evo/id>")?;
                    Command::Promote(PromoteArgs { git_ref: Some(a[0].clone()), sha: None, wait: t.wait()? })
                }
            }
        }
        (Some("rollback"), _) => {
            t.only(&["--reason", "--wait"])?;
            t.args(1, 0, "rollback [--reason TEXT]")?;
            let reason = t.val("--reason").unwrap_or("no reason given").to_string();
            Command::Rollback(RollbackArgs { reason, wait: t.wait()? })
        }
        (Some("vendor"), _) => {
            t.only(&["--no-wait", "--wait", "--workspace"])?;
            let a = t.args(1, 1, "vendor <crate>@<version>")?;
            let (krate, version) =
                a[0].split_once('@').ok_or_else(|| CliError::usage("expected <crate>@<version>, e.g. vt100@0.16.2"))?;
            let workspace = match t.val("--workspace") {
                None => None,
                Some("body") => Some(VendorWorkspace::Body),
                Some("ext") => Some(VendorWorkspace::Ext),
                Some(o) => return Err(CliError::usage(format!("--workspace must be body or ext, got `{o}`"))),
            };
            Command::Vendor(VendorArgs { krate: krate.into(), version: version.into(), workspace, wait: t.wait()? })
        }
        (Some("task"), Some("template")) => {
            t.only(&[])?;
            t.args(2, 0, "no arguments")?;
            Command::TaskTemplate
        }
        (Some("task"), Some("submit")) => {
            t.only(&["--no-wait", "--wait"])?;
            let a = t.args(2, 1, "task submit <dir>")?;
            Command::TaskSubmit(TaskSubmitArgs { dir: absolute(cwd, &a[0]), wait: t.wait()? })
        }
        (Some("task"), Some("list")) => {
            t.only(&[])?;
            t.args(2, 0, "no arguments")?;
            Command::TaskList
        }
        (Some("task"), Some("show")) => {
            t.only(&[])?;
            let a = t.args(2, 1, "task show <family>")?;
            Command::TaskShow(TaskShowArgs { family: a[0].clone() })
        }
        (Some("budget"), Some("set")) => {
            t.only(&[])?;
            if t.pos.len() < 3 {
                return Err(CliError::usage("expected budget set <key>=<value>..."));
            }
            let mut set = BTreeMap::new();
            for kv in &t.pos[2..] {
                let (k, v) = kv.split_once('=').ok_or_else(|| CliError::usage(format!("expected <key>=<value>, got `{kv}`")))?;
                if k.is_empty() {
                    return Err(CliError::usage(format!("empty key in `{kv}`")));
                }
                let v = match v.parse::<i64>() {
                    Ok(n) => BudgetValue::Int(n),
                    Err(_) => BudgetValue::Text(v.to_string()),
                };
                if set.insert(k.to_string(), v).is_some() {
                    return Err(CliError::usage(format!("key `{k}` given twice")));
                }
            }
            Command::BudgetSet(BudgetSetArgs { set })
        }
        (Some("model"), Some("set")) => {
            t.only(&[])?;
            let a = t.args(2, 1, "model set <provider>/<id>")?;
            Command::ModelSet(ModelSetArgs { model: a[0].clone() })
        }
        (Some("fleet"), Some("list")) => {
            t.only(&[])?;
            t.args(2, 0, "no arguments")?;
            Command::FleetList
        }
        (Some("fleet"), Some("spawn")) => {
            t.only(&["--parent", "--prompt-file", "--share-musd-day", "--agent"])?;
            t.args(2, 0, "fleet spawn [--parent SHA] [--agent NAME] --prompt-file FILE [--share-musd-day N]")?;
            let pf = t.val("--prompt-file").ok_or_else(|| CliError::usage("fleet spawn needs --prompt-file FILE"))?;
            let prompt = read_capped("--prompt-file", &cwd.join(pf), MAX_TEXT_BYTES)?;
            let share = t.val("--share-musd-day").map(|v| parse_u64("--share-musd-day", v)).transpose()?;
            Command::FleetSpawn(FleetSpawnArgs { parent: t.val("--parent").map(str::to_string), prompt, budget_share_musd_day: share, agent: t.val("--agent").map(str::to_string) })
        }
        (Some("fleet"), Some("send")) => {
            t.only(&["--text"])?;
            let a = t.args(2, 1, "fleet send <bid> --text TEXT")?;
            let text = t.val("--text").ok_or_else(|| CliError::usage("fleet send needs --text TEXT"))?;
            Command::FleetSend(FleetSendArgs { bid: a[0].clone(), text: text.to_string() })
        }
        (Some("fleet"), Some("stop")) => {
            t.only(&["--all", "--reason"])?;
            let bid = if t.has("--all") {
                t.args(2, 0, "fleet stop --all (no <bid>)")?;
                "all".to_string()
            } else {
                t.args(2, 1, "fleet stop (<bid> | --all)")?[0].clone()
            };
            Command::FleetStop(FleetStopArgs { bid, reason: t.val("--reason").map(str::to_string) })
        }
        (Some("kernel"), Some("propose")) => kernel_propose(&t, 2, cwd)?,
        (Some("propose-kernel"), _) => kernel_propose(&t, 1, cwd)?,
        (Some("kernel"), Some("promote")) => {
            t.only(&["--wait"])?;
            let a = t.args(2, 1, "kernel promote <kevo/id>")?;
            Command::KernelPromote(KernelPromoteArgs { git_ref: a[0].clone(), wait: t.wait()? })
        }
        (Some("kernel"), Some("status")) => {
            t.only(&[])?;
            t.args(2, 0, "no arguments")?;
            Command::KernelStatus
        }
        (Some("wait"), _) => wait_cmd(&t, 1, &opts)?,
        (Some("job"), Some("wait")) => wait_cmd(&t, 2, &opts)?,
        (Some("job"), Some("status")) => {
            t.only(&[])?;
            Command::JobStatus(JobArgs { job: t.args(2, 1, "job status <job>")?[0].clone() })
        }
        (Some("job"), Some("cancel")) => {
            t.only(&[])?;
            Command::JobCancel(JobArgs { job: t.args(2, 1, "job cancel <job>")?[0].clone() })
        }
        (Some("job"), Some("list")) => {
            t.only(&["--all"])?;
            t.args(2, 0, "job list [--all]")?;
            Command::JobList(JobListArgs { state: t.has("--all").then_some(JobListState::All), limit: None })
        }
        (Some("evo"), Some("frictions")) => {
            t.only(&["--all"])?;
            t.args(2, 0, "evo frictions [--all]")?;
            Command::EvoFrictions(EvoFrictionsArgs { all: t.has("--all").then_some(true) })
        }
        (Some("evo"), Some("propose")) => {
            t.only(&[])?;
            let a = t.args(2, 1, "evo propose <draft.json>")?;
            Command::EvoPropose(EvoProposeArgs { file: absolute(cwd, &a[0]) })
        }
        (Some("evo"), Some("variant")) => {
            t.only(&[])?;
            let a = t.args(2, 3, "evo variant <p-id> <k> <patch>")?;
            let k = parse_u64("<k>", &a[1])?;
            let k = u32::try_from(k).map_err(|_| CliError::usage("<k> is 1 to 3"))?;
            Command::EvoVariant(EvoVariantArgs { proposal: a[0].clone(), k, patch: absolute(cwd, &a[2]) })
        }
        (Some("evo"), Some("screen")) => {
            t.only(&["--wait", "--no-wait"])?;
            let a = t.args(2, 1, "evo screen <p-id> [--wait]")?;
            Command::EvoScreen(EvoScreenArgs { proposal: a[0].clone(), wait: t.wait()?, pool_repeats: None, pool_limit: None })
        }
        (Some("evo"), Some("calibrate")) => {
            // REAL-M7: the incumbent on the visible pool (seeds the pool cache and the miner; M7 line C)
            t.only(&["--repeats", "--limit", "--wait", "--no-wait"])?;
            t.args(2, 0, "evo calibrate [--repeats R] [--limit N] [--wait]")?;
            let num = |k: &str, d: u64| t.val(k).map_or(Ok(d), |v| parse_u64(k, v));
            let r = u32::try_from(num("--repeats", 2)?).map_err(|_| CliError::usage("--repeats is 1 to 6"))?;
            let n = u32::try_from(num("--limit", 0)?).map_err(|_| CliError::usage("--limit is a task count"))?;
            Command::EvoScreen(EvoScreenArgs { proposal: String::new(), wait: t.wait()?, pool_repeats: Some(r), pool_limit: Some(n) })
        }
        (Some("evo"), Some("status")) => {
            t.only(&[])?;
            let proposal = match t.pos.len() {
                2 => None,
                3 => Some(t.pos[2].clone()),
                _ => return Err(CliError::usage("expected evo status [<p-id>]")),
            };
            Command::EvoStatus(EvoStatusArgs { proposal })
        }
        (Some("evo"), Some("abandon")) => {
            t.only(&[])?;
            let a = t.args(2, 2, "evo abandon <p-id> <reason>")?;
            Command::EvoAbandon(EvoAbandonArgs { proposal: a[0].clone(), reason: a[1].clone() })
        }
        (Some("grant"), _) => {
            t.only(&["--hours", "--why"])?;
            let a = t.args(1, 2, "grant <kind> <value> [--hours H] --why TEXT")?;
            let why = t.val("--why").ok_or_else(|| CliError::usage("grant needs --why TEXT: the task, the limit you hit, the evidence"))?;
            let duration_h = t.val("--hours").map(|v| parse_u64("--hours", v)).transpose()?;
            Command::Grant(GrantArgs { kind: a[0].clone(), value: grant_value(&a[1])?, duration_h, why: why.to_string() })
        }
        (Some(w @ ("task" | "budget" | "model" | "fleet" | "kernel" | "job" | "evo")), sub) => {
            return Err(CliError::usage(format!("unknown subcommand `{w} {}`", sub.unwrap_or(""))));
        }
        (Some(other), _) => return Err(CliError::usage(format!("unknown command `{other}`"))),
    };
    cmd.validate().map_err(|e| CliError::usage(e.message))?;
    Ok(Parsed::Run(opts, cmd))
}

/// A grant value: digits → an integer; `[`…`]` or `{`…`}` → JSON (`dep`'s crate list); anything else
/// a string (money is a decimal string, `"2.00"`).
fn grant_value(v: &str) -> Result<init_proto::serde_json::Value, CliError> {
    if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(init_proto::serde_json::Value::from(parse_u64("<value>", v)?));
    }
    if v.starts_with('[') || v.starts_with('{') {
        return init_proto::serde_json::from_str(v).map_err(|e| CliError::usage(format!("<value> is not JSON: {e}")));
    }
    Ok(init_proto::serde_json::Value::from(v))
}

fn kernel_propose(t: &Toks, skip: usize, cwd: &Path) -> Result<Command, CliError> {
    t.only(&["--title", "--rationale-file"])?;
    let a = t.args(skip, 1, "kernel propose <patch-file> --title TEXT [--rationale-file FILE]")?;
    let title = t.val("--title").ok_or_else(|| CliError::usage("kernel propose needs --title TEXT"))?;
    let rationale = match t.val("--rationale-file") {
        Some(f) => read_capped("--rationale-file", &cwd.join(f), MAX_RATIONALE_BYTES)?,
        None => String::new(),
    };
    Ok(Command::KernelPropose(KernelProposeArgs { title: title.to_string(), patch: absolute(cwd, &a[0]), rationale }))
}

fn wait_cmd(t: &Toks, skip: usize, opts: &Opts) -> Result<Command, CliError> {
    t.only(&["--last", "--cmd"])?;
    let last = t.has("--last");
    let job = if last {
        t.args(skip, 0, "wait --last (no <job>)")?;
        None
    } else {
        Some(t.args(skip, 1, "wait (<job> | --last [--cmd NAME])")?[0].clone())
    };
    let cmd = match t.val("--cmd") {
        Some(c) => Some(
            CommandName::parse(c)
                .ok_or_else(|| CliError::usage(format!("unknown command `{c}` for --cmd")))?
                .as_str()
                .to_string(),
        ),
        None => None,
    };
    Ok(Command::JobWait(JobWaitArgs { job, last: last.then_some(true), cmd, timeout_s: opts.timeout }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(args: &[&str]) -> String {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        match parse(&argv, Path::new("/h/work")).unwrap() {
            Parsed::Run(_, c) => String::from_utf8(c.to_request("1").to_line()).unwrap().trim_end().to_string(),
            other => panic!("{other:?}"),
        }
    }

    fn usage(args: &[&str]) -> String {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let e = parse(&argv, Path::new("/h/work")).unwrap_err();
        assert_eq!(e.code, 64, "{args:?}");
        e.message
    }

    const SHA: &str = "4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807";

    /// REAL-M7: `evo calibrate` is `evo.screen` with `pool_repeats` (no proposal), validated by init-proto.
    #[test]
    fn evo_calibrate_line() {
        assert_eq!(line(&["evo", "calibrate", "--repeats", "2", "--limit", "4", "--wait"]),
                   r#"{"args":{"pool_limit":4,"pool_repeats":2,"proposal":"","wait":true},"cmd":"evo.screen","id":"1","v":1}"#);
        assert_eq!(line(&["evo", "calibrate"]), r#"{"args":{"pool_limit":0,"pool_repeats":2,"proposal":""},"cmd":"evo.screen","id":"1","v":1}"#);
        assert!(usage(&["evo", "calibrate", "p-000001"]).contains("evo calibrate"));
        let ok = |a: EvoScreenArgs| Command::EvoScreen(a).validate().is_ok();
        let a = |p: &str, r: Option<u32>| EvoScreenArgs { proposal: p.into(), wait: None, pool_repeats: r, pool_limit: None };
        assert!(ok(a("", Some(2))) && ok(a("p-000001", None)));
        assert!(!ok(a("", Some(0))) && !ok(a("", Some(7))) && !ok(a("p-000001", Some(2))) && !ok(a("", None)));
    }

    #[test]
    fn exact_json_lines() {
        // testing.md §3.2 initctl: argument parsing → exact JSON line.
        let cases: Vec<(Vec<&str>, String)> = vec![
            (vec!["status"], r#"{"args":{},"cmd":"status","id":"1","v":1}"#.into()),
            (vec!["ping"], r#"{"args":{},"cmd":"ping","id":"1","v":1}"#.into()),
            (vec!["level"], r#"{"args":{},"cmd":"level","id":"1","v":1}"#.into()),
            (vec!["reload"], r#"{"args":{},"cmd":"reload","id":"1","v":1}"#.into()),
            (vec!["reload", "--wait"], r#"{"args":{"wait":true},"cmd":"reload","id":"1","v":1}"#.into()),
            (vec!["promote", "evo/pty-tool"], r#"{"args":{"ref":"evo/pty-tool"},"cmd":"promote","id":"1","v":1}"#.into()),
            (vec!["promote", "evo/pty-tool", "--wait"], r#"{"args":{"ref":"evo/pty-tool","wait":true},"cmd":"promote","id":"1","v":1}"#.into()),
            (vec!["promote", "--sha", SHA], format!(r#"{{"args":{{"sha":"{SHA}"}},"cmd":"promote","id":"1","v":1}}"#)),
            (
                vec!["rollback", "--reason", "G4 will fail: canary pattern in loop.rs"],
                r#"{"args":{"reason":"G4 will fail: canary pattern in loop.rs"},"cmd":"rollback","id":"1","v":1}"#.into(),
            ),
            (vec!["rollback"], r#"{"args":{"reason":"no reason given"},"cmd":"rollback","id":"1","v":1}"#.into()),
            (vec!["vendor", "vt100@0.16.2"], r#"{"args":{"crate":"vt100","version":"0.16.2"},"cmd":"vendor","id":"1","v":1}"#.into()),
            (
                vec!["vendor", "vt100@0.16.2", "--no-wait", "--workspace", "ext"],
                r#"{"args":{"crate":"vt100","version":"0.16.2","wait":false,"workspace":"ext"},"cmd":"vendor","id":"1","v":1}"#.into(),
            ),
            (vec!["task", "template"], r#"{"args":{},"cmd":"task.template","id":"1","v":1}"#.into()),
            (vec!["task", "submit", "tasks/fam"], r#"{"args":{"dir":"/h/work/tasks/fam"},"cmd":"task.submit","id":"1","v":1}"#.into()),
            (vec!["task", "submit", "/h/work/t", "--no-wait"], r#"{"args":{"dir":"/h/work/t","wait":false},"cmd":"task.submit","id":"1","v":1}"#.into()),
            (vec!["task", "list"], r#"{"args":{},"cmd":"task.list","id":"1","v":1}"#.into()),
            (vec!["task", "show", "fam"], r#"{"args":{"family":"fam"},"cmd":"task.show","id":"1","v":1}"#.into()),
            (
                vec!["budget", "set", "gate_usd=2.00", "promotes_day=10"],
                r#"{"args":{"set":{"gate_usd":"2.00","promotes_day":10}},"cmd":"budget.set","id":"1","v":1}"#.into(),
            ),
            (vec!["model", "set", "deepseek/deepseek-v4-pro"], r#"{"args":{"model":"deepseek/deepseek-v4-pro"},"cmd":"model.set","id":"1","v":1}"#.into()),
            (vec!["fleet", "list"], r#"{"args":{},"cmd":"fleet.list","id":"1","v":1}"#.into()),
            (vec!["fleet", "send", "b1", "--text", "focus"], r#"{"args":{"bid":"b1","text":"focus"},"cmd":"fleet.send","id":"1","v":1}"#.into()),
            (vec!["fleet", "stop", "--all"], r#"{"args":{"bid":"all"},"cmd":"fleet.stop","id":"1","v":1}"#.into()),
            (vec!["fleet", "stop", "b2", "--reason", "idle"], r#"{"args":{"bid":"b2","reason":"idle"},"cmd":"fleet.stop","id":"1","v":1}"#.into()),
            (vec!["kernel", "promote", "kevo/gate-cache", "--wait"], r#"{"args":{"ref":"kevo/gate-cache","wait":true},"cmd":"kernel.promote","id":"1","v":1}"#.into()),
            (vec!["kernel", "status"], r#"{"args":{},"cmd":"kernel.status","id":"1","v":1}"#.into()),
            (vec!["wait", "j-000047", "--timeout", "600"], r#"{"args":{"job":"j-000047","timeout_s":600},"cmd":"job.wait","id":"1","v":1}"#.into()),
            (vec!["--timeout", "60", "wait", "j-000047"], r#"{"args":{"job":"j-000047","timeout_s":60},"cmd":"job.wait","id":"1","v":1}"#.into()),
            (vec!["wait", "--last"], r#"{"args":{"last":true},"cmd":"job.wait","id":"1","v":1}"#.into()),
            (vec!["wait", "--last", "--cmd", "promote"], r#"{"args":{"cmd":"promote","last":true},"cmd":"job.wait","id":"1","v":1}"#.into()),
            (vec!["job", "wait", "j-000047"], r#"{"args":{"job":"j-000047"},"cmd":"job.wait","id":"1","v":1}"#.into()),
            (vec!["job", "status", "j-000047"], r#"{"args":{"job":"j-000047"},"cmd":"job.status","id":"1","v":1}"#.into()),
            (vec!["job", "list"], r#"{"args":{},"cmd":"job.list","id":"1","v":1}"#.into()),
            (vec!["job", "list", "--all"], r#"{"args":{"state":"all"},"cmd":"job.list","id":"1","v":1}"#.into()),
            (vec!["job", "cancel", "j-000047"], r#"{"args":{"job":"j-000047"},"cmd":"job.cancel","id":"1","v":1}"#.into()),
            (vec!["--json", "--quiet", "--sock=/s", "ping"], r#"{"args":{},"cmd":"ping","id":"1","v":1}"#.into()),
            (
                vec!["grant", "extend.open_groups", "6", "--hours", "48", "--why", "a batch of 5 tools for one task"],
                r#"{"args":{"duration_h":48,"kind":"extend.open_groups","value":6,"why":"a batch of 5 tools for one task"},"cmd":"grant","id":"1","v":1}"#.into(),
            ),
            (
                vec!["grant", "money.day_usd", "2.00", "--why", "w"],
                r#"{"args":{"kind":"money.day_usd","value":"2.00","why":"w"},"cmd":"grant","id":"1","v":1}"#.into(),
            ),
            (
                vec!["grant", "dep", r#"[{"name":"vt100","version":"0.16.2","checksum":"ab"}]"#, "--why", "w"],
                r#"{"args":{"kind":"dep","value":[{"checksum":"ab","name":"vt100","version":"0.16.2"}],"why":"w"},"cmd":"grant","id":"1","v":1}"#.into(),
            ),
        ];
        for (args, want) in cases {
            assert_eq!(line(&args), want, "{args:?}");
        }
    }

    #[test]
    fn global_options() {
        let argv: Vec<String> = ["--sock", "/x/k.sock", "--json", "--quiet", "--timeout", "5", "status"].iter().map(|s| s.to_string()).collect();
        let Parsed::Run(o, _) = parse(&argv, Path::new("/")).unwrap() else { panic!() };
        assert_eq!(o, Opts { sock: Some("/x/k.sock".into()), json: true, quiet: true, timeout: Some(5) });
        assert_eq!(parse(&["--version".to_string()], Path::new("/")).unwrap(), Parsed::Version);
        assert_eq!(parse(&["help".to_string()], Path::new("/")).unwrap(), Parsed::Help);
    }

    #[test]
    fn usage_errors() {
        usage(&[]);
        usage(&["approve"]);
        usage(&["status", "extra"]);
        usage(&["gate"]); // full gate refused locally
        usage(&["promote"]);
        usage(&["promote", "main"]); // not evo/<id>
        usage(&["promote", "evo/x", "--sha", SHA]);
        usage(&["vendor", "vt100"]);
        usage(&["vendor", "vt100@^0.16"]);
        usage(&["vendor", "vt100@0.16.2", "--workspace", "kernel"]);
        usage(&["wait"]);
        usage(&["wait", "j-1"]);
        usage(&["wait", "j-000001", "--last"]);
        usage(&["wait", "--last", "--cmd", "nope"]);
        usage(&["wait", "j-000001", "--timeout", "3601"]);
        usage(&["--timeout", "x", "status"]);
        usage(&["kernel", "promote", "kevo/Bad"]);
        usage(&["kernel", "frob"]);
        usage(&["fleet", "spawn"]);
        usage(&["fleet", "send", "b1"]);
        usage(&["budget", "set", "novalue"]);
        usage(&["reload", "--wait", "--no-wait"]);
        usage(&["status", "--reason", "x"]);
        usage(&["--bogus", "status"]);
        usage(&["grant", "extend.open_groups", "6"]); // no --why
        usage(&["grant", "dep", "[not json", "--why", "w"]);
        usage(&["grant", "extend.open_groups", "6", "--hours", "0", "--why", "w"]);
        assert!(usage(&["gate"]).contains("--quick"));
    }
}
