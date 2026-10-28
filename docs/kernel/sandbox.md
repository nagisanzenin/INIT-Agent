# Sandbox: Seatbelt on macOS; bubblewrap, Landlock and seccomp on Linux

```
Status: v0.3 + build sync 2026-10-20 + ADR-009 (v0.1.1, owner decision 2026-10-20) · Area: kernel
Depends on: CONVENTIONS.md (v0.3), spec v0.2 (§2.1, §3, §7.4, §10 A6) and v0.3 (§12–§19), adr/007-root-and-autonomy-ladder.md,
            adr/009-codex-style-permissions.md,
            root/root.md (zone launch, spawn service), root/egress.md, kernel/supervise.md, kernel/initctl.md,
            kernel/gate.md, kernel/proxy.md, testing.md
Defines:    instance profiles `body` (live, safe, probe, fleet member), `build`, `git-read`, `check`: exact Seatbelt
            (SBPL) text and parameters for macOS; bwrap argv, Landlock ruleset and seccomp filter for Linux;
            the kernel's requirements of the agent zone, checked against root/root.md's `kernel` profile and
            zone tail; the parameter class rules for Root-rendered instances; the launch procedure and
            inheritance rules; the instance mark and the sweep's exclusion rule; how gate runs are sandboxed;
            the A6 sandbox test suite (S-M, S-B, S-G, S-Z, S-L); [sandbox] config keys
```

---

## 1. Purpose

Spec v0.3 §13 puts the kernel and every body inside one **agent zone**, which `init-root` confines and whose only network path is the Root's egress socket. The Root's zone tail is the security boundary (root/root.md §3.3). This doc's instance profiles remain inside it as a **stability layer**. The operating system, not a prompt, refuses a body that tries to write `kernel/`, `bin/` or `state/kernel/`, read the kernel's private `vault/` and `cache/`, or reach anything but the kernel's two sockets.

This doc fixes:
- the exact instance policies on macOS and Linux;
- what the kernel needs from the zone;
- how instances are launched. Seatbelt cannot nest sandboxes, so the Root spawns every agent-zone process on both platforms (root/root.md §3.5);
- the tests that prove it.

## 2. Responsibilities / non-goals

**Responsibilities:**
- the instance policy texts and parameters;
- how they are rendered: by the kernel on Linux, and by the Root from this doc's templates on macOS;
- inheritance;
- the sandboxes for gate work: builds, probes, git reads and acceptance checks;
- the kernel's requirements of the zone, checked against root/root.md;
- the platform prerequisites;
- the A6 test suite.

**Non-goals:**
- *When* instances are started and stopped: kernel/supervise.md.
- The zone's launch, its final policy text, and the Root's spawn service and sweep: root/root.md. This doc proposes the text and states the requirements.
- Egress classes, keys and ceilings: root/egress.md.
- Which providers the proxy routes to: kernel/proxy.md.
- `#![forbid(unsafe_code)]` enforcement: the compiler, checked by kernel/gate.md G0.
- Containers for Harbor benchmarks: Phase 2.

## 3. Design

### 3.1 Policy summary

Every instance below runs inside the agent zone (§3.11). The Root spawns it at the kernel's request, rendering this doc's text with root/root.md's prelude composition and zone tail (§3.11–§3.13). The `kernel` profile column is root/root.md §3.3.3's, summarized here.

| | `body` (live, safe, probe, fleet member) | `build` | `git-read` | `check` | `kernel` (root/root.md §3.3.3) |
|---|---|---|---|---|---|
| Runs | `init-body` and every tool child | `cargo` for candidates and stub builds | `git-upload-pack` on agent repos | gate acceptance checks (gate.md) | `init-kernel supervise` and its helpers (git, cargo for `vendor`, tar) |
| Read | system, toolchain, `body/ work/ state/ kernel/ bin/ lineage/ root/`, `INIT_HOME` listing; **live body and fleet members from v0.1.1:** the owner's home except the deny list (§3.1.1) | system, toolchain, `SRC`, `OUT`, `kernel/` | system, toolchain, `body/`, `work/` | system, toolchain, `SRC`, `OUT`, `kernel/` | system, toolchain, all of `INIT_HOME` |
| Write | `body/ work/`, `state/` except `state/kernel/`; **live body from v0.1.1:** plus the workspace, or the whole home except the deny list under `--full` (§3.1.1) | `OUT` only | nothing | `OUT` only | `INIT_HOME` except `kernel/`, `root/` and marks |
| Exec | yes | yes | yes | yes | yes |
| Network | connect to `KERNEL_SOCK` and `PROXY_SOCK`; Unix sockets under `work/` | none | none | none | connect to the Root egress socket only; serve Unix sockets under `run/` and `probe/` |
| Never | `vault/`, `cache/`, `ROOT_HOME`, the Root's sockets, the deny list (secrets, in every mode), other processes; v0.1.0 and probes: the owner's home | `vault/`, live `body/`/`state/`, `ROOT_HOME` | writes, `vault/`, `cache/`, `ROOT_HOME` | `vault/`, live state, `ROOT_HOME` | `ROOT_HOME`, IP sockets, the owner's home, other sandboxes' processes |

Probes and fleet members (L2) use the `body` profile with parameters that point at their own root (§3.3.3, §3.3.5). `check` is the `build` profile with different parameters.

#### 3.1.1 Access modes (v0.1.1, ADR-009)

The owner chose Codex-style permissions on 2026-10-20 (ADR-009, CONVENTIONS §6 OW-PERM). The Root decides the access of every instance from its kind, its lane and the Root's access state (root/root.md §3.17); the kernel never supplies it. A live body's mode, as the Root computes it and puts in `INIT_ACCESS`:

| `INIT_ACCESS` | When | Read (beyond §3.1) | Write (beyond §3.1) |
|---|---|---|---|
| `full` | v0.3.1 (ADR-012, §3.1.2): the default (`sandbox.mode = "full"`); with `sandbox.mode = "workspace"`, `/full on` after the Root's typed confirmation; never while the charter says `sandbox.allow_full = false` | everything outside `INIT_HOME` except the Root-protected set (§3.1.2) | everything outside `INIT_HOME` except the Root-protected set, plus the IP network |
| `workspace` | a workspace is set: `init-agent` ran in a directory that passed the Root's validation (root/root.md §3.17.1) | as `read_home` when `sandbox.read_home` is true | the workspace subtree |
| `read_home` | no workspace (`init-agent` ran in `~`, or the cwd was refused), and `sandbox.read_home = true` (the default) | the owner's home except the deny list | nothing: `INIT_HOME` only, as in v0.1.0 |
| `none` | no workspace and `sandbox.read_home = false` | nothing: the v0.1.0 behaviour | nothing |

Other instances:

| Instance | Read (beyond §3.1) | Write (beyond §3.1) | `INIT_ACCESS` · `INIT_WORKSPACE` |
|---|---|---|---|
| live body (lane live, safe mode included) | as the table above | as the table above | the mode above · the workspace, or `""` |
| fleet member (`fleet/<bid>`, L2) | the owner's home except the deny list, when `sandbox.read_home` is true | nothing | `read_home` or `none` · `""` |
| probe (G4 task runs, the probe round trip) | nothing: hermetic, as in v0.1 | nothing | `none` · `""` |
| `kernel`, `build`, `check`, `git-read` | unchanged; tail Z2's deny list now applies to them too | unchanged | — |

- **The deny list** holds in every mode, `--full` included: `ROOT_HOME`, `~/.config/init-agent`, `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/.kube`, `~/.docker/config.json`, `~/.netrc`, `~/.npmrc`, `~/.pypirc`, `~/Library/{Keychains,Cookies,Mail,Messages,Safari}`, `~/Library/Application Support/{Google/Chrome,Firefox,BraveSoftware,Microsoft Edge,Arc,1Password}` and `~/Library/Group Containers`, plus up to 32 charter `sandbox.read_deny` entries. It denies reads and writes. Tail Z2 also denies writes to `~/Library/LaunchAgents`, so no mode can install a login job.
- **Nothing the owner runs unsandboxed** (tails Z3a and Z3b, every mode, every instance): writes are denied to the shell startup files, git's user config (`~/.gitconfig`, `~/.config/git`), `~/.local/bin` and `~/bin` (so `init-agent` and `init-tui`, tui.md T-4), `install.json`'s `bin_dir`, and login items (`~/Library/LaunchAgents`, `~/.config/autostart`, `~/.config/systemd/user`). In any repository outside `INIT_HOME`, `.git/hooks`, `.git/config` and the same files under `.git/modules/` are denied too; the rest of `.git` stays writable, so the agent can commit (root/root.md §3.3.2).
- **Where it is enforced.** The Root generates two grant lines in ROOT_ALLOWS for the instance: a read of the home (`Z_HOME`) and a write of its scope (`Z_WRITE`: the workspace, or the home in full mode). Both exclude `INIT_HOME`, so inside `INIT_HOME` this doc's SECTION still rules: even in full mode a body cannot write `bin/`, `lineage/` or `state/kernel/`. Tail Z2 then denies each deny-list entry (`Z_DENY_<i>`), and tail Z3 confines writes to `INIT_HOME`, `Z_WRITE` and the devices (root/root.md §3.3.1–§3.3.2). Every `Z_*` value is the Root's (V1), so no kernel parameter can widen them. The body SECTION of §3.3.1 is unchanged.
- **The environment matches the sandbox.** For every `body`-profile spawn (live body, member or probe) the Root sets `INIT_ACCESS` and `INIT_WORKSPACE` in the child's environment, overwriting any value the kernel sent. The spawn's `cwd` stays under `INIT_HOME` (V4), and the body's tools `chdir` to `$INIT_WORKSPACE` when it is not empty (body/boot.md §3.2).
- **Gate measurements stay hermetic.** A probe never reads the owner's home and no gate instance writes a workspace, so a G4 result cannot depend on the owner's files, and a gate run cannot change the owner's project.
- **A profile is fixed at spawn.** A new workspace, `/full on|off` or a charter change reaches the live body through a graceful restart at its next idle boundary: the Root sends the kernel an `access` event with `restart:"idle"` (root/root.md §3.17.3). The conversation continues, because the body resumes its session log.

#### 3.1.2 Full access by default (v0.3.1, ADR-012)

The owner chose full access as the default on 2026-10-24 (ADR-012). With the charter's `sandbox.mode = "full"` (the default; `"workspace"` restores §3.1.1's ADR-009 behaviour unchanged), the live body's processes read, write and run anything the owner can and use the IP network, except the **Root-protected set**: `ROOT_HOME` (no file access, no Unix socket inside it, so neither `owner.sock` nor `egress.sock`), the Root's service file and its directory (`~/Library/LaunchAgents`; on Linux `~/.config/systemd/user`, plus the other user unit directories systemd reads drop-ins from, `~/.config/systemd/user.control` and `~/.local/share/systemd/user`), INIT's launchers (`<bin_dir>/init-agent`, `<bin_dir>/init` and `<bin_dir>/init-tui`; the rest of `bin_dir` is writable), `INIT_HOME`'s Root-written parts (Z4) and other marks (Z5), `~/.config/init-agent` and the charter's `sandbox.read_deny`, and no signals, process inspection, XPC, LaunchServices or AppleEvents outside the instance's own sandbox (Z7). Inside `INIT_HOME` this doc's `body` SECTION still rules.

- **Only the live body.** Fleet members and probes are never full (S-A18c–e, S-A19c–d); the kernel and the gate instances are unchanged.
- **The owner's session choice.** `init-agent --workspace` or `/full off` confine the session to §3.1.1's workspace mode; `/full on` returns to full (with the typed confirmation only when the charter says `"workspace"`). root/root.md §3.17.2.
- **Dropped in full mode** (still enforced in workspace mode): the ADR-009 secrets deny list (so `git push` over ssh works), Z3's write confinement, Z3a's shell startup files, git config, `~/bin` and `~/.local/bin` directory, Z3b's git hooks, and Z6's no-IP rule.
- **Enforcement.** macOS: ROOT_ALLOWS' full group (`Z_FULL` = `/`) and the full tail (root/root.md §3.3.1, §3.3.2a). Linux: the host root bound read-write with overlays for the protected set, the host network, and the seccomp program without the socket-domain rule (root/root.md §3.4.1).
- **The environment.** For a full grant the Root also sets `HOME` to the owner's home (so `~/Downloads` is the owner's), removes `CARGO_NET_OFFLINE`, and appends `<home>/.local/bin` and `<home>/.cargo/bin` to `PATH`.
- **Residuals.** Files the owner's own tools run later (shell startup files, PATH directories, editor configs, git hooks) run unsandboxed: full mode is trusted like the owner's own shell. Writes outside the workspace have no `/undo`. Any file the agent reads may reach the model provider.

### 3.2 How Codex CLI does it (the published design the spec follows)

All from `openai/codex` at commit `bb8f5304fcd9f901d8e0747fccb8c8904cbc504e` (main, 2026-10-19). The Seatbelt code moved from `codex-rs/core` into a `codex-rs/sandboxing` crate in March 2026 (#15593).

**macOS** (`codex-rs/sandboxing/src/seatbelt*.{rs,sbpl}`):

- **Base policy** (`seatbelt_base_policy.sbpl`), credited to Chrome's `common.sb`/`renderer.sb`:
  - `(version 1)`, `(deny default)`;
  - `(allow process-exec)`, `(allow process-fork)`, `(allow signal (target same-sandbox))`, `(allow process-info* (target same-sandbox))`, with the comment "child processes inherit the policy of their parent";
  - an explicit `sysctl-read` allowlist (about 50 names);
  - two mach services (`com.apple.system.opendirectoryd.libinfo`, `com.apple.PowerManagement.control`);
  - `ipc-posix-sem`;
  - PTYs: `/dev/ptmx`, plus `/dev/ttys*` only with `(extension "com.apple.sandbox.pty")`, so only PTYs opened inside the sandbox are usable.
- **Writable roots** (`build_seatbelt_access_policy`): each root is a parameter `WRITABLE_ROOT_<i>`. Read-only carve-outs become `(require-all (subpath (param "WRITABLE_ROOT_0")) (require-not (literal (param "WRITABLE_ROOT_0_EXCLUDED_0"))) (require-not (subpath (param "WRITABLE_ROOT_0_EXCLUDED_0"))))`. The `literal` clause closes "a gap for first-time creation of the protected directory itself". Protected names are `.git`, `.agents`, `.codex`, `.aws`. Each root gets an anchor `(deny file-write-unlink (require-all (literal (param "WRITABLE_ROOT_0")) (vnode-type DIRECTORY)))`, so it cannot be replaced.
- **Reads:** full read by default. With the `:minimal` read policy, `seatbelt_read_only_platform_defaults.sbpl` grants system paths (`/usr/lib`, `/usr/share`, `/System/…`, `/bin`, `/usr/bin`, `/private/etc`, `/dev/null`, `/dev/urandom`, …) and a longer list of mach services.
- **Network:** nothing by default. With a proxy: `(allow network-outbound (remote ip "localhost:<port>"))` per loopback proxy port. Allowlisted Unix sockets: `(allow system-socket (socket-domain AF_UNIX))`, `(allow network-bind (local unix-socket (subpath (param "UNIX_SOCKET_PATH_0"))))`, `(allow network-outbound (remote unix-socket (subpath (param "UNIX_SOCKET_PATH_0"))))`.
- **Hard denies emitted after every grant:** the daemon-socket directory (`(deny file-read* file-write* …)`, `(deny network-outbound (remote unix-socket (subpath …)))`) and `(deny mach-lookup (xpc-service-name-prefix ""))`.
- **Invocation:** `MACOS_PATH_TO_SEATBELT_EXECUTABLE = "/usr/bin/sandbox-exec"` (absolute, "against an attacker trying to inject a malicious version on the PATH"); argv `["-p", policy, "-DKEY=VALUE"…, "--", cmd…]`; the environment is cleared and rebuilt, with `CODEX_SANDBOX=seatbelt` set.
- Paths are canonicalized (`/tmp` → `/private/tmp`, `/var` → `/private/var`). Symlinked writable roots are rejected.

**Linux** (`codex-rs/linux-sandbox`):

- **bubblewrap is the filesystem sandbox** (system `bwrap` or a bundled one):
  - flags `--new-session --die-with-parent`, then the filesystem (`--ro-bind / /`, or for restricted reads `--tmpfs /` plus `--ro-bind` per readable root);
  - `--bind` for writable roots, `--ro-bind` for protected subpaths;
  - `--unshare-user --unshare-pid --unshare-ipc --unshare-net`, `--proc /proc`, `--cap-drop ALL`.
- **seccomp** (`seccompiler = 0.5.0`): default allow, matches return `EPERM`.
  - `io_uring_*` denied always ("io_uring can create AF_VSOCK sockets without a socket() syscall").
  - `ptrace`, `process_vm_readv/writev` denied.
  - In restricted network mode, `connect accept accept4 bind listen …` are denied and `socket`/`socketpair` are allowed only for `AF_UNIX`.
- **Landlock** remains only as a legacy path. The README says: "The legacy Landlock option is rejected for these policies because it cannot isolate app-server Unix sockets."
- **Proxy:** proxy traffic crosses the network namespace by fd passing: a host-side bridge, and a loopback listener inside the namespace.
- **NO_NEW_PRIVS and seccomp** are applied by an inner stage re-executed inside bwrap, after the mounts.

**Where INIT differs, and why:**

| Codex | INIT | Reason |
|---|---|---|
| `.git` re-protected read-only | `body/.git` writable | the agent must commit; authority is `lineage/body.git` (supervise.md Decision 2) |
| full read by default | v0.1.0: an explicit read allowlist. v0.1.1 (ADR-009): the live body reads the owner's home too, minus a deny list of secrets that holds in every mode (§3.1.1) | v0.1.0 kept the agent out of the owner's home; the owner then chose Codex's default (OW-PERM). Secrets stay out of model prompts through the deny list |
| proxy over loopback TCP | proxy over a Unix socket | no IP stack needed at all; one literal path to allow |
| Unix socket allowlist by `subpath` | the two kernel sockets by `path-literal`; `subpath` only for `work/` other lanes' sockets live under `probe/` and `fleet/`, so a subtree rule could reach a lane that is not the body's own |
| Linux: Landlock unused for fs | bwrap primary, Landlock as a second layer, seccomp | defense in depth; Landlock ABI 9 (Linux 7.1) can now restrict pathname-socket `connect` |
| Linux proxy via fd-passing bridge | bind-mount the proxy socket file | the proxy is already a Unix socket |
| kills the process group | kills the sandbox instance (mark / pid namespace) | processes can leave a process group (supervise.md §3.8.2) |

### 3.3 macOS: the `body` profile

#### 3.3.1 Exact text

On macOS this text is compiled into `init-root`. It is composed as PRELUDE (sections 1–4) + SECTION (sections 5–8) + the Root's mark allows + the zone tail, and rendered with `-p` when the kernel asks for a `body` instance (root/root.md §3.3.1; §3.11 here). On Linux the equivalent policy is the bwrap argv of §3.6.1.

v0.3 made two changes:
- section 5 gained `(allow file-read* (subpath (param "PUB")))`, so the agent can read the Root's published view `INIT_HOME/root/` (lineage, ledger, ceilings, charter, status; root/root.md §9.1 X2);
- section 8 lost the `OWNER_SOCK` deny, because the owner socket moved to `ROOT_HOME/run/`, which tail rules Z1 and Z6 cover.

This exact text was extracted from this document, composed with the tail and allows extracted from root/root.md, and run on macOS 14.6 (M2) on 2026-10-20 against the full suite of §8. **123/123 pass:**
- live 63/63;
- probe 9/9;
- fleet member 6/6;
- all four composed profiles parse;
- `build` 7/7;
- `git-read` 4/4;
- `kernel` profile 30/30. The text includes the `LINK_*` rule from reconciliation round 1, which lets a probe traverse its `kernel`/`lineage`/`bin` links.

ADR-009 (v0.1.1) does not change this text: the home read and the workspace write are Root-generated ROOT_ALLOWS lines (§3.1.1, root/root.md §3.3.1; SBX-009, 8570638). The block is `root/sandbox-profiles/profiles/{prelude,body}.sb` verbatim, and `tests/doc_drift.rs` compares it byte for byte, so change the files and this block together.

```scheme
(version 1)
; INIT body sandbox, macOS Seatbelt. Rendered by init-kernel; parameters via -D.
(deny default)

; ---- 1. processes: children inherit this policy; signals and inspection stay inside it ----
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))

; ---- 2. sysctl, mach, ipc: Codex base policy list plus the extras INIT measured ----
(allow sysctl-read
  (sysctl-name "hw.activecpu") (sysctl-name "hw.busfrequency_compat") (sysctl-name "hw.byteorder")
  (sysctl-name "hw.cacheconfig") (sysctl-name "hw.cachelinesize") (sysctl-name "hw.cachelinesize_compat")
  (sysctl-name "hw.cpufamily") (sysctl-name "hw.cpufrequency") (sysctl-name "hw.cpufrequency_compat")
  (sysctl-name "hw.cputype") (sysctl-name "hw.l1dcachesize_compat") (sysctl-name "hw.l1icachesize_compat")
  (sysctl-name "hw.l2cachesize_compat") (sysctl-name "hw.l3cachesize_compat") (sysctl-name "hw.logicalcpu")
  (sysctl-name "hw.logicalcpu_max") (sysctl-name "hw.machine") (sysctl-name "hw.memsize")
  (sysctl-name "hw.model") (sysctl-name "hw.ncpu") (sysctl-name "hw.nperflevels")
  (sysctl-name "hw.packages") (sysctl-name "hw.pagesize") (sysctl-name "hw.pagesize_compat")
  (sysctl-name "hw.physicalcpu") (sysctl-name "hw.physicalcpu_max") (sysctl-name "hw.tbfrequency_compat")
  (sysctl-name "hw.vectorunit") (sysctl-name-prefix "hw.optional.") (sysctl-name-prefix "hw.perflevel")
  (sysctl-name "machdep.cpu.brand_string") (sysctl-name "machdep.ptrauth_enabled")
  (sysctl-name "kern.argmax") (sysctl-name "kern.bootargs") (sysctl-name "kern.boottime")
  (sysctl-name "kern.clockrate") (sysctl-name "kern.hostname") (sysctl-name "kern.maxfilesperproc")
  (sysctl-name "kern.maxproc") (sysctl-name "kern.ngroups") (sysctl-name "kern.osproductversion")
  (sysctl-name "kern.osrelease") (sysctl-name "kern.ostype") (sysctl-name "kern.osvariant_status")
  (sysctl-name "kern.osversion") (sysctl-name "kern.secure_kernel") (sysctl-name "kern.sysv.semmns")
  (sysctl-name "kern.usrstack64") (sysctl-name "kern.version") (sysctl-name "sysctl.proc_cputype")
  (sysctl-name "vm.loadavg") (sysctl-name-prefix "kern.proc.pgrp.") (sysctl-name-prefix "kern.proc.pid."))
(allow sysctl-write (sysctl-name "kern.grade_cputype"))
(allow mach-lookup
  (global-name "com.apple.system.opendirectoryd.libinfo")
  (global-name "com.apple.system.opendirectoryd.membership")
  (global-name "com.apple.system.DirectoryService.libinfo_v1")
  (global-name "com.apple.system.notification_center")
  (global-name "com.apple.system.logger")
  (global-name "com.apple.bsd.dirhelper")
  (global-name "com.apple.logd")
  (global-name "com.apple.diagnosticd")
  (global-name "com.apple.PowerManagement.control"))
(allow ipc-posix-sem)
(allow ipc-posix-shm-read* (ipc-posix-name "apple.shm.notification_center"))
(allow iokit-open (iokit-registry-entry-class "RootDomainUserClient"))

; ---- 3. devices and PTYs (only PTYs created inside the sandbox are usable) ----
(allow file-read* file-write-data (literal "/dev/null") (literal "/dev/zero"))
(allow file-read* (literal "/dev/random") (literal "/dev/urandom") (literal "/dev/autofs_nowait"))
(allow file-read* file-write-data file-ioctl (literal "/dev/dtracehelper"))
(allow file-read-data file-write-data (subpath "/dev/fd"))
(allow file-read-metadata (literal "/dev"))
(allow pseudo-tty)
(allow file-read* file-write* file-ioctl (literal "/dev/ptmx"))
(allow file-read* file-write* (require-all (regex #"^/dev/ttys[0-9]+") (extension "com.apple.sandbox.pty")))

; ---- 4. system and toolchain reads (read-only) ----
(allow file-read* (literal "/"))
(allow file-read-metadata (literal "/etc") (literal "/tmp") (literal "/var") (literal "/private")
  (literal "/private/var") (literal "/private/tmp") (literal "/opt") (literal "/Library")
  (literal "/Library/Developer") (literal "/Users") (literal (param "HOME_REAL")))
(allow file-read* (subpath "/usr") (subpath "/bin") (subpath "/sbin") (subpath "/System")
  (subpath "/Library/Apple") (subpath "/Library/Preferences") (subpath "/private/etc")
  (subpath "/private/var/db/timezone") (subpath "/private/var/db/dyld") (subpath "/private/var/select")
  (subpath "/opt/homebrew") (subpath (param "DEVDIR")) (subpath (param "TOOLCHAIN")))
(allow file-read-metadata (path-ancestors (param "TOOLCHAIN")) (path-ancestors (param "DEVDIR")))
(allow file-map-executable (subpath "/usr/lib") (subpath "/System") (subpath "/Library/Apple")
  (subpath "/opt/homebrew") (subpath (param "DEVDIR")) (subpath (param "TOOLCHAIN")))

; ---- 5. INIT_HOME: read body/ work/ state/ kernel/ bin/ lineage/; write body/ work/ state/ except state/kernel/ ----
(allow file-read-metadata (path-ancestors (param "INIT_HOME")))
(allow file-read* (literal (param "INIT_HOME")))
(allow file-read* (subpath (param "BODY")) (subpath (param "WORK")) (subpath (param "STATE"))
  (subpath (param "KERNEL")) (subpath (param "BIN")) (subpath (param "LINEAGE")))
(allow file-read* (literal (param "LINK_KERNEL")) (literal (param "LINK_LINEAGE")) (subpath (param "LINK_BIN")))
(allow file-read* (subpath (param "PUB")))
(allow file-map-executable (subpath (param "BODY")) (subpath (param "WORK")) (subpath (param "BIN"))
  (subpath (param "KERNEL")))
(allow file-read-metadata (literal (param "RUN")))
(allow file-write* (subpath (param "BODY")) (subpath (param "WORK"))
  (require-all (subpath (param "STATE"))
    (require-not (literal (param "STATE_KERNEL")))
    (require-not (subpath (param "STATE_KERNEL")))))
(deny file-write-unlink (literal (param "BODY")) (literal (param "WORK")) (literal (param "STATE")))

; ---- 6. network: the two kernel sockets, plus agent-local Unix sockets under work/ ----
(allow system-socket (socket-domain AF_UNIX))
(allow network-outbound (remote unix-socket (path-literal (param "KERNEL_SOCK"))))
(allow network-outbound (remote unix-socket (path-literal (param "PROXY_SOCK"))))
(allow network-bind (local unix-socket (subpath (param "WORK"))))
(allow network-outbound (remote unix-socket (subpath (param "WORK"))))

; ---- 7. instance mark (lets the kernel find every process of this sandbox instance) ----
(allow file-read-data (literal (param "MARK")))

; ---- 8. hard denies, emitted last so no earlier allow can widen them ----
(deny file-read* file-write* (literal (param "VAULT")) (subpath (param "VAULT")))
(deny file-read* file-write* (literal (param "CACHE")) (subpath (param "CACHE")))
(deny file-read* file-write* (subpath "/opt/homebrew/var") (subpath "/opt/homebrew/etc"))
(deny mach-lookup (xpc-service-name-prefix ""))
```

#### 3.3.2 Parameters (live body and safe mode)

Every value is passed as `-D<NAME>=<value>` and must be an absolute, canonical path (`std::fs::canonicalize`). Seatbelt matches resolved paths, so `/tmp/x` must be given as `/private/tmp/x`.

| Param | Value |
|---|---|
| `INIT_HOME` | `$INIT_HOME` |
| `BODY` · `WORK` · `STATE` | `$INIT_HOME/body` · `/work` · `/state` |
| `STATE_KERNEL` | `$INIT_HOME/state/kernel` |
| `KERNEL` · `BIN` · `LINEAGE` | `$INIT_HOME/kernel` · `/bin` · `/lineage` |
| `RUN` | `$INIT_HOME/run` |
| `KERNEL_SOCK` · `PROXY_SOCK` | `$INIT_HOME/run/kernel.sock` · `/run/proxy.sock` |
| `VAULT` · `CACHE` | `$INIT_HOME/vault` · `$INIT_HOME/cache` |
| `PUB` | `$INIT_HOME/root` (v0.3: the Root-written published view: lineage, ledger, ceilings, charter, status; read-only; root/root.md §3.10.6, X2) |
| `MARK` | set by the Root, never by the kernel (root/root.md V1): `$INIT_HOME/run/mark/<instance id>`, created empty with mode `0400` before launch. The Root also appends its own allow for the mark and the zone mark (ROOT_ALLOWS). Linux: unused |
| `LINK_KERNEL` · `LINK_LINEAGE` · `LINK_BIN` | `$INIT_HOME/kernel` · `/lineage` · `/bin` (the real directories, so the rule adds nothing for the live body) |
| `TOOLCHAIN` | `[sandbox] toolchain` (e.g. `/Users/<owner>/.rustup/toolchains/1.97.1-aarch64-apple-darwin`) |
| `DEVDIR` | `[sandbox] developer_dir` (default `/Library/Developer/CommandLineTools`) |
| `HOME_REAL` | the owner's home from `getpwuid` (metadata only: lets tools `stat` it). From v0.1.1 the live body's home read, the workspace and the deny list are Root-set parameters, never the kernel's (§3.1.1; names in root/root.md §3.3.2) |

#### 3.3.3 Parameters for a probe

Same text, different values. The probe cannot reach the live body's files, because none of its allow rules name them.

| Param | Probe value |
|---|---|
| `INIT_HOME` | `$INIT_HOME/probe/<id>` |
| `BODY` · `WORK` · `STATE` · `STATE_KERNEL` · `RUN` | under `$INIT_HOME/probe/<id>/` |
| `KERNEL_SOCK` · `PROXY_SOCK` | `$INIT_HOME/probe/<id>/run/kernel.sock` · `…/proxy.sock` |
| `KERNEL` · `BIN` | `$INIT_HOME/probe/<id>/kernel` · `…/bin`: real directories in the probe root. On macOS they hold copy-on-write clones (`clonefile`) of `$INIT_HOME/kernel` and of the probe's binary plus its kept-component set (`bin/init-body`, `bin/ext/`); on Linux they are read-only bind mounts (supervise.md §3.6.3, D8) |
| `LINEAGE` · `VAULT` · `CACHE` | the real `$INIT_HOME/…` paths (`LINEAGE`: the probe's `body/` clone uses the real `lineage/body.git` as `origin` and alternates) |
| `PUB` | as for the live body |
| `MARK` | the probe's own mark |
| `LINK_KERNEL` · `LINK_LINEAGE` · `LINK_BIN` | equal to `KERNEL`, `LINEAGE`, `BIN` above: real, canonical directories, so the `LINK_*` rule adds nothing (D8) |

**v0.3 (D8): no symlinks in probe or member roots.** v0.2 put absolute symlinks to the real `kernel/`, `lineage/` and `bin/ext.d/<set>` in the probe root. Traversing a symlink needs read access to the link's own path, not only to its target (tested 2026-10-19), hence the `LINK_*` literal rule. The Root's V2 check, however, requires every parameter to be canonical, and a symlink's path is not. Supervise therefore fills the root with real directories instead:
- macOS: `clonefile(2)` copies, copy-on-write, with no data copied on APFS;
- Linux: bind mounts.

Every parameter the kernel sends is therefore canonical. The Root's V2 still accepts both `LINK_*` forms (D8): a real canonical directory, or a symlink whose parent is canonical. A symlink is accepted only inside a probe or member root; the live root's `LINK_*` must equal its target. The `LINK_*` rule stays in the text (changing SBPL is a Root release) and, with `LINK_*` equal to the real directories, adds nothing. The read rules used are the same `subpath` rules S-M18–S-M21 exercise.

#### 3.3.4 Why each section is there

- **§1** is Codex's process block. `same-sandbox` targets mean the body can signal and inspect only its own sandbox, never the kernel. This was tested: `kill -0` on the kernel and on launchd both fail. Nested `sandbox-exec` fails with `sandbox_apply: Operation not permitted`, which is fine: the policy cannot be widened from inside.
- **§2:** trimming Codex's sysctl list broke Rust std itself. With `hw.pagesize_compat` denied, every Rust binary aborted at start (`failed to allocate a guard page: Invalid argument … fatal runtime error`), and perl failed `sysconf`. The list is therefore Codex's list in full, plus `kern.bootargs`, `kern.boottime`, `kern.clockrate`, `kern.ngroups`, `hw.cachelinesize` and `machdep.ptrauth_enabled`, which `log show` reported as denied during `cargo build`, `git` and `perl` runs. The mach services are the minimum for user and group lookup and logging. Keychain (`com.apple.SecurityServer`), pasteboard and network configuration are not reachable.
- **§3:** `/dev/tty` is deliberately absent. The body has no controlling terminal, and tool children cannot read the owner's keystrokes. PTYs follow Codex: only those created inside the sandbox are usable.
- **§4:** an explicit read allowlist. From v0.1.1 the live body and fleet members also read the owner's home minus the deny list, through Root-generated ROOT_ALLOWS lines (§3.1.1, ADR-009); probes keep this list only. `/opt/homebrew` is readable, which gives the agent the owner's CLI tools; this was settled by the owner on 2026-10-20 as an L0 right (CONVENTIONS §6 ESC-2, spec v0.3 §15 and §19). `var/` and `etc/` (databases, service configs) are denied in §8, and again by tail Z2. `DEVDIR` and `TOOLCHAIN` let `cargo`, `rustc`, `cc`, `ld` and `git` run. With `SDKROOT` and `DEVELOPER_DIR` in the environment (supervise.md §4.1), `xcrun` never needs its cache under `/var/folders` (observed failing without them).
- **§5:** the `require-not` pair on `STATE_KERNEL` is Codex's carve-out pattern. It blocks writes, renames, unlinks and *first-time creation* of `state/kernel`. The anchors stop the agent from renaming or removing `body/`, `work/` or `state/` themselves. Read access to `bin/` is required to exec the body.
- **§6:** `path-literal` allows exactly two kernel socket paths, never a subtree that could contain kernel sockets (kernel/proxy.md Q-S1). The `subpath WORK` rules are for sockets the agent's own processes create; no kernel socket ever lives under `work/`. A symlink in `work/` pointing at the Root's socket was tested and is denied (S-M32b), because the check uses the resolved path. All IP sockets are refused, including loopback: on macOS a loopback connection would reach the owner's other local services. The body has `file-read-metadata` on `RUN` itself only, so it cannot `stat` `$INIT_KERNEL_SOCK` (`[ -S … ]` is always false, with `EPERM`), but connecting works. Probe the socket by connecting, not by `stat` (FIX1).
- **§7:** the mark (set by the Root). The process sweep identifies members of *this instance* by sandbox policy (supervise.md §3.8.2; the Root runs it, root/root.md §3.5.4). A pid is a member only if **all three** hold:
  - it is sandboxed;
  - `sandbox_check(pid, "file-read-data", SANDBOX_FILTER_PATH | SANDBOX_CHECK_NO_REPORT, MARK) == 0` (allowed);
  - the same check on an **exclusion path** returns non-zero (denied).

  The exclusion path is one that every instance denies but a permissive foreign sandbox can read. The recommended one is the Root's existing canary `ROOT_HOME/run/canary` (denied to every instance by tail Z1; root/root.md already uses it for its generic sweep). A sibling decoy `<MARK>.decoy` works too. Both were tested.

  The exclusion rule is new in v0.3. It fixes a real hazard found on 2026-10-20: many Apple-sandboxed per-user agents, and the Virtualization.framework VM process of a third-party app, may read whole directories. With the test mark under `/private/tmp`, the mark-only rule matched 31 foreign processes, and the test killed them (§8). With either exclusion path, membership was exact: 1 member, 31 excluded (S-M47). Both files must exist, because for a missing path `sandbox_check` reports "denied" even for unsandboxed processes (tested).
- **§8:** denies are emitted last within the SECTION, as Codex does for its daemon-socket directory, so that no earlier allow (for example `(literal INIT_HOME)` or a future broader read rule) can expose `vault/` or `cache/`. The Root's own paths need no rule here, because the zone tail follows and denies them (Z1, Z6). `xpc-service-name-prefix ""` blocks XPC services wholesale (Codex; tail Z7 repeats it).

#### 3.3.5 Parameters for a fleet member (L2)

A fleet member (supervise.md §3.14) is a body with its own root `$INIT_HOME/fleet/<bid>/`, kept while it runs and 7 days after it stops. It is laid out like a probe root, with the same clones and no symlinks (D8). Its parameters are those of §3.3.3, with `<probe root>` replaced by `$INIT_HOME/fleet/<bid>`, with one exception. Its lane sockets are `$INIT_HOME/run/fleet/<bid>/{kernel,proxy}.sock`, and `RUN` is that directory. The reason: root/root.md §3.3.3's `kernel` profile lets the kernel bind sockets only under `run/` and `probe/`. The sockets are `path-literal` in the body SECTION, so their location changes nothing for the member. A member therefore cannot read or write the primary's `body/`, `work/` or `state/`, another member's root, or `probe/` (tested, S-M55 and S-M57). The primary cannot read members' roots either. Members exchange results only through the kernel (promotions, qualified SHAs and the archive, evolution.md §3.15.5), not through each other's files.

### 3.4 macOS: the `build` and `check` profiles

Sections 1–4 are **byte-identical** to the `body` profile (only the comment on line 2 says `build`). Sections 5–8 are replaced by:

```scheme
; ---- 5. candidate source read-only; output dir read-write; kernel/ read-only (WIT) ----
(allow file-read-metadata (path-ancestors (param "INIT_HOME")) (path-ancestors (param "SRC")) (path-ancestors (param "OUT")))
(allow file-read* (subpath (param "SRC")) (subpath (param "OUT")) (subpath (param "KERNEL")))
(allow file-map-executable (subpath (param "OUT")))
(allow file-write* (subpath (param "OUT")))
(deny file-write-unlink (literal (param "OUT")))
; ---- 6. network: Unix sockets under OUT only (bind, accept, connect: the tests' own servers); no IP ----
(allow system-socket (socket-domain AF_UNIX))
(allow network-bind (local unix-socket (subpath (param "OUT"))))
(allow network-outbound (remote unix-socket (subpath (param "OUT"))))
; ---- 7. instance mark ----
(allow file-read-data (literal (param "MARK")))
; ---- 8. hard denies ----
(deny file-read* file-write* (literal (param "VAULT")) (subpath (param "VAULT")))
(deny mach-lookup (xpc-service-name-prefix ""))
```

| Use | `SRC` | `OUT` |
|---|---|---|
| candidate build / G0 (supervise.md §3.6.2) | `cache/build/slot/src` | `cache/build/slot/out` (holds `target/ tmp/ home/ cargo-home/`) |
| stub deps build (supervise.md §3.6.2) | `cache/build/slot/src` (holds the stub; the export waits in `slot/cand`) | `cache/build/slot/out`. The kernel clones `out/target` into `cache/deps/<key>.tmp/` and later clones a hit back into `out/target`; no build instance reads or writes `cache/deps/` |
| acceptance check (`check`, gate.md) | the check's inputs (task workspace after the run, check script) | a scratch dir under `cache/` |
| G1 selfmap (`--manifest`, `describe()`) | the export (`cache/build/slot/src`) | `cache/gate/<job>/` (holds the candidate binary and components; G3's inner gate: `cache/gate/<job>/g3/`); the child's `HOME`/`TMPDIR` are in `OUT/g1/` (as built, K-GATE) |

Tested 2026-10-19 and again on 2026-10-20 with the v0.3 text: a `cargo build --frozen` of a crate in `SRC` succeeded with output in `OUT`. Writing into `SRC`, listing the live `body/` and reading `ROOT_HOME` were all denied. No IP network, so `build.rs` and proc macros cannot exfiltrate anything, and they cannot write outside `OUT`.

**Unix sockets under `OUT` (provisional owner decision, 2026-10-20).** G0 runs the candidate's `cargo test` in this profile, and the seed's tests serve and connect Unix sockets: the `init-mockllm` provider tests (`INIT_MOCKLLM_BIN`, gate.md §3.3, testing.md Q10) and in-test stub servers. Section 6 therefore allows `bind` (which also lets the socket `accept`; Seatbelt checks no `network-inbound` for a Unix listener) and `connect` for Unix sockets whose path lies under `OUT`, and nothing else: no IP or IPv6 at all, and no Unix socket outside `OUT`. So a build can reach neither the kernel's lane sockets, nor the Root's `egress.sock` and `owner.sock`, nor any other socket on the machine (another `~/.init*`, `docker.sock`). Seatbelt matches the resolved path, so a symlink in `OUT` that names an outside socket is refused too (S-B4). Tests must therefore put their sockets under `TMPDIR`, `HOME` or `CARGO_TARGET_DIR`, which G0 sets inside `OUT`, and not under the source tree (`SRC` is read-only anyway). A socket path must still fit `sun_path` (103 bytes on macOS); a path relative to the cwd is checked by its resolved location. As built, G0's commands still run as the kernel's own children (WP8b deviation 1), so root/root.md §3.3.3 section 6a gives the `kernel` profile the same rule for the slot's `OUT` (`SLOT_OUT`). `SRC` and `OUT` must lie under `cache/`, and the Root checks this before it renders a macOS build instance (§3.12).

### 3.5 macOS: the `git-read` profile

Sections 1–4 as in `body`; sections 5–8:

```scheme
; ---- 5. read-only view of the agent's repositories; no writes anywhere ----
(allow file-read-metadata (path-ancestors (param "INIT_HOME")))
(allow file-read* (subpath (param "BODY")) (subpath (param "WORK")))
; ---- 6. network: none ----
; ---- 7. instance mark ----
(allow file-read-data (literal (param "MARK")))
; ---- 8. hard denies ----
(deny file-read* file-write* (literal (param "VAULT")) (subpath (param "VAULT")))
(deny file-read* file-write* (literal (param "CACHE")) (subpath (param "CACHE")))
(deny mach-lookup (xpc-service-name-prefix ""))
```

`init-kernel sandboxed-upload-pack <repo>` runs `$GIT upload-pack [-c uploadpack.allowAnySHA1InWant=true] <repo>` in a `git-read` instance that the Root spawns (both platforms). The shim reaches the Root through the kernel's relay socket `run/spawn.sock` (§3.12). For a probe or fleet repo, `BODY`/`WORK` point into that root. Tested 2026-10-19 and re-run on 2026-10-20 with the v0.3 text: a normal fetch into a bare repo succeeded. With `objects/info/alternates` pointing into `vault/` and a ref naming a vault commit, upload-pack failed with `not our ref`, and the object never reached the bare repo. Without the sandbox, the same setup copied the vault object.

### 3.6 Linux: bubblewrap + Landlock + seccomp

Since v0.3 the Root renders and launches every bwrap instance. It prepends its fixed zone arguments (namespaces, `--disable-userns`, `--cap-drop ALL`, the zone seccomp program) to the per-profile mounts below (§3.13; root/root.md §3.4). The mounts and the inner stage below are unchanged from v0.1 and stay owned here. Parameters are validated by the Root (V1–V6) and by §3.12's class rules.

Layers, outermost first (below the zone):

1. **bwrap** builds the filesystem view and the namespaces: user, pid, ipc, net, uts, cgroup. This is the primary boundary: anything not mounted does not exist (`ENOENT`), and read-only mounts return `EROFS`.
2. **Landlock**, applied by the inner stage after the mounts, restricts the same paths again. It also blocks hard-linking across hierarchies (`REFER`), abstract Unix sockets and signals outside the domain (ABI ≥ 6), TCP (ABI ≥ 4), and `connect` to pathname sockets other than the kernel's (ABI ≥ 9). Best effort: a missing ABI feature is recorded, not fatal.
3. **seccomp**, applied by the inner stage last, denies the syscalls that would bypass the first two or reach the network.

#### 3.6.1 `body` bwrap argv (rendered)

```
/usr/bin/bwrap
  --die-with-parent --new-session
  --unshare-user --unshare-pid --unshare-ipc --unshare-net --unshare-uts --unshare-cgroup-try
  --hostname init --cap-drop ALL
  --tmpfs /
  --ro-bind /usr /usr
  --ro-bind-try /bin /bin   --ro-bind-try /sbin /sbin
  --ro-bind-try /lib /lib   --ro-bind-try /lib64 /lib64   --ro-bind-try /lib32 /lib32
  --ro-bind /etc /etc
  --dev /dev
  --proc /proc
  --tmpfs /tmp
  --ro-bind $TOOLCHAIN $TOOLCHAIN
  --ro-bind $INIT_HOME/kernel  $INIT_HOME/kernel
  --ro-bind $INIT_HOME/bin     $INIT_HOME/bin
  --ro-bind $INIT_HOME/lineage $INIT_HOME/lineage
  --bind    $INIT_HOME/body    $INIT_HOME/body
  --bind    $INIT_HOME/work    $INIT_HOME/work
  --bind    $INIT_HOME/state   $INIT_HOME/state
  --ro-bind $INIT_HOME/state/kernel $INIT_HOME/state/kernel
  --ro-bind $INIT_HOME/run/kernel.sock $INIT_HOME/run/kernel.sock
  --ro-bind $INIT_HOME/run/proxy.sock  $INIT_HOME/run/proxy.sock
  [--ro-bind-try <p> <p>   for each [sandbox] extra_read path]
  --remount-ro /
  --chdir $INIT_HOME/work
  -- $INIT_HOME/kernel/bin/init-kernel sandbox-inner --profile body --home $INIT_HOME
       --toolchain $TOOLCHAIN -- $INIT_HOME/bin/init-body
```

- Order matters. bwrap applies mounts in argument order, so `--ro-bind state/kernel` after `--bind state` overlays a read-only mount on the writable one. `--remount-ro /` comes last and affects only the tmpfs root ("works only on the specified mount point").
- `vault/`, `cache/`, `probe/`, `fleet/`, the Root's sockets and the owner's home are simply never mounted. `ROOT_HOME` is not even mounted in the zone (§3.13). The parents of `$TOOLCHAIN` and `$INIT_HOME` exist only as empty tmpfs directories that bwrap creates.
- `--new-session` calls `setsid()`, so the sandbox has no controlling terminal; bwrap(1) recommends it against `TIOCSTI` injection (CVE-2017-5226).
- `--die-with-parent` sets `PR_SET_PDEATHSIG(SIGKILL)` on the namespace's init. When bwrap or the kernel dies, init dies and the kernel kills every process in the pid namespace. This is what makes the supervise sweep complete on Linux.
- The socket files are bind-mounted individually. They are recreated only when the kernel restarts, and a kernel restart sweeps every sandbox anyway, so a stale mount can never be observed. `connect(2)` to a socket on a read-only bind mount needs write permission on the socket inode, and the read-only-filesystem check applies only to regular files, directories and symlinks (fs/namei.c `sb_permission`), so `--ro-bind` is enough. UNVERIFIED by test; test S-L7 checks it.
- The environment is the one supervise.md §4.1 builds, set on the kernel's `Command` (bwrap passes it through), so `--clearenv` is not used.

**Probe:** the same argv with every live path replaced by the probe root's (the probe sees its own root at its own path, matching macOS), and `--ro-bind $INIT_HOME/kernel $PROBE/kernel --ro-bind $INIT_HOME/lineage $PROBE/lineage --ro-bind $INIT_HOME/bin/ext.d/<set> $PROBE/bin/ext --ro-bind $INIT_HOME/bin $INIT_HOME/bin` instead of symlinks (the candidate binary is executed from the real `bin/`). A G4 task run's workspace is `$PROBE/work/task`, already read-write through the `$PROBE/work` bind.

#### 3.6.2 The inner stage: `init-kernel sandbox-inner`

Runs inside bwrap as the namespace's first program, so it is subject to the mounts. It never runs with more privilege than the body: no capabilities (`--cap-drop ALL`).

```rust
fn sandbox_inner(a: InnerArgs) -> ! {
    // 0. Refuse to continue with any capability: capget() must report empty effective/permitted sets.
    // 1. Landlock (crate landlock 0.4.7, best effort up to ABI::V9)
    let abi = ABI::V9;
    let ro = AccessFs::from_read(abi);                          // Execute | ReadFile | ReadDir
    let rw = AccessFs::from_all(abi);
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))?
        .handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)?   // no net rules: all TCP denied (ABI ≥ 4)
        .scope(Scope::AbstractUnixSocket | Scope::Signal)?            // ABI ≥ 6
        .create()?
        .add_rules(path_beneath_rules(a.ro_paths(), ro))?             // /usr /bin /sbin /lib* /etc /proc $TOOLCHAIN kernel/ bin/ lineage/
        .add_rules(path_beneath_rules(a.rw_paths(), rw))?             // body/ work/ state/ /tmp /dev
        .add_rules(path_beneath_rules([a.home.join("run")], AccessFs::ResolveUnix))? // ABI ≥ 9: only the two mounted sockets live there
        .restrict_self()?;                                            // sets no_new_privs
    eprintln!("sandbox-inner: landlock {:?} abi<=V9", status.ruleset); // goes to the kernel's stderr log
    // 2. seccomp (crate seccompiler 0.5.0)
    let filter = SeccompFilter::new(rules(), SeccompAction::Allow,
                                    SeccompAction::Errno(libc::EPERM as u32), arch())?;
    let prog: BpfProgram = filter.try_into()?;
    seccompiler::apply_filter(&prog)?;                                // current thread; we exec next
    // 3. exec the program
    let err = Command::new(&a.program).args(&a.args).exec();          // CommandExt::exec
    panic!("exec failed: {err}");
}
```

- Landlock cannot carve `state/kernel/` out of the `state/` grant: rules only add access beneath a hierarchy. The read-only bind mount provides that carve-out (`EROFS`). The agent cannot remove the mount: it has no capabilities, and Landlock forbids mount topology changes.
- `RulesetStatus::NotEnforced` (kernel without Landlock) is allowed: bwrap and seccomp still hold. `status` reports it, and `boot.json` carries `sandbox.landlock`.

#### 3.6.3 seccomp rules (deny with `EPERM`; default allow)

```rust
fn rules() -> BTreeMap<i64, Vec<SeccompRule>> {
    use libc::*;
    let not_unix = || SeccompRule::new(vec![
        SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Ne, AF_UNIX as u64).unwrap()]).unwrap();
    let eq1 = |v: u64| SeccompRule::new(vec![
        SeccompCondition::new(1, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, v).unwrap()]).unwrap();
    let flag0 = |f: u64| SeccompRule::new(vec![
        SeccompCondition::new(0, SeccompCmpArgLen::Qword, SeccompCmpOp::MaskedEq(f), f).unwrap()]).unwrap();
    BTreeMap::from([
        (SYS_socket, vec![not_unix()]),               // only AF_UNIX sockets
        (SYS_socketpair, vec![not_unix()]),
        (SYS_io_uring_setup, vec![]), (SYS_io_uring_enter, vec![]), (SYS_io_uring_register, vec![]),
        (SYS_ptrace, vec![]), (SYS_process_vm_readv, vec![]), (SYS_process_vm_writev, vec![]),
        (SYS_bpf, vec![]), (SYS_perf_event_open, vec![]), (SYS_userfaultfd, vec![]),
        (SYS_keyctl, vec![]), (SYS_add_key, vec![]), (SYS_request_key, vec![]),
        (SYS_unshare, vec![]), (SYS_setns, vec![]),   // no nested namespaces (also no nested bwrap)
        (SYS_clone, vec![flag0(CLONE_NEWUSER as u64)]),
        (SYS_mount, vec![]), (SYS_umount2, vec![]), (SYS_pivot_root, vec![]), (SYS_chroot, vec![]),
        (SYS_open_tree, vec![]), (SYS_move_mount, vec![]), (SYS_fsopen, vec![]), (SYS_fsmount, vec![]),
        (SYS_ioctl, vec![eq1(TIOCSTI as u64), eq1(TIOCLINUX as u64)]),
    ])
}
fn arch() -> TargetArch { if cfg!(target_arch = "x86_64") { TargetArch::x86_64 } else { TargetArch::aarch64 } }
```

- An empty rule vector means an unconditional match; rules in one vector are OR-ed, and conditions inside a rule are AND-ed (seccompiler 0.5.0).
- `clone3` cannot be filtered on its flags (they are behind a pointer). `unshare`/`setns` are denied, and the user namespace has no capabilities, so this gap only allows plain threads and processes.
- **x32 ABI:** seccompiler 0.5.0 does not handle the x32 syscall bit. A kernel built with `CONFIG_X86_X32_ABI` could let x32 syscall numbers bypass a deny list. UNVERIFIED; `doctor` warns when `/proc/config.gz` or `/boot/config-*` shows `CONFIG_X86_X32_ABI=y`.
- AF_INET is denied even though the network namespace is empty, so behaviour matches macOS: a test suite that needs loopback fails the same way on both platforms.

#### 3.6.4 `build`, `check` and `git-read` on Linux

Same skeleton (tmpfs root, system ro-binds, toolchain, `--unshare-net`, inner stage) with:

- `build`/`check`: `--ro-bind $SRC $SRC --bind $OUT $OUT --ro-bind $INIT_HOME/kernel $INIT_HOME/kernel`, and no socket mounts. Landlock ro = system + `SRC` + `kernel/`, rw = `OUT` + `/tmp` + `/dev`. Unix sockets the build creates under `OUT` work as on macOS (§3.4 section 6): seccomp allows `AF_UNIX`, and no other socket path is mounted. A `/tmp` socket is also private to the instance (tmpfs).
- `git-read`: `--ro-bind $INIT_HOME/body … --ro-bind $INIT_HOME/work …`, nothing writable except a tmpfs `/tmp`.

#### 3.6.5 Instance identity on Linux

The instance is the pid namespace. The kernel records `stat(/proc/<namespace init>/ns/pid).st_ino` (supervise.md §3.4 step 5). Peer attribution (kernel/initctl.md §3.3) and the sweep (supervise.md §3.8.2) compare against it. Nothing can leave a pid namespace, so no mark is needed.

### 3.7 Launch and inheritance

**Launch** (supervise.md §3.4 has the kernel side; root/root.md §3.5 the Root side). Since v0.3 the Root launches every instance on both platforms:

```
kernel ──{"cmd":"spawn","args":{kind, profile, params, program, args, env, cwd, lane?, replaces?}} on fd 3──▶ init-root
init-root ──posix_spawn──▶ init-root exec-clean ──execv──▶ /usr/bin/sandbox-exec -p <composed profile> -D… -- <program>   (macOS)
                                                └─execv──▶ /usr/bin/bwrap <zone args> <mounts §3.6> -- init-kernel sandbox-inner … -- <program> (Linux)
init-root ──fd channel (fd 4): {"id","n":3} + SCM_RIGHTS(stdin_w, stdout_r, stderr_r)──▶ kernel, then the response {instance, pid, started_at}
```

- **Absolute launcher paths only** (`/usr/bin/sandbox-exec`; `[sandbox] bwrap`, default `/usr/bin/bwrap`), never resolved through `PATH` (Codex's reasoning).
- **One sandbox per instance**, applied before the body's first instruction. The body never sandboxes itself and never needs to.
- **Inheritance.** Every descendant of the body (bash, cargo, rustc, test binaries, background daemons) runs under the same policy. On macOS a sandboxed process can neither drop nor widen its policy, and `sandbox_apply` from inside fails, even when the outer profile is `(allow default)` (tested 2026-10-20; this is why the Root launches every instance). On Linux, mounts and namespaces are inherited, Landlock domains are inherited by children and can only be narrowed ("there is no way to remove its security policy"), and seccomp filters are inherited across `fork`/`execve` under `no_new_privs`.
- **Escape by fd, not by path.** Seatbelt checks paths at open time. An fd inherited from outside bypasses it: a sandboxed `bash` wrote into a file under `state/kernel/` through an fd opened outside (tested). Therefore the Root's `exec-clean` closes every fd ≥ 3, and the body's stdio are pipes created by the Root. The Root passes the *parent* ends of those pipes to the kernel, never to the instance, and keeps no copy (root/root.md §3.5.3).
- **Environment:** fully rebuilt by the kernel and sent in `spawn.env` (supervise.md §4.1). The Root refuses `INIT_ROOT_*` keys (V4). No secret is ever in it.

### 3.8 Platform prerequisites (`init-kernel doctor`)

| Check | Requirement | On failure |
|---|---|---|
| macOS version | 14.x tested (2026-10-19). 15.x and 26.x UNVERIFIED: run §8 on each before use | refuse until §8 passes |
| `/usr/bin/sandbox-exec` | present. The man page marks it DEPRECATED (also `sandbox_init(3)`), but it works on 14.6, and Codex and Claude Code both rely on it | refuse to start (`Halted{tooling}`) |
| `sandbox_check` | `dlsym(RTLD_DEFAULT, "sandbox_check")` non-null (needed by the Root's sweep and by initctl peer attribution, which works from inside the zone, S-Z10) | run degraded; alert |
| Root spawn broker | `init-root status --selftest` (RT-1, RT-2) and this doc's suite pass: every composed profile parses (S-M0) | the Root halts with `Halted{tooling}` (root/root.md §3.6.5) |
| Linux bwrap | `[sandbox] bwrap` exists; version ≥ 0.8.0 (UNVERIFIED that every flag used exists since then; `doctor` probes each flag with a dry run). bubblewrap ≥ 0.12.0 dropped setuid mode, so unprivileged user namespaces are mandatory | refuse to start |
| user namespaces | `bwrap --unshare-user --unshare-net --ro-bind / / /bin/true` succeeds | refuse; print the fix |
| Ubuntu ≥ 23.10 AppArmor | if `kernel.apparmor_restrict_unprivileged_userns = 1`, the zone's `unshare` and bwrap (bwrap runs inside unshare's namespace) need a profile granting `userns`; Ubuntu's `bwrap-userns-restrict` and `unshare-userns-restrict` do not cover that nesting (measured 2026-10-27), so INIT's own step installs root-owned copies of both with a profile that grants only them `userns` (docs/platforms.md §3.4.6). INIT never asks for the global sysctl | refuse; print INIT's one-time sudo commands (`install.sh --apparmor` runs them after asking) |
| Landlock ABI | read with `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)`; ≥ 1 recommended, ≥ 6 for scopes, ≥ 9 for `ResolveUnix` | warn only |
| git | ≥ 2.39.4 | refuse |
| Rust toolchain | `[sandbox] toolchain` exists, contains `lib/rustlib/wasm32-wasip2/` (target installed with `rustup target add wasm32-wasip2` by the owner) and `wasm-component-ld` under `lib/rustlib/<host>/bin/` (ext/authoring.md, ext/host.md). Both are read and executed through the `TOOLCHAIN` rule | refuse `install`; warn at start |

Landlock ABI by kernel: 1 = 5.13, 2 = 5.19 (`REFER`), 3 = 6.2 (`TRUNCATE`), 4 = 6.7 (TCP), 5 = 6.10 (`IOCTL_DEV`), 6 = 6.12 (scopes), 7 = 6.15 (audit flags), 8 = 7.0 (`TSYNC`), 9 = 7.1 (`RESOLVE_UNIX`), 10 = 7.2 (UDP), 11 = 7.3. The `landlock` crate 0.4.7 supports up to V9.

### 3.9 How gate runs are sandboxed

Spec §2.1 invariant 2: evaluation runs "in a fresh process outside the agent's sandbox". Concretely, every gate activity is a separate sandbox instance with its own parameters, started by the kernel. None of them shares the live body's instance, and none can read `vault/`:

| Gate activity | Profile | Sees | Cannot see |
|---|---|---|---|
| fetch of the candidate | `git-read` | agent repos, read-only | everything else; cannot write |
| G0 build/clippy/test, stub deps builds | `build` | exported source, its own output dir (and Unix sockets inside it, §3.4), toolchain | live `body/`, `state/`, `vault/`, IP network, every socket outside its output dir |
| G1 selfmap | `build`, as a subprocess (`SRC` = the export, `OUT` = `cache/gate/<job>/`): the generator runs `init-body --manifest` and instantiates components to call `describe()` (kernel/selfmap.md, kernel/gate.md), so it executes candidate code | the export, the candidate binary and components, toolchain | live `body/`, `state/`, `vault/`, network |
| G2 probe, G3 evolvability | `body` with probe parameters | its probe root (a copy of the log, a clone of lineage), its lane sockets | live `body/`/`work/`/`state/`, `vault/`, the Root's sockets |
| G4 task runs (gate.md) | `body` with probe parameters; the task's workspace copied into the probe's `work/` | the task prompt and workspace | holdout answers, checks, canaries (they stay in `vault/` and the kernel) |
| acceptance checks | `check` | the finished workspace (read-only `SRC`) and the check script copied from `vault/` by the kernel | other tasks, the rest of `vault/` |

The kernel copies exactly the inputs an instance needs into its root before starting it. Scores are computed by the kernel from exit codes and outputs; no sandboxed process writes a verdict. For G4 task runs nothing is copied back to agent-readable places (supervise.md §3.6.3 step 5), and the live profile cannot read `probe/` at all (default deny; S-M50), so holdout prompts never reach the live agent.

**What changed in v0.3.** The kernel itself is now inside the agent zone, so everything in the table above is hidden from *bodies*, but not from the zone. From L3 the gate is agent-evolvable. The honest measuring stick no longer rests on this table. It rests on the Root's sealed evaluator (spec v0.3 R6 and §16: the sealed task set, the gate-of-gates and the sealed audit), which runs in Root-launched instances that the zone cannot see (root/sealed-eval.md). This table remains the body-facing stability layer: a body under test still cannot see its holdout prompts or checks. Per D5, sealed audits and gate-of-gates runs never pass through the kernel or its probes. The Root launches an isolated reference body and evaluator itself, on its own egress lane (root/sealed-eval.md, root/root.md). It may render this doc's `body` and `check` templates for them with Root-chosen roots that no agent-zone process can read. Root/sealed-eval.md's M7 (a body reading sealed content) is an OS refusal here (S-X3).

### 3.10 Known limitations

- **No address-space limit.** The kernel sets no `RLIMIT_AS` and no profile limits virtual memory, because the body's Wasmtime pooling allocator reserves about 130 GiB of virtual address space (ext/host.md Q-H5). Physical memory is not reserved by this.
- **No CPU or memory limits on macOS** (no cgroups). A fork bomb is limited only by `kern.maxprocperuid`, which applies to the owner's whole session. Linux: optional `[sandbox] linux_memory_max` wraps bwrap in `systemd-run --user --scope -p MemoryMax=<v>` (UNVERIFIED on non-systemd hosts; off by default).
- **`sandbox-exec` is deprecated.** If a future macOS removes it, INIT refuses to start rather than run unsandboxed (Codex issue #38028 evaluates Endpoint Security as a successor; title only, not reviewed).
- **macOS `kern.proc.pid.*` sysctls** (kept from Codex's list) let the agent read basic information about other processes by pid. `ps` (which needs `kern.proc.all`) is denied (tested).
- **Programs that need `/dev/tty`** (ssh password prompts, sudo) do not work. They would be out-of-sandbox actions anyway.
- **`file-map-executable` rules** were not needed in testing (a proc-macro crate built fine without the `WORK` rule on 14.6). They stay for versions that enforce them.
- **Instance profiles are Root-rendered (v0.3), on both platforms.** The text of §3.3.1, §3.4, §3.5 and the Linux mount lists of §3.6 are compiled into `init-root` (root/root.md §3.3.1, §3.4). From L3 the agent can change the kernel but not these texts; a change needs a Root release. Only the inner stage `sandbox-inner` (Linux) stays kernel code, and it can only narrow.

### 3.11 The agent zone (v0.3): Root-composed profiles

Spec v0.3 §13 puts the kernel and every body in one **agent zone**, confined by `init-root`, whose only network path is the Root's egress socket. The only security boundary is Root versus everything else. Since v0.3 the instance profiles of §3.3–§3.6 (`body`, `build`, `git-read`, `check`) are a **stability layer**: they keep a body, which may be running a fresh self-edit, from damaging the kernel's files, the gate's inputs or another body. They are no longer what keeps the agent from the network or the Root.

**Seatbelt cannot nest** (tested 2026-10-20, macOS 14.6). `sandbox-exec -p '(version 1)(allow default)' sandbox-exec -p '(version 1)(allow default)' /usr/bin/true` fails with `sandbox_apply: Operation not permitted` (exit 71). A sandboxed kernel therefore cannot sandbox its bodies. root/root.md settles the consequence for both platforms, and this doc adopts it:

- **The Root spawns every agent-zone process** through its spawn broker (root/root.md §3.5): the kernel itself, live bodies, fleet members, probes, builds, checks and git reads. The kernel asks with `spawn` over the control link and receives the stdio pipes as passed fds (kernel/supervise.md §3.4, §3.13). One spawn path on both platforms keeps the kernel's code identical, and the Root always knows every instance.
- **Profile texts are compiled into `init-root`.** The kernel names a profile id and supplies parameter values, never text. root/root.md §3.3.1 composes each macOS profile as:

  ```
  profile(id) = PRELUDE       this doc §3.3.1 sections 1–4 (identical for every profile)
              + SECTION(id)   body: §3.3.1 sections 5–8 · build/check: §3.4 · git-read: §3.5 · kernel: root/root.md §3.3.3
              + ROOT_ALLOWS   the instance mark and the zone mark
              + ZONE_TAIL     root/root.md §3.3.2, deny-only, always last
  ```

  This doc still owns the text of PRELUDE and of the four instance SECTIONs. A change to any of them ships as a Root release (root/root.md §9.1 X2). Two Root tests make the rule mandatory, not just tidy. SBPL is evaluated as Scheme, so zone-authored text could `(define (deny . args) #t)` and switch the tail off. And the last matching rule wins.
- **The zone tail holds whatever an instance profile says.** It denies `ROOT_HOME` (Z1), denies reads of the Root's paths (Z2; since D9 Z2 denies only Root paths, so charter-allowed `extra_read` paths work; from v0.1.1 Z2 also denies reads and writes of ADR-009's deny list (`Z_DENY_<i>`) and writes to `~/Library/LaunchAgents`, in every mode), confines writes to `INIT_HOME` (Z3; from v0.1.1 also `Z_WRITE`: the validated workspace, or the owner's home under `--full`, Root-set; §3.1.1), protects the Root-written parts of `INIT_HOME` (Z4: `kernel/`, `root/`, `run/mark/`, `run/zone-mark`), lets an instance read only its own mark (Z5), allows no IP and only `INIT_HOME` Unix sockets plus the egress socket (Z6), and blocks XPC and cross-sandbox signals (Z7).
  - That is why this doc's instance SECTIONs name no Root path. The v0.1 `OWNER_SOCK` deny is gone (the owner socket now lives in `ROOT_HOME/run/`, under Z1 and Z6).
  - A body still cannot reach the egress socket that Z6 leaves open: its SECTION allows only `KERNEL_SOCK` and `PROXY_SOCK` (S-M32).
- **`MARK` is set by the Root** (`MARK = Z_MARK`, under `INIT_HOME/run/mark/<id>`). The kernel never supplies it (V1).

**What the kernel needs from the zone**, checked against root/root.md §3.3.3's `kernel` profile:

| Need | Why | root/root.md `kernel` profile | Status |
|---|---|---|---|
| read all of `INIT_HOME` except other marks | lineage, `bin/`, probe and fleet roots, session copies | §5 | met |
| write `INIT_HOME` except `kernel/`, `root/` and marks | `bin/`, `state/`, `lineage/`, `cache/`, `vault/`, `probe/`, `fleet/`, `run/` | §5 + tail Z4 | met |
| run git, tar, sh, `init-kernel`, cargo for `vendor` | lineage operations, exports, vendoring | prelude `process-exec` | met (git and tar tested by the Root; cargo by S-Z15 below) |
| bind and serve Unix sockets for every lane | `run/kernel.sock`, `run/proxy.sock`, probe lanes, **fleet lanes** | §6: `network-bind`/`inbound` only under `RUN` and `PROBE` | met, because fleet lane sockets live under `run/fleet/<bid>/` (§3.3.5), not under `fleet/` |
| connect the egress socket | the proxy's upstream; `vendor` fetches | §6 | met |
| `sandbox_check` on another instance's pid, with the path filter | initctl peer attribution (kernel/initctl.md §3.3) | no rule needed | **tested** 2026-10-20 with a compiled C helper from inside a sandbox: `sandboxed=1 mark=0 other=1`, exact (S-Z10). This settles root/root.md §9.1 X3's UNVERIFIED path-filter form |
| no signals to other instances | kills go through the Root's `kill` | tail Z7 | as designed (S-Z11) |
| read the published Root view `INIT_HOME/root/` | level, ceilings, lineage for `status`/`level` | §5 | met |

**Former conflicts with root/root.md, settled by the v0.3 decisions** (`docs/reconciliation/v03-decisions.md`):
1. **`extra_read` (D9):** tail Z2 now denies only Root paths, so `[sandbox] extra_read` paths that the charter allows outside `ROOT_HOME` are readable again (§4.3).
2. **`LINK_*` and V2 (D8):** probe and member roots no longer contain symlinks (§3.3.3), so every parameter the kernel sends is canonical. The Root still accepts both `LINK_*` forms: a symlink only inside a probe or member root, and the live root's `LINK_*` must equal its target.
3. **Sweep membership (D1):** every sweep and kill uses three conditions: sandboxed, may read the mark, and denied the Root canary or a sibling decoy (§3.3.4 §7, supervise.md §3.8.2). Sweep and kill tests run only in a disposable environment (CONVENTIONS §1).

The reference zone text this doc proposed earlier on 2026-10-20 is superseded by root/root.md §3.3.2–§3.3.3. The S-Z rows of §8 were re-run against root/root.md's composed texts.

### 3.12 macOS: rendering rules the Root applies to this doc's templates

The Root renders SECTION(id) from this doc and validates the kernel's parameters (root/root.md §3.3.4, V1–V6). This doc adds the class rules that keep each template inside its intended area. They are hygiene, because the tail holds regardless, and they mean that a wrong parameter is refused instead of silently widening a stability rule:

- `BODY`, `WORK`, `STATE` and `STATE_KERNEL` are exactly `<root>/body`, `<root>/work`, `<root>/state` and `<root>/state/kernel`, where `<root>` is `INIT_HOME`, `INIT_HOME/probe/<id>/` or `INIT_HOME/fleet/<bid>/`.
- `KERNEL_SOCK` and `PROXY_SOCK` are `INIT_HOME/run/{kernel,proxy}.sock` (live), `INIT_HOME/probe/<id>/run/…` (probe), or `INIT_HOME/run/fleet/<bid>/…` (member).
- `SRC` and `OUT` lie under `INIT_HOME/cache/`.
- `LINEAGE`, `VAULT`, `CACHE` and `PUB` are the real `INIT_HOME/{lineage,vault,cache,root}`. For a body, `KERNEL` and `BIN` are `INIT_HOME`'s or the instance root's (§3.3.3); `build` and `check` use `INIT_HOME/kernel`.
- `LINK_*` equal `KERNEL`, `LINEAGE`, `BIN` (real directories, D8), or, inside a probe or member root only, name a symlink there.

The stdio pipes are created by the Root and their parent ends are passed to the kernel over the fd channel (`SCM_RIGHTS`, root/root.md §3.5.3). Receiving fds over an inherited socketpair is not path-checked by Seatbelt, and the Root tested it under `(deny network*)`.

`init-kernel sandboxed-upload-pack <repo>` cannot reach the Root: it is a child of `git`, and the control link's fds are close-on-exec in the kernel. It therefore connects to the kernel's internal relay socket `run/spawn.sock`. Only the `kernel` profile can reach it, because no instance SECTION allows that path. The kernel then:
- checks that `<repo>` lies under `body/`, `work/`, a probe root or a fleet root;
- asks the Root to `spawn{kind:"git_read"}` running `$GIT upload-pack <repo>`;
- passes the three fds on to the shim.

The shim copies bytes between its own stdio and the instance, and exits with the instance's code (kernel/supervise.md §3.6.1).

### 3.13 Linux: Root-rendered bwrap; this doc's argv and inner stage

On Linux the Root also launches every instance (root/root.md §3.4). It prepends fixed zone arguments to this doc's per-profile argv (§3.6.1, §3.6.4):
- `--unshare-*` for every namespace;
- `--die-with-parent`;
- `--disable-userns`, so the zone cannot create further user namespaces;
- `--cap-drop ALL`;
- a zone seccomp program on fd 5.

There is therefore **no nesting** on Linux either. The body is one bwrap instance, and its mounts are this doc's.

This doc still owns:
- the mount list of each profile, with the parameter checks of §3.12;
- the inner stage `init-kernel sandbox-inner` (Landlock, then the body seccomp filter, then exec; §3.6.2), which runs inside the Root-launched bwrap as before.

The zone seccomp and the instance seccomp stack, and both can only narrow. The v0.1 rule "`unshare` and nested bwrap denied" (S-L6) is now also guaranteed by the Root's `--disable-userns`.

Kills: the Root sends `SIGKILL` to an instance's bwrap pid and confirms by pid-namespace inode (kernel/supervise.md §3.8.2, executed by the Root).

Peer attribution for initctl changes on Linux. The kernel and the bodies now live in *sibling* pid namespaces. `SO_PEERCRED` reports a peer's pid translated into the receiver's namespace, which is 0 when the peer is not visible there (UNVERIFIED by test; S-L19). So the kernel attributes a Linux connection by **socket path**. Each instance's mount namespace contains only its own two lane sockets, bound individually (§3.6.1), so a body cannot even name another lane's socket (kernel/initctl.md §3.3).

## 4. Interfaces

### 4.1 Types

```rust
pub enum ProfileId { Body, Build, Check, GitRead }      // wire ids: "body" "build" "check" "git-read"

pub struct SandboxParams {
    pub profile: ProfileId,
    pub home: PathBuf,                     // INIT_HOME as seen by the instance (probe or fleet root)
    pub real_home: PathBuf,                // the real INIT_HOME (kernel, bin, lineage, vault, cache, run)
    pub roots: BTreeMap<&'static str, PathBuf>, // BODY, WORK, STATE, SRC, OUT, LINK_KERNEL, … (all canonical, D8)
    pub sockets: Vec<(&'static str, PathBuf)>,  // KERNEL_SOCK, PROXY_SOCK
    pub toolchain: PathBuf, pub devdir: Option<PathBuf>, pub extra_read: Vec<PathBuf>,
}

pub fn spawn_args(p: &SandboxParams, kind: &str, program: &Path, args: &[OsString],
                  env: &BTreeMap<String, String>, cwd: &Path, lane: Option<&str>,
                  replaces: Option<&str>) -> serde_json::Value;
// → {"kind":"body"|"probe"|"build"|"check"|"git_read","profile":"body"|"build"|"check"|"git-read",
//    "params":{NAME: path…},"program","args","env","cwd","lane"?,"replaces"?}
// the `args` of root/root.md §4.1.3 `spawn`. MARK and every Z_* parameter are the Root's, never the kernel's (V1)
```

`spawn_args` canonicalizes every path, rejects any path containing `"`, a newline or NUL, rejects a `home` longer than 64 bytes, and applies §3.12's class rules. The Root repeats its own checks V1–V6 (root/root.md §3.3.4), because from L3 the kernel is agent-written. v0.1's `launcher_argv` moved into the Root.

### 4.2 Status reported to the body and to `status`

`boot.json.env.sandbox` (owned by supervise.md §4.3; read verbatim by awareness.md §4.2) carries the applied profile, so the body never has to infer its own sandbox:

```json
{"hidden":["cache/","fleet/","probe/","vault/"],"kind":"seatbelt","landlock":null,"landlock_abi":null,"network":"kernel-sockets-only","read_only":["bin/","kernel/","lineage/","state/kernel/"],"seccomp":false,"sweep":"full","write":["body/","state/","work/"],"zone":{"launcher":"root","network":"root-egress-only"}}
```

| Field | Values |
|---|---|
| `kind` | `seatbelt` \| `bwrap` |
| `write`, `read_only`, `hidden` | paths relative to `INIT_HOME`, sorted; from the profile's parameters. Paths outside `INIT_HOME` are not listed (system and toolchain reads are summarized by the body's own probe of available CLIs) |
| `network` | `kernel-sockets-only` (the two sockets plus agent-local Unix sockets under `work/`) |
| `landlock`, `landlock_abi` | Linux: `full` \| `partial` \| `none` and the ABI number; macOS: null |
| `seccomp` | Linux: true; macOS: false |
| `sweep` | `full` \| `degraded` (supervise.md §3.8.2) |
| `zone` | `{launcher, network}`. `launcher`: `root` (both platforms since v0.3). `network`: `root-egress-only` (the zone reaches the network only through the Root; the body reaches only the kernel's proxy) |

`initctl status` carries the same object as `sandbox`. `hidden` now lists `fleet/`. `read_only` would also list `root/` for bodies (the published view, `PUB`). The Root's own paths lie outside `INIT_HOME` and are not listed.

From v0.1.1 this object does not describe the home read or the workspace, which lie outside `INIT_HOME`. The body learns them from `INIT_ACCESS` and `INIT_WORKSPACE` (body/boot.md §3.2) and states them in the access line of its boot block (awareness.md §3.6).

### 4.3 Configuration (`config.toml` section `[sandbox]`; file ownership: kernel/config.md)

| Key | Default | Meaning |
|---|---|---|
| `toolchain` | resolved at install from `rustup which rustc` | read + exec |
| `developer_dir` | `/Library/Developer/CommandLineTools` (macOS) | `DEVDIR`, `DEVELOPER_DIR` |
| `sdkroot` | `xcrun --show-sdk-path` at install | `SDKROOT` |
| `bwrap` | `/usr/bin/bwrap` | Linux launcher |
| `extra_read` | `[]` | additional read-only paths (owner choice, e.g. `/opt/tools`). They are readable when the charter allows them and they lie outside `ROOT_HOME`: since D9 the Root's tail rule Z2 denies only Root paths |
| `linux_memory_max` | `""` (off) | systemd scope memory cap. With the zone in v0.3 this cap applies per instance inside it; the zone's own compute and disk quotas are the Root's (R2, root/egress.md) |

The home read, its deny list and `--full` are not config keys. They decide what the zone may reach, so they are charter keys the agent can never write: `sandbox.read_home`, `sandbox.read_deny` and `sandbox.allow_full` (root/root.md §3.12.1, ADR-009).

## 5. Failure modes and recovery

| Failure | Detection | Effect / recovery |
|---|---|---|
| profile syntax error (e.g. a bad parameter) | `sandbox-exec` exits 65 before exec | the instance crashes at once; kernel tests (S-M0) catch it at build time; `doctor` runs a dry launch |
| a tool needs a denied resource | tool error `Operation not permitted` / `EROFS` / `ENOENT` | visible to the agent in the tool output; the owner can add `extra_read`; sysctl/mach additions need a kernel release |
| bwrap missing or userns blocked | `doctor`, cold start | `Halted{tooling}`; never fall back to unsandboxed |
| Landlock unavailable | status `NotEnforced` | continue (bwrap + seccomp); report `landlock:"none"` |
| `sandbox_check` symbol gone | `dlsym` null | sweep degraded (process group + session); alert once |
| mark or exclusion path missing | the sweeper (the Root) checks that both exist before using them | it recreates the mark, and never sweeps by mark while the exclusion path (canary or decoy) is missing, because every foreign sandbox that can read the directory would match. It falls back to the process group and session instead (fail closed: kill less, never more) |
| owner's `INIT_HOME` moved under a symlink | canonicalization differs | refuse at start (`E_LAYOUT`) |
| v0.1.1: `init-agent` run in `~`, or a cwd the Root rejects (root/root.md §3.17.1) | the Root's validation | no workspace: `INIT_ACCESS=read_home` (or `none` with `sandbox.read_home = false`), the home is read-only, and the Root's notice says `[root] no workspace: cd into a project and run init-agent to let me edit it` |
| v0.1.1: a tool writes outside the workspace, or touches a deny-list path | `EPERM` from tail Z3 or Z2 | visible in the tool output; the boot block's access line names the access mode, so the agent can tell the owner to run `init-agent` in the right directory or turn on `--full` |
| the Root refuses a `spawn` (`root.args`: a parameter outside its class or the zone, an unknown profile) | error response | a kernel bug, or a hostile L3 kernel. The job fails with `E_INTERNAL`, and the kernel reports an alert |
| Root link down | EOF on fd 3 | no instance can start or be killed. The kernel stops its bodies and exits within 10 s, and the Root sweeps the zone before any restart (supervise.md §3.13.1) |

## 6. Performance notes (spec §9)

- **Rank 3, tool path:** the sandbox is applied once per instance, and tools pay nothing extra. Measured on M2, macOS 14.6: `sandbox-exec` with this profile adds about 20 ms to the body's start (21.7 ms median vs 1.7 ms for a bare `/usr/bin/true`). A `bash -c` running two commands inside the sandbox took 28.1 ms in total, so each exec inside costs a few ms, as it does outside. Linux bwrap start is similar (namespace setup); not measured here.
- **Builds:** a warm `cargo build` of a small crate inside the `build` profile took 0.98 s, the same as outside. Seatbelt checks are per open/exec, not per read/write.
- **Rank 5, footprint:** macOS sandboxes need no resident helper. On Linux, each instance adds one bwrap process (and its namespace init) of a few hundred KiB.
- **v0.3 launch cost:**
  - A spawn adds one request and response on the control link plus one `SCM_RIGHTS` datagram (sub-millisecond; UNVERIFIED, not measured) to the same ~20 ms `sandbox-exec` cost, once per instance.
  - Rendering adds the zone tail's ~30 rules to each profile; Seatbelt cost is per check, so tools still pay nothing extra.
  - Tool calls inside an instance pay nothing extra on either OS.

## 7. Invariants and security (spec §2.1; v0.3 §13–§14; A6)

Since v0.3 the **only security boundary is the Root versus everything else** (spec §13). The Root's zone tail, appended last to every agent-zone profile, enforces it, and the Root owns it (root/root.md §3.3). The instance profiles are a stability layer inside it: they keep a body, which may be running a fresh self-edit, from damaging the kernel's files, the gate's inputs or another body.

| Boundary / A6 clause / invariant | Enforced by |
|---|---|
| Root versus everything (security) | the tail on every profile: `ROOT_HOME` denied (S-Z4, S-M53); network only through `INIT_HOME` sockets and, where a SECTION allows it, the egress socket (S-Z5–S-Z7, S-M31, S-M32); `kernel/`, `root/` and marks read-only (S-Z3); no sandbox can be applied from inside (S-Z12); nothing can signal outside its own sandbox (S-Z11). Every instance is Root-launched from Root-compiled text (§3.11–§3.13) |
| writing `kernel/` refused | zone: tail Z4 (S-Z3) / read-only bind. Bodies: no write rule covers `KERNEL` (deny default) → `EPERM` (S-M10). Linux: `--ro-bind` → `EROFS`, plus Landlock ro |
| bodies cannot read `vault/` and `cache/` | macOS: not allowed, plus an explicit last-position deny including metadata → `EPERM`. Linux: never mounted → `ENOENT`. Since v0.3 `vault/` is kernel-private, not sealed: sealed material lives in the Root (R6) |
| bodies reach no host but the kernel's proxy | macOS: no IP `system-socket`/network rules; only two `path-literal` Unix sockets → `EPERM`; DNS fails (no mDNSResponder); the Root's sockets are unreachable (default deny and tail Z6; S-M31, S-M32, S-M32b). Linux: empty network namespace; seccomp denies non-`AF_UNIX` sockets; only the two sockets are mounted; Landlock `ResolveUnix` (ABI ≥ 9) |
| `unsafe` refused | compiler: `#![forbid(unsafe_code)]`; G0 checks the attribute is present and unchanged (kernel/gate.md) |
| recoverability: bodies cannot write `kernel/`, `bin/`, `lineage/`, `state/kernel/` | as above; the anchors and the `require-not` carve-out stop rename/unlink/create tricks; hard links into protected files are denied (tested). Whole-system recoverability is the Root's (R1): its LKG copies sit in `ROOT_HOME`, out of the zone's reach |
| honest measuring stick | §3.9 for the body-facing layer; the Root's sealed evaluator (R6) for the agent-proof layer |
| no keys anywhere in the zone | v0.3: keys live only in the Root (R2, root/egress.md). Nothing under `INIT_HOME` holds a key, and every environment is rebuilt without one (S-X1) |
| v0.1.1, workspace mode: the owner's secrets stay unreadable, and writes stay in scope | tail Z2 denies ADR-009's deny list and the service directory (S-A2, S-A14, S-A17b); tail Z3 confines writes to `INIT_HOME` plus the Root-validated workspace (S-A3, S-A7–S-A9); the grants exclude `INIT_HOME`; probes get no grant (S-A19); tails Z3a and Z3b keep shell startup files, git config and hooks, PATH launchers and login items unwritable (S-A20). All use Root-set parameters only |
| v0.3.1, full mode (ADR-012): the Root-protected set holds | the full tail denies `ROOT_HOME` and its sockets (S-A11, S-A15, S-A15c), the service file and directory (S-A14, S-A17b, S-A24), INIT's launchers (S-A23, S-A23a, S-A23b), `INIT_HOME`'s protected parts (S-A13), INIT's key directory and the charter's `read_deny` (S-A12c–f), and signals outside the sandbox (S-A28); members and probes are never full (S-A18–S-A19d) |

Bodies cannot signal, ptrace or inspect the kernel: Seatbelt `same-sandbox` targets (on macOS the kernel is in a different sandbox instance); Linux pid namespace, seccomp `ptrace` deny and Landlock `Scope::Signal`. The zone, in turn, can signal only its own sandbox. S-Z11 tested this against a body instance, and the same `(target same-sandbox)` rule covers the Root and every other process of the owner.

## 8. Test plan (the A6 sandbox suite)

`init-kernel doctor --sandbox-selftest` runs these checks **inside each profile** through `init-kernel sandbox-selftest-inner`, a built-in Rust routine that performs each operation and prints one JSON result line, so the suite does not depend on python or bash being present. CI runs it on every supported macOS version and on Linux (Ubuntu 24.04 with the AppArmor profile; Fedora; a kernel ≥ 7.1 for ABI 9). The runner hardware is **PENDING OWNER** (CONVENTIONS §6, CI; E4 in `reconciliation/evolution-testing.md`); until then the suite runs locally on the owner's macOS 14.6 machine.

**Live-system safety (CONVENTIONS §1; D1).** The suite has two halves:
- **Any machine, including the owner's:** every check that only opens files, sockets or programs, or signals processes the suite spawned itself, by the PID it holds. This covers S-M1–S-M39, S-M42–S-M46, S-M48–S-M61 (except S-M47), S-B, S-G, S-Z (except S-Z11, which signals only a body the suite spawned and is therefore allowed), and S-X1–S-X3. `init-kernel doctor --sandbox-selftest` runs only this half.
- **Disposable environment only** (a VM, a container, or a separate macOS user account with nothing else running): S-M40 (`kill -0` on a process the suite did not spawn), S-M41 (`kill -0` on pid 1), S-M47 (a sweep), S-Z17a–f (testing.md SW-1…SW-6), S-L12, S-L17, and every test of a sweep or kill rule. A sweep identifies its targets by a predicate, and on 2026-10-20 a sweep test on the live machine killed 33 of the owner's processes (below). On the owner's machine, S-M40 is replaced by S-M40b: `kill -0` on an unsandboxed `sleep` that the suite spawned, by its PID.

**How it was run before the build phase.** `sandbox-selftest-inner` does not exist yet. A scratch harness plays the Root and the kernel:
- It extracts every text verbatim: this doc's §3.3.1, §3.4 and §3.5, and root/root.md's ROOT_ALLOWS, zone tail and `kernel` SECTION.
- It composes each profile exactly as root/root.md §3.3.1 does: PRELUDE + SECTION + ROOT_ALLOWS + ZONE_TAIL, with every `Z_*` parameter filled as the Root would.
- It builds a throwaway `INIT_HOME` and `ROOT_HOME` (with `run/owner.sock`, `run/egress.sock`, `run/canary`).
- It runs each check inside a real `sandbox-exec` instance through a python probe, with cwd = the instance's `work/`, as the kernel sets it.

Socket paths are bound relative to their directory, because scratch paths exceed `sun_path`.

**Last run: macOS 14.6 (M2), 2026-10-20, v0.3 texts, 123/123 pass:**
- S-M0 4/4: the composed body, build, git-read and kernel profiles parse;
- live body 63/63;
- probe 9/9;
- fleet member 6/6;
- `build` 7/7;
- `git-read` 4/4;
- `kernel` profile (S-Z) 30/30.

An earlier run the same day used the uncomposed texts and this doc's superseded zone draft; it passed 113/113.

**On-machine targets (macOS 14.6, WP6a D5, D6, D8).** Seatbelt on 14.6 does not refuse socket creation, even with only `(allow system-socket (socket-domain AF_UNIX))`; it refuses `bind`, `connect` and `sendto`. The IP rows therefore target the runner's own loopback listener and UDP socket, so nothing leaves the host: S-M35, S-M37, S-Z7a–b, S-B4c, S-B4k and S-B4m (the runner's IPv6 `[::1]` listener); S-M36, S-M37b and S-B4l are ephemeral loopback binds. S-M38 (DNS) is skipped, because it would send DNS traffic; S-M34 covers the resolver socket, and S-Z7c connects to it. S-M16 and S-M17 are `sandbox_check` policy queries on existing paths (for a missing path it reports "denied" even for an unsandboxed process), calibrated by CAL-1 (`work/` allowed) and CAL-2 (`kernel/` denied); S-M17r is the same check as a real write inside the test directory. S-M46 builds a local proc-macro crate with no external dependencies, because `vendor/` is outside the sandbox. The Rust self-test (`root/sandbox-profiles`, `init-sbx-selftest`) ran 240 pass, 0 fail and 11 skipped of 251 checks on macOS 14.6 (2026-10-20, with the S-B4 and S-Z8p–t rows of the `OUT` socket rule).

Expected errno is `EPERM` for every deny. A probe started with a cwd outside its root fails path lookups that need the cwd (seen in R2), so the kernel always sets `cwd` inside the root (supervise.md §3.4).

**Sweep safety (S-M47), learned 2026-10-20.** The first v0.3 run used the v0.2 membership rule (mark only), on the owner's live machine, with the test mark under `/private/tmp`. `sandbox_check` reported 31 foreign sandboxed processes of the owner as able to read the mark: per-user Apple agents such as `cfprefsd`, `tccd`, `secd`, `trustd` and `NotificationCenter`, app XPC video decoders, and Docker Desktop's Virtualization.framework VM process. The test sweep sent 33 of them `SIGKILL`. launchd restarted the Apple agents; Docker Desktop stayed down. Rules since:
- **D1:** membership needs the mark allowed **and** an existing exclusion path denied, the Root canary or a sibling decoy (§3.3.4 §7);
- **CONVENTIONS §1:** sweep and kill tests run only in a disposable environment; on the owner's machine a test may signal only PIDs it spawned.

The later S-M47 runs that day listed members first and killed only the escapee the test had spawned, by its PID. Membership was exact with either exclusion path: 1 member, 31 excluded. Future S-M47 runs belong in a disposable environment.

| ID | Operation | Expect |
|---|---|---|
| S-M1–3 | write a file in `body/`, `work/`, `state/` | allow |
| S-M4–7 | write `state/kernel/x`; overwrite `lkg.json`; rename `state/kernel`; unlink `lkg.json` | deny |
| S-M8 | create `work/kl → kernel/` symlink and write through it | deny |
| S-M9 | hard-link `kernel/CONSTITUTION.md` into `work/` | deny |
| S-M10–13 | write in `kernel/`, `bin/`, `lineage/`, `INIT_HOME/` | deny |
| S-M14–15 | `rmdir body/`; `mv work work2` | deny |
| S-M16–17 | write under `/private/tmp`, the owner's real home (policy queries on existing paths; S-M17r is a real write in the test dir) | deny |
| S-M18–21 | read `kernel/`, `bin/`, `lineage/`, `state/kernel/` | allow |
| S-M22–25 | read, list, `stat` in `vault/`; read `cache/` | deny |
| S-M26–28 | list `~/.ssh`, `~/Documents`, `/opt/homebrew/var` | deny (v0.1.0, probes and `read_home = false`. From v0.1.1 the live body's and members' `~/Documents` is readable by default, S-A1 and S-A18; `~/.ssh` stays denied, S-A2) |
| S-M29–30 | connect `run/kernel.sock`, `run/proxy.sock` | allow |
| S-M31–32 | connect the Root's `ROOT_HOME/run/owner.sock`; connect `ROOT_HOME/run/egress.sock` (v0.3: replaced the `run/owner.sock` checks) | deny |
| S-M32b | connect the owner socket through a symlink created in `work/` | deny |
| S-M33 | unlink `run/kernel.sock` | deny |
| S-M34 | connect `/private/var/run/mDNSResponder` | deny |
| S-M35–38 | TCP connect to the runner's own `127.0.0.1` listener (was `:22`); TCP bind loopback; UDP send to the runner's own `127.0.0.1` socket (was `8.8.8.8:53`); DNS lookup (skipped on the owner's machine, see S-M34) | deny |
| S-M39 | bind a Unix socket in `work/` | allow |
| S-M39b–h | bind each kernel-owned path while it is absent: `run/kernel.sock`, `run/proxy.sock`, `run/spawn.sock`, `run/fleet/b2/{kernel,proxy}.sock`, `probe/p2/run/{kernel,proxy}.sock` (on an existing file the bind fails with `EADDRINUSE` before the sandbox decides; S-M33 shows a body cannot unlink the kernel's socket) | deny |
| S-M40–41 | `kill -0` on a process outside the sandbox that the suite did not spawn; on pid 1 (**disposable environment only**) | deny |
| S-M40b | `kill -0` on an unsandboxed process the suite spawned, by its PID (the live-machine variant of S-M40) | deny |
| S-M42 | nested `sandbox-exec` | deny |
| S-M43–45 | exec `git`, `cargo`, `initctl` | allow |
| S-M46 | `cargo build` of a crate with a proc-macro dependency in `work/` (on-machine: a local proc-macro crate) | allow (tested) |
| S-M47 (×2) | **disposable environment only.** `posix_spawn(POSIX_SPAWN_SETSID)` child (stdio on `/dev/null`), then the Root's sweep, once with a sibling decoy and once with the Root canary as the exclusion path | membership = exactly that child each time (mark allowed, exclusion denied); child killed. Last run 2026-10-20 on the live machine (31 foreign processes excluded), before the rule moved it to disposable environments |
| S-M48 | list `/dev/fd` in the body | only 0, 1, 2 |
| S-M0 | every profile parses: `sandbox-exec -p <text> -D… /usr/bin/true` exits 0 | allow |
| S-M49a–c | probe parameters (D8 layout: real directories, no symlinks): read `<probe root>/kernel/CONSTITUTION.md` and `<probe root>/bin/ext/<name>.wasm` in the clones, and the real `lineage/x` through `origin` | allow (tested 2026-10-20 with the earlier symlink layout; the clone layout uses only the `subpath` rules S-M18–S-M21 exercise; re-run at M2) |
| S-M49d–f | probe: write into `<probe root>/kernel/`; write the live `body/`; read the live `state/` | deny (tested with the link layout; re-run at M2) |
| S-M49g–h | probe: connect its own lane `kernel.sock`; connect the live `run/kernel.sock` | allow / deny (tested) |
| S-M49i | probe: connect the Root owner socket | deny (tested) |
| S-M49j | probe: bind in its own lane directory `<probe root>/run/` (kernel-owned) | deny (tested) |
| S-M50a–b | live body: list `$INIT_HOME/probe/`; read a task file inside a probe root | deny (tested) |
| S-M51 | `kill -TERM` a child the body's bash started | allow (tested; body/tools.md Q-T2, `(target same-sandbox)`) |
| S-M52 | exec `/bin/kill` (macOS has no `/usr/bin/kill`; Linux S-L14 covers it); create a file under `state/spill/<sid>/` | allow (tested) |
| S-M53–54 | read a file in `ROOT_HOME`; list `ROOT_HOME` | deny (tested) |
| S-M55 | live body: list `$INIT_HOME/fleet/` (other bodies' roots) | deny (tested) |
| S-M56 | list `/opt/homebrew/bin` (L0 right, ESC-2) | allow (tested) |
| S-M58–59 | read `INIT_HOME/root/ledger.json` (published view, `PUB`); write into `INIT_HOME/root/` | allow / deny (tested) |
| S-M60 | read another instance's mark under `run/mark/` | deny (tested; tail Z5) |
| S-M61 | read the Root canary `ROOT_HOME/run/canary` | deny (tested; needed by the sweep's exclusion rule) |
| S-M57a–f | member parameters (§3.3.5; lane sockets under `run/fleet/<bid>/`): write its own `work/` (allow); write the live `body/` (deny); write its own `state/kernel/` (deny); connect its own lane `kernel.sock` (allow); connect the live `run/kernel.sock` (deny); read `kernel/` through its link (allow) | as listed (tested) |
| S-M57g | member: bind in its own lane directory `run/fleet/b1/` (kernel-owned) | deny (tested) |
**ADR-009 and ADR-012 access rows (v0.1.1 and v0.3.1, S-A1–S-A28; `root/sandbox-profiles/src/selftest.rs` `access_checks`, SBX-009).** Each mode's composed profile must also parse (S-M0.read_home, S-M0.workspace, S-M0.full). The rows run against a fake home that the test builds in its scratch tree, with `Documents/`, `.ssh/id`, `.config/init-agent/`, a charter `read_deny` directory, a workspace `ws/` and a stand-in service directory `svc/`. The profile parameters take the home as a value, so the test passes the fake one and never touches the owner's real `~/.ssh` or any other real file; S-A6b, S-A9f and S-A16 only list or policy-query the real home. In full mode (`Z_FULL` = `/`) every write row targets a path the run created: the fake home or S-A27's temp directory.

| ID | Mode / instance | Operation | Expect |
|---|---|---|---|
| S-A1, S-A1b | live body, `read_home` | read `~/Documents/x`; list `~/Documents` | allow |
| S-A2, S-A2b–d | `read_home` | read the fake `~/.ssh/id`; list `~/.ssh`; read a file in `~/.config/init-agent/`; read a charter `read_deny` path | deny (Z2) |
| S-A3 | `read_home` | write `~/Documents` (no write grant) | deny |
| S-A4, S-A5, S-A5b–c | `read_home` | read `ROOT_HOME`; read `vault/`, `cache/`, a probe root's task (`INIT_HOME` keeps its own rules) | deny |
| S-A6, S-A6b | `read_home` | read the Root's service file (Z2); list the real home's `~/.ssh`, outside the fake home | deny |
| S-A7, S-A7b–c | `workspace` | write a new file in the workspace; overwrite a file deep in it; read `~/Documents/x` | allow |
| S-A8, S-A8b–c | `workspace` | write elsewhere in the home; write `~/Documents`; write in the home itself | deny |
| S-A9, S-A9b, S-A9d | `workspace` | write `ROOT_HOME` (Z1), `kernel/` (Z4), `vault/` | deny |
| S-A9c | `workspace` | write `work/` (the SECTION's own rule) | allow |
| S-A9e–f | `workspace` | policy query: write under `/private/tmp`; write the real home | deny |
| S-A10, S-A10b–c | `full` | write elsewhere in the home; write `~/Documents`; write the workspace | allow |
| S-A11, S-A11b | `full` | write and read `ROOT_HOME` (Z1) | deny |
| S-A11c | `full` | list `ROOT_HOME`'s keys (Z1) | deny |
| S-A12, S-A12b | `full` | write the fake `~/.ssh`; read `~/.ssh/id` (ADR-012: the secrets list is workspace mode's) | allow |
| S-A12c–f | `full` | write and read `~/.config/init-agent/`; write and read a `read_deny` path (Z2, the full lists) | deny |
| S-A13, S-A13b–f | `full` | write `kernel/` (Z4), `vault/`, `state/kernel/`, `run/mark/` (Z4), `lineage/`; read `vault/` (the grants exclude `INIT_HOME`) | deny |
| S-A14, S-A14b–c | `full` | add a file next to the Root's service file; overwrite it; read it (Z2, `Z_ROOT_SVC_DIR`) | deny |
| S-A15, S-A15c | `full` | connect the owner socket; connect the egress socket (both in `ROOT_HOME`, Z1) | deny |
| S-A15b, S-A15d–e | `full` | TCP connect to the suite's own `127.0.0.1` listener; bind a loopback TCP listener; UDP send to the suite's own socket (ADR-012) | allow |
| S-A16, S-A16b | `full` | policy query: write the real home; write under `/private/tmp` (ADR-012) | allow |
| S-A16c | `full` | run a program copied into the fake home (map-executable outside `INIT_HOME`) | allow |
| S-A17 | `full` | rename the fake `~/.ssh` away (ADR-012; the suite renames it back) | allow |
| S-A17b | `full` | rename the service directory (Z2) | deny |
| S-A18, S-A18b–d | fleet member, full access on for the live body | read `~/Documents/x` (read grant); write the workspace; write elsewhere in the home; TCP connect to the suite's listener (never full) | allow; deny |
| S-A18e, S-A19d | fleet member; probe | `grant_for` never gives them full access | pass |
| S-A19, S-A19b–c | probe, full access on for the live body | read `~/Documents/x`; write the workspace; TCP connect (no grant: hermetic) | deny |
| S-A20–S-A20d | `workspace` | add `.git/hooks/pre-commit`; write `.git/config`; a submodule's hook and config under `.git/modules/` (Z3b) | deny |
| S-A20e–g | `workspace` | write a git object, `.git/HEAD`, `.git/config.lock` (the rest of `.git` stays writable; the rename onto `config` is denied) | allow |
| S-A20h | `workspace` | write `~/.zshrc` (Z3, Z3a) | deny |
| S-A20i | `workspace` | write `.git/config` of a repository inside `INIT_HOME` (the kernel's clones) | allow |
| S-A21–S-A21e | `full` | write `~/.zshrc`, `~/.zshenv`, `~/.bash_profile`, `~/.profile`, `~/.config/fish/` (ADR-012: deferred execution is a residual) | allow |
| S-A22, S-A22b | `full` | write `~/.gitconfig`, `~/.config/git/` (ADR-012) | allow |
| S-A23, S-A23a, S-A23b | `full` | replace `<bin_dir>/init-agent`, `<bin_dir>/init` or `<bin_dir>/init-tui` (Z3a, INIT's launchers) | deny |
| S-A23c–d | `full` | add a command to `~/.local/bin` and to `~/bin` (ADR-012) | allow |
| S-A24 | `full` | add a login item in `~/Library/LaunchAgents` (Z3a, the service directory) | deny |
| S-A24b | `full` | add an entry in `~/.config/autostart` (ADR-012) | allow |
| S-A25–S-A25d | `full` | another repository's hook and `.git/config`; a hook and a git object in the workspace (ADR-012) | allow |
| S-A26, S-A26b | `full` | rename `~/.local/bin` away; rename the workspace's `.git/hooks` away (ADR-012; renamed back) | allow |
| S-A27 | `full` | write a new directory the suite created in the per-user temp directory (`/var/folders/…`, outside the home and `/tmp`; removed by its exact path) | allow |
| S-A28 | `full` | `kill -0` an unsandboxed `sleep` the suite spawned (the Root stand-in; Z7) | deny |

Last run (SBX-009, cc16577): the whole self-test, 330 pass, 0 fail, 13 skipped (11 disposable-environment rows and S-M38, 2 under `--no-builds`).

**macOS `build` profile:** S-B1 write `OUT` allow; S-B2 write `SRC` deny; S-B3 read live `body/` deny; S-B4 Unix sockets under `OUT` only, no IP (§3.4 section 6): S-B4a one build instance serves `OUT/tmp/a.sock` (bind, accept) and a second connects (allow), S-B4a2 bind `OUT/home/b.sock` (allow); deny: S-B4b connect the live `run/kernel.sock`, S-B4c TCP connect to the runner's own `127.0.0.1` listener, S-B4d bind in `SRC`, S-B4e bind `slot/outb` (`OUT`'s path is a string prefix of it), S-B4f connect a live listener at `slot/outl`, S-B4g connect the Root's `egress.sock` (the tail leaves it open; this SECTION does not), S-B4h connect `owner.sock`, S-B4i connect a live listener outside `INIT_HOME` (stands for `docker.sock` or another `~/.init*`), S-B4j connect `run/kernel.sock` through a symlink created in `OUT`, S-B4k UDP send to `127.0.0.1`, S-B4l TCP bind on loopback, S-B4m TCP connect to the runner's own `[::1]` listener; `check` shares the text: S-B4ac bind in its `OUT` (allow), S-B4bc connect `run/kernel.sock` (deny); S-B5 read `vault/` deny; S-B6 `cargo build --frozen` in `SRC` succeeds; S-B7 read `ROOT_HOME` deny (all tested 2026-10-20). The `kernel` profile's section 6a (root/root.md §3.3.3): S-Z8p a kernel instance serves `cache/build/slot/out/tmp/p.sock` and a second connects (allow); deny: S-Z8q bind in `slot/src` (writable to the kernel), S-Z8r connect `slot/outl`, S-Z8s connect a listener outside `INIT_HOME`, S-Z8t connect `owner.sock` through a symlink in `slot/out`.

**macOS `git-read` profile:** S-G1 fetch from `body/` succeeds; S-G2 alternates into `vault/` → the fetch fails and the object is absent from lineage; S-G3 any write denied; S-G4 read `ROOT_HOME` deny (all tested 2026-10-20).

**macOS `kernel` profile** (root/root.md §3.3.3, composed with ROOT_ALLOWS and the tail; tested 2026-10-20 as the `init-kernel` instance). These rows check the kernel's requirements of §3.11 against root/root.md's text and complement root/root.md's RT-1 and RT-2. The bind rules stay `subpath RUN` and `subpath PROBE` (root/root.md §3.3.3, drift-tested), because the kernel also binds `run/spawn.sock` and the judge relay at `probe/<12 hex>/p.sock`, which a literal-only rule would break (FIX1 D2; narrowing needs a root.md change first). S-Z8m–o and the body-side S-M39b–h, S-M49j and S-M57g pin what stays denied:

| ID | Operation (from inside the kernel instance) | Expect |
|---|---|---|
| S-Z1, S-Z2a–c | write `state/kernel/`, `bin/`, `lineage/`, `cache/`, `vault/`, `probe/`, `fleet/`, `run/` | allow |
| S-Z3a–e | write `kernel/`; rename `kernel/`; rename `INIT_HOME`; write `root/`; write `run/mark/` | deny |
| S-Z4 | read `ROOT_HOME` | deny |
| S-Z5 | connect the Root owner socket | deny |
| S-Z6 | connect the Root egress socket | allow |
| S-Z7a–c | TCP connect to the runner's own `127.0.0.1` listener; UDP send to the runner's own `127.0.0.1` socket; connect the resolver socket (the DNS path) | deny |
| S-Z8a | bind a Unix socket under `run/` | allow |
| S-Z8b | a body instance (separate sandbox) connects to a socket the kernel bound at its `KERNEL_SOCK`; the kernel accepts | allow |
| S-Z8c–d | bind under `run/fleet/b1/`; bind under `fleet/b1/run/` | allow / deny (why member lane sockets live under `run/fleet/`) |
| S-Z8e | the kernel serves `run/kernel.sock` with its lane sequence (mkdir; chmod the directory 0700 only if its mode differs; unlink a stale socket by its exact path; bind; chmod the socket 0600) over a stale socket file; a body instance connects and its line arrives | allow |
| S-Z8f | chmod `run/` itself, even to its current 0700 (tail Z4 denies it; this was the S1 gap-3 `EPERM`) | deny |
| S-Z8g–l | the same lane sequence on `run/proxy.sock`, `run/spawn.sock`, `run/fleet/b2/{kernel,proxy}.sock` (directory created), `probe/p2/run/{kernel,proxy}.sock` (directory chmod-ed) | allow |
| S-Z8m–o | bind in `run/mark/` (Z4); bind in `work/` (not under `RUN` or `PROBE`); bind outside `INIT_HOME` (Z6) | deny |
| S-Z9 | git (hardened prefix) writes an object into `lineage/body.git` | allow |
| S-Z10 | `sandbox_check` with the **path filter** on a body instance's pid: `sandboxed=1`, own mark allowed, the Root canary denied (initctl peer attribution from inside the kernel) | allow, exact values |
| S-Z11 | `kill -0` on a body instance | deny (kills go through the Root) |
| S-Z12 | nested `sandbox-exec`, even `(allow default)` | deny (exit 71) |
| S-Z13a–b | list `~/Documents`; write outside `INIT_HOME` | deny |
| S-Z14a–b | list `/opt/homebrew/bin`; list `/opt/homebrew/var` | allow / deny |
| S-Z15 | exec `cargo`, `tar` | allow |
| S-Z16 | read `INIT_HOME/root/ledger.json` | allow |

**Linux** (same operations; expected errno differs):

| ID | Operation | Expect |
|---|---|---|
| S-L1 | write `kernel/`, `bin/`, `lineage/`, `state/kernel/` | `EROFS` |
| S-L2 | read `vault/`, `cache/`, `ROOT_HOME`, the Root's sockets | `ENOENT` (never mounted into a body instance) |
| S-L3 | `socket(AF_INET, …)`, `socket(AF_INET6, …)`, `socket(AF_NETLINK, …)` | `EPERM` (seccomp) |
| S-L4 | `io_uring_setup` | `EPERM` |
| S-L5 | `ptrace(PTRACE_ATTACH, <kernel pid>)` | `EPERM` (and the pid is not visible) |
| S-L6 | `unshare(CLONE_NEWUSER)`; nested bwrap | `EPERM` (instance seccomp, and the Root's `--disable-userns`) |
| S-L7 | connect `run/kernel.sock`, `run/proxy.sock` (read-only bind mounts) | allow |
| S-L8 | ABI ≥ 9: connect a pathname socket created outside under any other path (test bind-mounts one under `work/`) | `EACCES` (Landlock) |
| S-L9 | ABI ≥ 6: connect an abstract socket of the host | fails (network namespace; Landlock scope) |
| S-L10 | hard link from `kernel/` into `work/` | `EXDEV` (different mounts) |
| S-L11 | `ioctl(TIOCSTI)` on fd 0 | `EPERM` |
| S-L12 | kill bwrap → every process in the namespace is gone within 100 ms | yes |
| S-L13 | `/proc/self/status` shows `NoNewPrivs: 1`, `Seccomp: 2`, `CapEff: 0` | yes |
| S-L14 | `git`, `cargo build`, `initctl ping` | allow |
| S-L15 | the Root-rendered argv (zone arguments + §3.6.1 mounts) starts a body whose `sandbox-inner` applies Landlock and seccomp | allow |
| S-L16 | from the kernel instance: read `ROOT_HOME`; `socket(AF_INET)`; connect the egress socket | `ENOENT`; `EPERM`; allow |
| S-L17 | the Root kills a body instance's bwrap → every process in its namespace is gone within 100 ms | yes |
| S-L18 | inside a body instance: `unshare(CLONE_NEWUSER)` | `EPERM` |
| S-L19 | `SO_PEERCRED` on a lane socket for a peer in a sibling pid namespace | pid reads 0 (so attribution is by socket path, §3.13) |

The Linux rows were not run: there is no Linux host here. They wait for the CI decision (ET-E4).

**Cross-cutting:** S-X1: grep the sandbox's environment and every readable file for the configured API key strings → no match (A6, invariant 3). S-X2: a promote whose `build.rs` tries to write `../../deps`, open a socket and read `vault/` → all denied, gate continues (supervise.md SV-21). S-X3 (root/sealed-eval.md M7): from a live body and from a probe, read `vault/holdout/`, `vault/canaries/` and the Root's sealed store under `ROOT_HOME/sealed/` → all refused by the OS (`EPERM` on macOS, `ENOENT` on Linux). That is S-M22–S-M24 and S-M53–S-M54 applied to M7's paths.

**testing.md T-S5…T-S8 map onto this suite** (testing.md §3.7 asked for sandbox IDs):

| testing.md | Here |
|---|---|
| T-S5: read, write or list the Root's private directory and key store | S-M53, S-M54 (`ROOT_HOME`; the key store lives under it) |
| T-S6: append to the readable R4 lineage view | S-M59 (write into `INIT_HOME/root/`) |
| T-S7: connect to the Root's owner socket and egress socket | S-M31, S-M32, S-M32b |
| T-S8: `kill -0 <Root pid>` | S-M40 variant with the Root's pid: in a disposable environment, or with a test Root the suite spawned itself |

**testing.md SW-1…SW-6 (sweep safety, D1) join the S-Z group as S-Z17a–f, disposable environment only** (testing.md's runner guard, SW-6, refuses them on the owner's account):

| ID | testing.md | Operation | Expect |
|---|---|---|---|
| S-Z17a | SW-1 | the Root's sweep over a zone instance and its `setsid` escapee (as S-M47) | both killed |
| S-Z17b | SW-2 | an unsandboxed process of the same user that can read the mark | not killed (fails condition a) |
| S-Z17c | SW-3 | a decoy under its own permissive Seatbelt profile that can read the mark and the canary | not killed (fails condition c) |
| S-Z17d | SW-4 | a decoy whose profile denies the canary but cannot read the mark | not killed (fails condition b) |
| S-Z17e | SW-5 | a full panic and a full rollback with S-Z17b–d decoys running | every zone process gone; every decoy alive, checked by its held PID |
| S-Z17f | SW-6 | any S-Z17 row started from the owner's account | refused with `skipped: needs a disposable environment`; nothing started |

## 9. Open questions

Settled:
- rounds 1 and 2: `docs/reconciliation/kernel-supervise.md`;
- `/opt/homebrew` (ESC-2): by the owner on 2026-10-20;
- root/root.md (2026-10-20): the spawn broker, Root-compiled texts with a deny-only tail, marks under `INIT_HOME/run/mark/`, TLS and keys in the Root, the `PUB` read (its X2);
- the v0.3 decisions D1 (three-condition sweep; tests in disposable environments), D8 (no symlinks in probe roots) and D9 (`extra_read`); changes logged in `docs/reconciliation/v03-kernel-supervise.md`;
- root/egress.md's ask: bodies cannot connect to the egress socket (S-M32, S-M32b); only the `kernel` instance can (S-Z6);
- root/sealed-eval.md's ask: M7 is an OS refusal (S-X3), and sealed runs are Root-launched (§3.9, D5).

**Own:**

1. Allow loopback TCP inside the Linux network namespace (safe there) for test suites that need it? Current answer: no, for parity with macOS, where loopback reaches host services.
2. CI runner hardware for §8 (macOS 14/15/26, Linux ≥ 7.1): PENDING OWNER (CONVENTIONS §6 ET-E4). The disposable-environment half of §8 (S-M40, S-M41, S-M47, S-L12, S-L17) needs such a runner or a VM; the Linux rows are unrun until then.
3. The probe and member clone layout (D8) is to be re-run at M2 (S-M49). The SBPL text did not change, so no Root-text re-test is due.

**Cross-doc follow-ups:**

4. **root/root.md:** apply D1's third condition in §3.5.4, and D9 in tail Z2. This doc's §3.11 and §8 assume both.
5. **testing.md:** adopt the T-S5…T-S8 and SW-1…SW-6 (S-Z17a–f) mappings of §8; S-M40, S-M41, S-M47 and S-Z17 move to its disposable-environment CI job.

## 10. Spec deltas

| Spec v0.2 / v0.3 | This doc | Why |
|---|---|---|
| v0.3 §13 "AGENT ZONE (sandboxed; network = Root egress socket only)" with the body zone nested inside | §3.11: the Root's zone tail, appended last to every profile, is the security boundary; this doc's instance profiles are a stability layer inside it. The kernel's requirements are checked against root/root.md's `kernel` profile (S-Z) | spec v0.3 §13; root/root.md §3.3 |
| v0.3 §13 body zone nested in the agent zone | nesting is **logical** on both platforms. The Root spawns every instance from Root-compiled texts (PRELUDE + SECTION + ROOT_ALLOWS + ZONE_TAIL; Linux: zone arguments + this doc's mounts) | Seatbelt refuses any nested sandbox, even under `(allow default)` (tested 2026-10-20); root/root.md chose one spawn path for both platforms |
| v0.2 §7.4 "Network: only the two kernel sockets" | bodies: unchanged. The kernel instance: the egress socket only (root/root.md §3.3.3) | spec v0.3 R2: keys and egress live in the Root |
| v0.2 hidden `run/owner.sock` | no owner socket under `INIT_HOME` any more. The `OWNER_SOCK` deny was removed from section 8; `ROOT_HOME` and its sockets are denied by the tail (Z1, Z6) | the owner channel moved to the Root (supervise.md §3.10) |
| (v0.2) no Root view | the body SECTION reads `INIT_HOME/root/` (`PUB`): published lineage, ledger, ceilings, charter | root/root.md §9.1 X2 |
| v0.1: the kernel renders and launches profiles | the Root renders (text compiled into `init-root`) and launches; the kernel sends a profile id and parameters (`spawn_args`, §4.1) | zone-authored SBPL could redefine `deny` (root/root.md, tested) |
| CONVENTIONS §6 ESC-2 (pending in v0.1) | `/opt/homebrew` readable, minus `var/` and `etc/`; no PENDING marker | settled as an L0 right (spec v0.3 §15, §19) |
| (v0.1) sweep membership by mark alone | mark allowed **and** an existing exclusion path denied (the Root canary or a sibling decoy); sweep and kill tests only in a disposable environment | D1 and CONVENTIONS §1, after the mark-only rule matched 31 foreign sandboxed processes and a live test killed 33 (2026-10-20, §8) |
| (not in spec) | fleet-member parameters (§3.3.5): one root per member, lane sockets under `run/fleet/<bid>/`; no body can read another's root | spec v0.3 §15 L2; evolution.md §3.15.5 |
| (v0.2) probe roots with symlinks and `LINK_*` literals | real directories (copy-on-write clones on macOS, bind mounts on Linux); `LINK_*` equal the real directories, and every parameter is canonical | D8: root/root.md's V2 accepts only canonical values |
| (v0.3 draft) `extra_read` unusable under tail Z2 | readable again when the charter allows it | D9 |
| (not in spec) | macOS initctl peer attribution stays in the kernel: `sandbox_check` with the path filter on another sandbox's pid works from inside the kernel instance (S-Z10). Linux: by socket path | keeps the Root smaller |
| §7.4 "`.git` metadata is re-protected read-only" | not done for `body/.git` | the agent must commit; authority moved to read-only `lineage/body.git` (supervise.md Decision 2) |
| §7.4 network "only the two kernel sockets" | plus Unix sockets bound by sandboxed processes under `work/` | local IPC among the agent's own processes (tmux, test servers); reaches no host |
| §7.4 "Hidden: `vault/`" | also hidden from bodies: `cache/`, `probe/` and `fleet/` (except the instance's own root), the Root's paths, and ADR-009's deny list of secrets. The owner's home is hidden in v0.1.0, from probes, and with `read_home = false`; from v0.1.1 the live body and fleet members read it | least privilege; secrets must not reach prompts; the owner chose Codex-style reads (OW-PERM) |
| §7.4 body writes `body/ work/ state/` | v0.1.1: the live body also writes the Root-validated workspace, or the whole home except the deny list under `--full` (§3.1.1) | owner decision OW-PERM, ADR-009 |
| §3 "Landlock+seccomp+bwrap on Linux" | bwrap primary, Landlock second layer, seccomp third | Codex found Landlock alone cannot isolate Unix sockets (before ABI 9) |
| (not in spec) | every gate activity (fetch, build, probe, task run, check) runs in its own sandbox instance | invariant 2 needs it in practice; v0.3 keeps it as the body-facing layer below R6 |

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.9):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP6a D2 | Both `LINK_*` forms are accepted (D8): a real canonical directory, or a symlink whose parent is canonical. A symlink is accepted only inside a probe or member root; the live root's `LINK_*` must equal its target. | §3.3.3, §3.11 item 2, §3.12 |
| build: WP6a D3 | The class rules use exact values (`BODY` = `<root>/body`, and so on). `KERNEL` and `BIN` of a body may be `INIT_HOME`'s or the instance root's. | §3.12 |
| build: WP6a D5, D6, D8 | On-machine targets (macOS 14.6): IP rows use the runner's own loopback listener and UDP socket; S-M38 is skipped; S-M16 and S-M17 are policy queries; S-M46 uses a local proc-macro crate; S-Z7c connects the resolver socket. | §8 |
| build: FIX1 D2, D3 | Bind rules stay `subpath RUN` and `subpath PROBE` (the kernel also binds `run/spawn.sock` and the judge relay). New rows S-Z8e–o, S-M39b–h, S-M49j and S-M57g. | §8 |
| build: FIX1 N | The body cannot `stat` `$INIT_KERNEL_SOCK` (`file-read-metadata` on `RUN` only). Probe the socket by connecting. | §3.3.4 |

## 11. References

Checked 2026-10-19 unless marked; v0.3 additions checked 2026-10-20.

- Local experiments on this machine (macOS 14.6, M2, 2026-10-20):
  - nested `sandbox-exec` refused (`sandbox_apply: Operation not permitted`, exit 71) even under `(allow default)`;
  - `sandbox_check` on a process in another sandbox works from inside a sandbox whose `process-info*` is restricted to `same-sandbox`;
  - the full v0.3 §8 suite against root/root.md-composed texts, 123/123 (and 113/113 earlier against the uncomposed texts);
  - sweep membership: mark only (31 foreign matches, which were killed) versus mark plus decoy, and mark plus Root canary (both exact).
  
  Evidence for the foreign kills: `log show` entries "exited due to SIGKILL | sent by sweep[13749]" for 33 per-user services, including `com.apple.Virtualization.VirtualMachine` of `com.docker.virtualization`.
- root/root.md (2026-10-20): §3.3 (composition, zone tail, `kernel` profile), §3.4 (Linux), §3.5 (spawn broker), §4.1 (control link), §9.1 (X2, X3).
- unix(7) `SCM_RIGHTS` — https://man7.org/linux/man-pages/man7/unix.7.html ; Rust `std::os::unix::net::SocketAncillary` (unstable, `unix_socket_ancillary_data`; UNVERIFIED that it is still unstable on 2026-10-20) — https://doc.rust-lang.org/std/os/unix/net/struct.SocketAncillary.html
- Landlock "Current limitations: filesystem topology modification" (a thread with filesystem restrictions cannot `mount`/`pivot_root`) — https://docs.kernel.org/userspace-api/landlock.html (recalled, not re-fetched on 2026-10-20: UNVERIFIED)

- Codex at `bb8f5304fcd9f901d8e0747fccb8c8904cbc504e`:
  - `codex-rs/sandboxing/src/seatbelt_base_policy.sbpl` — https://github.com/openai/codex/blob/bb8f5304fcd9f901d8e0747fccb8c8904cbc504e/codex-rs/sandboxing/src/seatbelt_base_policy.sbpl
  - `seatbelt_network_policy.sbpl`, `seatbelt_read_only_platform_defaults.sbpl`, `seatbelt.rs` (`build_seatbelt_access_policy`, `unix_socket_policy`, `MACOS_PATH_TO_SEATBELT_EXECUTABLE`), `seatbelt_daemon.rs` — same directory
  - `codex-rs/linux-sandbox/README.md`, `src/bwrap.rs`, `src/seccomp.rs` — https://github.com/openai/codex/tree/bb8f5304fcd9f901d8e0747fccb8c8904cbc504e/codex-rs/linux-sandbox
  - move to `codex-sandboxing`: commit 431af0807c (#15593)
- Codex docs (redirects from developers.openai.com/codex/… to learn.chatgpt.com/docs/…): sandboxing, agent-approvals-security, permissions. Read through a summarizing fetcher; wording paraphrased.
- Claude Code sandboxing (Seatbelt on macOS) — https://code.claude.com/docs/en/sandboxing
- `sandbox-exec(1)`, `sandbox_init(3)` (DEPRECATED), macOS 14.6 local man pages; `sandbox.h` (macOS 14.6 SDK)
- bubblewrap `bwrap.xml` (option semantics; `--new-session` and CVE-2017-5226) — https://github.com/containers/bubblewrap/blob/main/bwrap.xml ; releases (v0.13.1, 2026-10-14; setuid removed in 0.12.0) — https://github.com/containers/bubblewrap/releases
- Landlock userspace API (ABI table, `RESOLVE_UNIX`, inheritance) — https://docs.kernel.org/userspace-api/landlock.html
- `landlock` crate 0.4.7 — https://docs.rs/landlock/0.4.7/landlock/ ; `seccompiler` 0.5.0 — https://docs.rs/seccompiler/0.5.0/seccompiler/
- Ubuntu unprivileged userns restriction — https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces ; Ubuntu 24.04 release notes — https://discourse.ubuntu.com/t/ubuntu-24-04-lts-noble-numbat-release-notes/39890
- Anthropic, "Claude Code sandboxing" (filesystem and network isolation, 84% fewer prompts) — https://www.anthropic.com/engineering/claude-code-sandboxing
- Local experiments on this machine (macOS 14.6, M2, 2026-10-19): the §8 macOS suite; `log show` denial capture for the sysctl list; `sandbox_check` mark identification; sandboxed upload-pack; spawn timing.
