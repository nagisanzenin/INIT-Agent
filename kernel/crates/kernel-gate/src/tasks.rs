//! Agent-authored task families (kernel/gate.md §3.7.1, §3.7.5; v03-decisions D2, D3) and the G4 task
//! selection (§3.7.2).
//!
//! - **Sealing (D2):** ⌈n/2⌉ instances of a family are sealed at admission. Only visible instances enter
//!   the pool stratum; only sealed instances form the capability set A.
//! - **Eligibility (D3):** from Root lineage (R4) records only. A family judges a candidate C iff its
//!   registration `seq` precedes the R4 `promotion` record of the current LKG, C is more than K = 5
//!   promotions below the author, and it is not suspended.
//!
//! K and the admission repeats come from the Root (`[sealed]`, root/sealed-eval.md §4.3), never from
//! kernel config, so callers pass the Root's value; [`K_PROMOTIONS`] is the D3 default.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::stats::data::Source;
use crate::stats::rng::SplitMix64;

/// D3: a family never judges descendants of its author within K = 5 promotions.
pub const K_PROMOTIONS: u32 = 5;

/// `family.json` (§3.7.1): `{"v":1, "id", "labels", "instances"}`, 2 to 8 instances.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FamilyJson {
    pub v: u32,
    pub id: String,
    pub labels: Vec<String>,
    pub instances: Vec<String>,
}

fn valid_id(s: &str) -> bool {
    // ^[a-z0-9][a-z0-9._-]{0,63}$ (task.json id)
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|&c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}

fn valid_label(s: &str) -> bool {
    // ^[a-z0-9-]{1,40}$
    (1..=40).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

impl FamilyJson {
    /// The family-level part of the `gate.admit.schema` rule: v = 1, valid ids, 2–8 distinct instances,
    /// ≤ 16 unique labels. Returns every problem found.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        if self.v != 1 {
            errs.push(format!("family.json v must be 1, got {}", self.v));
        }
        if !valid_id(&self.id) {
            errs.push(format!(
                "family id `{}` does not match ^[a-z0-9][a-z0-9._-]{{0,63}}$",
                self.id
            ));
        }
        if !(2..=8).contains(&self.instances.len()) {
            errs.push(format!(
                "a family has 2 to 8 instances, this one has {}",
                self.instances.len()
            ));
        }
        let distinct: BTreeSet<&String> = self.instances.iter().collect();
        if distinct.len() != self.instances.len() {
            errs.push("instance ids repeat".into());
        }
        for i in &self.instances {
            if !valid_id(i) {
                errs.push(format!("instance id `{i}` is not a valid task id"));
            }
        }
        let labels: BTreeSet<&String> = self.labels.iter().collect();
        if labels.len() != self.labels.len() || self.labels.len() > 16 {
            errs.push("labels must be unique and at most 16".into());
        }
        for l in &self.labels {
            if !valid_label(l) {
                errs.push(format!("label `{l}` does not match ^[a-z0-9-]{{1,40}}$"));
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    /// Frozen instance ids: `<family id>.<id>`.
    pub fn instance_ids(&self) -> Vec<String> {
        self.instances
            .iter()
            .map(|i| format!("{}.{i}", self.id))
            .collect()
    }
}

/// D2 sealing: draw ⌈n/2⌉ of `instances` from 32 random bytes (the kernel reads them from
/// `/dev/urandom` at L1–L2; the Root draws at L3+). Returns `(sealed, visible)`, each sorted.
pub fn seal(instances: &[String], random: [u8; 32]) -> (Vec<String>, Vec<String>) {
    let mut ids: Vec<String> = instances.to_vec();
    ids.sort();
    let mut rng = SplitMix64::new(
        u64::from_be_bytes(random[..8].try_into().expect("8 bytes"))
            ^ u64::from_be_bytes(random[8..16].try_into().expect("8 bytes")),
    );
    rng.shuffle(&mut ids);
    let k = ids.len().div_ceil(2);
    let mut sealed: Vec<String> = ids[..k].to_vec();
    let mut visible: Vec<String> = ids[k..].to_vec();
    sealed.sort();
    visible.sort();
    (sealed, visible)
}

/// `sealed_commitment = sha256(salt ‖ sorted sealed instance ids)` (§3.7.5), with the ids joined by
/// `\n` (a separator the id grammar excludes), as `sha256:<hex>`.
pub fn sealed_commitment(salt: &[u8; 16], sealed: &[String]) -> String {
    let mut ids: Vec<&String> = sealed.iter().collect();
    ids.sort();
    let mut h = Sha256::new();
    h.update(salt);
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            h.update(b"\n");
        }
        h.update(id.as_bytes());
    }
    let d = h.finalize();
    let mut s = String::from("sha256:");
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The R4 records eligibility reads (root/root.md §3.10.5 kinds; synthetic in tests).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum R4Record {
    /// A family's registration; its `seq` is the registration anchor.
    TaskAdmit {
        seq: u64,
        family: String,
    },
    /// A body promotion reported by the kernel.
    Promotion {
        seq: u64,
        sha: String,
        lkg: bool,
    },
    TaskSuspended {
        seq: u64,
        family: String,
    },
    /// The Root rolled back to audited-good.
    Rollback {
        seq: u64,
    },
    /// A demotion.
    LadderDown {
        seq: u64,
    },
    /// A sealed audit; `pass` moves audited-good forward.
    Audit {
        seq: u64,
        pass: bool,
    },
}

impl R4Record {
    pub fn seq(&self) -> u64 {
        match self {
            R4Record::TaskAdmit { seq, .. }
            | R4Record::Promotion { seq, .. }
            | R4Record::TaskSuspended { seq, .. }
            | R4Record::Rollback { seq }
            | R4Record::LadderDown { seq }
            | R4Record::Audit { seq, .. } => *seq,
        }
    }
}

/// Why a family is not eligible (the record's `capability.excluded` counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Exclusion {
    /// No `task_admit` record, or registered at or after the current LKG's promotion record.
    LateRegistration,
    /// Within K promotions of its author.
    KGenerations,
    Suspended,
}

/// Eligibility of one family for one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Eligibility {
    pub registration_seq: Option<u64>,
    /// d = 1 + the ancestor `promotion{lkg:true}` records after registration.
    pub distance: u32,
    pub excluded: Option<Exclusion>,
}

impl Eligibility {
    pub fn eligible(&self) -> bool {
        self.excluded.is_none()
    }
}

fn registration(records: &[R4Record], family: &str) -> Option<u64> {
    records.iter().find_map(|r| match r {
        R4Record::TaskAdmit { seq, family: f } if f == family => Some(*seq),
        _ => None,
    })
}

/// Replays R4 in `seq` order: suspended by `task_suspended`, or by a Root `rollback` / `ladder{down}` to
/// an audited-good point that precedes the registration; lifted by the next passing sealed audit.
fn suspended(records: &[R4Record], family: &str, reg: u64) -> bool {
    let mut sorted: Vec<&R4Record> = records.iter().collect();
    sorted.sort_by_key(|r| r.seq());
    let mut audited_good: Option<u64> = None;
    let mut susp = false;
    for r in sorted {
        match r {
            R4Record::Audit { seq, pass: true } => {
                audited_good = Some(*seq);
                susp = false;
            }
            R4Record::TaskSuspended { family: f, .. } if f == family => susp = true,
            R4Record::Rollback { seq } | R4Record::LadderDown { seq }
                if *seq > reg && audited_good.is_none_or(|a| a < reg) =>
            {
                susp = true;
            }
            _ => {}
        }
    }
    susp
}

/// D3 eligibility of `family` for candidate C against the current LKG `lkg_sha`. `is_ancestor_of_c(sha)`
/// answers whether a promoted sha is an ancestor of (or equal to) C; `k` is the Root's K (5).
pub fn eligibility(
    records: &[R4Record],
    family: &str,
    lkg_sha: &str,
    is_ancestor_of_c: &dyn Fn(&str) -> bool,
    k: u32,
) -> Eligibility {
    let Some(reg) = registration(records, family) else {
        return Eligibility {
            registration_seq: None,
            distance: 0,
            excluded: Some(Exclusion::LateRegistration),
        };
    };
    let lkg_promo = records
        .iter()
        .filter_map(|r| match r {
            R4Record::Promotion {
                seq,
                sha,
                lkg: true,
            } if sha == lkg_sha => Some(*seq),
            _ => None,
        })
        .max();
    let distance = 1 + records
        .iter()
        .filter(|r| matches!(r, R4Record::Promotion { seq, sha, lkg: true } if *seq > reg && is_ancestor_of_c(sha)))
        .count() as u32;
    let excluded = if lkg_promo.is_none_or(|p| reg >= p) {
        Some(Exclusion::LateRegistration)
    } else if distance <= k {
        Some(Exclusion::KGenerations)
    } else if suspended(records, family, reg) {
        Some(Exclusion::Suspended)
    } else {
        None
    };
    Eligibility {
        registration_seq: Some(reg),
        distance,
        excluded,
    }
}

/// The families to suspend (R4 `task_suspended`) when a Root `rollback` or `ladder{down}` record arrives:
/// those registered after the audited-good point the Root returns to. Already-suspended ones are skipped.
pub fn families_to_suspend(
    records: &[R4Record],
    trigger: &R4Record,
    families: &[String],
) -> Vec<String> {
    if !matches!(
        trigger,
        R4Record::Rollback { .. } | R4Record::LadderDown { .. }
    ) {
        return vec![];
    }
    let t = trigger.seq();
    let audited_good = records
        .iter()
        .filter_map(|r| match r {
            R4Record::Audit { seq, pass: true } if *seq < t => Some(*seq),
            _ => None,
        })
        .max();
    families
        .iter()
        .filter(|f| match registration(records, f) {
            Some(reg) => {
                reg < t && audited_good.is_none_or(|a| a < reg) && !suspended(records, f, reg)
            }
            None => false,
        })
        .cloned()
        .collect()
}

/// The `capability.excluded` counts of the gate record.
pub fn excluded_counts(eligibilities: &[Eligibility]) -> BTreeMap<Exclusion, usize> {
    let mut m = BTreeMap::new();
    for e in eligibilities {
        if let Some(x) = e.excluded {
            *m.entry(x).or_insert(0) += 1;
        }
    }
    m
}

/// A frozen pool task as selection sees it. Agent tasks here are visible instances only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PoolTask {
    pub id: String,
    pub labels: Vec<String>,
    pub source: Source,
    pub family: Option<String>,
}

/// Selection parameters (`gate.g4.*`, `evolution.tasks.agent_stratum_max`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SelectParams {
    pub pool_k: usize,
    pub min_pool: usize,
    pub agent_stratum_max: usize,
    pub max_repro_tasks: usize,
}

impl Default for SelectParams {
    fn default() -> Self {
        SelectParams {
            pool_k: 12,
            min_pool: 8,
            agent_stratum_max: 6,
            max_repro_tasks: 5,
        }
    }
}

/// Stratified pick: round-robin over sorted labels, each label's tasks in a seeded shuffle (tasks
/// without labels form the `""` stratum); a task listed under several labels is taken once.
fn stratified(rng: &mut SplitMix64, tasks: &[&PoolTask], k: usize) -> Vec<String> {
    let mut by_label: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in tasks {
        if t.labels.is_empty() {
            by_label
                .entry(String::new())
                .or_default()
                .push(t.id.clone());
        }
        for l in &t.labels {
            by_label.entry(l.clone()).or_default().push(t.id.clone());
        }
    }
    let mut queues: Vec<Vec<String>> = by_label
        .into_values()
        .map(|mut v| {
            v.sort();
            rng.shuffle(&mut v);
            v.reverse(); // pop() from the end takes the shuffled order front to back
            v
        })
        .collect();
    let mut chosen: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    while chosen.len() < k && queues.iter().any(|q| !q.is_empty()) {
        for q in queues.iter_mut() {
            if chosen.len() >= k {
                break;
            }
            while let Some(id) = q.pop() {
                if seen.insert(id.clone()) {
                    chosen.push(id);
                    break;
                }
            }
        }
    }
    chosen
}

/// The pool sample of §3.7.2: K slots, owner/maintainer/imported tasks first (never pushed below
/// `min_pool` by agent tasks), at most min(`agent_stratum_max`, ⌊K/2⌋) slots for visible instances of
/// **eligible** agent families, then up to `max_repro_tasks` extra frozen tasks named by `Repro: task:`.
/// Seed = `sha256(gate_id + "/select")`. Ineligible agent tasks are skipped, never counted.
pub fn select_pool(
    gate_id: &str,
    pool: &[PoolTask],
    eligible_families: &BTreeSet<String>,
    repro_ids: &[String],
    p: SelectParams,
) -> Vec<String> {
    let mut rng = SplitMix64::for_gate(gate_id, "select");
    let owner: Vec<&PoolTask> = pool.iter().filter(|t| t.source != Source::Agent).collect();
    let agent: Vec<&PoolTask> = pool
        .iter()
        .filter(|t| {
            t.source == Source::Agent
                && t.family
                    .as_ref()
                    .is_some_and(|f| eligible_families.contains(f))
        })
        .collect();
    let cap = p.agent_stratum_max.min(p.pool_k / 2);
    let owner_quota = owner
        .len()
        .min(p.pool_k.saturating_sub(cap).max(p.min_pool))
        .min(p.pool_k);
    let agent_n = agent.len().min(cap).min(p.pool_k - owner_quota);
    let owner_n = owner.len().min(p.pool_k - agent_n);
    let mut out = stratified(&mut rng, &owner, owner_n);
    out.extend(stratified(&mut rng, &agent, agent_n));
    let frozen: BTreeSet<&str> = pool
        .iter()
        .filter(|t| t.source != Source::Agent)
        .map(|t| t.id.as_str())
        .collect();
    let mut extra = 0;
    for r in repro_ids {
        if extra >= p.max_repro_tasks {
            break;
        }
        if frozen.contains(r.as_str()) && !out.contains(r) {
            out.push(r.clone());
            extra += 1;
        }
    }
    out
}

/// One admitted family as the capability set sees it: its labels and its **sealed** instance ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SealedFamily {
    pub family: String,
    pub labels: Vec<String>,
    pub sealed: Vec<String>,
}

/// The capability set A (§3.7.2, §3.9.1 step 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilitySet {
    pub families: Vec<String>,
    pub instances: Vec<String>,
    /// `false` when A has fewer than `capability_min_tasks` instances or a single family: the record says
    /// `capability.status = "insufficient"`.
    pub sufficient: bool,
}

/// A = the sealed instances of every eligible family carrying a claimed label, whole families only, up
/// to `max_tasks` (a seeded sample over families when there are more; seed purpose `"cap-select"`).
pub fn select_capability(
    gate_id: &str,
    families: &[SealedFamily],
    eligible_families: &BTreeSet<String>,
    claimed: &[String],
    min_tasks: usize,
    max_tasks: usize,
) -> CapabilitySet {
    let mut cand: Vec<&SealedFamily> = families
        .iter()
        .filter(|f| {
            eligible_families.contains(&f.family)
                && f.labels.iter().any(|l| claimed.contains(l))
                && !f.sealed.is_empty()
        })
        .collect();
    cand.sort_by(|a, b| a.family.cmp(&b.family));
    let total: usize = cand.iter().map(|f| f.sealed.len()).sum();
    if total > max_tasks {
        let mut rng = SplitMix64::for_gate(gate_id, "cap-select");
        rng.shuffle(&mut cand);
    }
    let (mut fams, mut inst) = (Vec::new(), Vec::new());
    for f in cand {
        if inst.len() + f.sealed.len() <= max_tasks {
            fams.push(f.family.clone());
            inst.extend(f.sealed.iter().cloned());
        }
    }
    let sufficient = inst.len() >= min_tasks && fams.len() >= 2;
    CapabilitySet {
        families: fams,
        instances: inst,
        sufficient,
    }
}

/// Holdout sample (§3.7.2): all tasks if ≤ H; else a window of H over the sorted ids, rotated by the
/// gate counter so every holdout task is used at least once per ⌈|holdout| / H⌉ gates.
pub fn select_holdout(ids: &[String], h: usize, gate_counter: u64) -> Vec<String> {
    let mut v: Vec<String> = ids.to_vec();
    v.sort();
    if v.len() <= h || h == 0 {
        return if h == 0 { vec![] } else { v };
    }
    let rounds = v.len().div_ceil(h) as u64;
    let start = ((gate_counter % rounds) as usize) * h;
    (0..h).map(|i| v[(start + i) % v.len()].clone()).collect()
}

/// Canary sample: `n` ids in a seeded order (seed purpose `"canaries"`).
pub fn select_canaries(gate_id: &str, ids: &[String], n: usize) -> Vec<String> {
    let mut v: Vec<String> = ids.to_vec();
    v.sort();
    SplitMix64::for_gate(gate_id, "canaries").shuffle(&mut v);
    v.truncate(n);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn promo(seq: u64, sha: &str) -> R4Record {
        R4Record::Promotion {
            seq,
            sha: sha.into(),
            lkg: true,
        }
    }

    /// Lineage: family F registered at seq 10; LKG promotions p1..p7 at seq 20..80.
    fn lineage() -> Vec<R4Record> {
        let mut r = vec![
            R4Record::Audit { seq: 5, pass: true },
            R4Record::TaskAdmit {
                seq: 10,
                family: "fam".into(),
            },
        ];
        for i in 1..=7u64 {
            r.push(promo(10 + 10 * i, &format!("p{i}")));
        }
        r
    }

    /// All promotions up to `upto` are ancestors of C.
    fn anc(upto: u32) -> impl Fn(&str) -> bool {
        move |sha: &str| sha[1..].parse::<u32>().is_ok_and(|i| i <= upto)
    }

    /// G-41: registered after the LKG's promotion record; distance 5; distance 6; `task_flaky`; a
    /// `ladder{down}` past the registration.
    #[test]
    fn g41_eligibility() {
        let mut rec = lineage();
        rec.push(R4Record::TaskAdmit {
            seq: 85,
            family: "late".into(),
        });
        // Late: registered (85) after the LKG p7's promotion (80).
        let late = eligibility(&rec, "late", "p7", &anc(7), K_PROMOTIONS);
        assert_eq!(late.excluded, Some(Exclusion::LateRegistration));
        // Distance 5: four LKG promotions after registration are ancestors of C → d = 5 → excluded.
        let d5 = eligibility(&rec, "fam", "p4", &anc(4), K_PROMOTIONS);
        assert_eq!(
            (d5.distance, d5.excluded),
            (5, Some(Exclusion::KGenerations))
        );
        // Distance 6 → eligible.
        let d6 = eligibility(&rec, "fam", "p5", &anc(5), K_PROMOTIONS);
        assert_eq!((d6.distance, d6.excluded), (6, None));
        assert!(d6.eligible());
        // task_flaky → suspended, until the next passing sealed audit.
        let mut flaky = rec.clone();
        flaky.push(R4Record::TaskSuspended {
            seq: 90,
            family: "fam".into(),
        });
        assert_eq!(
            eligibility(&flaky, "fam", "p7", &anc(7), K_PROMOTIONS).excluded,
            Some(Exclusion::Suspended)
        );
        flaky.push(R4Record::Audit {
            seq: 95,
            pass: true,
        });
        assert!(eligibility(&flaky, "fam", "p7", &anc(7), K_PROMOTIONS).eligible());
        // ladder{down} with audited-good (5) before the registration (10) → suspended, and the trigger
        // names the family for a `task_suspended` record.
        let down = R4Record::LadderDown { seq: 100 };
        assert_eq!(
            families_to_suspend(&rec, &down, &["fam".into(), "late".into()]),
            vec!["fam".to_string(), "late".to_string()]
        );
        let mut demoted = rec.clone();
        demoted.push(down);
        assert_eq!(
            eligibility(&demoted, "fam", "p7", &anc(7), K_PROMOTIONS).excluded,
            Some(Exclusion::Suspended)
        );
        // An audit that passed after the registration protects the family from a later rollback.
        let mut audited = rec.clone();
        audited.push(R4Record::Audit {
            seq: 60,
            pass: true,
        });
        let rb = R4Record::Rollback { seq: 100 };
        assert!(families_to_suspend(&audited, &rb, &["fam".into()]).is_empty());
        audited.push(rb);
        assert!(eligibility(&audited, "fam", "p7", &anc(7), K_PROMOTIONS).eligible());
        // The record's counts.
        let counts = excluded_counts(&[late, d5, d6]);
        assert_eq!(counts.get(&Exclusion::LateRegistration), Some(&1));
        assert_eq!(counts.get(&Exclusion::KGenerations), Some(&1));
        // Unknown family and an LKG without a promotion record (the seed): late.
        assert_eq!(
            eligibility(&rec, "nope", "p7", &anc(7), 5).excluded,
            Some(Exclusion::LateRegistration)
        );
        assert_eq!(
            eligibility(&rec, "fam", "seed", &anc(7), 5).excluded,
            Some(Exclusion::LateRegistration)
        );
    }

    /// Non-ancestor promotions (another branch of history) do not count toward the distance.
    #[test]
    fn distance_counts_only_ancestor_promotions() {
        let mut rec = lineage();
        rec.push(R4Record::Promotion {
            seq: 81,
            sha: "side".into(),
            lkg: true,
        });
        rec.push(R4Record::Promotion {
            seq: 82,
            sha: "p8".into(),
            lkg: false,
        });
        let e = eligibility(&rec, "fam", "p7", &anc(7), K_PROMOTIONS);
        assert_eq!(e.distance, 8);
    }

    /// G-48: 20 eligible agent tasks and 4 owner tasks, K = 10, `agent_stratum_max` = 5.
    #[test]
    fn g48_agent_stratum() {
        let mut pool: Vec<PoolTask> = (0..4)
            .map(|i| PoolTask {
                id: format!("owner-{i}"),
                labels: vec!["edit".into()],
                source: Source::Owner,
                family: None,
            })
            .collect();
        for f in 0..10 {
            for i in 0..2 {
                pool.push(PoolTask {
                    id: format!("fam{f}.v{i}"),
                    labels: vec![format!("l{}", f % 3)],
                    source: Source::Agent,
                    family: Some(format!("fam{f}")),
                });
            }
        }
        let eligible: BTreeSet<String> = (0..10).map(|f| format!("fam{f}")).collect();
        let p = SelectParams {
            pool_k: 10,
            min_pool: 8,
            agent_stratum_max: 5,
            max_repro_tasks: 5,
        };
        let s = select_pool("j-000048", &pool, &eligible, &[], p);
        let agents = s.iter().filter(|id| id.starts_with("fam")).count();
        assert!(agents <= 5, "{s:?}");
        for i in 0..4 {
            assert!(s.contains(&format!("owner-{i}")));
        }
        assert_eq!(
            s,
            select_pool("j-000048", &pool, &eligible, &[], p),
            "seeded: same gate, same sample"
        );
        // Ineligible families are skipped, not counted.
        let none = select_pool("j-000048", &pool, &BTreeSet::new(), &[], p);
        assert_eq!(none.len(), 4);
    }

    #[test]
    fn pool_quota_keeps_min_pool_and_adds_repro_tasks() {
        let mut pool: Vec<PoolTask> = (0..20)
            .map(|i| PoolTask {
                id: format!("m-{i:02}"),
                labels: vec![if i % 2 == 0 { "a".into() } else { "b".into() }],
                source: Source::Maintainer,
                family: None,
            })
            .collect();
        for i in 0..10 {
            pool.push(PoolTask {
                id: format!("f.v{i}"),
                labels: vec![],
                source: Source::Agent,
                family: Some("f".into()),
            });
        }
        let elig: BTreeSet<String> = ["f".to_string()].into();
        let s = select_pool(
            "j-1",
            &pool,
            &elig,
            &["m-00".into(), "m-01".into(), "nope".into()],
            SelectParams::default(),
        );
        let base = &s[..12];
        assert_eq!(
            base.iter().filter(|x| x.starts_with("m-")).count(),
            8,
            "{s:?}"
        );
        assert_eq!(base.iter().filter(|x| x.starts_with("f.")).count(), 4);
        // Stratified: both labels appear among the maintainer picks.
        let a = base
            .iter()
            .filter(|x| x.starts_with("m-") && x[2..].parse::<u32>().unwrap() % 2 == 0)
            .count();
        assert_eq!(a, 4);
        // Repro tasks not already chosen are appended; unknown ids are ignored.
        for r in ["m-00", "m-01"] {
            assert_eq!(s.iter().filter(|x| *x == r).count(), 1);
        }
        assert!(!s.contains(&"nope".to_string()));
        // A small pool is used whole.
        let small: Vec<PoolTask> = pool[..5].to_vec();
        assert_eq!(
            select_pool("j-1", &small, &elig, &[], SelectParams::default()).len(),
            5
        );
    }

    #[test]
    fn capability_set_is_sealed_eligible_claimed_whole_families() {
        let fam = |name: &str, label: &str, n: usize| SealedFamily {
            family: name.into(),
            labels: vec![label.into()],
            sealed: (0..n).map(|i| format!("{name}.s{i}")).collect(),
        };
        let fams = vec![
            fam("a", "interactive", 2),
            fam("b", "interactive", 2),
            fam("c", "other", 2),
            fam("d", "interactive", 2),
        ];
        let elig: BTreeSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let a = select_capability("j-42", &fams, &elig, &["interactive".into()], 4, 8);
        assert_eq!(a.families, vec!["a", "b"]);
        assert_eq!(a.instances.len(), 4);
        assert!(a.sufficient);
        // Only 3 instances (G-43).
        let fams3 = vec![fam("a", "interactive", 2), fam("b", "interactive", 1)];
        assert!(
            !select_capability("j-43", &fams3, &elig, &["interactive".into()], 4, 8).sufficient
        );
        // More than max: a seeded sample of whole families, never more than 8 instances.
        let many: Vec<SealedFamily> = (0..6)
            .map(|i| fam(&format!("m{i}"), "interactive", 3))
            .collect();
        let e: BTreeSet<String> = (0..6).map(|i| format!("m{i}")).collect();
        let s = select_capability("j-44", &many, &e, &["interactive".into()], 4, 8);
        assert_eq!(s.instances.len(), 6);
        assert_eq!(
            s,
            select_capability("j-44", &many, &e, &["interactive".into()], 4, 8)
        );
        // No claim, no set.
        assert!(
            select_capability("j-45", &fams, &elig, &[], 4, 8)
                .instances
                .is_empty()
        );
    }

    #[test]
    fn sealing_halves_and_commitment() {
        let inst: Vec<String> = (0..5).map(|i| format!("fam.v{i}")).collect();
        let (sealed, visible) = seal(&inst, [7u8; 32]);
        assert_eq!((sealed.len(), visible.len()), (3, 2));
        let all: BTreeSet<&String> = sealed.iter().chain(visible.iter()).collect();
        assert_eq!(all.len(), 5);
        let (s2, _) = seal(&inst, [9u8; 32]);
        let salt = [1u8; 16];
        let c = sealed_commitment(&salt, &sealed);
        assert!(c.starts_with("sha256:") && c.len() == 71);
        let mut rev = sealed.clone();
        rev.reverse();
        assert_eq!(c, sealed_commitment(&salt, &rev), "order-independent");
        assert_ne!(c, sealed_commitment(&[2u8; 16], &sealed), "salted");
        let _ = s2;
        let (s4, v4) = seal(&inst[..4], [3u8; 32]);
        assert_eq!((s4.len(), v4.len()), (2, 2));
    }

    #[test]
    fn family_json_validation() {
        let ok = FamilyJson {
            v: 1,
            id: "pty-repl".into(),
            labels: vec!["interactive".into()],
            instances: vec!["a".into(), "b".into()],
        };
        assert!(ok.validate().is_ok());
        assert_eq!(ok.instance_ids(), vec!["pty-repl.a", "pty-repl.b"]);
        let one = FamilyJson {
            instances: vec!["a".into()],
            ..ok.clone()
        };
        assert!(one.validate().is_err());
        let nine = FamilyJson {
            instances: (0..9).map(|i| format!("i{i}")).collect(),
            ..ok.clone()
        };
        assert!(nine.validate().is_err());
        let bad = FamilyJson {
            v: 2,
            id: "Bad".into(),
            labels: vec!["UP".into()],
            instances: vec!["a".into(), "a".into()],
        };
        assert_eq!(bad.validate().unwrap_err().len(), 4);
        let parsed: FamilyJson =
            serde_json::from_str(r#"{"v":1,"id":"f","labels":[],"instances":["a","b"]}"#).unwrap();
        assert!(parsed.validate().is_ok());
        assert!(
            serde_json::from_str::<FamilyJson>(
                r#"{"v":1,"id":"f","labels":[],"instances":["a","b"],"x":1}"#
            )
            .is_err()
        );
    }

    #[test]
    fn holdout_rotation_and_canaries() {
        let ids: Vec<String> = (0..10).map(|i| format!("h{i}")).collect();
        let mut seen = BTreeSet::new();
        for g in 0..3 {
            let s = select_holdout(&ids, 4, g);
            assert_eq!(s.len(), 4);
            seen.extend(s);
        }
        assert_eq!(seen.len(), 10, "every holdout task within ⌈10/4⌉ = 3 gates");
        assert_eq!(select_holdout(&ids[..3], 4, 7).len(), 3);
        let c = select_canaries("j-1", &ids, 2);
        assert_eq!(c.len(), 2);
        assert_eq!(c, select_canaries("j-1", &ids, 2));
    }
}
