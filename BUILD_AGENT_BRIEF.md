# Brief for build agents (read this first)

You build one **work package (WP)** of INIT Agent. The orchestrator gives you the WP id, the lane and the files you own.

## Inputs
- `BUILD_PLAN.md`: tonight's goals and the Slice-1 priority.
- `docs/roadmap.md` §3.3: your WP row, which gives inputs, outputs, tests and dependencies.
- The docs named in your WP row, plus `docs/CONVENTIONS.md` and `docs/reconciliation/v03-decisions.md` (binding).
- The docs are the spec. When code and docs disagree, follow the docs. If a doc is ambiguous or wrong, choose the simplest correct option and record it in your status file under "Deviations".

## How to work
1. Edit only the crate or paths you were assigned. Read anything else, but do not edit it. If you need another crate changed, record that under "Needs from others".
2. Write real code with real tests. Use the $0 mock model (`init-mockllm`) once it exists; until then use in-test stubs.
3. Run offline builds: `cargo build --offline` and `cargo test --offline -p <your crate>` in the right workspace (`kernel/`, `root/` or `seed/`). Run `cargo clippy --offline -p <crate> -- -D warnings` where it is practical.
4. Dependencies are already vendored in the repo-root `vendor/`, which holds 290 crates. If you truly need a new crate, add it to your Cargo.toml and run **`ci/vendor.sh`**. It syncs all four manifests. Never run `cargo vendor` on a single manifest, because that deletes the other workspaces' crates. Network access is limited to crates.io, with a generic User-Agent.
4b. There are no git worktrees. Everyone works in the one checkout, in their own paths. **Always `source ci/env.sh` before any cargo command** (no incremental caches, line-table-only debug info). Use each workspace's **default** target dir (`kernel/target`, `root/target`, `seed/target`; the ext workspace uses `CARGO_TARGET_DIR=seed/target`, because the target dir must not live inside `seed/ext/`). Lanes share these, so cargo may wait on its build lock; that is expected. Never create another target dir without the orchestrator's OK.
4d. **Wrap every heavy command in a build slot:** `ci/slot.sh cargo test --offline ...`, `ci/slot.sh cargo build ...`, and installs and scenario runs. At most 3 run at once (the real-model lane has its own slot: `INIT_SLOT_POOL=real`). Never hold a slot while thinking or editing.
4c. Docs and shared files: edit with exact, small replacements (the Edit tool), never by rewriting a whole file, because other lanes may be editing the same doc at the same time.
5. Commit only your own paths, and only when your tests pass:
   `git add -- <paths> && git commit -m "WPxx: <summary>\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -- <paths>`
   If git reports `index.lock`, retry up to 10 times. Never `git add -A`. Never commit `vendor/`, `target/` or anything under `~`. Do not push; the orchestrator pushes.
6. Write `build/status/WPxx.md` with these sections: done, tests (command plus result), deviations, needs from others, and what is left. Commit it together with your work.

## Safety (binding)
- Signal or kill only processes **you spawned**, identified by PID. Never sweep or pattern-kill on the live system. Test sweep logic against fake process tables only.
- Every test uses a temporary home under `target/test-homes/<wp>/`. Never touch `~/.init`, `~/.init-root` or any other path outside the repo. Sandbox tests may apply profiles only to processes they spawn.
- Delete only by exact path. If `df -h /` shows less than **5 GB** free, stop and report. Clean up your own temporary files and test homes by exact path when you finish.
- The owner's email and git config never go into any request. The only install allowed is `rustup target add wasm32-wasip2`, and WP0 has already done it.
- **Sealed S (M7, docs/m7-design.md §3.1.2):** build lanes must never read `ROOT_HOME/sealed/S/`, `ROOT_HOME/sealed/seeds/` or any live sealed draw (instances, seeds, ids, per-task results), on any machine or test home. Pack generators under `root/sealed/packs/` are maintainer content: only the M7 pack lanes (S1-CORE, S1-E, S1-B, S1-C, S1-T) may read and write them, testing with dev seeds on the mock model; real-model results reach them only as per-template aggregates. Every other lane leaves `root/sealed/packs/` alone.
- **Real model:** the DeepSeek key file at `~/.config/init-agent/deepseek.key` may be read **only** by the Slice-1 integration WP. Never print, log, commit or echo the key. Tonight's real-model spend cap is $1.00 across the whole build. Every other test uses the mock.

## Reply format
At most 10 lines: WP and status (done / partial / blocked), test results, files, deviations, needs from others.
