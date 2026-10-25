//! The owner channel (root/root.md §3.15, §4.2): `ROOT_HOME/run/owner.sock` with its peer check,
//! request frames, the attach channel, the relay queue and replay ring, alerts (§4.6), the Root's
//! shared state and cold start (§3.6.5), and the CLI's client side.
//! WP21: `owner.in` delivery, acknowledgement and redelivery, interrupts, `stop`/`start` through the
//! supervisor, the record hooks (level, pointers, `level` events). The kernel control link is in sup.rs.
//! WP26 (tui.md §9 Q1): `rseq` and `src` on every frame, `attach{since, epoch}`, `replay_end{epoch, gap}`,
//! `code` on alert notices, a throttled notice per `ceiling_hit`.
//! Lock order: `lin` before `st` before `sup.ptrs`; `ring` before `clients`; `hits` is a leaf; nothing is
//! emitted while `st` is held.

use crate::charter::{self, Loaded};
use crate::install::{self, Paths};
use crate::ledger::{self, Level};
use crate::lineage::{self, Lineage};
use crate::sup::{Ev, Plan, Slot};
use crate::util::{self, icj, now_ms, rfc3339, write_atomic};
use crate::{panic, sys};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

const FRAME_MAX: u64 = 1 << 20;
/// The largest owner line the Root queues for the kernel (900 KiB), leaving room for the `owner.in` wrapping.
const OWNER_LINE_MAX: usize = 900 * 1024;
pub type Refusal = (&'static str, String);

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct St {
    pub slot: Slot,
    pub mode: &'static str,
    pub charter: Option<Loaded>,
    pub pending: bool,
    pub level: Level,
    pub queued: usize,
    started: i64,
}

impl St {
    /// The Root epoch (tui.md §3.11): `status`'s `root.started_at` and `root.pid`, as `"<started_at>/<pid>"`.
    pub fn epoch(&self) -> String {
        format!("{}/{}", rfc3339(self.started), std::process::id())
    }
}

/// The replay ring: `(rseq, line)` pairs, their bytes, and the last `rseq` assigned (counted per Root
/// process from 1, under this lock, so ring order is `rseq` order).
#[derive(Default)]
struct Ring {
    frames: VecDeque<(u64, String)>,
    bytes: usize,
    last: u64,
}

pub struct Root {
    pub benchmark: Option<crate::benchmark::Policy>,
    pub p: Paths,
    pub st: Mutex<St>,
    pub lin: Mutex<Option<Lineage>>,
    clients: Mutex<Vec<UnixStream>>,
    ring: Mutex<Ring>,
    replay: (usize, usize),
    /// The last `ceiling_hit` notice per ceiling (ms), for the one-a-minute throttle.
    hits: Mutex<HashMap<String, i64>>,
    /// R2/R3, started by the daemon after cold start (root/egress.md §4.3).
    pub egress: OnceLock<Arc<crate::egress::Egress>>,
    /// `install.json`: toolchain, devdir, sdkroot, git.
    pub info: Value,
    /// R1: the spawn broker's instance table (zone.rs) and the supervisor's shared state (sup.rs).
    pub zone: crate::zone::Broker,
    pub sup: crate::sup::Shared,
    /// SBX-009 (ADR-009): the workspace and full mode (access.rs); a leaf lock.
    pub access: Mutex<crate::access::St>,
    /// ADR-011 §3 (X3): the harm sentinel; never held while appending.
    pub sentinel: crate::sentinel::Shared,
    /// ADR-011 §5 (X5): attribution, the usage ledger and the cap table (extend/); a leaf lock.
    pub extend: crate::extend::Extend,
}

impl Root {
    /// Cold start (§3.6.5) up to the services: pid, charter, layout, tools, lineage, latch, ledger, view.
    /// The caller holds `run/root.lock` (and, for the daemon, has written `run/root.pid`).
    /// Then the previous Root's instances (zone.rs `cold_start`); the tuple choice is the supervisor's (sup.rs).
    pub fn boot(p: Paths) -> io::Result<Arc<Root>> {
        let benchmark = crate::benchmark::load(&p.rh)?;
        let cx = p.ctx();
        let active = charter::load(&p.r("state/charter.active.toml"), &cx);
        #[cfg(feature = "test-fixtures")]
        if active.as_ref().is_ok_and(|l| l.ch.root.test_mode)
            && let Some(t) = std::env::var("INIT_ROOT_TEST_NOW").ok().and_then(|s| util::parse_rfc3339(&s))
        {
            let _ = crate::testfix::set_now(t);
        }
        let active = active.map(|l| crate::grant::merged(&p, l)); // X7: the owner's grant overlay on top (ADR-011 §7.4)
        let pending = match (&active, charter::load(&p.r("charter.toml"), &cx)) {
            (Ok(a), Ok(c)) => a.digest != c.digest,
            _ => true,
        };
        let extra = active.as_ref().map(|l| l.ch.sandbox.extra_read.clone()).unwrap_or_default();
        let info: Value = std::fs::read_to_string(p.r("install.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let lin = Lineage::open(&p.rh, Some(p.i("root/lineage.jsonl")));
        let records = lineage::records(&p.rh).unwrap_or_default();
        let test_chain = records.first().is_some_and(|g| g["data"]["test"] == true);
        let why = [
            active.as_ref().err().map(|f| format!("active charter: {}", f.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))),
            install::check_layout(&p, &extra).err(),
            install::check_tools(info["git"].as_str().unwrap_or("/usr/bin/git")).err(),
            lin.as_ref().err().map(ToString::to_string),
            (test_chain && !cfg!(feature = "test-fixtures")).then(|| "a test:true genesis on a production Root".to_string()),
        ];
        let halt = why.iter().position(Option::is_some).map(|i| ["layout", "layout", "tooling", "lineage", "lineage"][i]);
        let r = active.as_ref().map(|l| (l.ch.root.replay_frames, l.ch.root.replay_bytes)).unwrap_or((2000, 262_144));
        let queued = std::fs::read_to_string(p.r("state/owner-queue.jsonl")).map(|s| s.lines().count()).unwrap_or(0);
        let st = St { slot: halt.map_or(Slot::Stopped, |reason| Slot::Halted { reason, retry_at: None }), mode: "normal", charter: active.ok(),
            pending, level: Level::fold(&records), queued, started: now_ms() };
        let access = Mutex::new(crate::access::load(&p));
        let root = Arc::new(Root { benchmark, p, st: Mutex::new(st), lin: Mutex::new(lin.ok().filter(|_| halt != Some("lineage"))),
            clients: Mutex::new(Vec::new()), ring: Mutex::default(), replay: (r.0 as usize, r.1 as usize), hits: Mutex::default(),
            egress: OnceLock::new(), info, zone: Default::default(), sup: Default::default(), access,
            sentinel: Default::default(), extend: Default::default() });
        records.iter().for_each(|r| lock(&root.sup.ptrs).apply(r));
        records.iter().for_each(|r| root.extend.apply(r));
        match halt {
            Some(reason) => {
                let detail = why.into_iter().flatten().next().unwrap_or_default();
                util::log(&root.p.rh, "error", &format!("Halted{{{reason}}}: {detail}"));
                root.append("halt", json!({"on": true, "reason": reason, "retry_at": null}));
                root.alert("halted", &format!("[root] Halted{{{reason}}}: the zone will not run until this is fixed (see logs/root.jsonl)"));
            }
            None => {
                crate::zone::cold_start(&root);
                panic::cold_start(&root, &records);
            }
        }
        if pending {
            root.alert("charter_pending", "[root] charter.toml differs from the active charter; run init-root charter check --apply");
        }
        root.publish();
        Ok(root)
    }

    /// Appends a Root record and folds it into the level. None if the lineage is unavailable.
    pub fn append(&self, kind: &str, data: Value) -> Option<Value> {
        self.append_by("root", kind, data).map_err(|e| util::log(&self.p.rh, "error", &format!("lineage append {kind}: {e}"))).ok()
    }

    /// Appends a record signed by the Root on behalf of `by` (`root`, or `kernel:<12>`).
    pub fn append_by(&self, by: &str, kind: &str, data: Value) -> io::Result<Value> {
        let mut lg = lock(&self.lin);
        let rec = self.append_locked(&mut lg, by, kind, data);
        drop(lg);
        if let Ok(r) = &rec {
            self.after(r);
        }
        rec
    }

    /// The append under a lineage lock the caller holds (panic and resume keep their records together).
    pub fn append_locked(&self, lg: &mut Option<Lineage>, by: &str, kind: &str, data: Value) -> io::Result<Value> {
        let rec = lg.as_mut().ok_or_else(|| util::bad("the lineage is unavailable"))?.append(by, kind, data)?;
        lock(&self.st).level.apply(&rec);
        lock(&self.sup.ptrs).apply(&rec);
        self.extend.apply(&rec);
        Ok(rec)
    }

    /// After a record (with no lineage lock held): R7's evaluation; after a `ladder` record, the egress
    /// level and the kernel's `level` event instead (§3.13.1). A `ceiling_hit` also gets a Root notice.
    pub fn after(&self, rec: &Value) {
        crate::sentinel::record(self, rec); // X3: H1, H3, H4 and the rollback confirmations
        if rec["kind"] == "ceiling_hit" {
            self.ceiling_notice(&rec["data"]);
        }
        if rec["kind"] != "ladder" {
            return ledger::tick(self);
        }
        self.sync_egress();
        let (d, max) = (&rec["data"], lock(&self.st).charter.as_ref().map_or(0, |l| l.ch.autonomy.max_level));
        if let Some(k) = self.sup.kernel() {
            k.event("level", json!({"cap": ledger::cap(max), "cause": d["cause"], "evolution_share_max_permille": null, "from": d["from"],
                "r4_seq": rec["seq"], "to": d["to"], "trigger": d["trigger"]}));
        }
    }

    /// `egress::set_level` on every level and mode change (root/root.md §3.13.1).
    pub fn sync_egress(&self) {
        let (level, safe) = { let st = lock(&self.st); (st.level.level, st.mode == "safe") };
        if let Some(e) = self.egress.get() {
            e.set_level(level, safe);
        }
    }

    pub fn publish(&self) {
        {
            let st = lock(&self.st);
            let digest = st.charter.as_ref().map(|l| l.digest.as_str());
            let panicked = matches!(st.slot, Slot::Panicked { .. });
            let mut sv = install::status_view(st.slot.name(), st.mode, panicked, st.level.level, digest);
            let p = lock(&self.sup.ptrs).clone();
            let t = |id: &Option<String>| id.as_ref().and_then(|i| crate::tuple::load(&self.p.rh, i)).unwrap_or_default();
            (sv["tuples"], sv["kernel_build"]) = (p.json(), t(&p.current)["kernel"]["init_kernel"].clone());
            sv["audited_good"] = json!({"body_sha": t(&p.audited)["body"]["sha"], "tuple": p.audited});
            if let Err(e) = install::publish(&self.p, st.charter.as_ref(), &st.level, &sv) {
                util::log(&self.p.rh, "error", &format!("publish view: {e}"));
            }
        }
        if let Some(e) = self.egress.get() {
            e.publish();
        }
    }

    /// A kernel frame (`owner.out`, relayed by sup.rs) to the clients: stamped `src:"kernel"`, overwriting any
    /// `src` the kernel set. The only public emitter, so a frame the Root did not build is never `src:"root"`.
    pub fn emit(&self, frame: &Value) {
        self.out(frame, "kernel");
    }

    /// One frame to every attached client and into the replay ring (§3.15), with `src` and the next `rseq`.
    fn out(&self, frame: &Value, src: &str) {
        let mut guard = lock(&self.ring);
        let ring = &mut *guard;
        let Some(line) = stamp(frame, src, ring.last + 1) else { return };
        ring.last += 1;
        ring.bytes += line.len();
        ring.frames.push_back((ring.last, line.clone()));
        while ring.frames.len() > self.replay.0 || ring.bytes > self.replay.1 {
            let Some(x) = ring.frames.pop_front() else { break };
            ring.bytes -= x.1.len();
        }
        lock(&self.clients).retain_mut(|c| c.write_all(line.as_bytes()).is_ok());
    }

    pub fn notice(&self, level: &str, text: &str) {
        self.out(&json!({"level": level, "t": "notice", "text": text, "ts": rfc3339(now_ms())}), "root");
    }

    /// A Root notice with a `code` (SBX-009: `access`, `snapshot`), so a client can react without parsing text.
    pub fn notice_code(&self, level: &str, code: &str, text: &str) {
        self.out(&json!({"code": code, "level": level, "t": "notice", "text": text, "ts": rfc3339(now_ms())}), "root");
    }

    /// A notice per `ceiling_hit` record, at most one per ceiling a minute; no `alerts.jsonl`, no `notify.cmd`.
    fn ceiling_notice(&self, d: &Value) {
        let (name, now) = (d["ceiling"].as_str().unwrap_or("unknown"), now_ms());
        {
            let mut hits = lock(&self.hits);
            if hits.get(name).is_some_and(|&t| now - t < 60_000) {
                return;
            }
            hits.insert(name.to_string(), now);
        }
        let unit = if name.starts_with("money.") { " micro-USD" } else { "" };
        let of = match (d["attempted"].as_i64(), d["limit"].as_i64()) {
            (Some(a), Some(l)) => format!("; attempted {a} of {l}{unit}"),
            _ => String::new(),
        };
        let demotes = if d["demotes"] == true { " (counts toward demotion)" } else { "" };
        let text = format!("[root] ceiling reached: {name} ({}){of}; refused{demotes}", d["code"].as_str().unwrap_or("unknown"));
        self.out(&json!({"code": "ceiling_hit", "level": "alert", "t": "notice", "text": text, "ts": rfc3339(now)}), "root");
    }

    pub fn alert(&self, code: &str, msg: &str) {
        self.alert_from("root", code, msg);
    }

    /// §4.6: a notice to every client (with its `code`; `src:"kernel"` for a kernel's alert), a line in
    /// `alerts.jsonl`, and `notify.cmd` (no shell, 10 s).
    pub fn alert_from(&self, source: &str, code: &str, msg: &str) {
        let n = json!({"code": code, "level": "alert", "t": "notice", "text": msg, "ts": rfc3339(now_ms())});
        self.out(&n, if source == "kernel" { "kernel" } else { "root" });
        let a = json!({"code": code, "message": msg, "source": source, "ts": rfc3339(now_ms()), "v": 1});
        let _ = util::append_line(&self.p.r("alerts.jsonl"), &a);
        let cmd = lock(&self.st).charter.as_ref().map(|l| l.ch.notify.cmd.clone()).unwrap_or_default();
        let Some((exe, args)) = cmd.split_first() else { return };
        let child = Command::new(exe).args(args).env_clear().env("PATH", "/usr/bin:/bin").stdin(Stdio::piped())
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        let Ok(mut c) = child else { return };
        let line = icj(&a).unwrap_or_default() + "\n";
        std::thread::spawn(move || {
            if let Some(mut i) = c.stdin.take() {
                let _ = i.write_all(line.as_bytes());
            }
            for _ in 0..100 {
                if c.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = c.kill(); // our own child, by its handle
            let _ = c.wait();
        });
    }

    pub fn status(&self) -> Value {
        let head = lock(&self.lin).as_ref().and_then(|l| l.head.clone());
        let st = lock(&self.st);
        let ch = st.charter.as_ref().map(|l| &l.ch);
        let max = ch.map(|c| c.autonomy.max_level);
        let money = |s: Option<&String>| s.and_then(|s| charter::musd(s));
        json!({"v": 1, "slot": st.slot.name(), "mode": st.mode,
            "halted": if let Slot::Halted { reason, .. } = &st.slot { json!(reason) } else { Value::Null },
            "panic": if let Slot::Panicked { at, channel } = &st.slot { json!({"at": rfc3339(*at), "channel": channel}) } else { Value::Null },
            "level": st.level.level, "cap": max.map(ledger::cap), "max_level": max, "supported_max_level": ledger::SUPPORTED_MAX_LEVEL,
            "charter_sha256": st.charter.as_ref().map(|l| &l.digest), "charter_pending": st.pending,
            "ceilings": {"day_musd": money(ch.map(|c| &c.money.day_usd)), "total_musd": money(ch.map(|c| &c.money.total_usd))},
            "tuples": lock(&self.sup.ptrs).json(), "last_audit": null, "instances": self.zone.list(),
            "kernel": self.sup.kernel().map(|k| json!({"instance": k.inst, "pid": k.pid, "ready": k.ready.load(std::sync::atomic::Ordering::SeqCst),
                "reason": k.plan.reason, "boot_seq": self.sup.boot_seq()})),
            "sweep": if cfg!(feature = "live-sweep") { "d1" } else { "held_only" },
            "mirror": crate::lineage::mirror::status(),
            "queued": st.queued, "lineage": {"hash": head.as_ref().map(|h| &h.hash), "seq": head.as_ref().map(|h| h.seq)},
            "root": {"epoch": st.epoch(), "now": rfc3339(now_ms()), "pid": std::process::id(), "started_at": rfc3339(st.started),
                "test_mode": ch.is_some_and(|c| c.root.test_mode), "version": install::VERSION}})
    }

    /// `status` with the SBX-009 `access` block (AccessView); computed without `st` held.
    pub fn status_full(&self) -> Value {
        let mut v = self.status();
        v["access"] = crate::access::view(self);
        v
    }

    pub fn ledger(&self) -> Value {
        let st = lock(&self.st);
        st.level.json(st.charter.as_ref().map_or(0, |l| l.ch.autonomy.max_level))
    }

    /// `charter.apply` (§3.12.4): the Root re-reads and re-validates the file itself, then records and applies it.
    pub fn apply_charter(&self) -> Result<Value, Refusal> {
        let l = charter::load(&self.p.r("charter.toml"), &self.p.ctx())
            .map_err(|f| ("root.args", f.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")))?;
        let l = crate::grant::merged(&self.p, l); // X7: the grant overlay stays on top of a newly applied charter
        let ie = |e: io::Error| ("root.internal", e.to_string());
        let mut lg = lock(&self.lin);
        let lin = lg.as_mut().ok_or(("root.mode", "the lineage is unavailable (Halted)".to_string()))?;
        let (old, level) = {
            let st = lock(&self.st);
            (st.charter.as_ref().map(|o| (charter::effective(&o.ch), o.digest.clone(), o.ch.constitution.text.clone())), st.level.level)
        };
        if old.as_ref().is_some_and(|o| o.1 == l.digest) {
            lock(&self.st).pending = false;
            return Ok(json!({"changed": [], "digest": l.digest}));
        }
        let sandbox_changed = old.as_ref().is_some_and(|o| o.0["sandbox"] != charter::effective(&l.ch)["sandbox"]);
        write_atomic(&self.p.r("state/charter.active.toml"), l.src.as_bytes(), 0o600).map_err(ie)?;
        let rec = install::charter_record(lin, &l, old.as_ref().map(|o| &o.0), "owner").map_err(ie)?;
        let mut recs = vec![rec.clone()];
        if old.as_ref().is_none_or(|o| o.2 != l.ch.constitution.text) && self.p.i("kernel").is_dir() {
            write_atomic(&self.p.i("kernel/CONSTITUTION.md"), l.ch.constitution.text.as_bytes(), 0o644).map_err(ie)?;
        }
        if l.ch.autonomy.max_level < level {
            let cap = json!({"cause": "charter_max", "direction": "cap", "from": level, "to": l.ch.autonomy.max_level,
                "trigger": {"kind": "charter", "seq": rec["seq"]}});
            recs.push(lin.append("root", "ladder", cap).map_err(ie)?);
        }
        // money, classes and keys → egress::reload; quotas, ladder, mirror and root.* apply at their next use (§3.12.4)
        let ch = l.ch.clone();
        {
            let mut st = lock(&self.st);
            recs.iter().for_each(|r| st.level.apply(r));
            (st.charter, st.pending) = (Some(l), false);
        }
        drop(lg);
        recs.iter().for_each(|r| self.after(r));
        if let Some(e) = self.egress.get() {
            e.reload(&ch);
        }
        self.sync_egress();
        self.publish();
        // F2 (demo 2026-10-25): a new `extend.mode` reaches the running kernel now (the `ceilings` event carries
        // `extend`), so its next promote takes the new path instead of failing at handoff as `unwatched_change`.
        if old.as_ref().is_some_and(|o| o.0["extend"]["mode"] != charter::effective(&ch)["extend"]["mode"]) && let Some(k) = self.sup.kernel() {
            k.event("ceilings", crate::sup::ceilings(self));
        }
        if sandbox_changed {
            crate::access::charter_applied(self);
        }
        Ok(json!({"changed": rec["data"]["changed"], "digest": rec["data"]["digest"], "seq": rec["seq"]}))
    }

    fn request(self: &Arc<Self>, v: &Value) -> Value {
        // SBX-009: the access commands refuse with `error.data` (root.confirm's phrase, root.mode's reason).
        let a3: Option<Result<Value, crate::zone::Err3>> = match (v["v"] == 1, v["cmd"].as_str().unwrap_or("")) {
            (true, "access.get") => Some(Ok(crate::access::view(self))),
            (true, "workspace.set") => Some(crate::access::workspace_set(self, &v["args"])),
            (true, "full.set") => Some(crate::access::full_set(self, &v["args"])),
            (true, "undo") => Some(crate::access::undo(self, &v["args"])),
            _ => None,
        };
        if let Some(r) = a3 {
            return match r {
                Ok(res) => json!({"id": v["id"], "ok": true, "result": res, "v": 1}),
                Err((code, m, data)) => json!({"error": {"code": code, "data": data, "details": [], "message": m}, "id": v["id"], "ok": false, "v": 1}),
            };
        }
        let r: Result<Value, Refusal> = if v["v"] != 1 {
            Err(("root.version", "unsupported protocol version".into()))
        } else {
            match v["cmd"].as_str().unwrap_or("") {
                "status" => Ok(self.status_full()),
                "ledger" => Ok(self.ledger()),
                "panic" => Ok(panic::panic(self, "owner_socket")),
                "resume" => panic::resume(self),
                "stop" | "start" => {
                    let slot = lock(&self.st).slot.clone();
                    match (&slot, v["cmd"] == "start") {
                        (Slot::Panicked { .. }, true) => Err(("root.mode", "panicked: run init-root resume".into())),
                        (s, start) => {
                            self.sup.send(if start { Ev::Start(Plan::plain("cold")) } else { Ev::Stop });
                            Ok(json!({"slot": s.name()}))
                        }
                    }
                }
                "charter.apply" => self.apply_charter(),
                // UX-KEYMODEL (providers.rs); `keys.set` is answered in `conn`, which zeroes the secret
                "providers.list" => crate::providers::list_req(self),
                "providers.add" => crate::providers::add_req(self, &v["args"]),
                "model.use" => crate::providers::use_req(self, &v["args"]),
                "keys.reload" => Ok(self.egress.get().map_or_else(|| json!({"reloaded": false}), |e| e.reload_keys())),
                "egress.stats" => self.egress.get().map(|e| e.ceilings()).ok_or(("root.mode", "the egress is not running".into())),
                "audit.run" | "sealed.gog" | "sealed.draw" | "sealed.calibrate" | "sealed.probe" | "sealed.milestone" | "sealed.slice" => {
                    crate::sealed::request(self, v["cmd"].as_str().unwrap_or(""), &v["args"])
                }
                #[cfg(feature = "test-fixtures")]
                "test.clock" => crate::testfix::clock(self, &v["args"]),
                #[cfg(feature = "test-fixtures")]
                "test.append" => crate::testfix::append(self, &v["args"]),
                c => Err(("root.unknown_cmd", format!("unknown command {c:?}"))),
            }
        };
        match r {
            Ok(res) => json!({"id": v["id"], "ok": true, "result": res, "v": 1}),
            Err((code, m)) => json!({"error": {"code": code, "details": [], "message": m}, "id": v["id"], "ok": false, "v": 1}),
        }
    }

    /// `attach{replay, since?, epoch?}`: the ring (only `rseq > since` when `since` is given and `epoch` is
    /// absent or this Root's), then `replay_end{epoch, gap}` and the greeting, both `src:"root"` with no `rseq`.
    fn attach(&self, w: &UnixStream, v: &Value) -> io::Result<()> {
        let mut c = w.try_clone()?;
        c.set_write_timeout(Some(Duration::from_secs(1)))?;
        let (slot, level, queued, epoch) = { let st = lock(&self.st); (st.slot.name(), st.level.level, st.queued, st.epoch()) };
        let replay = v["replay"] != false;
        let since = v["since"].as_u64().filter(|_| replay && (v["epoch"].is_null() || v["epoch"] == epoch.as_str()));
        let ring = lock(&self.ring);
        let mut cl = lock(&self.clients);
        for (_, l) in ring.frames.iter().filter(|f| replay && since.is_none_or(|s| f.0 > s)) {
            c.write_all(l.as_bytes())?;
        }
        let oldest = ring.frames.front().map_or(ring.last + 1, |f| f.0);
        let end = json!({"epoch": epoch, "gap": since.is_some_and(|s| oldest > s.saturating_add(1)), "src": "root", "t": "replay_end"});
        let hello = format!("[root] attached: slot {slot}, level L{level}, {queued} queued. Type /panic to stop everything, /root help for more.");
        let hello = json!({"level": "info", "src": "root", "t": "notice", "text": hello, "ts": rfc3339(now_ms())});
        c.write_all(format!("{}\n{}\n", icj(&end)?, icj(&hello)?).as_bytes())?;
        cl.push(c);
        Ok(())
    }

    /// Owner lines (§4.2): `/panic` first, then the Root's own lines; everything else is queued for the kernel.
    fn line(self: &Arc<Self>, text: &str) {
        let t = text.trim();
        let rootish = matches!(lock(&self.st).slot, Slot::Panicked { .. } | Slot::Halted { .. } | Slot::Stopped);
        match t {
            "/panic" => drop(panic::panic(self, "owner_socket")),
            "/resume" if rootish => match panic::resume(self) {
                Ok(_) => {}
                Err((_, m)) => self.notice("warn", &format!("[root] {m}")),
            },
            "/root status" => self.notice("info", &format!("[root] {}", icj(&self.status()).unwrap_or_default())),
            "/root ledger" => self.notice("info", &format!("[root] {}", icj(&self.ledger()).unwrap_or_default())),
            "/root stop" => {
                self.sup.send(Ev::Stop);
                self.notice("info", "[root] stopping the zone (init-root start or /resume starts it again)");
            }
            "/root help" | "/root" => self.notice("info", "[root] /panic · /resume · /root status · /root ledger · /root stop · /root help · /workspace · /full on|off · /undo · /rollback [sha|cap|settled] · /grants · /grant <id> · /deny <id> · /release <cap> · /twin [refreeze] · /goal [\"<intent>\" …|pause|resume|stop|continue|accept|reject|adopt <id>]"),
            _ if crate::sentinel::owner_text(self, t) => {} // X3: `/rollback` (H3)
            _ if crate::goal::line(self, t) => {} // ADR-013: `/goal …` (only the owner sets a goal)
            _ if crate::grant::line(self, t) => {} // X7: `/grants`, `/grant`, `/deny`, `/release` (ADR-011 §7.4, §6.3)
            _ if crate::twin::owner_text(self, t) => {} // X9: `/twin`, `/twin refreeze` (ADR-011 Evaluation plan)
            _ if crate::access::line(self, t) => {}
            // The kernel link carries this line inside `owner.in{qid, frame}`; past ~1 MiB that frame
            // would overflow FRAME_MAX, the link would drop and the queued line would crash-loop the
            // kernel at every restart (K-LINE). Refuse it here, as the TUI does.
            _ if text.len() > OWNER_LINE_MAX => self.notice(
                "warn",
                &format!("[root] line refused: {} bytes is over the {} byte limit for one owner line", text.len(), OWNER_LINE_MAX),
            ),
            _ => self.enqueue(json!({"t": "line", "text": text})),
        }
    }

    /// `owner.in{qid, frame}` to a ready kernel (§3.15); false when none runs. The `qid` and the line are
    /// kept (1,000) for the `owner` record check (§3.10.5).
    pub fn deliver(&self, qid: &str, frame: &Value) -> bool {
        let Some(k) = self.sup.kernel().filter(|k| k.ready.load(std::sync::atomic::Ordering::SeqCst)) else { return false };
        let mut pend = lock(&k.pending); // held across the send, so the answer always finds its qid
        if !pend.1.insert(qid.into()) {
            return true; // already sent to this kernel (a line typed while `ready` redelivers)
        }
        pend.0.insert(k.request("owner.in", json!({"frame": frame, "qid": qid})), qid.into());
        let mut rel = lock(&self.sup.relayed);
        rel.push_back((qid.into(), frame["text"].as_str().unwrap_or("").into()));
        if rel.len() > 1000 {
            rel.pop_front();
        }
        drop((rel, pend));
        crate::sentinel::relayed(self, qid, frame); // X3: owner turns, steers, interrupts, deafness
        crate::twin::relayed(self, qid, frame); // X9: a session's first line enters the replay store
        true
    }

    fn queue(&self) -> Vec<Value> {
        let s = std::fs::read_to_string(self.p.r("state/owner-queue.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// After each `ready`: every unacknowledged line again, in order; the kernel deduplicates by `qid`.
    pub fn redeliver(&self) {
        for e in self.queue() {
            self.deliver(e["qid"].as_str().unwrap_or(""), &e["frame"]);
        }
    }

    /// The kernel holds the line durably: drop it from the queue.
    pub fn ack(&self, qid: &str) {
        crate::sentinel::acked(self, qid); // X3: H2
        let rest: Vec<Value> = self.queue().into_iter().filter(|e| e["qid"] != qid).collect();
        let body: String = rest.iter().filter_map(|e| icj(e).ok()).map(|l| l + "\n").collect();
        if write_atomic(&self.p.r("state/owner-queue.jsonl"), body.as_bytes(), 0o600).is_ok() {
            lock(&self.st).queued = rest.len();
        }
    }

    /// The owner queue (`state/owner-queue.jsonl`, fsynced): held until a kernel acknowledges.
    fn enqueue(&self, frame: Value) {
        let (n, max) = {
            let st = lock(&self.st);
            (st.queued, st.charter.as_ref().map_or(200, |l| l.ch.root.owner_queue_max as usize))
        };
        if n >= max {
            return self.notice("warn", &format!("[root] the owner queue is full ({max}); your message was not queued"));
        }
        let qid = util::hex(&util::random::<8>().unwrap_or_default());
        let text = frame["text"].clone();
        if let Err(e) = util::append_line(&self.p.r("state/owner-queue.jsonl"), &json!({"frame": frame, "qid": qid, "v": 1})) {
            return self.notice("warn", &format!("[root] could not queue your message: {e}"));
        }
        lock(&self.st).queued = n + 1;
        if !self.deliver(&qid, &frame) {
            self.out(&json!({"qid": qid, "t": "echo", "text": text}), "root"); // a running kernel echoes it itself
            self.notice("info", &format!("[root] the system is restarting; your message is queued ({})", n + 1));
        }
    }

    fn conn(self: Arc<Self>, s: UnixStream) {
        let peer = sys::peer(s.as_raw_fd());
        if !peer.as_ref().is_ok_and(|&(uid, pid)| uid == sys::euid() && sys::sandboxed(pid) != Some(true)) {
            return util::log(&self.p.rh, "warn", &format!("owner.sock: peer rejected ({peer:?}: other uid or sandboxed)"));
        }
        let Ok(mut w) = s.try_clone() else { return };
        let mut rd = BufReader::new(s);
        loop {
            let mut buf = String::new();
            if !matches!((&mut rd).take(FRAME_MAX + 1).read_line(&mut buf), Ok(n) if n > 0) || !buf.ends_with('\n') {
                break;
            }
            let Ok(v) = serde_json::from_str::<Value>(&buf) else {
                let _ = w.write_all(b"{\"error\":{\"code\":\"root.proto\",\"details\":[],\"message\":\"malformed frame\"},\"id\":null,\"ok\":false,\"v\":1}\n");
                continue;
            };
            if v.get("cmd").is_some() {
                let mut v = v;
                let a = if v["cmd"] == "keys.set" && v["v"] == 1 {
                    // UX-KEYMODEL: the secret is taken out of the request and zeroed; nothing of it is logged
                    let r = crate::providers::keys_set_req(&self, &mut v);
                    let mut raw = std::mem::take(&mut buf).into_bytes();
                    sys::zero(&mut raw);
                    match r {
                        Ok(res) => json!({"id": v["id"], "ok": true, "result": res, "v": 1}),
                        Err((code, m)) => json!({"error": {"code": code, "details": [], "message": m}, "id": v["id"], "ok": false, "v": 1}),
                    }
                } else {
                    self.request(&v)
                };
                let r = icj(&a).unwrap_or_default() + "\n";
                if w.write_all(r.as_bytes()).is_err() {
                    break;
                }
                continue;
            }
            match v["t"].as_str() {
                Some("panic") => drop(panic::panic(&self, "owner_socket")),
                Some("attach") if self.attach(&w, &v).is_ok() => {}
                Some("line") => self.line(v["text"].as_str().unwrap_or("")),
                // STEER: `interrupt{then?}` and `retract{seq}` go straight to a ready kernel, never queued
                Some(t @ ("interrupt" | "retract")) => {
                    let sent = control_frame(&v).map(|f| self.deliver(&util::hex(&util::random::<8>().unwrap_or_default()), &f));
                    if t == "retract" && sent == Some(false) {
                        self.notice("info", "[root] nothing taken back: the kernel is not running, so the line stays queued");
                    }
                }
                _ => break,             // detach, a failed attach, or an unknown frame
            }
        }
        let _ = w.shutdown(std::net::Shutdown::Both);
    }
}

/// STEER (§4.2): the `owner.in` frame for a client `interrupt` or `retract`. `then` is kept only when it is
/// `"continue"` (interrupt-and-steer); a `retract` needs an integer `seq` (the kernel echo's). None: nothing
/// to forward (the frame is ignored; the connection stays up).
fn control_frame(v: &Value) -> Option<Value> {
    match v["t"].as_str()? {
        "interrupt" if v["then"] == "continue" => Some(json!({"t": "interrupt", "then": "continue"})),
        "interrupt" => Some(json!({"t": "interrupt"})),
        "retract" => v["seq"].as_u64().map(|seq| json!({"seq": seq, "t": "retract"})),
        _ => None,
    }
}

/// A ring line (ICJ): `frame` with `rseq` and `src`, overwriting any the sender set. None for a frame that
/// is not an object or not ICJ.
fn stamp(frame: &Value, src: &str, rseq: u64) -> Option<String> {
    let mut f = frame.as_object()?.clone();
    f.insert("rseq".into(), json!(rseq));
    f.insert("src".into(), json!(src));
    icj(&Value::Object(f)).ok().map(|s| s + "\n")
}

/// Binds `run/owner.sock` (0600) and serves it forever, one thread per client.
pub fn serve(root: Arc<Root>) -> io::Result<()> {
    let sock = root.p.r("run/owner.sock");
    if std::fs::symlink_metadata(&sock).is_ok_and(|m| m.file_type().is_socket()) {
        std::fs::remove_file(&sock)?; // a stale socket of a dead Root; we hold run/root.lock
    }
    let l = UnixListener::bind(&sock)?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    util::log(&root.p.rh, "info", &format!("owner.sock up, pid {}", std::process::id()));
    for c in l.incoming().flatten() {
        let r = root.clone();
        std::thread::spawn(move || r.conn(c));
    }
    Ok(())
}

/// Why a CLI request did not produce a result.
#[derive(Debug)]
pub enum Fail {
    Unreachable(io::Error),
    Protocol(String),
    Refused(String, String),
}

/// One request frame over `owner.sock` (the CLI side).
pub fn request(p: &Paths, cmd: &str, args: Value, timeout: Duration) -> Result<Value, Fail> {
    let mut s = UnixStream::connect(p.r("run/owner.sock")).map_err(Fail::Unreachable)?;
    s.set_read_timeout(Some(timeout)).map_err(|e| Fail::Protocol(e.to_string()))?;
    let req = icj(&json!({"args": args, "cmd": cmd, "id": "c-1", "v": 1})).unwrap_or_default();
    writeln!(s, "{req}").map_err(|e| Fail::Protocol(e.to_string()))?;
    let mut line = String::new();
    BufReader::new(&s).take(FRAME_MAX).read_line(&mut line).map_err(|e| Fail::Protocol(e.to_string()))?;
    let v: Value = serde_json::from_str(&line).map_err(|_| Fail::Protocol(if line.is_empty() { "connection closed".into() } else { "malformed response".into() }))?;
    if v["ok"] == true {
        return Ok(v["result"].clone());
    }
    let e = &v["error"];
    Err(Fail::Refused(e["code"].as_str().unwrap_or("").into(), e["message"].as_str().unwrap_or("").into()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// tui.md §9 Q1: a kernel cannot forge `src:"root"` or an `rseq`; the stamp overwrites both, in ICJ.
    #[test]
    fn stamp_overwrites_src_and_rseq() {
        let forged = json!({"level": "alert", "rseq": 1, "src": "root", "t": "notice", "text": "[root] level L0→L4"});
        let line = super::stamp(&forged, "kernel", 42).unwrap();
        assert_eq!(line, "{\"level\":\"alert\",\"rseq\":42,\"src\":\"kernel\",\"t\":\"notice\",\"text\":\"[root] level L0→L4\"}\n");
        assert!(super::stamp(&json!("not an object"), "kernel", 1).is_none());
        assert!(super::stamp(&json!({"t": "display", "x": 1.5}), "kernel", 1).is_none(), "not ICJ");
    }

    /// STEER: `then` survives only as `"continue"`; `retract` needs an integer `seq`; other frames are not control frames.
    #[test]
    fn control_frames_for_the_kernel() {
        let f = |v| super::control_frame(&v);
        assert_eq!(f(json!({"t": "interrupt"})), Some(json!({"t": "interrupt"})));
        assert_eq!(f(json!({"t": "interrupt", "then": "continue"})), Some(json!({"t": "interrupt", "then": "continue"})));
        for then in [json!("stop"), json!(1), json!(null), json!({"x": 1})] {
            assert_eq!(f(json!({"t": "interrupt", "then": then})), Some(json!({"t": "interrupt"})), "{then}");
        }
        assert_eq!(f(json!({"seq": 7, "t": "retract", "x": 1})), Some(json!({"seq": 7, "t": "retract"})));
        for seq in [json!(null), json!(-1), json!(1.5), json!("7")] {
            assert_eq!(f(json!({"seq": seq, "t": "retract"})), None, "{seq}");
        }
        assert_eq!(f(json!({"t": "retract"})), None);
        assert_eq!(f(json!({"t": "line", "text": "x"})), None);
    }
}
