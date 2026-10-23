# WP6a: macOS sandbox profiles, Root composition and the sandbox self-test

Status: **done** (2026-10-20, lane X). Crate `root/sandbox-profiles/` (package `init-sandbox-profiles`), a member of the root workspace.

## Done
- **Profile texts** (`profiles/`), byte-identical to the docs: PRELUDE and the `body` SECTION (kernel/sandbox.md §3.3.1), `build`/`check` (§3.4), `git-read` (§3.5), the `kernel` SECTION (root/root.md §3.3.3), ROOT_ALLOWS (§3.3.1) and the zone tail Z1–Z7 with Z2 per D9 (§3.3.2). `tests/doc_drift.rs` fails if a text drifts from its doc block. The texts and the composition are behind `cfg(target_os = "macos")`. The Linux renderer is WP6b.
- **Composition** (`seatbelt.rs`), which `init-root` will call: `render(id, extra_read)` = PRELUDE + SECTION + ROOT_ALLOWS + ZONE_TAIL. `compose(id, params, layout, mark)` adds `MARK = Z_MARK` and every `Z_*` (`ZoneLayout::zone_params`), then checks that the parameters the text uses and the map agree exactly. `Rendered::sandbox_exec_argv()` gives `/usr/bin/sandbox-exec -p … -DK=V…`, so values are never spliced into text. Also here: `compose_over` (the tail over any inner text, for RR-1), `create_mark` (0400, `O_CREAT|O_EXCL|O_NOFOLLOW`), `new_instance_id`, and `kernel_profile_params`. Unit tests check that the tail comes last, is deny-only, contains no `define`, and that the per-profile parameter lists match the texts.
- **Validation** (`params.rs`, platform-neutral): V1 (names, at most 32, no `Z_*`, no `MARK`), V2 (absolute, at most 1024 bytes, no NUL, no `.`/`..`, canonical, plus the `LINK_*` exemption of D8), V3 (roots), V4 (`check_spawn_misc`), V5 (kind/profile pairs), and kernel/sandbox.md §3.12's class rules (live, probe and member roots; lane sockets; `SRC`/`OUT` under `cache/`). Entry point: `validate_spawn_params(kind, profile, params, ctx)`. A refusal is an `ArgsError`, which maps to `root.args`.
- **Self-test** (`selftest.rs`, `probe.rs`, `sys.rs`). This is the library that lane R exposes as `doctor --sandbox-selftest`: `selftest::run(&SelftestConfig)`, with `probe::inner_main` as the in-sandbox probe. The `init-sbx-selftest` binary (`inner …` / `run --base DIR`) is the stand-alone runner.
  - It needs no Root process. It builds a throwaway `INIT_HOME` and `ROOT_HOME` under `root/target/test-homes/wp6a/<id>/`, validates and composes each instance as the Root would, and launches it via `/usr/bin/sandbox-exec`.
  - It binds its own stand-ins for `owner.sock`, `egress.sock` and the lane sockets.
  - It applies an exec-clean equivalent to every launch: fds ≥ 3 are set close-on-exec and the signal mask is emptied.
  - The run directory is removed by its exact path when everything passes, and kept for inspection on failure.

## Tests
- `cd root && cargo test --offline -p init-sandbox-profiles`: all green.
  - 10 unit tests;
  - 2 doc-drift tests;
  - 4 parameter tests: RR-4 fuzz (Z_ names, MARK, relative paths, `..`, NUL, symlinked values, a `LINK_*` with a non-canonical parent, roots outside V3, class violations, V5, V4) and RR-4b;
  - 1 self-test run.
- `cargo clippy --offline -p init-sandbox-profiles --all-targets -- -D warnings`: clean.
- **Self-test, macOS 14.6 (23G80, M2): 202 pass, 0 fail, 11 skipped (213 checks), about 8 s** (`init-sbx-selftest run`, and identically inside `cargo test`).

  | Group | Pass |
  |---|---|
  | S-M (live body, probe, member, S-M0 parse ×8) | 84 |
  | S-Z (`kernel` profile, including S-Z8b, S-Z10, S-Z11 across two instances) | 39 |
  | RR-1 (zone tail over `(allow default)`) + RR-2 (kernel marks, decoy, canary, `extra_read`) + RR-4b | 39 |
  | S-B (`build` ×9 incl. the S-B6 `cargo build --frozen`; `check` ×3) | 12 |
  | S-G (S-G1 fetch through a sandboxed upload-pack; S-G2 alternates into `vault/` with an unsandboxed control that does leak) | 5 |
  | S-X3 (holdout, canaries, sealed; live and probe) + S-X1k (key store) | 6 |
  | D9 `extra_read` (body) | 2 |
  | V.* (the runner's own parameter maps pass V1–V5 and the class rules) | 8 |
  | CTRL (the same probe without a profile is allowed, so every refusal comes from the profile) + CAL (policy-query calibration) + SYS-1 (`sandbox_check` resolves) | 7 |

- **Skipped (11).** Disposable-env only: S-M40, S-M41, S-M47a, S-M47b and S-Z17a–f. No sweep or kill test was implemented or run. S-M38 (DNS) is also skipped: it would send DNS traffic, and the resolver socket is covered by S-M34.

## Deviations
1. **Crate location.** `root/sandbox-profiles/` per the orchestrator. roadmap.md's WP6a row says `init-kernel::sandbox`, but since v0.3 the Root owns the texts. The validation and composition code that root.md budgets inside `zone.rs` (§4.8) now lives in this crate, outside the `rootloc` count. Lane R or the orchestrator should decide whether it counts toward the budget.
2. **`LINK_*` (D8).** The docs disagree. root/root.md V2 says a `LINK_*` names a symlink or nothing. kernel/sandbox.md §3.3.3 says `LINK_*` equal real, canonical directories. Both forms are accepted. A symlink is accepted only inside a probe or member root, and the live root's `LINK_*` must equal its target.
3. **Class rules** use the exact values from sandbox.md's tables (`BODY = <root>/body`, …), not the looser "lie together under". `KERNEL`/`BIN` may be `INIT_HOME`'s or the instance root's (§3.3.3 table; §3.12 says "real INIT_HOME"). The parameter set must match the profile exactly: a missing or unused name is `root.args` (V1).
4. **Orchestrator note "Z1–Z4".** The full tail Z1–Z7 is implemented verbatim. It is root.md's exact text, and Z5–Z7 are needed for marks, sockets and signals.
5. **IP rows** (the network checks used Unix sockets plus traffic-free loopback). Seatbelt on 14.6 does **not** refuse `socket(AF_INET|AF_UNIX)` creation, even with only `(allow system-socket (socket-domain AF_UNIX))` or no rule at all. The refusal comes at bind, connect or sendto (`EPERM`). So:
   - S-M35 and S-M37 (and S-Z7a/b, RR-1v3/v4, S-B4c) target **the runner's own** 127.0.0.1 TCP listener and UDP socket instead of `:22` and `8.8.8.8:53`. Nothing leaves the host, and no foreign service is touched.
   - S-M36 and S-M37b are ephemeral loopback binds.
   - S-B4 is "bind a Unix socket in OUT" plus connects.
6. **S-M16 and S-M17** are `sandbox_check` policy queries on the existing `/private/tmp` and `$HOME`, so nothing is ever written outside the repo. They are calibrated by CAL-1 (`work/` allowed) and CAL-2 (`kernel/` denied). Finding: for a *missing* path, `sandbox_check` returns "denied" even unsandboxed, so queries must name existing paths. S-M17r is the same check as a real write, inside the test dir.
7. **S-M45** runs a copy of the probe as `kernel/bin/initctl` until WP1's binary exists.
8. **S-M46** builds a local proc-macro crate with no external dependencies, which exercises proc-macro dylib loading in `work/`. It does so because `vendor/` is outside the sandbox.
9. **`root/Cargo.lock`** is committed with only this crate's new entry (8 lines), so `--locked` builds keep working. Of `root/Cargo.toml`, only the `members` line changed.

## Needs from others
- **Lane R (WP21, `init-root`):**
  - Depend on `init-sandbox-profiles = { path = "../../sandbox-profiles" }`.
  - In `zone.rs`, call `params::validate_spawn_params` and `params::check_spawn_misc` (V1–V5), then `seatbelt::compose`, `create_mark` and `Rendered::sandbox_exec_argv`.
  - For `doctor --sandbox-selftest`:
    - call `selftest::run(&SelftestConfig{ probe_exe: <init-root binary>, probe_prefix: ["sandbox-selftest-inner"], toolchain/devdir/sdkroot/home_real/git from install.json, .. })`;
    - add the hidden subcommand `sandbox-selftest-inner` → `probe::inner_main`;
    - pick a `base` short enough that `<base>/<6 hex>/i/probe/p1/run/kernel.sock` is at most 103 bytes. The runner refuses a longer one.
- **Docs owners (sandbox.md, root.md):**
  - reconcile the `LINK_*` wording (deviation 2) and `KERNEL`/`BIN` in §3.12 (deviation 3);
  - write down the on-machine targets for S-M35, S-M37 and S-M38 (deviation 5);
  - give the **sealed tail** an exact text. root.md §3.3.6 is prose only, so it is not implemented here.

## Left
- WP6b: the Linux renderer (bwrap argv, Landlock, seccomp, V6) and the S-L rows.
- The disposable-env half: S-M40, S-M41, S-M47a/b, S-Z17a–f, RR-6, RR-7 and RR-11–RR-13, on a VM or a separate user account.
- The sealed tail (§3.3.6), once it has text.
- S-X1, in full (grep every readable file for key strings), and S-X2 (needs supervise).
- Re-running the suite against the real Root and Root paths (T-S5…T-S8).
- Runs on macOS 15 and 26.
