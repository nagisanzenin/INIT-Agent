# Changelog

All notable changes to INIT Agent. Versions follow [semver](https://semver.org). The design is `INIT Agent spec.md`; each line below points at the build status file that records it (`build/status/`).

## v0.5.2-preview (2026-10-29)

### TUI: what a first-time user saw that was not true (R-TUI; docs/user-sessions/v0.5.1-new.md, v0.5.1-returning.md)
- **The cold dependency build is its own step.** The first self-change of the compiled core spends 10–20 min building
  its dependencies inside G0; the gate track said `G0 build + tests □ 12 min 3 s` with no estimate, then
  `■ pass · 2 min 32 s · waited 17 min 46 s`, and a stranger read the build as idle waiting. The kernel now sends an
  owner-only `gate_deps` event when that build starts and adds `build_ms`, `deps_ms` and `deps_mode` to G0's owner
  notice (`kernel-supervise` `promote/stages.rs` `prepare_deps`, `promote/record.rs` `Record::owner_extra`; the body's
  copy of the notice is unchanged). The track shows `deps dependencies □ 12 min 3 s` with
  `first build of dependencies (not cached yet): usually 10–20 min on an M2` under it, then
  `■ built · 17 min 16 s · not cached before; later changes reuse it`, and G0's row is its body build plus tests, so the
  rows add up to the job record's G0 time.
- **`/panic` then `/resume`.** The status bar kept the dropped body's id (`body 9aa7d39624ce`) while `/status` named the
  one running: it now hides the body while the slot is panicked, halted or stopped and after a panic, resume, halt or
  crash-loop notice or a new kernel instance, until a fresh `state` frame or the kernel's `/status` line names it. A
  successful resume shows `✓`, not `✕`. When the running version is newer than the audited-good one, `/panic`'s
  question now says that the self-changes kept since then stop running after `/resume`.
- **Unknown commands are refused locally.** `/undo list` in v0.5.1 went to the model as a chat message and was billed.
  A line whose first word looks like a command that no part of INIT reads is not sent and stays in the box
  (`unknown command — /help lists them; start the line with // to send it as a message`); new `/help` lists the
  commands. `/undo list` and `/undo <id>` (R-UNDO) are in the command list and completion and go to the Root as lines.
- **Money labels.** A turn seen only in the attach replay read `$0.000` (the ledger does not move while a replay is
  read); it now reads `$? before this launch` (or `≥$0.002` for the part seen live), and a self-change seen only in the
  replay is not shown as a `$0.000` check window. `incl. checks` now appears only when a step that makes model
  requests (G2, G3, G4) was in flight during the run, not merely a self-change building.
- **`/details` shows what it says.** The body now sends a shell command's first 3 lines (at most 400 characters, `…`
  when cut) as its preview, and the last 10 lines of its output for every command that ran, not only for failures
  (`seed/src/tools/mod.rs` `command_preview`, `fail_detail`; display only: the model's result is unchanged and the
  session log already held the output). Ctrl-O and `/details` show every line of the call, say when it is a preview,
  and show `output (last N lines)`; a shell call from an older body says that none was sent.
- **Smaller things a stranger hit.** After a relaunch the replayed calls read `ran 5 tools in 0.0 s` and
  `started 11:19:10Z · 0.0 s` for calls that took minutes: they now read `before this launch`. `init-tui` ignores a
  word starting with `#` and the rest (zsh's default `interactive_comments` off passed the README's `# first run: …`
  on, and `init-tui` refused `unknown argument "#"`). Plain `/undo` refused as `unchanged` (R-UNDO) shows the Root's
  message, and the TUI's own `/undo: restored snapshot …` line no longer prints the snapshot's UTC time (the Root's
  restore notice gives the local one).

### Root: `/undo` after a relaunch, and an upgrade cut off part way (R-UNDO; docs/user-sessions/v0.5.1-new.md, the post-release review of v0.5.1)
- **`/undo` reaches the state before the agent's edits after INIT is reopened.** Every launch snapshots the folder, and
  v0.5.1's `/undo` restored the newest snapshot, which after a relaunch already held the agent's edits, so `hello.py`
  was "restored" into place. `/undo` now restores the newest snapshot whose content differs from the folder now (a
  sha256 digest of the Root's own clone; times are not in it). A launch whose folder is unchanged since the newest
  snapshot keeps no new one and says so, so relaunches no longer push the useful snapshot out of the 5 kept.
  `/undo list` shows each snapshot, newest first: its id, time on your clock (not UTC), what took it (launch, full
  access on, before an `/undo`) and folder, with the one `/undo` restores marked. `/undo <id>` restores a chosen one
  (`/undo <id> confirm`). Unknown ids, and a snapshot equal to the folder now, are refused with a message that names
  `/undo list`. Every safety property of the restore is kept (busy check, typed confirmation, the current state saved
  first, links never followed). New: the confirmation restores what it named (a folder changed since the question is
  asked again), and the snapshot being restored is never pruned (`root/crates/init-root/src/access.rs`; root.md §3.17.4).
- **The launch notice is true.** "snapshot … taken (/undo restores it)" became false after a relaunch; it now says
  `(/undo list shows what /undo can restore)`. `/undo list` is not counted as an owner correction by the harm sentinel.
- **A line queued during a panic says so.** The notice "the system is restarting; your message is queued" during a
  panic or a stop now says INIT is stopped and that the line goes to the agent after `/resume` (`link.rs`).
- **An upgrade cut off part way never runs the agent on a half-upgraded install.** install.sh leaves the service file
  in place, so a terminal closed during the probe and then a login started the new Root on the new tuple while the
  rollback anchors still named the old kernel. `init-root start` now finishes the upgrade the one sound way. If its
  commit had begun, it commits and starts. Otherwise it rolls back to the install from before and exits, so the
  service starts the old Root. Either way it logs the outcome, writes an alert and prints what to run
  (`upgrade.rs` `at_start`; root.md §4.3). The health probe's own Root is marked and still runs.
- **`upgrade --commit` installs the release's get.sh** into `~/.init-root/cache/` (staged by the replace step). A
  commit run by hand after a failed in-script commit no longer leaves `init-agent upgrade` on the old release's get.sh.
- **No stale "next" hint.** install.sh's replace step passes `--by-installer`, so the Root no longer prints
  `"next": "… upgrade --probe, then --commit"` after the installer already ran both. The bare step still prints it.
- **Small:** a failed rollback's message names every pointer it tried, `settled` included (`tuple.rs`).

### Body tests: no more load-induced refusals of a good self-change (R-FLAKE; REL-051 class 6)
- G0 runs the body's own tests on the owner's machine, and on 2026-10-28 two of them failed 3 of 5 macOS self-change
  runs on a loaded Mac, refusing changes that were good. `t_aw_5_probe_timeout_and_failure_give_unknowns` asserted
  that the hanging fake `git` had started, which a 2 s timeout under load can pre-empt; `took >= timeout` is the proof
  of the timeout path, and the mark is now a diagnostic. `crash_restart_continues_pending_run` read a request body
  that `init-mockllm` was still writing ("EOF while parsing"): the mock now writes every recorded body atomically
  (a hidden temp name, then rename; `kernel/crates/init-testkit` `mockllm` `write_atomic`), its log reader skips a
  line still being appended, and the body's readers retry a partial file for up to 5 s.
- An audit of every wall-time assertion in the body suite: sleeps used as synchronisation now wait for the event
  (a cancel after the spawn, an abort once the 429's retry began or the guest's process started), and bounds that
  measured the scheduler rather than the behaviour are generous or measured on the body's own clock (heartbeat
  intervals from `uptime_ms`, an interrupt's 200 ms to the bash call's end, the lock-release check as one `try_lock`).
  Product latencies (abort, cancel, Esc) keep their bounds. Root's `m7_draw_and_g4_slice`: its fake kernel gave
  every control-link call 30 s and died when a loaded `sealed.g4_slice` took longer (measured 25 s at load 45); an
  injected call now waits its `wall_s` + 120 s (`tests/support/fakekernel.rs`), and the test bounds the answer by 4x
  a slice it measured earlier in the same run (120–720 s), not a fixed 60 s.

### Installers and docs: what the post-release review of v0.5.1 found (R-POST)
- **INIT installed in another ROOT_HOME.** An owner who installed with `INIT_ROOT_HOME=X` and the service, then pasted
  the README one-liner without it, got a second install in `~/.init-root`: install.sh rewrote the one LaunchAgent
  (`dev.init.root`) or systemd user unit (`init-root.service`) for the new home and overwrote the `init-agent` launcher,
  which left the evolved INIT in X behind. Before a fresh install, get.sh and install.sh now read the `INIT_ROOT_HOME`
  of the service file this install would write (not with `--no-service`, and only when HOME is the passwd home, the
  only case where install.sh writes one) and the ROOT_HOME baked into INIT's launchers in `INIT_BIN_DIR`. When that
  home holds an install.json they refuse, before anything is downloaded or written, with that home's upgrade line
  (`INIT_ROOT_HOME=X init-agent upgrade`, or the piped get.sh for 0.4.1 and 0.5.0, or the uninstall before 0.4.1),
  and say how to make a separate second INIT instead (its own homes and bin directory, `--no-service`)
  (`svc_file`, `other_home`; tests/install/test_installed_elsewhere.py).
- **No bundle for this machine.** A Linux aarch64 install of 0.4.1 (no 0.5.x aarch64 bundle) was told to upgrade, and
  the upgrade then stopped on a missing bundle with advice for a fresh install. get.sh now reads the latest release's
  latest.json before it advises an upgrade over an install (`latest_without_target`; it fetches nothing else, and a
  failed fetch leaves the advice as it was). When no bundle for this target is listed, it says so, that the installed
  INIT keeps working, and to upgrade when a bundle for this target is released (the releases page). An upgrade that
  meets a missing bundle says nothing was changed and to run it again then.
- **install.sh's closing `Uninstall:` line** printed the uninstaller's path without `INIT_ROOT_HOME=` for a custom home;
  it is now `INIT_ROOT_HOME=… bash '<release>/scripts/uninstall.sh'`, quoted, and runs as printed.
- **uninstall.sh honours install.json's `bin_dir`.** After an install with `INIT_BIN_DIR` set, uninstalling without it
  left `init-agent` and `init-tui` behind (it looked only in `~/.local/bin`). `INIT_BIN_DIR` still wins when set
  (tests/install/test_uninstall.py).
- **The launcher reads the passwd home with `/usr/bin/python3`**, as the other scripts do, and only when a message needs
  it. `(unset HOME; echo ~)` returns `$HOME` under bash 5.3, so with a HOME that is not the passwd home the printed
  `start --foreground` line lacked `INIT_ROOT_HOME=` and the LaunchAgent path named the wrong home (checked under every
  bash on the machine).
- 21 mutations of these fixes, each run on a copy of the scripts, are all killed by the tests above (release protocol
  §4.1).
- **Docs.** README and docs/guide.md: the first run is `~/.local/bin/init-agent` (a default macOS shell has no
  `~/.local/bin` on its `PATH`, so plain `init-agent` was "command not found"), with the line that adds it. The README
  status no longer says the v0.5.1 macOS bundle took a core change through the gates (its attempt passed G0–G2 and
  lost G3's inner G0 to a known flaky test; x86_64 passed), and gives REL-051's count, 2,082 passed. The cold
  core-change time is the receipt's 788 s (13.1 min; v0.5.0, M2, mock model; build/status/bundle-e2e-t2.json,
  `promote_wait`), not 838 s, in README, the guide and the v0.5.1 notes. The guide's uninstall is the curl line get.sh
  prints (the `~/.local/share/init/<version>/` copy does not exist after an upgrade, which unpacks into
  `~/.init-root/cache/upgrade/<version>/`), with the `INIT_BIN_DIR` note, and its code blocks carry no `#` comments
  (zsh does not take them as comments when pasted). The v0.5.1 notes: one flaky-test paragraph that says v0.5.2,
  `/usr/bin:/bin:/usr/sbin:/sbin`, and absolute links (a GitHub release body renders relative links as 404s).
  docs/foundations.md: the TCB is §10, `upgrade_commit` moves lkg and the twin too, and the protocol reports the TCB's
  size from v0.5.3, not yet. docs/release-protocol.md: "§4 CI once, the body suite ×5 under load" (v1.1).

## v0.5.1-preview (2026-10-28)

### Workspace mode: git config can no longer be redirected (post-release review)
- **The hole.** In workspace mode the agent may write the folder INIT was started in, including a git repository
  there. The macOS zone tail's rule Z3b denied writes only to `.git/hooks`, `.git/config` and the same two under
  `.git/modules/<m>/`. git takes its config and command definitions from several other places the agent could still
  write, so an agent acting against the owner could redirect git's config to a file it controls, and the owner's next
  `git` command in that repository (for example `git status` running a `core.fsmonitor` or `core.hooksPath` hook) would
  run that command unsandboxed, with the owner's full rights — a path that can reach and write the Root's files.
- **Who could hit it.** A model acting against the owner in workspace mode writes the redirect; the owner then runs
  git in that repository. Full access was already a documented residual (the agent can write what the owner can, except
  the Root), so this was a workspace-mode promise (guide.md, ADR-012/ADR-009) that did not hold. Linux is unaffected:
  workspace mode there caps the home grant off (the agent works in `INIT_HOME` only), so there is no owner workspace to
  redirect.
- **What changed.** Z3b now also denies the `.git` entry itself (`/\.git$`, so `.git` cannot be renamed aside,
  replaced, or created as a `gitdir:` file) and git's config and command sources — `config`, `config.worktree`,
  `commondir`, `gitdir` and `hooks/` — inside any git directory (`.git/`, `.git/modules/<m>/`, `.git/worktrees/<w>/`)
  of a repository outside `INIT_HOME`. Because Seatbelt checks a directory rename by the source and destination
  directory paths and not the files inside, Z3b also denies writes to the `modules`/`worktrees` admin directories and
  to a submodule or linked-worktree gitdir entry inside them, at any depth (so the agent cannot build a directory
  holding a config and rename it onto `.git/modules/<m>` or `.git/worktrees/<w>`), while excluding the `refs/` and
  `logs/` ref namespaces so a branch named `modules/*` still commits. The rest of `.git` (objects, refs, index, HEAD,
  logs, packed-refs, COMMIT_EDITMSG…) stays writable, so the agent can still commit; `git init` of a brand-new
  repository, and `git submodule`/`git worktree add`, in the workspace are now refused (their fresh gitdir would
  otherwise be agent-writable). `.git/info/attributes` is left writable (it only names filter/diff drivers whose
  commands live in the now-denied config) and `objects/info/alternates` is an object source, not a command source.
- **Tests.** New rows in the macOS sandbox self-test (`init-sbx-selftest`, kernel/sandbox.md §8), run under the real
  rendered workspace profile: S-A20l–t reproduce the bypasses — `.git/commondir`, `.git/config.worktree`, the
  `.git/worktrees/<w>/` and `.git/modules/<m>/` config/command files, a `.git` gitdir: file, and renaming `.git` aside
  — and must now be refused; S-A20u–z reproduce the rename-swap bypass (renaming an agent-built directory onto a
  `modules`/`worktrees` admin entry or a gitdir inside it, at any depth, and moving such a directory aside); S-A20j–k,
  S-A20A–B and S-A20C–D run a real `git add`/`git commit`, a commit on a branch named `modules/x`, and an add+commit
  inside a submodule gitdir, to prove committing still works. Mutation-tested (release-protocol §4.1): with the old
  file-only Z3b, exactly S-A20l–t fail; with the admin-directory rule reverted, exactly S-A20u–z fail; the regression
  rows pass throughout.
- root/sandbox-profiles/profiles/zone-tail.sb, docs/root/root.md §3.3.2, docs/kernel/sandbox.md, docs/guide.md,
  docs/adr/009 and docs/adr/012.

### Upgrade: a local bundle is checked where the agent cannot reach it; --version 0.5.0 works (post-release review)
- **The hole.** `init-agent upgrade --bundle FILE` (and `get.sh --upgrade --from FILE`) hashed the bundle where it
  lay and then unpacked it from the same path. Without `--sha256`, it took the expected sha256 from `FILE.sha256`
  beside it. The unpacked release's `install.sh --upgrade` then ran as the owner and replaced the Root. With full access
  (the default) the agent can write that folder, so it could write a `.sha256` that matches a bundle of its own, or
  swap the bundle between the check and the unpack. docs/guide.md and the v0.5.0 release notes said the checked copy
  lived in `~/.init-root/cache/upgrade/` "so it cannot swap the code between the check and its run". That was true
  for the download only. The same review found more. Every upgrade, the download included, ran curl, tar, shasum, git,
  rustup and the rest from the owner's `PATH`, whose directories (`~/.cargo/bin`, Homebrew's) the agent can write in
  full access. curl read `~/.curlrc`. install.sh wrote the new `init-agent` launcher in `TMPDIR` before copying it
  into place. And `init-root upgrade` ran `git bundle list-heads` on the release's kernel source from the caller's
  working directory, so an upgrade started inside a repository read that repository's `.git/config`.
- **Who could hit it.** An owner in full access whose agent acts against them, who then upgrades from their own
  shell: with `--bundle` on a file in a folder the agent can write (`~/Downloads`, say), or with any upgrade while a
  tool the agent planted sits on `PATH` ahead of `/usr/bin`. In workspace mode only a bundle, tool or repository inside
  the workspace folder was exposed. No exploit was seen; the review found it.
- **What changed.** In an upgrade, get.sh copies a local bundle into `ROOT_HOME/cache/upgrade/` first and checks and
  unpacks only that copy. It takes a local bundle's sha256 from `--sha256` only, and without one it stops before
  writing anything, printing the exact `init-agent upgrade --bundle <path> --sha256 <hex>` command, where to find the
  hex (the release page), and `init-agent upgrade --version <V>`. We chose `--sha256` over fetching the official
  `.sha256` from GitHub because it works offline and for a bundle you built yourself, which are the reasons to use
  `--bundle` at all. Online, `--version` already downloads and checks the release. Fresh installs, which have no agent
  yet, still accept the `.sha256` beside the bundle. `init-agent upgrade`, `get.sh --upgrade` and
  `install.sh --upgrade` (from a bundle) run the system tools only (PATH `/usr/bin:/bin:/usr/sbin:/sbin`), from `/`.
  The launcher uses only bash builtins before it hands over. curl runs with `-q`. install.sh keeps its scratch files
  under `ROOT_HOME/cache/` and runs no rustup, since the upgrade keeps the toolchain install.json records. Every Root
  git call runs from `/` with `core.fsmonitor=false`, on top of the existing no system or global config, no hooks and
  no inherited `GIT_*` (install.rs `git`). This covers the kernel source and the seed bundle at install too.
- **--version.** `init-agent upgrade --version 0.5.0` asked for the tag `v0.5.0` and got HTTP 404, because the tags
  carry the label (`v0.5.0-preview`) while `init-root --version` and the site say 0.5.0. Now a plain version tries
  `vV`, then `vV-preview`. A version with a label is that tag, and if neither exists the error names both tags and
  the releases page.
- **Leaving 0.5.0.** `init-agent upgrade` runs the get.sh the installed release kept, so an upgrade from 0.5.0 still
  runs 0.5.0's get.sh, with the holes above, and its `--version 0.5.1` still asks for `v0.5.1`.
  docs/guide.md "Upgrade" gives the command that runs the new get.sh with the system's curl and sh. The pre-release
  review found the scripts still printing `init-agent upgrade` to 0.5.0 owners: get.sh, install.sh and the launcher now
  print that command for an installed 0.4.1 or 0.5.0 (with `--from`/`--sha256` for a bundle; `init-agent upgrade` from
  0.5.1 on), with `INIT_ROOT_HOME=` on the `sh` side for a ROOT_HOME other than `~/.init-root`; the launcher names the
  systemd user unit on Linux, a failed upgrade commit says the Root stays stopped and how to start it, and `--version`
  probes its tags silently and says when it is ignored (tests/install, each mutation-tested).
- **Breaking for scripts.** `get.sh --upgrade --from FILE` without `--sha256` now exits 1. scripts/upgrade-e2e.sh and
  scripts/install-matrix.sh pass it.
- Tests: tests/install/test_upgrade_trust.py (sidecar refused, the copy under ROOT_HOME, a wrong sha256, tools planted
  on PATH in get.sh, install.sh and the launcher, the four `--version` cases); tests/upgrade.rs
  `an_upgrade_started_inside_a_repository_reads_none_of_its_git_config`; upgrade.rs
  `root_git_runs_from_the_filesystem_root_with_no_outside_config`. Each was mutation-tested (release-protocol §4.1):
  with its fix reverted, it fails.

### Upgrade: a panic after an upgrade resumes the upgraded tuple
- **The bug.** `init-root upgrade --commit` moved `lkg` to the upgraded tuple but left `audited` (and `settled` on
  the free path) on the tuple from before the upgrade. A panic resume and a demotion roll back to that pointer
  (root.md §3.9.5), so after `/panic` and `/resume` the Root reinstalled the old `init-kernel` from its store and ran
  it under the new Root. `/root status` then showed `current` and `audited` on the old tuple while every banner said
  the new version. A returning-user session saw it on 0.4.1 → 0.5.0 (docs/user-sessions/v0.5.0-returning.md);
  tests/upgrade.rs reproduces it without that session's harness. docs/guide.md "Upgrade" listed it as a limit of
  0.5.0; the commit now removes that limit.
- **Who could hit it.** Anyone who upgraded and then panicked or was demoted before a sealed audit passed on the new
  tuple. In Phase 0 that audit is a manual owner step, so in practice that was every upgraded install.
- **What changed.** At the commit, each rollback anchor whose kernel is not the release's becomes {the release's
  kernel, the anchor's own body and ext}: an `audited` or `settled` record with `from` and `upgrade`, and `anchors` in
  the commit's output. An anchor whose tuple file cannot be read is cleared instead (`{tuple: null, skipped}`), and
  the commit reports it in its output, its journal, an alert and one plain line. The commit's `lkg` record carries
  `upgrade`, and `settled` counts only `lkg` moves from there on. root.md §3.7.2 records the decision. Only
  `upgrade --rollback`, before the commit, and the genesis tuple, the last resort when no other tuple verifies, go
  back to the old kernel.
- **Leaving 0.5.0.** An install upgraded to 0.5.0 keeps its old anchor until its next upgrade. That upgrade's commit
  is the new release's own, so it moves the anchor.
- **What the pre-release reviews added.** Several more paths could run the old kernel under the new Root, or delete
  the install:
  - **`/rollback twin`.** The twin is re-anchored too, keeping its body, ext and freeze: its replays never run its
    kernel.
  - **A release that keeps the kernel.** Each anchor is compared with the release's kernel, also when the release
    kept the kernel.
  - **A commit cut short.** It now records `committing` before it removes the snapshot, and is finished by re-running
    `--commit`. `--rollback` is refused from then on, and also whenever the snapshot is incomplete. Before, a rollback
    after such a crash deleted the Root, the kernel files and the launchers.
  - **A rollback after a commit cut short earlier.** The rollback now restores `lkg`, `audited`, `settled`, the twin
    and `refs/heads/lkg` from the journal.
  - **A cleared anchor.** It is reported from the lineage records, so a re-run commit reports it too.
  - **Goal readings.** Those in flight on pre-upgrade changes are cancelled, with a `card_reading{action:"cancelled"}`
    record. No later reading takes its baseline or rollback target from before the upgrade.

  root.md §3.7.2 argues each decision. Tests in tests/upgrade.rs:
  - `the_commit_moves_the_twin_so_rollback_twin_runs_the_release_kernel`
  - `a_commit_that_keeps_the_kernel_still_moves_an_anchor_left_on_an_older_one`
  - `a_commit_cut_short_while_the_snapshot_goes_finishes_and_is_never_rolled_back`
  - `a_rollback_on_an_incomplete_snapshot_changes_nothing`
  - `a_rollback_after_a_commit_cut_short_restores_lkg_the_anchors_and_the_twin`
  - the re-run in `an_unreadable_anchor_is_reported_and_cleared_at_the_commit`

  Unit tests: goal `readings_never_span_an_owner_upgrade`, upgrade `undo_restores_nothing_from_an_incomplete_snapshot`
  and twin `an_upgrade_keeps_the_twin_freeze`. Each was mutation-tested.
- Tests: tests/upgrade.rs `a_panic_after_a_committed_upgrade_resumes_on_the_upgraded_kernel` (upgrade, probe,
  commit, panic, resume: the resumed tuple and `kernel/bin/init-kernel` are the release's) and
  `an_unreadable_anchor_is_reported_and_cleared_at_the_commit`; snapshot/tests.rs
  `no_tuple_from_before_an_upgrade_settles_after_its_commit`. Each was mutation-tested (release-protocol §4.1): with
  its fix reverted, it fails.
- root/crates/init-root/src/tuple.rs (`reanchor`), upgrade.rs (`commit`), snapshot.rs (`settled_of`); docs/root/root.md
  §3.7.2, §3.9.5 and §4.3; docs/guide.md "Upgrade".

### TUI: what the user sessions found (docs/user-sessions/v0.5.0-new.md, v0.5.0-returning.md)
- `/` then Tab completed to `/panic`, which ran with no confirmation. Completions now list safe commands first and `/resume`, `/root stop`, `/panic` last, and `/panic` in the TTY says what it does and waits for you to type `panic` (Esc or any other line cancels). Line mode is unchanged. The pre-release review found two holes, now closed: an agent question (or a setup answer) that arrived during the confirmation cancelled it and took the keys, so typing `panic` answered the agent's picker and nothing stopped (it now waits until the confirmation is sent or cancelled, then shows); and `/panic` again or Up+Enter cancelled and asked again without end (`panic` and `/panic`, any case, now confirm). Tests `nothing_preempts_a_pending_panic_confirmation` and `panic_again_confirms_the_panic`; the unpriced-model text now says `$0.000` as the screen does, and the start hint names a systemd user unit on Linux.
- An answered `ask_owner` picker came back after a relaunch, and "waiting on you" stayed after Esc. An owner line the kernel echoes, live or replayed, now closes the question as `answered`, and Esc ends the waiting state with a one-line hint.
- `/usage` said "evolution checks today $0.00" while the gate's probes were about two thirds of the ledger. That figure was a stale kernel number. The row now shows what the Root metered while a self-change ran, labelled as that. The Root has no per-purpose split yet (the requirement is docs/tui.md §9 Q5).
- The turn's `$` kept rising during a gate after the reply, with its tokens fixed. It now stops when the run ends and says `incl. checks` when a gate ran during the run.
- `$0.05` on the status bar against `$0.04` in `/usage`: every amount now uses one rule, a tenth of a cent below $1 (`$0.046`) and `<$0.001` for spend under that, never `$0.000`.
- G0's timer showed 8 min 33 s and its row 1 min 39 s, leaving 8 minutes on no row. A step now shows `waited …` for the time outside its own work, so the rows add up to the total. A G4 with nothing to score reads `not scored`, not `pass · 0.0 s`.
- `/root status` "ceilings $5/$25" beside "caps off" now reads `dollar refs … (charter records, not caps)`.
- The lineage panel's "9 records" above seq 1–8 now names the range: `newest 8 of 9: seq 1–8`, `9 records (seq 0–8)`.
- Ctrl-D left half an input box. Quitting now erases the live region and prints one line: the agent keeps running, how to come back, and the ledger total.
- With no workspace, the screen now says why, that the agent works in its own folder (`~/.init/work`), and what `/undo` cannot restore. `/undo` no longer gives the circular "cd into a project" advice, and the status bar no longer says "no workspace · WORKSPACE".
- "start it with `init-root start`" (not on PATH) now names the binary by its full path, and the slot word "backoff" now reads "agent failed to start, retrying".
- Tests: app/fix_tests.rs, proto `stopping_commands_come_last`, input `tab_on_a_bare_slash_is_never_a_stop`, status `self_change_spend_is_what_the_ledger_metered` and `slot_words_are_plain`, model `turn_cost_stops_when_the_run_ends`, gates `rows_add_up_and_an_unscored_g4_is_not_a_pass`, access `no_workspace_says_where_the_agent_works_and_what_undo_restores`, plus lineage, table and render `usd_formats`. Each was mutation-tested: with its fix reverted, it fails (release-protocol §4.1). Goldens regenerated for the intended changes (money format, gate waits, popup order, no-workspace badge, `/usage` row, lineage rule). tests/e2e_root.rs types the `/panic` confirmation.

### Root: every command it prints runs as printed; refusals say the rule and the fix
- The toolchain hint after `launchctl bootout` now says `launchctl bootstrap gui/<uid> <plist>` (kickstart only for a job still loaded), as install.sh does.
- docs/foundations.md v1 states INV-2, INV-3 and INV-5 as the code is, with their gaps: the admission's Root re-hash (ADR-011 A0) is not built; a free-path promotion is matched by commit sha only; a sealed audit can move lkg backwards. Planned for v0.5.2 (build/status/PLAN-052.md, lane L2).
- **Commands not on PATH.** `init-root` lives in `~/.init-root/bin` and only `init-agent` is on PATH, so the Root's
  own advice failed with "zsh: command not found: init-root" (docs/user-sessions/v0.5.0-returning.md). Every command
  the Root prints now runs as printed: this install's binary (`~/.init-root/bin/init-root …`, or
  `INIT_ROOT_HOME=… …/bin/init-root …` for another ROOT_HOME; `util::root_cmd`) or the `init-agent` form (`/resume`,
  `/key`). That covers the panic lines, `/root stop`, `start` while panicked, charter-pending, proposals, a rejected
  key, providers, sealed and toolchain messages; a kernel alert's `` `init-root …` `` is rewritten the same way.
- **An unreachable Root** (`init-root` CLI) now says how to start it: `launchctl kickstart …` when its service is
  installed, else `… start --foreground` in a terminal of its own. It said "is `init-root start` running?".
- **The toolchain hint was wrong.** It said `init-root stop && init-root toolchain --set && init-root start`, but
  `stop` stops only the agent, so `--set` was refused while the Root held its lock. It now names how to stop and start
  the Root itself.
- **A hand-started Root** (`init-root start` in a terminal) now says the terminal is the Root's and to open INIT from
  another one; it printed nothing.
- **HOME mismatch.** A Root started from a shell whose HOME is not the account's home still refuses the agent (the same
  `root.args` refusal), but the alert now says what is wrong and how to fix it, in place of
  "root.args V3: HOME_REAL: outside INIT_HOME, TOOLCHAIN, DEVDIR and extra_read".
- **Queued lines** (kernel). "queued: the body is backoff" now says what is happening and what to do, for example
  "your message is queued, not lost: the agent could not start, and INIT tries again in 1 s; … The Root says: <its
  reason>". A crash, a stop, a halt and a full disk each say what happens next. `/resume` with nothing to resume says
  so; it printed nothing.
- **Workspace refusal.** The notice now says why the start folder was refused, the rule (a folder inside your home,
  not the home itself and not one of INIT's own or protected folders), that without one the agent works in
  `~/.init/work` and `/undo` has nothing to restore, and how to get one. `/workspace` and `/undo` say the same. The
  rule itself is unchanged. `/full off` now says it holds for this session only (a plain relaunch was back to FULL),
  and how to keep workspace mode.
- **Integrity halts.** `/resume` refused after one now says it cannot clear it, where the cause is logged and how to
  restart the Root; it said "fix it and restart the Root".
- **Money.** `status`'s `ceilings` block now carries `"enforced": false, "policy": "observe"`: the charter's $5 a day
  and $25 in all are references, tracked and not caps, which the TUI's `dollar refs` row reads.
- **Docs.** docs/guide.md and the charter template print `~/.init-root/bin/init-root …`; root.md §3.17.1 (the
  no-workspace notice) and §4.6 (the one path an alert may name: this install's `init-root`).
- **Upgrade messages** (upgrade.rs): the pending-upgrade refusal, the panic-latch refusal (which now says to start the
  Root and `/resume` first), the `next` hint, a failed restore and an unfinished replace print `Paths::cmd`, so
  tests/msg.rs has no exemption list left.
- **TUI key setup** (setup.rs): "store it with `init-root keys set …`" and "use init-root providers / keys set" name this
  install's binary in the Root's form (`app::root_cmd`, from the TUI's ROOT_HOME and HOME). The TUI's unreachable-Root
  hint (`start_hint`) uses the same form now, in place of `…/bin/init-root --root-home DIR start`.
- **Installers** (install.sh, get.sh): every `init-root` command they print goes through `rcmd`, the Root's form with
  the absolute path (`$RH/bin/init-root …`, with `INIT_ROOT_HOME=` in front when ROOT_HOME is not `~/.init-root`, and
  quoted when the path needs it). install.sh's closing T0-only steps said `init-root stop; init-root toolchain --set;
  init-root start`, and `--set` was refused because `stop` stops only the agent. They now stop the Root itself
  (launchctl bootout, systemctl --user stop, or Ctrl-C), run `toolchain --set`, and start it again, as the Root's own
  hint does (docs/release.md's owner steps had the same wrong sequence; fixed). get.sh's "From a shell instead" key
  line no longer drops a non-default ROOT_HOME, and neither does install.sh's failed-rollback hint.
- **Docs that disagreed with the code.** ADR-011 §3.2, §3.3 and §9 carry a dated note with the sentinel's real numbers:
  a ring of 40 units, not 40 turns; j = 8 at α = 0.005 for H5b, H8, H10 and H11; α = 0.002 for H7 and H9
  (sentinel/engine.rs). ADR-013 decision 4's "≈ 2.3%" false rollbacks per guardrail is ≈ 5.1% at the default 5 readings
  against 40 (Welch's df ≈ 5); the fix is planned for v0.5.2. docs/guide.md's Linux requirement and limit bullets
  described Linux before v0.5.0 ("next release", x86_64 not run); they now say Linux x86_64 ships, experimental,
  tested on a hosted Ubuntu 24.04 VM, and aarch64 stays on v0.4.1.
- Tests: tests/msg.rs (a source audit of every string the Root prints, the charter template and docs/guide.md; an
  unreachable Root; live: the panic message and `status`, a spawn refused for HOME, the workspace refusal, its notice
  and `/undo`); access.rs `no_workspace_says_the_rule_where_the_agent_works_and_the_fix`; util.rs
  `root_cmd_runs_as_printed`; sup.rs `a_kernel_alert_names_init_root_runnable_as_printed`; kernel machine_tests.rs
  `waiting_lines_say_what_happens_and_what_to_do`; TUI setup `key_setup_texts_name_an_init_root_that_runs_as_printed`,
  app/fix_tests.rs `the_apps_setup_names_its_own_root_home` and `root_cmd_is_the_roots_form`;
  tests/install/test_printed_commands.py (an audit of install.sh, get.sh, init-agent and uninstall.sh for a bare or
  typed-out `init-root` command, `rcmd`'s output run by a shell against a stand-in binary, and the toolchain steps' order
  on each service kind). Each was mutation-tested (release-protocol §4.1): with its fix reverted, it fails.

## v0.5.0-preview (2026-10-28)

Workspaces are at version 0.5.0 (seed tag `seed-v0.5.0`). Release notes: docs/release-notes/v0.5.0-preview.md.

### `init-agent upgrade` (build/status/UPGRADE.md)
- `init-agent upgrade [--bundle FILE] [--sha256 HEX] [--version V]` moves an install to a new release and keeps the
  evolved body, its extensions and skills, the sessions, the signed lineage, the keys and the charter. Installs of
  0.4.1, which have no `upgrade`, run `curl -fsSL …/get.sh | sh -s -- --upgrade` once. The bundle is checked by its
  SHA-256 and unpacked under `ROOT_HOME/cache/upgrade/`, which the agent cannot write in any access mode.
- `install.sh --upgrade` stops the Root (a foreground Root is refused, never signalled), runs the new
  `init-root upgrade --release DIR` (no downgrade, no same release, the body's control protocol, manifest format and
  WIT majors checked against the release's own body, a snapshot, the replace, a new tuple with the new kernel), installs
  the launchers, then `init-root upgrade --probe` and `--commit`, or `--rollback` when the probe fails
  (root/crates/init-root/src/upgrade.rs; `--status [--json]`). An open free-mode change group refuses the upgrade.
- Not done: merging a release's new seed into an evolved body, a live kernel swap, signed releases. The service path
  (launchctl, systemd) of `install.sh --upgrade` uses the commands install.sh and uninstall.sh already run; only the
  no-service path ran end to end.
- Tests: tests/upgrade.rs (replace, probe, commit; a broken kernel rolled back; refusals; the zone in full and workspace
  mode cannot run the upgrade or write what it replaces), tests/install/test_upgrade_guards.py,
  scripts/upgrade-e2e.sh with scripts/upgrade-e2e-t1.sh (a T1-evolved 0.4.1 install).

### The installer asks one question (build/status/UPGRADE.md)
- install.sh asks no provider or model (the TUI's first run picks them) and no limits one by one. It prints the
  settings it writes into the charter (disk floor, the day and total spending references, the autonomy cap, the access
  mode) and asks `Install with these settings? [Y/n]` once; `--disk-floor`, `--day-usd`, `--total-usd`,
  `--max-level`, `--access full|workspace` and `--purpose` set each value.
- Spending is labelled as what it is: the day and total amounts are tracked and shown, not caps; a goal's `usd=` is the
  one dollar limit the Root enforces (installer text, charter template comment, docs/guide.md "Spending").
- macOS: v0.4.1's `init` shim is gone. install.sh installs no `init` on either platform and removes INIT's own old one
  (the pre-v0.4.1 launcher or the shim, by path, regular file and content markers); `scripts/init-shim` is deleted.

### TUI: colour means state (build/status/TUI-BRAND.md)
- Semantic palette roles (ink, dim, mute, line, root, rewrite, pass, harm, warn) in four tiers (truecolor, 256, 16,
  `NO_COLOR`) with dark and light variants; the background is read with OSC 11 or `COLORFGBG`, or set with
  `--theme dark|light`. The Root's amber is used only for the Root.
- A liveness row above the input while the agent works (waiting for the model, thinking, writing, a quiet stream, a tool
  by name, with times; `waiting on you` while the agent's question waits). The status bar is split: the agent on the
  left, the Root on the right ending in `root ok` or its alarm. A one-line identity at the first attach.
- `/root status`, `/root ledger` and `/usage` render as aligned tables in the TUI (line mode keeps the raw JSON).
  Identical notices in a row collapse into one line with a count.
- Renderer property tests through a terminal emulator (`vt100`, dev-dependency only) found and fixed a redraw bug: the
  live region's last row was erased when it had not changed, so the hint line vanished.

### TUI: self-changes on screen (build/status/TUI-OBSERVE.md)
- A gate track: one live block per self-change (G0 … G4, hand-off) collapsing into one line: kept, refused at a gate
  (`still on <parent>`), or rolled back after it went live (`back on LKG`). The kernel sends owner-only `gate_start`
  and `rolled_back` events and structured `data` on its notices; the body's notices are unchanged.
- A failed tool call shows the last lines of its output (the body's `tool_end` `detail`, at most 10 lines, 2,000 bytes).
- Ctrl-O ends with a lineage panel: `verify ✓` or the first failing record, and the newest records, from the Root's new
  read-only `lineage.tail` owner request.
- `scripts/demo/record-mock.sh`: a recording kit that installs a bundle with the mock model and records a kept T1
  change and a refused one to an asciinema cast, every frame captioned "mock model".

### TUI: pickers and the agent's questions (build/status/TUI-PICKER.md)
- Every list choice is an inline picker (arrow keys, Enter, Esc, 1-9, type to filter, multi-select): the first-run
  provider and model, `/model`, `/key`'s provider. Line mode keeps numbered prompts.
- A body tool `ask_owner` (2-4 options, a recommended one, multi-select): the turn ends and the owner's answer is the
  next message; no timer, no default. The TUI cleans and caps the question, shows it under its own label in plain
  roles, and quotes a label that starts with `/`, so an option can never pass for the Root or a command.
- The Root stops a goal's wall clock while an agent question waits on the owner, and starts it again at the owner's next
  line or the next admitted model request; money, tokens and rounds are unchanged (goal.rs; ADR-013).

### Robustness, offline lineage verify, proof export (build/status/HYGIENE.md)
- G0 passes with a 63-64 byte `INIT_HOME` (the seed's test sockets bind through a short relative path when the absolute
  one does not fit). A3's G3 stops when the Root rolls its change group back (`gate.g3.stopped`, no veto). The body's
  boot block names the Root's rollback instead of a stale zone cause.
- `init-root audit verify --file RECORDS --pub KEY [--head HASH]` checks a published lineage file offline (hash chain,
  Ed25519 signatures from genesis, key rotation) and prints its limits.
- `scripts/proof/export.sh` copies sessions, the signed lineage with its key, one diff per self-change and the spend
  ledger, and refuses (exit 3) when its scan finds a key or token.
- Docs: a Spending FAQ in guide.md; README states that the Root protects INIT, not your files, in full mode, and the
  Root's real size.

### Linux x86_64 (build/status/LINUX-X64.md)
- get.sh names the platforms a release has when it has no bundle for the machine (HTTP 404), instead of "download
  failed".
- bundle.sh names each bundle's target triple and stops unless rustc's host, the release's MANIFEST host and every
  binary header match it (linux-x86_64 = x86_64-unknown-linux-gnu).
- A hosted-runner workflow (`ci/github/linux-x86_64.yml`): the x86_64 bundle in debian:bookworm-slim, the Linux zone
  qualification on x86_64, and bundle-e2e on ubuntu-24.04 after INIT's AppArmor step (below). Results at integration:
  build/status/REL-050.md.

### Ubuntu 24.04: INIT's own AppArmor step (build/status/REL-050.md)
- Ubuntu 24.04 restricts unprivileged user namespaces through AppArmor, and the zone needs two: util-linux `unshare`,
  then bubblewrap inside it. Measured on a hosted Ubuntu 24.04 VM: Ubuntu's `bwrap-userns-restrict` and
  `unshare-userns-restrict` profiles each pass their own program, but bwrap inside unshare is refused, so docs/platforms.md's
  earlier step (the bwrap profile) never let INIT install there.
- New one-time step, printed by install.sh when the namespace probes fail and the restriction is on
  (`--print-apparmor` prints only the commands; `--apparmor` asks, then runs them with sudo; otherwise INIT never runs
  sudo): root-owned copies of `unshare` and `bwrap` in `/usr/local/lib/init-agent/` (mode 0750, your group) and
  `/etc/apparmor.d/init-agent`, which grants only those two paths `userns`. uninstall.sh prints (or with `--apparmor`
  runs) the removal. The global sysctl is never recommended.
- The Root (init-sandbox-profiles `entry.rs`) starts the zone through the copies when present, and only when the
  canonical file and every directory above it are owned by root and writable by root alone; a copy that fails refuses
  the zone instead of falling back. Tests: entry.rs (5), tests/install/test_apparmor_step.py (3).

### Providers: cache writes billed, live qualification prepared (build/status/PROVIDERS-LIVE.md)
- The Root bills cache writes at the charter's write prices (Anthropic 5-minute and 1-hour, OpenAI and gateways one
  class); a model row without them bills writes at 2x input, so it can over-count, never under-count. Before, writes
  were billed at the input price (about 20 % under on a cold Anthropic or OpenAI request). DeepSeek billing and its
  records are unchanged. `providers add` writes the catalog's write prices; gpt-6.1-sol and gpt-6-astra gained theirs.
- Per-model `tested` label in `providers list` (DeepSeek's two models; Anthropic and OpenAI are not live-tested).
- `scripts/qualify-provider.sh` for the owner: one short live session inside a Root-metered `/goal usd=` budget (cap
  at most $10), never reading a key; checks streaming, a tool round trip, pause and resume, cache reads, tokens and cost
  against the Root's ledger. Tested on the mock only (tests/real/qualify_mock.sh).

### Linux: the build disk floor read the sandbox's tmpfs; fixed (build/status/REL-050.md)
- The kernel decided `disk.min_free_build_gib` (and the run floor at a body crash) from statvfs of INIT_HOME. Inside
  the Linux kernel zone INIT_HOME's subtrees are bound, not INIT_HOME itself, so that read the zone's tmpfs: 64 MiB for
  a home under /tmp (every promote refused `E_DISK`; the x86_64 T2 e2e), about half the RAM for `~/.init`. It now reads
  `INIT_HOME/cache` (bound; where builds write), with INIT_HOME as the fallback (kernel-supervise `disk_probe_paths`).
  Tests: `disk_probe_tests`, and the Linux qualification `linux_kernel_zone_disk_probe_reads_the_real_filesystem`
  (inside the real kernel zone, cache has the host's device and INIT_HOME has not). The Root's disk floor is measured
  by the Root outside every zone and was not affected; macOS was not affected.

### A race in a body test that could refuse a good self-change (build/status/REL-050.md)
- `loop::tests::t22_task_id` read the session hook's log right after the `/new` ack, which the loop sends before it
  calls the hook. G0 runs the body's own tests at every T2 promote, so on a loaded machine the race refused a good
  change (seen on the hosted x86_64 runner). The test now waits for the hook; `seed/self.json` re-blessed.
- The same class in `t26_interrupt_and_steer_during_a_batch` and `t20_stop_grace_and_eof`: an interrupt or stop that
  landed after `tool_start` but before the bash process existed ended the call "not executed" (correct), which the
  tests read as a failure (run 8). They now wait until the command runs, as t09 already did; 50/50 under load.
- `seed/self.json` is regenerated on the packaged seed tree (seedpack rewrites `.cargo/config.toml`'s vendor path), not
  in the repository; an in-repo `--write` is 3 bytes off and bundle.sh's G1 refuses it (docs/release.md §1).

### After the release: get.sh names the installed release (build/status/REL-050.md)
- On an install from before 0.4.1, get.sh told the user to run `init-agent upgrade` or `--upgrade`, which such an
  install does not have and the upgrade does not support (seen on the owner's 0.3.1 Mac). get.sh now reads the
  installed release (`init-root --version`): before 0.4.1 it refuses, also with `--upgrade`, and prints the uninstall
  command; 0.4.1 gets the `--upgrade` command and 0.5.0 or later `init-agent upgrade`. README's install block no longer
  chains `init-agent upgrade` after the install. The release's get.sh asset was replaced; the bundles are unchanged.
- Verified in a test home: 0.3.1 installed and running, the release's get.sh reproduces the refusal, the fixed one
  names 0.3.1, the printed uninstall command refuses while the Root runs and then moves 0.3.1 to the Trash, a fresh
  0.5.0 install answers on the mock, and the owner's homes are unchanged.
- The same rule in `install.sh` (the source install), whose "already installed" message had the same flaw: before
  0.4.1 it refuses, also with `--upgrade`, and prints `bash <repo>/scripts/uninstall.sh`; 0.4.1 gets
  `scripts/install.sh --upgrade`. Tests in tests/install/test_upgrade_guards.py; both fixes mutation-tested (reverted,
  exactly the six new tests fail).
- docs/release-protocol.md: the gates every release passes, in order, including the install matrix
  (`scripts/install-matrix.sh`: a fresh home and every public release since 0.3.1), the agent-run user session and the
  post-release review. docs/release.md §5b rewritten (it still described the get.sh of v0.2). README's autonomy ladder
  marks v0.5.

### Release (lane REL-050, build/status/REL-050.md)
- History: 46ec430 (TUI-PICKER's goal clock under a TUI-BRAND message, from a shared-index race) reworded as f385527;
  the empty ddf8248 and 07b64d8 dropped; the tree unchanged.
- Leftovers: `scripts/init-shim` and its TUI test case removed; `1 line` in the bash and read summaries; a clippy
  doc-comment lint in tests/upgrade.rs; `/rollback`'s whole-system forms in guide.md; README's Root size counted.

## v0.4.1-preview (2026-10-27)

Workspaces are at version 0.4.1 (seed tag `seed-v0.4.1`). Release notes: docs/release-notes/v0.4.1-preview.md.

### The command is `init-agent` (nagisanzenin/INIT-Agent#1, build/status/REL-041.md)
- The launcher `init` is renamed `init-agent` on all platforms: on Linux `~/.local/bin` often precedes `/sbin` in
  PATH, so `init` shadowed `/sbin/init` in user shells. `scripts/init` is now `scripts/init-agent`; `init-root`,
  `init-tui` and `initctl` keep their names. Every owner-facing string that named the command says `init-agent` (the
  Root's no-workspace notice and access status line, the guided-setup hint, the TUI's hints, the charter template
  comment, the body's access line), as do README and the docs.
- Linux: install.sh never installs a file named `init` (its `put_bin` refuses one), and removes INIT's old launcher at
  `INIT_BIN_DIR/init` and `~/.local/bin/init` when it finds one: exact path, a regular file, the old launcher's header
  line and its `INIT_ROOT_HOME_DEFAULT=` line. Any other file named `init` is left as it is, with a note.
- macOS, this release only: `init` is installed as a shim (`scripts/init-shim`, marker `INIT_AGENT_SHIM=1`) that prints
  the new name on stderr and runs `init-agent` with the same arguments; it is written only where no `init` exists or
  over INIT's own old launcher or shim. The next release drops it.
- install.sh refuses a relative `INIT_BIN_DIR` and one that is `/sbin`, `/usr/sbin`, `/bin` or `/usr/bin`, compared by
  device and inode, so a symlink, `..`, doubled or trailing slashes and case variants match too. It is the first
  preflight check, before anything is written (tests/install/test_install_guards.py; a new ci/local.sh step).
- uninstall.sh moves `init-agent`, `init-tui` and INIT's `init` (the old launcher or the shim, by marker) to the Trash;
  a foreign `init` stays.
- The Root's full-access write deny list (ADR-012, Z3a) now names `<bin_dir>/init-agent` besides `<bin_dir>/init` and
  `<bin_dir>/init-tui` (`access::LAUNCHERS`), so the agent cannot replace the owner's launcher under its new name.
  Tests: `full_lists_keep_only_the_root_protected_set`, Seatbelt selftest S-A23a, the Linux full-isolation test
  (write, unlink and rename refused for all three), `full_body_writes_the_home_not_the_root` (both names).
- Harnesses: release-e2e.sh plants a foreign `init` that install and uninstall must leave untouched; bundle-e2e.sh
  plants INIT's old launcher, which the Linux install must remove and the macOS install must turn into the shim
  (`init --which` = `init-agent --which`, the notice on stderr).

### Release (lane REL-041, build/status/REL-041.md)
- Bundles for macOS arm64 and Linux arm64 from 8539ee8 (the Linux body bit for bit the one built from 7c15772). On
  both: a fresh install with INIT's old launcher planted (removed on Linux, the shim on macOS) and the pause check; on
  macOS also the source install with a foreign `init` kept (`release-e2e.sh`) and a no-op T2 promote (G0–G3). Full
  `ci/local.sh` on macOS: 1,925 passed, 4 failed, 38 ignored, the two failing steps rerun green at 96008ac
  (`kernel test` 887 / 0 / 12, `tui test` 98 / 0 / 1): 1,929 passed, 0 failed, 38 ignored. clippy `-D warnings` clean in
  the changed workspaces.
- Two failures found on the way and fixed: root.md §3.3.2a's verbatim copy of the full tail's Z3a comment said
  init-agent while `zone-tail-full.sb` did not (`doc_drift`; the profile comment now names it, 8539ee8), and the TUI's
  launcher test still ran `scripts/init` (now `scripts/init-agent`, plus a check of the macOS shim, 96008ac).

## v0.4.0-preview (2026-10-26)

Workspaces are at version 0.4.0 (seed tag `seed-v0.4.0`). Release notes: docs/release-notes/v0.4.0-preview.md.

### Release (lane REL-040, build/status/REL-040.md)
- Bundles for macOS arm64 and Linux arm64 from 4a76b12; verified on both (fresh install, the pause check), a no-op T2
  promote on macOS, the GBAL mock flow on the macOS bundle (`tests/real/gbal_proof.sh --mock`, new knob `GBAL_BUNDLE`),
  and one live DeepSeek turn. Full `ci/local.sh` on macOS: 1,929 passed, 0 failed, 38 ignored.
- `cargo clippy --all-targets -- -D warnings` is clean in all four workspaces: the pre-existing lints in the Root
  (`sealed/`, `zone.rs`, `sys.rs`, `egress/money.rs`, `tests/wp22.rs`) and the kernel (`kernel-proxy/config.rs`,
  `kupgrade/kstate.rs`, `init-kernel/upgrade.rs`, `supervisor.rs`) are fixed or allowed with a reason, and one in the
  seed (`boot/gitsync.rs`, a needless borrow from the G2 fix) is fixed. `ci/local.sh` has no clippy step; this was run
  by hand.

### GBAL: goal-directed self-modification (ADR-013, build/status/GBAL.md)
- The owner sets a goal with `/goal "<intent>" [done=…] [usd=…] [wall=…] [rounds=…] [guard=…]` (TUI, `init-root attach`);
  the agent can only propose one. The Root meters the goal's spend, wall time and rounds and stops it at its budget.
- The agent files a prediction before each change (`initctl goal predict …`); the Root timestamps it and marks any
  reading of a change without an earlier prediction `registered: false`.
- Six Root observation cards (`cost_turn`, `wall_turn`, `requests_turn`, `cache_read`, `provider_errors`, `corrections`)
  read from the Root's own egress and owner-channel data; readings carry n, mean, σ, se, z and a verdict.
  `Measure: card:<id>` and `card:<id> gate` join `Measure: g4`; `initctl goal read|measure`.
- Outcomes (card reading, owner accept/reject, guardrail harm) and a Brier score per goal kind; calibration sets only
  the rounds between owner check-ins (1–12), never the owner's budget and never the autonomy ledger.
- Owner guardrail cards (and the agent's own card gate) roll a change back on H12's path.
- Seed skills `goal-loop` (artifact goals: frozen checklist, pairwise judging with position swap, ≥ 3 runs, stop reasons)
  and `gbal` (mode A/B/hybrid by three instrument tests, predict → change → measure → keep at 2σ → stop with a reason).
- Tests: Root `goal::tests` (7), the mock-model acceptance `root tests/gbal.rs` driven by `scenarios/root/gbal.json`.

### Fixes from the real-model demo of 2026-10-25 (the owner's private demo records, F1–F6)
- **F1** A fresh gated install (no sealed S, empty pool) rolled every T1/T2 change back ~1.3 s after its swap (G4
  inconclusive → escalated → `inconclusive_final`). G4 with no instrument is now `accept: no_instrument` at once: kept on
  G0–G3, one `g4_no_instrument` alert (kernel-gate `no_instrument_is_no_evidence_not_a_rollback`, kernel-supervise
  `f7_f11_f13_f14_g4_branches`).
- **F3** An owner `/rollback` of the first free-path change quarantined GENESIS.md, SELF.md and four seed skills: a parent
  tuple without a t0 blob made every T0 cap an `add`. Such caps are now `baseline`: learned, never attributed to a group,
  never quarantined (`f3_seed_caps_on_a_parent_without_t0_are_baseline_never_attributed`).
- **F2** Switching `extend.mode` while the kernel ran was not picked up (the next handoff was refused as
  `unwatched_change`, logged as an L0→L0 demotion, whole-system restart). `charter.apply` now pushes the mode to the
  kernel and the kernel's ceilings refresh re-reads it (`f2_a_charter_mode_switch_reaches_the_running_kernel`). An
  incident at L0 is recorded as `ladder{direction:"floor"}` with an `incident` alert, not a demotion; it still counts
  toward the 3-in-24-h safe-mode rule (`an_incident_at_l0_is_floor_not_a_demotion`).
- **F4** Gate notices said only `[kernel] gate`; they now name stage, verdict, sha and reason with what happens next
  (`f4_gate_notices_say_why`).
- **Docs** docs/guide.md: `/rollback` and `/goal` in the daily-use table; the free path needs `disk_floor_gib` + 2 GiB free.

### Fixes from the GBAL real-model proof (build/status/GBAL-REAL.md)
- **G1** The Root closed an owner turn only after 30 s with no model request, so an owner line sent sooner after the
  reply counted as a steer of the finished turn: on the $0 dry run five questions 2 s apart made one turn and the GBAL
  cards saw no turns at all (`goal read` n = 0, no reading, `met` unverifiable). The kernel's `state{busy:false}` now
  closes the owner turn once its requests have ended (sentinel `kernel_idle_closes_the_owner_turn`); a line typed while
  the body works is still a steer.
- **G3** `initctl goal predict` accepted a prediction for a change already live (it can never be registered) and spent
  one of the owner's rounds on it; on the real run the model amended its commit, promoted, then re-filed, and its goal
  change stayed uncalibrated. It is now refused (`goal.late`, naming the tuple and saying to predict the final commit
  before `initctl promote`), and an abbreviated sha is refused (it never matched the tuple's full sha)
  (`goal::tests::predict_measure_outcome_guardrail_budget_and_stop`).
- `tests/real/gbal_proof.sh` runs: `--mock` ($0, scripted init-mockllm) and the real mode, with the key copied into the
  test Root (`keys set` reads only a TTY), owner lines through `init --raw` (`tests/real/gbal_drive.py`; there is no
  `attach --line`), the spend from `egress stats` `money.total_spent_musd`, a workspace-mode charter generated from the
  template, a spend and disk watchdog, and a verdict that sets the exit status.

- **G2** (serious) A guardrail or gate rollback of a free-path change whose parent was not the kernel's LKG (every round
  after the first while a group stays open) was not applied by the kernel: the bad change stayed live until the Root's
  120 s whole-system fallback, and the agent's own `initctl rollback` meanwhile went all the way to LKG (genesis),
  dropping the good change too. The kernel now rolls back to any parent it reaches through admitted t0 changes only as a
  data-only rollback (the running binary, a new body instance; lineage `main` moves before it boots, and the body's
  `body/` resync now also leaves a rolled-back `evo/<id>` checkout, so the target's T0 data is what the head reads),
  confirmed to the Root by `body_rollback` at once. `initctl rollback` on the free path undoes only the group holding the
  running change: an open group is vetoed (H12), a closed one rolled back to its parent (kernel-supervise
  `g2_rollback_to_an_open_groups_parent_is_data_only_and_prompt`, `g2_data_only_rollback_to_a_non_lkg_target`; seed
  `g2_head_on_a_rolled_back_commit_is_resynced_to_main`).
- **G4** Calibration scored a guardrail rollback as o = 0 even when the prediction forecast that very regression
  (P-000003: `expect=worse`, p 0.9, read worse at z −2.7, Brier 0.81). The outcome now scores the forecast on its own
  card: o = 1 when the card moved in the predicted direction beyond noise; a guardrail or gate rollback is recorded on
  the outcome (`harm`) but is not by itself a miss; a reading with no noise floor scores nothing; a change rolled back
  before its card reading had n turns is read on the turns it had (≥ 3) (`goal::tests::g4_a_predicted_regression_caught_by_a_guardrail_is_a_hit`).
- **G5** The kernel's `Predicts:` grammar refused card ids (`requests_turn`: "unknown metric"), which cost the model two
  amends. `card:<id>` clauses now parse (unscored by the kernel: the Root reads the card and `initctl goal predict` is
  the registered forecast), and a bare card id is refused with the `card:<id>` spelling (`g5_card_clauses_parse`).
- **G6** skills/gbal said to predict "before it goes live", which the model read as "before the next owner turn". It now
  says: on the final sha (after any amend), before `initctl promote`; it also states G4's scoring. seed/self.json
  re-blessed.

- **H2 ack race** An owner line's `owner.in` ack that the kernel sent before link.rs reported the line to the sentinel
  was lost, so the line counted as deaf; two such lines in a row fired H2 (a kernel restart with no group open, else
  every open group rolled back). The GBAL e2e failed about 2 runs in 6 on it. The sentinel now keeps an ack that comes
  before its line (last 64) and the line is born acked (sentinel `an_ack_before_its_line_is_not_deafness`).

### Fixes after the GBAL rerun (build/status/GBAL-REAL.md "Rerun", build/status/REL-040.md)
- **G7** The kernel's commit rules refused the promote *after* the prediction (8 refusals in 3 real runs: an unknown
  subject type, a missing `Why:`, prose in `Repro:`), each amend made the filed prediction stale, and every re-file spent
  one of the owner's rounds and one check-in step: run A used up `rounds=3` before anything went live. Three changes:
  `initctl promote --check evo/<id>` runs promote's synchronous commit checks (fetch, candidate checks, the validator under
  the current `extend.mode`, the scope) and answers at once, with no job, no quota ticket, no gate and no record
  (kernel-supervise `promote::check`, kernel-serve `Backend::promote_check`; `f1_f2_f3_f4_f9_failures_before_and_at_the_handoff`
  asserts the check and promote report the same codes, and that a good commit passes with nothing left behind; initctl
  golden rows); a re-file that replaces the goal's newest prediction while its change never went live keeps the round,
  spends no round and no check-in step, and closes the old prediction `superseded` with its pending readings, so it never
  registers (`goal::tests::g7_a_refile_before_live_supersedes_and_spends_no_round`); skills/gbal runs `promote --check`
  before predicting and checks the prediction's answer before promoting, and skills/extend lists the subject types and
  the exact `Repro:` forms. `initctl goal status` lists the open predictions.
- **`met` requires attribution** (owner decision). A card goal used to be `met`, verified, whenever the newest kept
  reading reached the threshold, also on noise (runs B and C: 2.4 → 2.0 requests per turn, z +1.64, threshold ≤ 2). It is
  now `met` only when, in addition, a registered change moved the goal card beyond noise in its prediction's direction
  toward the threshold (`attributed` on the stop record); reaching the threshold otherwise is recorded as
  `met_unattributed` (`claimed: "met"`, `verified: false`) with a `goal_met_unattributed` owner alert
  (`goal::tests::met_requires_a_registered_change_beyond_noise`; ADR-013 decision 5; skills/gbal).
- **Goal meter = egress ledger.** The per-goal money meter added each closed turn's cost, so it missed requests that
  settled after their turn closed and the turn in which the agent stopped the goal (rerun C: 51,290 against the ledger's
  59,718 µUSD). The Root now charges every egress request it settles while the goal is active, at settlement, with the
  `end` record's own `cost_musd`; the money budget is checked there too (`the_budget_meter_stops_the_goal`: paused spend
  is not the goal's, no double count; the mock-model e2e `tests/gbal.rs` asserts goal meter = egress ledger to the µUSD).
- seed/self.json re-blessed (skills/gbal, skills/extend).

## v0.3.3-preview (2026-10-25)

Workspaces are at version 0.3.3 (seed tag `seed-v0.3.3`). Release notes: docs/release-notes/v0.3.3-preview.md.

### DeepSeek: no refused request after a pause (f3f8dbf, docs/body/provider.md §3.8.1, build/status/REL-032.md "Observation")
- Every assistant message on the DeepSeek wire now carries `reasoning_content`, `""` when it has no DeepSeek
  thinking: a turn paused before its first token (`[no visible output]`), a tool call whose reasoning came back
  empty, and a foreign turn after a provider switch. Before, each such resume was refused once (400 `reasoning_content
  missing`) and the `deepseek_empty_reasoning_foreign_turns` fallback retried; the fallback stays as a safety net.
  Both request builders (the context renderer the loop sends, and `Provider::build`) changed together.
- Tests: `pause_before_first_token_resumes_without_400` (seed/tests/boot.rs, strict DeepSeek mock: the resume is one
  accepted request, no 400, no fallback); the strict-mock foreign-turn test now expects an accepted first request and
  checks the fallback on a body without the field.

### Release (lane REL-033, build/status/REL-033.md)
- Bundles for macOS arm64 and Linux arm64 from cb83a41; verified on both (fresh install, the pause with the strict
  DeepSeek mock: two requests and no 400), a no-op T2 promote on macOS, and one live DeepSeek turn.

## v0.3.2-preview (2026-10-25)

Workspaces are at version 0.3.2 (seed tag `seed-v0.3.2`). Release notes: docs/release-notes/v0.3.2-preview.md.

### An owner interrupt pauses the task (67cf3c1, docs/body/loop.md §3.5.1)
- Esc or Ctrl-C while busy used to end the task; an interrupt before the first token left an empty assistant
  message, and the next run read the owner's words as a new request. The input thread now records owner interrupts
  (`Abort::trigger_owner`, `trigger_owner_continue`), and a run that ends aborted with that record appends one harness
  message `[init:paused] …` after the results: finished work stands, calls marked `aborted: not executed` did not
  run, and the owner's next message resumes the task unless it changes or drops it. The loop shows `paused · type to
  continue, or say what to do instead` instead of `aborted`.
- The `stop` grace, EOF and faults still abort, show `aborted` and add no note; interrupt-and-steer that continues
  the run adds no note. The note is appended once and never rewritten (the prefix cache is unaffected).
- TUI: the Esc hint says `pausing · type to continue, or say what to do instead`; the Esc help line says pause.

### Release (lane REL-032, build/status/REL-032.md)
- Two prebuilt bundles, `init-v0.3.2-preview-macos-arm64.tar.gz` and `init-v0.3.2-preview-linux-arm64.tar.gz`, each
  with a `.sha256`, and one `latest.json` for both, built from 37be317 (`seed/self.json` re-blessed for the seed
  change). Verified on these exact bundles: fresh macOS and Ubuntu 24.04 installs, a no-op T2 promote (G0–G3
  and handoff, 13 min cold on an M2), the pause on both bundles (a stalled turn paused by SIGINT, then resumed with
  the `[init:paused]` note in the next request) and one live DeepSeek turn.
- Docs: docs/guide.md and docs/tui.md say Ctrl-C and Esc pause the turn.

## v0.3.1-preview (2026-10-24)

Workspaces are at version 0.3.1 (seed tag `seed-v0.3.1`). Release notes: docs/release-notes/v0.3.1-preview.md.

### Release (lane FULLACCESS-release, build/status/FULLACCESS.md "Release")
- Two prebuilt bundles, `init-v0.3.1-preview-macos-arm64.tar.gz` and `init-v0.3.1-preview-linux-arm64.tar.gz`, each
  with a `.sha256`, and one `latest.json` for both, built from e4215ed. Verified on these exact bundles: fresh macOS
  and Ubuntu 24.04 installs (access FULL by default), a no-op T2 promote (G0–G3 and handoff, 17 min cold on an M2) and
  one live DeepSeek turn.

### Full access by default (ADR-012, lane FULLACCESS, build/status/FULLACCESS.md)
- **The live body now has full access by default:** it reads, writes and runs anything the owner can and uses the IP
  network, except the Root-protected set: `ROOT_HOME` (files and its sockets), the Root's service file and directory
  (`~/Library/LaunchAgents`, `~/.config/systemd/user`), INIT's launchers (`<bin_dir>/init`, `<bin_dir>/init-tui`),
  `INIT_HOME`'s Root-written parts and marks, `~/.config/init-agent` and the charter's `sandbox.read_deny`; no signals,
  XPC, LaunchServices or AppleEvents outside its sandbox. Fleet members and probes never get it.
- ADR-009's behaviour is kept, unchanged, as **workspace mode**: charter `[sandbox] mode = "workspace"` (new key,
  default `"full"`), `init --workspace` for one session, or `/full off`. `/full on` needs a typed confirmation only
  when the charter says `"workspace"`. `allow_full = false` still forbids full access.
- A full grant sets the body's `HOME` to the owner's home (so `~/Downloads` is the owner's), drops
  `CARGO_NET_OFFLINE` and adds `~/.local/bin` and `~/.cargo/bin` to `PATH`. The body's git sync now runs with
  `GIT_CONFIG_GLOBAL=/dev/null`, so the owner's `~/.gitconfig` does not affect it. The boot block's access line says
  what full access allows and denies.
- macOS: a full ROOT_ALLOWS group and a deny-only tail `zone-tail-full.sb`. Linux: bubblewrap without a network
  namespace, the host root bound read-write with the protected set hidden or read-only, and a seccomp program without
  the socket-domain rule. Full access works on Linux; workspace mode there stays the agent's own home.
- TUI: `--workspace` (excludes `--full`), a `FULL` (red) or `WORKSPACE` (yellow) badge that is never dropped, new
  `/full` help; `init-root status` shows the access mode. `install.sh` states the default and how to confine it.
- **Residuals** (ADR-012, build/status/RED-TEAM.md §6): writes outside the start folder have no `/undo`; files the
  owner's tools run later (shell startup files, `PATH` directories, editor configs, git hooks) are writable, so full
  access is trusted like the owner's shell; anything readable may reach the provider or the network;
  `launchctl remove dev.init.root` from inside any sandbox stops the Root (fail-safe: it ends INIT, nothing escapes);
  on Linux, abstract Unix sockets and the system D-Bus (logind) stay reachable.

## v0.3.0-preview (2026-10-24)

A research preview for macOS (Apple silicon) and, experimentally, Linux aarch64. Workspaces are at version 0.3.0
(seed tag `seed-v0.3.0`). Release notes: docs/release-notes/v0.3.0-preview.md. Autonomy stays capped at L0.

### Release and install (lane REL-030, build/status/REL-030.md)
- Two prebuilt bundles: `init-v0.3.0-preview-macos-arm64.tar.gz` and `init-v0.3.0-preview-linux-arm64.tar.gz`
  (built on Debian 12, glibc ≥ 2.36), each with a `.sha256`, and one `latest.json` for both.
- `get.sh` installs Rust 1.97.1 with wasm32-wasip2 and clippy through rustup (no shell-file or default-toolchain
  changes; `--no-rust` opts out), so T1/T2 self-rewrite works on a fresh install. Clippy was missing from the
  minimal profile, and the first no-op T2 promote on a fresh bundle install failed G0 (`gate.g0.clippy`); install.sh
  now records a toolchain only when it has clippy. The first T2 promote on a fresh install
  builds cold (measured on an M2: G0 16 min, 26 min end to end with G3); later ones reuse `cache/deps` (about 2 min
  for a small change).
- `latest.json` carries `sha256_<target>` for every bundle; `get.sh` reads its own target's checksum from it (the
  `.sha256` next to the bundle stays the fallback).
- `scripts/bundle-e2e.sh` follows get.sh's default: Rust is installed into the test home (`RUSTUP_HOME`/`CARGO_HOME`
  there), so the plain run checks a T2-capable install and `--t2` no longer needs the maintainer's rustup.

### Steering: fluid and visible (lane STEER, build/status/STEER.md)
- A pending tray above the input box shows each owner line until the body reads it; it then moves into the
  transcript as `· seen` (the kernel's `state` frame carries `steer_pending`/`steer_seen`/`steer_retracted`).
- Esc with a steer pending interrupts and continues with the note (`interrupt{then:"continue"}`); Up on an empty
  box takes back the newest unread line (`retract{seq}`); Tab queues the box as `/later`.
- `init-root attach` sends a bracketed paste (or `\` continuation) as one line. Line mode prints
  `[steer queued]`, `[steer seen]` and `[steer retracted]`.

### Sub-agents: the built-in `delegate` tool (lane DELEGATE, build/status/DELEGATE.md)
- Ported from the `delegate` tool INIT wrote for itself. A batch's delegates run in parallel (default 4, at most 8),
  results in call order; writers run one at a time. Modes `read` (default; write/edit refused, bash and ext are not
  confined) and `write`. Bounds per sub-task: 8 turns, 24 tool calls, 12 requests, 1M tokens, 600 s wall; no
  recursion; an owner abort reaches every sub-task. A durable record per sub-task under `state/spill/`.
- Sub-requests carry `x-init-subtask: delegate-<call id>`; the TUI nests sub-task calls under the delegate row.

### Root sentinel: steering and sub-tasks are not harm (lane MERGE, build/status/MERGE.md)
- An owner correction is now only an interrupt (Esc/Ctrl-C, also `then:"continue"`), `/stop`, `/panic`, `/undo`,
  `/reject` or `/rollback`. Plain steers go to a weak `steered` count; `/later` counts as nothing; a retracted line
  takes its steer back. The `v2_fix` long-session behaviour is unchanged.
- The kernel proxy forwards a well-formed `x-init-subtask` to the Root as `x-egress-subtask` (never upstream). H10
  (requests/turn) and H5b (cost/turn) count only the main loop's share, so `delegate` is not read as a harm spike.

### Warm T2 builds (lane MERGE, build/status/MERGE-T2CACHE.md)
- The kernel's `cache/deps` is now filled and reused: on a miss a stub workspace builds only the third-party
  dependencies before the candidate's code runs; the entry is published only after the candidate's G0 passes, and a
  hit is used only when its tree digest matches. Vendor mtimes are pinned and slot builds use a fixed
  `SOURCE_DATE_EPOCH`, which forced rebuilds before. Measured on the real seed under the real Seatbelt profiles
  (mock, load 10–23): first promote 1029 s (build 925 s), second promote of a small change 126 s (build 32 s).

### Linux: native support (lane LINUX, build/status/LINUX.md)
- **Verified:** Debian 12 (build and tests) and Ubuntu 24.04 (no-Rust bundle e2e), Linux 6.10, aarch64, in Docker at
  $0 on the mock model. **Not verified:** x86_64 (the scripts build it, but nothing ran there), real hardware or a VM,
  Ubuntu's AppArmor user-namespace restriction, SELinux, kernels before 6.1, WSL2, and a live provider on Linux.
- **The zone:** bubblewrap ≥ 0.13.0 at `/usr/bin/bwrap`, which you build from the upstream release (docs/platforms.md
  §3.4.0), plus seccomp and an empty network namespace. The zone now keeps the owner's uid and gid. The cc1 fix
  (c4ab500) is qualified natively, and the install-mode kernel builds the genesis body inside the zone.
- **Memory and disk:** the macOS watchdog semantics now hold behind bwrap; one tool call is killed, not the whole
  instance. A cgroup v2 `memory.max` backstop applies per workload where the systemd unit delegates memory. The disk
  floor is unchanged.
- **Install:**
  - `get.sh` picks `init-v<version>-linux-arm64` or `-linux-x86_64`.
  - `install.sh` checks bwrap, user namespaces, python3, git and a systemd user session, then enables
    `~/.config/systemd/user/init-root.service` (skipped with `--no-service`).
  - `uninstall.sh` disables the service and moves everything to the freedesktop Trash.
- **Limits on Linux:** ADR-009 home reads, workspace writes and full mode are capped off, so the agent works in its
  own home only. Keys are 0600 files. No Landlock yet.
- **Root fixes found by the native suite:**
  - the kernel's own exit code (78 config, 75 swap) is recorded behind bwrap;
  - the probe kernel keeps fds 3 and 4;
  - unbound lane sockets no longer refuse a body;
  - Linux test portability;
  - `ci/local.sh` builds root and seed before the kernel tests, and gets a native Linux zone step.

## v0.2.0-preview (2026-10-23)

A research preview for macOS. It folds in v0.1.1, which was never cut, and everything since: reward-free self-extension (ADR-011), the memory watchdog, observe-only spending, provider and Linux groundwork, and the CI fixes. There is no evidence yet that self-evolution improves results (see "Research results" below). Autonomy stays capped at L0.

### Self-extension: ADR-011 (docs/adr/011-reward-free-self-extension.md)
- **Two modes.** The charter's `[extend]` section (rule C17) sets `extend.mode = "gated"` (the default: every change passes G0–G4) or `"free"`. On the free path a change is admitted by mechanical checks A0–A5 only; G4, the sealed set and forecasts become an opt-in `Measure:` trailer, and `Predicts:` is optional (X1, X2, INTEG-A).
- **The Root harm sentinel** (`root/crates/init-root/src/sentinel/`): generic harm signals H1–H12 counted per session, with exact exceedance and binomial tests, change groups and their windows. It rolls back the group that caused harm and never needs a task-specific reward. Under `v2_fix`, H5b, H8, H10 and H11 use the 8th largest of a ring of 40 at α = 0.005, so they stay live in long sessions (X3, X10).
- **Reversibility.** Root system snapshots, R2 restore, the A4 rollback probe in Root-owned probe roots, the body's `check` mode and the `settled` pointer (X4).
- **Attribution.** The Root's egress tap, the body's `x-init-caps` forwarded as `x-egress-caps`, the usage ledger, `cap_use` digests and `caps.json` (X5).
- **Selection by use, levels and grants.** Decay and archive of unused capabilities, Fisher-test quarantine, a bloat budget; levels L0–L4 from records; owner grants with expiry through `/grants`, `/grant <id> [for <n>d|<n>h]` (typed confirmation) and `/deny <id>`; an owner-corrections trend that blocks level-ups. RT-GAP-1 (self-reported promotions could unlock L1) is closed (X6, X7).
- **The frozen twin.** A nightly shadow replay of a frozen earlier version on sealed lanes, with an exact signed-rank cost test; report only, re-frozen only by the owner (`/twin`, `/twin refreeze`) (X9).
- **The agent knows.** An awareness line and the `extend` skill tell the body which mode it runs in (X11).
- The broker's `root.unwatched_change` check: on the free path a live body must run an admitted body binary (INTEG-A).

### Evolution engine and the measuring stick (proof sprint, 2026-10-20 to 10-21)
- `kernel-evo`: a friction miner over the agent's own traces, proposals, kernel-built variants, a screen with a go/no-go estimate, and `initctl evo frictions|propose|variant|screen|status|abandon` (EVO-LITE).
- G4 v2: an exact sign-flip test with a non-inferiority margin and A/A calibration, per-run caps, a pool cache and failing-run log retention (GATE-V2a, GATE-V2b, ROOT-S1b).
- Sealed set S v1: pack install and draw, the G4 slice, canaries and tamper checks, calibrate, probe and milestone commands; the `s1` pack has 56 templates. A visible pool `pool-v1` of 20 tasks (ROOT-S1a, S1-*, POOL-LITE).
- Real-model harnesses with money and memory guards: `tests/real/m7.sh` and `tests/real/evo_test.sh` (REAL-M7, EVO-TEST-PREP).

### Memory watchdog and workload disk floor (MEM-RUNAWAY, 2026-10-22)
In M7 one sandboxed process grew to 43.9 GB of compressed memory, and swap filled the disk to 0.24 GB free. Seatbelt has no memory limit, so the agent's bash tool had the same exposure.
- **The Root polls every workload each second** (zone instances and sealed runs). A tool call's or build step's process tree whose memory (resident + compressed) passes `sandbox.mem_cap_mib` (charter, default 4096 MiB, 512–65536) is killed alone, by pid. The Root and the kernel are never killed (MEM-RUNAWAY).
- **Disk floor.** Below `sandbox.disk_floor_gib` (default 5 GiB) of free disk, the Root refuses new workloads and kills the running ones; the agent and the kernel stay up (MEM-RUNAWAY).
- **The agent is told.** bash reports `process exceeded memory cap N MiB, killed` as a tool error, and refuses new commands while the disk is below the floor. The owner gets an alert, `mem_cap` or `disk_floor` (MEM-RUNAWAY).
- Linux (cgroups `memory.max`) is not built yet.

### Permissions: Codex-style (owner decision 2026-10-20; docs/adr/009-codex-style-permissions.md)
The owner found v0.1.0's sandbox too strict for daily work: the agent could not even read a project outside `~/.init`. It now works like Claude Code or Codex, while the Root, the keys, the signed lineage, the gate and the network stay as they were.
- **Read your home, except secrets.** The live body reads the owner's home, minus a deny list that holds in every mode: `~/.init-root`, `~/.config/init-agent`, `~/.ssh`, `~/.gnupg`, cloud and registry credentials (`~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/.kube`, `~/.docker/config.json`, `~/.netrc`, `~/.npmrc`, `~/.pypirc`), keychains, cookies, Mail, Messages, Safari, browser profiles, 1Password and `~/Library/Group Containers`. The charter adds entries with `sandbox.read_deny` (up to 32) and turns the home read off with `sandbox.read_home = false` (SBX-009).
- **Write the workspace.** `init`'s cwd is the workspace: the TUI sends it once per launch (`workspace.set`), and the Root validates it (inside the home, not the home itself, not a Root path, not on the deny list). The body then writes `~/.init` plus that folder. Launched from `~`, the agent can read but not edit (SBX-009, WIRE-009).
- **`init --full` and `/full on|off`.** Full mode makes the whole home writable except the deny list and the Root's paths, after a typed confirmation (`/full on confirm`) the first time in a session. The charter can forbid it (`sandbox.allow_full = false`), and the status bar shows `FULL` (SBX-009, WIRE-009).
- **Snapshots and `/undo`.** The Root takes an APFS clone of the workspace into `~/.init-root/snapshots/` when the workspace is set and when full mode turns on. `/undo` (then `/undo confirm`) restores the latest one, after saving the current state, so an undo can be undone. The Root keeps 5 snapshots per workspace; a folder over 2 GiB, off APFS or on another volume gets none, with a notice (SBX-009).
- **Enforced by the Root's tail.** Two Root-generated grants (`Z_HOME` read, `Z_WRITE` write, both excluding `~/.init`), per-entry deny lines in tail Z2 plus a write deny on `~/Library/LaunchAgents`, and `Z_WRITE` in tail Z3. The kernel's `spawn` parameters are unchanged, and the Root sets `INIT_ACCESS` and `INIT_WORKSPACE` on every body spawn. Probes stay hermetic; fleet members read but do not write the home (SBX-009).
- **The agent knows its access.** The body reads `INIT_ACCESS` (`none`, `read_home`, `workspace`, `full`) and `INIT_WORKSPACE`, starts its tools in the workspace, and states its access in one line of the boot block, so the cached prompt head does not change. A workspace change restarts the body at an idle boundary on the same conversation (WIRE-009).
- **No unsandboxed execution through the obvious paths.** In every mode, tail Z3a write-denies the shell startup files, `~/.gitconfig` and `~/.config/git`, `~/.local/bin`, `~/bin`, login items (LaunchAgents, autostart, systemd user units) and the installer's `bin_dir` (`init-root install --bin-dir`). Tail Z3b denies `.git/hooks`, `.git/config` and the same under `.git/modules/` in every repository outside `~/.init`; commits still work (SBX-009).
- **`/undo` waits for an idle agent.** It answers `root.busy` while the body is busy, before asking for the confirmation. Every access change is a path-free `access` lineage record (SBX-009).
- **Known risk.** Workspace build and task files (Makefile, package scripts, `.envrc`) can still run with the owner's rights later, and in full mode so can editor configs, language tool directories and other PATH directories (ADR-009, Threat model delta).

### Spending: observe only (owner decision 2026-10-23)
- **The Root no longer enforces dollar limits.** Live, sealed, audit and twin requests are never refused, stopped or demoted because a dollar amount is reached. The Root's money block reports `"enforced": false, "policy": "observe"`. `money.day_usd`, `money.total_usd`, `sealed.reserve_usd` and the twin's dollar keys are kept as metadata; the charter no longer checks them against each other (C5, C12, C17).
- **What stays enforced:** request validation, a bounded output size on every request, per-class request counts, bytes per day and rate per minute, and run time, request and token limits.
- **Accounting.** Every metered request is logged with the provider's token usage and a tariff-based cost estimate. Missing usage keeps the reservation estimate and marks token totals incomplete, never zero. `total_reserved_musd` shows in-flight estimates.
- **The kernel** turns its dollar soft budgets off when the Root reports observe-only money; an older Root without the marker keeps the old behaviour.
- **The TUI** shows the estimated live total, live tokens and today's evolution-check cost in the status bar, with a `/usage · /stop` hint, and `/usage` prints the full estimate locally.
- **Risk:** a self-modifying agent can spend until the owner stops it (`/stop`, `/panic`). Set a spend limit at the provider.

### Providers (groundwork, experimental; docs/provider-configuration.md)
- Owner-configured provider aliases with `api = "chat" | "responses" | "messages"`, a literal `base_path`, exact destinations and explicit tariffs. Redirects and ambient HTTP proxy settings are disabled.
- Local servers on `127.0.0.1` or `localhost` without a key (`local = true`, `auth = "none"`).
- Model descriptors in the kernel config: context window, output limits, `request_max_tokens`, `chat_stream_usage`, `native_extensions`.
- Offline tests only. DeepSeek is still the only provider used against a live API and the only one in the installer.

### Linux (groundwork, unverified)
- A fail-closed Linux mount plan (`root/sandbox-profiles/src/linux.rs`) and a Linux zone in the Root: fd-pinned mounts, network isolation and pidfd-tracked STOP/KILL, which passed CPU tests on Modal. Broad home and workspace grants are refused on Linux until their deny overlays exist.
- A native Linux build test failed (the compiler could not run `cc1` inside the build namespace); the fix (`c4ab500`) is not yet qualified. No cgroup memory limit yet.

### Kernel upgrade and release qualification (groundwork)
- Kernel promotion build, admission and swap orchestration, the Root's conformance probe and durable verdict replay (K8, `init-kernel` `upgrade.rs`). Real kernel probation and rollback are not qualified, and the L0 cap keeps kernel evolution off.
- A release A/B harness (`tests/real/release_ab/`, 48 paired tasks, a DeepSeek gateway with reservations and cache accounting) and a platform release gate (`tests/platform_release/`). The A/B has not been run (0 of 48 tasks).

### Fixed
- A proposal interrupted during G4 could be stranded; the kernel now resumes proposals from the Root's records at start (STRANDED-PROPOSAL).
- G3 stage bounds are now derived from the build timeouts (G3-STAGE-TIMEOUT, found when a cold build slot timed out G3).
- Probe roots made from the read-only pool clone could not be deleted (probe-root leak).
- macOS CI after `42ac6e5` (`107555d`): the grant merge test still asserted the removed money rule; `GENESIS.md` was over its 1,400-byte budget; `seed/self.json` had drifted; the TUI end-to-end test still expected a ceiling refusal.

### Research results
- **Proof sprint** (2026-10-21, deepseek-flash, $2.67): the loop ran end to end on a real model and one self-change reached G4, which did not establish non-inferiority (d = −50‰, inside the A/A noise of 57‰); the Root rolled it back. Per the erratum, this does not show the change was harmful. `reports/INIT proof sprint verdict.md`, `reports/INIT technical report.md`.
- **ADR-011 evolution test** (2026-10-22, $0.94): on deepseek-flash no task was found where INIT failed and a missing mechanism was to blame; where INIT failed, Claude Code on the same model failed too. The evolution phases never ran, so self-evolution is untested, not disproven. `measurements/evo-test-protocol.md` §10–§11.

### Known limitations
- macOS only. Linux is unverified; Windows is unsupported.
- Autonomy is capped at L0 (`SUPPORTED_MAX_LEVEL = 0` in release builds).
- No evidence yet that self-evolution improves results. The free path of ADR-011 has not run on a real model.
- No real-model request has been made on the code since `42ac6e5`; the changes since are checked at $0 on the mock model and in macOS CI.
- Spending is not capped by INIT (above).
- The install wizard still asks for a daily and a total "spending ceiling"; the answers are stored as metadata and not enforced.
- Experimental providers have offline tests only. Native Gemini, Ollama's native API, Bedrock, Azure and Vertex authentication are not built.
- In `free` mode under strongly clustered sessions the simulated null false-rollback rate is 3.09%, above the 2.5% bound; `gated` stays the default.
- `root.unwatched_change` can be bypassed by a kernel using a wrapper or an interpreter (only relevant above L0).
- There is no sub-agent runtime (`fleet_spawn` answers "not built").
- The sentinel is about 1,050 lines, against an ADR estimate of about 380.
- Eight test-home unit tests are skipped without the `test-fixtures` feature (their paths exceed the production 64-byte limit).

## v0.1.0 (2026-10-20)

The first owner-usable release: one command installs INIT on a Mac, and it chats on DeepSeek through the Root. Autonomy is capped at L0 in this build.

### Install and use
- `scripts/install.sh`: one command. It builds the release binaries, asks a short charter wizard (purpose, daily and total ceilings, max level, provider, model, disk floor; `--yes` takes the defaults), runs `init-root install` with the seed bundle, writes `kernel/config.toml`, installs the `init` command and loads the LaunchAgent. It never touches the provider key (REL).
- `scripts/release.sh`: the release directory (`target/dist/v0.1.0/`): `init-root`, `init-kernel`, `initctl`, `init-mockllm`, `init-body`, `init-tui`, the kernel source and seed bundles, the kernel docs, WIT and the template extension, with a `MANIFEST.json` of hashes. It runs the release-mode tests of root and seed as its quality guard (REL).
- `scripts/uninstall.sh`: stops INIT, unloads the service and moves both homes to the Trash after a typed confirmation; it never deletes (REL).
- `init`: opens the TUI when it is installed, else `init-root attach` (REL).
- **The `init` TUI** (`init-tui`, a new `tui/` workspace): a terminal client of the Root's owner socket and nothing else. It holds no keys, writes no files and is never linked into `init-root`. It streams text, shows one row per tool call, a transcript view (Ctrl-O), a multi-line input box with history and `/` completion, and a status bar with level and cap, model, run cost, today against the ceiling and the Root's state. Ctrl-C interrupts, a second Ctrl-C detaches, and `/panic` falls back to a fresh connection and then to `init-root panic` when the attached session cannot deliver it. It reconnects across Root restarts without resending a line, and `--plain`/`--raw` give a line mode for scripts (WP26; docs/tui.md).
- The release ships `init-mockllm`, the $0 mock used by the gate's G0 tests; it makes no network calls. Every install copies it into `kernel/bin/`, and `install` refuses a release directory without it (G0-ROOT).

### The system
- **Root** (`init-root`): charter (R5) and its validation, owner socket and CLI, cold start, kernel supervision with crash-loop escalation and system safe mode (R1), whole-system tuples, LKG and rollback, panic and stop, the spawn broker with the macOS zone profiles, the egress gateway with keys and integer micro-USD ceilings (R2), signed lineage with verification and a mirror (R4), the minimal sealed evaluation (R6) and the autonomy ledger (R7). The install path builds the seed in the kernel's install mode and forms the genesis tuple (WP20–WP25, WP21b).
- **Kernel** (`init-kernel`): supervise with overlapped handoff and rollback (WP4), the model proxy with lanes, soft budgets and traces (WP5), the gate G0–G4 and the gate-of-gates judge (WP8, WP8b, WP8c, FIX2), the self-map (WP9), the `initctl` server (WP7, WP7b) and the macOS sandbox (WP6a).
- **Body** (`init-body`, the seed): DeepSeek, Anthropic and OpenAI providers (WP10), the loop and five tools (WP11), the event log and context assembly with a bounded view (WP12, FIX8), boot and awareness files (WP13), and the Wasmtime 48 extension host (WP14, FIX3).
- **Test kit**: the $0 deterministic model `init-mockllm`, fake body, kernel and Root, the scenario runner (WP2, WP3); acceptance A1–A6 and NO-OWNER-1 green at $0 (WP15); Phase 0 measurements M-1, M-2, M-4 (WP16) and M-5 (M5 lane) pass.
- **CI**: `ci/local.sh` (the full macOS suite) and the GitHub `fast-linux` workflow (the portable kernel crates; the seed as an informational job). `ci/local.sh` gains three steps: **seed G0**, the seed passing its own G0 (build, check, clippy, ext and `cargo test` under the real Seatbelt texts with the shipped mock; G0-SEED); **promote under Seatbelt**, the quick gate and a promote G0 → G1 → G2 → G3 → handoff → G4 → LKG with every instance checked by the Root's own spawn rules and run under `sandbox-exec` (K-GATE); and the TUI's build, tests and end-to-end run against the real Root in test mode (WP26).
- **Tests that cannot hang**: every process-spawning kernel test arms a watchdog with a measured bound and spawns its children so they cannot inherit its pipes (K-FLAKE).

### Safety: owner-machine mode (ADR-008)
- The Root signals only processes it holds: `sys::signal` refuses any pid that is not its own unreaped child, so panic, stop, rollback and crash handling reach only INIT's instances and their own process groups. Cold start never signals a pid a previous Root recorded (R-SAFE).
- The process sweep (`live-sweep`) is compiled out: no feature implies it, a release build that names it fails to compile, and even a debug build with it scans only on a disposable account marked by a root-owned sentinel (R-SAFE).
- `init-root doctor --sweep-dry-run` applies the sweep rule to the live process table and lists what it would kill, sending no signal. On the owner's account it selected exactly the test's own instances across 448–475 processes (R-SAFE).
- A process that leaves its group (`setsid`) and survives a panic is reported in an `escapee` lineage record and an owner alert, never killed (R-SAFE; review F5).
- `resume` is refused while the lineage mirror's `refs/heads/panic` branch exists, and the Root raises its open-file soft limit to min(hard, 4096) at start (R-SAFE).

### Security review fixes (build/review/root-security-2026-10-20.md: 0 critical, 1 high, 3 medium, 4 low)
- F1 (high): money admission cannot overflow; a request's `max_tokens` is clamped, and a `u64::MAX` request is refused at the ceiling (R-DEC).
- F2: the egress meters by default-deny: every billable route is an explicit `(provider, method, path)` row, and anything else is refused before a key is attached (R-SEC).
- F3: at most 96 egress connections (32 per sealed lane) with timed reads and writes; beyond that, `root.busy` (R-SEC, R-DEC).
- F4: the held reservation counts input at one token per request byte, so the money ceiling is a hard wall (R-DEC).
- F5: escapee detection (above). F6: the fd stash keeps at most 64 descriptors for at most 10 s (R-DEC). F7: the health and ceiling endpoints sit behind the in-flight guard (R-SEC).

### Provisional owner decisions (2026-10-20; the owner may override any of them)
The owner said not to wait, so the orchestrator took these for v0.1.0 (RELEASE_PLAN.md; docs/CONVENTIONS.md §6):
- The Root's size budget is ≈ 7,500 lines as per-module sub-budgets (measured + 5%), re-measured at release.
- G4 moves to n = 30, R = 3 plus an A/A calibration per model once the sealed set S has 30 tasks (M7); v0.1 keeps the 12-task stand-in S.
- The constitution is built as written and stays PENDING OWNER.
- M-5 is measured with `phys_footprint` on macOS, baselined at the first context epoch, thresholds unchanged. Under it M-5 passes 3 of 3 (M5).
- Three demotions at L0 within 24 h put the system in safe mode (R-DEC).
- The Root bills cache hits at the provider's cache-read price and keeps the peak window: S1's traffic went from 3.73× to 2.00× the proxy's meter (R-DEC).
- A sealed set S below 12 tasks is refused at import, and a gate-of-gates on a smaller S records `underpowered`, alerts and does not demote (R-DEC).
- The real-model batch uses deepseek-flash only, with a hard cap of $1.00 Root-metered (S-REAL).
- No disposable account is required for v0.1, under owner-machine mode (ADR-008).

### Fixed during the release push (build-deviations.md §6)
- The seed failed its own G0: integration-test roots lacked `#![forbid(unsafe_code)]`, so every T2 promotion and quick gate was rejected (`fade67a`).
- G0's `cargo test` failed the seed in a real install: the `build` profile now allows Unix sockets under the build's `OUT` only (and the kernel profile the same under the build slot's `OUT`, while G0 runs as kernel children); `init-mockllm` ships in every install; the seed's tests keep their homes and sockets in the target dir (`d05c1e3`, `eec8d5e`, `3008357`, `f76e749`).
- G1 rejected every candidate with `gate.g1.manifest_error`: its sandbox `OUT` did not hold the candidate's `bin/` and `ext/` (`70f30d3`).
- A body display line near 1 MiB, or a fast stream, broke the kernel's link to the Root and counted as a crash: the kernel now splits, cuts and paces owner frames (`15dbf74`), and the Root refuses owner lines over 900 KiB (`44b9c6e`).
- A handoff could wait forever when the new body crashed while the old one retired (`5e50940`).
- Build timeouts from `config.toml` were ignored, and a build could wait on a descendant holding its pipes (`28f204d`, `2794924`).
- `initctl gate --quick` could not fetch `HEAD` or a snapshot commit (`f8c212d`), and `selfmap --root .` broke extension builds (`40600e0`).

### Known limitations
- L0 only (`SUPPORTED_MAX_LEVEL = 0` in release builds); macOS only (the Linux sandbox is unverified); DeepSeek only for the installer's wizard. The off-machine lineage mirror is off until its repository exists.
- **Panic and escapees** (ADR-008 residual risk): a zone process that calls `setsid()` leaves its process group and survives a panic until `live-sweep` is enabled on a build verified on a disposable account. It keeps the zone's sandbox (no IP network, no access to the Root's files or keys), its egress socket closes at panic, and it is reported by pid so the owner can end it.
- The process sweep (`live-sweep`) stays compiled out on the owner's machine until the disposable-account tests run (ADR-008).
- G0 runs the candidate's tests as the kernel's children in the kernel's sandbox profile, not in separate `build` instances; the socket rule for G0 in the kernel profile is temporary.
- M-5 passes on the provisional rule only (it fails on `ps` RSS from step 100), and the ratio's margin is thin (worst 1.198 against 1.25), with peaks at compaction. M-3 (agents writing extensions) is not measured.
- Every agent T2 attempt runs a cold `selfmap` build of the body (about 1 GB and 5 min inside the zone), and the build stage's environment differs from G0's, so wasmtime is probably recompiled for about 2 min on every T2 gate.
- Sealed evaluation runs are not in the Root's spawn-broker table. They stay within `max_instances` through the zone's quota, so this is bookkeeping, not a safety gap.
- The Root/proxy money ratio on real traffic is unmeasured since cache-read billing: the real-model batch ran on a Root built before it.
- Extensions: after a body restart, `initctl reload` reports "not built" until the extension's source is touched (a no-op cargo build keeps the old mtime). Probes get an empty extension set, and a T2 that also changes `ext/` hands off with the old set; a kept T1 set does survive a T2 handoff.
- `install --upgrade` is not built, and the GitHub `fast-linux` workflow has not run yet.
