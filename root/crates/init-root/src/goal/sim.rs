//! Lane M (#2 per reading, #3) and L3 (v0.5.3): null simulations of GBAL's card readings and of `met`, through the
//! Root's own code: `reading`, `fails` (guardrails and gates), `measure` and `complete` (the pending readings, their
//! records and outcomes) and `met_check` (what `stop` verifies). No model, no files, no clock: synthetic owner turns
//! from a seeded generator, so every number is reproducible bit for bit. "Null" means the change does nothing: the
//! owner turns before and after it come from one distribution, so every `better`/`worse` verdict, every guardrail
//! rollback and every verified `met` is false.
//!
//! The turn model is RF-11's (sentinel/tests.rs, ADR-010's simulation, X10's clustering): sessions of geometric length
//! (mean `l`) share a log offset S ~ N(0, σ_s²), and per turn cost = 3000·e^(S + σ_t·Z) µUSD and wall = 40,000·e^(S +
//! σ_t·Z′) ms with σ_t = √(1 − σ_s²) (log sd 1 overall: INIT's lognormal cost model, ADR-011 §3.3), requests = (1 +
//! U{0..7})·e^S, provider errors Bin(requests, 2%), a correction with probability 10%. Cache reads (‰ of input tokens)
//! are 0 on 15% of turns (a cold cache) and U{500..1000} otherwise: a stand-in, since no real distribution is measured.
//!
//! The quick checks run in the unit suite. The full tables are `#[ignore]`d; the documented command (debug build, the
//! repo's target dir) is `cargo test --offline -p init-root --bin init-root goal::sim -- --ignored --nocapture
//! --test-threads 1`, with `INIT_STATS_OUT=<dir>` to write `<dir>/gbal-null.json` and `<dir>/gbal-power.json`
//! (build/status/STATS-053.md).

use super::*;

/// SplitMix64 (Steele, Lea, Flood 2014): a seeded stream for synthetic data only (every decision under test is the
/// Root's integer code).
pub(crate) struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    /// Box–Muller.
    fn normal(&mut self) -> f64 {
        (-2.0 * self.unit().ln()).sqrt() * (std::f64::consts::TAU * self.unit()).cos()
    }
    fn bern(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

/// RF-11's turn model: mean session length `l` (1: every turn its own session) and the shared log offset's sd.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Model {
    pub name: &'static str,
    pub l: f64,
    pub sigma_s: f64,
}

pub(crate) const IID: Model = Model { name: "iid", l: 1.0, sigma_s: 0.0 };
/// X10's heavy clustering (RF-11 `rf11_clustered_sessions_keep_the_null_bound`): sessions of 4 turns, σ_s = 0.6.
pub(crate) const CLUSTERED: Model = Model { name: "clustered_l4_s0.6", l: 4.0, sigma_s: 0.6 };

/// `n` consecutive owner turns on `tuple`, ending at `t0 + 60 s × (i + 1)`; `s` carries the session offset across
/// calls, so a change can go live in the middle of a session (the realistic case).
fn turns(r: &mut Rng, m: Model, s: &mut f64, n: usize, tuple: &str, t0: i64, seq0: u64) -> Vec<Obs> {
    let sigma_t = (1.0 - m.sigma_s * m.sigma_s).sqrt();
    (0..n).map(|i| {
        if r.bern(1.0 / m.l) {
            *s = m.sigma_s * r.normal();
        }
        let s = *s;
        let mut e = |scale: f64| (scale * (s + sigma_t * r.normal()).exp()).round().max(1.0) as u64;
        let (cost, wall) = (e(3000.0), e(40_000.0));
        let req = ((1 + r.next() % 8) as f64 * s.exp()).round().max(1.0) as u32;
        let perr = (0..req).filter(|_| r.bern(0.02)).count() as u32;
        let cache = if r.bern(0.15) { 0 } else { 500 + r.next() % 501 };
        Obs { seq: seq0 + i as u64, end: t0 + 60_000 * (i as i64 + 1), tuple: tuple.into(), cost, wall, req, perr, corr: r.bern(0.10), input: 1000, cache }
    }).collect()
}

/// Owner turns into the Root's card state as `turn_closed` keeps it (the newest `OBS_MAX`); `t` moves to the last end.
fn push_obs(st: &mut State, o: Vec<Obs>, t: &mut i64) {
    *t = o.last().map_or(*t, |x| x.end);
    st.obs.extend(o);
    let over = st.obs.len().saturating_sub(OBS_MAX);
    st.obs.drain(..over);
}

fn vals(c: &Card, o: &[Obs]) -> Vec<i64> {
    o.iter().filter_map(c.value).collect()
}

// ------------------------------------------------------------------------------------------------ the t candidate

/// ln Γ(x), Lanczos (g = 7, 9 terms; Numerical Recipes 3rd ed. §6.1): the Welch–t candidate only (floats are fine
/// here: it is a measured alternative, not the Root's code).
fn ln_gamma(x: f64) -> f64 {
    const G: [f64; 9] = [0.999_999_999_999_809_9, 676.520_368_121_885_1, -1_259.139_216_722_402_8, 771.323_428_777_653_1,
        -176.615_029_162_140_6, 12.507_343_278_686_905, -0.138_571_095_265_720_12, 9.984_369_578_019_572e-6, 1.505_632_735_149_311_6e-7];
    if x < 0.5 {
        return (std::f64::consts::PI / (std::f64::consts::PI * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = G[0];
    let t = x + 7.5;
    for (i, g) in G.iter().enumerate().skip(1) {
        a += g / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// The regularized incomplete beta I_x(a, b) by Lentz's continued fraction (Numerical Recipes §6.4).
fn inc_beta(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let front = (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x > (a + 1.0) / (a + b + 2.0) {
        return 1.0 - inc_beta(1.0 - x, b, a);
    }
    let (tiny, mut c, mut d) = (1e-300, 1.0, 1.0 - (a + b) * x / (a + 1.0));
    d = if d.abs() < tiny { tiny } else { d };
    d = 1.0 / d;
    let mut f = d;
    for m in 1..300 {
        let m = m as f64;
        for num in [m * (b - m) * x / ((a + 2.0 * m - 1.0) * (a + 2.0 * m)), -(a + m) * (a + b + m) * x / ((a + 2.0 * m) * (a + 2.0 * m + 1.0))] {
            d = 1.0 + num * d;
            d = if d.abs() < tiny { tiny } else { d };
            c = 1.0 + num / c;
            c = if c.abs() < tiny { tiny } else { c };
            d = 1.0 / d;
            f *= d * c;
        }
        if (d * c - 1.0).abs() < 1e-14 {
            break;
        }
    }
    front * f / a
}

/// P(T_ν ≥ t) for Student's t.
fn t_upper(t: f64, nu: f64) -> f64 {
    let tail = 0.5 * inc_beta(nu / (nu + t * t), nu / 2.0, 0.5);
    if t >= 0.0 { tail } else { 1.0 - tail }
}

/// The Welch–Satterthwaite candidate for a reading (v0.5.1's planned fix): one-sided p-values (better, worse) of the
/// difference in means against t at Welch's df. With se = 0 the sign decides, as the Root's z does (`reading`).
fn welch_t(lower_better: bool, before: &[i64], after: &[i64]) -> (f64, f64) {
    let m = |x: &[i64]| x.iter().map(|&v| v as f64).sum::<f64>() / x.len() as f64;
    let v = |x: &[i64], mu: f64| x.iter().map(|&a| (a as f64 - mu).powi(2)).sum::<f64>() / (x.len() as f64 - 1.0);
    let (mb, ma) = (m(before), m(after));
    let (vb, va) = (v(before, mb), v(after, ma));
    let (qb, qa) = (vb / before.len() as f64, va / after.len() as f64);
    let better = if lower_better { mb - ma } else { ma - mb };
    let se = (qb + qa).sqrt();
    if se == 0.0 {
        return match better.partial_cmp(&0.0) {
            Some(std::cmp::Ordering::Greater) => (0.0, 1.0),
            Some(std::cmp::Ordering::Less) => (1.0, 0.0),
            _ => (1.0, 1.0),
        };
    }
    let df = (qb + qa).powi(2) / (qb * qb / (before.len() as f64 - 1.0) + qa * qa / (after.len() as f64 - 1.0));
    let t = better / se;
    (t_upper(t, df), t_upper(-t, df))
}

// ------------------------------------------------------------------------------------------------ provenance

fn cmd(bin: &str, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new(bin).args(args).current_dir(env!("CARGO_MANIFEST_DIR")).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// The label every measurement file carries (lane M): the commit (and whether the tree had uncommitted changes), the
/// machine, the seed, the iterations and "model: none (pure simulation)".
pub(crate) fn provenance(command: &str, seed: u64, iterations: Value) -> Value {
    json!({"commit": cmd("git", &["rev-parse", "HEAD"]), "dirty": cmd("git", &["status", "--porcelain", "--untracked-files=no"]).map(|s| !s.is_empty()),
        "machine": cmd("sysctl", &["-n", "hw.model"]).or_else(|| cmd("uname", &["-m"])), "model": "none (pure simulation)",
        "seed": format!("{seed:#018x}"), "iterations": iterations, "command": command})
}

/// Prints the table, and writes `$INIT_STATS_OUT/<name>.json` when that is set.
pub(crate) fn emit(name: &str, v: &Value) {
    let s = serde_json::to_string_pretty(v).unwrap();
    eprintln!("{name}: {s}");
    if let Ok(dir) = std::env::var("INIT_STATS_OUT") {
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.json")), s + "\n").unwrap();
    }
}

fn rate(k: u64, n: u64) -> Value {
    // the Monte Carlo standard error alongside, so a reader can tell a real excess from noise
    let p = k as f64 / n.max(1) as f64;
    json!({"k": k, "n": n, "rate": (p * 1e6).round() / 1e6, "se": ((p * (1.0 - p) / n.max(1) as f64).sqrt() * 1e6).round() / 1e6})
}

// ------------------------------------------------------------------------------------------------ per reading

/// One cell of the per-reading table: `n` null readings of card `c` (`nb` turns before, `na` after) under model `m`:
/// how often the Root says better or worse, rolls back an owner guardrail (tolerance 0, the strictest) or the agent's
/// gate; and the same for the Welch–t candidate at the Root's noise level.
fn per_reading(c: &'static Card, m: Model, nb: usize, na: usize, n: u64, seed: u64) -> Value {
    let mut r = Rng(seed);
    let (mut better, mut worse, mut guard, mut gate, mut tb, mut tw) = (0, 0, 0, 0, 0, 0);
    let guard_m = Measure { guard: Some(0), ..Measure::default() };
    let gate_m = Measure { gate: true, ..Measure::default() };
    for _ in 0..n {
        let mut s = m.sigma_s * r.normal();
        let o = turns(&mut r, m, &mut s, nb + na, "t", 0, 0);
        let (b, a) = (vals(c, &o[..nb]), vals(c, &o[nb..]));
        let rd = reading(c, &b, &a);
        better += u64::from(rd["verdict"] == "better");
        worse += u64::from(rd["verdict"] == "worse");
        guard += u64::from(fails(c, &guard_m, &rd));
        gate += u64::from(fails(c, &gate_m, &rd));
        let (pb, pw) = welch_t(c.lower_better, &b, &a);
        tb += u64::from(pb <= NOISE_ALPHA);
        tw += u64::from(pw <= NOISE_ALPHA);
    }
    json!({"card": c.id, "unit": c.unit, "model": m.name, "n_before": nb, "n_after": na, "false_better": rate(better, n), "false_worse": rate(worse, n),
        "guard_rollback_tol0": rate(guard, n), "gate_rollback": rate(gate, n), "welch_t": {"false_better": rate(tb, n), "false_worse": rate(tw, n)}})
}

/// The noise level of a verdict as ADR-013 decision 4 states it: the normal tail at z = 2, Φ(−2) = 0.0227501…
const NOISE_ALPHA: f64 = 0.022_750_131_948_179_2;

// ------------------------------------------------------------------------------------------------ per goal

/// The baseline-turn policy of a goal's changes: `stacked`, the next change goes live after the previous one's
/// reading (5 turns), so each reading's baseline is the parent's 5 turns (the first change's: 40 incumbent turns);
/// `wait40`, the agent waits 40 owner turns on each change, so every baseline is 40.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Mode {
    Stacked,
    Wait40,
}

/// What one simulated goal did: whether the Root verified `met`, at which round, and its goal-card verdicts.
pub(crate) struct GoalRun {
    pub met: Option<u64>,
    pub better: u64,
    pub worse: u64,
    pub readings: u64,
}

/// One null goal on card `c` with `rounds` rounds: each round the agent predicts its change moves the card toward the
/// threshold (`better`), the change goes live, owner turns arrive, the Root completes the reading, and the agent
/// stops `met` the first time the Root would verify it (`met_check`) — the false-`met`-seeking strategy. The
/// threshold is always reached (`<=` 10¹², or `>=` 0 for a higher-is-better card), the worst case for the Root:
/// attribution is then the only guard.
pub(crate) fn goal_run(c: &'static Card, m: Model, mode: Mode, rounds: u64, r: &mut Rng) -> GoalRun {
    let gid = "G-000001";
    let (op, value) = if c.lower_better { ("<=", 1_000_000_000_000i64) } else { (">=", 0) };
    let mut st = State::default();
    st.goals.push(Goal { id: gid.into(), kind: format!("card:{}", c.id), done: json!({"card": c.id, "kind": "card", "op": op, "value": value}),
        usd_musd: u64::MAX, wall_s: 1_000_000, rounds, state: "active".into(), ..Goal::default() });
    let (mut recs, mut seq, mut t, mut s) = (Vec::<Value>::new(), 0u64, 1_900_000_000_000i64, m.sigma_s * r.normal());
    let base = turns(r, m, &mut s, BASE_MAX, "t0", t, 0);
    push_obs(&mut st, base, &mut t);
    let after_n = match mode { Mode::Stacked => BASE_MIN, Mode::Wait40 => BASE_MAX };
    let mut run = GoalRun { met: None, better: 0, worse: 0, readings: 0 };
    for round in 1..=rounds {
        let (sha, tid, pid) = (format!("{round:040x}"), format!("t{round}"), format!("P-{round:06}"));
        seq += 1;
        st.preds.push(Pred { id: pid.clone(), goal: gid.into(), sha: sha.clone(), card: Some(c.id.into()), expect: "better".into(), p: 500, seq,
            kind: format!("card:{}", c.id), round, ..Pred::default() });
        measure(&mut st, c.id, &sha, None, false, None).unwrap();
        seq += 1;
        recs.push(json!({"kind": "tuple", "by": "root", "seq": seq, "at": util::rfc3339(t + 1), "data": {"id": tid, "tuple": {"body": {"sha": sha}}}}));
        let o = turns(r, m, &mut s, after_n, &tid, t + 1, seq * 1000);
        push_obs(&mut st, o, &mut t);
        let mut out = Vec::new();
        complete(&mut st, &recs, &mut out);
        for o in out {
            if let Out::Append(kind, data) = o {
                if kind == "card_reading" && data["card"] == c.id {
                    run.readings += 1;
                    run.better += u64::from(data["verdict"] == "better");
                    run.worse += u64::from(data["verdict"] == "worse");
                }
                seq += 1;
                recs.push(json!({"kind": kind, "by": "root", "seq": seq, "data": data}));
            }
        }
        if met_check(&st.goals[0], &recs, &st.preds).0 == true {
            run.met = Some(round);
            break;
        }
    }
    run
}

fn per_goal(c: &'static Card, m: Model, mode: Mode, rounds: u64, goals: u64, seed: u64) -> Value {
    let mut r = Rng(seed);
    let (mut met, mut better, mut worse, mut readings, mut at) = (0, 0, 0, 0, 0);
    for _ in 0..goals {
        let g = goal_run(c, m, mode, rounds, &mut r);
        met += u64::from(g.met.is_some());
        at += g.met.unwrap_or(0);
        (better, worse, readings) = (better + g.better, worse + g.worse, readings + g.readings);
    }
    json!({"card": c.id, "model": m.name, "mode": format!("{mode:?}").to_lowercase(), "rounds": rounds, "false_met_per_goal": rate(met, goals),
        "mean_round_of_false_met": if met > 0 { json!(at as f64 / met as f64) } else { Value::Null },
        "per_reading": {"false_better": rate(better, readings), "false_worse": rate(worse, readings)}})
}

// ------------------------------------------------------------------------------------------------ power

/// The harm a power cell applies to the turns after a change: twice the cost, wall time or requests, half the cache
/// reads, provider errors at 10 % of requests instead of 2 %, corrections on half the turns instead of a tenth.
fn harm(c: &Card, r: &mut Rng, o: &mut Obs) {
    match c.id {
        "cost_turn" => o.cost *= 2,
        "wall_turn" => o.wall *= 2,
        "requests_turn" => o.req *= 2,
        "cache_read" => o.cache /= 2,
        "provider_errors" => o.perr = (0..o.req).filter(|_| r.bern(0.10)).count() as u32,
        _ => o.corr = r.bern(0.5),
    }
}

/// How often an owner guardrail (tolerance 0) rolls back a change that does the card's `harm`: the other side of
/// the per-reading table, so a lower false-rollback rate is never bought silently with less protection.
fn per_reading_power(c: &'static Card, m: Model, nb: usize, na: usize, n: u64, seed: u64) -> Value {
    let mut r = Rng(seed);
    let guard_m = Measure { guard: Some(0), ..Measure::default() };
    let mut caught = 0;
    for _ in 0..n {
        let mut s = m.sigma_s * r.normal();
        let mut o = turns(&mut r, m, &mut s, nb + na, "t", 0, 0);
        for x in &mut o[nb..] {
            harm(c, &mut r, x);
        }
        caught += u64::from(fails(c, &guard_m, &reading(c, &vals(c, &o[..nb]), &vals(c, &o[nb..]))));
    }
    json!({"card": c.id, "model": m.name, "n_before": nb, "n_after": na, "harm_caught": rate(caught, n)})
}

// ------------------------------------------------------------------------------------------------ discrete-card guardrails

/// The discrete and count cards: their guardrails lose the most power to the exact level (STATS-053, "Power").
const DISCRETE: [&str; 4] = ["requests_turn", "cache_read", "provider_errors", "corrections"];

/// A k-fold rise of the harm a discrete card's guardrail watches, applied to one turn after the change: k× requests,
/// cache reads ÷ k, provider errors at k × 2 % of requests, corrections on k × 10 % of turns (k = 1: no change).
fn rise(c: &Card, k: u64, r: &mut Rng, o: &mut Obs) {
    if k == 1 {
        return;
    }
    match c.id {
        "requests_turn" => o.req *= k as u32,
        "cache_read" => o.cache /= k,
        "provider_errors" => o.perr = (0..o.req).filter(|_| r.bern(0.02 * k as f64)).count() as u32,
        _ => o.corr = r.bern(0.10 * k as f64),
    }
}

/// Monte Carlo relabelings of the studentized candidate: (1 + B) = 10,000, so its smallest p is 100 ppm, below the
/// guardrail's 22,750.
const T_DRAWS: u64 = 9_999;

/// The studentized permutation candidate (option c): the permutation test with Welch's t as its statistic, in the
/// harm direction. Any statistic gives an exact test under exchangeability (the turns' labels are uniform given the
/// pool); a studentized one keeps z's sensitivity where one arm has no spread (Janssen 1997; Chung & Romano 2013).
/// p = (1 + #{t* ≥ t})/(1 + B), valid at any B (Phipson & Smyth 2010). Returns p in ppm, rounded up.
fn studentized_worse_ppm(c: &Card, before: &[i64], after: &[i64], r: &mut Rng) -> u64 {
    let pool: Vec<f64> = before.iter().chain(after).map(|&v| v as f64).collect();
    let (n, na) = (pool.len(), after.len());
    let (tot, totq) = (pool.iter().sum::<f64>(), pool.iter().map(|v| v * v).sum::<f64>());
    // Welch's t of the after-arm with sum `sa` and sum of squares `qa`, signed so that positive is harm
    let t = |sa: f64, qa: f64| {
        let nb = (n - na) as f64;
        let (sb, qb, naf) = (tot - sa, totq - qa, na as f64);
        let (ma, mb) = (sa / naf, sb / nb);
        let (va, vb) = (((qa - naf * ma * ma) / (naf - 1.0)).max(0.0), ((qb - nb * mb * mb) / (nb - 1.0)).max(0.0));
        let d = if c.lower_better { ma - mb } else { mb - ma };
        let se = (va / naf + vb / nb).sqrt();
        if se > 1e-12 { d / se } else if d.abs() < 1e-12 { 0.0 } else { d.signum() * f64::INFINITY }
    };
    let obs = t(after.iter().map(|&v| v as f64).sum(), after.iter().map(|&v| (v * v) as f64).sum());
    let mut idx: Vec<usize> = (0..n).collect();
    let mut count = 1u64;
    for _ in 0..T_DRAWS {
        let (mut sa, mut qa) = (0.0, 0.0);
        for i in 0..na {
            let j = i + (r.next() % (n - i) as u64) as usize;
            idx.swap(i, j);
            let v = pool[idx[i]];
            (sa, qa) = (sa + v, qa + v * v);
        }
        count += u64::from(t(sa, qa) >= obs - 1e-9);
    }
    (u128::from(count) * 1_000_000).div_ceil(u128::from(T_DRAWS) + 1) as u64
}

/// One cell: `n` readings of card `c`, `nb` turns before and `na` after a change that raises its harm k-fold, under
/// the three guardrail rules (tolerance 0) on the same readings: L3's exact test (the Root's `reading` and `fails`);
/// v0.5.2's z ≥ 2 kept for the harm side (`worse` exactly when the same reading's z_permille ≤ −2000, then `fails`);
/// and the studentized permutation candidate at the same 2.275 %. k = 1 is the null: the rates are false rollbacks.
fn discrete_cell(c: &'static Card, nb: usize, na: usize, k: u64, n: u64, seed: u64) -> Value {
    let (mut r, mut rt) = (Rng(seed), Rng(seed ^ 0x5717_d3e7_7000_0001));
    let guard_m = Measure { guard: Some(0), ..Measure::default() };
    let (mut exact, mut zw, mut stud) = (0, 0, 0);
    for _ in 0..n {
        let mut s = 0.0;
        let mut o = turns(&mut r, IID, &mut s, nb + na, "t", 0, 0);
        for x in &mut o[nb..] {
            rise(c, k, &mut r, x);
        }
        let (b, a) = (vals(c, &o[..nb]), vals(c, &o[nb..]));
        let rd = reading(c, &b, &a);
        exact += u64::from(fails(c, &guard_m, &rd));
        let with = |v: &str| {
            let mut x = rd.clone();
            x["verdict"] = json!(v);
            x
        };
        zw += u64::from(fails(c, &guard_m, &with(if rd["z_permille"].as_i64().unwrap_or(0) <= -2000 { "worse" } else { "noise" })));
        let pw = studentized_worse_ppm(c, &b, &a, &mut rt);
        stud += u64::from(fails(c, &guard_m, &with(if pw <= NOISE_ALPHA_PPM { "worse" } else { "noise" })));
    }
    json!({"card": c.id, "n_before": nb, "n_after": na, "rise": k, "readings": n,
        "exact_l3": rate(exact, n), "z_worse_v052": rate(zw, n), "studentized_perm": rate(stud, n)})
}

const SEED: u64 = 0x6ba1_053d_5eed_0001;

fn seed_of(parts: &[&str]) -> u64 {
    let h = sha256_hex(parts.join("/").as_bytes());
    SEED ^ u64::from_str_radix(&h[..16], 16).unwrap()
}

/// The full tables (lane M #2 per reading, #3; and after L3). Debug build, about a few minutes; see the module head.
#[test]
#[ignore = "full GBAL null tables: cargo test --offline -p init-root --bin init-root goal::sim -- --ignored --nocapture --test-threads 1"]
fn gbal_null_tables() {
    let (n_read, n_read40, n_goal, n_goal40) = (20_000u64, 4_000u64, 4_000u64, 1_000u64);
    let mut reading_rows = Vec::new();
    for m in [IID, CLUSTERED] {
        for c in CARDS.iter() {
            for (nb, na) in [(5, 5), (10, 5), (40, 5), (40, 3)] {
                let n = if nb >= 40 { n_read40 } else { n_read };
                reading_rows.push(per_reading(c, m, nb, na, n, seed_of(&["reading", m.name, c.id, &nb.to_string(), &na.to_string()])));
            }
        }
    }
    let mut goal_rows = Vec::new();
    for m in [IID, CLUSTERED] {
        for id in ["cost_turn", "requests_turn", "corrections", "cache_read"] {
            let c = card(id).unwrap();
            for rounds in [3, 5, 10, 20] {
                goal_rows.push(per_goal(c, m, Mode::Stacked, rounds, n_goal, seed_of(&["goal", m.name, id, "stacked", &rounds.to_string()])));
            }
            goal_rows.push(per_goal(c, m, Mode::Wait40, 10, n_goal40, seed_of(&["goal", m.name, id, "wait40", "10"])));
        }
    }
    let v = json!({"provenance": provenance("cargo test --offline -p init-root --bin init-root goal::sim -- --ignored --nocapture --test-threads 1", SEED,
            json!({"readings_per_cell": n_read, "readings_per_cell_nb40": n_read40, "goals_per_cell": n_goal, "goals_per_cell_wait40": n_goal40})),
        "noise_alpha": NOISE_ALPHA, "per_reading": reading_rows, "per_goal": goal_rows});
    emit("gbal-null", &v);
}

/// The guardrail's power on real regressions, per card (`harm`), independent turns, 5 after against 5 and 40 before:
/// the counterpart of the null tables, measured on main and after L3 so a change in false rollbacks shows what it
/// costs in protection. `INIT_STATS_OUT=<dir>` writes `<dir>/gbal-power.json`.
#[test]
#[ignore = "guardrail power: cargo test --offline -p init-root --bin init-root goal::sim -- --ignored --nocapture --test-threads 1"]
fn gbal_power_table() {
    let mut rows = Vec::new();
    for c in CARDS.iter() {
        for (nb, n) in [(5usize, 4_000u64), (40, 2_000)] {
            rows.push(per_reading_power(c, IID, nb, 5, n, seed_of(&["power", c.id, &nb.to_string()])));
        }
    }
    let v = json!({"provenance": provenance("cargo test --offline -p init-root --bin init-root goal::sim -- --ignored --nocapture --test-threads 1", SEED,
        json!({"readings_per_cell": 4_000, "readings_per_cell_nb40": 2_000})),
        "harm": "cost, wall, requests ×2; cache reads ÷2; provider errors 10 % of requests (from 2 %); corrections 50 % of turns (from 10 %)", "per_reading": rows});
    emit("gbal-power", &v);
}

/// The discrete-card guardrail table (the coordinator's question after L3): detection of a 2×, 3× and 5× rise and the
/// null false-rollback rate per reading, for L3's exact test reading 5, 10, 15 or 20 turns after (against as many
/// before, the parent's when changes stack, or 40), v0.5.2's z rule for the harm side, and the studentized
/// permutation candidate. With `INIT_STATS_OUT=<dir>`: `<dir>/gbal-discrete-guard.json`. Slow in a plain debug build
/// (most cells take the Monte Carlo path); the documented run compiles this crate optimized:
/// `cargo test --offline -p init-root --bin init-root --config 'profile.dev.package.init-root.opt-level=3'
/// goal::sim::gbal_discrete_guard_table -- --ignored --nocapture --test-threads 1`.
#[test]
#[ignore = "discrete-card guardrails: see the doc comment for the command"]
fn gbal_discrete_guard_table() {
    let (n_null, n_harm) = (4_000u64, 1_000u64);
    let mut rows = Vec::new();
    for id in DISCRETE {
        let c = card(id).unwrap();
        for (nb, na) in [(5, 5), (10, 10), (15, 15), (20, 20), (40, 5), (40, 10), (40, 15), (40, 20)] {
            for k in [1, 2, 3, 5] {
                let n = if k == 1 { n_null } else { n_harm };
                rows.push(discrete_cell(c, nb, na, k, n, seed_of(&["discrete", id, &nb.to_string(), &na.to_string(), &k.to_string()])));
            }
        }
    }
    // what reading more turns costs: the guardrail reads `n_after` owner turns of the change before its verdict, and a
    // stacked change waits for it; X10's owner works 4 turns an hour (ledger/tests.rs, the corrections-trend null)
    let cost: Vec<Value> = [5u64, 10, 15, 20].iter().map(|&na| json!({"n_after": na, "extra_owner_turns_per_change": na - 5,
        "extra_hours_per_change_at_4_turns_an_hour": (na - 5) as f64 / 4.0, "extra_owner_turns_per_10_round_goal": 10 * (na - 5)})).collect();
    let v = json!({"provenance": provenance("cargo test --offline -p init-root --bin init-root --config 'profile.dev.package.init-root.opt-level=3' goal::sim::gbal_discrete_guard_table -- --ignored --nocapture --test-threads 1",
            SEED, json!({"readings_null": n_null, "readings_per_rise": n_harm, "studentized_draws": T_DRAWS})),
        "level": NOISE_ALPHA_PPM, "rules": {"exact_l3": "the Root's `reading` (exact permutation of the after-sum) and `fails`",
            "z_worse_v052": "v0.5.2: `worse` when the same reading's z_permille <= -2000 (z >= 2 in the harm direction), then `fails`",
            "studentized_perm": "permutation test with Welch's t as the statistic, 9,999 Monte Carlo relabelings, at the same level"},
        "rise": "k x requests; cache reads / k; provider errors at k x 2 % of requests; corrections on k x 10 % of turns (k = 1: null)",
        "cost": cost, "cells": rows});
    emit("gbal-discrete-guard", &v);
}

/// The unit-suite check: the Welch–t candidate's machinery agrees with known t tails (so the D1 comparison measures
/// the method, not a bug in it), and a short null goal run goes through the Root's code end to end.
#[test]
fn sim_machinery() {
    // t tails: t(5) at 2.015 → 0.05; t(1) at 1 → 0.25 (Cauchy); t(30) at 2.042 → 0.025 (standard tables)
    assert!((t_upper(2.015_048, 5.0) - 0.05).abs() < 1e-5, "{}", t_upper(2.015_048, 5.0));
    assert!((t_upper(1.0, 1.0) - 0.25).abs() < 1e-9);
    assert!((t_upper(2.042_272, 30.0) - 0.025).abs() < 1e-5);
    assert!((t_upper(-1.0, 1.0) - 0.75).abs() < 1e-9);
    let (pb, pw) = welch_t(true, &[10, 12, 11, 13, 9], &[10, 12, 11, 13, 9]);
    assert!((pb - 0.5).abs() < 1e-9 && (pw - 0.5).abs() < 1e-9);
    let mut r = Rng(SEED);
    let g = goal_run(card("cost_turn").unwrap(), IID, Mode::Stacked, 3, &mut r);
    assert!(g.readings >= 1 && g.readings <= 3, "{}", g.readings);
}

/// The L3 gate in the unit suite (fails on main): through the Root's code, a change that does nothing is rolled back by
/// an owner guardrail (tolerance 0, the strictest) on at most 2.3 % of readings, on every card, at 5 turns against 5
/// and against 40; and a goal of 10 rounds whose changes all do nothing ends verified `met` in at most 5 % of goals,
/// whenever the agent stops. Reduced sizes, seeded (so the result is fixed), with two Monte Carlo standard errors of
/// slack on the sample rate; the full tables are `gbal_null_tables`.
#[test]
fn l3_gate_null_rates() {
    let within = |v: &Value, bound: f64| v["rate"].as_f64().unwrap() <= bound + 2.0 * v["se"].as_f64().unwrap();
    for c in CARDS.iter() {
        for (nb, n) in [(5usize, 2_000u64), (40, 400)] {
            let row = per_reading(c, IID, nb, 5, n, seed_of(&["l3-gate", c.id, &nb.to_string()]));
            assert!(within(&row["guard_rollback_tol0"], 0.022_75), "{}: 5 against {nb}: {}", c.id, row["guard_rollback_tol0"]);
            assert!(within(&row["false_better"], 0.022_75), "{}: 5 against {nb}: {}", c.id, row["false_better"]);
        }
    }
    let g = per_goal(card("cost_turn").unwrap(), IID, Mode::Stacked, 10, 400, seed_of(&["l3-gate", "met"]));
    assert!(within(&g["false_met_per_goal"], 0.05), "{g}");
}
