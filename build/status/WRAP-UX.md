# WRAP-UX: install and daily-use UX for the release (docs/ux-v1.md)

Status: **paused** (orchestrator, 2026-10-22 ~19:00 +07: the evolution test runs first; this lane resumes only if
it passes). **No code was changed and nothing was committed except this file**, so HEAD builds as before.

## Done (before the pause)
- Read: BUILD_AGENT_BRIEF, CONVENTIONS, V1_PLAN, RELEASE_PLAN, docs/ux-v1.md, scripts/{install.sh,init,release.sh,
  uninstall.sh}, root `install.rs`/`cli.rs`/`link.rs`/`sup.rs::install_seed`, kernel `kernel-supervise/src/{install.rs,
  build.rs, promote/stages.rs}`, `init-kernel/src/serve.rs`, the TUI crate layout, and the Root's notice and alert codes.
- **Agreed with WRAP-PROV** (their message, 18:5x; they record it in build/status/WRAP-PROV.md "CLI contract for WRAP-UX"):
  1. `init-root providers detect --json` → `{"local":[{"kind":"ollama"|"lmstudio"|"llama-server"|"vllm","addr":"127.0.0.1:<port>","models":[{"id"}]}]}`
     (loopback `GET /v1/models` on 11434, 1234, 8080, 8000; 300 ms each; no `tools` field: show "tools: untested").
  2. `init-root providers configure --charter FILE --provider <id> --model <id> [--base-url https://… --price IN,CACHED,OUT]
     [--local 127.0.0.1:PORT --server ollama|lmstudio|llama-server|vllm --context N] [--level 0] [--kernel-config OUT]`:
     appends `[actions.providers.providers.<id>]` to FILE, runs the charter check, writes `[model]` + `[providers.<id>]` to OUT.
     Exit 0, or 2 with one line on stderr. Catalog providers need no URL; a custom URL needs `--price`; `--local` is $0.
  3. `init-root keys set <id>` stays TTY-only (the Root reads the key; `init` never sees it), with a $0 verify. Exit:
     0 stored+verified · 2 not a TTY/bad id · 3 rejected (401/403), not stored · 4 stored, unverified · 5 stored, no credit.
     `init-root providers --help` exists once (1) and (2) land.

## Plan for the resume (release scope, priority order; all tests $0, scratch homes only)
1. **Prebuilt body, bit-for-bit, no cold build at install.**
   - Kernel `kernel-supervise/src/install.rs` (additive): if `INIT_HOME/kernel/prebuilt/init-body` (+ `.sha256`) exists
     (Root-written, read-only to the zone), skip step 5's build: copy it to `bin/init-body.staged` as a new inode, check its
     sha256, then continue (store.put, promotion, lkg). Write `state/kernel/warmup.json` `{state:"pending", sha, shipped_sha256}`.
     The kernel takes no new arguments, so `sup.rs` (X8) needs no change.
   - Root `install.rs` (small, additive): `--prebuilt-body FILE --prebuilt-sha256 HEX` → verify the file, copy it to
     `INIT_HOME/kernel/prebuilt/`; after `install_seed`, check the genesis tuple's body blob sha256 == HEX (bit-for-bit).
   - **Background warm-up** (new `kernel-supervise/src/warmup.rs` + a hook of a few lines in `init-kernel/src/serve.rs` `start`): a thread that
     holds `env.slot_lock` (so evolution waits on it), starts about 60 s after boot, builds the LKG sha with exactly G0's
     `BuildSlot` path, compares the sha256 with the shipped one → `verified` / `differs` (`state/kernel/warmup.json`
     and an owner notice), and **commits the deps cache** (today nothing in production calls `DepsCache::commit`, so
     every G0 build is cold). No toolchain gives `no_toolchain` (chat still works).
   - Finding: bit-for-bit across homes is unlikely without `--remap-path-prefix` (slot, toolchain, CARGO_HOME) **and**
     no macOS OSO debug-map paths (the dev profile keeps debug info for the body crate; the std rlib paths include the
     toolchain dir). So `differs` must be a normal, explained outcome, not an error (ux-v1 §3.5.3 "rebuilt_local").
2. **Bundle + one-line installer.** `scripts/release.sh --bundle`: the flat release dir (kept, so `init-root install
   --release` works unchanged) + `init` (launcher) + `init-tui` + `body/init-body` (built in a slot-like dir from the
   seed bundle with `BUILD_ARGS` and the slot env) + `SHA256SUMS` (POSIX `shasum -c`; no python or jq needed) + a
   `MANIFEST.json` `body{tag, commit, sha256, profile:"dev"}` section → `init-<v>-<target>.tar.gz` and `<channel>.json`.
   `scripts/get-init.sh` (POSIX sh, ux-v1 §3.2.3): platform → channel → disk → download → sha256 → unpack to
   `~/.local/share/init/versions/<v>` → `current` symlink → `~/.local/bin/{init,init-agent}` symlinks (v0.1's bash launcher
   counts as INIT's; a foreign file stops the install) → PATH line. Pass `--bin-dir <app dir>` at install so the zone can
   never write the app dir (`~/.local/bin` is already denied by Z3a). Release host and signing: PENDING OWNER (OW-UX1/2).
3. **Launcher `init`** (new crate `tui/crates/init-cli`, binary `init`, std + serde_json): no install → first run with
   6 line-mode screens (checks incl. PATH and "Rust not set up: chat works now"; provider: DeepSeek / OpenAI-compatible /
   LOCAL through WRAP-PROV's CLI above; limits; autonomy cap plus ADR-011 `extend.mode` shown simply (default "gated");
   this folder; ready + service), then `init-root install … --prebuilt-body …`, then `init-root keys set <id>` with the
   terminal handed over, then the service or a detached start, then exec `init-tui`. `--yes` with flags for tests. With an
   install: make sure the Root runs, then exec `init-tui`. Subcommands start/stop/status/panic/keys/doctor/--which/--version/--help.
   Deviation to record: the first run lives in the launcher instead of a Root setup mode (§3.4.1), so the Root does not grow.
4. **Daily use (TUI and Root).** `/start` (Root alias in `link.rs` `line()`: rootish → `panic::resume`, else the kernel's
   resume), `/resume` → sessions (kernel `owner_in`: `/sessions` lists `state/sessions/*.jsonl`; `/resume <id>` writes
   `state/session` and restarts the body; a bare `/resume` while stopped still resumes, with "use /start"), a clearer status bar,
   plain `ux.*` copy (root down, no key, key invalid, ceiling, provider down), Root alerts rendered with the action
   (`grant_request` → `/grant <id>` · `/deny <id>`; rollback, quarantine, demoted, level_up, ceiling_hit, halted, safe_mode).
5. `init --help` and a README quickstart (one line, then `init`).

## Tests planned
UX-T3-style installer test (fixture bundle, scratch HOME, `sh` and `/bin/dash`), the launcher's dispatch and first-run
goldens, a first-run e2e in a test-mode Root on the mock (`--service none`, the key file written by the harness), and a
kernel test for the prebuilt install and the warm-up's verified/differs/no_toolchain paths. Never `~/.init`, `~/.init-root`, or the
owner's `~/.local/bin`.

## Needs from others
- WRAP-PROV: the three commands above (agreed; WRAP-PROV is also paused and **none is built yet**, so the first run
  keeps a DeepSeek-only fallback and hides the other two rows until `init-root providers --help` exists).
- X8-KLANE / INTEG-A: none yet. My planned edits to `install.rs` (root and kernel) and `serve.rs` are additive, and `sup.rs` is not touched.

## Deviations (planned)
- The first run lives in the launcher, not in a Root setup mode (ux-v1 §3.4.1).
- The bundle is flat (the existing release dir plus extras), not ux-v1 §3.2.2's `bin/`/`body/` split, so `--release` is unchanged.
- No minisign signature until the release key and host are decided (OW-UX1, OW-UX2). Trust on first install rests on HTTPS plus sha256.
