# ADR-012: Full access by default (the Root-protected set)

Status: accepted by the owner on 2026-10-24 (target v0.3.1) · Area: root
Depends on: ADR-007, ADR-008, ADR-009 (its behaviour becomes the opt-in `workspace` mode, unchanged), kernel/sandbox.md (§3.1.1, §3.1.2), root/root.md (§3.3, §3.4.1, §3.17), tui.md, awareness.md   Defines: charter key `sandbox.mode` (`"full"` default, `"workspace"`), the Root-protected set, the full zone tail (`zone-tail-full.sb`) and the Linux full layout, the per-session access override (`workspace.set{cwd, mode}`, `full.set` without confirmation under a full-default charter), `init-agent --workspace`, the body environment of a full grant

## Context

ADR-009 (v0.1.1) let the live body read the owner's home except a secrets list, write `INIT_HOME` plus the folder `init-agent` was started in, and reach the network only through the Root's egress to the model providers. Full mode existed, but behind a typed confirmation, and even then it kept the secrets list, the no-IP rule and the deferred-execution denies.

In daily use this was too narrow. The case that decided it: the owner asked the agent to work with files in `~/Downloads`. The body's `HOME` was `INIT_HOME`, so `~/Downloads` named a directory that does not exist; the real one was readable but not writable unless `init-agent` had been started there; and anything the task needed from the network (a `git clone`, a package download) was refused, because the zone has no IP. Each of these is a correct ADR-009 rule. Together they made the agent unable to do ordinary work the owner's own shell does in seconds.

ADR-007 already says the only security boundary is the Root against everything else. What must hold is that the owner can always **stop** the agent, **see** what it did (the signed lineage, the egress ledger) and **roll back** INIT itself (the Root's snapshots and last known-good tuple). Whether the agent may read the owner's files or open a socket is not one of those guarantees. It is a usability and privacy choice, and the owner chose the permissive one.

## Decision

From v0.3.1 the live body runs with **full access by default**. ADR-009's behaviour stays available, unchanged, as **workspace mode**.

1. **Charter.** `[sandbox] mode = "full"` (the default) or `"workspace"`. Any other value is a charter validation error. `allow_full = false` still forces workspace mode, whatever `mode` says. The key is omitted from the serialized charter when it has its default, so existing charter digests stay stable.
2. **What full access allows** (live body only: kind `body` with the live `INIT_HOME`):
   - read, write and map-executable everywhere outside `INIT_HOME`; inside `INIT_HOME` the body profile's own rules still apply;
   - IP networking in and out (TCP and UDP, local servers), plus the system services DNS, TLS and file watching need (on macOS the mach names `com.apple.SecurityServer`, `networkd`, `ocspd`, `trustd`, `trustd.agent`, `SystemConfiguration.DNSConfiguration`, `SystemConfiguration.configd`, `dnssd.service`, `mDNSResponder`, `FSEvents`);
   - `sysctl-read` and POSIX shared memory and semaphores.
3. **What it still denies: the Root-protected set.** Each entry is there because it protects stop, see or rollback:

   | Denied | Why it is kept |
   |---|---|
   | `ROOT_HOME` entirely: no read, no write, no Unix-socket connect inside it (`owner.sock`, `egress.sock`) | The keys, the charter, the signed lineage, the egress ledger and the snapshots live there. Reading it would leak the keys; writing it would forge the history (see) or the last known-good tuple (rollback); `owner.sock` would let the agent issue owner commands, and `egress.sock` would let a body talk to the Root's egress directly, past the kernel's proxy. |
   | The Root's service file and its directory (`~/Library/LaunchAgents`; on Linux `~/.config/systemd/user`), write | A changed plist or a systemd drop-in could replace the Root's program or arguments at the next login, so the next "Root" would be the agent's. That breaks all three. |
   | INIT's launchers `<bin_dir>/init-agent`, `<bin_dir>/init` and `<bin_dir>/init-tui` (`bin_dir` from `install.json`, default `~/.local/bin`; `init-agent` from v0.4.1), write. On macOS the files only: the agent can rename `bin_dir` itself and put its own folder there (see "Renaming a parent folder" below) | The owner types `init-agent` to see and stop the agent. A replaced launcher could show a fake view or swallow `/panic` (tui.md T-4). The rest of `bin_dir` is now writable. |
   | `INIT_HOME`'s Root-written parts (tail Z4: `kernel/`, `root/`, `run/mark/`, `run/zone-mark`), the marks of Z5 and the kernel lineage git | These are what the Root reads to decide what runs and what to roll back to. |
   | `~/.config/init-agent` (the directory INIT's docs name for provider key files) and every charter `sandbox.read_deny` entry, read and write. On macOS a renamed parent folder exposes them (see "Renaming a parent folder" below) | A key file there is a provider key the Root is meant to hold alone; `read_deny` is the owner's own list, which the owner expects to hold in every mode. |
   | Signals and process inspection outside the instance's own sandbox (Z7; Linux: its own pid namespace); XPC services; LaunchServices and AppleEvents (not in the mach allow list) | Signalling the Root or the kernel would defeat stop; LaunchServices would let the agent start an app outside the sandbox, which escapes everything else in this table. |

4. **What full access drops** (all still enforced in workspace mode): the ADR-009 secrets deny list (`~/.ssh`, cloud credentials, keychains, browser and mail data), so that `git push` over ssh and the owner's own CLIs work; Z3's write confinement; Z3a (shell startup files, git's user config, `~/bin`, the `~/.local/bin` directory, login items other than the Root's service directory); Z3b (the `.git` entry and git's config, hooks and command sources — `config`, `config.worktree`, `commondir`, `gitdir`, `hooks/` — outside `INIT_HOME`); Z6 (no IP).
5. **The session override.** The Root keeps a per-session `Option<bool>` over the charter default (`None` = the default). Effective full = `override.unwrap_or(mode == "full") && allow_full`.
   - `workspace.set{cwd}` (every `init-agent` launch) resets the override to `None`; `workspace.set{cwd, mode:"workspace"}` (from `init-agent --workspace`) sets it to `Some(false)`.
   - `full.set{on:false}` (`/full off`) sets `Some(false)`. `full.set{on:true}` (`/full on`, `init-agent --full`) needs no confirmation when the charter default is full, and ADR-009's typed confirmation when the charter says `"workspace"`.
   - `AccessView` gains `mode` (the charter default, `"full"` or `"workspace"`) and keeps `access`, `full`, `workspace`, `allow_full`, `read_home`, `restart_pending` and `snapshot`.
   - The TUI shows `FULL` (red, bold) or `WORKSPACE` (yellow) in the status bar; neither is ever dropped on a narrow terminal. `init-root status` prints the mode and how to change it.
6. **The body's environment for a full grant.** The Root sets `HOME` to the owner's canonical home (so `~/Downloads` is the owner's), removes `CARGO_NET_OFFLINE`, appends `<home>/.local/bin` and `<home>/.cargo/bin` to `PATH`, and sets `INIT_ACCESS=full`. The body's own git sync sets `GIT_CONFIG_GLOBAL=/dev/null`, so the owner's `~/.gitconfig` cannot change how the body's repository is synced. The boot block's access line says, in one line, what the agent may do (awareness.md).
7. **Who gets it.** Only the live body. Fleet members, probes (G4 stays hermetic), the kernel, `build`, `check` and `git-read` are unchanged.

**Where it is enforced.**
- macOS: ROOT_ALLOWS gains a group emitted only for a full grant (`Z_FULL` = `/`, excluding `Z_INIT_HOME`), and the new deny-only tail `zone-tail-full.sb` replaces `zone-tail.sb` for that grant. It keeps Z1 (file and Unix-socket denies on `ROOT_HOME`), Z2 (service file and directory, the full read-deny list), Z3a reduced to the two launchers, Z4, Z5 and Z7, and drops Z3, Z3b and Z6. The exact SBPL is in root/root.md §3.3, kept byte-identical by `tests/doc_drift.rs`.
- Linux: bubblewrap in a user namespace **without** a network namespace; the host root bound read-write; `INIT_HOME` replaced by a tmpfs holding the normal body binds; `ROOT_HOME` a read-only tmpfs; each read-deny path hidden (a tmpfs, or `/dev/null` for a file); each write-deny path bound read-only; `/run/user/<uid>/systemd` hidden and `/run/user/<uid>/bus` replaced by `/dev/null`, which blocks `systemctl --user stop`. The pid, IPC and UTS namespaces, `--disable-userns`, `--cap-drop ALL` and `--die-with-parent` stay. The seccomp program drops only its socket-domain rule; the ptrace, bpf, mount, unshare, setns, keyctl, io_uring and `TIOCSTI` blocks stay (root/root.md §3.4.1).

## How other agents do it (as of 2026-10-24)

| Agent | Default | Most permissive mode | Source |
|---|---|---|---|
| Claude Code | No sandbox: "The sandbox is off by default". Permission prompts for edits and for Bash commands that are not allow-listed in `default` mode; newer terminal versions start in a classifier-based `auto` mode. When the sandbox is turned on: Seatbelt on macOS, bubblewrap on Linux and WSL2, writes limited to the cwd and temp, network through a proxy with an allow-list. | `bypassPermissions` / `--dangerously-skip-permissions`: no prompts and no sandbox | code.claude.com/docs/en/sandboxing; code.claude.com/docs/en/permissions; code.claude.com/docs/en/permission-modes.md |
| Codex CLI | `workspace-write` sandbox with network off ("By default, the agent runs with network access turned off."). Writable roots: the cwd and `/tmp`, and `$TMPDIR` (inferred from the config keys `exclude_slash_tmp` and `exclude_tmpdir_env_var`). `.git`, `.codex` and `.agents` are protected read-only; untrusted or non-git folders may start read-only. Enforced by Seatbelt on macOS and bubblewrap plus seccomp on Linux. | `danger-full-access`, and `--dangerously-bypass-approvals-and-sandbox` (`--yolo`): no sandbox | learn.chatgpt.com/docs/agent-approvals-security (developers.openai.com/codex now redirects there); learn.chatgpt.com/docs/config-file/config-reference.md |

INIT's full mode is closest to running Claude Code without its sandbox, or Codex with `danger-full-access`, with one difference: the Root-protected set is still enforced by the OS, so the agent cannot reach the keys, forge the history, replace the Root or its launcher, or signal the Root.

## Facts measured on the owner's Mac (macOS 14, 2026-10-24)

| Check | Result |
|---|---|
| `curl https://example.com` and `git ls-remote https://…` from a deny-default Seatbelt profile with the mach names of item 2 | work (HTTP 200; refs listed) |
| `open` (LaunchServices) from that profile | fails with `kLSServerCommunicationErr`: `com.apple.coreservices.launchservicesd` and AppleEvents are not allowed, so the agent cannot launch an app outside the sandbox. This must stay so. |
| `launchctl bootout`, `stop`, `disable`, `kickstart`, `kill` of a gui job, and `launchctl submit`, from any Seatbelt sandbox (today's profile included) | refused |
| legacy `launchctl remove <label>` from any Seatbelt sandbox (tested on a test job, not on `dev.init.root`) | **succeeds**. See the residual below. |
| Seatbelt operations tried to block it: `mach-bootstrap`, `job-creation`, `lsopen`, `mach-priv-host-port` | none blocks it |

## Threat model delta

**Damage.** The agent can change or delete any file the owner can, outside the Root-protected set. Mitigations: the Root's own state and its rollback are out of reach, so INIT itself can always be restored; the workspace snapshot and `/undo` still cover the folder `init-agent` was started in; git covers repositories. **Residual: writes outside the start folder have no `/undo`.** The Root's rollback is unchanged and covers INIT (its body, kernel and tuple), not the owner's files.

**Privacy.** Any file the agent reads may be sent to the model provider, now including `~/.ssh`, cloud credentials, browser data and mail, which workspace mode denies. The money ceilings no longer bound this, because spending is observed, not capped (2026-10-23), and in full mode the agent has its own network, so a file can also leave by a path other than the provider. Mitigations: `sandbox.read_deny` still holds in full mode, except through a renamed parent folder on macOS (below); workspace mode restores the secrets list and the no-IP rule. **Residual: in full mode, treat everything the owner can read as readable by the agent and by whoever it sends data to.**

**Deferred execution.** Files the owner's own tools run later (shell startup files, every directory on `PATH` except the two launchers, editor configs, git hooks, language site-packages, tools that run configured commands) are writable, and they run with the owner's full rights, unsandboxed. ADR-009 listed these for its full mode and denied the most obvious ones in every mode; full access by default drops those denies. **Residual: full mode is trusted like the owner's own shell.** The Root's own start path (its service file, its directory, its launchers, `ROOT_HOME`) is still denied, so a planted file can act as the owner but cannot replace the Root.

**Renaming a parent folder (macOS; found by the v0.5.4 post-release review).** The full tail's denies (Z2, Z3a) cover a protected path and what lies under it, not its parent folders. So in full mode the agent can `mv ~/.local/bin ~/.local/bin.old` and put a folder of its own in its place, which replaces INIT's launchers, or `mv ~/.config ~/.cfg` and read the provider key files under `~/.config/init-agent`, or any `sandbox.read_deny` entry the same way. On Linux the full layout pins every ancestor of a deny entry below the home, so a rename there moves nothing. Workspace mode keeps the agent out unless the start folder holds such a parent. **Residual until v0.5.5, which adds denies on the parent folders on macOS.**

**`launchctl remove` (measured, pre-existing in every mode).** A process in any Seatbelt sandbox can run `launchctl remove dev.init.root`. launchd then stops the Root, which handles `SIGTERM` by stopping the zone and exiting (panic.rs signal channel), and does not restart it, because the job is gone. **This is fail-safe: the agent can end INIT, not escape it.** Every agent process stops with the Root; nothing keeps running unsandboxed, and the plist stays on disk, so the next login, or `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.init.root.plist`, starts the Root again. Seatbelt has no operation that blocks it (see the table above). We do **not** add `(deny process-exec (literal "/bin/launchctl"))`: a process can reach launchd's legacy interface (`launch_msg`) without that binary, so the rule would only look like protection.

**Linux residuals.**
- Without a network namespace, abstract-namespace Unix sockets of the host are reachable: they have no filesystem path, so a bind mount cannot hide them. A service that listens on one and trusts its peer by uid would accept the agent.
- The system D-Bus stays reachable. Through it, logind lets a user end their own sessions (`loginctl terminate-user`), which stops the Root's user service: like `launchctl remove`, a stop, not an escape. Hiding `/run/user/<uid>/bus` and `/run/user/<uid>/systemd` blocks `systemctl --user` against the Root's unit.
- Not a residual, a limit: Linux still has no home grants in workspace mode (`LINUX_HOME_GRANTS` is off), so on Linux workspace mode is the agent's own home only, as in v0.3.0, while full mode works as on macOS.

**Not changed.** The agent still cannot read or write `ROOT_HOME`, the keys or the charter; append to the signed lineage except through the Root's checks; raise a level or a quota; alter the gate or the sealed set; signal a process outside its sandbox; use XPC or LaunchServices; or change the Root's service or INIT's launchers (on macOS, short of renaming a parent folder, above). Fleet members and probes never get full access.

## Alternatives considered

| Alternative | Why not chosen |
|---|---|
| Keep ADR-009's workspace mode as the default, with full mode behind a confirmation | It is what failed in daily use (Context). Kept, unchanged, as `sandbox.mode = "workspace"` and `init-agent --workspace`. |
| Full access with a network allow-list through a Root proxy (Claude Code's sandbox model) | A useful middle ground, but it needs a CONNECT proxy, a host allow-list the owner maintains, and per-tool proxy settings, and it still breaks tools that ignore proxy variables. Possible later as a third mode. |
| Full access that keeps the secrets deny list | `git push` over ssh, cloud CLIs and package publishing would keep failing, which is most of the reason for full access. The owner can add any path to `sandbox.read_deny`, which full mode honours. |
| Deny `/bin/launchctl` by path | Bypassable through `launch_msg`, so it would be a rule that only looks like protection (see the residual). |
| Per-action approval prompts, as Claude Code does by default | ADR-007 and ADR-009 decided against prompts; protection comes from what the Root keeps out of reach, plus undo for INIT. |

## Consequences

- **A new default for every install.** A fresh v0.3.1 install starts in full access. An existing charter without `sandbox.mode` also gets full access. `install.sh` says so at the end of the install, with the three ways to confine the agent.
- **How to confine the agent:**
  - for one session: `init-agent --workspace`, or `/full off` in the chat (`/full on` lifts it again);
  - always: `sandbox.mode = "workspace"` in `~/.init-root/charter.toml`, then `init-root charter check --apply`; `/full on` then asks for the typed confirmation, as in ADR-009;
  - never full: `sandbox.allow_full = false`.
- **Undo.** `/undo` restores only the folder `init-agent` was started in. The Root's rollback of INIT is unchanged.
- **Network.** In full mode the agent's tools reach the network directly. Model requests still go through the Root's egress, which meters and logs them; other traffic is not metered or logged by INIT.
- **The awareness line** for `full` names the permissions and the denials in one line, so the agent knows that `~` is the owner's home and that only the workspace has `/undo`.
- **The TUI** gets `--workspace` (it excludes `--full`), the `FULL`/`WORKSPACE` badge and new `/full` help texts. The launcher `scripts/init-agent` passes the flag through.
- **Root size.** The new code is in `root/sandbox-profiles` (the full lists, the full group, the full tail, the Linux layout) and in `init-root`'s `access.rs`, `charter.rs` and `zone.rs`.

## Tests (all $0, on test homes)

- **Sandbox self-test rows** (kernel/sandbox.md §8, S-A10–S-A28, full mode): writes and reads outside `INIT_HOME` allowed, including the fake `~/.ssh`; TCP connect, a loopback listener and UDP to the suite's own sockets allowed; running a program copied into the fake home allowed; `ROOT_HOME` and its two sockets, the service file and directory, the launchers, `INIT_HOME`'s protected parts, `~/.config/init-agent` and a `read_deny` path denied; signals outside the sandbox denied; fleet members and probes never full.
- **Profile unit tests** (`root/sandbox-profiles`): the full tail is deny-only, its parameters match its text for both tails, and the Linux argv has no `--net`, binds `/` read-write, makes `ROOT_HOME` a read-only tmpfs, binds the service and launchers read-only, and has no final `--remount-ro /`.
- **Root** (`access.rs` unit tests and `tests/access.rs`): full by default; `--workspace` for one session; `/full off` and `/full on` with and without confirmation by charter mode; `allow_full = false`; an unknown `sandbox.mode` refused.
- **Body**: the `full` access line (awareness tests); the git sync ignores an owner `~/.gitconfig` (`owner_global_git_config_is_ignored`).
- **TUI**: `--workspace` sends `mode:"workspace"`; `/full on` under a full-default charter has no confirmation round; the status-bar goldens for `FULL` and `WORKSPACE`; the launcher passes `--workspace` through.
- **Red team**: build/status/RED-TEAM.md §6.

**Docs updated:** kernel/sandbox.md §3.1.2, root/root.md (§3.3, §3.4.1, §3.17), tui.md, awareness.md, README, the user guide, CHANGELOG (v0.3.1) and build/status/RED-TEAM.md.
