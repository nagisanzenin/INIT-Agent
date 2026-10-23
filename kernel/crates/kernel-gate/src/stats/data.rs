//! G4 run data as the statistics see it (kernel/gate.md §3.8.1, §4.4 RunRecord).
//!
//! The G4 driver (later, once supervise's `probe()` exists) fills these from RunRecords. Canaries and the
//! capability set A are kept apart from the pool ∪ holdout tasks, as §3.8.1 requires.

use serde::{Deserialize, Serialize};

/// The two arms of a paired G4: `c` (candidate) and `p` (LKG).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arm {
    C,
    P,
}

/// The split a judged task belongs to. Canaries and the capability set never enter [`TaskRuns`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskSet {
    Pool,
    Holdout,
}

/// `task.json` `source.kind` (gate.md §3.7.1). `Agent` marks the agent-authored stratum (visible family
/// instances only, D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Session,
    Owner,
    Maintainer,
    Agent,
}

/// The run values the statistics use, in the RunRecord's integer units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RunValues {
    pub score_permille: u32,
    pub cost_ref_musd: u64,
    pub wall_ms: u64,
    pub turns: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub gap_ms_p50: u64,
    pub cache_read: u64,
    pub input_total: u64,
}

impl RunValues {
    /// A run with only a score; every other metric is 0.
    pub fn score(score_permille: u32) -> Self {
        RunValues {
            score_permille,
            ..Default::default()
        }
    }
}

/// One judged task with the repeats of both arms (`c[r]`, `p[r]`, r = 0..R).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRuns {
    pub id: String,
    pub set: TaskSet,
    pub source: Source,
    /// The agent family of an agent-authored instance.
    pub family: Option<String>,
    pub c: Vec<RunValues>,
    pub p: Vec<RunValues>,
}

impl TaskRuns {
    pub fn arm(&self, a: Arm) -> &[RunValues] {
        match a {
            Arm::C => &self.c,
            Arm::P => &self.p,
        }
    }

    /// `S[a][i]`: the mean score over the arm's repeats, in [0, 1].
    pub fn mean_score(&self, a: Arm) -> f64 {
        let runs = self.arm(a);
        if runs.is_empty() {
            return 0.0;
        }
        runs.iter()
            .map(|r| r.score_permille as f64 / 1000.0)
            .sum::<f64>()
            / runs.len() as f64
    }

    /// `d[i] = S[c][i] − S[p][i]`.
    pub fn d(&self) -> f64 {
        self.mean_score(Arm::C) - self.mean_score(Arm::P)
    }

    pub fn is_agent(&self) -> bool {
        self.source == Source::Agent
    }
}

/// Metrics of gate.md §3.8.1 (names match evolution.md §3.10.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    PassRate,
    CostUsd,
    WallS,
    Turns,
    TokensIn,
    TokensOut,
    GapMsP50,
    CacheHit,
    CostXWall,
}

impl Metric {
    pub const ALL: [Metric; 9] = [
        Metric::PassRate,
        Metric::CostUsd,
        Metric::WallS,
        Metric::Turns,
        Metric::TokensIn,
        Metric::TokensOut,
        Metric::GapMsP50,
        Metric::CacheHit,
        Metric::CostXWall,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Metric::PassRate => "pass_rate",
            Metric::CostUsd => "cost_usd",
            Metric::WallS => "wall_s",
            Metric::Turns => "turns",
            Metric::TokensIn => "tokens_in",
            Metric::TokensOut => "tokens_out",
            Metric::GapMsP50 => "gap_ms_p50",
            Metric::CacheHit => "cache_hit",
            Metric::CostXWall => "cost_x_wall",
        }
    }

    /// Log-scale metrics compare as `ln T(c) − ln T(p)`; linear ones as `T(c) − T(p)`.
    pub fn is_log(self) -> bool {
        !matches!(self, Metric::PassRate | Metric::CacheHit)
    }

    fn value(self, r: &RunValues) -> f64 {
        match self {
            Metric::PassRate => r.score_permille as f64 / 1000.0,
            Metric::CostUsd => r.cost_ref_musd as f64,
            Metric::WallS => r.wall_ms as f64,
            Metric::Turns => r.turns as f64,
            Metric::TokensIn => r.tokens_in as f64,
            Metric::TokensOut => r.tokens_out as f64,
            Metric::GapMsP50 => r.gap_ms_p50 as f64,
            Metric::CacheHit | Metric::CostXWall => unreachable!("composite metric"),
        }
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// `ln` of a natural-unit value, floored at 1 unit (1 µUSD, 1 ms, 1 token) so that a $0 mock arm never
/// produces −∞. Real values are far above the floor, so it never moves a real comparison.
pub fn ln_floor(x: f64) -> f64 {
    x.max(1.0).ln()
}

/// The arm statistic `T_m(a)` of §3.8.1 over `tasks`, using for each task only the repeats `pick(i)`
/// (all repeats in a normal computation; half-groups in the noise split).
pub fn arm_stat_with<'p>(
    m: Metric,
    tasks: &[&TaskRuns],
    a: Arm,
    pick: &dyn Fn(usize) -> &'p [usize],
) -> f64 {
    let runs = |i: usize| -> Vec<&RunValues> {
        let all = tasks[i].arm(a);
        pick(i).iter().filter_map(|&r| all.get(r)).collect()
    };
    let per_task_mean = |m: Metric, i: usize| -> Option<f64> {
        let rs = runs(i);
        if rs.is_empty() {
            None
        } else {
            Some(rs.iter().map(|r| m.value(r)).sum::<f64>() / rs.len() as f64)
        }
    };
    match m {
        Metric::PassRate => {
            let v: Vec<f64> = (0..tasks.len())
                .filter_map(|i| per_task_mean(m, i))
                .collect();
            if v.is_empty() {
                0.0
            } else {
                v.iter().sum::<f64>() / v.len() as f64
            }
        }
        Metric::CacheHit => {
            let (mut read, mut total) = (0u128, 0u128);
            for i in 0..tasks.len() {
                for r in runs(i) {
                    read += r.cache_read as u128;
                    total += r.input_total as u128;
                }
            }
            if total == 0 {
                0.0
            } else {
                read as f64 / total as f64
            }
        }
        Metric::CostXWall => {
            arm_stat_with(Metric::CostUsd, tasks, a, pick)
                * arm_stat_with(Metric::WallS, tasks, a, pick)
        }
        _ => median(
            (0..tasks.len())
                .filter_map(|i| per_task_mean(m, i))
                .collect(),
        ),
    }
}

/// `T_m(a)` over all repeats.
pub fn arm_stat(m: Metric, tasks: &[&TaskRuns], a: Arm) -> f64 {
    let all: Vec<Vec<usize>> = tasks
        .iter()
        .map(|t| (0..t.arm(a).len()).collect())
        .collect();
    arm_stat_with(m, tasks, a, &|i| all[i].as_slice())
}

/// The value on the metric's comparison scale: `ln` (floored) for log metrics, identity otherwise.
/// For `cost_x_wall` this is `ln T_cost + ln T_wall`.
pub fn scaled<'p>(
    m: Metric,
    tasks: &[&TaskRuns],
    a: Arm,
    pick: &dyn Fn(usize) -> &'p [usize],
) -> f64 {
    match m {
        Metric::CostXWall => {
            ln_floor(arm_stat_with(Metric::CostUsd, tasks, a, pick))
                + ln_floor(arm_stat_with(Metric::WallS, tasks, a, pick))
        }
        m if m.is_log() => ln_floor(arm_stat_with(m, tasks, a, pick)),
        m => arm_stat_with(m, tasks, a, pick),
    }
}

/// The effect `Δ_m`: `ln T_m(c) − ln T_m(p)` for log metrics, `T_m(c) − T_m(p)` for linear ones.
pub fn effect(m: Metric, tasks: &[&TaskRuns]) -> f64 {
    let all_c: Vec<Vec<usize>> = tasks.iter().map(|t| (0..t.c.len()).collect()).collect();
    let all_p: Vec<Vec<usize>> = tasks.iter().map(|t| (0..t.p.len()).collect()).collect();
    scaled(m, tasks, Arm::C, &|i| all_c[i].as_slice())
        - scaled(m, tasks, Arm::P, &|i| all_p[i].as_slice())
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// A task whose arms have the given scores per repeat (permille) and constant metrics.
    pub fn task(id: &str, set: TaskSet, source: Source, c: &[u32], p: &[u32]) -> TaskRuns {
        let mk = |s: &[u32], cost: u64| -> Vec<RunValues> {
            s.iter()
                .map(|&x| RunValues {
                    score_permille: x,
                    cost_ref_musd: cost,
                    wall_ms: 100_000,
                    turns: 10,
                    tokens_in: 1000,
                    tokens_out: 100,
                    gap_ms_p50: 2000,
                    cache_read: 900,
                    input_total: 1000,
                })
                .collect()
        };
        TaskRuns {
            id: id.to_string(),
            set,
            source,
            family: None,
            c: mk(c, 20_000),
            p: mk(p, 20_000),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::task;
    use super::*;

    #[test]
    fn arm_statistics_match_definitions() {
        let mut a = task("a", TaskSet::Pool, Source::Owner, &[1000, 0], &[1000, 1000]);
        let mut b = task("b", TaskSet::Pool, Source::Owner, &[1000, 1000], &[0, 0]);
        a.c[0].cost_ref_musd = 10;
        a.c[1].cost_ref_musd = 30; // mean 20
        b.c[0].cost_ref_musd = 40;
        b.c[1].cost_ref_musd = 40; // mean 40
        let ts = [&a, &b];
        assert!((arm_stat(Metric::PassRate, &ts, Arm::C) - 0.75).abs() < 1e-12);
        assert!((arm_stat(Metric::PassRate, &ts, Arm::P) - 0.5).abs() < 1e-12);
        assert!((arm_stat(Metric::CostUsd, &ts, Arm::C) - 30.0).abs() < 1e-12); // median of {20, 40}
        assert!((arm_stat(Metric::CacheHit, &ts, Arm::C) - 0.9).abs() < 1e-12);
        assert!((a.d() - -0.5).abs() < 1e-12);
        assert!((effect(Metric::PassRate, &ts) - 0.25).abs() < 1e-12);
        let e = effect(Metric::CostUsd, &ts);
        assert!((e - (30f64.ln() - 20_000f64.ln())).abs() < 1e-12);
    }

    #[test]
    fn zero_cost_arms_have_zero_log_effect() {
        let mut a = task("a", TaskSet::Pool, Source::Owner, &[1000], &[1000]);
        a.c[0].cost_ref_musd = 0;
        a.p[0].cost_ref_musd = 0;
        assert_eq!(effect(Metric::CostUsd, &[&a]), 0.0);
        assert!(effect(Metric::CostXWall, &[&a]).abs() < 1e-12);
    }
}
