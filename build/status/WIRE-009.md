# WIRE-009: ADR-009 permissions, the client, kernel and body side

Status: **done** except the `seed/self.json` bless, which the orchestrator deferred for disk (2026-10-20, lane WIRE-009). Spec: `build/specs/ADR-009-spec.md`; the Root's protocol:
`build/status/SBX-009.md` "Protocol for WIRE-009".

## Done
- **TUI** (`tui/crates/init-tui`, fc52bb6, f0be4e4)
  - `src/access.rs`: a pure state machine for the owner's side of SBX-009 §A.
    - **Workspace.** The cwd (as the OS gives it) goes as `workspace.set{cwd}` on its own connection right after the
      first attach. Owner lines wait until it is answered. It is sent once per launch.
    - **Full mode.** `--full` sends `full.set{on:true}` after the workspace. `/full on` and `/full off` are
      `full.set`.
    - **Undo.** `/undo` sends `undo{}`.
    - **Confirmation.** A `root.confirm` error shows the Root's `message` and asks for `data.phrase`; the phrase is
      never hard-coded. The owner's next line is the answer. If it equals the phrase, the same request is resent
      with `confirm`; any other line cancels with "not confirmed; nothing changed".
    - **Never on the attach channel.** Access commands and confirmation answers never reach the attach channel as
      lines. While a request is in flight, other lines wait.
    - **Errors.** Refusals get a readable text: `no_workspace`, `root.not_found` (no snapshot) and `charter`
      (`allow_full = false`).
    - **No workspace.** The TUI shows the notice "no workspace: cd into a project and run `init` to let me edit it
      (<refused.message>)" only when `changed` is false. When the workspace did change, the Root's `code:"access"`
      notice already reaches every client, so the TUI does not repeat it.
    - **Older Root.** A Root that predates ADR-009 (`root.unknown_cmd`) is handled silently.
  - `/workspace`, `/full`, `/full on confirm` and `/undo confirm` typed by hand are Root lines: they are forwarded
    and expect no echo. The command popup lists `/undo`, `/full on`, `/full off` and `/workspace`.
  - **Status bar.** It reads `access` from `status` (and from each answer):
    - `FULL` in red bold (reverse with NO_COLOR), never dropped;
    - the workspace as a short path (`~/…`, at most 32 columns, `…/<last parts>`), or a dim `no workspace`;
    - the status is polled at once on notices with `code` `access` or `snapshot`.
  - The TTY hint shows "type <phrase> and press Enter to confirm; anything else cancels" while a confirmation is
    pending. Line mode prints the same as `[tui] …` lines. `--wait-idle` and EOF wait for pending access requests.
  - `main.rs`: `--full`, and the usage text explains the workspace.
- **Launcher** `scripts/init`: it already passed every argument and the cwd through (`exec`). It now documents
  `--full` and the workspace, and a test proves both.
- **Kernel** (28fd8e8)
  - **Protocol.** `init-proto`: `RootEvent::Access(AccessData{access, full, restart, workspace})` (event `access`,
    SBX-009 §D). The `hello` result carries an optional `BootBlock.access`. Unknown fields are kept.
  - **Relay.** `kernel-supervise`: the access is taken from `hello`, else from the kernel's own
    `INIT_WORKSPACE`/`INIT_ACCESS`, and updated by every `access` event. Live body spawns get `INIT_WORKSPACE`
    (`""` when there is none) and `INIT_ACCESS`. Probes and fleet members get neither from the kernel, and the
    Root sets theirs. No new spawn params are sent.
  - **Restart.** On `restart:"idle"`, `machine.rs` `Ev::Restart{cause}` waits for the primary to be ready and idle,
    with no handoff, rollback or owner stop under way. It then sends `stop{reason:"restart"}` and, at the exit,
    spawns the same version as `handoff` with `boot.json.cause.kind = "workspace_change"` and the same session. It
    is not counted as a crash (crashes_total and restarts_since_lkg stay 0). After stop grace + exit grace the body
    is killed. Any other new primary spawn drops a pending restart, because it reads the new env anyway.
- **Body** (seed, 6a47d11; done by a helper of this lane)
  - **Env.** `boot/env.rs`: `BootEnv.workspace` and `access` come from `INIT_WORKSPACE`/`INIT_ACCESS`, leniently: a bad
    value gives a boot-facts warning and never exit 78.
  - **Tools.** `ProcFacts.cwd` is the workspace when it is a directory. It is what reaches the tools'
    `PathCx.cwd` (read/write/edit relative paths, the bash `current_dir`, the ext workspace). The process cwd stays
    under INIT_HOME, as the Root requires (SBX-009 §D: the tools chdir to `$INIT_WORKSPACE`).
  - **Boot block.** `boot/awareness.rs` adds one `- access: …` line right after `- ceilings (Root): …`. It goes in
    the boot event (log-context §3.8.2: nothing volatile in the head; the grant is fixed for a body's lifetime, so
    not the signal line either). It is omitted when `INIT_ACCESS` is absent, so the old goldens are unchanged.
    The texts are frozen (DOC-009 copied them, T-AW-16):
    - `workspace`: `- access: write the workspace <path> and INIT_HOME (your tools start in the workspace) · read the owner's home, secret stores denied`
    - `read_home`: `- access: no workspace (init was launched outside a project): write INIT_HOME only · read the owner's home, secret stores denied`
    - `none`: `- access: INIT_HOME only (no home access; the v0.1 sandbox)`
    - `full`: `- access: FULL: write the owner's whole home except secret stores and the Root · workspace <path|none>`
    - any other value: `- access: <value> · workspace <path|none>`
    - a workspace that is not a directory at boot: the parenthetical reads `(it is not a directory now: your tools start in <cwd>)`
  - **Handoff cause.** `workspace_change` renders "your owner's workspace changed".

## Tests (all $0)
- TUI: `cd tui && cargo test --offline --bin init-tui --test proto --test tty`, run through `ci/slot.sh` after
  `source ci/env.sh`. Results: 52 unit (incl. 6 `access::tests`, the golden `wire9_access_in_the_status_bar` →
  `tests/golden/wire9_access_status.txt`), 12 proto (incl. `wire9_workspace_first_undo_and_full_confirmations`,
  `wire9_home_has_no_workspace_and_full_flag_confirms`, `wire9_launcher_passes_flags_and_cwd_through`), 2 tty
  (incl. `wire9_tty_undo_and_full_with_confirmations` through a pty). All pass. clippy `--all-targets -D warnings`
  is clean.
- Kernel: `cargo test --offline -p init-proto -p init-testkit -p kernel-supervise` over every target except
  `promote_seatbelt` (see Needs). Everything passes: machine tests `restart_waits_for_idle_and_boots_handoff_with_the_cause`,
  `restart_kills_a_body_that_ignores_the_stop` and `restart_without_a_running_body`; spawnspec
  `body_env_relays_workspace_and_access`; init-proto `access_event`; the relay test `tests/access.rs`
  (`relay_and_graceful_restart_at_idle`: relayed from `hello`, no restart while busy, the restart after the turn
  ends, same session, `handoff` + `workspace_change`, exit 0, 0 crashes, a line after it reaches the new body, an
  unchanged event restarts nothing; `no_access_from_the_root_sets_nothing`). clippy is clean.

- Body: `cd seed && INIT_MOCKLLM_BIN=../kernel/target/debug/init-mockllm cargo test --offline`: 280 + 17 + 9 pass, 0
  fail (env leniency, process_facts cwd, the four line forms and their position, the workspace_change reason, tools
  writing and `pwd` in the workspace, and a boot e2e with the mock: a relative write and `pwd` land in the workspace,
  then a `workspace_change` handoff, then a `none` boot). clippy is clean; the T-AW-3 volatility lint passes.
- **e2e** `tui/crates/init-tui/tests/e2e_root.rs::wire9_workspace_tool_write_and_undo` ($0):
  - The setup is the REAL `init-root` (test-fixtures) with a Python fake kernel (`tests/support/w9_kernel.py`,
    stdlib only; it speaks the control link with SCM_RIGHTS like the Root's Rust fake kernel). Its body is `/bin/sh`
    under the real `body` profile, and `init-mockllm --strict deepseek` is the model.
  - `init-tui --plain` is launched in a fixture project. The Root announces `[root] workspace: <proj>`, sends
    `access{restart:"idle"}`, and the fake kernel restarts the body, which reports `ws=<proj> access=workspace`.
  - `!tool …` makes one chat request through the Root's egress; the mock answers a `write` tool call, and the
    sandboxed body writes `notes.txt` in the workspace.
  - `/undo` brings the Root's phrase; it is typed back and the file is restored. Neither `/undo` nor the phrase
    reached the kernel. There are 2 snapshots (`session`, `pre_undo`).
  - It passes against SBX-009's committed Root (cc16577, test-fixtures build), together with
    `tui_r_real_root_session` (1fed6ce). Run it with `cd tui && INIT_TUI_E2E=1 INIT_ROOT_BIN=… INIT_MOCKLLM_BIN=… cargo test --offline --test e2e_root`
    (ci/local.sh's "tui e2e" step runs it as is). Both e2e TUIs now run
    from the test HOME, so no workspace is set and nothing of the source tree is snapshotted.

## Deviations
- The TUI does not repeat the Root's own `code:"access"` no-workspace notice. It shows its own only when the answer
  says `changed:false`, so the owner sees the notice exactly once.
- The e2e's fake kernel is Python, not the Root's Rust `init-root-fakekernel` (root/**, SBX-009's): a "mock tool
  call that writes in the workspace" needs a model call through egress and a shell body, and the TUI crate has no
  libc for SCM_RIGHTS. The real `init-kernel` path (relay, idle restart) is covered by `kernel-supervise/tests/access.rs`.
- `/undo` while the agent is busy (SBX-009 refuses it with `root.busy`) gets a readable warning in the TUI.
- A Root without `workspace.set` is handled silently. Nothing changes for the owner then (v0.1 access), and the
  existing line-mode golden stays byte-identical.

## Needs from others
- (Resolved) SBX-009's 8570638 broke `kernel-supervise/tests/promote_seatbelt.rs`; SBX-009 fixed it in da210eb. The whole kernel workspace compiles its tests again.
- **CI (ci/local.sh, orchestrator):** the "seed test" step does not pass `INIT_MOCKLLM_BIN`, so `seed/tests/boot.rs`'s
  mock tests (the new ADR-009 one included) skip there.
- **seed/self.json bless:** deferred by the orchestrator (disk); "seed G0 + G1" stays red until it runs (the SELFJSON
  command; it needs about 1.8 GB transient).
- **DOC-009:** the kernel's restart sends `stop{reason:"restart"}`, and the new body boots `handoff` with
  `boot.json.cause.kind = "workspace_change"`.

## Left
- The `seed/self.json` bless, deferred by the orchestrator for disk. The seed tree is final at 6a47d11. The command
  is the one in build/status/SELFJSON.md (about 1.8 GB transient in `seed/target/g0`, which is removed after).
