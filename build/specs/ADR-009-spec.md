# ADR-009 spec: Codex-style permissions (owner decision 2026-10-20)

The owner said the v0.1 sandbox is too strict for daily use ("các agent hiện tại đều freely traverse"). They chose the Codex-style default. This file is the binding spec for lanes DOC-009, SBX-009 and WIRE-009.

## Unchanged
The Root stays untouchable:
- ROOT_HOME (Z1) and the protected parts of INIT_HOME (Z4: kernel/, root/, run/mark/, run/zone-mark);
- keys (`~/.config/init-agent/` and the Root keystore);
- signed lineage, the charter, sealed S;
- network: provider-only egress through the Root (Z6), with money ceilings;
- no XPC and no cross-sandbox signals (Z7);
- the gate and self-evolution machinery (T0/T1/T2 worktrees under INIT_HOME/work).

## New default for the agent zone (body instances, and the kernel only where it needs it)
1. **Read: the owner's whole home directory**, plus the system paths that are readable today. The exception is a deny list that applies even under `--full`:
   - `ROOT_HOME`, `~/.config/init-agent`, `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/.kube`, `~/.docker/config.json`, `~/.netrc`, `~/.npmrc`, `~/.pypirc`;
   - `~/Library/Keychains`, `~/Library/Cookies`, `~/Library/Mail`, `~/Library/Messages`, `~/Library/Safari`;
   - `~/Library/Application Support/{Google/Chrome,Firefox,BraveSoftware,Microsoft Edge,Arc,1Password}`, `~/Library/Group Containers` (password managers and messengers).
   - The charter may add entries with `sandbox.read_deny = [...]` (≤ 32). `sandbox.read_home = false` restores the v0.1 behaviour (only INIT_HOME, plus `extra_read`).
2. **Write: INIT_HOME (as today), plus the current WORKSPACE.** The workspace is the directory the owner was in when they launched `init` (the TUI sends its cwd). The Root validates it:
   - absolute and canonical (symlinks resolved);
   - exists, is a directory, and lies inside the owner's home;
   - is not the home itself, and not inside or containing ROOT_HOME or INIT_HOME;
   - not on the deny list.
   If invalid, or `init` was launched in `~`, there is no workspace: the home is read-only, and the TUI shows a notice ("cd into a project and run `init` to let me edit it"). `/tmp/init-<uid>` (or TMPDIR under INIT_HOME, as today) stays writable.
3. **`--full` mode.** `init --full`, and `/full on|off` in the TUI, make the whole home writable except the deny list and the Root paths. The Root asks for a typed confirmation the first time per session. The charter can disable it with `sandbox.allow_full = false` (default true). The status bar shows `FULL` while it is on.
4. **Undo safety net.** Before the first agent turn that may write to a workspace in a session (or when `--full` turns on: snapshot only the workspace, or warn that full mode has no snapshot), the Root makes an APFS clone snapshot of the workspace into `ROOT_HOME/snapshots/<ts>-<name>/` using clonefile / `cp -c -R`. The zone can never read or alter it. `/undo` (an owner command over owner.sock, Root-side) restores the latest snapshot after a typed confirmation. The current state is first cloned to another snapshot, so the undo can itself be undone. Keep the last 5 per workspace. Skip with a notice if the workspace is over 2 GiB or not on APFS. Snapshots never leave the machine.
5. **Workspace change.** Launching `init` in a different directory updates the workspace. The body's profile is fixed at spawn, so the Root applies the change with a graceful body restart at the next idle boundary. The conversation continues, because the body resumes its session log. A notice says "workspace: <path>".
6. **The agent knows its permissions.** The body gets `INIT_WORKSPACE` (empty if none) and `INIT_ACCESS = read_home | workspace | full`. Tools default to cwd = the workspace when one exists. The awareness head states the permissions in one short line. This is level/status data, not a prefix-churning value: keep the prefix stable, putting it in the existing status/signals block if that's where volatile facts go (log-context §3.8 rules).

## Threat model delta (for the ADR)
- **New risk: privacy.** Files the agent reads may be sent to the model provider, the same as with Claude Code or Codex. Mitigations: the deny list for secrets, provider-only egress (no arbitrary network exfil), and the money ceilings.
- **New risk: damage.** The agent can write the owner's project without review. Mitigations: the workspace scope (not the whole home by default), APFS snapshots plus `/undo`, and git when the workspace is a repo.
- **Not changed:** the agent still cannot touch the Root, keys, lineage, ceilings or the gate, and cannot reach the network except the providers.

## Tests (all $0)
- **Sandbox selftest rows:**
  - read `~/Documents/x` (a test fixture dir the TEST creates under its own fake HOME): allowed;
  - read the fake `~/.ssh/id`: denied;
  - write the workspace: allowed;
  - write outside the workspace in home: denied;
  - write ROOT_HOME: denied;
  - `--full`: write elsewhere in home allowed, but ROOT_HOME and deny-list paths denied.
  Use a test HOME under target/test-homes. Never touch the real `~/.ssh` etc.: the profile params take HOME, so tests pass a fake HOME.
- **Root:** workspace validation table, snapshot + `/undo` round trip on a small fixture dir (APFS clone), the size skip.
- **e2e (TUI + real Root in test mode + mock):** `init` in a fixture project dir; the mock's tool call writes a file in the workspace; `/undo` restores it.
