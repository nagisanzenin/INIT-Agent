//! Cold-start recovery from the Root's tuple (supervise.md §3.9 step 6; root/root.md §3.7.3).
//!
//! After a whole-system rollback (`rollback_system`, `demotion`, `panic_resume`) the Root's tuple
//! wins over every local file: each of `bin/init-body`, `bin/init-body.lkg` and the kept
//! components whose SHA-256 differs from the tuple is fetched with `store.get` and written as a
//! new inode (macOS code-signing rule), and `bin/ext`/`bin/ext.lkg` are re-pointed. For any other
//! reason only the LKG side is checked, because a provisional body may run ahead of the tuple.

use crate::binstore;
use crate::link::{RootLink, STORE_TIMEOUT};
use init_proto::root_link::{KernelRequest, StoreGetArgs, TupleBody};
use std::path::Path;

/// Fetch blob `sha256` from the Root's store into `bin/<name>` (new inode); checks the hash.
pub fn store_get_into(link: &RootLink, dir: &Path, name: &str, sha256: &str) -> Result<(), String> {
    let (_, mut fds) = link
        .call_fds(&KernelRequest::StoreGet(StoreGetArgs { sha256: sha256.to_string() }), 1, STORE_TIMEOUT)
        .map_err(|e| format!("store.get {}: {e}", &sha256[..sha256.len().min(12)]))?;
    let mut f = std::fs::File::from(fds.pop().unwrap());
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let got = binstore::write_new_inode(dir, name, &mut f).map_err(|e| format!("write {name}: {e}"))?;
    if got != sha256 {
        let _ = std::fs::remove_file(dir.join(name));
        return Err(format!("store.get {name}: hash mismatch"));
    }
    Ok(())
}

/// Re-point a `bin/ext*` symlink to `ext.d/<set>` atomically (`.ext.<name>.tmp` + rename).
fn point_ext(bin: &Path, link_name: &str, set: &str) -> std::io::Result<()> {
    let tmp = bin.join(format!(".{link_name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(format!("ext.d/{set}"), &tmp)?;
    std::fs::rename(&tmp, bin.join(link_name))
}

/// ADR-011 X0b: at kernel start (after the job store marked interrupted jobs `kernel_restart`),
/// every proposal past `screened` resumes from the Root's records, read from its published lineage
/// view (`INIT_HOME/root/lineage.jsonl`, `by:"root"` `change_open`/`change_close` only):
/// `opening` → `open` in the Root's group (and its close, if the group closed meanwhile); a proposal
/// with nothing live left (`opening` never opened, `promoted` whose gate or G4 job the restart
/// ended) → `screened`. Each transition is logged as a kernel-local `proposal` line. Returns how
/// many transitions were made.
pub fn resume_proposals(home: &Path, lineage: &dyn crate::promote::GateLineage) -> usize {
    let root = kernel_evo::proposal::root_change_records(home);
    let recs = kernel_evo::proposal::resume(home, &root);
    for r in &recs {
        lineage.local("proposal", r.clone());
    }
    recs.len()
}

/// X0b's state hook for the pipeline: apply `ev` to the proposals whose chosen variant is `sha`
/// and log the transitions (gated path: G4's final verdict; a rollback without one → `screened`).
pub fn proposal_event(home: &Path, lineage: &dyn crate::promote::GateLineage, sha: &str, ev: &kernel_evo::proposal::ChangeEv) {
    for r in kernel_evo::proposal::on_sha(home, sha, ev) {
        lineage.local("proposal", r);
    }
}

/// §3.9 step 6. Returns the steps taken.
pub fn restore_from_tuple(home: &Path, link: &RootLink, tb: &TupleBody, rollback_class: bool) -> Result<Vec<String>, String> {
    let bin = home.join("bin");
    let mut done = Vec::new();
    if tb.binary.is_empty() {
        return Ok(done); // genesis: no tuple yet (install mode)
    }
    let names: &[&str] = if rollback_class { &["init-body", "init-body.lkg"] } else { &["init-body.lkg"] };
    for name in names {
        if binstore::sha_of(&bin, name).as_deref() != Some(tb.binary.as_str()) {
            store_get_into(link, &bin, name, &tb.binary)?;
            done.push(format!("restored bin/{name} from the Root's store"));
        }
    }
    if let Some(set) = &tb.ext.set {
        let dir = bin.join("ext.d").join(set);
        for c in &tb.ext.components {
            let file = format!("{}.wasm", c.name);
            if crate::fsx::sha256_file(&dir.join(&file)).ok().as_deref() != Some(c.sha256.as_str()) {
                store_get_into(link, &dir, &file, &c.sha256)?;
                done.push(format!("restored component {}", c.name));
            }
        }
        let links: &[&str] = if rollback_class { &["ext", "ext.lkg"] } else { &["ext.lkg"] };
        for l in links {
            let want = format!("ext.d/{set}");
            if std::fs::read_link(bin.join(l)).ok().map(|p| p.to_string_lossy().into_owned()).as_deref() != Some(want.as_str()) {
                point_ext(&bin, l, set).map_err(|e| format!("bin/{l}: {e}"))?;
                done.push(format!("bin/{l} → {want}"));
            }
        }
        let _ = crate::fsx::sync_dir(&bin);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Rec(Mutex<Vec<(String, Value)>>);
    impl crate::promote::GateLineage for Rec {
        fn append_wait(&self, _kind: &str, _data: Value) -> Result<u64, String> {
            Err("not in this test".into())
        }
        fn local(&self, kind: &str, data: Value) {
            self.0.lock().unwrap().push((kind.into(), data));
        }
    }

    fn proposal(id: &str, state: &str, sha: &str) -> Value {
        json!({"v": 1, "id": id, "scope": "inv", "frictions": [], "hypothesis": "h", "change": "c", "repro": [], "predicts": [],
            "subject": "s", "compensates_for": "n", "prefix_change": false, "lessons": [], "k": 1, "tier_hint": "t2",
            "s_measurable": true, "targets": [], "by": null, "state": state, "est": 300, "go": true,
            "chosen": {"k": 1, "sha": sha, "aim": "better", "s_measurable": true}, "variants": [], "created": "x", "history": []})
    }

    /// RF-01 through the Root's published lineage view: a restart resumes an `opening` proposal
    /// from the Root's `change_open` (a kernel-signed copy and other kinds ignored), and a stranded
    /// gated `promoted` one returns to `screened`; each transition is a kernel-local `proposal` line.
    #[test]
    fn rf01_resume_proposals_from_the_root_lineage_view() {
        let home = crate::testutil::test_dir("k1-rf01");
        let pdir = kernel_evo::proposal::dir(&home);
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::create_dir_all(home.join("root")).unwrap();
        for (id, st, sha) in [("p-000001", "opening", "a1a1"), ("p-000002", "promoted", "b2b2")] {
            std::fs::write(pdir.join(format!("{id}.json")), serde_json::to_vec(&proposal(id, st, sha)).unwrap()).unwrap();
        }
        let line = |seq: u64, by: &str, kind: &str, data: Value| json!({"at": "t", "by": by, "data": data, "kind": kind, "prev": "p", "seq": seq, "v": 1}).to_string();
        let view = [
            line(1, "root", "tuple", json!({"id": "x"})),
            line(2, "kernel:0123456789ab", "change_open", json!({"sha": "b2b2", "group": 2})),
            line(3, "root", "change_open", json!({"sha": "a1a1", "group": 3, "scope": "t2"})),
        ]
        .join("\n");
        std::fs::write(home.join("root/lineage.jsonl"), format!("{view}\n")).unwrap();
        let lin = Rec::default();
        assert_eq!(resume_proposals(&home, &lin), 2);
        let p1 = kernel_evo::proposal::load(&home, "p-000001").unwrap().unwrap();
        assert_eq!((p1.state.as_str(), p1.live.map(|c| (c.group, c.seq))), ("open", Some((3, Some(3)))));
        assert_eq!(kernel_evo::proposal::load(&home, "p-000002").unwrap().unwrap().state, "screened");
        let logged = lin.0.lock().unwrap();
        assert_eq!(logged.iter().map(|(k, d)| (k.as_str(), d["state"].as_str().unwrap())).collect::<Vec<_>>(), [("proposal", "open"), ("proposal", "screened")]);
        drop(logged);
        // No view at all: nothing to resume from; the `open` one stays open.
        std::fs::remove_file(home.join("root/lineage.jsonl")).unwrap();
        assert_eq!(resume_proposals(&home, &lin), 0);
        crate::testutil::remove_test_dir(&home);
    }
}
