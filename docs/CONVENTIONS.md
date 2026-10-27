# INIT Agent docs: conventions and contract registry

*Status: v0.3 + build sync 2026-10-20 + the provisional owner decisions of the v0.1.0 release push (§6). Aligned with spec v0.3, which adds the Root and the autonomy ladder (ADR-007).*

`INIT Agent spec.md` (v0.2) is the source of truth for the design. The docs in `docs/` turn it into implementation detail. They have to be detailed enough that the build phase is only transcription and verification. Every cross-doc decision is recorded in `docs/reconciliation/<area>.md`.

## 1. Doc rules

- Write in English, in Markdown.
- One area owns each doc (see §4). A doc may *use* a contract another doc owns, but it must not redefine it. If you need a change to someone else's contract, raise it under **Open questions → cross-doc**, and it gets settled in a reconciliation ledger.
- Every doc opens with this header block:
  ```
  # <Title>
  Status: draft v0.1 · 2026-10-19 · Area: <kernel|body|ext|evolution|overview>
  Depends on: <docs>   Defines: <contracts this doc owns>
  ```
- Required sections, in this order:
  1. **Purpose**
  2. **Responsibilities / non-goals**
  3. **Design**
  4. **Interfaces**
  5. **Failure modes and recovery**
  6. **Performance notes** (spec §9)
  7. **Invariants and security** (spec §2.1)
  8. **Test plan** (use testing.md scenario IDs)
  9. **Open questions**
  10. **Spec deltas**
  11. **References** (URL and the date you checked it)
- Rust snippets use real, current APIs, checked against docs.rs or crates.io as of October 2026. Mark anything you could not check `UNVERIFIED`.
- Give concrete defaults, not "configurable".
- No code files until the build phase.
- **Privacy:** never put the owner's email, or anything taken from git config, into a network request. That includes User-Agent strings. Never install toolchains. Delete scratch files only by their exact path.
- **Live-system safety (added 2026-10-20 after an incident):** tests and experiments on the owner's machine may signal, kill or modify only processes and files they created themselves, identified by a PID or path they hold. Never kill by pattern, sweep or broad predicate on the live system. A sweep or kill rule must be tested in a disposable environment, such as a VM, a container or a separate user account. On 2026-10-20 a sandbox sweep test SIGKILLed 33 of the owner's processes, including Docker Desktop.

## 2. Repository layout (GitHub `nagisanzenin/INIT-Agent`, private)

```
INIT-Agent/
  README.md
  (no repo-root CONSTITUTION.md: the text has one canonical file and two copies, listed below the tree)
  INIT Agent spec.md            spec v0.3 (Part II = the Root and the autonomy ladder)
  docs/                         implementation docs; docs/reconciliation/ holds the cross-doc ledgers
  reports/  research_notes/     the research behind the design
  wit/init-ext/v1/init-ext.wit  authoritative WIT, package init:ext@1.0.0 (kernel-owned; one dir per major)
  kernel/                       Cargo workspace (maintainer-owned)
    crates/init-kernel/         binary `init-kernel`: supervise, proxy, gate, selfmap, serve, install, attach, keys, panic …
    crates/initctl/             binary `initctl`: std + init-proto only; no async runtime, no network crates
    crates/init-proto/          kernel-side shared types (serde); never a dependency of the body
    crates/init-testkit/        publish = false: init-mockllm, init-fakebody, init-measure, scenario runner (init-mockllm ships in every install since eec8d5e; init-fakebody only with --with-testkit)
  root/                         Cargo workspace for `init-root` (maintainer/owner-owned; never evolvable by the agent; spec v0.3 §13, root/root.md)
  tui/                          Cargo workspace for `init-tui`, the owner's terminal client opened by `init-agent` (maintainer/owner-owned; never evolvable by the agent; never linked into `init-root`; tui.md)
  ci/local.sh  .github/workflows/ci.yml   local-first CI (testing.md)
  vendor/                       the single dev-checkout vendor dir for every workspace: kernel, root, seed, seed/ext, and tui from WP26 (not committed; ci/vendor.sh)
  measurements/                 Phase 0 measurement results (testing.md M-1..M-5)
  seed/                         the seed body, its OWN Cargo workspace (never a member of kernel/)
    Cargo.toml                  crate `init-body`; [workspace] exclude = ["ext"]; build config owned by body/boot.md §3.16
    .cargo/config.toml          vendored sources, offline (dev checkout: directory = "../vendor", the repo-root vendor/)
    src/  tests/  vendor/       vendor/ only in the seed repo and the installed body (see below the tree)
    GENESIS.md  SELF.md         T0 prompt data (awareness.md)
    skills/                     including ext-authoring/{SKILL.md, assets/template/…}
    wit/init-ext/v1/init-ext.wit  byte-identical copy of the authoritative WIT (G1 check: gate.g1.wit_copy_drift)
    ext/                        separate extension workspace: Cargo.toml, Cargo.lock, members = ["*"]
```

**Vendoring.** The dev checkout has one repo-root `vendor/`, and each workspace's `.cargo/config.toml` points at `../vendor`. The seed repo and the installed body carry their own cut `vendor/` with `directory = "vendor"`, as WP21b's seed packaging writes it.

**Constitution text.** The canonical file is `root/crates/init-root/assets/CONSTITUTION.md`, embedded in the Root (text: awareness.md §3.3; PENDING OWNER APPROVAL). Two byte-identical copies exist: `seed/src/boot/CONSTITUTION.md` (inside the body crate, so the installed body is self-contained and no editable `body/CONSTITUTION.md` can shadow `kernel/CONSTITUTION.md`) and `seed/src/context/testdata/head/CONSTITUTION.md` (the golden snapshot for T-CTX-2). `ci/local.sh` should check the copies for drift, like the WIT copy check.

The body never links kernel code. It reaches the kernel only through files, environment variables, its stdin and stdout (the stdio-jsonl/1 protocol), and the two sockets.

## 3. Runtime layout (`INIT_HOME`, default `~/.init`)

kernel/supervise.md §3.1 is authoritative. This table mirrors it.

| Path | Owner | Agent access | Contents |
|---|---|---|---|
| `kernel/` | kernel | read | `bin/{init-kernel,initctl,init-mockllm}` (`init-mockllm`, the $0 mock that G0's tests use, is in every install since eec8d5e and is not part of a tuple; `init-fakebody` only with `--with-testkit`), `CONSTITUTION.md`, `wit/`, `docs/`, `config.toml` (edited by the owner) |
| `bin/` | kernel | read and exec | `init-body`, `init-body.prev`, `init-body.staged`, `init-body.lkg` (hard link). Extension sets: `ext.d/<set>/<name>.wasm` (immutable), with symlinks `ext`, `ext.prev`, `ext.staged`, `ext.lkg` |
| `lineage/` | kernel | read | `body.git`, the **authoritative** lineage that holds `main`, `refs/init/*` and the notes `refs/notes/gate` and `refs/notes/predict`. `seed.git`, the seed mirror (`refs/upstream/*`). `trees/<sha12>/`, exported LKG and current trees |
| `body/` | agent | read-write | The agent's working repo, **not authoritative**. Remotes: `origin` = `lineage/body.git`, `upstream` = `lineage/seed.git`, both read-only. The body syncs itself at boot, and the kernel never runs git inside `body/`. |
| `work/` | agent | read-write | Task workspaces (the body's cwd), worktrees `wt-<id>` on `evo/<id>`, the agent's own `target/`, and `$TMPDIR` |
| `state/` | agent | read-write | `sessions/<session_id>.jsonl`, `session` (the current id. At spawn the kernel reads it, or picks a new `YYYYMMDD-HHMMSS-<6hex>` if it is missing or invalid, and always passes it as `INIT_SESSION`. The body writes it when it creates the session file, and generates a new id for `/new`.), `tasks/<task_id>/`, `spill/<session_id>/`, `blobs/` (content-addressed), `ext/{catalog.json,<name>/data/,<name>/log}`, `run/{ready,heartbeat}` |
| `state/kernel/` | kernel | read | `lkg.json`, `boot.json`, `update-failed.json`, `supervisor.json`, `inbox.jsonl`, `alerts.jsonl`, `jobs/`, `gate/<job>/report.json`, `probe/`, `vendor/`, `proposals/`, `logs/`, `usage.json`, `thinning.json` |
| `probe/<id>/` | kernel | none | One root per probe instance; each instance sees only its own root. Lane sockets live at `probe/<id>/run/{kernel,proxy}.sock`. |
| `cache/` | kernel | none | `deps/<key>/` (dependency build cache), `build/slot/{src,out}/` (the single build slot), `gate/<job>/`, `vendor/<job>/`, `git-home/`. There is no shared target dir, because a candidate's `build.rs` could poison later builds. |
| `vault/` | kernel | none (hidden) | `keys/<provider_id>` (mode 0600), `traces/v1/{req/,packs/,scopes.jsonl}`, `gate/{pool/,jobs/,noise.json}`, `holdout/`, `canaries/`, `lineage.jsonl`, `usage.jsonl` |
| `run/` | kernel | connect to `kernel.sock` and `proxy.sock` only | `kernel.sock`, `proxy.sock`, `kernel.lock`, `kernel.pid`. As of v0.3, the owner socket, the panic latch and sandbox marks are Root-owned and live under `~/.init-root` (root/root.md). |

## 4. Contract registry (one owner each)

| Contract | Owner doc | Fixed now |
|---|---|---|
| **Trust zones, Root responsibilities R1–R7, autonomy ladder L0–L4** | spec v0.3 §12–§19, ADR-007 | The only security boundary is the Root versus everything else. The agent can never modify the Root. Humans: O(1), no review. |
| Root process, R1 (stop, whole-system LKG, panic), R4 (lineage hash chain, off-machine mirror), R5 (charter format), R7 (autonomy ledger, demotion) | root/root.md | Binary `init-root`, owner OS user, outside every sandbox; designed for about 1,000–1,500 LOC. The budget is now ≈ 7,500 as per-module sub-budgets of measured + 5%, re-measured at release, with a core subtotal ≤ 1,770 and `root/sandbox-profiles` on its own lines (root.md §4.8). This is a provisional owner decision of 2026-10-20 that the owner may override (§6). 7,085 lines were measured at cdb49e8, where the table totals 7,540. |
| R2 (egress gateway: keys, metering, hard ceilings, quotas), R3 (external-action classes) | root/egress.md | The agent zone's only network path is the Root's egress socket |
| R6 (sealed task set S, mutation suite M / gate-of-gates, sealed audit), agent-authored task admission | root/sealed-eval.md (admission rules shared with kernel/gate.md) | |
| Runtime layout (§3), supervisor state machine, handoff, exit codes | kernel/supervise.md | Exit codes: `0` stop · `75` handoff complete · `78` at boot twice in a row → `Halted{config}` (not a crash) · any other code = crash. Boot sub-codes 64/70/78: body/boot.md |
| Env vars passed to the body | kernel/supervise.md | `INIT_HOME`, `INIT_SESSION` (never empty from the production kernel; an empty value, in tests and manual runs only, means the body starts a new session), `INIT_SHA`, `INIT_BOOT_ID`, `INIT_BOOT_REASON`, `INIT_MODE` (normal, probe, safe), `INIT_UI`, `INIT_KERNEL_VERSION`, `INIT_KERNEL_SOCK`, `INIT_PROXY_SOCK`. `INIT_TEST_*` is honored only when `INIT_TEST=1`. |
| Owner channel and `/panic` | root/root.md | v0.3 moves both to the Root (`init-root attach`), so panic never depends on an evolvable kernel. |
| The `init-agent` TUI: placement and trust rules (T-1…T-5), TTY layout, key map, rendering of owner-channel frames, notice classes, status bar, text sanitizer, client-side reconnect and dedup, line mode, exit codes, TUI-* tests | tui.md | A client of the owner channel only: workspace `tui/`, binary `init-tui` (opened by the `init-agent` launcher). It reaches only `ROOT_HOME/run/owner.sock`, holds no keys, writes no files, never resends owner input, and is never linked into `init-root`. It uses the owner channel's frames and does not redefine them. |
| Body stdio protocol | kernel/supervise.md | `INIT_UI=stdio-jsonl/1`. Frames carry ids so the body can drop duplicates. The Root spawns every agent-zone process, including bodies and probes, because macOS cannot nest sandboxes (exit 71, tested). The kernel requests spawns over the Root↔kernel socket. |
| `ready` and `heartbeat` formats; `lkg.json`, `boot.json`, `update-failed.json`, `supervisor.json`; `vault/lineage.jsonl` envelope; `alerts.jsonl`; refs `refs/init/*` and `refs/upstream/*` | kernel/supervise.md | |
| Git authority | kernel/supervise.md | `lineage/body.git` is authoritative and written only by the kernel, through a sandboxed `git-upload-pack` fetch and CAS `update-ref`. The kernel never runs git in `body/`. |
| initctl wire protocol | kernel/initctl.md | JSON lines over `run/kernel.sock`. Request: `{"v":1,"id","cmd","args"}`. Response: `{"v":1,"id","ok":true,"result"}`, or `{"ok":false,"error":{"code","message","details[]"}}`. Also defines progress frames and the job model, and uses dotted error codes. |
| Sandbox profiles (Seatbelt, Landlock, seccomp, bwrap): `body`, `build`, `git-read` | kernel/sandbox.md | |
| Proxy routing, lanes, auth, caps, stats, traces | kernel/proxy.md | `POST /<provider_id>/<path>`, checked against a per-provider allowlist. Lanes (proxy scopes) are bound to sockets. The headers `x-init-session`, `x-init-purpose`, `x-init-attempt` and `x-init-task` are read and stripped by the proxy. A cap refusal returns 429 with `x-init-cap` and `type: init_cap_exceeded`. `GET /_init/stats` uses integers only (ICJ). Seed providers: `deepseek`, `anthropic`, `openai`; `mock` is available in `proxy.test_mode` only. |
| `tools_sha256` (`init-tools-v1`), `head_bytes_hash` | kernel/proxy.md | |
| `config.toml` file format, validation and assembled schema | kernel/config.md | Section semantics are owned as follows: `[model]`, `[proxy]`, `[budget]` money keys, `[traces]` and `[providers.*]` by proxy.md; `[gate]` by gate.md; `[budget]` count keys and `[evolution]` by evolution.md; `[supervise]` by supervise.md; `[sandbox]` by sandbox.md. |
| Gate stages, `task.json` v1 (task, holdout, canary), accept rule, statistics, gate record (`kind:"gate"`), `gate.*` codes, notes | kernel/gate.md | Notes are written in `lineage/body.git` under `refs/notes/gate`. `refs/notes/predict` is used by evolution.md. |
| `self.json` schema, generator, `@init compensates-for` marker grammar | kernel/selfmap.md | Component ids are defined in evolution.md §3.1. |
| Provider adapters, normalized stream types, mock script format | body/provider.md | |
| Agent loop, steering, abort, parallelism, step boundary | body/loop.md | |
| Tool schemas and descriptions (`read`, `write`, `edit`, `bash`, `ext`) | body/tools.md | |
| Body boot, modes, `--manifest`, **seed build configuration** (`seed/Cargo.toml`, `.cargo/config.toml`, profiles) | body/boot.md | |
| Event log envelope and types; **INIT canonical JSON (ICJ v1)**; **`prefix_hash` (head hash)**; `error.code` registry | body/log-context.md | Envelope: `{"id","parent_id","ts","type",…}` in ICJ (sorted keys, integers only). proxy.md recomputes `prefix_hash` and gate.md checks it. |
| Context assembly, cache layout, masking, compaction | body/log-context.md | |
| Genesis prompt, CONSTITUTION text, `SELF.md`, environment snapshot, self-signals | awareness.md | |
| WIT world `init:ext@1.0.0` | ext/wit.md | Package `init:ext@1.0.0`, world `tool` exporting `describe` and `call`, plus interface `process`. Authoritative file: `kernel/wit/init-ext/v1/init-ext.wit`. Minor versions are additive only. The body supports majors N and N−1. |
| Extension host (Wasmtime), catalog, reload protocol | ext/host.md | |
| Extension authoring workflow and template | ext/authoring.md | |
| Tier flows, version state machine, task identity, commit grammar (a typed subject plus trailers), validator codes, predictions and the content of `refs/notes/predict`, retirement, thinning, upstream (the `lineage/seed.git` sync policy), the body resync procedure | evolution.md | Agent identity: `INIT Agent <agent@init.local>`. Trailers: `Why`, `Predicts`, `Compensates-for`, `Prefix-change`, `Repro`, `Retires`, `Thins`, `Upstream`, `Reverts`. Task id = the id of the owner input that started the run (chosen by the body). Kernel refusals use `E_*` codes, for example `E_PROVISIONAL` and `E_QUOTA`. |
| Acceptance tests A1–A6, CI, the test harness (`init-testkit`, mock harness and responders; the mock script format is in body/provider.md §3.12), the `INIT_FAULT` mechanism (each owning doc places its own fault points), Phase 0 measurement procedures | testing.md | |
| Build plan, milestones, work packages | roadmap.md | |
| ADRs | adr/NNN-*.md | ADR-008 (owner-machine mode: held-PID signals only, live-sweep compiled out, sweep dry-run; lane R-SAFE) is added for v0.1.0; ADR-009 (Codex-style permissions: home read, workspace write, `--full`, snapshots and `/undo`) for v0.1.1 |
| Release process: `scripts/{release,install,uninstall}.sh`, release profiles, the install path | release.md (lane REL) | v0.1.0 (RELEASE_PLAN.md) |
| **Reports and proposals** (no contract; exempt from §1's section list, but they keep the header block) | spec-v0.4-proposal.md (every doc's Spec deltas and v03 D1–D14, folded against the spec); phase0-report.md (the M6 report); reconciliation/build-deviations.md (build → docs ledger) | M6 DoD items (roadmap.md M6) |

## 5. Global decisions

Docs do not re-open these. If a doc disagrees, it raises an open question instead.

- **Language:** Rust 2024; rustc 1.97.1 is the local toolchain.
- **Body code:**
  - Every crate in the body workspace carries `#![forbid(unsafe_code)]`.
  - There is **no async code in the body**; it uses std threads only. The tokio runtime that `wasmtime-wasi` builds internally for WASI I/O, and Wasmtime's rayon pool used during compilation, are accepted as internals of allowed dependencies.
  - Extension-workspace crates (Wasm guests) are exempt from the unsafe ban. They must be `cdylib`-only, and nothing in the body workspace may depend on them.
- **Body dependencies (vendored):** `serde`, `serde_json` (never with the `preserve_order` feature), and `wasmtime` and `wasmtime-wasi` pinned at **`=48.0.6`**. 48.0.6 is the LTS release with security support until about 2028-08; every doc and the roadmap use it, not 49.x. The extension workspace uses `wit-bindgen =0.63.0`, and it is never linked into `init-body`.
- **Kernel dependencies:**
  - tokio, hyper, hyper-util and http-body-util (proxy); TLS (ureq/rustls) lives in the Root only (root/egress.md §6)
  - syn 3.0.9 and proc-macro2, with the `span-locations` feature (selfmap)
  - wit-component and wit-parser 0.262.0
  - tiktoken-rs 0.12.1 (`o200k_base`)
  - wasmtime and wasmtime-wasi `=48.0.6`, used only in the `selfmap --describe` child process
  - toml 1.1.8
  - sha2 0.11.1

  The kernel's serde_json also never enables `preserve_order`.
- **Root build profile:** `root/Cargo.toml` builds `sha2` and its hashing crates (`digest`, `block-buffer`, `crypto-common`, `hybrid-array`, `cpufeatures`) at `opt-level = 3` in the dev profile, so tuple hashing of the debug kernel is fast. The Root itself stays a plain debug build, and release is unchanged.
- **Determinism (ICJ):** anything that reaches a model prompt, the event log or a hash uses INIT canonical JSON. That means sorted keys and integers only: money in micro-USD (`*_musd`), ratios in permille, durations in ms. `HashMap` order never reaches a prompt.
- **Money:**
  - In config, money values are decimal strings.
  - Internal accounting uses integer pico-USD.
  - Any money value that reaches a prompt, a log or a note is integer micro-USD.
- **Keys:** API keys live only in the Root (root/egress.md, `init-root keys set`). They never appear in the agent zone, in kernel config or in `vault/`.
- **Providers:** DeepSeek is the first real provider for the Phase 0 flow test, and the owner supplies the key. Current models are `deepseek-flash` (V4.1-Flash-1017) and `deepseek-v4-pro`; `deepseek-chat` and `deepseek-reasoner` were retired on 2026-07-24. Anthropic Messages and OpenAI Responses follow. Only frontier-class tool-calling models are supported, and the model id is checked at boot.
- **Platforms:** macOS on Apple Silicon (M2) is primary for Phase 0. Linux comes second.
- **Disk:**
  - Keep at least 20 GB free throughout the build phase.
  - M0 needs at least 35 GB free to start: the 20 GB floor plus a build peak of about 15 GB (roadmap.md). On 2026-10-19 the machine had about 9.6 GiB free, so the build is blocked until the owner frees space.
  - Delete files only by their exact path.

## 6. Owner decisions

**v0.3 (2026-10-20):** the owner chose a minimal immutable Root, with the agent autonomous for everything else (ADR-007). Human review is removed. ESC-1 is settled as no approval for crates, and ESC-2 as `/opt/homebrew` readable; both are L0 rights. OW-1 (the constitution text) is being rewritten for the Root and the ladder. Ceilings E1/ET-E1 move into the charter (root/root.md). Charter template (owner, 2026-10-20): `max_level = 4`; `/panic` demotes one level (`stop` does not); lineage mirror to the private repo `nagisanzenin/INIT-Lineage`.


**Decided by the owner after v0.3:**

| Id | Date | Decision | Docs |
|---|---|---|---|
| OW-TUI | 2026-10-20 | Add a terminal UI, the command `init`, in the style of Claude Code. It is a separate client outside the Root and talks to the Root over the owner socket (`ROOT_HOME/run/owner.sock`), the same channel `init-root attach` uses. The Root stays small, and the TUI has no special trust: it holds no keys and reaches nothing but the owner socket. The agent may later evolve the UI, through its body's output or through proposals, never by giving the TUI trust. | tui.md; roadmap.md WP26 |
| OW-PERM | 2026-10-20 | Codex-style permissions (v0.1.1). The live body reads the owner's whole home except a secrets deny list, and writes `INIT_HOME` plus the workspace, the directory `init` was run in, as validated by the Root. `init --full` or `/full on` (typed confirmation, charter `sandbox.allow_full`) makes the whole home writable except the deny list and the Root's paths. The Root takes an APFS snapshot of the workspace before the first writing turn, and `/undo` restores it. Charter keys `sandbox.read_home`, `read_deny`, `allow_full`. The Root, keys, lineage, ceilings, gate and network are unchanged. | ADR-009; kernel/sandbox.md; root/root.md §3.17; tui.md; body/boot.md; awareness.md |

**Provisional owner decisions (2026-10-20 12:16, release push).** The owner said not to wait ("chủ động làm đi đừng chờ tôi"), so the orchestrator took these for v0.1.0 (RELEASE_PLAN.md; build-deviations.md §3). Each is a **provisional owner decision 2026-10-20 (owner may override)**, and every doc that carries one says so.

| Decision (build-deviations §3) | Taken as | Docs |
|---|---|---|
| §3.1 Root size | Budget ≈ 7,500: per-module sub-budgets of measured + 5%, rounded up to 10, re-measured at release. The core (zone, sup, panic, egress money, lineage verify) is counted separately, and `root/sandbox-profiles` has its own lines. At cdb49e8: 7,085 measured (table total 7,540), core 1,656 (budget 1,770). | spec §13, root.md §4.8 and RR-34, egress.md §6, sealed-eval.md §4.2, roadmap M6 and R13, §4 above |
| §3.2 G4 power | n = 30 (18 + 12), R = 3, plus one A/A calibration per model, as the default once S has ≥ 30 tasks (M7). v0.1 keeps the 12-task stand-in S. | gate.md §3.8.3 and §3.8.5, roadmap M7 |
| §3.4 Constitution | Built as written. It stays PENDING OWNER (OW-1), and the owner should still read it. | OW-1 below |
| M-5 method (FIX8) | `phys_footprint` on macOS, baseline at the first context epoch, thresholds unchanged | testing.md M-5 |
| §3.9 Repeated demotion | 3 demotions at L0 within 24 h trigger system safe mode | root.md §3.13.3 |
| §3.13 Root money meter | Bill cache hits at the provider's cache-read price; keep the peak window | egress.md |
| §3.15 Small S | S < 12 tasks gives `underpowered`, with no demotion, plus an alert | sealed-eval.md |
| §3.17 Real-model batch | deepseek-flash only. Hard cap $1.00 Root-metered; no new checks after $0.50 Root-metered. | build-deviations §3.17 |
| Disposable account | Not required for v0.1, under owner-machine mode (ADR-008) | ADR-008, build-deviations §3.14 |

**Provisional owner decisions (2026-10-20, M7 design).** Taken under the same "do not wait" instruction by docs/m7-design.md (§3.0, §10) and folded by lane DOC-M7. Each is a **provisional owner decision 2026-10-20 (owner may override)**.

| Id | Taken as | Docs |
|---|---|---|
| D-M1 | S v1 = pack `s1`: maintainer-written generators shipped only inside the Root release; each Root draws its own instances from seeds that never leave `ROOT_HOME` (32 G4 + 24 held-out reserve + 4 canaries, targeted to a 20–80% incumbent pass rate) | root/sealed-eval.md §3.1.1 |
| D-M2 | G4's gain test runs on S in the Root (`sealed.g4_slice`); the visible pool is a reject-only screen | kernel/gate.md §3.7.1a, §3.9; root/root.md §4.1.3 |
| D-M7 | At most 20 valid S looks per S version, one per candidate binary; 25% rotation per epoch; the reserve is scored only at milestone checks | root/sealed-eval.md §3.1.1, §3.5.1 |
| D-M9 | M7 is planned in **real** dollars against the DeepSeek balance: ≤ $3.00 real of the owner's $5, off-peak only, a 10-run cost probe first; the M7 test home's Root caps sit at 2× (Root-metered $6.00) as a safeguard while the Root bills at peak price | m7-design.md §3.6; kernel/gate.md §3.11; evolution.md §3.14 |
| D-M10 | The lineage ratchet (`lineage_gain`, at most 4 unconfirmed per span) and the milestone check on the untouched reserve, with the new `confirmed` pointer | kernel/gate.md §3.9; root/sealed-eval.md §3.5.1; root/root.md §3.7.2 |
| Root growth (M7) | ROOT-S1's ≈ 360 lines raise the `sealed/` sub-budget (840 → ≈ 1,100), set at release as measured + 5% | root/root.md §4.8 |
| Dev checkout in `read_deny` | The dev checkout path is in the owner's charter `sandbox.read_deny`, and an install from a dev checkout (`--seed-source`) writes it there. With ADR-009's `read_home` the live body could otherwise read the S pack generators in `~/Documents/INIT Agent/root/sealed/packs/` and their `.git` objects. The workspace rule then refuses the checkout as an `init-agent` workspace; INIT is developed with Claude Code, not with itself. The owner may remove the entry. | root/root.md §3.12.2; RELEASE_PLAN.md v0.1.1; m7-design.md §3.1.2 |

**Pending.** Docs carry the recommended value as a provisional default, marked `PENDING OWNER (CONVENTIONS §6 <id>)`.

| Id | Decision | Recommended | Ledger |
|---|---|---|---|
| OW-1 | Approve the CONSTITUTION.md text (awareness.md §3.3) | Approve as written. The owner is reading it first. v0.1.0 ships it as written (provisional owner decision 2026-10-20, above). | body-log-awareness |
| OW-2 | Approve the genesis prompt text (awareness.md §3.2) | Approve as written. The owner is reading it first. | body-log-awareness |
| E1 / ET-E1 | Phase 0 money. Hard ceilings belong in the **charter** (root/root.md): $5.00/day by default, $10.00 on days that use a real model, and a top-up of $25 at DeepSeek as the hard total. Soft defaults stay in kernel config: `gate_usd` $3.00, task $0.50, `evolution_share` 0.6. | As listed | kernel-proxy-gate, evolution-testing |
| ET-E2 | Which model runs G4 | `deepseek-flash` within the $3 gate cap. Use `deepseek-v4-pro` only for component trials and real acceptance runs (G4 on v4-pro costs about $13). | evolution-testing |
| ESC-1 | Does `initctl vendor` (adding new crates) need owner approval? | No. Allow only crates.io and exact versions, at most 20 per day; refuse while provisional; run the full G4. | kernel-supervise |
| ESC-2 | Make `/opt/homebrew` readable in the sandbox, except `var/` and `etc/`? | Yes | kernel-supervise |
| ET-E4 | CI hardware | Run CI locally first (`ci/local.sh`) and decide at M7. The alternative is hosted macOS runners on release tags only, about $7/month. Linux Landlock check S-L8: run it once on a short-lived Linux ≥ 7.1 VM at M7, or defer. | evolution-testing |

**Settled by the main session (2026-10-19):**
- Only probe-mode boots make the probe model round trip; normal and safe boots do not (body-core E1).
- The G4 time limit is 90 minutes, with 3–4 runs in parallel (kernel-proxy-gate E2).
- M0 requires at least 35 GB of free disk (evolution-testing E3; §5).
- Body defaults (body-core R2 Q-P5, Q-P6, Q-T5, Q-T6, Q-L5, Q-L6, Q-B8), all kept as the docs propose:
  - Anthropic server-side fallbacks stay **off**, so no silent model switch can happen inside a gate measurement.
  - DeepSeek effort defaults to `high`.
  - bash's default timeout is 120 s; the model passes `timeout` explicitly for long builds.
  - Background jobs with unredirected output are killed after 2 s, with an explanation to the model.
  - A plain owner message sent during a run acts as a **steer**; `/later` queues a follow-up instead.
  - A length stop with no tool calls continues automatically, up to 3 times in a row.
  - The G2 probe uses one real, mostly cached model call pair per arm, per spec §6.3.

## Spec deltas

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.2):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP0 D1 | The dev checkout uses the single repo-root `vendor/` (each `.cargo/config.toml` points at `../vendor`). The seed repo and the installed body carry their own `vendor/` with `directory = "vendor"`. | §2 |
| build: WP0 D4, WP5 Done | Kernel dependencies are tokio, hyper, hyper-util and http-body-util. TLS (ureq/rustls) lives in the Root only. kernel-proxy no longer uses tokio-tungstenite or rustls. | §5 |
| build: WP13 D4, WP20 Done, WP12 D13 | There is no repo-root `CONSTITUTION.md`. The canonical file is `root/crates/init-root/assets/CONSTITUTION.md`, with two byte-identical copies (`seed/src/boot/`, `seed/src/context/testdata/head/`). A `ci/local.sh` drift check is wanted. | §2 |
| build: FIX4 (orchestrator decision) | `root/Cargo.toml` builds `sha2` and its hashing crates at `opt-level = 3` in the dev profile. Release is unchanged. | §5 |
| build-deviations §3 (provisional owner decisions 2026-10-20, owner may override) | §6 carries RELEASE_PLAN's decision table. §4's Root row has the ≈ 7,500 per-module budget. §4 registers ADR-008, release.md and the two M6 reports (spec-v0.4-proposal.md, phase0-report.md). | §4, §6 |
| build-deviations §6.4 (G0-ROOT eec8d5e) | `init-mockllm` ships in every install's `kernel/bin/` (G0 runs the seed's tests against it); only `init-fakebody` needs `--with-testkit` | §2, §3 |
