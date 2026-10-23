//! The kernel's lineage refs in `lineage/body.git` (supervise.md §3.11.1, §3.11.4), behind a
//! small trait so the supervisor runs on homes without a lineage repo (fresh test homes).

use crate::config::KernelCfg;
use crate::git::Git;
use std::path::Path;
use std::time::Duration;

const T: Duration = Duration::from_secs(120);

/// The ref moves the supervisor makes.
pub trait LineageRefs: Send {
    /// Handoff commit / data-only promotion: `update-ref main <new> <old>` (skip if already).
    fn move_main(&self, new: &str, old: &str) -> Result<(), String>;
    /// Rollback: keep `refs/init/rolled-back/<from>` (create-only), then `main <to> <from>`.
    fn rollback_main(&self, from: &str, to: &str) -> Result<(), String>;
    /// LKG commit or reset: `refs/heads/lkg` ← `sha`.
    fn set_lkg(&self, sha: &str) -> Result<(), String>;
    /// The current `main`.
    fn main(&self) -> Option<String>;
}

/// `lineage/body.git` through the hardened prefix `$G`.
pub struct GitRefs(pub Git);

impl LineageRefs for GitRefs {
    fn move_main(&self, new: &str, old: &str) -> Result<(), String> {
        if self.main().as_deref() == Some(new) {
            return Ok(());
        }
        self.0.update_ref("refs/heads/main", new, Some(old), Some(&format!("init: handoff {}->{}", short(old), short(new)))).map_err(|e| e.to_string())
    }

    fn rollback_main(&self, from: &str, to: &str) -> Result<(), String> {
        let keep = format!("refs/init/rolled-back/{from}");
        if self.0.rev_parse_commit(&keep).is_none() && self.0.rev_parse_commit(from).is_some() {
            self.0.update_ref(&keep, from, Some(""), None).map_err(|e| e.to_string())?;
        }
        let cur = self.main().unwrap_or_default();
        if cur == to {
            return Ok(());
        }
        self.0.update_ref("refs/heads/main", to, Some(&cur), Some(&format!("init: rollback {}->{}", short(&cur), short(to)))).map_err(|e| e.to_string())
    }

    fn set_lkg(&self, sha: &str) -> Result<(), String> {
        let old = self.0.rev_parse_commit("refs/heads/lkg").unwrap_or_default();
        if old == sha {
            return Ok(());
        }
        self.0.update_ref("refs/heads/lkg", sha, Some(&old), Some(&format!("init: lkg {}", short(sha)))).map_err(|e| e.to_string())
    }

    fn main(&self) -> Option<String> {
        self.0.rev_parse_commit("refs/heads/main")
    }
}

/// No lineage repo (a fresh test home): moves are logged and succeed.
pub struct NoLineage;

impl LineageRefs for NoLineage {
    fn move_main(&self, new: &str, old: &str) -> Result<(), String> {
        crate::klog::log("lineage", &format!("no lineage/body.git: main {old} -> {new} not recorded"));
        Ok(())
    }
    fn rollback_main(&self, from: &str, to: &str) -> Result<(), String> {
        self.move_main(to, from)
    }
    fn set_lkg(&self, _sha: &str) -> Result<(), String> {
        Ok(())
    }
    fn main(&self) -> Option<String> {
        None
    }
}

fn short(s: &str) -> String {
    s.chars().take(12).collect()
}

/// The lineage of `home`: [`GitRefs`] when `lineage/body.git` exists.
pub fn for_home(home: &Path, cfg: &KernelCfg) -> Box<dyn LineageRefs> {
    if home.join("lineage/body.git").is_dir() {
        let _ = T;
        Box::new(GitRefs(Git::body(&cfg.supervise.git, home)))
    } else {
        Box::new(NoLineage)
    }
}
