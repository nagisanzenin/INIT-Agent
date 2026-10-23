# K-GATE: promote/gate stages under the real sandbox profiles

Status: **done** (P1 and P2). Commits: `70f30d3` (G1 fix), `900feef` (Seatbelt integration test and ci step), plus this status file.

## Done
1. **G1's sandbox OUT, the release blocker (`70f30d3`).** S-REAL saw it live: `gate.g1.manifest_error`, `init-body --manifest: cannot start`, and `ext/count-lines: no built component`.
   - The cause: `promote/stages.rs` `g1()` ran selfmap in the `build` profile with `OUT = cache/gate/<job>/g1`, so the candidate's `bin/` and `ext/` were outside the profile.
   - The fix: `Built` now carries the dir that holds `bin/` and `ext/`. G1's `OUT` is that dir, as sandbox.md §3.4/§3.9 and gate.md §3.4 say:
     - for a promote or quick gate: `cache/gate/<job>/`;
     - for G3's inner gate: `cache/gate/<job>/g3/`.
     The pre-fix code would have failed the inner gate the same way.
   - `HOME`, `TMPDIR`, the cwd and `selfmap.log` are in `OUT/g1/`. The output is `OUT/self.json` (gate.md §3.4).
   - The request is built by `stages::g1_child` (pub), so the tests run the exact production request.
2. **Audit of every stage's profile and params** against sandbox.md §3.4/§3.9 and gate.md:
   | Stage | Profile, params (as built) | Verdict |
   |---|---|---|
   | build (§3.6.2) | `build`, SRC `cache/build/slot/src`, OUT `cache/build/slot/out` | matches |
   | G0 | the kernel's children in the `kernel` zone profile (WP8b deviation 1; section 6a sockets under SLOT_OUT) | known deviation, unchanged |
   | G1 | `build`, SRC the export, OUT the job dir (fixed) | matches after `70f30d3` |
   | G2 arms, G3 probe, G4 runs | `body` with probe params (`body_params(Probe)`): binary and `kernel/` cloned into the probe root | matches; the Root's class rules pass (test) |
   | G4 checks | `check`, SRC `OUT/ws`, OUT `cache/gate/<job>/check/<run_id>/` | matches gate.md's check protocol |
   | fetch (promote, quick, G3) | plain `git-upload-pack` inside the kernel profile (WP8b deviation 2), not `git-read` | known deviation, unchanged |
   | G3 inner gate | build + G0 + G1 (OUT `cache/gate/<job>/g3/`) + G2 | was broken like G1; fixed |
   | handoff | the supervisor's live `body` spawn of the staged binary (same params as every live boot) | matches; live spawns are proven by S-REAL's real runs |
   | quick gate | the same `g0_to_g3` | covered |
3. **$0 integration test** `kernel/crates/kernel-supervise/tests/promote_seatbelt.rs` (`900feef`; macOS).
   - Every broker instance (build, G1, probes, checks) is refused or launched as `init-root`'s broker does it:
     - the Root's own `params::validate_spawn_params` (V1–V5 and class rules) and `check_spawn_misc`;
     - a fresh instance mark;
     - `seatbelt::compose` over the compiled-in texts;
     - then `sandbox-exec`.
   - It uses the root crate `init-sandbox-profiles` as a test-only dev-dependency, so there is no copy of the texts or the rules to drift.
   - G0's commands and every `$G` command (`[supervise] git` = a wrapper, so the plain upload-pack runs there too) run under the `kernel` zone profile with the Root's `kernel_profile_params`.
   - `g1_selfmap_request_runs_under_the_real_build_profile` (fast, ~1 s, in the plain suite) runs the exact `g1_child` request with a stub selfmap:
     - it reads and runs the binary, reads the components, and writes the output, HOME and TMPDIR;
     - the pre-fix OUT is refused (`manifest_error`), which shows the harness really enforces.
   - `a3_promote_g0_to_g4_under_real_seatbelt_profiles` (ignored, ~70 s) is A3 on WP8b's fixture body (sv-agentbody execed from the instance's `kernel/bin`) with init-mockllm:
     - `gate --quick`, then promote: G0, G1, G2, G3 (with the inner gate), handoff, async G4 accept `better`, LKG;
     - G4 runs are neither infra nor no_ready, and the candidate scores 1000;
     - no refusal by the Root's rules;
     - instance counts per kind and profile, and G0 commands under the kernel zone, are asserted.
   - `ci/local.sh` runs it in the step "promote under Seatbelt (A3, real profiles)", right after "seed G0".
     - It builds into the shared kernel target dir (no new target dir; the fixture body is a no-deps crate).
     - Homes are under `kernel/target/test-homes/sb/`, removed by exact path on pass.

4. **G4 never scores runs that never ran.** S-REAL's j-000002 had all 13 pairs `cost_capped` with 0 requests and still got a verdict.
   - **Run errors.** A run with `requests = 0` is an error, not a score (`g4::run_error`).
     - Incumbent error: the task is left out.
     - Candidate-only error: the run scores 0 at the incumbent's cost, so it can never be `cheaper`. A candidate capped on every run is rejected.
   - **Voiding.** If incumbent errors exceed 10 % of its runs (at least 1), or every incumbent run errs, G4 stops launching runs. It ends `error: runs_failed:<cause>`, with an agent-safe note.
   - **Cost caps.** G4 refuses to start on a task whose `max_cost_musd` is below one request's reservation in the dearest price window: `error: task_cost_cap`. The reservation comes from the new `ProxyHandle::request_reservation_musd`, wired through `GateProxy` and init-kernel's `KernelGate`.
   - **No retry.** Neither error is retried (only `infra` is). The incumbent stays LKG, and an alert is raised.
   - Canaries stay tripwires: a leak needs no request. The judge test's M4 caught that during the work.

5. **Release check: shipped task caps clear the floor.** `kernel-supervise/tests/shipped_task_caps.rs` fills in `scripts/install.sh`'s proxy block and compiles it. The floor it gets is 65,536 × $1.20/Mtok = 78,643 µUSD.
   - Every shipped cap clears it:
     - the stand-in sealed S (12 tasks and the canary): 200,000 each;
     - `initctl task template`: 500,000;
     - the Root's sealed default: 500,000;
     - gate.md's import default: 500,000.
   - The seed ships no task. Nothing needed fixing.
   - The floor uses the dearest window (peak), not the cheapest. A cap between the off-peak and peak reservations would work at one time of day and fail at the other.

## Tests
- `cargo test --offline -p kernel-supervise --lib promote::`: 7 passed. New: `g1_child_paths_lie_inside_the_build_profile`, which checks the promote job and the inner gate.
- `cargo test --offline -p kernel-supervise --test promote_seatbelt -- --include-ignored`: 2 passed, **3 runs in a row** (67.8 s, 57.9 s, 80.2 s).
- K-GATE G4 run errors: `cargo test -p kernel-gate` 191 + 12 + 4 (judge, including M1–M7 and C1–C3) passed.
  - New unit tests: `a_run_without_a_model_request_is_an_error`, `every_run_capped_is_runs_failed_not_a_verdict`, `errors_within_the_bound_leave_their_task_out`, `a_capped_candidate_cannot_win_cheaper`, `a_task_cap_below_one_reservation_refuses_g4`, and kernel-proxy `request_reservation_is_the_default_output_at_the_dearest_price`.
  - New Seatbelt test: `g4_refuses_caps_below_one_request_and_never_scores_runs_that_never_ran`. It covers `task_cost_cap` with no runs, then real capped runs giving `runs_failed:cost_capped`; neither is retried, and LKG is unchanged.
  - Also green: `kernel-supervise --test promote --test serve_seam` (7) and `--lib` (82).
- `cargo clippy --offline -p kernel-gate -p kernel-proxy -p kernel-supervise -p init-kernel --tests -- -D warnings`: clean. `cargo check --offline --workspace --tests` (kernel): clean.

## Deviations
1. G1's scratch (`HOME`, `TMPDIR`, cwd, log) is in `OUT/g1/`; the docs did not say where. There are as-built notes in gate.md §3.4 and the sandbox.md §3.4 table.
2. The G1 child has write access to `OUT`, which holds the candidate's own binary and components, so candidate code (`--manifest`, `describe()`) could rewrite them before G2 and the handoff. This is what the doc specifies. It adds nothing new: G0 already runs candidate tests with write access to the target dir the binary comes from. In v0.3 the zone is the security boundary.

## Findings for others (not fixed here)
1. **The kept component set is never handed to probes, and a T2 handoff does not move it.**
   - G2, G3 and G4 call `probe_scoped` with `ext_set_dir: None` (pipeline.rs, stages.rs, g4run.rs), so every probe gets an empty `bin/ext`.
   - gate.md §3.5 says arm c gets the candidate's set and arm p the current one. gate.md §3.12 says G4's c gets the current or the job's set and p gets `lkg.json.ext_set`.
   - supervise.md §3.6.5 ("kept T1 components move with the binary") is implemented only on the data-only path (`run_data_only` → `extset::publish` + `DataOnly.ext`). The handoff path publishes nothing.
   - Effects: G2 never boot-tests the candidate's components (G1's `describe()` still checks them), and a T2 promotion leaves `bin/ext` as it was.
   - It does not block E2E-3 or A3-real:
     - the install publishes no set;
     - the seed's request `tools` is the static registry, so the tools hash is unaffected.
   - Owner: WP7b/WP8b (supervisor handoff + promote). It touches the supervisor's handoff command, so it is not a release-push change.
   - **Tagged v0.1.x** after the orchestrator's follow-up: does an extension survive a T2? See the next item.
2. **A kept extension survives a T2 promote and handoff (proven, $0).** `a3_promote_g0_to_g4_under_real_seatbelt_profiles` checks this case.
   - Setup: before the T2, a set holding `count-lines` is published and `bin/ext` and `bin/ext.lkg` point at it. That is the state a T1 promotion and its LKG commit leave.
   - After the handoff, G4 accept and LKG:
     - both links still name that set;
     - the new body was spawned with `INIT_HOME` = the home and the new sha, so `Boot` loads `KEPT/*.wasm` (ext/host.md §3.9; seed tests `h18_trial_kept_resolution`, `h24_…`);
     - `promotion{lkg:true}` for the T2 carries `ext_set` and `components`, and the LKG commit `store.put` the component.
   - Why it survives: `commit_bin`, `install_from` and `install_lkg` rename only `init-body*`, and nothing on the handoff path touches `bin/ext`. `lkg_commit` reports and stores whatever set `bin/ext` names. A rollback makes `bin/ext` follow `ext.lkg`.
   - Remaining staleness: a T2 that also changes `ext/` hands off with the old set. That is the §3.6.5 gap above, v0.1.x.
   - **A trial-only extension** (built in `work/target`, loaded with `initctl reload`, never promoted) is dropped at every body process start, by design: ext/host.md §3.8, "At boot and at each new session, only kept components load". This applies to a handoff, a crash restart and a rollback alike, and the §3.6.5 fix would not change it. To keep an extension the agent T1-promotes it (A2-real).
   - Usability trap for WP14/docs: after a restart, the trial artifact's mtime is older than the new `session_start`. An unchanged `cargo build` does not refresh it, so `initctl reload` reports "not built" until the source is touched or the crate rebuilt.
3. S-REAL must rebuild and freeze `init-kernel` from `70f30d3` or later before rerunning E2E-3 and A3-real.

## Left
- Not covered by the new test:
  - the kernel's in-process file work runs unsandboxed in the test process (production runs it under the `kernel` profile);
  - the live body and the handoff go through the in-test fake Root unsandboxed;
  - the real seed with ext components: too heavy for today's disk, about 1.3 GB of slot target per build. A `INIT_PROMOTE_SBX_SEED=1` variant on seed_g0's warm dir is a v0.1.x candidate.
