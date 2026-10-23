# WP8c: `init-kernel gate judge` (lane K1)

Status: **done for Phase 0 tree candidates** (2026-10-20, about 05:35 +07). The real `gate judge` runs inside the Root's sealed gate-of-gates. Against the stand-in M it rejects all seven mutants and accepts C2 and C3. C1 depends on which task the gog draws as its holdout (finding 1).

## Done
**`kernel-gate::judge`** implements kernel/gate.md §4.1a over the existing gate logic. Code is in `src/judge/{mod,tree,exec,relay}.rs`.

- **CLI.** `init-kernel [-c <key>=<value>]… gate judge --home --candidate --running --lkg [--stages] [--level L0..L4|0..4] --out`.
  - It writes `{"verdict","reason","scope","stages":[…],"record":{…}}` to `--out`, with keys sorted explicitly.
  - Exit codes: 0 accept, 1 reject, 2 inconclusive, 3 error, 64 usage.
  - `init-kernel` main splits off leading `-c` pairs (`-c` is accepted for `gate` only) and dispatches `gate judge`.
- **Candidates are trees** (WP24's contract): `--candidate`, `--running` and `--lkg` are directories. The judged form is the script body, an executable `/usr/bin/perl` `init-body` with its manifest after `__DATA__` and a `--selftest` entry point.
  - **G0** applies the cargo stage's defenses, with gate.md's codes:
    - safety pragmas `use strict; use warnings;` in the header (`gate.g0.forbid_missing`);
    - no top-level `no strict` or `no warnings` (`gate.g0.build_config`);
    - `perl -c` (`gate.g0.check`);
    - `--selftest`, run by the gate and judged by its exit code only (`gate.g0.test`);
    - a per-step timeout (`gate.g0.timeout`).
  - **G1**: the tool manifest must parse, be non-empty, have unique names, and give each tool a name, a description and object `parameters` (`gate.g1.manifest_error`). It also records `tools_sha256` and `body_tokens` (⌈bytes/4⌉).
  - **G2 and G3** are recorded as `skipped` for trees, because they need probes on a running kernel.
  - **G4** is the real driver `g4::run_g4`: canaries first, the seeded pairs, infra re-runs and the top-up. It then applies `accept::judge`: δ from the LKG repeats, the bootstrap NI test, the split and divergence guards, CT, size and `cost_guard`.
- **D5, no traces.**
  - Bodies run as children of the judge with their workspace at `<home>/probe/<12 hex>/work/task`. Every probe root has the same length, so both arms pay the same prompt tokens for the path. Each root is deleted by its exact path after its check.
  - `<home>/judge/<gate_id>/` is G0 scratch, also deleted. `probe/` and `judge/` are removed only if this judge created them.
  - Nothing else is written: no lineage, no note, no `noise.json`, no trace or ledger. The live kernel proxy is never used.
- **Egress (`gate.judge.egress`, default `auto`).**
  - `relay`: a per-run in-memory relay (`judge::relay`) between the body and `INIT_PROXY_SOCK`. It tracks usage, the finish reason and the per-run cost cap in memory, and runs go in parallel (`gate.g4.parallel`).
  - `direct`: the body talks to `INIT_PROXY_SOCK` itself. Runs are serialized, and each run's usage is the difference of the upstream's `GET /_init/stats` before and after it.
  - `auto` probes whether a socket can be bound. **The Root's sealed profile denies `network-bind`**, so inside the gog the judge goes direct.
  - Both modes price tokens at the reference rate: `deepseek-flash` peak, $0.30 in and $1.20 out per Mtok.
- **Determinism.**
  - The gate id is `j-` plus 12 hex of a digest of the three tree digests, every task directory, the level, the stages and the configuration. It seeds every shuffle and resample.
  - There is no `noise.json` history, because its age filter reads the clock.
  - `ct_include_wall` defaults to false in judge mode.
  - Runs in the record are sorted, with timing fields zeroed, so two runs give byte-identical output.
- **Configuration**: defaults, then `<home>/kernel/config.toml` `[gate.g4]` and `[gate.judge]`, then `-c` overrides. An unknown `-c` key is a usage error. The same `2 ≤ repeats ≤ repeats_max ≤ 5` rule as `GateCfg` applies.

**WP24 test switch** (`root/crates/init-root/tests/wp24.rs`, the only Root file touched):
- `INIT_GOG_GATE_BIN=<init-kernel>` makes `gate_of_gates_admits_the_correct_gate` run the real binary with `-c gate.g4.min_pool=4 -c gate.g4.min_holdout=1`, matching the stand-in S's 4 + 1 sample.
- The expected reasons are the gate codes: M1 `test`, M2 `forbid_missing`, M6 `manifest_error`. The rest are the same as with the stub.
- The broken-gate variants (MS-G1…G3, SE-18) stay on the stub.
- **The stub stays the default.** Speed is not the reason: the real run took 54 s, the stub run 60–77 s. The reasons are that the real binary fails MS-C1 on most holdout draws (finding 1), and that a Root test should not need a kernel build.

## Tests
- `cd kernel && CARGO_TARGET_DIR=target/k1 CARGO_INCREMENTAL=0 cargo test --offline -p kernel-gate`: lib **183 passed** (6 new judge tests), 1 ignored; `tests/g0.rs` 12 passed; **`tests/judge.rs` 4 passed** (about 40 s, $0, `init-mockllm` in socket mode). The four tests:
  - `g45_g50_…`: all 10 classes with the holdout on `sx-calc-fix`. M1 `reject/test`, M2 `forbid_missing`, M3 and M5 `score_inferior`, M4 and M7 `canary`, M6 `manifest_error`, C1 `accept/better`, C2 `cheaper`, C3 `smaller`.
    - Exit codes match the verdicts.
    - C1 run twice gives byte-identical output (**G-45**).
    - After every class, the home copy's file list is unchanged, with no `vault/traces` and no `noise.json` (**G-50**).
  - `g45_exit_codes_and_refusals`: `candidate_form`, `unsupported_tree` (cargo tree) and `no_egress` give exit 3; `--stages G0,G1,G2` gives `accept/stages_pass` with G2 skipped; the home is untouched.
  - `c1_with_an_unexercised_holdout_is_pool_holdout_divergence`: finding 1, pinned. It gives L = 20‰, d_holdout = 0 and δ = 0.
  - `direct_egress_prices_runs_by_the_lane_stats_difference`: C2 and M3 give the same verdicts and identical per-run reference costs in relay and direct mode.
- `cargo test --offline -p init-kernel`: unit tests 2, `e2e` 1 (WP7b's test, still green), **`judge_cli` 1** (usage 64, `-c` for gate only, an unknown key gives 64, a bad tree gives 3 with the JSON in `--out` and nothing else written).
- `cargo clippy --offline -p kernel-gate -p init-kernel --all-targets -- -D warnings`: clean. New files are `rustfmt --edition 2024`.
- Root, default (stub): `cd root && cargo test --offline -p init-root --features test-fixtures --test wp24 gate_of_gates`: **2 passed** (77 s).
- **Root with the real gate, run once**: `INIT_GOG_GATE_BIN=kernel/target/k1/debug/init-kernel cargo test … --test wp24 gate_of_gates_admits_the_correct_gate` (54 s, $0):

  | Class | Verdict |
  |---|---|
  | M1 | reject/test |
  | M2 | reject/forbid_missing |
  | M3 | reject/score_inferior |
  | M4 | reject/canary |
  | M5 | reject/score_inferior |
  | M6 | reject/manifest_error |
  | M7 | reject/canary |
  | C2 | accept/cheaper |
  | C3 | accept/smaller |
  | **C1** | **reject/pool_holdout_divergence** |

  The resulting record was `mutation{total 7, rejected 7, controls_ok 2, failed ["C1"], disagreements 0, pass:false, spent_musd 44994}`. **MS-1…MS-7, MS-C2 and MS-C3 pass and the agreement check holds; MS-C1 fails, so the test fails** (finding 1).

## Findings
1. **MS-C1 versus the divergence guard (needs a gate.md / sealed-eval.md decision).**
   - C1 restores M3's regression, which touches only `sx-calc-fix`, `sx-primes` and `sx-squares`.
   - With a deterministic model, δ = 0, so the guard `d_pool − d_holdout > 2·√(δ_p² + δ_h²)` fires on any gain the holdout does not show. With the gog's 1-task holdout, C1 is accepted only when the random holdout is `sx-calc-fix`, about 1 draw in 5. Every other draw rejects it as `pool_holdout_divergence`.
   - This is gate.md §3.8.3 as written. It says a capability the holdout does not exercise must come in through `capability_gain`. It conflicts with sealed-eval.md §3.3's requirement that a correct gate accepts C1.
   - Options:
     - (a) give the split and divergence guards a "half a task" floor, like the margin's 0.5/n;
     - (b) have the gog build C1 so its effect reaches every task, or stratify the holdout;
     - (c) grow the stand-in S so that the holdout has 4 tasks.

   I did not change the accept rule.
2. **The sealed profile denies `network-bind`.** Hence the direct egress. It also means the judge cannot hold its own per-run cost cap inside the gog; the lane's `gog_usd` cap applies.
3. **M4 and M7 are caught only because bodies can read the home copy's vault.** Inside the Root's instance the judge cannot nest a Seatbelt profile, so the leak canary is the defense, as WP24's stub assumed. A judge that sandboxed its bodies would see M4 and M7 as plain `cheaper`, because the stand-in mutants fall back to solving.

## Deviations
1. **Tree candidates, not shas** (WP24 deviation 1). A non-directory `--candidate` gives `error: candidate_form`. A cargo tree gives `error: unsupported_tree`, because judge has no build slot or selfmap yet.
2. G0 and G1 for script bodies are analogues of the cargo stages and reuse their codes. The reason is the code's last segment, as in promote records.
3. **Judge-mode defaults:**
   - `ct_include_wall = false`, because wall time is not reproducible (§4.1a determinism);
   - no `noise.json` history and no futility look;
   - reference prices are constants (deepseek-flash peak);
   - the record's per-run `spent_musd` is that reference estimate, while the Root measures its real lane spend;
   - `budget` uses `GateCfg`'s defaults (`cost_guard` only).
4. The record adds `runs` (unredacted: the reader is the Root), `tasks` (counts and skipped ids), `config`, `egress` and `candidate.form: "tree"`. The `scope` is always `t2`.
5. `-c` is a new global `init-kernel` option. gog passes its `--` args before `gate judge`, and gate.md's CLI has no config flag.

## Needs from others
- **gate.md / sealed-eval.md owners**: finding 1, then flip the `INIT_GOG_GATE_BIN` run to green. Also note in sealed-eval.md §3.2 that a judge in the sealed profile cannot bind sockets (finding 2).
- **WP24 / Root**: when the home copy gets `body.git`, pass shas, and add a cargo-tree G0 through the build slot to judge.

## Left
- Sha candidates (`<home>/body.git` → `git archive`), cargo trees (real `g0::run`, selfmap G1), and G2/G3 in judge (K2 needs them).
- `test-weaken-canary`, `test-reject-all` and `test-score-skew` kernel features (gate.md §4.1) for MS-G1…G3 on the real binary.
- `gate simulate` CLI wiring.
