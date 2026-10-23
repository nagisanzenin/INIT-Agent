# M7 line P: the cost probe (real deepseek-flash, 2026-10-21)

```
Status: measured · 2026-10-21 03:37–03:44 +07 (20:37–20:44Z, off-peak) · Area: evolution (lane REAL-M7)
Protocol: measurements/m7-runbook.md §5.1 · Artifacts: build/real-artifacts/m7/real/P/ (redacted; 0 leaks)
```

## What ran
- **Home:** `target/m7-homes/r`, installed by the real install path with a fresh seed build and the 20-task pool v1. Charter: `read_home = false`, real cap $0.06, Root ceilings sealed $0.40 and total $0.77 (factor 2.0 off-peak, plus 4 reservations).
- **Draw:** `init-root sealed draw --pack-dir root/sealed/packs/s1`, model-free and $0. Counts only:
  - G4: E 14, C 9, T 9;
  - reserve: E 10, C 5, T 5;
  - 4 canaries;
  - 0 templates skipped;
  - `s1.0`.
- **Probe:** `init-root sealed probe --per-family 2 --usd 0.48 --real`: 2 throw-away instances per group, R = 1 on the LKG (genesis), deleted afterwards.
- **Pool:** `initctl evo calibrate --repeats 1 --limit 2`, run by the live agent in 2 owner turns, kernel job `j-000001`: 2 pool tasks at R = 1.

## Cost per task type (real $, priced from the Root's own usage ledger at the off-peak list price)

| Type | Runs | Passes | Turns | Real $ | Real $ per run | Root-metered $ | Wall per run |
|---|---|---|---|---|---|---|---|
| C (CRUX change) | 2 | 2 | 9 | 0.0033 | **0.0017** | 0.0067 | 17 s |
| E.py (Exercism, Python) | 2 | 1 | 12 | 0.0099 | **0.0049** | 0.0197 | 53 s |
| E.rust (Exercism, Rust) | 2 | 1 | 16 | 0.0085 | **0.0042** | 0.0169 | 60 s (1 turn-cap hit) |
| T (terminal) | 2 | 1 | 20 | 0.0074 | **0.0037** | 0.0147 | 39 s |
| pool (kernel lane; includes the agent's 2 turns) | 2 | 2 | – | 0.0065 | **0.0033** | 0.0130 | – |
| **All** | **10** | **7** | | **0.0356** | **0.00356** | **0.0711** | |

## Meters
- **Priced usage (real):** $0.0356.
- **Root meter:** $0.0711. Root/real = **1.999**, inside [1.8, 2.2]: the Root bills peak prices, with cache hits at the cache-read price, as expected.
- **Balance:** $9.56 → **$9.53** (−$0.03 at the endpoint's 2-decimal resolution), settled after the run. It agrees with the priced estimate.
- **Guard:** no stop; the line's real cap of $0.06 was not reached. The guard's in-process balance read failed on its first poll; m7.sh's own reads before and after succeeded. Fixed for later lines: the guard now retries the start balance, adopts a late first read, and makes no back-to-back read.

## What it decides (runbook §4, D-0 item 6)
- **Plan: trim.** The mean of $0.00356/run is just above the full band ($0.00347) and below the trim band ($0.00369). Trim keeps C as designed (32 × 6 + pool 20 × 2), runs M at R = 2, and allows D at standard or lean.
- **`gate.g4.cost_prior_usd` = $0.0049**, the largest per-type mean (E.py), set in the kernel config for C and D.
- **D tier: standard.** It fits even if C spends its whole cap: what is left after C (≥ $1.88, R's cap reserved) covers D's $0.94 + M's $0.36 + G1L's $0.58.
- The incumbent passed 7 of 10 probe runs (sealed 5 of 8). Calibration's band selection decides S's targeting.

## Expected cost of the next lines at P's mean (real $, off-peak)

| Line | Runs | Expected | Cap (the guard's stop) |
|---|---|---|---|
| C calibration + pool | 232 | **$0.83** | $0.93 |
| R (inside D) | worker + E3 | $0.09 | $0.15 |
| D G0–G3 + G4 v2, standard, both arms fresh | 211 | **$0.78** | $0.94 (line cap with R: $1.09) |
| M milestone (20 × 2 × 2 + 3) + run-down | 83 | **$0.30** | $0.36 |
| G1L (only if D counts) | 127 | $0.48 | $0.58 |
| **Program incl. P, D not accepted** | | **$2.03** | |
| **Program incl. P, D accepted (+ G1L)** | | **$2.51** | ≤ $3.00 |

Wall time at the measured per-run times: C about 45–50 min (192 sealed runs at parallel 4, plus 40 pool runs); D about 1.5 h (agent session, screen, G0–G3 with a cold build, the slice); M about 20 min.
