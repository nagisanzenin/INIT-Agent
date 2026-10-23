//! Serve-side daily count quotas (kernel/initctl.md §4.2 `E_QUOTA`): `vendor_day` (20),
//! `proposals_day` (5), `task_submits_day` (`budget.task_submits_day`, default 10) and
//! `fleet_spawns_day` (50). A request takes a unit before the backend runs and gives it back if
//! the synchronous part fails, so concurrent requests cannot overshoot. Counts persist in
//! `state/kernel/serve-quotas.json` and reset at 00:00 UTC.
//!
//! The evolution count keys (`promotes_day`, `quick_gates_day`, `promote_attempts_task`) belong
//! to evolution.md and are enforced by the backend.

use crate::clock;
use crate::fsx;
use init_proto::errors::{QuotaData, QuotaName};
use init_proto::initctl::CommandName;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The quota limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaLimits {
    pub vendor_day: u64,
    pub proposals_day: u64,
    pub task_submits_day: u64,
    pub fleet_spawns_day: u64,
}

impl Default for QuotaLimits {
    fn default() -> QuotaLimits {
        QuotaLimits { vendor_day: 20, proposals_day: 5, task_submits_day: 10, fleet_spawns_day: 50 }
    }
}

/// The quota a command counts against.
pub fn quota_of(cmd: CommandName) -> Option<QuotaName> {
    match cmd {
        CommandName::Vendor => Some(QuotaName::VendorDay),
        CommandName::KernelPropose => Some(QuotaName::ProposalsDay),
        CommandName::TaskSubmit => Some(QuotaName::TaskSubmitsDay),
        CommandName::FleetSpawn => Some(QuotaName::FleetSpawnsDay),
        _ => None,
    }
}

fn key(q: QuotaName) -> &'static str {
    match q {
        QuotaName::VendorDay => "vendor_day",
        QuotaName::ProposalsDay => "proposals_day",
        QuotaName::TaskSubmitsDay => "task_submits_day",
        QuotaName::FleetSpawnsDay => "fleet_spawns_day",
        QuotaName::PromotesDay => "promotes_day",
        QuotaName::QuickGatesDay => "quick_gates_day",
        QuotaName::PromoteAttemptsTask => "promote_attempts_task",
        // ADR-011 (X2): Root-enforced budgets, reported in `E_QUOTA` but never counted here.
        QuotaName::ExtendBudget => "extend_budget",
        QuotaName::ExtendBloat => "extend_bloat",
        QuotaName::ExtendArchiveDebt => "extend_archive_debt",
        QuotaName::ExtendQuarantineDebt => "extend_quarantine_debt",
        QuotaName::ExtendReadmit => "extend_readmit",
        QuotaName::ExtendDepGrant => "extend_dep_grant",
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct File {
    day: String,
    counts: BTreeMap<String, u64>,
}

struct Inner {
    path: PathBuf,
    limits: QuotaLimits,
    day: i64,
    counts: BTreeMap<String, u64>,
}

/// The daily counters. Cheap to clone.
#[derive(Clone)]
pub struct Quotas {
    inner: Arc<Mutex<Inner>>,
}

/// One taken unit; given back on drop unless committed.
pub struct QuotaTicket {
    q: Quotas,
    name: QuotaName,
    day: i64,
    committed: bool,
}

impl QuotaTicket {
    /// Keep the unit (the synchronous part succeeded).
    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for QuotaTicket {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut g = self.q.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.day == self.day
            && let Some(n) = g.counts.get_mut(key(self.name))
        {
            *n = n.saturating_sub(1);
            g.save();
        }
    }
}

impl Inner {
    fn limit(&self, q: QuotaName) -> u64 {
        match q {
            QuotaName::VendorDay => self.limits.vendor_day,
            QuotaName::ProposalsDay => self.limits.proposals_day,
            QuotaName::TaskSubmitsDay => self.limits.task_submits_day,
            QuotaName::FleetSpawnsDay => self.limits.fleet_spawns_day,
            _ => u64::MAX,
        }
    }

    fn roll(&mut self, now_ms: i64) {
        let today = clock::utc_day(now_ms);
        if today != self.day {
            self.day = today;
            self.counts.clear();
        }
    }

    fn save(&self) {
        let f = File { day: clock::day_string(self.day), counts: self.counts.clone() };
        let _ = fsx::write_json(&self.path, &f);
    }

    fn full(&self, q: QuotaName) -> Option<QuotaData> {
        let limit = self.limit(q);
        (self.counts.get(key(q)).copied().unwrap_or(0) >= limit)
            .then(|| QuotaData { quota: q, limit, resets_at: clock::day_start(self.day + 1) })
    }
}

impl Quotas {
    /// Load `path` (a missing or unreadable file starts at zero).
    pub fn open(path: &Path, limits: QuotaLimits) -> Quotas {
        let today = clock::utc_day(clock::now_ms());
        let counts = std::fs::read(path)
            .ok()
            .and_then(|b| init_proto::files::decode::<File>(&b).ok())
            .filter(|f| f.day == clock::day_string(today))
            .map(|f| f.counts)
            .unwrap_or_default();
        Quotas { inner: Arc::new(Mutex::new(Inner { path: path.to_path_buf(), limits, day: today, counts })) }
    }

    /// The `E_QUOTA` data when `cmd`'s quota is used up.
    pub fn peek(&self, cmd: CommandName) -> Option<QuotaData> {
        let q = quota_of(cmd)?;
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.roll(clock::now_ms());
        g.full(q)
    }

    /// Take one unit of `cmd`'s quota (`Ok(None)` when the command has none).
    pub fn take(&self, cmd: CommandName) -> Result<Option<QuotaTicket>, QuotaData> {
        let Some(q) = quota_of(cmd) else { return Ok(None) };
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.roll(clock::now_ms());
        if let Some(full) = g.full(q) {
            return Err(full);
        }
        *g.counts.entry(key(q).to_string()).or_insert(0) += 1;
        g.save();
        let day = g.day;
        drop(g);
        Ok(Some(QuotaTicket { q: self.clone(), name: q, day, committed: false }))
    }

    /// Units used today.
    pub fn used(&self, q: QuotaName) -> u64 {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.roll(clock::now_ms());
        g.counts.get(key(q)).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_refund_persist() {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/wp7/unit-quota-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("serve-quotas.json");
        let limits = QuotaLimits { proposals_day: 2, ..QuotaLimits::default() };
        let q = Quotas::open(&p, limits);
        q.take(CommandName::KernelPropose).unwrap().unwrap().commit();
        drop(q.take(CommandName::KernelPropose).unwrap().unwrap()); // refunded
        assert_eq!(q.used(QuotaName::ProposalsDay), 1);
        q.take(CommandName::KernelPropose).unwrap().unwrap().commit();
        let full = q.take(CommandName::KernelPropose).err().unwrap();
        assert_eq!((full.quota, full.limit), (QuotaName::ProposalsDay, 2));
        assert!(full.resets_at.ends_with("T00:00:00.000Z"));
        assert!(q.take(CommandName::Ping).unwrap().is_none());
        // Persisted across a restart.
        let q2 = Quotas::open(&p, limits);
        assert!(q2.peek(CommandName::KernelPropose).is_some());
        assert!(q2.peek(CommandName::Vendor).is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
