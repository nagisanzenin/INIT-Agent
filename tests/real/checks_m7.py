#!/usr/bin/python3 -I
"""REAL-M7: the lean M7 proof lines on the real model, and their $0 dry run on init-mockllm.
Binding scope: build/specs/PROOF-LEAN.md. Design: docs/m7-design.md §3.6–§3.7, docs/evolution-v2.md §3.10.
Protocol and pre-registration: measurements/m7-runbook.md. Driven by tests/real/m7.sh.

  checks_m7.py selftest                      pure logic at $0 (no Root, no network, no key)
  checks_m7.py ready [--json]                which interfaces the lines need are present → blocker list
  checks_m7.py plan [--mean-musd N]          the lean plan and every line's real cap
  checks_m7.py offpeak [--wall-s N]          the advisory price window
  checks_m7.py charter|kconfig …             the test home's charter / kernel config for one line
  checks_m7.py prereg …                      D-0: freeze the pre-registration (sha256 + S record seqs)
  checks_m7.py run <line> …                  drive one line under the money guard (the Root is running)
  checks_m7.py verdict <line> …              one JSON verdict from the run dir; updates the ledger
  checks_m7.py redact <dir>                  e-mail / git-identity / key-shape pass and a final scan (counts only)

Lines: P (cost probe), C (S calibration + A/A), D (mine → propose → screen → G4, evolution-v2 §3.10 with
PROOF-LEAN D-L5; line R's $0.15 is a sub-cap inside D), M (milestone on the reserve + run-down),
G1L (lean real A/A, only after D was accepted as `better` or `lineage_gain`), G1D (mock only: the $0 exact
arm-symmetry check of a null change, the precondition for any real D).

Money. Every line has a REAL cap. A guard thread prices the Root's own egress usage at DeepSeek's list price
for each request's window every POLL_S (30 s), reads the free balance every BAL_S (300 s), and freezes spend
when the line's real cap is reached: it rewrites the test home's charter money ceilings to what is already
spent and applies it live (the Root then refuses every request). The charter ceiling itself is the line's
Root-metered cap plus the reservations of the parallel runs (a runaway backstop). This script never reads the
key file and never prints the owner's e-mail or git identity. It never reads ROOT_HOME/sealed/{S,seeds,g4}
or any per-task sealed result: S evidence reaches it only as Root lineage records (aggregates)."""
import argparse
import calendar
import glob
import hashlib
import json
import math
import os
import re
import socket
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import checks as S  # noqa: E402  S-REAL's plumbing: http over unix sockets, Owner (attach driver), lineage, traces

# ============================================================================================ constants
MUSD = 1_000_000
ENVELOPE_MUSD = 3_000_000          # M7 ≤ $3.00 real of the owner's $5 (D-M9, PROOF-LEAN)
PLAN_MUSD_PER_RUN = 3_000          # $0.003/run off-peak, until P measures it (m7 §3.6)
PLAN_MARGIN = 0.95                 # a plan fits when its expected total ≤ 95% of the envelope
CAP_FACTOR = 1.2                   # a line's cap = its estimate × 1.2, rounded up to the cent
RESERVATION_MUSD = 78_643          # one deepseek-flash request's Root reservation (65,536 out at the peak price)
PARALLEL = 4                       # sealed.parallel (m7 §4.3)
POLL_S = int(os.environ.get("M7_POLL_S", "30"))
BAL_S = int(os.environ.get("M7_BAL_S", "300"))
# deepseek-flash list prices in µUSD per Mtok (kernel config, real.sh); off-peak is half
PRICE = {"peak": (300_000, 6_000, 1_200_000), "offpeak": (150_000, 3_000, 600_000)}  # (input, cache_read, output)
# DeepSeek peak: 01:00–04:00 and 06:00–10:00 UTC, Mon–Fri (m7 §3.2.9). Chinese public holidays are off-peak at
# DeepSeek, but this table treats every weekday as having peak windows: the safe (costlier) direction.
PEAK_UTC_MIN = ((60, 240), (360, 600))

# S v1-lean (D-L1): G4 32 (E 14, C 9, T 9), reserve 20 held-out templates (E 10, C 5, T 5), 4 canaries
S_COUNTS = {"g4": {"E": 14, "C": 9, "T": 9}, "reserve": {"E": 10, "C": 5, "T": 5}, "canary": 4}
S_G4, S_RESERVE, S_CANARY = 32, 20, 4
POOL_N = 20                        # D-L4
SCREEN_K, CANARY_RUNS = 12, 7      # m7 §3.2.2, §3.2.6
TIER = {"lean": (24, 2), "standard": (32, 3), "strong": (32, 5)}
FIXED = {"worker": 30_000, "g0g3": 30_000, "rundown": 2_000}
CAP_FIXED = {"P": 60_000, "R": 150_000}   # pre-registered (m7 §3.6 P; evolution-v2 §3.10 D-0 line R)

# The lean plans (runbook §4): C's shape, M's repeats, the D tiers allowed
# C = the lean Root's `sealed calibrate --repeats 6` (32 × 6; ROOT-S1a cut replacements and template probes) + the
# kernel's pool calibration (`initctl evo calibrate`, 20 × 2). The plans differ in M's repeats and D's tiers.
PLANS = {
    "full": {"c": {"n": 32, "r": 6, "pool_r": 2}, "m_r": 3, "tiers": ("strong", "standard", "lean")},
    "trim": {"c": {"n": 32, "r": 6, "pool_r": 2}, "m_r": 2, "tiers": ("standard", "lean")},
    "t5": {"c": {"n": 32, "r": 6, "pool_r": 2}, "m_r": 2, "tiers": ("lean",)},
}
P_PROBE_PER_GROUP, P_GROUPS, P_POOL = 2, 4, 2      # `sealed probe --per-family 2` over E-py, E-rs, C, T + 2 pool runs
TASK_CAP_MUSD = 100_000                             # a task's max_cost_musd: the Root and kernel reserve it per run
EST_RUN_ROOT_MUSD = 5_000                           # the Root's slice estimate per run without history (slice.rs)
PLAN_DEFAULT_TIER = {"full": "standard", "trim": "standard", "t5": "lean"}

# D-0 (evolution-v2 §3.10, PROOF-LEAN D-L5). measurements/m7-runbook.md §D-0 carries the same values as text;
# selftest `d0_drift` fails if they diverge.
D0 = {
    "K": 3, "go_permille": 80, "better_permille": 150, "better_permille_lean": 250, "transfer_permille": 800,
    "q_permille": 500, "min_pool_tasks": 3, "min_families": 2, "attempts": 1, "r_cap_musd": 150_000,
    "predicts": "pass_rate@sealed >= parent+15%; cost_usd <= parent+30%",
    "sticks_classes": ("better", "lineage_gain"),
}
ACCEPT_STICKS = set(D0["sticks_classes"])
ALLOWED_EMAILS = {"agent@init.local", "maintainer@init.invalid", "noreply@anthropic.com", "git@github.com"}
EMAIL_RE = re.compile(rb"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}")
KEY_RE = re.compile(rb"sk-[A-Za-z0-9_\-]{12,}")
SEALED_FORBIDDEN = ("sealed/S/", "sealed/seeds/", "sealed/g4/")

# ============================================================================================ small helpers


def cent_up(musd):
    return int(math.ceil(musd / 10_000.0)) * 10_000


def usd(musd):
    return f"{musd / MUSD:.2f}" if musd is not None else "?"


def iso(t):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))


def parse_iso(s):
    try:
        return calendar.timegm(time.strptime(s[:19], "%Y-%m-%dT%H:%M:%S")) if s else None
    except (ValueError, TypeError):
        return None


def wjson(p, v):
    tmp = p + ".tmp"
    with open(tmp, "w") as f:
        json.dump(v, f, sort_keys=True, indent=1)
    os.replace(tmp, p)


def jload(p, default=None):
    return S.jload(p, default)


# ============================================================================================ price window (advisory)


def window_at(t):
    g = time.gmtime(t)
    if g.tm_wday >= 5:
        return "offpeak"
    m = g.tm_hour * 60 + g.tm_min
    return "peak" if any(a <= m < b for a, b in PEAK_UTC_MIN) else "offpeak"


def next_change(t):
    w, s = window_at(t), int(t) - int(t) % 60
    for k in range(1, 7 * 24 * 60 + 1):
        if window_at(s + 60 * k) != w:
            return s + 60 * k
    return None


def offpeak_advice(now, wall_s):
    """Advisory only (the orchestrator's rule): off-peak halves the real price but is not required. The cost
    factor rescales a line's off-peak estimate; the cap check, not this function, decides whether a line runs."""
    samples = max(1, int(wall_s // 60))
    peak = sum(1 for k in range(samples) if window_at(now + 60 * k) == "peak")
    frac = peak / samples
    w = window_at(now)
    nc = next_change(now)
    adv = {"window_now": w, "peak_fraction": round(frac, 3), "cost_factor": round(1 + frac, 3),
           "root_real_factor": 2.0 if peak == 0 else 1.0, "next_change_utc": iso(nc) if nc else None}
    if w == "peak":
        adv["advice"] = f"peak now: real cost x2 for this part; off-peak starts {adv['next_change_utc']}"
    elif frac > 0:
        adv["advice"] = f"off-peak now, but a peak window starts {adv['next_change_utc']} inside the projected wall"
    else:
        adv["advice"] = "off-peak for the whole projected wall"
    return adv


# ============================================================================================ the plan


def runs_c(c):
    return c["n"] * c["r"] + POOL_N * c["pool_r"]


def slice_est_root(tier):
    """The Root's own pre-check of a slice budget (ROOT-S1a slice.rs): (5 + 2·n·R) × $0.005."""
    n, r = TIER[tier]
    return (5 + 2 * n * r) * EST_RUN_ROOT_MUSD


def runs_d(tier, screen_fresh=False):
    n, r = TIER[tier]
    return n * r * 2 + SCREEN_K * (2 if screen_fresh else 1) + CANARY_RUNS


def runs_m(r):
    return S_RESERVE * r * 2 + 3


def line_estimates(plan, mean, tier=None):
    p = PLANS[plan]
    tier = tier or PLAN_DEFAULT_TIER[plan]
    return {"C": runs_c(p["c"]) * mean, "R": FIXED["worker"] + D0["K"] * 6 * mean,
            "D": FIXED["g0g3"] + runs_d(tier) * mean, "M": FIXED["rundown"] + runs_m(p["m_r"]) * mean,
            "G1L": FIXED["g0g3"] + runs_d("lean", screen_fresh=True) * mean}


def cap_of(est):
    return cent_up(est * CAP_FACTOR)


def caps_for(plan, mean, tier=None):
    e = line_estimates(plan, mean, tier)
    c = {k: cap_of(v) for k, v in e.items()}
    c["R"] = CAP_FIXED["R"]
    return e, c


def plan_fits(name, mean, spent_p):
    """A plan fits when C can finish at its estimate with every later line's cap reserved (R, D at the plan's
    default tier, M, G1L) inside the envelope."""
    e, caps = caps_for(name, mean)
    later = caps["R"] + caps["D"] + caps["M"] + caps["G1L"]
    return spent_p + e["C"] + later <= ENVELOPE_MUSD, e, caps, later


def pick_plan(mean, spent_p):
    """The richest plan that fits (runbook §4). `stop` above the t5 band."""
    for name in ("full", "trim", "t5"):
        fits, e, caps, later = plan_fits(name, mean, spent_p)
        if fits:
            caps["C"] = min(caps["C"], ENVELOPE_MUSD - spent_p - later)
            total = spent_p + sum(e.values())
            return {"plan": name, "mean_musd_per_run": mean, "expected_total_musd": int(total), "est": {k: int(v) for k, v in e.items()},
                    "caps": caps, "c": PLANS[name]["c"], "m_r": PLANS[name]["m_r"], "tiers": list(PLANS[name]["tiers"])}
    return {"plan": "stop", "mean_musd_per_run": mean,
            "why": "P's mean is above the t5 band: tighten the caps (turns 6, output 12,000), re-probe once, re-plan"}


def plan_bands():
    """The largest mean $/run (µUSD) at which each plan still fits, with P at 10 runs of that mean."""
    out = {}
    for name in ("full", "trim", "t5"):
        lo, hi = 100, 20_000
        while hi - lo > 1:
            mid = (lo + hi) // 2
            lo, hi = (mid, hi) if plan_fits(name, mid, 10 * mid)[0] else (lo, mid)
        out[name] = lo
    return out


def d_tier(plan, remaining, caps_m, caps_g1l, mean):
    """D-0's tier rule: the strongest tier the plan allows whose cap, plus M's and G1L's caps, fits what is left."""
    for t in PLANS[plan]["tiers"]:
        cap_d = cap_of(FIXED["g0g3"] + runs_d(t) * mean)
        if cap_d + caps_m + caps_g1l <= remaining:
            return t, cap_d
    return None, None


# A line's share of runs on the Root's sealed lanes, which are charged to `sealed.reserve_usd` and nothing else
# (egress.md §3.3, money.rs); the rest goes through the zone (`money.total_usd − reserve`). Permille, by run count.
SEALED_SHARE = {"P": 600, "C": 870, "D": 900, "M": 980, "G1L": 760, "G1D": 760}


def line_budget(led, line):
    """A line's real cap, and the G4 tier for D and G1L, from the ledger (runbook §3–§4). cap 0 = may not run."""
    plan = led.get("plan") or {}
    name, mean = plan.get("plan", "full"), int(plan.get("mean_musd_per_run") or PLAN_MUSD_PER_RUN)
    left = ENVELOPE_MUSD - spent_real(led)
    if os.environ.get("M7_LEFT_BY_BALANCE_MUSD"):
        # the owner's rule from 2026-10-21: $3.00 real measured by balance (floor $6.56); the harness passes balance - floor
        left = int(os.environ["M7_LEFT_BY_BALANCE_MUSD"])
    if line == "P":
        return {"cap_real_musd": min(CAP_FIXED["P"], left), "tier": None, "left_musd": left}
    if name == "stop":
        return {"cap_real_musd": 0, "tier": None, "why": "plan stop", "left_musd": left}
    m_r = PLANS[name]["m_r"]
    cap_m = cap_of(FIXED["rundown"] + runs_m(m_r) * mean)
    cap_g1l = cap_of(FIXED["g0g3"] + runs_d("lean", screen_fresh=True) * mean)
    if line == "C":
        cap = int((plan.get("caps") or {}).get("C") or cap_of(runs_c(PLANS[name]["c"]) * mean))
        return {"cap_real_musd": min(cap, left), "tier": None, "left_musd": left}
    if line == "D" and os.environ.get("M7_D_TIER") in TIER:
        # the tier D-0 item 6 fixed after line C (recorded by D's first run); a continuation after the DV-1/DV-2 bug
        # stops keeps it. Only the line's own cap must fit what is left; later lines start only if theirs fit then.
        t = os.environ["M7_D_TIER"]
        cap_d = cap_of(FIXED["g0g3"] + runs_d(t) * mean)
        if CAP_FIXED["R"] + cap_d > left:
            return {"cap_real_musd": 0, "tier": None, "why": f"pinned tier {t} does not fit what is left", "left_musd": left}
        return {"cap_real_musd": CAP_FIXED["R"] + cap_d, "cap_r_musd": CAP_FIXED["R"], "cap_d_musd": cap_d, "tier": t,
                "tier_pinned": True, "left_musd": left}
    if line == "D":
        tier, cap_d = d_tier(name, left - CAP_FIXED["R"], cap_m, cap_g1l, mean)
        if not tier:
            return {"cap_real_musd": 0, "tier": None, "why": "no D tier fits with M and G1L reserved", "left_musd": left}
        return {"cap_real_musd": CAP_FIXED["R"] + cap_d, "cap_r_musd": CAP_FIXED["R"], "cap_d_musd": cap_d, "tier": tier,
                "reserved_after_musd": cap_m + cap_g1l, "left_musd": left}
    if line == "M":
        if os.environ.get("M7_LEFT_BY_BALANCE_MUSD"):  # bounded by the balance left; the guard's floor stops it
            return {"cap_real_musd": max(0, min(cap_m, left)), "tier": None, "m_r": m_r, "left_musd": left}
        return {"cap_real_musd": cap_m if cap_m <= left else 0, "tier": None, "m_r": m_r, "left_musd": left}
    if line in ("G1L", "G1D"):
        return {"cap_real_musd": cap_g1l if cap_g1l <= left else 0, "tier": "lean", "left_musd": left}
    return {"cap_real_musd": 0, "tier": None, "left_musd": left}


def pools_spent(rh):
    u = priced_usage(rh, 0)
    sealed = (u["by_lane"].get("sealed") or {}).get("root_musd", 0)
    return sealed, u["root_musd"] - sealed


def params_for(led, line, mode, rh, repo, advice, mock_port=None, sealed_m7_keys=False, g4_keys=False, toolchain="", devdir=""):
    """The charter and kernel-config parameters of one line. Root ceilings (Root-metered, peak prices): each pool
    gets its share of cap × the Root/real factor (2.0 off-peak, 1.0 when any peak minute is projected), plus 4 parallel
    reservations, above what it has already spent. They are a runaway backstop; the guard's real cap is the tight
    stop (a ceiling at exactly the cap would refuse in-flight reservations, and a refused candidate run scores 0)."""
    b = line_budget(led, line)
    cap = b["cap_real_musd"]
    ratio = led.get("root_real_ratio")
    factor = 1.0 if advice.get("peak_fraction", 0) > 0 else (min(2.0, max(1.0, ratio)) if ratio else 2.0)
    share = SEALED_SHARE.get(line, 800)
    sealed_spent, zone_spent = pools_spent(rh) if rh and os.path.isdir(rh) else (0, 0)
    h_sealed = cent_up(cap * factor * share / 1000) + PARALLEL * RESERVATION_MUSD
    h_zone = cent_up(cap * factor * (1000 - share) / 1000) + PARALLEL * RESERVATION_MUSD
    reserve = cent_up(sealed_spent) + cent_up(h_sealed)
    zone_cap = cent_up(zone_spent) + cent_up(h_zone)
    deny = [os.path.join(repo, "root"), os.path.join(repo, ".git")]
    # Root sealed operations (probe, calibrate, milestone) and kernel gates reserve a run's whole task cap before it
    # starts, so both budgets carry PARALLEL × that cap on top of the line's own share (else runs stop as `partial`).
    sealed_op = cent_up(cap * factor * share / 1000) + PARALLEL * TASK_CAP_MUSD
    gate = cent_up(cap * 2.0) + PARALLEL * TASK_CAP_MUSD
    if b.get("tier"):
        gate = max(gate, cent_up(slice_est_root(b["tier"]) * 1.15) + PARALLEL * TASK_CAP_MUSD)
    return {"mode": mode, "line": line, "cap_real_musd": cap, "tier": b.get("tier"), "budget": b, "factor": factor,
            "reserve_musd": reserve, "ceiling_musd": reserve + zone_cap, "zone_cap_musd": zone_cap, "read_deny": deny,
            "mock_port": mock_port, "sealed_m7_keys": sealed_m7_keys, "m_r": b.get("m_r", 3), "g4_keys": g4_keys,
            # the kernel's per-task budget must hold the spend plus one request reservation (~$0.04 off-peak at 64k output
            # tokens): at $0.05 the real D-3 worker was refused after $0.0068 (REAL-M7 real D, first try)
            "toolchain": toolchain, "devdir": devdir, "task_musd": CAP_FIXED["R"] if line == "D" else 100_000,
            "sealed_op_musd": sealed_op, "gate_musd": gate,
            "cost_prior_musd": led.get("cost_prior_musd"),
            # D's G4 retry (orchestrator, 2026-10-21): sealed.parallel 2 and gate.g4.parallel 1 lower the peak memory and
            # disk; the money reservations above keep PARALLEL (more headroom, never less)
            "sealed_parallel": int(os.environ.get("M7_SEALED_PARALLEL") or PARALLEL),
            "g4_parallel": int(os.environ.get("M7_G4_PARALLEL") or 0),
            # G3's stage timeouts (kernel defaults 300 s): a cold build slot needs ~280 s before the model's turns
            "g3_timeout_s": int(os.environ.get("M7_G3_TIMEOUT_S") or 0)}


def frozen(params, rh):
    """Ceilings := what each pool has spent (rounded down to the cent); the zone total gets one cent more because the
    charter requires money > 0 (C5). Every request reserves ≥ RESERVATION_MUSD (65,536 output tokens), far above one
    cent, so the Root refuses every further request in both pools."""
    sealed_spent, zone_spent = pools_spent(rh)
    reserve = (sealed_spent // 10_000) * 10_000
    return dict(params, reserve_musd=reserve, ceiling_musd=reserve + (zone_spent // 10_000) * 10_000 + 10_000, frozen=True)


# ============================================================================================ meters


def priced_usage(rh, since_ts, until_ts=None, ratio_window=True):
    """The Root's egress usage ledger (RH/egress/usage/*.jsonl) since `since_ts`: requests, tokens, the Root's
    own meter (`cost_musd`, peak prices) and a real-$ estimate at DeepSeek's list price for each request's window.
    Usage = {input (cache reads included), cache_read, output} (root egress tap.rs). A 2xx with no usage is
    counted at the Root's charge (its reservation), the safe direction."""
    lanes, out = {}, {"requests": 0, "in": 0, "cr": 0, "out": 0, "root_musd": 0, "real_musd": 0, "by_lane": {}, "no_usage": 0}
    recs = S.jl(os.path.join(rh, "egress/usage/*.jsonl"))
    for r in recs:
        if r.get("t") == "start" and r.get("id"):
            lanes[r["id"]] = r.get("lane") or "?"
    for r in recs:
        if r.get("t") not in ("end", "sealed"):
            continue
        t = parse_iso(r.get("at"))
        if t is None or t < since_ts or (until_ts and t > until_ts):
            continue
        lane = r.get("lane") or lanes.get(r.get("id"), "?")
        u = r.get("usage")
        root = int(r.get("cost_musd") or 0)
        if isinstance(u, dict):
            inp, cr, o = int(u.get("input") or 0), int(u.get("cache_read") or 0), int(u.get("output") or 0)
            cr = min(cr, inp)
            pi, pc, po = PRICE[window_at(t)] if ratio_window else PRICE["peak"]
            real = -(-((inp - cr) * pi + cr * pc + o * po) // MUSD)
        else:
            inp = cr = o = 0
            real = root
            if 200 <= int(r.get("status") or 0) < 300:
                out["no_usage"] += 1
        if root == 0 and real == 0:
            continue
        out["requests"] += 1
        out["in"] += inp
        out["cr"] += cr
        out["out"] += o
        out["root_musd"] += root
        out["real_musd"] += real
        bl = out["by_lane"].setdefault(lane, {"requests": 0, "root_musd": 0, "real_musd": 0})
        bl["requests"] += 1
        bl["root_musd"] += root
        bl["real_musd"] += real
    return out


def parse_balance(v):
    """`GET user/balance` → µUSD of the USD balance, or None."""
    try:
        for i in v.get("balance_infos") or []:
            if i.get("currency") == "USD":
                d = str(i.get("total_balance"))
                whole, _, frac = d.partition(".")
                return int(whole) * MUSD + int((frac + "000000")[:6])
    except (AttributeError, ValueError, TypeError):
        pass
    return None


def read_balance(ih, mode, sim=None):
    """Real: the free balance endpoint through the kernel proxy → Root egress (as S-REAL). Mock: a simulated
    balance (the mock serves no balance endpoint): B0 − priced usage + an injected drain, for the stop tests."""
    if mode == "real":
        try:
            st, _, b = S.http(os.path.join(ih, "run/proxy.sock"), "GET", "/deepseek/user/balance",
                              headers={"x-init-session": "m7balance", "x-init-purpose": "turn"}, timeout=20)
            return parse_balance(json.loads(b)) if st == 200 else None
        except Exception:
            return None
    if sim is None:
        return None
    return sim()


class Guard:
    """The per-line money guard: pure decisions on readings (unit-tested in selftest).
    Stops: a tripwire record (halts all spend); the priced real estimate reaching the cap; the balance drop
    reaching the cap; the balance falling faster than the priced usage (> 25% + $0.02, after ≥ $0.10).
    Two unreadable balances in a row → `tighten`: the Root ceiling must then bound the rest at peak prices."""

    def __init__(self, cap_real, bal0, mismatch_permille=250, floor=None):
        self.cap, self.bal0, self.mm, self.floor = cap_real, bal0, mismatch_permille, floor
        self.bal_fail, self.stop, self.halt, self.tightened = 0, None, None, False

    def eff_cap(self):
        """The line's cap, bounded by the program's balance floor (orchestrator 2026-10-21: $3.00 real by balance
        from $9.56, so the floor is $6.56): what is left above the floor at the line's start."""
        if self.floor is not None and self.bal0 is not None:
            return max(0, min(self.cap, self.bal0 - self.floor))
        return self.cap

    def tick(self, real_est, balance="skip", tripwire=None):
        if self.stop:
            return self.stop, None
        if tripwire:
            self.stop, self.halt = f"tripwire:{tripwire}", f"tripwire:{tripwire}"
            return self.stop, "freeze"
        cap = self.eff_cap()
        if real_est >= cap:
            self.stop = "real_cap:priced_usage" if cap == self.cap else "program_cap:priced_usage"
            return self.stop, "freeze"
        if balance != "skip":
            if balance is None:
                self.bal_fail += 1
                if self.bal_fail >= 2 and not self.tightened:
                    self.tightened = True
                    return None, "tighten"
            elif self.floor is not None and balance <= self.floor:
                self.stop = self.halt = "program_cap:balance_floor"
                return self.stop, "freeze"
            elif self.bal0 is not None:
                self.bal_fail = 0
                drop = self.bal0 - balance
                if drop >= cap:
                    self.stop = "real_cap:balance"
                    return self.stop, "freeze"
                if drop >= 100_000 and drop > real_est * (1000 + self.mm) // 1000 + 20_000:
                    self.stop = self.halt = "meter_mismatch"
                    return self.stop, "freeze"
        return None, None


# ============================================================================================ D: selection, go/no-go, Predicts


def families_of(f):
    fam = [x for x in (f.get("families") or []) if x]
    if not fam:
        fam = [x for x in (f.get("labels") or []) if x not in ("live", "smoke")]
    return sorted(set(fam))


def select_cluster(frictions, sreal_counts=None, exclude=()):
    """D-0's selection rule: among eligible, untried clusters spanning ≥ 3 pool tasks and ≥ 2 families, the
    highest s_c; ties → more S-REAL occurrences of the signature; then the lower friction id."""
    sreal_counts = sreal_counts or {}
    ok = []
    for f in frictions:
        c = f.get("counts") or {}
        if not f.get("eligible") or f.get("tried") or f.get("id") in exclude:
            continue
        if int(c.get("tasks") or 0) < D0["min_pool_tasks"] or len(families_of(f)) < D0["min_families"]:
            continue
        ok.append(f)
    ok.sort(key=lambda f: (-int(f.get("s_c_permille") or 0), -int(sreal_counts.get(f.get("signature"), 0)), f.get("id") or ""))
    return ok[0] if ok else None


def est_permille(s_c_permille, fixed, targeted, q_permille=None, t_permille=None):
    q = D0["q_permille"] if q_permille is None else q_permille
    t = D0["transfer_permille"] if t_permille is None else t_permille
    fix = (fixed + 1) / (targeted + 2)
    return int(round(1000 * (q / 1000) * (s_c_permille / 1000) * fix * (t / 1000)))


def go_no_go(est, tier):
    """→ (go, aim). At lean only `better` is aimed for, at ≥ 250‰ (NI is underpowered at lean, so the ratchet
    cannot fire there)."""
    better = D0["better_permille_lean"] if tier == "lean" else D0["better_permille"]
    if est >= better:
        return True, "better"
    if tier != "lean" and est >= D0["go_permille"]:
        return True, "lineage_gain"
    return False, "below_resolution"


def predicts_line(est, aim):
    n = 15 if aim == "better" else max(1, int(round(est / 10)))
    return f"pass_rate@sealed >= parent+{n}%; cost_usd <= parent+30%"


PRED_CLAUSE = re.compile(r"^(pass_rate|cost_usd|wall_s|cache_hit|turns|tokens_in|tokens_out|gap_ms_p50|body_tokens|ext_calls:[a-z0-9_-]+)"
                         r"(@(all|pool|holdout|sealed|label:[a-z0-9-]{1,40}|task:[A-Za-z0-9._-]+))?\s*(<=|>=|<|>)\s*"
                         r"(parent([+-][0-9]+(\.[0-9]+)?%)?|[0-9]+(\.[0-9]+)?)$")


def predicts_ok(line):
    """evolution.md §3.10.1's metric clauses (the subset the demo uses), plus its `sealed` scope rule."""
    for c in [x.strip() for x in line.split(";") if x.strip()]:
        m = PRED_CLAUSE.match(c)
        if not m:
            return False
        if m.group(3) == "sealed" and (m.group(1) != "pass_rate" or m.group(4) not in (">=", ">") or not m.group(5).startswith("parent")
                                        or (m.group(6) or "+").startswith("-")):
            return False
    return True


T2_PREFIX = "src/"


def tier_of_paths(paths):
    """The demo's tier rule (orchestrator, D-L2: S slices measure only T2/dep changes, and dep is forbidden to the
    engine at L0): T2 iff ≥ 1 changed path under the body crate's `src/` and nothing under `vendor/`."""
    ps = [p.strip() for p in paths if p.strip()]
    if any(p.startswith("vendor/") or p == "vendor" for p in ps):
        return "dep"
    if any(p.startswith(T2_PREFIX) for p in ps):
        return "t2"
    if any(p.startswith("ext/") for p in ps):
        return "t1"
    return "t0" if ps else "none"


def accept_class(gate):
    """The gate record's accept class (better | lineage_gain | tie_break:<which>) and whether it counts for
    "sticks" (D-0: only better or lineage_gain)."""
    g = gate or {}
    verdict = g.get("verdict")
    cls = g.get("accept_class") or g.get("class")
    if not cls and verdict == "accept":
        r = g.get("reason") or ""
        cls = r if r in ("better", "lineage_gain") else f"tie_break:{r}" if r else None
    return {"verdict": verdict, "class": cls, "counts_for_sticks": verdict == "accept" and cls in ACCEPT_STICKS}


# ============================================================================================ redaction


def git_identity():
    """The global git identity, read in-process only to redact it; never printed or written."""
    vals = []
    for k in ("user.email", "user.name"):
        try:
            r = subprocess.run(["/usr/bin/git", "config", "--global", "--get", k], capture_output=True, text=True, timeout=5,
                               env={"PATH": "/usr/bin:/bin", "HOME": os.path.expanduser("~")})
            v = r.stdout.strip()
            if len(v) >= 3:
                vals.append(v.encode())
        except Exception:
            pass
    return vals


def redact_bytes(b, secrets):
    n = 0
    for s in secrets:
        if s and s in b:
            n += b.count(s)
            b = b.replace(s, b"[REDACTED:id]")
    cnt = [0]

    def em(m):
        if m.group(0).decode("latin-1").lower() in ALLOWED_EMAILS:
            return m.group(0)
        cnt[0] += 1
        return b"[REDACTED:email]"
    b = EMAIL_RE.sub(em, b)
    b, c2 = KEY_RE.subn(b"[REDACTED]", b)
    return b, n + cnt[0] + c2


def leaks_in(b, secrets):
    hits = sum(b.count(s) for s in secrets if s)
    hits += sum(1 for m in EMAIL_RE.finditer(b) if m.group(0).decode("latin-1").lower() not in ALLOWED_EMAILS)
    hits += len(KEY_RE.findall(b))
    return hits


def redact_tree(d, secrets=None):
    """Rewrites text files under `d` in place; returns counts only. A final scan must find 0 leaks."""
    secrets = git_identity() if secrets is None else secrets
    files = red = leaks = 0
    for root, _, names in os.walk(d):
        for nm in names:
            p = os.path.join(root, nm)
            if os.path.islink(p) or not os.path.isfile(p) or os.path.getsize(p) > (4 << 20):
                continue
            with open(p, "rb") as f:
                b = f.read()
            if b"\0" in b[:4096]:
                continue
            nb, c = redact_bytes(b, secrets)
            if nb != b:
                with open(p, "wb") as f:
                    f.write(nb)
                red += max(c, 1)
            files += 1
            leaks += leaks_in(nb, secrets)
    return {"files": files, "redacted": red, "leaks_after": leaks}


# ============================================================================================ readiness (interfaces)

# Each item: (lane, what, where, needle). `where`: a binary name under the bins dir, or "path:<repo-relative>".
# Needles are the documented command or key strings (m7-design §4, evolution-v2 §4); they are confirmed against
# each lane's status file when it lands. Lines list what they need.
IFACE = {
    "charter_read_home": ("SBX-009", "charter sandbox.read_home", "init-root", b"read_home"),
    "sealed_draw": ("ROOT-S1a", "init-root sealed draw + sealed_draw record", "init-root", b"sealed_draw"),
    "sealed_probe": ("ROOT-S1a", "init-root sealed calibrate --probe 2", "init-root", b"--probe"),
    "sealed_calibrate": ("ROOT-S1a", "init-root sealed calibrate (+ sealed_calibrate record, pseudo-A/A)", "init-root", b"sealed_calibrate"),
    "milestone": ("ROOT-S1a", "init-root audit run --reason milestone --set reserve (+ milestone record)", "init-root", b"milestone"),
    "g4_slice": ("ROOT-S1a", "control link sealed.g4_slice (+ sealed_g4 record)", "init-root", b"sealed.g4_slice"),
    "sealed_charter_keys": ("ROOT-S1a", "charter [sealed] pack / ratchet_k / milestone_repeats", "init-root", b"milestone_repeats"),
    "signflip": ("ROOT-S1b", "exact sign-flip + A/A math", "init-root", b"sealed_calibrate"),
    "slice_client": ("GATE-V2a", "kernel G4 v2 slice client + accept_v2", "init-kernel", b"accept_class"),
    "g4_tier_key": ("GATE-V2a", "kernel config [gate.g4] tier / cost_prior_usd", "init-kernel", b"cost_prior_usd"),
    "pool_cache": ("GATE-V2b", "pool cache (incumbent reuse) + screen", "init-kernel", b"screen_inferior"),
    "pool_calibrate": ("REAL-M7 (087b5e5)", "initctl evo calibrate: the LKG on the pool, seeds the pool cache", "initctl", b"evo calibrate"),
    "screen_e3": ("GATE-V2b/EVO-LITE", "E3 candidate-only mini-run", "init-kernel", b"screen.mini_inferior"),
    "evo_frictions": ("EVO-LITE", "initctl evo frictions + state/kernel/evo/frictions.jsonl", "initctl", b"frictions"),
    "evo_propose": ("EVO-LITE", "initctl evo propose / variant / screen / status", "initctl", b"evo variant"),
    "trace_retention": ("GATE-V2b", "failing pool runs' session logs retained (EV-TRACE)", "init-kernel", b"sessions"),
    "pool_tasks": ("POOL-LITE", "20 visible pool tasks + installer import", "path:fixtures/pool-v1", None),
    "pack_s1": ("S1-CORE + S1-E/C/T", "pack s1 (pack.json) for the real lines", "path:root/sealed/packs/s1/pack.json", None),
    "pack_fixture": ("S1-*", "the pack the dry run draws (M7_FIXTURE_PACK, default pack s1)", "path:root/sealed/packs/s1/pack.json", None),
    "mock_balance": ("(optional) testkit", "init-mockllm GET user/balance (the dry run simulates it otherwise)", "init-mockllm", b"user/balance"),
}
LINE_NEEDS = {
    "P": ["charter_read_home", "sealed_draw", "sealed_probe", "pool_tasks", "pool_calibrate"],
    "C": ["charter_read_home", "sealed_calibrate", "signflip", "pool_tasks", "pool_calibrate", "pool_cache"],
    "D": ["charter_read_home", "evo_frictions", "evo_propose", "trace_retention", "screen_e3", "pool_cache", "slice_client",
          "g4_tier_key", "g4_slice", "signflip"],
    "M": ["charter_read_home", "milestone", "signflip"],
    "G1L": ["slice_client", "g4_tier_key", "g4_slice"],
    "G1D": ["slice_client", "g4_slice", "pack_fixture"],
}


def lane_status(repo, lane):
    p = os.path.join(repo, "build/status", f"{lane.split('/')[0].split(' ')[0]}.md")
    if not os.path.exists(p):
        return "no status file"
    head = open(p, errors="replace").read(1500)
    m = re.search(r"(?im)^\s*\**status:?\**\s*:?\s*\**([a-z ()/-]+)", head)
    return (m.group(1).strip() if m else "status file present")[:60]


def readiness(repo, bins):
    out, blobs = {}, {}
    for k, (lane, what, where, needle) in IFACE.items():
        if where.startswith("path:"):
            ok = os.path.exists(os.path.join(repo, where[5:]))
        elif where.startswith("env:"):
            v = os.environ.get(where[4:])
            ok = bool(v) and os.path.isdir(v)
        elif where.startswith("status:"):
            st = lane_status(repo, where[7:])
            ok = st.lower().startswith("done")
        else:
            if where not in blobs:
                p = os.path.join(bins, where)
                blobs[where] = open(p, "rb").read() if os.path.exists(p) else None
            ok = blobs[where] is not None and needle in blobs[where]
        out[k] = {"ok": bool(ok), "lane": lane, "what": what, "lane_status": lane_status(repo, lane)}
    lines = {ln: {"ready": all(out[i]["ok"] for i in need), "missing": [i for i in need if not out[i]["ok"]]} for ln, need in LINE_NEEDS.items()}
    return {"items": out, "lines": lines}


# ============================================================================================ test-home config


def charter_text(p):
    """The M7 test home's charter. read_home = false (PROOF-LEAN), the dev checkout's pack and git dirs denied,
    money ceilings = the line's Root-metered cap + parallel reservations (set per line, frozen on a stop)."""
    deny = ", ".join(json.dumps(x) for x in p["read_deny"])
    sealed = ""
    if p.get("sealed_m7_keys"):
        # the lean Root's M7 keys (ROOT-S1a; looks, rotation and the partition sizes are cut or fixed by D-L1/D-L2)
        sealed = f"pack = \"s1\"\nratchet_k = 4\nmilestone_repeats = {p.get('m_r', 3)}\n"
    providers = (f"destinations = [\"127.0.0.1:{p['mock_port']}\"]\npaths = [\"chat/completions\", \"models\", \"user/balance\"]\n"
                 f"[actions.providers.providers.deepseek]\nhost = \"127.0.0.1:{p['mock_port']}\"\n") if p["mode"] == "mock" else (
                 "destinations = [\"api.deepseek.com:443\"]\npaths = [\"chat/completions\", \"models\", \"user/balance\"]\n")
    return f"""schema_version = 1
[purpose]
text = "REAL-M7 test home ({p['mode']}): the lean M7 proof lines; line {p['line']}, real cap ${usd(p['cap_real_musd'])}."
[autonomy]
max_level = 4
[money]
day_usd = "{usd(p['ceiling_musd'])}"
total_usd = "{usd(p['ceiling_musd'])}"
day_starts_at_utc_offset = "+00:00"
[quotas]
max_instances = 12
disk_min_free_gib = {p.get('disk_floor_gib', 5)}
[sealed]
parallel = {p.get('sealed_parallel') or PARALLEL}
margin_max = 0.25
audit_n = 32
audit_repeats = 3
reserve_usd = "{usd(p['reserve_musd'])}"
audit_usd = "{usd(p['reserve_musd'])}"
audit_schedule = "manual"
{sealed}[sandbox]
read_home = false
read_deny = [{deny}]
allow_full = false
[mirror]
url = ""
[root]
test_mode = {"true" if p["mode"] == "mock" else "false"}
[actions.providers]
{providers}[actions.providers.models."deepseek-flash"]
min_level = 0
input_musd_per_mtok = 300000
cache_read_musd_per_mtok = 6000
output_musd_per_mtok = 1200000
reserve_output_tokens = 65536
"""


def kconfig_text(p):
    """kernel/config.toml for one line: deepseek-flash only; soft budgets under the charter; the G4 tier pinned
    (m7 §3.2.6 'during the M7 program the tier is pinned') when the kernel knows the key."""
    g4 = ""
    if p.get("g4_keys") and (p.get("tier") or p.get("cost_prior_musd")):
        g4 = "\n[gate.g4]\n" + (f"tier = \"{p['tier']}\"\n" if p.get("tier") else "")
        g4 += f"cost_prior_usd = \"{p['cost_prior_musd'] / MUSD:.4f}\"\n" if p.get("cost_prior_musd") else ""
    if p.get("g4_parallel"):
        g4 = (g4 or "\n[gate.g4]\n") + f"parallel = {int(p['g4_parallel'])}\n"
    if p.get("g3_timeout_s"):
        g4 += f"\n[gate.g3]\nagent_timeout_s = {int(p['g3_timeout_s'])}\ninner_timeout_s = {int(p['g3_timeout_s'])}\n"
    pt = "true" if p["mode"] == "mock" else "false"
    gate = usd(p.get("gate_musd") or max(p["cap_real_musd"], 100_000))
    return f"""schema_version = 2
[model]
provider = "deepseek"
id = "deepseek-flash"

[proxy]
test_mode = {pt}

[budget]
day_usd = "{usd(max(2 * p['cap_real_musd'], 200_000))}"
task_usd = "{usd(p.get('task_musd', 50_000))}"
request_max_usd = "0.15"
gate_usd = "{gate}"
quick_gate_usd = "0.10"
probe_usd = "0.10"

[sandbox]
toolchain = "{p['toolchain']}"
developer_dir = "{p['devdir']}"

[supervise]
build_timeout_cold_s = 3600
build_timeout_warm_s = 1800
{g4}
[providers.deepseek.schedule]
tz      = "+00:00"
default = "offpeak"
windows = [
  {{ days = "mon-fri", from = "01:00", to = "04:00", use = "peak" }},
  {{ days = "mon-fri", from = "06:00", to = "10:00", use = "peak" }},
]

[providers.deepseek.models."deepseek-flash"]
max_output            = 393216
reserve_output_tokens = 65536
reference_window      = "peak"
prices.peak    = {{ input = "0.30", cache_read = "0.006", output = "1.20" }}
prices.offpeak = {{ input = "0.15", cache_read = "0.003", output = "0.60" }}
"""


# ============================================================================================ ledger


def ledger_path(home_dir):
    return os.path.join(home_dir, "ledger.json")


def ledger_load(home_dir):
    return jload(ledger_path(home_dir), None) or {"v": 1, "envelope_musd": ENVELOPE_MUSD, "lines": {}, "plan": None, "halt": None,
                                                   "root_real_ratio": None, "prereg": None}


def spent_real(led):
    """Program spend so far, per line the larger of the priced estimate and the settled balance drop."""
    return sum(max(int(v.get("real_est_musd") or 0), int(v.get("balance_drop_musd") or 0)) for v in led["lines"].values())


ORDER = ["P", "C", "D", "M", "G1L"]


def may_start(led, line, mode, mock_led=None):
    """Order, halts and the envelope (runbook §3). Mock homes follow the same rules at $0."""
    if led.get("halt"):
        return False, f"halted: {led['halt']} (a $0 fix and the orchestrator's clear-halt first)"
    done = {k for k, v in led["lines"].items() if v.get("state") in ("done", "stopped")}
    passed = {k for k, v in led["lines"].items() if v.get("pass")}
    if line == "G1D":
        return (mode == "mock"), "G1D runs on the mock only"
    if line == "P":
        return True, "ok"
    if line == "C" and "P" not in passed:
        return False, "C needs P passed (the plan comes from P's measured mean)"
    if line in ("C", "D", "M", "G1L") and (led.get("plan") or {}).get("plan") == "stop":
        return False, "P's plan is `stop`: re-probe with tightened caps first"
    if line == "D":
        if "C" not in passed:
            return False, "D needs C passed (S frozen and calibrated: FA, FR, m, n_kept; the pool cache seeded)"
        g1d = (led.get("lines", {}).get("G1D") or {}).get("pass") or ((mock_led or {}).get("lines", {}).get("G1D") or {}).get("pass")
        if mode == "real" and not g1d and not os.environ.get("M7_G1D_WAIVED"):
            return False, "D needs the $0 arm-symmetry check G1D green on the mock home (target/m7-homes/m), or M7_G1D_WAIVED set by the orchestrator"
    if line == "M" and "D" not in done:
        return False, "M runs after D"
    if line == "G1L":
        d = (led["lines"].get("D") or {}).get("accept") or {}
        if not d.get("counts_for_sticks"):
            return False, "G1L runs only after D was accepted as better or lineage_gain"
    return True, "ok"


# ============================================================================================ Root / kernel plumbing


class Ctx:
    def __init__(self, a):
        self.a, self.ih, self.rh, self.run, self.repo, self.mode = a, a.ih, a.rh, a.run, a.repo, a.mode
        self.home = a.home or os.path.expanduser("~")
        self.home_dir = os.path.dirname(a.rh)
        self.log_f = open(os.path.join(a.run, "m7.log"), "a")

    def log(self, **kw):
        kw["t"] = iso(time.time())
        self.log_f.write(json.dumps(kw, sort_keys=True) + "\n")
        self.log_f.flush()

    def root(self, *args, timeout=600):
        env = {"HOME": self.home, "INIT_ROOT_HOME": self.rh, "PATH": "/usr/bin:/bin"}
        try:
            r = subprocess.run([os.path.join(self.rh, "bin/init-root"), *args], capture_output=True, text=True, env=env, timeout=timeout)
            return r.returncode, r.stdout, r.stderr[-2000:]
        except subprocess.TimeoutExpired:
            return 124, "", "timeout"

    def root_bg(self, *args):
        env = {"HOME": self.home, "INIT_ROOT_HOME": self.rh, "PATH": "/usr/bin:/bin"}
        out = open(os.path.join(self.run, f"root-{args[0]}-{args[1] if len(args) > 1 else ''}.out"), "a")
        return subprocess.Popen([os.path.join(self.rh, "bin/init-root"), *args], stdout=out, stderr=subprocess.STDOUT, env=env)

    def initctl(self, *args, timeout=600):
        env = {"HOME": self.home, "INIT_HOME": self.ih, "PATH": "/usr/bin:/bin"}
        try:
            r = subprocess.run([os.path.join(self.ih, "kernel/bin/initctl"), "--sock", os.path.join(self.ih, "run/kernel.sock"), *args],
                               capture_output=True, text=True, env=env, timeout=timeout)
            return r.returncode, r.stdout, r.stderr[-2000:]
        except subprocess.TimeoutExpired:
            return 124, "", "timeout"

    def records(self, kinds=None, since=0):
        lin = S.lineage(self.rh)[since:]
        return [r for r in lin if kinds is None or r.get("kind") in kinds]

    def charter_apply(self, params):
        with open(os.path.join(self.rh, "charter.toml"), "w") as f:
            f.write(charter_text(params))
        os.chmod(os.path.join(self.rh, "charter.toml"), 0o600)
        rc, out, err = self.root("charter", "check", "--apply", "--yes", timeout=60)
        self.log(ev="charter_apply", rc=rc, ceiling_musd=params["ceiling_musd"], err=err[-300:] if rc else "")
        return rc == 0


TRIP_KINDS = ("canary", "tamper", "leak", "health")


def tripwire_in(records):
    for r in records:
        d = r.get("data") or {}
        st = d.get("status") or d.get("reason") or ""
        if r.get("kind") in ("sealed_g4", "gate", "milestone") and any(st == k or str(st).endswith(":" + k) for k in TRIP_KINDS[:3]):
            return st
        if r.get("kind") == "alert" and d.get("kind") in ("sealed_leak", "canary"):
            return d.get("kind")
    return None


class Runner(threading.Thread):
    """Polls the meters for one line and applies the guard; on a stop it freezes spend (charter ceilings := spent)."""

    def __init__(self, cx, cap_real, params, t0, lin0):
        super().__init__(daemon=True)
        self.cx, self.cap, self.params, self.t0, self.lin0 = cx, cap_real, params, t0, lin0
        self.stop_ev, self.done_ev = threading.Event(), threading.Event()
        self.bal0 = None
        for _ in range(3):  # the free balance endpoint is occasionally slow: retry before giving up
            self.bal0 = read_balance(cx.ih, cx.mode, self._sim)
            if self.bal0 is not None:
                break
            time.sleep(5)
        fl = os.environ.get("M7_BALANCE_FLOOR_MUSD")
        self.g = Guard(cap_real, self.bal0, floor=int(fl) if fl else None)
        self.last = {}

    def _sim(self):
        b0 = int(os.environ.get("M7_SIM_BALANCE0_MUSD", "10000000"))
        drain = int(os.environ.get("M7_SIM_DRAIN_MUSD", "0"))
        u = priced_usage(self.cx.rh, getattr(self, "t0", time.time()))
        return b0 - u["real_musd"] - drain

    def freeze(self, why):
        ok = False
        if self.params:
            p = frozen(self.params, self.cx.rh)
            ok = self.cx.charter_apply(p)
            self.cx.log(ev="freeze", why=why, reserve_musd=p["reserve_musd"], ceiling_musd=p["ceiling_musd"], applied=ok)
        if not ok:  # fallback: stop the agent zone through the owner CLI (our own test Root)
            rc, _, _ = self.cx.root("stop", timeout=60)
            self.cx.log(ev="freeze_fallback_stop", why=why, rc=rc)
        self.stop_ev.set()

    def run(self):
        last_bal = time.monotonic()  # the start balance was just read
        while not self.done_ev.is_set():
            u = priced_usage(self.cx.rh, self.t0)
            bal = "skip"
            if time.monotonic() - last_bal >= BAL_S:
                last_bal = time.monotonic()
                bal = read_balance(self.cx.ih, self.cx.mode, self._sim)
            if bal not in ("skip", None) and self.g.bal0 is None:
                self.g.bal0 = self.bal0 = bal  # the start balance arrives late: the balance checks start from here
                self.cx.log(ev="balance0_late", balance_musd=bal)
            trip = tripwire_in(self.cx.records(since=self.lin0))
            stop, act = self.g.tick(u["real_musd"], bal, trip)
            self.last = {"real_est_musd": u["real_musd"], "root_musd": u["root_musd"], "requests": u["requests"],
                         "balance_musd": bal if bal != "skip" else self.last.get("balance_musd"), "stop": stop}
            self.cx.log(ev="poll", **self.last)
            if act == "freeze":
                self.freeze(stop)
                break
            if act == "tighten" and self.params:
                # the balance is unreadable: the Root ceilings alone must now bound the rest, at peak prices (factor 1)
                left = max(0, self.cap - u["real_musd"])
                share = SEALED_SHARE.get(self.params["line"], 800)
                f = frozen(self.params, self.cx.rh)
                reserve = f["reserve_musd"] + cent_up(left * share / 1000) + PARALLEL * RESERVATION_MUSD
                zone = (f["ceiling_musd"] - f["reserve_musd"]) + cent_up(left * (1000 - share) / 1000) + PARALLEL * RESERVATION_MUSD
                p = dict(self.params, reserve_musd=cent_up(reserve), ceiling_musd=cent_up(reserve) + cent_up(zone))
                self.cx.charter_apply(p)
                self.cx.log(ev="tighten", reserve_musd=p["reserve_musd"], ceiling_musd=p["ceiling_musd"])
            self.done_ev.wait(POLL_S)


# ============================================================================================ line drivers


def wait_records(cx, kinds, since, timeout, stop_ev):
    end = time.monotonic() + timeout
    while time.monotonic() < end and not stop_ev.is_set():
        r = cx.records(kinds, since)
        if r:
            return r
        time.sleep(5)
    return cx.records(kinds, since)


def root_job(cx, args, timeout, stop_ev):
    """A long owner CLI command through the running Root (our own child, stopped by its handle)."""
    p = cx.root_bg(*args)
    end = time.monotonic() + timeout
    while p.poll() is None and time.monotonic() < end:
        if stop_ev.is_set():
            try:
                p.wait(timeout=600)  # the freeze makes the Root refuse requests: the job ends as partial
            except subprocess.TimeoutExpired:
                p.terminate()
                p.wait(timeout=10)
            break
        time.sleep(2)
    if p.poll() is None:
        p.terminate()
        p.wait(timeout=10)
    return p.returncode


def owner(cx):
    """The owner driver (S-REAL's attach child), after quiescing: a run the body resumed from an interrupted earlier
    line (pending input at boot) is interrupted first, so it can neither spend nor take the next scripted turn."""
    o = S.Owner(cx.a)
    o.drain(3)
    end = time.monotonic() + 90
    if o.busy:
        cx.log(ev="quiesce", note="a resumed run was busy at attach; interrupting it")
        import signal as _sig
        o.p.send_signal(_sig.SIGINT)  # our own attach child: it sends `interrupt`
        while o.busy and time.monotonic() < end:
            o.drain(1)
    return o


CAL_LINE = ("Run this exact command with the bash tool and a timeout of 600 seconds: `initctl evo calibrate --repeats {r} --limit {n} "
            "--no-wait`. It starts a kernel job that measures the visible pool for a harness; do nothing else. Reply with its "
            "output only.")


def jobs(ih, cmd):
    out = {}
    for f in glob.glob(os.path.join(ih, "state/kernel/jobs/j-*.json")):
        r = jload(f, {}) or {}
        if r.get("cmd") == cmd:
            out[r.get("id")] = r
    return out


def promote_outcome(cx, before):
    """The promote job the owner turn started (`state/kernel/jobs`): None when the validator refused before any job
    (the agent saw an initctl error) or the job failed; else the job record. Lets a driver stop at once instead of
    waiting hours for a boot and a G4 that will never come."""
    new = {k: v for k, v in jobs(cx.ih, "promote").items() if k not in before}
    job = max(new.values(), key=lambda j: j.get("id") or "") if new else None
    if not job or job.get("state") in ("failed", "cancelled"):
        return None, job
    return job, job


def calib_jobs(ih):
    """The kernel's pool-calibration job records (`state/kernel/jobs/j-*.json`, cmd evo.screen with pool_repeats)."""
    out = {}
    for f in glob.glob(os.path.join(ih, "state/kernel/jobs/j-*.json")):
        r = jload(f, {}) or {}
        if r.get("cmd") == "evo.screen" and (r.get("args") or {}).get("pool_repeats"):
            out[r.get("id")] = r
    return out


def pool_calibrate(cx, n, r, stop_ev, timeout=4 * 3600):
    """The kernel's pool calibration (`initctl evo calibrate`, REAL-M7 087b5e5): the LKG on the visible pool, r runs
    per task (n tasks, 0 = all), seeding the pool cache and the miner's failing runs. The kernel socket serves only
    the zone (E_PEER for the harness), so the live agent starts the fixed command (as S-REAL's promote checks did);
    the harness then waits on the kernel's job record and reads the result from it, not from the model's reply."""
    before = set(calib_jobs(cx.ih))
    o = owner(cx)
    try:
        S.new_session(o, cx.ih)
        res = o.task(CAL_LINE.format(r=r, n=n), 900, cx.root_cap)
    finally:
        o.drain(2)
        o.close()
    end, job = time.monotonic() + timeout, None
    while time.monotonic() < end and not stop_ev.is_set():
        new = {k: v for k, v in calib_jobs(cx.ih).items() if k not in before}
        job = max(new.values(), key=lambda j: j.get("id") or "") if new else None
        if job and job.get("finished_at"):
            break
        if not job and time.monotonic() > end - timeout + 120:
            break  # the agent did not start a job within 2 minutes
        time.sleep(5)
    jr = (job or {}).get("result") or {}
    out = {"job": (job or {}).get("id"), "state": (job or {}).get("state"), "runs": int(jr.get("runs") or 0), "tasks": int(jr.get("tasks") or 0),
           "failed": int(jr.get("failed") or 0), "requests": int(jr.get("requests") or 0), "spent_musd": jr.get("spent_musd"),
           "turn_rc": res.get("rc"), "model_turns": res.get("model_turns")}
    cx.log(ev="pool_calibrate", **out)
    return (0 if out["runs"] and out["requests"] else 1), out


def sealed_op_args(cx):
    """--usd (the operation's Root-metered budget) and --real outside a test-mode Root."""
    a = ["--usd", usd(int(cx.params.get("sealed_op_musd") or 0))]
    return a + (["--real"] if cx.mode == "real" else [])


def line_P(cx, st, stop_ev):
    if not cx.records(["sealed_draw"]):
        # ROOT-S1a: the draw installs the pack read-only into ROOT_HOME (`sealed draw --pack-dir`); the harness passes
        # the directory and never reads it (real: pack s1; mock: ROOT-S1a's fixture pack)
        pack = cx.a.pack_dir or ""
        if not pack or not os.path.isfile(os.path.join(pack, "pack.json")):
            raise SystemExit(f"no S pack to draw from (--pack-dir {pack!r})")
        rc, out, err = cx.root("sealed", "draw", "--pack-dir", pack, timeout=3600)
        st["draw_rc"], st["draw_err"] = rc, (err[-300:] if rc else "")
    d = cx.records(["sealed_draw"])
    st["sealed_draw"] = (d[-1].get("data") if d else None)
    st["sealed_draw_seq"] = d[-1].get("seq") if d else None
    n0 = len(S.lineage(cx.rh))
    st["probe_rc"] = root_job(cx, ["sealed", "probe", "--per-family", str(P_PROBE_PER_GROUP)] + sealed_op_args(cx), 3600, stop_ev)
    rec = cx.records(["sealed_calibrate"], n0)
    st["probe_record"] = rec[-1].get("data") if rec else None
    if not stop_ev.is_set():
        st["pool_rc"], st["pool_result"] = pool_calibrate(cx, P_POOL, 1, stop_ev)


def line_C(cx, st, stop_ev, led):
    c = (led.get("plan") or {}).get("c") or PLANS["full"]["c"]
    n0 = len(S.lineage(cx.rh))
    args = ["sealed", "calibrate", "--repeats", str(c["r"])] + sealed_op_args(cx)
    st["args"] = args
    st["calibrate_rc"] = root_job(cx, args, 4 * 3600, stop_ev)
    rec = cx.records(["sealed_calibrate"], n0)
    st["calibrate_record"] = rec[-1].get("data") if rec else None
    st["sealed_calibrate_seq"] = rec[-1].get("seq") if rec else None
    if not stop_ev.is_set():
        st["pool_rc"], st["pool_result"] = pool_calibrate(cx, 0, c["pool_r"], stop_ev)


D3_LINE = ("Evolution task (REAL-M7 D-3). Follow `skills/evolve/SKILL.md` for friction {fid} ({sig}): read it with `initctl evo "
           "frictions --json --all` and its evidence refs, then file ONE proposal for it with `initctl evo propose`, with exactly "
           "this predicts list: [\"pass_rate@sealed >= parent+15%\", \"cost_usd <= parent+30%\"] and `\"tier_hint\":\"t2\"`. "
           "Submit up to {k} genuinely different variants (`initctl evo variant`), then run `initctl evo screen <proposal> --wait` "
           "(bash timeout 3600 s). KEEP_BASE is a valid outcome. Do NOT promote anything. Reply with the proposal id, each "
           "variant's screen result, est and go, in at most six lines.")
D6_LINE = ("Evolution task (REAL-M7 D-6). Promote proposal {pid}'s chosen variant {k}, as `skills/evolve/SKILL.md` §5 says: "
           "`git -C \"$INIT_HOME/body\" fetch --quiet origin && git -C \"$INIT_HOME/body\" branch -f evo/{pid} "
           "origin/init/evo/{pid}/v{k}`, then `cd \"$INIT_HOME/body\" && initctl promote evo/{pid} --wait` (bash timeout 3600 s). "
           "Reply with the promote result in one line.")


def frictions(cx):
    return S.jl(os.path.join(cx.ih, "state/kernel/evo/frictions.jsonl"))


def proposal(cx, pid):
    return jload(os.path.join(cx.ih, "state/kernel/evo/proposals", f"{pid}.json"), None)


def sreal_census(repo):
    """S-REAL's 6 sessions as supplementary occurrence counts per signature (ties only; evolution-v2 D-1). A $0
    scan of the committed redacted artifacts: a tool result starting `error: <code>` → `tool.error:<tool>:<code>`."""
    cnt = {}
    for p in sorted(set(glob.glob(os.path.join(repo, "build/real-artifacts/real/*/init/sessions/*.jsonl")))):
        for e in S.jl(p):
            if e.get("type") == "tool_result" and isinstance(e.get("content"), str) and e["content"].startswith("error: "):
                code = e["content"][7:].split(":", 1)[0].strip()
                sig = f"tool.error:{e.get('tool') or e.get('name') or '?'}:{code}"
                cnt[sig] = cnt.get(sig, 0) + 1
    return cnt


def variant_paths(cx, ref):
    try:
        out = S.git(os.path.join(cx.ih, "lineage/body.git"), "diff", "--name-only", "main", ref, check=False)
        return [x for x in out.splitlines() if x.strip()]
    except Exception:
        return []


def line_D(cx, st, stop_ev, led):
    plan = led.get("plan") or {}
    pre = led.get("prereg")
    if not pre:
        raise SystemExit("D-0 missing: run `checks_m7.py prereg` (m7.sh does) before any friction is read")
    st["prereg_sha256"] = pre.get("sha256")
    rn = getattr(cx, "runner", None)
    full_cap = rn.g.cap if rn else None
    if rn:
        rn.g.cap = min(full_cap, D0["r_cap_musd"])  # D-0 item 11: mining, the worker and the screens stay under line R's cap
    o = owner(cx)
    resume = os.environ.get("M7_D_RESUME")
    if resume:
        try:
            return line_D_resume(cx, st, stop_ev, o, resume, os.environ.get("M7_D_PHASE", "screen"), rn, full_cap)
        finally:
            o.drain(2)
            o.close()
    try:
        st["new_session"] = S.new_session(o, cx.ih)
        census = sreal_census(cx.repo)
        tried = []
        for attempt in range(2):                     # cluster #1, then cluster #2 once if line R's cap allows
            fr = frictions(cx)
            st.setdefault("census", {"clusters": len(fr), "eligible": sum(1 for f in fr if f.get("eligible"))})
            f = select_cluster(fr, census, exclude=tried)
            forced = cx.mode == "mock" and os.environ.get("M7_DRY_FORCE", "1") == "1"
            if not f and forced and fr:  # MOCK ONLY: reach G4 even when the mock's census has no qualifying cluster
                f, st["forced_cluster"] = fr[0], True
            if not f:
                st["stop"] = "no_proposal" if st.get("no_proposal") else "no_eligible_friction"
                return
            tried.append(f["id"])
            st[f"cluster{attempt + 1}"] = {k: f.get(k) for k in ("id", "signature", "s_c_permille", "counts", "families", "labels")}
            r_before = priced_usage(cx.rh, cx.t0)["real_musd"]
            if r_before >= D0["r_cap_musd"] or stop_ev.is_set():
                st["stop"] = "r_cap"
                return
            res = o.task(D3_LINE.format(fid=f["id"], sig=f.get("signature"), k=D0["K"]), 3600, cx.root_cap)
            st[f"worker{attempt + 1}"] = {k: res.get(k) for k in ("rc", "model_turns", "wall_ms", "spend_cap_hit")}
            m = re.search(r"\bp-\d{6}\b", res.get("reply") or "")
            pid = m.group(0) if m else None
            pf = os.path.join(cx.ih, "work/m7dry/p.id")
            if cx.mode == "mock" and os.path.exists(pf):
                pid = open(pf).read().strip() or pid
            pr = wait_proposal(cx, pid, 1800, stop_ev) if pid else None
            st[f"proposal{attempt + 1}"] = {"id": pid, "state": (pr or {}).get("state"), "est": (pr or {}).get("est")}
            if not pr:
                st["no_proposal"] = st.get("no_proposal", 0) + 1  # the worker filed none (KEEP_BASE, a refusal, or a cap)
                continue
            tier = cx.params.get("tier")
            st["tier"], st["cap_d_musd"] = tier, (cx.params.get("budget") or {}).get("cap_d_musd")
            est = int(pr.get("est") or 0)
            go, aim = go_no_go(est, tier or "lean")
            if not go and forced:  # MOCK ONLY, labelled in the verdict
                go, aim, st["forced_go"] = True, "better", True
            st[f"go{attempt + 1}"] = {"est_permille": est, "go": go and tier is not None, "aim": aim}
            if not (go and tier):
                mean = int(plan.get("mean_musd_per_run") or PLAN_MUSD_PER_RUN)
                if priced_usage(cx.rh, cx.t0)["real_musd"] + FIXED["worker"] + D0["K"] * 6 * mean > D0["r_cap_musd"]:
                    st["stop"] = "below_resolution" if tier else "d_does_not_fit"
                    return
                continue
            # the kernel's screen chose the variant (E3 ranking); D-0 item 8: it must be T2
            ch = pr.get("chosen") or {}
            k = int(ch.get("k") or 0)
            var = next((v for v in (pr.get("variants") or []) if int(v.get("k") or 0) == k), {})
            paths = var.get("paths") or variant_paths(cx, ch.get("sha") or var.get("ref") or "HEAD")
            vt = tier_of_paths(paths)
            st["variant"] = {"k": k, "sha": ch.get("sha"), "aim_kernel": ch.get("aim"), "s_measurable": ch.get("s_measurable"),
                             "tier": vt, "paths_n": len(paths)}
            if not k:
                st["stop"] = "no_chosen_variant"
                return
            if vt != "t2" or not ch.get("s_measurable"):
                st["stop"] = f"variant_not_t2:{vt}"   # orchestrator rule: S slices measure T2 only
                return
            pred = "; ".join(pr.get("predicts") or [])   # the kernel writes the commit message from the proposal
            st["predicts"] = pred
            st["predicts_ok"] = predicts_ok(pred) and bool(re.search(r"pass_rate@sealed >= parent\+([1-9][0-9]*)%", pred))
            if not st["predicts_ok"]:
                st["stop"] = "predicts_not_preregistered"
                return
            n0 = len(S.lineage(cx.rh))
            nev = len(S.all_session_events(cx.ih))
            before = set(jobs(cx.ih, "promote"))
            if rn:
                rn.g.cap = full_cap  # D-6 onwards: the line's whole cap (G0-G3 and the G4)
            st["promote"] = {k2: v for k2, v in o.task(D6_LINE.format(pid=pid, k=k), 5400, cx.root_cap).items() if k2 != "reply"}
            okp, job = promote_outcome(cx, before)
            st["promote_job"] = {k2: (job or {}).get(k2) for k2 in ("id", "state", "error")}
            if not okp:
                st["stop"] = "promote_refused"
                return
            b = S.wait_boot(cx.ih, "promote", nev, 600)
            st["boot_after"] = {k2: b.get(k2) for k2 in ("sha", "reason", "prefix_hash")} if b else None
            g = wait_g4_v2(cx, n0, 4 * 3600, stop_ev)
            st["g4"] = g
            st["accept"] = accept_class((g or {}).get("gate"))
            return
        st["stop"] = st.get("stop") or "no_go"
    finally:
        o.drain(2)
        o.close()


SCREEN_LINE = ("Run this exact command with the bash tool and a timeout of 3600 seconds: `initctl evo screen {pid} --wait`. It screens "
               "the variants of your proposal {pid}. Do nothing else. Reply with its last output lines only.")


def line_D_resume(cx, st, stop_ev, o, pid, phase, rn, full_cap):
    """D continued on the orchestrator's decision after DV-1/DV-2 (measurements/m7-results.md), under the frozen D-0.
    phase `screen`: the agent re-runs `initctl evo screen <pid>` (E0 + E3), then est and go/no-go are recorded.
    phase `promote`: the screened proposal's chosen variant (T2 check) is promoted: G0-G3, handoff, G4 v2 at the
    pinned tier. The Root stops between the phases, so m7.sh clears the build slot.
    phase `g4retry` (DV-5): as `promote`, after rolling back a still-provisional chosen sha whose G4 never finished."""
    st["resume"] = {"proposal": pid, "phase": phase}
    S.new_session(o, cx.ih)
    pr = proposal(cx, pid)
    if not pr:
        st["stop"] = "no_such_proposal"
        return
    st["cluster1"] = {"id": (pr.get("frictions") or [None])[0]}
    tier = cx.params.get("tier")
    st["tier"], st["cap_d_musd"] = tier, (cx.params.get("budget") or {}).get("cap_d_musd")
    if phase == "screen":
        if pr.get("state") != "variants":
            st["stop"] = f"proposal_state:{pr.get('state')}"
            return
        st["screen_turn"] = {k: v for k, v in o.task(SCREEN_LINE.format(pid=pid), 3900, cx.root_cap).items() if k != "reply"}
        pr = wait_proposal(cx, pid, 3600, stop_ev)
    pr = pr or {}
    est = int(pr.get("est") or 0)
    go, aim = go_no_go(est, tier or "lean")
    st["proposal1"] = {"id": pid, "state": pr.get("state"), "est": pr.get("est"), "go_kernel": pr.get("go")}
    st["go1"] = {"est_permille": est, "go": go and tier is not None, "aim": aim}
    st["screens"] = [{"k": v.get("k"), "screen": v.get("screen")} for v in (pr.get("variants") or [])]
    if not (go and tier):
        # D-0 item 5: cluster #2 only within line R's cap, which DV-1 already exhausted
        st["stop"] = "no_go:below_resolution (no friction the gate can resolve; line R's cap is spent, so no cluster #2)"
        return
    if phase == "screen":
        st["stop"] = "screened_go"  # not a verdict: the promote phase follows with a cleared build slot
        return
    ch = pr.get("chosen") or {}
    k = int(ch.get("k") or 0)
    var = next((v for v in (pr.get("variants") or []) if int(v.get("k") or 0) == k), {})
    paths = var.get("paths") or variant_paths(cx, ch.get("sha") or "HEAD")
    vt = tier_of_paths(paths)
    st["variant"] = {"k": k, "sha": ch.get("sha"), "aim_kernel": ch.get("aim"), "s_measurable": ch.get("s_measurable"), "tier": vt,
                     "paths": paths}
    if not k:
        st["stop"] = "no_chosen_variant"
        return
    if vt != "t2" or not ch.get("s_measurable"):
        st["stop"] = f"variant_not_t2:{vt}"
        return
    pred = "; ".join(pr.get("predicts") or [])
    st["predicts"] = pred
    st["predicts_ok"] = predicts_ok(pred) and bool(re.search(r"pass_rate@sealed >= parent\+([1-9][0-9]*)%", pred))
    if not st["predicts_ok"]:
        st["stop"] = "predicts_not_preregistered"
        return
    if phase == "g4retry":
        why = dv5_rollback(cx, st, o, ch.get("sha"))
        if why:
            st["stop"] = why
            return
    n0 = len(S.lineage(cx.rh))
    nev = len(S.all_session_events(cx.ih))
    before = set(jobs(cx.ih, "promote"))
    if rn:
        rn.g.cap = full_cap
    st["promote"] = {k2: v for k2, v in o.task(D6_LINE.format(pid=pid, k=k), 5400, cx.root_cap).items() if k2 != "reply"}
    okp, job = promote_outcome(cx, before)
    st["promote_job"] = {k2: (job or {}).get(k2) for k2 in ("id", "state", "error")}
    if not okp:
        st["stop"] = "promote_refused"
        return
    b = S.wait_boot(cx.ih, "promote", nev, 900)
    st["boot_after"] = {k2: b.get(k2) for k2 in ("sha", "reason", "prefix_hash")} if b else None
    g = wait_g4_v2(cx, n0, 4 * 3600, stop_ev)
    st["g4"] = g
    st["accept"] = accept_class((g or {}).get("gate"))
    if (g or {}).get("g0_g3_reject"):
        st["stop"] = f"g0_g3_reject:{g['g0_g3_reject'].get('reason')}"


ROLLBACK_LINE = ("Run this exact command with the bash tool and a timeout of 600 seconds: "
                 "`initctl rollback --reason \"DV-5 rerun of the single G4 that the disk stop interrupted\" --wait`. "
                 "It is an operator step of a measurement. Reply in one line.")


def supervisor_current(ih):
    sv = jload(os.path.join(ih, "state/kernel/supervisor.json")) or {}
    cur = sv.get("current") or {}
    lkg = ((sv.get("binaries") or {}).get("lkg") or {}).get("built_from")
    return cur.get("sha"), bool(cur.get("provisional")), lkg


def dv5_decision(cur, provisional, lkg, chosen):
    """DV-5 (m7-results.md): D's single G4 re-runs on the SAME chosen sha. The kernel has no verb that re-runs G4 on the
    current provisional (an interrupted job ends as `kernel_restart`; the provisional only expires after 24 h), so a
    still-provisional chosen sha is rolled back first and then promoted again (G0-G3, handoff, G4). -> 'rollback',
    'promote' (already at the LKG), or a stop reason."""
    if not chosen or not lkg:
        return "dv5_unknown_state"
    if cur == chosen and provisional:
        return "rollback"
    if cur == lkg and not provisional:
        return "promote"
    if cur == chosen and not provisional:
        return "dv5_chosen_already_lkg"
    return "dv5_unexpected_current"


def dv5_rollback(cx, st, o, chosen):
    cur, prov, lkg = supervisor_current(cx.ih)
    act = dv5_decision(cur, prov, lkg, chosen)
    st["dv5"] = {"before": {"current": cur, "provisional": prov, "lkg": lkg}, "action": act}
    if act not in ("rollback", "promote"):
        return act
    if act == "rollback":
        st["dv5"]["turn"] = {k2: v for k2, v in o.task(ROLLBACK_LINE, 900, cx.root_cap).items() if k2 != "reply"}
        end = time.monotonic() + 600
        while time.monotonic() < end:
            cur, prov, _ = supervisor_current(cx.ih)
            if cur == lkg and not prov:
                break
            time.sleep(5)
        st["dv5"]["after"] = {"current": cur, "provisional": prov}
        if not (cur == lkg and not prov):
            return "dv5_rollback_failed"
        S.new_session(o, cx.ih)  # the promote turn starts on the LKG body with a fresh session
    return None


def dv5_reopen(path, sha, now_iso, why=None):
    """DV-5b (orchestrator's decision): the proposal file of a chosen variant whose single G4 never produced a verdict
    (its job ended `kernel_restart`) goes back from `promoted` to `screened`, so the validator takes the SAME chosen sha
    again (`proposal.state`). Only with the Root stopped; refuses unless state == promoted and chosen.sha == sha. The
    history gains one entry naming the harness and the reason. Returns the new record or raises ValueError."""
    p = json.load(open(path))
    if p.get("state") != "promoted" or (p.get("chosen") or {}).get("sha") != sha:
        raise ValueError(f"refused: state {p.get('state')!r}, chosen {((p.get('chosen') or {}).get('sha') or '')[:12]!r}")
    p["state"] = "screened"
    p.setdefault("history", []).append({"note": {"by": "harness:REAL-M7", "reason": why or ("DV-5b: G4 job j-000007 ended kernel_restart without a "
                                                 "verdict; the same chosen sha is promoted again for D-0's single G4"), "sha": sha},
                                        "state": "screened", "ts": now_iso})
    tmp = path + ".dv5.tmp"
    with open(tmp, "w") as f:
        f.write(json.dumps(p, indent=2) + "\n")
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)
    return p


def wait_proposal(cx, pid, timeout, stop_ev):
    """The proposal record once its screen is over (state screened/composed/promoted/verdict or abandoned:*)."""
    end = time.monotonic() + timeout
    pr = None
    while time.monotonic() < end and not stop_ev.is_set():
        pr = proposal(cx, pid)
        stt = str((pr or {}).get("state") or "")
        if stt in ("screened", "banked", "composed", "promoted", "verdict") or stt.startswith("abandoned"):
            return pr
        time.sleep(10)
    return pr


def wait_g4_v2(cx, n0, timeout, stop_ev):
    end = time.monotonic() + timeout
    while time.monotonic() < end and not stop_ev.is_set():
        new = cx.records(None, n0)
        gates = [r for r in new if r.get("kind") == "gate" and (r.get("data") or {}).get("gate_kind") == "g4"]
        # a promote gate that rejected (G0-G3, e.g. G3 `timeout`): no G4 will come (REAL-M7 retry 2)
        pre = [r for r in new if r.get("kind") == "gate" and (r.get("data") or {}).get("gate_kind") == "promote"
               and (r.get("data") or {}).get("verdict") != "accept"]
        if pre and not gates:
            d = pre[-1].get("data") or {}
            return {"gate": None, "g0_g3_reject": {k: d.get(k) for k in ("verdict", "reason", "sha", "spent_musd")}, "sealed_g4": None,
                    "after": [r.get("kind") for r in new][-12:]}
        if gates or any(r.get("kind") in ("rollback", "demotion") for r in new):
            time.sleep(15)
            new = cx.records(None, n0)
            sg = [r.get("data") for r in new if r.get("kind") == "sealed_g4"]
            gd = gates[-1].get("data") if gates else None
            keep = ("verdict", "reason", "accept_class", "class", "tier", "sha", "gate_id", "milestone_required", "sealed")
            return {"gate": {k: gd.get(k) for k in keep} if gd else None,
                    "sealed_g4": {k: (sg[-1] or {}).get(k) for k in ("status", "tier", "n", "repeats", "d_permille", "m_permille",
                                                                      "cpp_ratio_permille", "looks_left", "dup", "ratchet_signal")} if sg else None,
                    "after": [r.get("kind") for r in new][-12:]}
        time.sleep(10)
    return {"timeout": not stop_ev.is_set(), "stopped": stop_ev.is_set()}


def line_M(cx, st, stop_ev, led):
    n0 = len(S.lineage(cx.rh))
    m_r = int((led.get("plan") or {}).get("m_r") or 3)
    st["milestone_rc"] = root_job(cx, ["sealed", "milestone", "--repeats", str(m_r)] + sealed_op_args(cx), 3 * 3600, stop_ev)
    m = cx.records(["milestone"], n0)
    st["milestone"] = m[-1].get("data") if m else None
    out = ""
    for f in glob.glob(os.path.join(cx.run, "root-sealed-milestone*.out")):
        out += open(f, errors="replace").read()
    if not m and "nothing to check" in out:
        st["milestone_skipped"] = "current is the confirmed tuple (nothing was promoted): no reserve is spent"
    if stop_ev.is_set():
        return
    # run-down: one owner turn on the current LKG, then an idle watch with no rollback, demotion or crash loop
    o = owner(cx)
    try:
        S.new_session(o, cx.ih)
        st["rundown_turn"] = {k: v for k, v in o.task("Reply with the single word ok.", 300, cx.root_cap).items() if k != "reply"}
        o.drain(int(os.environ.get("M7_RUNDOWN_S", "600")))
    finally:
        o.close()
    st["rundown_records"] = sorted({r.get("kind") for r in cx.records(None, n0)})
    st["rundown_ok"] = not any(k in st["rundown_records"] for k in ("rollback", "demotion", "safe_mode"))


NOP_G1 = """refactor(loop): REAL-M7 null change for the real A/A (G1L)

A comment-only change: both arms of the G4 run the same behaviour, so the
slice must show no gain. It checks the pipeline's arm symmetry on the real
model before the demo's accept is claimed.

Why: real A/A (m7-design 3.2.10) as a lean, conditional line (runbook 5.6).
Predicts: pass_rate >= parent
Compensates-for: none (measurement)
Prefix-change: no
"""


def line_G1(cx, st, stop_ev, led):
    """G1L (real, lean tier) and G1D (mock): the harness-committed comment-only T2 change (made by `prepare` before
    the Root started) through the full pipeline."""
    st["null_commit"] = jload(os.path.join(cx.run, "nop-commit.json"))
    if not st["null_commit"]:
        raise SystemExit("no null commit: `checks_m7.py prepare` must run before the Root starts")
    n0 = len(S.lineage(cx.rh))
    nev = len(S.all_session_events(cx.ih))
    before = set(jobs(cx.ih, "promote"))
    o = owner(cx)
    try:
        st["promote"] = {k: v for k, v in o.task(
            "Run this exact command with the bash tool and a timeout of 3600 seconds: `cd \"$INIT_HOME/body\" && initctl promote "
            "evo/m7-null --wait`. It promotes a comment-only change a harness committed for a measurement. Reply in one line.",
            5400, cx.root_cap).items() if k != "reply"}
        ok, job = promote_outcome(cx, before)
        st["promote_job"] = {k: (job or {}).get(k) for k in ("id", "state", "error")}
        if not ok:
            st["stop"] = "promote_refused"
            return
        b = S.wait_boot(cx.ih, "promote", nev, 600)
        st["boot_after"] = {k: b.get(k) for k in ("sha", "reason")} if b else None
        st["g4"] = wait_g4_v2(cx, n0, 3 * 3600, stop_ev)
    finally:
        o.drain(2)
        o.close()


def prepare_null_commit(a):
    """As S-REAL's prepare_nop_commit (tests/real/checks.py): a comment-only change on evo/m7-null in body/, with
    self.json refreshed by `init-kernel selfmap --write` (a ~1 GB body build in body/target, removed afterwards)."""
    body = os.path.join(a.ih, "body")
    S.git(body, "checkout", "-q", "-f", "-B", "evo/m7-null", "origin/main")
    with open(os.path.join(body, "src/loop/mod.rs"), "a") as f:
        f.write("\n// REAL-M7 G1: a comment-only change (A/A); no behaviour or prompt byte changes.\n")
    selfmap_write(a, body)
    S.git(body, "add", "-A")
    mf = os.path.join(a.run, "nop-msg.txt")
    open(mf, "w").write(NOP_G1)
    S.git(body, "commit", "-q", "-F", mf)
    S.wjson(os.path.join(a.run, "nop-commit.json"), {"sha": S.git(body, "rev-parse", "HEAD"), "parent": S.git(body, "rev-parse", "HEAD^")})


def g1_pass(g4, exact):
    """G1L passes iff the slice is not superior/inferior/tripwire and |d| ≤ 150‰ (the smallest effect a `better`
    accept at standard rests on); G1D (mock, deterministic) needs d = 0 exactly and a non-accept verdict."""
    sg = (g4 or {}).get("sealed_g4") or {}
    gate = (g4 or {}).get("gate") or {}
    st, d = sg.get("status"), sg.get("d_permille")
    if st is None or d is None:
        return False, "no sealed_g4 record"
    if st in ("superior", "inferior", "inferior_early", "canary", "tamper", "leak", "health") or str(st).startswith("error"):
        return False, f"status {st}"
    if exact:
        return (d == 0 and gate.get("verdict") != "accept"), f"d={d} verdict={gate.get('verdict')}"
    return abs(int(d)) <= 150, f"d={d}"


DRIVERS = {"P": line_P, "C": line_C, "D": line_D, "M": line_M, "G1L": line_G1, "G1D": line_G1}


PROPOSE_SH = r"""#!/bin/sh
# REAL-M7 dry run (MOCK ONLY): a canned proposal, so the D line's plumbing runs end to end at $0. The real line's
# proposal is written by the agent itself (D-3); nothing here is used in real mode.
set -e
cd "$INIT_HOME/work/m7dry"
python3 - "$INIT_HOME/state/kernel/evo/frictions.jsonl" <<'PYX'
import json, sys
fr = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
f = next((x for x in fr if x.get("eligible")), fr[0] if fr else {"id": "f-000001", "refs": []})
p = {"v": 1, "scope": "inv", "frictions": [f["id"]], "hypothesis": "m7 dry run", "change": "m7 dry run: a comment-only T2 change",
     "repro": [r for r in (f.get("refs") or []) if r.startswith("run:")][:2] or (f.get("refs") or [])[:2],
     "predicts": ["pass_rate@sealed >= parent+15%", "cost_usd <= parent+30%"], "subject": "refactor(loop): m7 dry run comment",
     "compensates_for": "none", "prefix_change": False, "lessons": [], "k": 1, "tier_hint": "t2"}
open("proposal.json", "w").write(json.dumps(p))
PYX
initctl evo propose "$INIT_HOME/work/m7dry/proposal.json" > propose.out 2>&1 || true
python3 -c 'import re,sys; m=re.search(r"p-[0-9]{6}", open("propose.out").read()); print(m.group(0) if m else "")' > p.id
test -s p.id || { cat propose.out; exit 1; }
initctl evo variant "$(cat p.id)" 1 "$INIT_HOME/work/m7dry/v1.patch"
initctl evo screen "$(cat p.id)" --wait
"""
PROMOTE_SH = r"""#!/bin/sh
# MOCK ONLY: the chosen variant, promoted as skills/evolve/SKILL.md §5 says
set -e
p="$(cat "$INIT_HOME/work/m7dry/p.id")"
k="$(initctl evo status "$p" | python3 -c 'import json,re,sys; t=sys.stdin.read(); m=re.search(r"\"chosen\":\s*\{[^}]*\"k\":\s*([0-9]+)", t); print(m.group(1) if m else 1)')"
git -C "$INIT_HOME/body" fetch --quiet origin
git -C "$INIT_HOME/body" branch -f "evo/$p" "origin/init/evo/$p/v$k"
cd "$INIT_HOME/body" && initctl promote "evo/$p" --wait
"""


def selfmap_write(a, wt):
    tc = os.path.realpath(os.path.expanduser("~/.rustup/toolchains/stable-aarch64-apple-darwin"))
    ch = os.path.join(a.run, "cargo-home")
    os.makedirs(ch, exist_ok=True)
    r = subprocess.run([os.path.join(a.ih, "kernel/bin/init-kernel"), "selfmap", "--root", wt, "--write"], cwd=wt,
                       capture_output=True, text=True,
                       env={"PATH": f"{tc}/bin:/usr/bin:/bin", "HOME": ch, "CARGO_HOME": ch, "CARGO_NET_OFFLINE": "true"})
    S.rm_exact(os.path.join(wt, "target"))  # selfmap's body build (~1 GB), unused afterwards
    S.rm_exact(ch)
    if r.returncode != 0:
        raise SystemExit(f"selfmap: {r.stderr[-500:]}")


def prepare_variant_patch(a, d):
    """MOCK ONLY: a comment-only T2 variant as `git diff --cached origin/main` with self.json refreshed, made in a
    throwaway branch of body/ before the Root starts (the real line's variants are the agent's own)."""
    body = os.path.join(a.ih, "body")
    orig = S.git(body, "rev-parse", "--abbrev-ref", "HEAD", check=False) or "main"
    S.git(body, "checkout", "-q", "-f", "-B", "m7-dry-variant", "origin/main")
    with open(os.path.join(body, "src/loop/mod.rs"), "a") as f:
        f.write("\n// REAL-M7 dry run: a comment-only T2 variant (mock only).\n")
    selfmap_write(a, body)
    S.git(body, "add", "-A")
    patch = S.git(body, "diff", "--cached", "origin/main")
    with open(os.path.join(d, "v1.patch"), "w") as f:
        f.write(patch + "\n")
    S.git(body, "reset", "-q", "--hard", "origin/main")
    S.git(body, "checkout", "-q", "-f", orig, check=False)
    S.git(body, "branch", "-q", "-D", "m7-dry-variant", check=False)


def prepare(a):
    """Before the Root starts ($0, local): the null commit for G1L/G1D; the dry run's canned D files (mock only)."""
    if a.line in ("G1L", "G1D"):
        prepare_null_commit(a)
        print(json.dumps({"prepared": ["evo/m7-null"]}))
        return 0
    if a.mode != "mock" or a.line != "D":
        print(json.dumps({"prepared": []}))
        return 0
    d = os.path.join(a.ih, "work/m7dry")
    os.makedirs(d, exist_ok=True)
    prepare_variant_patch(a, d)
    for name, body in (("propose.sh", PROPOSE_SH), ("promote.sh", PROMOTE_SH)):
        with open(os.path.join(d, name), "w") as f:
            f.write(body)
    print(json.dumps({"prepared": sorted(os.listdir(d))}))
    return 0


def run_line(a):
    cx = Ctx(a)
    led = ledger_load(cx.home_dir)
    mock_led = ledger_load(os.path.join(os.path.dirname(cx.home_dir), "m")) if a.mode == "real" else None
    ok, why = may_start(led, a.line, a.mode, mock_led)
    params = jload(os.path.join(cx.home_dir, "charter.params.json"))
    if ok and (not params or params.get("line") != a.line or int(params.get("cap_real_musd") or 0) <= 0):
        ok, why = False, f"no budget for {a.line}: {((params or {}).get('budget') or {}).get('why', 'params missing or for another line')}"
    if not ok:
        print(json.dumps({"rc": 5, "refused": why}))
        return 5
    cap = int(params["cap_real_musd"])
    cx.params = params
    cx.t0 = time.time()
    cx.root_cap = int(params["zone_cap_musd"])
    lin0 = len(S.lineage(cx.rh))
    st = {"line": a.line, "mode": a.mode, "cap_real_musd": cap, "tier": params.get("tier"), "started": iso(cx.t0),
          "offpeak": offpeak_advice(cx.t0, a.wall_s), "ceilings": {k: params.get(k) for k in ("reserve_musd", "ceiling_musd", "factor")}}
    rn = Runner(cx, cap, params, cx.t0, lin0)
    st["balance_before_musd"] = rn.bal0
    cx.runner = rn
    S.wjson(os.path.join(a.run, "run.json"), st)  # an interrupted line is still priced from its real start
    rn.start()
    rc = 0
    try:
        if a.line in ("C", "D", "M", "G1L", "G1D"):
            DRIVERS[a.line](cx, st, rn.stop_ev, led)
        else:
            DRIVERS[a.line](cx, st, rn.stop_ev)
    except SystemExit as e:
        st["error"], rc = str(e), 3
    except Exception as e:  # the evidence must survive a driver bug
        st["error"], rc = repr(e), 3
    finally:
        rn.done_ev.set()
        rn.join(timeout=POLL_S + 30)
    st["guard_stop"] = rn.g.stop
    st["halt"] = rn.g.halt
    st["guard_last"] = rn.last
    st["ended"] = iso(time.time())
    S.wjson(os.path.join(a.run, "run.json"), st)
    print(json.dumps({"rc": rc, "stop": rn.g.stop}))
    return rc


# ============================================================================================ verdicts


def verdict(a):
    run = jload(os.path.join(a.run, "run.json"), {}) or {}
    home_dir = os.path.dirname(a.rh)
    led = ledger_load(home_dir)
    t0, t1 = parse_iso(run.get("started")) or 0, parse_iso(run.get("ended"))
    u = priced_usage(a.rh, t0, t1)
    bal_after = parse_balance(jload(os.path.join(a.run, "balance-after.json"), {}) or {}) if a.mode == "real" else None
    b0 = run.get("balance_before_musd")
    v = {"line": a.line, "mode": a.mode, "cap_real_musd": run.get("cap_real_musd"), "real_est_musd": u["real_musd"],
         "root_musd": u["root_musd"], "requests": u["requests"], "by_lane": u["by_lane"], "no_usage": u["no_usage"],
         "balance_drop_musd": (b0 - bal_after) if (isinstance(b0, int) and isinstance(bal_after, int)) else None,
         "guard_stop": run.get("guard_stop"), "error": run.get("error"), "offpeak": run.get("offpeak")}
    v["root_real_ratio"] = round(u["root_musd"] / u["real_musd"], 3) if u["real_musd"] else None
    ln = a.line
    if ln == "P":
        pr = run.get("probe_record") or {}
        pf = (pr.get("probe") or {}).get("per_family") or {}
        w = (run.get("offpeak") or {}).get("window_now", "offpeak")
        per_type = {}
        for g, e in pf.items():
            n_runs = int(e.get("runs") or 0)
            cr = min(int(e.get("tokens_cached") or 0), int(e.get("tokens_in") or 0))
            pi, pc, po = PRICE[w]
            real = -(-((int(e.get("tokens_in") or 0) - cr) * pi + cr * pc + int(e.get("tokens_out") or 0) * po) // MUSD)
            per_type[g] = {"runs": n_runs, "real_musd": real, "real_musd_per_run": real // max(1, n_runs), "root_musd": e.get("cost_musd"),
                           "passes": e.get("passes"), "turns": e.get("turns"), "caps_hit": e.get("caps_hit"), "wall_ms": e.get("wall_ms")}
        pool_lane = sum(x["real_musd"] for k2, x in u["by_lane"].items() if k2 != "sealed")
        pool_runs = int((run.get("pool_result") or {}).get("runs") or 0)
        if pool_runs:
            per_type["pool"] = {"runs": pool_runs, "real_musd": pool_lane, "real_musd_per_run": pool_lane // pool_runs}
        runs = sum(x["runs"] for x in per_type.values()) or 1
        mean = u["real_musd"] // runs if u["real_musd"] else None
        v["probe"] = {"per_type": per_type, "runs": runs, "mean_real_musd_per_run": mean, "sealed_draw": run.get("sealed_draw")}
        if mean is not None:
            led["cost_prior_musd"] = max([mean] + [x["real_musd_per_run"] for x in per_type.values() if x["runs"]])
        ratio_ok = v["root_real_ratio"] is not None and (1.8 <= v["root_real_ratio"] <= 2.2 or (run.get("offpeak") or {}).get("peak_fraction", 0) > 0)
        v["ratio_ok"] = ratio_ok
        draw_ok = (run.get("sealed_draw") or {}).get("counts") is not None
        groups_ok = bool(pf) and all(int(e.get("requests") or 0) > 0 for e in pf.values())
        pool_ok = int((run.get("pool_result") or {}).get("requests") or 0) > 0
        v["requests_ok"] = {"probe_groups": groups_ok, "pool": pool_ok}
        v["pass"] = (bool(pr) and mean is not None and ratio_ok and draw_ok and groups_ok and pool_ok and not run.get("error")
                     and not pr.get("partial"))
        if mean is not None:
            led["plan"] = pick_plan(mean, spent_real(led) + u["real_musd"])  # the program's spend so far (P0 included)
            led["root_real_ratio"] = v["root_real_ratio"]
            v["plan"] = led["plan"]
        if v["root_real_ratio"] is not None and not ratio_ok:
            led["halt"] = f"P: Root/real ratio {v['root_real_ratio']} outside [1.8, 2.2]"
    elif ln == "C":
        c = run.get("calibrate_record") or {}
        fa, fr = c.get("fa_permille"), c.get("fr_permille")
        r3 = (c.get("delta_permille") or {}).get("r3")
        m_std = max(2 * r3, 16) if isinstance(r3, int) else None
        v["calibrate"] = {k: c.get(k) for k in ("s_version", "n_kept", "selected", "fa_permille", "fr_permille", "delta_permille",
                                                "mde_permille", "fa_upper_permille")}
        v["m_standard_permille"] = m_std
        v["pass"] = (isinstance(fa, int) and fa <= 50 and (c.get("fa_upper_permille") is None or c["fa_upper_permille"] <= 70)
                     and isinstance(fr, int) and fr <= 100 and m_std is not None and m_std <= 250 and int(c.get("n_kept") or 0) >= 24)
        if isinstance(fa, int) and (fa > 70 or (c.get("fa_upper_permille") or 0) > 70):
            led["halt"] = f"C: FA {fa}‰ above 70‰ — statistics or harness broken; debug at $0"
    elif ln == "D":
        v.update({k: run.get(k) for k in ("cluster1", "cluster2", "go1", "go2", "tier", "variant", "predicts", "g4", "accept", "stop",
                                          "census", "prereg_sha256", "boot_after", "forced_cluster", "forced_go", "resume", "proposal1",
                                          "screens", "promote_job")})
        acc = run.get("accept") or {}
        v["accepted_for_sticks"] = bool(acc.get("counts_for_sticks"))
        sg = ((run.get("g4") or {}).get("sealed_g4") or {})
        est = ((run.get("go2") or run.get("go1") or {}).get("est_permille"))
        if sg.get("d_permille") is not None and est is not None:
            v["est_d_pair"] = [est, sg["d_permille"]]
        n = re.search(r"parent\+(\d+)%", run.get("predicts") or "")
        if n and sg.get("d_permille") is not None:
            v["predict_hit"] = sg["d_permille"] >= 10 * int(n.group(1))
        # the line ran as designed whether or not it was accepted; a stop before G4 is a valid, reported outcome
        v["pass"] = not run.get("error") and (bool(sg) or bool(run.get("stop")))
        led["lines"].setdefault("D", {})["accept"] = acc
    elif ln == "M":
        m = run.get("milestone") or {}
        v["milestone"] = {k: m.get(k) for k in ("outcome", "n", "repeats", "held_out", "d_permille", "reason", "lineage_gains")}
        v["rundown_ok"] = run.get("rundown_ok")
        v["milestone_skipped"] = run.get("milestone_skipped")
        v["no_regression"] = m.get("outcome") in ("confirmed", "not_confirmed") or bool(run.get("milestone_skipped"))
        v["pass"] = (not run.get("error") and bool(run.get("rundown_ok"))
                     and ((bool(m) and m.get("outcome") != "partial") or bool(run.get("milestone_skipped"))))
    elif ln in ("G1L", "G1D"):
        ok, why = g1_pass(run.get("g4"), exact=(ln == "G1D")) if not run.get("stop") else (False, run.get("stop"))
        v["g1"] = {"ok": ok, "why": why, "g4": run.get("g4")}
        v["pass"] = ok and not run.get("error")
        if ln == "G1L" and not ok:
            led["lines"].setdefault("D", {})["accept_voided"] = f"G1L failed: {why}"
    if run.get("halt"):
        led["halt"] = run["halt"]
    rec = led["lines"].setdefault(os.environ.get("M7_LEDGER_KEY") or ln, {})
    rec.update({"state": "stopped" if run.get("guard_stop") else ("done" if v.get("pass") else "failed"), "real_est_musd": v["real_est_musd"],
                "balance_drop_musd": v["balance_drop_musd"], "root_musd": v["root_musd"], "cap_real_musd": v["cap_real_musd"],
                "pass": v.get("pass"), "started": run.get("started"), "ended": run.get("ended"), "stop": run.get("guard_stop") or run.get("stop")})
    led["spent_real_musd"] = spent_real(led)
    wjson(ledger_path(home_dir), led)
    v["program_spent_real_musd"] = led["spent_real_musd"]
    print(json.dumps(v, sort_keys=True))
    return 0


# ============================================================================================ D-0 freeze


def d0_block(repo):
    t = open(os.path.join(repo, "measurements/m7-runbook.md")).read()
    m = re.search(r"<!-- D0:BEGIN -->(.*?)<!-- D0:END -->", t, re.S)
    return m.group(1) if m else None


def prereg(a):
    """D-0: before any friction is read, record the pre-registration's sha256, the S records' seqs and the time."""
    home_dir = os.path.dirname(a.rh)
    led = ledger_load(home_dir)
    if led.get("prereg"):
        print(json.dumps({"prereg": led["prereg"], "note": "already frozen"}))
        return 0
    blk = d0_block(a.repo)
    if not blk:
        print(json.dumps({"error": "no D-0 block in the runbook"}))
        return 2
    seqs = {k: [r.get("seq") for r in S.lineage(a.rh) if r.get("kind") == k][-1:] for k in ("sealed_draw", "sealed_calibrate")}
    fr = os.path.join(a.ih, "state/kernel/evo/frictions.jsonl")
    led["prereg"] = {"sha256": hashlib.sha256(blk.encode()).hexdigest(), "at": iso(time.time()), "seqs": seqs,
                     "frictions_present_at_freeze": os.path.exists(fr)}
    wjson(ledger_path(home_dir), led)
    print(json.dumps({"prereg": led["prereg"]}))
    return 0


# ============================================================================================ selftest


def selftest(repo):
    fails, n = [], 0

    def ok(cond, name):
        nonlocal n
        n += 1
        if not cond:
            fails.append(name)

    T = lambda s: calendar.timegm(time.strptime(s, "%Y-%m-%dT%H:%M"))  # noqa: E731  (UTC)
    # price windows
    ok(window_at(T("2026-10-20T16:07")) == "offpeak", "window tue 16:07Z offpeak")
    ok(window_at(T("2026-10-21T01:00")) == "peak", "window wed 01:00Z peak")
    ok(window_at(T("2026-10-21T03:59")) == "peak", "window wed 03:59Z peak")
    ok(window_at(T("2026-10-21T04:00")) == "offpeak", "window wed 04:00Z offpeak")
    ok(window_at(T("2026-10-21T09:30")) == "peak", "window wed 09:30Z peak")
    ok(window_at(T("2026-10-24T02:00")) == "offpeak", "window sat offpeak")
    adv = offpeak_advice(T("2026-10-21T00:00"), 7200)
    ok(adv["peak_fraction"] == 0.5 and adv["cost_factor"] == 1.5 and adv["root_real_factor"] == 1.0, f"advice {adv}")
    ok(offpeak_advice(T("2026-10-20T16:00"), 3600)["root_real_factor"] == 2.0, "advice offpeak factor 2")
    ok(iso(next_change(T("2026-10-21T00:30"))) == "2026-10-21T01:00:00Z", "next change")
    # the plan
    p = pick_plan(3000, 30_000)
    ok(p["plan"] == "full" and p["caps"]["C"] == 840_000 and p["caps"]["D"] == 800_000 and p["caps"]["M"] == 450_000
       and p["caps"]["G1L"] == 500_000 and p["caps"]["R"] == 150_000, f"plan full {p.get('caps')}")
    ok(30_000 + sum(p["caps"][k] for k in ("C", "R", "D", "M", "G1L")) <= ENVELOPE_MUSD, "caps fit the envelope")
    ok(runs_c(PLANS["full"]["c"]) == 232 and slice_est_root("standard") == 985_000 and runs_d("standard") == 211 and runs_d("strong") == 339 and runs_m(3) == 123
       and runs_d("lean", True) == 127, "run counts")
    b = plan_bands()
    ok(b["full"] < b["trim"] < b["t5"] and 3200 <= b["full"] <= 3600, f"bands {b}")
    ok(pick_plan(b["full"] + 50, 10 * b["full"])["plan"] == "trim", "plan trim")
    ok(pick_plan(b["trim"] + 50, 10 * b["trim"])["plan"] == "t5", "plan t5")
    ok(pick_plan(b["t5"] + 50, 10 * b["t5"])["plan"] == "stop", "plan stop")
    for m in (2000, 3000, b["full"], b["trim"] - 1, b["t5"] - 1):
        pk = pick_plan(m, 10 * m)
        ok(pk["caps"]["C"] >= pk["est"]["C"], f"C cap ≥ its estimate at {m}")
    ok(d_tier("full", 1_990_000, 450_000, 500_000, 3000) == ("standard", 800_000), "tier standard after C")
    ok(d_tier("full", 2_300_000, 450_000, 500_000, 3000)[0] == "strong", "tier strong when it fits")
    ok(d_tier("t5", 2_000_000, 300_000, 500_000, 3000)[0] == "lean", "tier t5 lean")
    ok(d_tier("full", 900_000, 450_000, 500_000, 3000) == (None, None), "tier none")
    # the guard
    g = Guard(60_000, 10_000_000)
    ok(g.tick(59_999, "skip") == (None, None), "guard under cap")
    ok(g.tick(60_000, "skip") == ("real_cap:priced_usage", "freeze"), "guard est cap")
    g = Guard(60_000, 10_000_000)
    ok(g.tick(10_000, 9_940_000) == ("real_cap:balance", "freeze"), "guard balance cap")
    g = Guard(1_000_000, 10_000_000)
    ok(g.tick(100_000, 9_850_000) == ("meter_mismatch", "freeze") and g.halt == "meter_mismatch", "guard mismatch halts")
    g = Guard(1_000_000, 10_000_000)
    ok(g.tick(100_000, 9_890_000) == (None, None), "guard lag tolerated")
    g = Guard(1_000_000, 10_000_000)
    ok(g.tick(0, None) == (None, None) and g.tick(0, None) == (None, "tighten") and g.tick(0, None) == (None, None), "guard tighten once")
    g = Guard(1_000_000, 10_000_000)
    ok(g.tick(0, "skip", tripwire="canary") == ("tripwire:canary", "freeze") and g.halt == "tripwire:canary", "guard tripwire halts")
    # meters
    import tempfile
    with tempfile.TemporaryDirectory(dir=os.environ.get("M7_SCRATCH")) as td:
        os.makedirs(os.path.join(td, "egress/usage"))
        recs = [{"t": "start", "id": "a", "lane": "live", "at": "2026-10-20T16:00:00Z"},
                {"t": "end", "id": "a", "at": "2026-10-20T16:00:05Z", "cost_musd": 2000, "status": 200,
                 "usage": {"input": 10000, "cache_read": 8000, "output": 1000}},
                {"t": "sealed", "id": "b", "lane": "sealed", "at": "2026-10-21T02:00:00Z", "cost_musd": 1500, "status": 200,
                 "usage": {"input": 1000, "cache_read": 0, "output": 1000}},
                {"t": "end", "id": "c", "at": "2026-10-20T16:00:09Z", "cost_musd": 78643, "status": 200, "usage": None}]
        with open(os.path.join(td, "egress/usage/2026-10-20.jsonl"), "w") as f:
            f.write("\n".join(json.dumps(r) for r in recs) + "\n")
        u = priced_usage(td, T("2026-10-20T15:00"))
        # a: offpeak (2000×0.15 + 8000×0.003 + 1000×0.6) = 300+24+600 = 924; b: peak 1000×0.3+1000×1.2 = 1500; c: 78643
        ok(u["real_musd"] == 924 + 1500 + 78643 and u["root_musd"] == 2000 + 1500 + 78643 and u["no_usage"] == 1
           and u["by_lane"]["sealed"]["requests"] == 1, f"priced usage {u}")
    ok(parse_balance({"balance_infos": [{"currency": "CNY", "total_balance": "1"}, {"currency": "USD", "total_balance": "10.41"}]}) == 10_410_000,
       "balance parse")
    # D: selection, est, go/no-go, Predicts, T2, accept classes
    fr = [{"id": "f-000001", "signature": "path.home_relative", "eligible": True, "s_c_permille": 0, "counts": {"tasks": 5}, "families": ["py", "rust"]},
          {"id": "f-000002", "signature": "finish.tests_failing", "eligible": True, "s_c_permille": 400, "counts": {"tasks": 4}, "families": ["py", "rust"]},
          {"id": "f-000003", "signature": "cap.hit:turns", "eligible": True, "s_c_permille": 400, "counts": {"tasks": 3}, "labels": ["terminal", "repo", "live"]},
          {"id": "f-000004", "signature": "bash.exit:cargo", "eligible": True, "s_c_permille": 900, "counts": {"tasks": 2}, "families": ["rust", "py"]},
          {"id": "f-000005", "signature": "x", "eligible": False, "s_c_permille": 950, "counts": {"tasks": 9}, "families": ["a", "b"]}]
    ok(select_cluster(fr)["id"] == "f-000002", "select by s_c then id")
    ok(select_cluster(fr, {"cap.hit:turns": 3})["id"] == "f-000003", "select tie → S-REAL count")
    ok(select_cluster(fr, exclude=("f-000002", "f-000003"))["id"] == "f-000001", "select cluster #2")
    ok(select_cluster([fr[3], fr[4]]) is None, "select none")
    ok(est_permille(500, 3, 4) == 133 and est_permille(1000, 4, 4) == 333, "est formula")
    ok(go_no_go(150, "standard") == (True, "better") and go_no_go(149, "standard") == (True, "lineage_gain")
       and go_no_go(79, "strong") == (False, "below_resolution") and go_no_go(200, "lean") == (False, "below_resolution")
       and go_no_go(250, "lean") == (True, "better"), "go/no-go")
    ok(predicts_line(160, "better") == D0["predicts"] and predicts_line(96, "lineage_gain") == "pass_rate@sealed >= parent+10%; cost_usd <= parent+30%",
       "predicts line")
    ok(predicts_ok(D0["predicts"]) and predicts_ok("pass_rate >= parent") and not predicts_ok("pass_rate >= parent+0.15 sealed")
       and not predicts_ok("cost_usd@sealed <= parent") and not predicts_ok("pass_rate@sealed >= parent-5%"), "predicts grammar")
    ok(tier_of_paths(["src/loop/mod.rs", "skills/x/SKILL.md"]) == "t2" and tier_of_paths(["skills/x/SKILL.md"]) == "t0"
       and tier_of_paths(["ext/a/src/lib.rs"]) == "t1" and tier_of_paths(["src/a.rs", "vendor/x/y"]) == "dep"
       and tier_of_paths(["tests/a.rs"]) == "t0", "T2 rule")
    ok(accept_class({"verdict": "accept", "accept_class": "better"})["counts_for_sticks"]
       and accept_class({"verdict": "accept", "reason": "lineage_gain"})["counts_for_sticks"]
       and not accept_class({"verdict": "accept", "accept_class": "tie_break:smaller"})["counts_for_sticks"]
       and accept_class({"verdict": "accept", "reason": "cheaper"})["class"] == "tie_break:cheaper"
       and not accept_class({"verdict": "reject", "reason": "no_gain"})["counts_for_sticks"], "accept classes")
    ok(g1_pass({"sealed_g4": {"status": "tie", "d_permille": 0}, "gate": {"verdict": "reject"}}, True)[0]
       and not g1_pass({"sealed_g4": {"status": "tie", "d_permille": 50}, "gate": {"verdict": "reject"}}, True)[0]
       and g1_pass({"sealed_g4": {"status": "underpowered", "d_permille": -100}}, False)[0]
       and not g1_pass({"sealed_g4": {"status": "superior", "d_permille": 200}}, False)[0], "G1 rules")
    # ledger order
    led = {"lines": {}, "plan": None, "halt": None}
    ok(may_start(led, "C", "real")[0] is False and may_start(led, "P", "real")[0], "order P first")
    led["lines"]["P"] = {"state": "done"}
    ok(not may_start(led, "C", "real")[0], "C needs P passed, not only run")
    led["lines"]["P"]["pass"] = True
    led["plan"] = {"plan": "full"}
    led["lines"]["C"] = {"state": "stopped", "pass": False}
    ok(not may_start(led, "D", "mock")[0], "D needs C passed")
    led["lines"]["C"] = {"state": "done", "pass": True}
    ok(may_start(led, "D", "real")[0] is False and may_start(led, "D", "mock")[0], "D needs G1D on real")
    led["lines"]["G1D"] = {"pass": True}
    ok(may_start(led, "D", "real")[0] and not may_start(led, "G1L", "real")[0], "D ok; G1L needs an accept")
    led["lines"]["D"] = {"state": "done", "accept": {"counts_for_sticks": True}}
    ok(may_start(led, "G1L", "real")[0], "G1L after a counting accept")
    led["halt"] = "x"
    ok(not may_start(led, "M", "real")[0], "halt blocks")
    # per-line budgets and the charter's per-pool ceilings
    led = {"lines": {"P": {"real_est_musd": 30_000}, "C": {"real_est_musd": 900_000}}, "plan": pick_plan(3000, 30_000), "halt": None}
    bd = line_budget(led, "D")
    ok(bd["tier"] == "standard" and bd["cap_real_musd"] == 950_000 and bd["reserved_after_musd"] == 950_000, f"budget D {bd}")
    ok(line_budget(led, "M")["cap_real_musd"] == 450_000 and line_budget(led, "G1L")["tier"] == "lean", "budget M, G1L")
    ok(line_budget({"lines": {}, "plan": None}, "P")["cap_real_musd"] == 60_000, "budget P")
    ok(line_budget({"lines": {}, "plan": {"plan": "stop"}}, "C")["cap_real_musd"] == 0, "budget stop")
    led2 = {"lines": {"P": {"real_est_musd": 30_000}, "C": {"real_est_musd": 1_400_000}}, "plan": pick_plan(3000, 30_000)}
    ok(line_budget(led2, "D")["tier"] == "lean", "budget D falls to lean when the envelope is short")
    led2["lines"]["C"]["real_est_musd"] = 1_500_000
    ok(line_budget(led2, "D")["cap_real_musd"] == 0, "budget D refused when no tier fits")
    pp = params_for({"lines": {}, "plan": None}, "P", "real", None, "/r", {"peak_fraction": 0})
    ok(pp["factor"] == 2.0 and pp["reserve_musd"] == 400_000 and pp["ceiling_musd"] == 770_000 and pp["read_deny"] == ["/r/root", "/r/.git"],
       f"params P {pp['reserve_musd']} {pp['ceiling_musd']}")
    ok(params_for({"lines": {}, "plan": None}, "P", "real", None, "/r", {"peak_fraction": 0.2})["factor"] == 1.0, "params factor at peak")
    with tempfile.TemporaryDirectory(dir=os.environ.get("M7_SCRATCH")) as td:
        os.makedirs(os.path.join(td, "egress/usage"))
        with open(os.path.join(td, "egress/usage/d.jsonl"), "w") as f:
            f.write(json.dumps({"t": "sealed", "lane": "sealed", "at": "2026-10-20T16:00:00Z", "cost_musd": 123_456, "status": 200,
                                "usage": {"input": 1, "cache_read": 0, "output": 1}}) + "\n")
            f.write(json.dumps({"t": "start", "id": "z", "lane": "live", "at": "2026-10-20T16:00:00Z"}) + "\n")
            f.write(json.dumps({"t": "end", "id": "z", "at": "2026-10-20T16:00:01Z", "cost_musd": 45_678, "status": 200,
                                "usage": {"input": 1, "cache_read": 0, "output": 1}}) + "\n")
        fz = frozen({"line": "D"}, td)
        ok(fz["reserve_musd"] == 120_000 and fz["ceiling_musd"] == 120_000 + 40_000 + 10_000
           and fz["reserve_musd"] - 123_456 < RESERVATION_MUSD and fz["ceiling_musd"] - fz["reserve_musd"] - 45_678 < RESERVATION_MUSD
           and fz["ceiling_musd"] > 0, f"frozen ceilings {fz.get('reserve_musd')} {fz.get('ceiling_musd')}")
    # the guard thread end to end on a fake Root dir: the poll loop, the simulated balance and the freeze call
    class FakeCx:
        def __init__(self, rh):
            self.rh, self.ih, self.mode, self.applied = rh, rh, "mock", []

        def log(self, **kw):
            pass

        def records(self, kinds=None, since=0):
            return []

        def charter_apply(self, p):
            self.applied.append(p)
            return True

        def root(self, *a, **k):
            return 0, "", ""
    g_poll, g_bal = globals()["POLL_S"], globals()["BAL_S"]
    globals()["POLL_S"], globals()["BAL_S"] = 0.05, 0.05
    try:
        for drain, want, reserve in ((0, "real_cap:priced_usage", 100_000), (120_000, "real_cap:balance", 20_000)):
            with tempfile.TemporaryDirectory(dir=os.environ.get("M7_SCRATCH")) as td:
                os.makedirs(os.path.join(td, "egress/usage"))
                up = os.path.join(td, "egress/usage/d.jsonl")
                rec = lambda c: json.dumps({"t": "sealed", "lane": "sealed", "at": "2026-10-20T16:00:00Z", "cost_musd": c, "status": 200,  # noqa: E731
                                            "usage": None}) + "\n"
                open(up, "w").write(rec(20_000))
                os.environ["M7_SIM_DRAIN_MUSD"] = "0"
                fc = FakeCx(td)
                rn = Runner(fc, 100_000, {"line": "P", "mode": "mock"}, 0, 0)
                os.environ["M7_SIM_DRAIN_MUSD"] = str(drain)
                rn.start()
                time.sleep(0.3)
                early = rn.stop_ev.is_set()
                if drain == 0:
                    open(up, "a").write(rec(85_000))
                rn.stop_ev.wait(3)
                rn.done_ev.set()
                rn.join(2)
                ok((not early or drain) and rn.g.stop == want and fc.applied and fc.applied[-1].get("frozen")
                   and fc.applied[-1]["reserve_musd"] == reserve, f"runner stop {drain}: {rn.g.stop} {fc.applied[-1:] and fc.applied[-1].get('reserve_musd')}")
    finally:
        globals()["POLL_S"], globals()["BAL_S"] = g_poll, g_bal
        os.environ.pop("M7_SIM_DRAIN_MUSD", None)
    # redaction (synthetic values only)
    sec = [b"Jane Q Example"]
    raw = (b"by Jane Q Example <jane.example@example.com>, agent@init.local, key " + b"sk" + b"-ABCDEFGHIJKLMNOPQRST"
           + b", noreply@anthropic.com")  # built at runtime so repo-wide key scans stay at 0
    red, cnt = redact_bytes(raw, sec)
    ok(b"Jane" not in red and b"jane.example" not in red and b"sk-ABC" not in red and b"agent@init.local" in red
       and b"noreply@anthropic.com" in red and leaks_in(red, sec) == 0 and leaks_in(raw, sec) >= 3, "redaction")
    # charter / kconfig text
    ct = charter_text({"mode": "real", "line": "P", "cap_real_musd": 60_000, "ceiling_musd": 770_000, "reserve_musd": 400_000,
                       "read_deny": ["/r/root", "/r/.git"]})
    ok("read_home = false" in ct and 'total_usd = "0.77"' in ct and 'reserve_usd = "0.40"' in ct and "allow_full = false" in ct and "api.deepseek.com:443" in ct
       and "test_mode = false" in ct and '"/r/root", "/r/.git"' in ct, "charter text")
    kt = kconfig_text({"mode": "mock", "cap_real_musd": 800_000, "tier": "standard", "g4_keys": True, "toolchain": "/t", "devdir": "/d"})
    ok('tier = "standard"' in kt and 'gate_usd = "0.80"' in kt and "test_mode = true" in kt, "kconfig text")
    ok("[sealed]\nparallel = 4\n" in ct, "charter: sealed.parallel defaults to 4")
    ct2 = charter_text({"mode": "real", "line": "D", "cap_real_musd": 60_000, "ceiling_musd": 770_000, "reserve_musd": 400_000,
                        "read_deny": ["/r/root"], "sealed_parallel": 2})
    ok("[sealed]\nparallel = 2\n" in ct2, "charter: sealed.parallel 2 for D's G4 retry")
    kt2 = kconfig_text({"mode": "real", "cap_real_musd": 800_000, "tier": "standard", "g4_keys": True, "toolchain": "/t", "devdir": "/d",
                        "g4_parallel": 1})
    ok('[gate.g4]\ntier = "standard"\nparallel = 1\n' in kt2 and kt2.count("[gate.g4]") == 1, "kconfig: gate.g4.parallel 1")
    ok("parallel" not in kt.split("[gate.g4]")[1].split("[")[0], "kconfig: no gate.g4.parallel unless set")
    kt3 = kconfig_text({"mode": "real", "cap_real_musd": 800_000, "tier": "standard", "g4_keys": True, "toolchain": "/t", "devdir": "/d",
                        "g4_parallel": 1, "g3_timeout_s": 1800})
    ok("[gate.g3]\nagent_timeout_s = 1800\ninner_timeout_s = 1800\n" in kt3 and "[gate.g3]" not in kt2, "kconfig: gate.g3 timeouts only when set")
    ok(Guard(1_070_000, 8_220_000, floor=6_560_000).eff_cap() == 1_070_000 and Guard(2_000_000, 8_220_000, floor=6_560_000).eff_cap() == 1_660_000,
       "guard: cap bounded by the balance floor")
    ok(Guard(2_000_000, 8_220_000, floor=6_560_000).tick(1_660_000) == ("program_cap:priced_usage", "freeze"), "guard: program cap on the estimate")
    g3 = Guard(2_000_000, 8_220_000, floor=6_560_000)
    ok(g3.tick(100_000, 6_560_000) == ("program_cap:balance_floor", "freeze") and g3.halt == "program_cap:balance_floor", "guard: balance floor halts")
    ok(Guard(1_070_000, 8_220_000).tick(500_000, 7_800_000) == (None, None), "guard: no floor, no change")
    # DV-5b: a stranded `promoted` proposal goes back to `screened`, only for the same chosen sha
    import tempfile
    with tempfile.TemporaryDirectory() as td:
        pf = os.path.join(td, "p-000001.json")
        json.dump({"state": "promoted", "chosen": {"sha": "c" * 40}, "history": [{"state": "promoted"}]}, open(pf, "w"))
        try:
            dv5_reopen(pf, "d" * 40, "t")
            bad = False
        except ValueError:
            bad = True
        r = dv5_reopen(pf, "c" * 40, "t")
        try:
            dv5_reopen(pf, "c" * 40, "t")
            twice = False
        except ValueError:
            twice = True
        ok(bad and twice and r["state"] == "screened" and len(r["history"]) == 2 and json.load(open(pf))["state"] == "screened",
           "dv5 reopen: same sha only, once, history kept")
    # DV-5: the single G4 re-runs on the same sha (rollback first while it is still provisional)
    c, l = "c88be5f0" * 5, "aaf07b72" * 5
    ok(dv5_decision(c, True, l, c) == "rollback" and dv5_decision(l, False, l, c) == "promote"
       and dv5_decision(c, False, l, c) == "dv5_chosen_already_lkg" and dv5_decision("x" * 40, True, l, c) == "dv5_unexpected_current"
       and dv5_decision(c, True, None, c) == "dv5_unknown_state", "dv5 decision")
    # D-0 drift: the runbook's pre-registration states the constants this script enforces
    blk = d0_block(repo) or ""
    for msg in (NOP_G1,):
        ok(all(32 <= ord(ch) < 127 or ch == "\n" for ch in msg), "commit messages are printable ASCII (trailer rule)")
    for needle in ("K = 3", "80‰", "150‰", "250‰", "T = 0.8", "$0.15", "pass_rate@sealed >= parent+15%", "cost_usd <= parent+30%",
                   "One G4 attempt", "`better` or `lineage_gain`", "tie_break", "T2", "≥ 3 pool tasks", "≥ 2 families"):
        ok(needle in blk, f"d0 drift: {needle!r}")
    print(json.dumps({"selftest": "pass" if not fails else "fail", "checks": n, "failed": fails}, ensure_ascii=False))
    return 0 if not fails else 1


# ============================================================================================ CLI


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=("selftest", "ready", "plan", "offpeak", "params", "charter", "kconfig", "prereg", "prepare", "run",
                                    "verdict", "redact", "clear-halt", "ledger", "freeze", "dv5-reopen"))
    ap.add_argument("line", nargs="?")
    ap.add_argument("--repo", default=os.path.dirname(os.path.dirname(HERE)))
    ap.add_argument("--bins")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--mean-musd", type=int, default=PLAN_MUSD_PER_RUN)
    ap.add_argument("--spent-p-musd", type=int, default=30_000)
    ap.add_argument("--wall-s", type=int, default=3600)
    ap.add_argument("--ih")
    ap.add_argument("--rh")
    ap.add_argument("--run")
    ap.add_argument("--mode", choices=("mock", "real"))
    ap.add_argument("--home")
    ap.add_argument("--params")
    ap.add_argument("--out")
    ap.add_argument("--cap-real-musd", type=int, default=0)
    ap.add_argument("--root-cap-musd", type=int, default=0)
    ap.add_argument("--reason")
    ap.add_argument("--mock-port", type=int)
    ap.add_argument("--pack-dir")
    ap.add_argument("--toolchain", default="")
    ap.add_argument("--devdir", default="")
    a = ap.parse_args()
    if a.cmd == "selftest":
        return selftest(a.repo)
    if a.cmd == "ready":
        r = readiness(a.repo, a.bins or os.path.join(a.repo, "target/m7-bin"))
        if a.json:
            print(json.dumps(r, sort_keys=True))
        else:
            for ln, v in r["lines"].items():
                print(f"{ln:4} {'READY' if v['ready'] else 'blocked'}  {', '.join(v['missing'])}")
            for k, v in r["items"].items():
                print(f"  {'ok ' if v['ok'] else '-- '} {k:20} {v['lane']:18} {v['what']}  [{v['lane_status']}]")
        return 0
    if a.cmd == "plan":
        print(json.dumps({"pick": pick_plan(a.mean_musd, a.spent_p_musd), "bands_max_musd_per_run": plan_bands()}, sort_keys=True))
        return 0
    if a.cmd == "offpeak":
        print(json.dumps(offpeak_advice(time.time(), a.wall_s), sort_keys=True))
        return 0
    if a.cmd == "params":
        home_dir = os.path.dirname(a.rh)
        led = ledger_load(home_dir)
        mock_led = ledger_load(os.path.join(os.path.dirname(home_dir), "m")) if a.mode == "real" else None
        ok_, why = may_start(led, a.line, a.mode, mock_led)
        if not ok_:  # refused before anything starts: no Root, no spend, no ledger entry
            print(json.dumps({"refused": why, "cap_real_musd": 0, "budget": {"why": why}}))
            return 5
        bins = a.bins or os.path.join(home_dir, "bin")
        rd = readiness(a.repo, bins)["items"]
        p = params_for(led, a.line, a.mode, a.rh, a.repo, offpeak_advice(time.time(), a.wall_s), a.mock_port,
                       sealed_m7_keys=rd["sealed_charter_keys"]["ok"], g4_keys=rd["g4_tier_key"]["ok"], toolchain=a.toolchain, devdir=a.devdir)
        wjson(os.path.join(home_dir, "charter.params.json"), p)
        print(json.dumps(p, sort_keys=True))
        return 0 if p["cap_real_musd"] > 0 else 5
    if a.cmd in ("charter", "kconfig"):
        p = jload(a.params)
        with open(a.out, "w") as f:
            f.write(charter_text(p) if a.cmd == "charter" else kconfig_text(p))
        return 0
    if a.cmd == "prereg":
        return prereg(a)
    if a.cmd == "prepare":
        return prepare(a)
    if a.cmd == "run":
        a.line = a.line
        return run_line(a)
    if a.cmd == "verdict":
        return verdict(a)
    if a.cmd == "redact":
        print(json.dumps(redact_tree(a.line)))
        return 0
    if a.cmd == "ledger":
        print(json.dumps(ledger_load(os.path.dirname(a.rh)), sort_keys=True, indent=1))
        return 0
    if a.cmd == "freeze":
        # emergency stop for the orchestrator (and the live freeze test): ceilings := spent, applied live
        home_dir = os.path.dirname(a.rh)
        params = jload(os.path.join(home_dir, "charter.params.json"))
        if not params:
            print(json.dumps({"error": "no charter.params.json"}))
            return 2
        f = frozen(params, a.rh)
        with open(os.path.join(a.rh, "charter.toml"), "w") as fh:
            fh.write(charter_text(f))
        env = {"HOME": a.home or os.path.expanduser("~"), "INIT_ROOT_HOME": a.rh, "PATH": "/usr/bin:/bin"}
        r = subprocess.run([os.path.join(a.rh, "bin/init-root"), "charter", "check", "--apply", "--yes"], capture_output=True, text=True,
                           env=env, timeout=60)
        print(json.dumps({"rc": r.returncode, "reserve_musd": f["reserve_musd"], "ceiling_musd": f["ceiling_musd"],
                          "err": r.stderr[-300:] if r.returncode else ""}))
        return 0 if r.returncode == 0 else 1
    if a.cmd == "dv5-reopen":
        # $0, Root stopped (checked by the owner socket): see dv5_reopen
        sock = os.path.join(a.rh, "run/owner.sock")
        try:
            c = socket.socket(socket.AF_UNIX)
            c.settimeout(2)
            c.connect(sock)
            c.close()
            print(json.dumps({"refused": "the Root is running"}))
            return 2
        except OSError:
            pass
        path = os.path.join(a.ih, "state/kernel/evo/proposals", (a.line or "") + ".json")
        try:
            p = dv5_reopen(path, a.reason or "", iso(time.time()), os.environ.get("M7_DV5_WHY"))
        except (ValueError, OSError) as e:
            print(json.dumps({"refused": str(e)}))
            return 2
        print(json.dumps({"reopened": p.get("id"), "state": p.get("state"), "history_n": len(p.get("history") or [])}))
        return 0
    if a.cmd == "clear-halt":
        led = ledger_load(os.path.dirname(a.rh))
        led.setdefault("cleared", []).append({"halt": led.get("halt"), "reason": a.reason or "", "at": iso(time.time())})
        led["halt"] = None
        wjson(ledger_path(os.path.dirname(a.rh)), led)
        print(json.dumps({"cleared": True}))
        return 0
    return 64


if __name__ == "__main__":
    sys.exit(main())
