# ADR-009: Codex-style permissions (read the home, write the workspace)

Status: accepted by the owner on 2026-10-20 (target v0.1.1) · Area: root
Depends on: ADR-007, ADR-008, build/specs/ADR-009-spec.md (the binding spec), kernel/sandbox.md (§3.1, §3.11), root/root.md (§3.3, §3.12, §3.17, §4.2), tui.md, body/boot.md, awareness.md   Defines: the agent zone's read and write scope from v0.1.1, the workspace and its validation, the access modes `none`, `read_home`, `workspace` and `full`, the secrets deny list, workspace snapshots and `/undo`

## Context

The owner used v0.1.0 and found its sandbox too strict for daily work. A live body could read only the system paths, the toolchain and `INIT_HOME`, and write only `INIT_HOME`. Asked to look at a project in `~/Documents`, the agent got `EPERM`; to let it help, the owner had to copy the project into `~/.init/work` and copy the result back. Other agents read freely: Claude Code and Codex CLI read the whole disk by default and write the project the owner started them in. The owner's words: "các agent hiện tại đều freely traverse" (today's agents all traverse freely).

The v0.1 policy came from kernel/sandbox.md §3.2, which departed from Codex on purpose ("the agent has no business reading the owner's home; secrets stay out of model prompts too"). Since v0.3 (ADR-007), though, the only security boundary is the Root against everything else. The instance profiles are a stability layer, and the zone tail is what protects the Root: its home, the keys, the lineage, the ceilings and the gate. The explicit read allowlist protected the Root, but it also locked away the owner's own files, which are what the owner wants the agent to work on. Whether the agent may read or edit them is not a Root question. It is a usability and privacy choice that belongs to the owner, and the owner chose the Codex default.

## Decision

From v0.1.1 the live body works in Codex style. The Root, the keys, the lineage, the ceilings, the gate and the network stay exactly as they are.

1. **Read: the owner's whole home**, plus the system paths that are readable today, **except a deny list** that holds even under `--full`:
   - `ROOT_HOME`, `~/.config/init-agent`, `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/.kube`, `~/.docker/config.json`, `~/.netrc`, `~/.npmrc`, `~/.pypirc`;
   - `~/Library/Keychains`, `~/Library/Cookies`, `~/Library/Mail`, `~/Library/Messages`, `~/Library/Safari`;
   - `~/Library/Application Support/{Google/Chrome,Firefox,BraveSoftware,Microsoft Edge,Arc,1Password}` and `~/Library/Group Containers` (password managers and messengers).

   The charter adds entries with `sandbox.read_deny` (at most 32). `sandbox.read_home = false` turns the home read off; with no workspace and full mode off, that is the v0.1 behaviour (only `INIT_HOME`, plus `sandbox.extra_read`).
2. **Write: `INIT_HOME`, as before, plus the current workspace.** The workspace is the directory the owner was in when they ran `init-agent`; the TUI sends its cwd once per launch (`workspace.set`). The Root accepts it only when it is absolute and canonical (symlinks resolved), exists, is a directory inside the owner's home, is not the home itself, neither lies inside nor contains `ROOT_HOME`, `INIT_HOME` or `~/Library/LaunchAgents`, and is not on the deny list (root/root.md §3.17.1). Otherwise, or when `init-agent` was started in `~`, there is no workspace: the home is read-only, and the Root's notice says "cd into a project and run init-agent to let me edit it". The body's `TMPDIR` stays under `INIT_HOME`, as today.
3. **`--full` mode.** `init-agent --full`, or `/full on` in the TUI, makes the whole home writable except the deny list and the Root's paths; `/full off` ends it. The first time in each owner session the Root asks for a typed confirmation (`/full on confirm`; the phrase comes from the Root). The charter turns the mode off with `sandbox.allow_full = false` (default `true`). The status bar shows `FULL` while it is on. Each new `init-agent` launch starts a new owner session, with full mode off.
4. **Undo safety net.** When the workspace is set, before the first agent turn can write it, the Root takes an APFS clone snapshot of the workspace (`clonefile`, as `cp -c -R` does) into `ROOT_HOME/snapshots/<ts>-<name>/`. When full mode turns on, it snapshots the workspace again, or warns that full mode has no snapshot when there is no workspace. `/undo` is an owner command handled by the Root over `owner.sock`. After a typed confirmation (`/undo confirm`), it restores the latest snapshot, first cloning the current state into a new snapshot, so an undo can itself be undone. The Root keeps the last 5 snapshots per workspace, skips a workspace over 2 GiB, one not on APFS or one on another volume (with a notice), and never lets a snapshot leave the machine.
5. **Workspace change.** Running `init-agent` in another directory changes the workspace. A body's profile is fixed at spawn, so the Root sends the kernel an `access` event, and the kernel restarts the live body gracefully at its next idle boundary. The conversation continues, because the body resumes its session log, and a notice says `workspace: <path>`.
6. **The agent knows its permissions.** The Root sets `INIT_WORKSPACE` (empty when there is none) and `INIT_ACCESS` (`full`, `workspace`, `read_home`, or `none` when `sandbox.read_home = false` and there is no workspace) on every body spawn, overwriting anything the kernel sent. The body's tools start in the workspace when one exists. The boot block states the access in one short line. It sits at the tail, so the cached head stays byte-stable (body/log-context.md §3.8).

**Where it is enforced.** The Root generates two grant lines in the instance's ROOT_ALLOWS: a read of the home (`Z_HOME`) and a write of the workspace, or of the home in full mode (`Z_WRITE`). Both exclude `INIT_HOME`, so inside it the instance profile still rules. The zone tail, appended last to every agent-zone profile, then denies each deny-list entry and all writes to `~/Library/LaunchAgents` (Z2), and confines writes to `INIT_HOME`, `Z_WRITE` and the devices (Z3), minus the files the owner's own tools run (Z3a, Z3b; below). Every `Z_*` value is the Root's (V1); the kernel sends the same `spawn` parameters as before and cannot widen any of this. Grants by instance:
- the live body gets the home read and the write grant;
- fleet members get the home read only;
- probes, which run G4 tasks, get neither, so a measurement never depends on the owner's files and never writes the owner's project;
- the kernel, `build`, `check` and `git-read` are unchanged, except that the deny list now applies to them too.

The exact SBPL is lane SBX-009's (`root/sandbox-profiles`), and `tests/doc_drift.rs` keeps kernel/sandbox.md §3.3.1 and root/root.md §3.3.1–§3.3.2 byte-identical to it. The wire protocol is in root/root.md §3.17 and §4.2.

## Threat model delta

**New risk: privacy.** Any file the agent reads may be sent to the model provider, as with Claude Code or Codex. Mitigations:
- the deny list keeps credentials, keychains, browser profiles, mail, messages and password managers unreadable, even under `--full`;
- egress is still provider-only through the Root (Z6): the zone has no IP network, so nothing it reads can go anywhere except the charter's model providers;
- the money ceilings bound how much it can send.

Residual: everything else in the home can reach the provider, including a project's `.env` files and documents the owner considers private. The owner adds paths with `sandbox.read_deny`, or sets `sandbox.read_home = false`.

**New risk: damage.** The agent can change the owner's project with no review. Mitigations:
- the write scope is the workspace, not the whole home, unless the owner turns on `--full`;
- the APFS snapshot taken at every `init-agent` launch in the workspace, plus `/undo`, which is refused while the agent is busy;
- git, when the workspace is a repository;
- **no deferred execution through the obvious paths.** A file the agent writes runs with the owner's full rights, outside the sandbox, if the owner's own tools run it later, and such a file could reach the Root's files. So in every mode, `--full` included, the tail write-denies (Z3a) the shell startup files (`.zshrc`, `.zprofile`, `.zshenv`, `.zlogin`, `.zlogout`, `.bashrc`, `.bash_profile`, `.bash_login`, `.bash_logout`, `.profile`, `.config/fish`), git's user config (`.gitconfig`, `.config/git`), `~/.local/bin` and `~/bin` (so the `init-agent` launcher and `init-tui`, which tui.md's T-4 relies on), `install.json`'s `bin_dir`, and login items (`~/Library/LaunchAgents`, `.config/autostart`, `.config/systemd/user`, `.config/environment.d`, `.pam_environment`). It also denies (Z3b) the `.git` entry itself and git's config, hooks and command sources inside any git directory (`config`, `config.worktree`, `commondir`, `gitdir`, `hooks/` under `.git/`, `.git/modules/<m>/` and `.git/worktrees/<w>/`), plus the `modules`/`worktrees` admin directories and any gitdir entry inside them (so a whole gitdir cannot be swapped in by a directory rename, which Seatbelt judges by the directory paths, not the files inside; the `refs/`/`logs/` ref namespaces are excluded), in every repository outside `INIT_HOME`, as Codex re-protects `.git`; the rest of `.git` stays writable, so commits work. A workspace inside one of these entries is refused. (Widened in v0.5.1: the original Z3b only matched `.git/hooks`, `.git/config` and the submodule equivalents, which left the owner's `git` reading a redirected config — see the residual below, now closed.)

Residual:
- `/undo` returns to the session's snapshot, so it also reverts edits the owner made by hand since then. The state it replaces is cloned first, so nothing is lost.
- A workspace over 2 GiB, one not on APFS or on another volume than `ROOT_HOME`, and the home outside the workspace in full mode have no snapshot.
- The busy check before `/undo` reads the last 5 s heartbeat, so a turn that starts during a restore can still write.
- **Deferred execution, what remains writable** (SBX-009's list; root/root.md §9.3 Q5):
  - in the workspace, in every mode: build and task files the owner runs (`Makefile`, `package.json` scripts, `build.rs`, `.envrc` for direnv, `.vscode/tasks.json`, `.idea/` run configurations, test files). (Closed in v0.5.1: a `.git` *file* (`gitdir:`) redirecting the git directory, renaming `.git` aside, a `commondir`/worktree redirect, or a `config.worktree` — Z3b now denies the `.git` entry itself and those config/command files in every git directory.)
  - in full mode: editor startup files (`.vimrc`, `.config/nvim`, `.emacs.d`, VS Code settings and extensions); language user-site and tool bin directories (`~/Library/Python/*/lib/python/site-packages` with `.pth` files, `~/.cargo/bin`, `~/.npm-global`, `~/go/bin`, `~/.rustup`); tools that run configured commands (`~/.tmux.conf` `run-shell`, `~/.hammerspoon/init.lua`, `~/.config/karabiner`); apps under `~/Library/Application Support/` that load plugins; and every directory on the owner's PATH other than `~/.local/bin` and `~/bin`. (`~/.ssh/config` is covered, because the read deny list denies writes too.)

  So full mode is about as trusted as the owner's own shell, and the owner should review what they run from a project the agent edited.

**Not changed.** The agent still cannot read or write `ROOT_HOME`, the keys, the signed lineage, the charter or the sealed set S. It cannot raise a ceiling, alter the gate, signal a process outside its sandbox, use XPC, or reach the network except the providers through the Root.

## Alternatives considered

| Alternative | Why not chosen |
|---|---|
| Full user access by default: read and write the whole home, as Claude Code and Codex allow in their most permissive modes | A mistake can damage anything in the home, and there is no snapshot beyond the workspace. Kept as the opt-in `--full`, with a confirmation, a charter switch and a visible `FULL`. |
| Keep the v0.1 sandbox and add a workspace clone: the agent edits an APFS clone of the project under `INIT_HOME/work`, and the owner merges it back | The safest option, since the owner's tree changes only when the owner merges. But it keeps the copy-in, copy-out friction the owner complained about, and the agent still cannot read the rest of the home. The owner chose Codex-style. |
| Read-only home with no workspace writes | It answers questions about the owner's files but cannot edit them. This remains the behaviour when there is no valid workspace. |
| Per-action approval prompts, as Claude Code does by default | Human review was removed in v0.3 (ADR-007), and the owner does not want prompts. The protection comes from scope plus undo, not from questions. |

## Consequences

**What stays unchanged:**
- the Root: `ROOT_HOME` (Z1) and the protected parts of `INIT_HOME` (Z4: `kernel/`, `root/`, `run/mark/`, `run/zone-mark`);
- the keys: `~/.config/init-agent/` and the Root's keystore;
- the signed lineage, the charter and the sealed set S;
- the money ceilings and the quotas;
- the gate and self-evolution: T0, T1 and T2 worktrees under `INIT_HOME/work`, and the gate's own instances (probes stay hermetic);
- the network: provider-only egress through the Root (Z6), no XPC and no cross-sandbox signals (Z7);
- ADR-008's owner-machine mode: the Root signals only the processes it holds.

**The Root grows.** Workspace validation, the access state, the typed confirmations, snapshots, `/undo`, three charter keys, the new owner-socket frames (`workspace.set`, `full.set`, `undo`, `access.get`) and the kernel `access` event are Root code, because each one decides what the zone may write. SBX-009 added about 400 lines in `init-root`'s `access.rs` and 350 in `root/sandbox-profiles`' `access.rs` (raw lines, before §4.8's counting rule). root/root.md §4.8 gets a sub-budget line for them, measured at the v0.1.1 release under the same rule (measured + 5%, rounded up to 10). The ≈ 7,500 total is a provisional owner decision, so the owner may raise it.

**Other consequences:**
- The Root writes a new directory, `ROOT_HOME/snapshots/`. Clones share blocks with the workspace, so a snapshot costs almost nothing until one side changes. They count toward the owner's disk like any other file, and the disk floor (`quotas.disk_min_free_gib`) still applies.
- A change of workspace or of full mode restarts the live body at an idle boundary (handoff cause `workspace_change`). The prompt cache keeps its head, and the new boot event shows the new access line.
- The TUI starts sending its cwd. It still opens no file and holds no key (tui.md T-1, T-2), and T-4 still holds in every mode, because Z3a denies `~/.local/bin`, `~/bin` and `bin_dir`. `init-root install --bin-dir` records a custom `bin_dir`.
- Every access change is a Root lineage record `access` with no path in it, because the lineage is mirrored.
- kernel/sandbox.md §3.2 no longer departs from Codex on reads, and on `.git` it now follows Codex too: hooks and config are write-protected outside `INIT_HOME`, while the body's own repositories inside it stay fully writable.

**Tests (all $0, all on test homes; none touches the owner's real `~/.ssh` or other files):**
- **Sandbox self-test rows** (kernel/sandbox.md §8, S-A1–S-A26b, in `init-sbx-selftest`; S-A20–S-A26b cover Z3a and Z3b): with a fake home in the test's scratch tree, which the profile parameters take as the home:
  - read a fixture file under the fake `~/Documents`: allowed;
  - read the fake `~/.ssh/id`: denied;
  - write the workspace: allowed;
  - write elsewhere in the home: denied;
  - write `ROOT_HOME`: denied;
  - in full mode: writing elsewhere in the home is allowed, while `ROOT_HOME`, deny-list paths, `~/Library/LaunchAgents` and `INIT_HOME`'s protected parts stay denied;
  - in every mode: shell startup files, git's user config, `~/.local/bin`, `~/bin`, login items and git hooks and config outside `INIT_HOME` are denied, while git objects and `HEAD` stay writable;
  - a probe reads nothing in the home and writes no workspace; a fleet member reads the home but writes no workspace.
- **Root** (root/root.md §8, RR-36–RR-40, built as `root/crates/init-root/tests/access.rs`): the workspace validation table; a snapshot and `/undo` round trip on a small fixture directory (an APFS clone); `/undo` refused while busy; the size and volume skips; full mode's confirmation and the charter switch; the Root's `INIT_ACCESS` and `INIT_WORKSPACE` overriding the kernel's.
- **Body** (awareness.md T-AW-16): the access line for each mode, with the head's `prefix_hash` unchanged.
- **TUI** (tui.md §8, TUI-15): one `workspace.set` per launch, and the `--full` confirmation flow.
- **End to end** (tui.md §8, TUI-R4): the TUI, the real Root in test mode and the mock. `init-agent` runs in a fixture project directory, the mock's tool call writes a file in the workspace, and `/undo` restores it.

**Docs updated:** kernel/sandbox.md, root/root.md, tui.md, body/boot.md, awareness.md, CONVENTIONS §6, README, CHANGELOG (v0.1.1) and RELEASE_PLAN.
