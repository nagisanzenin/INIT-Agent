# Platforms: Linux, macOS and Windows

```
Status: draft v0.1 · 2026-10-20 · Area: root (platform layer) · kernel (inner stage, IPC client) · release (lane DES-PLAT)
Depends on: V1_PLAN.md (mandate 4), CONVENTIONS.md (§1 live-system safety, §3, §4, §5), adr/008-owner-machine-safety-v0.1.md,
            adr/009-codex-style-permissions.md, root/root.md (§3.1, §3.2, §3.4, §3.5, §3.9, §3.13, §3.17, §4.1–§4.3, §4.8),
            kernel/sandbox.md (§3.6, §3.7, §3.8, §3.13, §8), kernel/supervise.md (§3.6.2, §4.5), root/egress.md (§3.2),
            docs/ux-v1.md (§3.2 bundle and installers, §3.4.5 key storage, §3.4.8 service, §3.5 warm-up, §3.6.5 checkpoints,
            §3.11), release.md, testing.md (§3.1a), research_notes/INIT v1 providers UX vision/cross_platform.md
Defines:    the platform layer (seams ConfinedLaunch, HeldKill, LocalIpc + HandlePass, Snapshot, Service, plus Keystore and
            Paths) and where each lives; the macOS mapping onto it; the release tiers and the guarantees of each tier; the
            platform autonomy cap rule; the Linux zone additions (fd-pinned binds, the split-bind home plan for ADR-009,
            the zone seccomp namespace denies, args and info fds); the Linux and Windows deny-list entries; the
            per-OS snapshot object paths and restore rules under ux-v1.md's checkpoint store; the keystore backends; the
            Windows native design (MXC, Job Objects, named pipes, handle duplication, Task Scheduler, Credential Manager)
            and the MXC spike; macOS signing and minimum version; the build matrix, CI workflows and the Actions minutes
            budget; the per-target reproducibility expectations; work packages PLAT-*; test ids PL-*
Proposes:   (cross-doc, §9) changes to root/root.md §3.4, §3.13.1, §3.17.1, §4.1, §4.8; kernel/sandbox.md §3.8;
            kernel/supervise.md §3.6.2; root/egress.md §3.2; ux-v1.md §3.2.1, §3.4.5, §3.6.5, §3.11; body/provider.md and
            body/tools.md (Windows native only)
```

---

## 1. Purpose

The owner requires releases on Linux, macOS and Windows (V1_PLAN.md item 4, 21:55). INIT's security model is the same on every OS: an immutable Root outside every sandbox, one confined agent zone, provider-only egress through the Root, and signals only to processes the Root holds (spec v0.3 §13, ADR-007, ADR-008). Today only macOS implements it. The Linux paths in root/root.md §3.4 and kernel/sandbox.md §3.6 are written but not built (`zone.rs` returns "the Linux zone … is not built yet (WP6b)"), and Windows has no design.

This doc makes the three platforms buildable:
- a **platform layer** with the research's five seams (confined launch, held-process kill, local IPC with handle passing, snapshot, service install) plus a keystore and path conventions, onto which today's macOS code maps with no behaviour change (§3.2, §3.3);
- the **Linux** design: bubblewrap with Landlock and seccomp, the empty network namespace, the user-namespace preflight, systemd, snapshots, Secret Service, minimum versions (§3.4);
- the **Windows** tiering and the native design behind it (§3.5);
- **macOS** signing, notarization and the minimum version (§3.6);
- **build, CI and testing** without a Windows machine and inside GitHub's free minutes (§3.7);
- the **prebuilt body** and what reproducibility can be expected on each target (§3.8);
- the **work breakdown** (§3.9).

## 2. Responsibilities / non-goals

**Responsibilities:**
- the platform traits, their backends per OS, and how a backend is chosen;
- the tiers, the guarantee table and the platform cap rule;
- the Linux additions to the zone that ADR-009 and the Linux boundary need, proposed into root/root.md §3.4 and kernel/sandbox.md §3.6;
- the Windows native design and the MXC spike;
- the per-OS implementations under contracts other docs own: the snapshot object paths under ux-v1.md §3.6.5's store, the keystore backends under root/egress.md §3.2 and ux-v1.md §3.4.5, the service files under root/root.md §4.3 step 9 and ux-v1.md §3.4.8;
- the build matrix, the CI workflows, the minutes budget and its guard;
- macOS code signing and notarization procedure;
- per-target reproducibility expectations and the build-environment normalization they need;
- work packages PLAT-*, tests PL-*.

**Non-goals (owned elsewhere, used here by name):**
- The bundle layout, `MANIFEST.json` and its signature, the installers' behaviour, the app directory, the launcher, the first run, the warm-up states and copy: ux-v1.md. This doc adds only the per-OS deltas (§3.5.2, §3.7).
- The checkpoint store's format, retention and the `/undo` and `/rewind` protocol: ux-v1.md §3.6.5 and root/root.md §3.17.4.
- Profile texts and their parameters: kernel/sandbox.md and root/root.md §3.3. This doc proposes the Linux and Windows renderings.
- Key-storage policy (which store is the default): root/egress.md §3.2. This doc proposes per-OS defaults and builds the backends.
- Provider adapters, the TUI, the model catalog.
- Code signing on Windows (no affordable path for an individual outside the US and Canada; cross_platform.md §5). Native Windows is v1.x anyway (§3.5.1).

## 3. Design

### 3.1 Tiers at a glance

| Tier id | Platform | Release | Zone confinement | Provider-only egress | Platform cap |
|---|---|---|---|---|---|
| `macos` | macOS 14+ on Apple Silicon; Intel best effort (§3.6.1) | v1.0 | Seatbelt, Root-composed (today's code) | enforced | charter's (L4) |
| `linux` | Linux x86_64 and aarch64, kernel ≥ 5.10, glibc ≥ 2.31, bwrap ≥ 0.8.0 with user namespaces (§3.4.1) | v1.0 | bwrap namespaces + seccomp, Landlock best effort | enforced: empty network namespace, only `egress.sock` bound in | charter's (L4) |
| `wsl2` | Windows 10 22H2 or 11, a WSL 2 distro meeting `linux` | v1.0 | the `linux` tier inside the WSL VM | enforced, as `linux` | charter's (L4) |
| `win-mxc` | native Windows 11 24H2 (build 26100.9278+) or later | v1.x, after spike PLAT-N0 passes | MXC process container (AppContainer) + a Root-owned Job Object | enforced only if the spike shows it (§3.5.4) | L0 until the S-N suite is green on the build, then the charter's |
| `win-elevated` | Windows 10 1809+ with a one-time admin setup (Codex's elevated design) | not planned for v1; v2 at the earliest | restricted token, dedicated sandbox users, ACLs | WFP filters per sandbox user | decided with its own suite |
| `win-none` | native Windows without MXC and without admin | **refused** | a restricted token cannot deny reads, so the keys and `ROOT_HOME` are readable | not enforceable | the zone is never started |

**Recommendation for v1.0:** ship `macos`, `linux` and `wsl2`. On Windows, `install.ps1` installs the Linux build into WSL 2 (§3.5.2), which gives Windows users the full security model with no new sandbox code. Native Windows follows in v1.x behind the MXC spike, and never in a tier that cannot protect the Root's keys.

### 3.2 The platform layer

INIT keeps one Root-centred model and moves every OS-specific mechanism behind seven seams. Five are the research's (cross_platform.md §6); the keystore and the path conventions are the other two.

| Seam | What it hides | Lives in | macOS (today's code) | Linux | Windows native (v1.x) |
|---|---|---|---|---|---|
| **ConfinedLaunch** | rendering a profile and starting one instance under it | Root: `init-root/src/plat/`; pure renderers in `root/sandbox-profiles` | `zone.rs::launcher` (compose → `sandbox-exec` argv) and `zone.rs::launch` (exec-clean, own process group) | bwrap: zone args, profile mounts, fd-pinned binds, home plan, seccomp fd (§3.4.2) | MXC process container via `mxc-sdk` (§3.5.3) |
| **HeldKill** | signalling, waiting, reaping, CPU accounting, emptiness check | Root | `sys::{signal, held, wait_exit, reap}` (ADR-008 locks) | the same calls, plus a pid-namespace emptiness check | a Job Object per instance (`TerminateJobObject`) |
| **LocalIpc + HandlePass** | the control link, the owner and egress endpoints, the stdio hand-over | Root (server, sender); kernel (`kernel-supervise::sys`, receiver); kernel servers for `kernel.sock` and `proxy.sock` | socketpairs on fds 3 and 4, `SCM_RIGHTS`, `UnixListener` | unchanged (`MSG_CMSG_CLOEXEC` on receive) | named pipes; `DuplicateHandle` into the kernel plus a `handles` frame (§3.5.3) |
| **Snapshot** | putting one file into the checkpoint store and restoring a tree, race-safely | Root | `access.rs::{take, restore}` with `sys::clonefile` (whole-tree APFS clone, ADR-009) | `FICLONE` per file on btrfs, XFS and bcachefs, else copy (§3.4.8) | copy; ReFS block clone later |
| **Service** | the login service file, its render, its load and unload commands | Root (render, write); launcher (load, with consent: ux-v1.md §3.4.8) | `install.rs::service_file` (LaunchAgent plist) | systemd user unit; `manual` fallback (§3.4.7) | Task Scheduler logon task (§3.5.3) |
| **Keystore** | where a provider key or token lives | Root (egress) | 0600 files (R-F4) | Secret Service via `secret-tool`, else 0600 file (§3.4.9) | Credential Manager, else DPAPI file (§3.5.3) |
| **Paths** | the homes, the endpoint names, the service path, the deny lists, case rules | Root, kernel, launcher | `install::Paths`, `access.rs` lists | §3.4.10 | §3.5.3 |

Rules for the layer:
- **Compile-time choice per OS, runtime choice within an OS.** `plat/mod.rs` declares the traits. `plat/macos.rs`, `plat/linux.rs` and `plat/windows.rs` each export one type per seam, selected by `cfg(target_os)`. There are no trait objects in the hot path. Where an OS has alternatives (Linux snapshots, keystore, service; Windows keystore), the backend is chosen at install, written to `install.json.platform` (§4.2) and re-probed by `doctor`.
- **Renderers are pure and compile on every host.** The bwrap argv, the zone seccomp rules, the MXC policy JSON, the systemd unit, the plist and the task XML are functions from inputs to text in `root/sandbox-profiles` with no OS calls. Their golden tests therefore run in `ci/local.sh` on the M2 as well as on the hosted runners, so a Linux or Windows render cannot drift unnoticed between hosted runs.
- **`unsafe` stays in one place per crate.** `init-root/src/sys.rs` becomes `sys/{mod.rs, unix.rs, macos.rs, linux.rs, windows.rs}`, the only module tree with `#[allow(unsafe_code)]` (root/root.md §4.8). tests/rsafe.rs (ADR-008 lock 4) is updated to the new paths in the same commit and gains the Windows calls `TerminateJobObject` and `TerminateProcess`.
- **The kernel needs only two seams.** It receives handles (`HandleRx`, today `kernel-supervise::sys::recv_fds`) and serves its lane endpoints. Its spawn seam already exists: `kernel-supervise::instance::Spawner` asks the Root and never spawns a zone process itself, so it is unchanged on every OS. On Linux the kernel also owns the inner stage `init-kernel sandbox-inner` (crate `kernel-sandbox`, kernel/sandbox.md §3.6.2).
- **The body never links platform code.** Its only platform contact is connecting to `INIT_KERNEL_SOCK` and `INIT_PROXY_SOCK`. On Unix that is `std::os::unix::net::UnixStream`, as today. On native Windows it opens a named pipe through `std::fs::OpenOptions` (§3.5.3), which needs no `unsafe` and no new dependency, so `#![forbid(unsafe_code)]` holds.

The trait sketches are in §4.1.

### 3.3 macOS mapping, with no behaviour change

PLAT-0 moves code; it does not change what the Root does on macOS.

| Today | After PLAT-0 | Proof that nothing changed |
|---|---|---|
| `zone.rs::launcher` (cfg macOS) | `plat::macos::Seatbelt::render` calling `init_sandbox_profiles::seatbelt::compose` | PL-A1: the composed SBPL text and the `sandbox-exec` argv for every profile id × {no grant, `read_home`, workspace, full} × {live, probe, member}, captured before the refactor, are byte-identical after it |
| `zone.rs::launch` (exec-clean, `process_group(0)`, mark creation and removal) | `Seatbelt::start` returning a `Held` | PL-A1, plus `tests/wp21.rs` and `tests/rsafe.rs` green |
| `sys::signal(pid, sig, group)` with its `held()` check | `plat::unix::UnixKill::signal(&Held, Stop)`, which still calls `sys::signal`, so the runtime check stays as defence in depth | PL-A2: rsafe audit green with the reviewed call sites renamed, not added |
| `sys::send_fds` / `recv_fds` on the fd channel | `plat::unix::FdChannel` implementing `HandleTx` (Root) and `HandleRx` (kernel) | the WP21/WP22 link tests; kernel `link.rs` tests |
| `access.rs::take` / `restore` with `sys::clonefile`, the 2 GiB and APFS skips | `plat::macos::ApfsTree`, the `Snapshot` backend for ADR-009's session snapshot | RR-36…RR-40 and TUI-R4 unchanged |
| `install.rs::service_file` and `service_path` | `plat::macos::LaunchAgent::{render, unit_path}` | PL-A3: the plist bytes are identical for a fixture `Paths` |
| key files `ROOT_HOME/keys/providers/<id>` (R-F4) | `plat::FileKeys`, the only keystore on macOS in v1.0 | egress key tests unchanged |
| `install::Paths`, `access::{BUILTIN_READ_DENY, BUILTIN_WRITE_DENY}` | `plat::Paths`; the lists become `deny::{COMMON, MACOS, LINUX, WINDOWS}`, and macOS uses `COMMON ∪ MACOS`, which equals today's lists entry for entry | PL-A4: the resolved deny parameters (`Z_DENY_<i>`, `Z_WDENY_<i>`) are identical |

Changes that **do** alter macOS behaviour are separate, flagged steps after PLAT-0:
- the checkpoint store of ux-v1.md §3.6.5 (UX-8), with APFS `clonefile` as its per-file object path, which replaces the whole-tree session clone and turns today's "skip: not on APFS" into a copied checkpoint;
- the Keychain keystore, only after Developer ID signing (§3.6.2);
- the version gate by self-test (§3.6.1).

### 3.4 Linux

#### 3.4.1 Support and minimum versions

| Requirement | Minimum | Why | `doctor` on failure |
|---|---|---|---|
| Architecture | x86_64, aarch64 | Rust Tier 1 `*-unknown-linux-gnu` | `ux.platform` |
| Kernel | 5.10 | Debian notes bubblewrap 0.12 needs at least Debian 11's kernel; `openat2` (5.6) for fd-pinned binds; `close_range` (5.9) for exec-clean | refuse |
| glibc | 2.31 | the bundle is built against Debian 11's glibc (§3.7.1; ux-v1.md §3.2.1). musl distros are refused in v1 | refuse (installer) |
| bubblewrap | **0.8.0** at `/usr/bin/bwrap` (or `[sandbox] bwrap`), not setuid | `--disable-userns` and `--assert-userns-disabled` arrived in 0.8.0; 0.12.0 dropped setuid builds | refuse below 0.8.0. Warn below 0.12.0 unless the distro package is on the Root's fixed list for GHSA-pxhw-h44j-8pfx (symlink-traversal escape fixed in 0.12.0; its preconditions are UNVERIFIED, §9.3 I1) |
| Unprivileged user namespaces | available to `/usr/bin/bwrap` | bwrap ≥ 0.12 has no setuid mode | refuse, print §3.4.6's fix |
| seccomp | `CONFIG_SECCOMP_FILTER` | zone and instance filters | refuse |
| Landlock | optional. ABI ≥ 1 recommended, ≥ 4 for TCP rules, ≥ 6 for scopes, ≥ 9 (Linux 7.1) for `RESOLVE_UNIX` | defence in depth only; the mount namespace is the boundary | record `landlock_abi`; warn below 1 |
| git | 2.39.4 | unchanged (kernel/sandbox.md §3.8) | refuse |
| systemd user manager | optional | without it the service is `manual` (§3.4.7) | info |
| `secret-tool` (libsecret-tools) and a Secret Service | optional | without them keys stay in 0600 files (§3.4.9) | info |

**Distributions** (versions from distro trackers as remembered, UNVERIFIED until PL-L13 records them):

| Distro | Kernel → Landlock ABI | bwrap | v1.0 |
|---|---|---|---|
| Ubuntu 24.04 LTS | 6.8 → 4 | 0.9.0 | supported, after the AppArmor step (§3.4.6) |
| Ubuntu 26.04 LTS | 7.0 → 8 | newer; the `bwrap-userns-restrict` profile ships with apparmor | supported |
| Ubuntu 22.04 LTS | 5.15 → 1 | 0.6.1 | **refused in v1.0** (bwrap < 0.8.0). v1.0.x may accept 0.6.x once PL-L12 shows that the zone seccomp namespace denies of §3.4.3 alone stop nested user namespaces |
| Debian 12 | 6.1 → 2 | 0.8.0 | supported |
| Debian 13 | 6.12 → 6 | 0.12.0 (stable update `0.12.0-1~deb13u1`) | supported |
| Fedora (current) | current → current | current | supported |
| Alpine, Void, other musl | — | — | refused in v1 (glibc bundle) |

#### 3.4.2 The zone on Linux

The Root launches every instance, as on macOS (root/root.md §3.4, §3.5). The chain is `init-root` → `init-root exec-clean --keep 5 --keep 6 --keep 7 [--keep 3 --keep 4] -- /usr/bin/bwrap --args 6 -- …` → bwrap → (bodies) `init-kernel sandbox-inner` → the program.

**bwrap's own fds.**
- **fd 6, `--args 6`**: the whole argument list, NUL-separated, written by the Root into a pipe. A home plan can reach a few hundred mount operations (§3.4.4), so the list never meets `ARG_MAX`, and the owner's file names never appear in `/proc/<pid>/cmdline`.
- **fd 5, `--add-seccomp-fd 5`**: the zone seccomp program (cBPF from `seccompiler` 0.5.0).
- **fd 7, `--info-fd 7`**: bwrap writes one JSON object with `child-pid` and the namespace inodes. The Root reads exactly one object, keeps `pid-namespace` as the instance's identity (kernel/sandbox.md §3.6.5) and closes its end. If the object lacks the field, the Root stats `/proc/<child-pid>/ns/pid` instead. The field names are UNVERIFIED; PL-L4 checks them.
- PL-L5 checks that none of fds 5–7 is still open in the sandboxed program (`/proc/self/fd` lists only 0–2, plus 3–4 for the kernel).

**Order of the argument list.** Mounts apply in order, so the list is:
1. the zone arguments (root/root.md §3.4): `--die-with-parent --new-session --unshare-user --unshare-pid --unshare-ipc --unshare-net --unshare-uts --unshare-cgroup-try --disable-userns --assert-userns-disabled --cap-drop ALL --add-seccomp-fd 5 --info-fd 7`;
2. the system view of kernel/sandbox.md §3.6.1, from `--tmpfs /` to `--tmpfs /tmp`, then the toolchain;
3. the ADR-009 home plan (§3.4.4), for the live body and fleet members only;
4. the profile's `INIT_HOME` mounts (kernel/sandbox.md §3.6.1, §3.6.4; root/root.md §3.4 for `kernel`), the two lane sockets, and for the kernel `--tmpfs /run/init-root` plus the egress socket;
5. charter `extra_read` entries;
6. `--remount-ro /`, `--chdir`, `--`, the program.

**Fd-pinned binds (new: a Linux boundary gap in root/root.md §3.4).** A bind source that lies in a tree the zone can write can be swapped for a symlink between the Root's validation (V2) and bwrap's `mount()`. The kernel instance writes all of `INIT_HOME`, so agent code from L3 could rename `INIT_HOME/probe/<id>/work` and plant a symlink to `~/.ssh` just before the Root launches a probe. bwrap would follow the link, and the probe would get a writable `~/.ssh`. On macOS this race is harmless, because the zone tail checks paths at access time; on Linux "not mounted" *is* the boundary. Therefore:
- The Root opens every bind source under `INIT_HOME`, under the workspace and under the home plan with `openat2(anchor, rel, O_PATH | O_CLOEXEC, RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_BENEATH)`, walking from anchors it opened at start (`INIT_HOME`, the owner's home). It checks the file type, clears `FD_CLOEXEC`, and passes the source as `/proc/self/fd/<n>`. `mount(2)` on a `/proc/self/fd` link binds the inode the fd holds, not whatever the path names by then. UNVERIFIED that bwrap passes such a source to `mount` unchanged; PL-L6 races a renamer against 10,000 spawns and fails on the first instance that sees the planted target. If bwrap resolves the source itself, the fallback is to bind-mount each `INIT_HOME` top-level directory separately in the kernel instance (a mount point cannot be renamed: `EBUSY`) and to refuse any probe, fleet or build root whose path contains a symlink when re-checked after spawn through `/proc/<child-pid>/mountinfo`.
- System sources (`/usr`, `/etc`, `/lib*`, the toolchain) are not zone-writable and stay plain paths.

**The zone seccomp program** extends root/root.md §3.4's list with namespace denies, so that no nested user namespace can be made even if bwrap's `--disable-userns` were absent or bypassed:
- `unshare`, `setns`: `EPERM`;
- `clone` with any of `CLONE_NEWUSER | CLONE_NEWNS | CLONE_NEWPID | CLONE_NEWNET | CLONE_NEWIPC | CLONE_NEWUTS | CLONE_NEWCGROUP` in its flags argument: `EPERM`;
- `clone3`: `ENOSYS`. Its flags sit behind a pointer that seccomp cannot read, and glibc ≥ 2.34 falls back to `clone` on `ENOSYS`, as container runtimes rely on (UNVERIFIED for every libc in the zone; PL-L12 runs cargo, git and the body's tests under it).

The rest of the zone list is unchanged: `io_uring_*`, `ptrace`, `process_vm_*`, `bpf`, `perf_event_open`, `userfaultfd`, the keyring calls, `kexec_*`, `open_by_handle_at`, and `socket`/`socketpair` outside `AF_UNIX`.

#### 3.4.3 Egress on Linux

- `--unshare-net` leaves the instance with only its own loopback. There is no route to anything.
- seccomp refuses every socket domain except `AF_UNIX`, so behaviour matches macOS, where a test needing TCP loopback fails the same way (kernel/sandbox.md §3.6.3).
- Abstract Unix sockets are per network namespace, so the host's abstract sockets (some D-Bus session buses, X11's `@/tmp/.X11-unix/X0`) are unreachable. Pathname sockets exist only where bound: `/run/user/<uid>/bus`, `docker.sock`, `/tmp/.X11-unix` and the Root's `ROOT_HOME/run/*.sock` are never mounted.
- The kernel instance alone gets `--tmpfs /run/init-root` and a bind of `ROOT_HOME/run/egress.sock` at `/run/init-root/egress.sock`. Its `hello.paths.egress_sock` is therefore `/run/init-root/egress.sock` on Linux (root/root.md §4.1.3 shows the macOS value). The kernel derives the Root canary from that directory only on macOS; Linux needs no canary (the sweep is by pid namespace).
- The Root's egress server accepts a connection only when `SO_PEERCRED`'s pid has the kernel instance's pid-namespace inode (the Root sees host pids of its descendants). The owner socket keeps root/root.md §4.2's rule: the peer's pid namespace must be the Root's.

This is the strongest egress of the three platforms: the zone has no IP interface at all, and one bound socket file is its only way out.

#### 3.4.4 ADR-009 on Linux: the split-bind home plan

ADR-009 lets the live body read the owner's home minus a deny list, and write the workspace or, in full mode, the home. bwrap cannot carve a subpath out of a bind mount, and Landlock only adds access. So the Root builds the instance's view of the home from a skeleton:

```
input:  HOME, the grant (read_home, write W: none | the workspace | HOME in full mode),
        D = read-deny ∪ write-deny (resolved, §3.4.5), X = {ROOT_HOME, INIT_HOME}
plan(dir):                                  # dir is a skeleton directory: --dir dir (on the root tmpfs)
  for e in readdir(dir), by symlink_metadata (never followed), sorted by name:
    if e ∈ X or e ∈ read-deny:              skip                      # absent in the instance: ENOENT
    elif e is a symlink:                    --symlink <target> e      # resolved inside the instance
    elif e is a directory and (some d ∈ D ∪ X ∪ {W} lies strictly under e):
                                            --dir e; plan(e)          # expand
    elif W is not none and (e is W or e lies under W) and e ∉ write-deny:
                                            --bind   /proc/self/fd/<n> e      # writable
    elif read_home:                         --ro-bind /proc/self/fd/<n> e     # readable
    else:                                   skip
# W itself is never expanded: it contains no deny entry (`contains_deny`, below), so it is bound whole.
# In full mode W = HOME, so every entry that is not denied is writable, except write-deny entries (read-only).
```

- **What the instance sees.** Everything readable is bound, each denied entry is simply missing, and the expanded directories (`~`, `~/.config`, `~/.local`, …) are directories on the root tmpfs. `--remount-ro /` at the end makes them read-only, so the agent cannot create, rename or delete entries at an expanded level; it can write only inside bound writable children.
- **Late entries.** An entry created at an expanded level after the spawn (for example the owner creating `~/.aws` later) is invisible to the running instance; it appears at its next spawn under the same rules. A new deny entry is always at an expanded level, because every ancestor of a deny path is expanded. Entries created inside a bound child appear live, which is what the owner expects of a project.
- **Delta from macOS.** In full mode the agent cannot create a new file directly in `~` or in an expanded directory (`EROFS`); on macOS it can. Everything else matches ADR-009's table.
- **Workspaces that contain a deny entry.** On Linux and Windows the Root refuses a workspace that contains a deny-list entry, `ROOT_HOME` or `INIT_HOME` (new refusal reason `contains_deny`, proposed for root/root.md §3.17.1). Without it W would have to be expanded, and its own children are zone-writable names, the only place a symlink race on a plan path could remain. macOS keeps its rule, because the tail holds there whatever is bound.
- **Size.** A typical home has 40–80 top-level entries, and the expansion of `~/.config`, `~/.local` and `~/.local/share` adds about 150, so a plan is about 200–300 operations. Above 2,048 operations the Root drops the home read for that spawn (the instance gets workspace-only access, `INIT_ACCESS` says so) and sends a `notice` with `code: "access"`.
- **Cost.** One `readdir` per expanded directory and one `openat2` per bound entry, about 1–3 ms per spawn (UNVERIFIED; PL-L1 measures it).
- **Landlock agrees.** When Landlock is present, the inner stage's read rules for a live body add the same bound paths, so a second layer refuses what the mounts refuse. It cannot reach paths the mounts hide, which is the point.

Probes never get a home plan (hermetic, ADR-009). Fleet members get the read-only plan.

#### 3.4.5 Deny lists per OS

ADR-009's lists name macOS paths. Each OS uses `COMMON ∪ <its own>`; the charter's `sandbox.read_deny` adds to all of them. The lists move to `root/sandbox-profiles/src/deny.rs` (PLAT-0; contract: ADR-009, root/root.md §3.17).

| List | Entries (relative to the home unless absolute) |
|---|---|
| read, COMMON | `.config/init-agent`, `.ssh`, `.gnupg`, `.aws`, `.azure`, `.config/gcloud`, `.kube`, `.docker/config.json`, `.netrc`, `.npmrc`, `.pypirc` (today's dotfile entries) |
| read, MACOS | `Library/Keychains`, `Library/Cookies`, `Library/Mail`, `Library/Messages`, `Library/Safari`, `Library/Application Support/{Google/Chrome,Firefox,BraveSoftware,Microsoft Edge,Arc,1Password}`, `Library/Group Containers` (today's) |
| read, LINUX | `.local/share/keyrings` (GNOME Keyring), `.local/share/kwalletd` (KWallet), `.password-store` (pass), `.config/1Password`, `.mozilla`, `.thunderbird`, `.config/google-chrome`, `.config/chromium`, `.config/BraveSoftware`, `.config/microsoft-edge`, `.pki` (NSS), `.config/Signal`, `.local/share/TelegramDesktop` |
| read, WINDOWS | `AppData/Roaming/Microsoft/{Credentials,Protect,Crypto}`, `AppData/Local/Microsoft/Credentials`, `AppData/Local/Google/Chrome/User Data`, `AppData/Local/Microsoft/Edge/User Data`, `AppData/Local/BraveSoftware`, `AppData/Roaming/Mozilla`, `AppData/Roaming/Thunderbird`, `AppData/Local/1Password`, `AppData/Roaming/Signal`, `AppData/Roaming/gnupg`, `AppData/Roaming/gcloud`, `_netrc`, `AppData/Roaming/init-agent` |
| write, COMMON | today's shell files (`.zshrc` … `.profile`), `.config/fish`, `.gitconfig`, `.config/git`, `.local/bin`, `bin`, plus the app directory (ux-v1.md §3.2.4) |
| write, MACOS | `Library/LaunchAgents` |
| write, LINUX | `.config/autostart`, `.config/systemd/user`, `.config/environment.d`, `.pam_environment` (today's), plus `.bash_aliases`, `.xprofile`, `.xsessionrc`, `.xinitrc`, `.config/plasma-workspace/env`, `.local/share/applications` |
| write, WINDOWS | `AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Startup`, `Documents/PowerShell`, `Documents/WindowsPowerShell`, `AppData/Local/Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState` (startup commands), the app directory |

Proposed additions to COMMON for every OS (cross-doc to ADR-009, §9.1 X3), found while porting: `.git-credentials` (git's plaintext credential store), `.cargo/credentials.toml` (crates.io token), `.config/gh` (GitHub CLI tokens), `.config/hub`.

#### 3.4.6 Preflight and the Ubuntu AppArmor guidance

`init-root doctor` and `install` run these checks. Each is a probe that exits on its own; none signals anything it did not start.

| Check | Method | Result |
|---|---|---|
| bwrap present, version, not setuid | `stat` (no `S_ISUID`), `bwrap --version` | §3.4.1 |
| flags | `bwrap --args` dry runs of each flag the Root renders | refuse on a missing flag |
| user namespaces | `bwrap --unshare-user --unshare-net --disable-userns --ro-bind / / /bin/true` exits 0 | refuse with the text below |
| AppArmor restriction | `sysctl kernel.apparmor_restrict_unprivileged_userns` (`/proc/sys/kernel/apparmor_restrict_unprivileged_userns`) | informs the message only; the probe above decides |
| seccomp | `prctl(PR_GET_SECCOMP)` and the zone filter loading in the probe | refuse |
| Landlock ABI | `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` | recorded |
| x32 ABI | `/proc/config.gz` or `/boot/config-$(uname -r)` shows `CONFIG_X86_X32_ABI=y` | warn (kernel/sandbox.md §3.6.3) |
| WSL | `/proc/sys/fs/binfmt_misc/WSLInterop` or `WSL_DISTRO_NAME` | tier `wsl2` (§3.5.2) |
| systemd user manager | `systemctl --user is-system-running` answers | service `systemd-user`, else `manual` |
| Secret Service | `secret-tool` exists and a lookup of a sentinel attribute returns within 2 s | keystore `secret-service`, else `file` |
| reflink | a `FICLONE` of a 1-byte scratch file under `ROOT_HOME` | snapshot objects `reflink`, else `copy` |

When the namespace probe fails and the sysctl reads 1, `doctor` prints exactly this (copy id `ux.sandbox`, Linux variant):

```
INIT can't start its sandbox: Ubuntu's AppArmor blocks user namespaces for bubblewrap
(kernel.apparmor_restrict_unprivileged_userns = 1). INIT never runs without its sandbox.

Fix it once (needs your password; INIT never runs sudo itself):
  Ubuntu 25.04 and later:  sudo systemctl reload apparmor
  Ubuntu 24.04:            sudo apt install apparmor-profiles
                           sudo cp /usr/share/apparmor/extra-profiles/bwrap-userns-restrict /etc/apparmor.d/
                           sudo apparmor_parser -r /etc/apparmor.d/bwrap-userns-restrict
Weaker alternative (affects every program on this computer):
  sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
Then run: init doctor
```

- The purpose-built `bwrap-userns-restrict` profile is preferred over a broad `userns,` profile, which Canonical calls "a trivial bypass", and over the sysctl. It is tied to `/usr/bin/bwrap`, which is why INIT uses the distro's bwrap and never bundles one.
- UNVERIFIED: the profile's path inside `apparmor-profiles` on 24.04, and that the profile's limits on bwrap's children leave INIT's inner stage working. INIT needs no capability inside the namespace (`--cap-drop ALL`; Landlock and seccomp are unprivileged under `no_new_privs`), so it should. PL-L9 runs all three options on a hosted ubuntu-24.04 runner, which is a disposable VM where changing the sysctl is allowed.
- Inside unprivileged containers (Docker without `--privileged`) bwrap cannot mount `/proc`. INIT treats that as a failed probe; it never falls back to sharing the host's `/proc` (Claude Code's `enableWeakerNestedSandbox`).
- **No degraded mode.** Without user namespaces there is no mount or network namespace. A Landlock-plus-seccomp mode could block IP sockets, but it cannot stop `connect` to pathname sockets such as `docker.sock` before Landlock ABI 9 (Linux 7.1), which no LTS kernel has. INIT refuses instead of offering it, and never fails open.

#### 3.4.7 Held kill and the service on Linux

**Held kill.** The leader the Root holds is bwrap: the pid it spawned through exec-clean, unreaped. `sys::signal` keeps ADR-008's `waitid(WNOWAIT)` check.
- `kill{mode:"kill"}`, panic and stop: `SIGKILL` to the held bwrap pid and its group. `--die-with-parent` makes the namespace's init die with bwrap, and the kernel then kills every process in the pid namespace. The Root confirms by checking, every 20 ms for up to 1 s, that no `/proc/<pid>/ns/pid` has the instance's inode. That check only reads.
- **Completeness.** Nothing can leave a pid namespace, so a `setsid` or double-fork escapee dies too. ADR-008's residual risk does not exist on Linux, no live sweep is ever needed, and `status.sweep` reads `"held_pidns"`.
- `kill{mode:"term"}`: the program runs in its own session inside the namespace, so the only held target is bwrap, and a `SIGTERM` to bwrap ends the namespace with `SIGKILL`. Graceful stops therefore go through the stdio `stop` frame first (kernel/supervise.md), and `term` on Linux acts as `kill` after the grace period. The kernel must not rely on `SIGTERM` reaching a body on Linux.
- pidfds are not needed: the Root holds unreaped children, so their pids cannot be reused.

**Service.** root/root.md §4.3 step 9's unit stays, with a description line:

```ini
[Unit]
Description=INIT Root (owner-side supervisor of the INIT agent)

[Service]
Type=simple
ExecStart="<ROOT_HOME>/bin/init-root" start
Environment="INIT_ROOT_HOME=<ROOT_HOME>"
Restart=always
RestartSec=2
KillMode=mixed
TimeoutStopSec=60

[Install]
WantedBy=default.target
```

- `KillMode=mixed`: `SIGTERM` goes to the Root, which stops gracefully through its self-pipe, and anything left in the unit's cgroup gets `SIGKILL` afterwards. That matches `--die-with-parent`: on Linux the zone never outlives the Root. On macOS the zone outlives a Root crash for up to 10 s, until the control link's EOF reaches the kernel (kernel/supervise.md §4.5). The panic latch file survives either way.
- `NoNewPrivileges=yes` is deliberately absent. AppArmor may refuse the profile transition for `/usr/bin/bwrap` under `no_new_privs` (UNVERIFIED), and the Root needs no setuid program anyway.
- Load and unload are the launcher's, with consent (ux-v1.md §3.4.8): `systemctl --user daemon-reload`, `systemctl --user enable --now init-root.service`; `systemctl --user disable --now init-root.service` before the unit goes to the freedesktop Trash. `loginctl enable-linger` is only mentioned, never run.
- **`manual` fallback** (no systemd user manager: OpenRC, runit, WSL without systemd, containers): no unit file. The launcher starts `init-root start` detached with `setsid` when `owner.sock` does not answer, and `status.service` reads `manual`. A crashed Root is then restarted by the next `init`, not by a service manager.
- `systemd-analyze --user verify` checks the rendered unit in PL-L10.

#### 3.4.8 Snapshots on Linux (the checkpoint store's platform rules)

ux-v1.md §3.6.5 defines the checkpoint store: a manifest per checkpoint (path → sha256, mode, symlink target) and content-addressed objects under `ROOT_HOME/snapshots/<folder-id>/objects/`. That store is git-style without git, and on ext4 and NTFS, which most Linux and Windows users have, it is **the** undo path. This doc fixes the platform rules under it, on every OS:
- **Putting an object.** Hash the file (sha256, streaming), and if the object is new, place it by the fastest method the volume allows: APFS `clonefile` (macOS), `ioctl(dst, FICLONE, src)` (Linux btrfs, XFS with `reflink=1`, bcachefs), `FSCTL_DUPLICATE_EXTENTS_TO_FILE` on ReFS or a Dev Drive (Windows, later), else a copy. `EXDEV`, `EOPNOTSUPP` and `EINVAL` from a clone fall back to a copy for that file and are counted; the method in use is `install.json.platform.snapshot`.
- **No git process.** The Root never runs git on a workspace for snapshots. A work tree's `.gitattributes` can name filters and its `.git/` holds hooks and config the agent writes; the Root's own walker plus sha2 (already a Root dependency) needs no new crate and interprets no workspace file.
- **Walk.** `symlink_metadata` only; never follow a link; record symlinks as text; skip FIFOs, sockets and devices (recorded as skipped). The workspace's `.git/` **is** included, so an agent that damages the owner's repository can be undone.
- **Exclusions** (proposed to ux-v1.md §3.6.5, §9.1 X6): regenerable caches are recorded as excluded and left untouched by a restore, so they neither count toward the 2 GiB limit nor make a Rust or Node project too big to checkpoint: `target/` beside a `Cargo.toml`, `node_modules/`, a `venv`-style directory holding `pyvenv.cfg`, `__pycache__/`, `.pytest_cache/`, `.mypy_cache/`, `.gradle/`, `.next/`, `.tox/`. Single files over 256 MiB are skipped with a notice.
- **Restore, race-safely.** The Root opens the workspace with `O_DIRECTORY | O_NOFOLLOW`, re-validates it (root/root.md §3.17.1), and does every operation relative to directory fds (`openat`, `mkdirat`, `unlinkat`, `renameat`, `symlinkat`) with `O_NOFOLLOW`. A symlink the agent planted is therefore replaced, never followed, and nothing outside the workspace is touched. Files are written to a temporary name and renamed into place; directories are fsynced. Entries not in the manifest and not excluded are removed, by exact relative path. The restore waits for the agent to be idle (ux-v1.md §3.6.5; root/root.md §9.3 Q4).
- **Garbage collection.** At pruning, objects no kept manifest references are removed by exact path.

#### 3.4.9 Keys on Linux

**Recommended default (`keys.store = "auto"`):** the Secret Service when one answers at `keys set` time, else the 0600 file of R-F4. This departs from ux-v1.md §3.4.5's "file everywhere by default" on Linux only (§9.1 X7). The reasons ux-v1.md gives for not defaulting to the OS store are macOS Keychain's code-identity ACLs, which the Secret Service does not have; and the Secret Service keeps the key encrypted at rest and out of backups of `~/.init-root`.
- **Backend `secret-service`:** the CLI `/usr/bin/secret-tool` (libsecret-tools), added to the Root's fixed program list (R-X).
  - store: `secret-tool store --label "INIT Root · <id>" service dev.init.root kind provider id <id>`, with the secret written to its stdin and never in argv or the environment;
  - lookup: `secret-tool lookup service dev.init.root kind provider id <id>`, read from a pipe into the zeroing secret type;
  - delete: `secret-tool clear …` with the same attributes.
  - `ROOT_HOME/keys/index.json` (ICJ, no secret) records which backend holds each key.
- **Why the CLI, not a crate.** `secret-service` and `keyring` pull in zbus with an async executor, or libdbus. The first conflicts with ADR-003 (std threads in the Root), the second with a lean static-friendly build; the CLI adds no dependency to root/root.md §4.8's list.
- **A locked keyring.** If a lookup fails or blocks (a lingering service started before the desktop login), egress answers that provider with `egress.key_locked`, alerts once, and retries every 60 s. It never falls back to another key.
- **The zone cannot reach it.** The session bus socket is never mounted, abstract buses are in another network namespace, and the Secret Service has no per-application ACL, so isolation is entirely the mount and network namespaces.
- UNVERIFIED: that `secret-tool store` reads the secret from a non-terminal stdin, and its exit codes. PL-K2 runs it under `dbus-run-session` with `gnome-keyring-daemon` on a hosted runner.

#### 3.4.10 Paths on Linux

- `ROOT_HOME` `~/.init-root` and `INIT_HOME` `~/.init`, as on macOS (ux-v1.md §3.2.4 keeps them; the 64-byte rule fits `sun_path`'s 108). XDG base directories are rejected for the homes: Z1 and R-F5 rely on one 0700 `ROOT_HOME`. The app directory follows `XDG_DATA_HOME` (ux-v1.md).
- `PATH` and `LANG=C.UTF-8` for the kernel are root/root.md §4.4's.
- Comparisons are byte-exact (case-sensitive filesystems).

### 3.5 Windows

#### 3.5.1 The tiering decision

**v1.0 ships Windows through WSL 2.** The Linux build runs inside the WSL VM with the full security model of §3.4. It needs no new sandbox code, the Linux CI tests the code that runs there, and the Windows-specific work is a doctor row, a hint and an installer mode (§3.5.2).

**Native Windows is v1.x**, built on Microsoft Execution Containers (MXC): GA on 2026-10-07, AppContainer-based process containers on Windows 11 24H2+ with filesystem read-only, read-write and deny lists, outbound network control, and a Rust SDK (`mxc-sdk` 1.0.1 on crates.io; OpenAI Codex already ships on it). Native costs about 110–160 agent-hours (§3.9): named pipes through the kernel, proxy, serve, body, TUI and initctl; Job Objects; the Task Scheduler; Credential Manager; the body's tools without bash; a Windows toolchain for self-evolution. Its only test hardware is GitHub's hosted runners, and whether MXC runs on their Windows Server 2025 image is unknown (spike Q7). So it waits for the spike.

**The Codex-style elevated fallback** (restricted token, dedicated sandbox users created with admin rights, WFP filters) is not planned for v1. It leaves persistent users and firewall rules on the owner's machine, which needs its own consent and uninstall design. It would be considered only if demand from pre-24H2 machines appears.

**No tier runs without MXC and without admin.** A restricted token alone cannot deny reads, so the zone could read `ROOT_HOME`'s keys and spend the owner's money outside every ceiling.

#### 3.5.2 The WSL 2 tier (v1.0)

| Topic | Rule |
|---|---|
| Requirements | Windows 10 22H2 or Windows 11; WSL 2 (`wsl --status` reports version 2); a distro meeting §3.4.1, Ubuntu 24.04 recommended (the AppArmor step of §3.4.6 applies only where AppArmor is enforced inside WSL; the probe decides) |
| What holds | everything in §3.4, inside the WSL VM. `doctor` reports `tier: wsl2` |
| Windows interop | WSL runs `.exe` files through a `binfmt_misc` handler (`/init`) and the `WSL_INTEROP` socket under `/run/WSL`. The zone has no `/init`, no `/run/WSL` and no `/mnt/c`, and the Root clears the environment, so starting a Windows program from the zone fails. PL-W1 checks it (UNVERIFIED until run on WSL) |
| Windows drives | `/mnt/c` (drvfs) is outside the Linux home, so a workspace there is refused (`outside_home`). The notice adds: `keep projects in the Linux filesystem (\\wsl$\<distro>\home\<you>): /mnt/c is slow and outside INIT's grant` |
| Snapshots | the distro's ext4 VHD has no reflink, so checkpoints use copies |
| Keys | usually no Secret Service in WSL, so the 0600 file. Windows processes of the same user can read the distro's files through `\\wsl$`; they are the owner's own unsandboxed programs, trusted exactly as on macOS |
| Service | systemd user unit when the distro runs systemd (`/etc/wsl.conf` `[boot] systemd=true`, the default on current Ubuntu images: UNVERIFIED); else `manual`. Whether WSL keeps the VM running while only a user service runs is UNVERIFIED (PL-W3). The panic latch and the LKG survive VM restarts, because they are files |
| Owner terminal | Windows Terminal → the distro → `init`. `install.ps1` also installs `%LOCALAPPDATA%\Programs\INIT\bin\init.cmd`, which runs `wsl.exe -d <distro> -- ~/.local/bin/init %*`; WSL translates the Windows cwd into the Linux one, so `\\wsl$\…\proj` becomes `/home/<you>/proj` and `C:\proj` becomes `/mnt/c/proj` (refused with the hint above) |
| `install.ps1` in v1.0 | a WSL bootstrapper (delta to ux-v1.md §3.2.1, §9.1 X8): check `wsl.exe --status`; with no WSL 2 distro, print the owner's one-time command `wsl --install -d Ubuntu-24.04` (admin and a reboot; INIT never runs it) and exit 2; otherwise run the Linux installer inside the distro (`wsl.exe -d <distro> -- sh -c "curl -fsSL <DIST>/install.sh \| sh"`), then install the `init.cmd` shim and the user PATH entry as ux-v1.md describes |
| Notifications | the TUI's BEL (Windows Terminal flashes the taskbar); `notify.cmd` has no default in WSL |
| Guarantees | as `linux` (§3.5.5); platform cap L4 |

#### 3.5.3 Native Windows (v1.x, after PLAT-N0)

**Confined launch: MXC.** The Root renders each profile into an MXC request (`mxc-sdk` `v1::spawn` with a `v1::ContainerRequest`; field names UNVERIFIED until the spike) in a pure renderer `root/sandbox-profiles/src/mxc.rs`:
- read-only: the system and toolchain paths, the profile's read-only `INIT_HOME` parts, and the home minus the deny list for the live body and members;
- read-write: the profile's writable `INIT_HOME` parts, plus the workspace or the home under full mode;
- denied: `ROOT_HOME`, the deny lists of §3.4.5, `INIT_HOME/{kernel,root}`;
- network: outbound denied, no loopback, no local binding; no UI or desktop access;
- process: program, arguments, cwd, the complete environment.
The kernel's lane endpoints are named pipes (below), not network, so the zone needs no network capability at all.

**Held kill: a Job Object per instance.** The Root creates the job (`CreateJobObjectW`), sets `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION` and never `BREAKAWAY_OK`, and puts the instance in it before it runs its first instruction: created suspended, `AssignProcessToJobObject`, then resumed, or through `PROC_THREAD_ATTRIBUTE_JOB_LIST`. Which of these MXC permits is spike Q2.
- Kill: `TerminateJobObject(job, 137)`. Empty: `QueryInformationJobObject(JobObjectBasicAccountingInformation)` reports `ActiveProcesses == 0`. CPU: its `TotalUserTime + TotalKernelTime` gives `rusage_ms`.
- If the Root dies, its handles close and `KILL_ON_JOB_CLOSE` ends the zone, as `--die-with-parent` does on Linux.
- `Held` owns the job handle and the process handle. No pid-based kill exists in INIT on Windows, so ADR-008's lock 1 becomes a type rule. `status.sweep` reads `"held_job"`.
- Residual: a process created through WMI (`Win32_Process.Create`) is not put in the caller's job. From an AppContainer, WMI is expected to be unreachable (spike Q2).

**IPC: named pipes, not AF_UNIX.** AF_UNIX on Windows has no ancillary data (no `SCM_RIGHTS`) and no `socketpair`, std's support is unstable (rust-lang/rust#147335), and the body could use it only through FFI. A named-pipe client is `std::fs::OpenOptions::new().read(true).write(true).open(r"\\.\pipe\…")`, safe std code for the body, the TUI, the launcher and initctl.
- **Names:** `\\.\pipe\init-<id16>-<endpoint>`, where `<id16>` is 16 random hex digits chosen at install and kept in `install.json`, and `<endpoint>` is `owner`, `egress`, `ctl-<boot>`, `kernel-<lane>` or `proxy-<lane>`.
- **Servers** use `PIPE_ACCESS_DUPLEX`, `FILE_FLAG_FIRST_PIPE_INSTANCE` (so no other process can squat a name first) and `PIPE_REJECT_REMOTE_CLIENTS`. Security descriptors: `owner` grants only the owner's SID; `egress` grants only the kernel instance's container SID; each lane pipe grants only its instance's SID. Each carries a low mandatory label (`S:(ML;;NW;;;LW)`), because AppContainer processes run at low integrity and the default medium label would forbid their writes (UNVERIFIED; spike Q1).
- **Peer checks:** `GetNamedPipeClientProcessId`, then for `owner` refuse any client whose token is an AppContainer token (`TokenIsAppContainer`), and for `egress` require `IsProcessInJob(client, kernel_job)`.
- **The control link** replaces fds 3 and 4. The Root creates the `ctl-<boot>` server instance, opens the client end itself as an inheritable handle, and lets the kernel inherit only that handle through `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`; the kernel finds it in `INIT_ROOT_HANDLE=<decimal>`. If MXC cannot pass an inherited handle (spike Q3), the kernel instead connects to `ctl-<boot>` by name and sends `INIT_ROOT_CTL_TOKEN` (32 random hex digits from its environment) as its first frame; the pipe allows one instance and one client.
- **Handle hand-over** replaces the fd channel. The Root, which holds the kernel's process handle with `PROCESS_DUP_HANDLE` from creation, calls `DuplicateHandle(GetCurrentProcess(), h, kernel, &out, 0, FALSE, DUPLICATE_SAME_ACCESS | DUPLICATE_CLOSE_SOURCE)` for each of the three pipe ends, then sends `{"v":1,"ev":"handles","data":{"id":"k-42","h":[1234,1238,1242]}}` on the control link **before** the response line, keeping root/root.md §4.1.1's ordering rule. The kernel takes ownership with `OwnedHandle::from_raw_handle`. The kernel never holds a handle to the Root process.
- **Kernel servers** (kernel-proxy and kernel-serve, on tokio): `tokio::net::windows::named_pipe::ServerOptions` with `first_pipe_instance` and `reject_remote_clients`; the security descriptor is passed through its raw security-attributes option (exact API UNVERIFIED).
- **Body** (body/provider.md, cross-doc): `INIT_KERNEL_SOCK` and `INIT_PROXY_SOCK` carry pipe names. A busy pipe (`ERROR_PIPE_BUSY`) is retried 50 times, 20 ms apart; `WaitNamedPipe` would need FFI.

**Service: a Task Scheduler logon task** `\INIT\Root` (ux-v1.md §3.4.8), registered by the launcher with `%SystemRoot%\System32\schtasks.exe /Create /TN "INIT\Root" /XML <file>`. The XML the Root renders:
- a `LogonTrigger` for the owner's user;
- `Principal`: `LogonType = InteractiveToken`, `RunLevel = LeastPrivilege` (no stored password, no elevation);
- `Settings`: `MultipleInstancesPolicy = IgnoreNew`, `DisallowStartIfOnBatteries = false`, `StopIfGoingOnBatteries = false`, `ExecutionTimeLimit = PT0S`, `RestartOnFailure` with `Interval = PT1M` and `Count = 999`, `Hidden = true`;
- an `Exec` action: `<ROOT_HOME>\bin\init-root.exe start`.
UNVERIFIED that a standard user can register it without admin or a stored password (spike Q8). The fallback is the launcher starting the Root on demand (`DETACHED_PROCESS`), as `manual` does on Linux.

**Keys: Credential Manager**, generic credentials with target `INIT Root/provider/<id>` and `CRED_PERSIST_LOCAL_MACHINE` (never roaming). A blob larger than `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2,560 bytes; some OAuth tokens) goes to a DPAPI-encrypted file under `ROOT_HOME` with an owner-only ACL, which is ux-v1.md §3.4.5's Windows default. The owner's prompt asks for Credential Manager; it is proposed as the native default, with the DPAPI file as the fallback (§9.1 X7). That an AppContainer cannot read the owner's credential vault is spike Q9.

**Paths.**
- `ROOT_HOME` `%LOCALAPPDATA%\INIT\root`, `INIT_HOME` `%LOCALAPPDATA%\INIT\home`, the app directory `%LOCALAPPDATA%\Programs\INIT` (ux-v1.md §3.2.4). They are short on purpose: cargo target paths inside `INIT_HOME\work` can exceed `MAX_PATH` (260), and long-path support needs the admin-only `LongPathsEnabled` policy. `doctor` reports it and warns when it is off.
- Comparisons for the deny list and workspace validation are case-insensitive on the final path (`GetFinalPathNameByHandleW`, `CompareStringOrdinal` with `ignoreCase`).
- INIT's own repositories (`body.git`, the lineage) set `core.autocrlf=false`.
- Replacing a running `init-root.exe`: rename it to `init-root.exe.old`, write the new one, and delete the old file at the next start (Windows refuses to delete a running image).

**Panic channels.** There is no `SIGUSR1` on Windows. The owner pipe, the latch file, the watch paths and the mirror ref remain (root/root.md §3.9.1).

**The body's `bash` tool.** It runs `bash.exe` from Git for Windows, at a path fixed at install. Whether the MSYS2 runtime works inside an AppContainer is spike Q10; the fallback is PowerShell with a different tool description (body/tools.md, cross-doc).

**Toolchain for self-evolution.** `x86_64-pc-windows-msvc` needs Visual Studio Build Tools and a Windows SDK, a large install with admin rights and a licence the owner must accept. **Recommended:** build the body for `x86_64-pc-windows-gnullvm` (and `aarch64-pc-windows-gnullvm`) with llvm-mingw fetched by `init-fetch` like the Rust toolchain (ux-v1.md §3.4.9): no admin, no Visual Studio licence. The prebuilt body uses the same target, so the repro check compares like with like. UNVERIFIED that wasmtime 48.0.6 and the body build on `gnullvm` (spike Q11). INIT's own binaries stay `msvc`, Rust's best-supported Windows target, built on hosted runners where Visual Studio is preinstalled.

#### 3.5.4 The MXC spike (PLAT-N0, the first Windows WP)

Run on a hosted `windows-2025` runner, a disposable VM. A Rust test program uses `mxc-sdk` 1.0.1 to start a container under a deny-all-network policy and answers each question with a logged pass or fail.

| Q | Question | Pass |
|---|---|---|
| Q1 | Can a container open a named pipe created by the medium-integrity Root whose DACL grants the container's SID and whose label is low? With network egress denied and no loopback? The same for an AF_UNIX socket file, for completeness. | the pipe works; nothing else answers |
| Q2 | Does MXC expose the child's process handle or pid early enough to put it in a Root-owned Job Object before it runs, or accept one? Does `TerminateJobObject` end the whole container tree, including grandchildren and detached children? Can anything break away (job flags, WMI)? | the Root's job contains and ends every process |
| Q3 | Can the Root give the container specific inheritable handles (its own stdio pipes, the control link)? Can it `DuplicateHandle` into the container process? | yes to one of the two |
| Q4 | With egress denied: do TCP to 127.0.0.1, TCP to the internet, UDP, and DNS all fail? Does any mode need `allow_local_binding` or a loopback exemption (Codex's note for MXC-managed networking)? | all fail; INIT needs no exemption |
| Q5 | How are the read-only, read-write and deny lists enforced: ACL edits on the owner's files, are they persistent, are they removed at exit? Does deny win inside a read-write subtree? **Does a junction or symlink the zone plants in a read-write path redirect a grant or an ACL edit onto another file?** Does the container read the owner's home by default? | deny wins; no persistent ACL left behind; no grant follows a reparse point; nothing readable unless listed |
| Q6 | Lifetime: do children end when the leader exits (Codex reports they do)? | matches INIT's one-instance-per-leader model |
| Q7 | Does the process container run on Windows Server 2025 (build 26100), the `windows-2025` runner image? MXC documents only Windows 11 24H2+. | yes, or the owner must provide a Windows 11 24H2 machine or VM (§9.2 O5) |
| Q8 | Can a standard user register the logon task of §3.5.3 without admin or a stored password? | yes, or the on-demand fallback |
| Q9 | Can a container read the owner's Credential Manager vault or DPAPI master keys? | no |
| Q10 | Do Git for Windows' `bash.exe`, `git`, `cargo` and `rustc` work inside a container with the toolchain read-only? | yes, or PowerShell for the bash tool |
| Q11 | Do wasmtime 48.0.6 and the seed body build and pass their tests on `x86_64-pc-windows-gnullvm` with llvm-mingw? | yes, or MSVC Build Tools become a prerequisite for self-evolution |
| Q12 | Does each instance get its own SID, so that a body cannot open another lane's pipe? | yes, or per-lane pipe tokens |
| Q13 | Spawn latency and file I/O overhead of a container | body spawn ≤ 300 ms p50 |

**Decision rule.** Q1, Q2, Q4, Q5 and Q12 must pass, and Q7 must pass or the owner must provide hardware, before N1–N5 start. Otherwise native Windows stops, WSL 2 stays the Windows answer, and the spike is re-run on the next MXC release.

#### 3.5.5 Guarantees per tier

| Guarantee | `macos` | `linux` | `wsl2` | `win-mxc` (target) | `win-none` |
|---|---|---|---|---|---|
| The zone cannot read or write `ROOT_HOME`, the keys, the charter or the lineage | enforced (tail Z1) | enforced (never mounted) | enforced | spike Q5, Q9 | **no**, so refused |
| Zone writes confined to `INIT_HOME`, plus the workspace or full mode | enforced (Z3) | enforced (mounts; fd-pinned) | enforced | spike Q5 | partial, refused |
| Secrets deny list unreadable | enforced (Z2) | enforced (absent from the home plan) | enforced for the Linux home; Windows files invisible | spike Q5 | no |
| Provider-only egress | enforced (no IP; Unix sockets by literal path) | enforced (empty netns, seccomp, one bound socket) | enforced | spike Q1, Q4 | no |
| Held kill reaches every zone process | held instances and their groups; a `setsid` escapee is reported, not killed (ADR-008) | complete (pid namespace) | complete | complete if Q2 passes; WMI residual | — |
| Money ceilings (the keys stay in the Root) | enforced | enforced | enforced | follows rows 1 and 4 | — |
| Login persistence denied to the zone | `~/Library/LaunchAgents` (Z2) | systemd user, autostart, `environment.d` (write deny) | as `linux` | Startup folder, profiles; registry Run keys by AppContainer default (Q5) | — |
| Undo checkpoints | APFS clone objects | reflink or copy objects | copy objects | copy objects | — |
| Panic channels | 5 | 5 | 5 (inside WSL) | 4 (no signal) | — |
| Platform cap | charter's | charter's | charter's | 0 until S-N is green on that build | zone never starts |

**The platform cap rule** (proposed for root/root.md §3.13.1, §9.1 X2):

```
platform_cap(tier) =
  refuse (the Root never starts the zone)  if the zone could read the keys or ROOT_HOME, or write outside its grant
  0                                        if provider-only egress is not enforced
  1                                        if egress is enforced but a held-kill or deny-list gap is known and unreviewed
  4                                        otherwise
cap = min(charter.max_level, SUPPORTED_MAX_LEVEL, platform_cap)
```

When egress is not enforced, the Root also says so: `status.platform.egress = "not_enforced"`, the boot block carries it, and the TUI's status bar shows `egress: not enforced` in the alert colour (tui.md, cross-doc). It forces `read_home` off, so only the workspace and `INIT_HOME` are within reach, and starts the zone only after the owner types `accept unenforced egress` once per install. No v1.0 tier hits this branch; it exists so that a future tier cannot silently weaken the model.

### 3.6 macOS

#### 3.6.1 Minimum version

- **macOS 14 (Sonoma) on Apple Silicon**, the version the A6 sandbox suite was run on (14.6, 23G80, M2). Rust's own floor for `aarch64-apple-darwin` is 11.0; INIT's is set by what has been tested.
- **macOS 15 and 26**: kernel/sandbox.md §3.8 refuses an untested version. Proposed instead (§9.1 X4): `install` runs the live-safe rows of the A6 self-test (`init-sbx-selftest run`, which signals only the probes it holds) on the owner's own machine, and a pass accepts that version, recorded in `install.json`. Every release tag also runs the suite on a hosted `macos-15` runner, and `macos-26` once the image exists (§3.7.4).
- **Intel (`x86_64-apple-darwin`)**: cross-built on the M2 and smoke-tested under Rosetta 2 (PL-M4). Best effort: no Intel hardware runs the sandbox suite, so the release notes say so.

#### 3.6.2 Signing and notarization (the owner's $99/yr decision, OW-UX2)

**Without the Apple Developer Program** (the default until the owner decides):
- The binaries carry the linker's ad-hoc signature, which Apple Silicon requires to run them.
- `curl … | sh` and the Homebrew tap formula download with curl, which sets no `com.apple.quarantine` attribute, so Gatekeeper never assesses the files (UNVERIFIED on macOS 15; ux-v1.md UX-T3 checks it). These paths work unsigned.
- A browser download is quarantined, and Gatekeeper blocks it. The README says to use curl or brew, or `xattr -d com.apple.quarantine <file>`.
- A Homebrew **cask** is not possible: Homebrew 5.0 deprecated casks that fail Gatekeeper and disabled them from 2026-09-01.
- The Keychain keystore is not offered: an ad-hoc identity changes with each build, so the Keychain would prompt after every update.

**With the program** ($99 per membership year; the owner enrols and holds the certificate; agents never handle it):
1. **Sign** each Mach-O in the bundle (`init`, `init-tui`, `init-fetch`, `init-root`, `init-kernel`, `initctl`, `init-mockllm`, the prebuilt `init-body`) on the M2: `codesign --force --options runtime --timestamp --sign "Developer ID Application: <owner>" [--entitlements <plist>] <file>`.
2. **Entitlements.** Under the hardened runtime, Cranelift's JIT in `init-kernel` (selfmap) and `init-body` (the extension host) needs `com.apple.security.cs.allow-jit`. Which of `allow-jit` and `allow-unsigned-executable-memory` wasmtime 48 needs is UNVERIFIED; PL-M2 runs the extension tests under the signed binaries. `init-root` needs none: it runs `/usr/bin/sandbox-exec`, an Apple binary.
3. **Notarize**: zip the signed binaries and run `xcrun notarytool submit init-<v>-<target>.zip --keychain-profile init-notary --wait`, with the owner's stored profile. Bare binaries cannot be stapled, so Gatekeeper checks a quarantined copy online at first run. A stapled, signed `.pkg` ("Developer ID Installer" certificate, `productsign`) is an optional later channel for browser and offline installs.
4. **Repack** the signed files into the bundle; `MANIFEST.json` then records the signed hashes, so the owner signs the manifest last (§3.7.2).

What signing does **not** touch: bodies the agent builds itself are linked locally, ad-hoc signed and never quarantined, so they run as today, and Developer ID never enters the self-evolution loop. R-F3 (a new inode for every binary write) still applies.

**Recommendation:** enrol before the public release (V1_PLAN phase F), not before. v1.0 betas use the curl and tap paths unsigned. Quality guard: the macOS bundles are built and fully tested on the M2 from the tagged commit, and are covered by the signed manifest either way.

### 3.7 Build and CI

#### 3.7.1 Target matrix

| Target | Contents | Built on | Tested on | Release |
|---|---|---|---|---|
| `aarch64-apple-darwin` | full bundle | the owner's M2 (`ci/local.sh`, release tier) | M2: the full suite. Hosted `macos-15` per release tag: the A6 suite | v1.0 |
| `x86_64-apple-darwin` | full bundle | M2, cross (`rustup target add x86_64-apple-darwin`, an owner-approved toolchain change) | M2 under Rosetta: PL-M4 smoke | v1.0, best effort |
| `x86_64-unknown-linux-gnu` | full bundle, glibc 2.31 baseline | hosted `ubuntu-24.04` inside a `debian:11` container | hosted ubuntu-24.04 (disposable): full Linux suite; ubuntu-22.04 refusal check; Debian 12 and Fedora containers: smoke | v1.0 |
| `aarch64-unknown-linux-gnu` | full bundle | M2: Docker Desktop, `linux/arm64`, `debian:11` image ($0) | M2 Docker (`--privileged` container, disposable): the Linux suite | v1.0 |
| `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | INIT's binaries; the body as `*-gnullvm` (§3.5.3) | hosted `windows-2025` | hosted `windows-2025`: `cargo check` and portable tests from v1.0; the S-N suite and e2e smoke from v1.x | v1.x |
| `wasm32-wasip2` | the extension components | every host above | the bytes must match across hosts (PL-B4) | v1.0 |

- **gnu, not musl, for Linux**: ux-v1.md §3.2.2 owns the bundle and chose gnu, because the local toolchain builds gnu and the repro check compares like with like. A glibc 2.31 baseline (Debian 11's) covers Debian 11+, Ubuntu 22.04+ and Fedora; the refusal of Ubuntu 22.04 comes from bwrap's version, not glibc.
- `ring` (rustls) needs a C compiler for each target; the `debian:11` images provide gcc. On the arm64 image the build is native, so no cross toolchain is needed.
- Why the M2 builds aarch64 Linux: hosted arm64 runners for private repositories on the Free plan are UNVERIFIED, and Docker Desktop is already on the M2. The Rust toolchain lives in the image, not on the host (CONVENTIONS §1: never install toolchains on the owner's machine). The lane needs at least 30 GB free before it starts, stops below 20 GB, and removes its volume by exact name.

#### 3.7.2 Release tooling: INIT's own scripts, not cargo-dist

- **Decision:** extend `scripts/release.sh` per target, as ux-v1.md's UX-1 already plans, instead of adopting cargo-dist.
- **Why:**
  - INIT has four workspaces (kernel, root, seed, tui), seed and kernel-source bundles, docs, the WIT, the extension components and a prebuilt body tuple per target;
  - cargo-dist generates hosted CI that builds macOS on hosted runners at 10× minutes, while INIT builds macOS on the M2;
  - signing stays offline with the owner;
  - cargo-dist has no winget or Scoop output, and its maintenance status in 2026 is unclear (cross_platform.md §5, gaps).
- **What is borrowed:** artifact names `init-<version>-<target>.tar.gz` / `.zip`, the `curl | sh` and `irm | iex` installer shapes, and a manifest per artifact.
- **Assembly and signing:**
  1. CI (Linux x86_64) and the M2 (macOS, Linux aarch64) each produce an unsigned staging tree per target with its `MANIFEST.json`.
  2. The owner's machine fetches the CI tree as a workflow artifact and checks its file hashes against the manifest.
  3. The owner signs each manifest with the release key (minisign format, ux-v1.md §3.2.2) through `scripts/release-sign.sh`, and the archives are packed deterministically: `tar --sort=name --mtime=@$SOURCE_DATE_EPOCH --owner=0 --group=0 --numeric-owner`, then `gzip -n`.
  4. The owner or the orchestrator uploads them with `gh release upload`.
  **No signing key ever enters GitHub.**
- **Channels and package managers:** the Homebrew tap formula `nagisanzenin/homebrew-tap/init.rb` (v1.1 per ux-v1.md) points at the darwin and Linux archives (`on_macos`, `on_linux`); winget and Scoop follow native Windows (v1.x). winget needs a public pull request to `microsoft/winget-pkgs` with automated antivirus validation, and no signing. `.deb`, `.rpm`, AUR and Nix come later, AUR first.
- **A public download origin is required.** Release assets of a private repository need authentication. Until the owner decides OW-UX1, installs use `gh release download` with the owner's own login, then the installer with `INIT_DIST_BASE` pointing at a local directory (tests only), or the from-source `scripts/install.sh`.

#### 3.7.3 Workflows

| Workflow | Trigger | Runner | Jobs |
|---|---|---|---|
| `gate` (today's `fast-linux`, renamed; `seed-linux` folded in once green) | push to `main` and pull requests, code paths only (today's `paths-ignore`) | `ubuntu-24.04` (pinned, not `-latest`) | the portable kernel crates; the seed and the extension template |
| `linux-full` | nightly 02:00 UTC, only if `main` moved since the last run | `ubuntu-24.04` | the root workspace, kernel-supervise, kernel-serve, kernel-sandbox, kernel-proxy; S-L1…S-L19 and PL-L1…L12; the sweep and kill tests (a hosted runner is a disposable environment, testing.md §3.1a; the job writes the root-owned sentinel `/etc/init-test-disposable` with `sudo` first) |
| `windows-check` | weekly (Sunday) and manual during v1.0; nightly when `main` moved during native work | `windows-2025` | `cargo check --target x86_64-pc-windows-msvc` for every workspace; portable tests; renderer goldens; `install.ps1` tests with a stub `wsl.exe` |
| `release` | tag `v*` and `v*-rc*` | `ubuntu-24.04` (+ `debian:11` container), `windows-2025`, `macos-15` (final tags only) | the Linux x86_64 build and bundle; the Linux e2e smoke with `init-mockllm`; the `install.ps1` tests; the A6 suite on macOS 15 |
| `spike` | manual | `windows-2025` | PLAT-N0, PLAT-W3 |

Every workflow keeps `permissions: contents: read`, `concurrency` with `cancel-in-progress`, the `vendor/` and target caches keyed on lock files, and `timeout-minutes` on each job. No workflow holds a secret.

#### 3.7.4 GitHub Actions minutes on the Free plan

A private repository gets 2,000 included minutes a month. Windows minutes count 2× and macOS 10×, and each job rounds up to whole minutes. Self-hosted runners and public repositories cost nothing.

| Workflow | Billed minutes per run | Runs per month (estimate) | Billed per month |
|---|---|---|---|
| `gate` | 12 | 35 code pushes | 420 |
| `linux-full` | 25 | 20 nights with changes | 500 |
| `windows-check` (weekly in v1.0) | 15 × 2 = 30 | 5 | 150 |
| `release`: Linux build 20 + e2e 10 + `install.ps1` 8 × 2 + macOS 15 suite 8 × 10 | 126 | 2 | 252 |
| `spike` (N0, W3; one-off) | 20 × 2 = 40 | 3 | 120 |
| budget guard (1 min inside each scheduled job) | 1 | 30 | 30 |
| **total** | | | **≈ 1,470** (73% of 2,000; about 530 spare) |

- **Guard (`ci/minutes.sh`, first step of every scheduled and release job):** it lists this month's runs (`GET /repos/{owner}/{repo}/actions/runs?created=>=<YYYY-MM-01>`) and their jobs (`…/runs/{id}/jobs`), rounds each job's `completed_at − started_at` up to whole minutes, and multiplies by 2 for Windows labels and 10 for macOS labels. The `…/runs/{id}/timing` endpoint is not used: GitHub marks it as closing down.
  - ≥ 1,400 used: `linux-full` runs every other night and `windows-check` is skipped.
  - ≥ 1,700: only `gate` and `release` run.
  - ≥ 1,900: only `gate`; a release needs `workflow_dispatch` plus the owner's explicit approval of the overage (V1_PLAN: GitHub minutes beyond the free tier are an owner decision).
  A skipped job exits 0 with `skipped: minutes budget` in its summary, never a fake pass.
- **The development loop stays local.** Linux work iterates in the M2's Docker lane ($0, any number of runs); hosted runs are for what only a real distro shows: AppArmor, systemd, the Ubuntu kernels, Windows. macOS is built and tested on the M2 (`ci/local.sh`), so hosted macOS appears only once per final tag. Quality guard: every release tag still runs the full macOS suite on the M2 and the A6 suite on hosted macOS 15; only where they run changes.
- **If the repository becomes public** (OW-UX1), hosted minutes are free: run `linux-full` on every push, `windows-check` nightly, and macOS on every release candidate.

#### 3.7.5 Testing without a Windows machine

- **v1.0 (`wsl2` tier).** The Linux CI tests the code that runs in WSL. WSL-specific rows PL-W1…W4 stay UNVERIFIED until they run on a real Windows 10 or 11 machine; `init doctor` prints a WSL report the first Windows user can paste. PLAT-W3 tries WSL 2 on the hosted `windows-2025` image, whose release notes list a WSL 2.7 component; nested virtualization on hosted runners is UNVERIFIED. If it works, a WSL job joins `release`.
- **`install.ps1`**: on `windows-2025`, PSScriptAnalyzer lint plus runs against a stub `wsl.exe` on PATH that simulates "no WSL", "WSL 1 only", and "WSL 2 with Ubuntu", checking each exit code and message.
- **Native code health from v1.0**: `windows-check` keeps every workspace compiling for `x86_64-pc-windows-msvc` with `cfg` stubs, and runs the portable tests (init-proto, initctl's logic, kernel-gate, kernel-selfmap, the renderers' goldens), so the native work does not start from months of Unix-only drift.
- **Native v1.x**: the hosted runner is a disposable VM, so the S-N suite, the Job Object kill tests and the e2e smoke (install from the zip, the Root in the foreground, one `init` line answered by `init-mockllm`, a file written, `/undo`, `audit verify --full`) all run there at $0 in money and about 60 billed minutes per run, if spike Q7 passes.

### 3.8 The prebuilt body and reproducibility

ux-v1.md owns the owner-visible side: the bundle's prebuilt body (§3.2.2), `init-root install --prebuilt-body` (§3.5.1), the warm-up, the `repro` lineage record with results `verified` and `rebuilt_local`, and `root.repro_pending` (§3.5.3). This section fixes what the build side must do per target and what each target can be expected to give.

**Build-environment normalization** (the release and the kernel's G0 build must match; to kernel/supervise.md §3.6.2, as ux-v1.md §9 Q4 also proposes):
- the same command (`cargo build --frozen --bin init-body --profile dev`), the same vendored sources and the same pinned rustc 1.97.1;
- the body directory and `CARGO_HOME` remapped with `--remap-path-prefix=<slot>=/init/body` and `--remap-path-prefix=<CARGO_HOME>=/init/cargo` (which also keeps the owner's home path out of binaries);
- `SOURCE_DATE_EPOCH` set to the seed commit's time;
- the same target triple as the local build: `*-unknown-linux-gnu`, `*-apple-darwin`, or `*-pc-windows-gnullvm` (§3.5.3).

**Expected `repro` results per target** (expectations, measured by PLAT-4 before anyone relies on them):

| Target | Linker inputs | Expected on another machine | Why |
|---|---|---|---|
| `wasm32-wasip2` (extension components) | `rust-lld`, self-contained | `verified`, on any host OS | no system input reaches the link |
| `*-unknown-linux-gnu` | `cc` as the driver (x86_64 uses `rust-lld` by default since Rust 1.90, UNVERIFIED here; aarch64 uses the system linker) plus the system's glibc crt objects and `libc.so` script | `rebuilt_local` unless the machine has the build container's glibc and gcc | crt objects and the linker script differ by distro |
| `*-apple-darwin` | Apple's `ld` from the Command Line Tools; SDK version in `LC_BUILD_VERSION` | `verified` only with the same CLT version, else `rebuilt_local` | linker version, SDK version |
| `*-pc-windows-gnullvm` | `rust-lld` plus llvm-mingw's crt, both fetched at pinned versions | `verified` if both pins match (UNVERIFIED) | everything is pinned |
| `*-pc-windows-msvc` (INIT's own binaries only) | `link.exe` | not used for a repro check | PE timestamps and linker version |

**Trust does not rest on reproducibility.** The prebuilt body is trusted because its sha256 is in the signed `MANIFEST.json` and the Root refuses a mismatched blob. A `verified` result is a bonus that tells the owner this machine rebuilds the shipped agent bit for bit; `rebuilt_local` costs about a minute and keeps every later gate comparison local against local. Promotion always builds from source through G0–G4, as before.

**Across OSes.** A lineage, its tuples and its body binary belong to one machine and one target. The body's source (`body.git`) is portable, and the extension components should hash identically on macOS, Linux and Windows (PL-B4), which is a cheap cross-OS reproducibility signal on every release. Moving an `INIT_HOME` between operating systems is not supported in v1: the Root refuses a home whose `install.json.platform.os` differs from its own.

### 3.9 Work breakdown

Agent-hours are focused build plus verification. "Hosted" marks WPs that need GitHub-hosted runs (billed minutes, inside §3.7.4's budget); everything else runs on the M2 at $0. Every test is $0 in money (no model calls; `init-mockllm` where a model is needed). ux-v1.md's WPs consume these seams; hours are not counted twice: UX-1 (bundle), UX-2 (installers), UX-7 (warm-up), UX-8 (checkpoint store) and UX-16 (launcher-level Linux and Windows details) stay theirs.

**v1.0: common**

| WP | Scope | Depends on | Agent-h | Tests | Hosted |
|---|---|---|---|---|---|
| PLAT-0 | the `plat` traits and module tree; macOS backends by moving today's code (§3.3); `sys/` split; deny lists into `deny.rs`; rsafe audit paths | SBX-009 merged | 10–14 | PL-A1…A4; ci/local.sh 3× green | no |
| PLAT-1 | the Snapshot seam under UX-8's store: per-file clone or copy (`clonefile`, `FICLONE`), the walker, the race-safe restore primitives, exclusions | PLAT-0; with UX-8 | 6–9 | PL-S1…S6 (M2), PL-S7 | S7 (btrfs loop image) |
| PLAT-2 | the Keystore seam; `file` backend (as today); `secret-service` via `secret-tool`; `keys/index.json` | PLAT-0; UX-4's `keys.set` | 5–7 | PL-K1…K5 | K2, K5 |
| PLAT-3 | workflows of §3.7.3; `ci/minutes.sh`; the `debian:11` build container; the M2 Docker arm64 lane and release tier in `ci/local.sh`; `scripts/release-sign.sh`; deterministic packing | UX-1's per-target `release.sh` | 12–16 | PL-R1…R5 | yes |
| PLAT-4 | build normalization (remap, `SOURCE_DATE_EPOCH`, slot path); the reproducibility measurements per target; the cross-OS extension check | PLAT-3; UX-7 | 6–9 | PL-B1…B5 | B3, B4 |
| | **subtotal** | | **39–55** | | |

**v1.0: Linux**

| WP | Scope | Depends on | Agent-h | Tests | Hosted |
|---|---|---|---|---|---|
| PLAT-L0 | the whole tree builds on Linux; `cfg` gaps in init-root, sandbox-profiles and the kernel crates; portable tests green | — | 6–10 | `gate`, `linux-full` (build steps) | yes |
| PLAT-L1 | the bwrap launcher: zone args, the zone seccomp with namespace denies, fd-pinned binds, the split-bind home plan, `--args`/`--info-fd`, exec-clean with `close_range`, held kill and the pidns check, egress and owner peer checks, the in-namespace egress path | L0, PLAT-0 | 22–32 | PL-L1…L8, S-L1…S-L19 | M2 Docker first, then `linux-full` |
| PLAT-L2 | `init-kernel sandbox-inner` (landlock 0.4.7, seccompiler 0.5.0), with the live body's Landlock read rules matching the home plan | L0 | 8–12 | S-L8, S-L13, S-L15 | yes |
| PLAT-L3 | preflight and `doctor` rows (§3.4.6); the AppArmor copy; the bwrap version policy and fixed-package list | L1 | 5–7 | PL-L9 | yes (sysctl toggles) |
| PLAT-L4 | the systemd unit render and verify; the `manual` fallback; `KillMode` behaviour | PLAT-0 | 3–4 | PL-L10, PL-L11 | yes |
| PLAT-L5 | the Linux deny-list entries and the `contains_deny` refusal | PLAT-0 | 2–3 | PL-L2 | no |
| PLAT-L6 | the disposable safety suite: kill completeness, escapees, the bind race, fd hygiene | L1, L2 | 6–8 | PL-L5, L6, S-L12, S-L17 | yes |
| PLAT-L7 | the Linux bundles (x86_64 hosted, aarch64 M2) and the e2e smoke with the mock on ubuntu-24.04, Debian 12 and Fedora; the ubuntu-22.04 refusal | L1, L2, PLAT-3, UX-1 | 5–7 | PL-L13, PL-L14 | yes |
| | **subtotal** | | **57–83** | | |

**v1.0: WSL 2 and macOS**

| WP | Scope | Depends on | Agent-h | Tests | Hosted |
|---|---|---|---|---|---|
| PLAT-W1 | WSL detection, the `wsl2` tier, `doctor` rows, the drvfs hint, the interop check | L1, L3 | 4–6 | PL-W1…W4 (fixtures now; real WSL UNVERIFIED) | no |
| PLAT-W2 | `install.ps1` WSL mode and the `init.cmd` shim (a mode of UX-2's script) | UX-2 | 4–6 | PL-W5, PL-W6 | yes |
| PLAT-W3 | spike: WSL 2 on the hosted `windows-2025` image | — | 2–3 | pass or fail logged | yes |
| PLAT-M1 | Developer ID signing, notarization and entitlement scripts; signed-binary tests | UX-1; **OW-UX2 ($99, the owner's)** | 6–8 | PL-M1, PL-M2 | no |
| PLAT-M2 | the macOS 15 (and 26) A6 run per release tag; the version gate by self-test | PLAT-3 | 3–5 | PL-M3 | yes (10×) |
| PLAT-M3 | `x86_64-apple-darwin` cross build and the Rosetta smoke | UX-1 | 3–4 | PL-M4 | no |
| | **subtotal** | | **22–32** | | |

**v1.x: native Windows**

| WP | Scope | Depends on | Agent-h | Tests | Hosted |
|---|---|---|---|---|---|
| PLAT-N0 | **the MXC spike, Q1–Q13 (§3.5.4)** | — (can start today) | 10–14 | the spike report | yes (≈ 80 billed min) |
| PLAT-N1 | Root backends: Job Objects, named pipes and handle duplication, the control link, the task XML, Credential Manager and the DPAPI file, paths, deny lists, `sys/windows.rs` | N0 passes; PLAT-0, 1, 2 | 30–45 | PL-N1…N4 | yes |
| PLAT-N2 | kernel-proxy, kernel-serve and initctl on named pipes; the body's, TUI's and launcher's pipe clients | N1 | 20–30 | PL-N2, N3; the kernel suites on Windows | yes |
| PLAT-N3 | the body's tools on Windows (bash or PowerShell, paths, CRLF), the seed's tests, the `gnullvm` toolchain through `init-fetch` | N2; N0 Q10, Q11 | 20–30 | the seed suite on Windows | yes |
| PLAT-N4 | the MXC launcher, its policy renderer and the S-N suite | N0, N1 | 20–30 | PL-N5…N9 | yes |
| PLAT-N5 | the Windows bundles (x86_64, aarch64), `install.ps1` native mode, the e2e smoke | N1–N4; UX-1, UX-2 | 8–12 | PL-N10 | yes |
| | **subtotal** | | **108–161** | | |

**Totals.** v1.0 platform work: 118–170 agent-hours (common 39–55, Linux 57–83, WSL 2 and macOS 22–32). Native Windows (v1.x): 108–161. All: 226–331.

**Critical path for v1.0:** PLAT-0 → PLAT-L1 → PLAT-L6 → PLAT-L7, about 45–60 agent-hours. **Parallel lanes:**
- A, Linux sandbox: L0, L1, L2, L3, L6;
- B, state: PLAT-1, PLAT-2, L4, L5;
- C, release and CI: PLAT-3, PLAT-4, L7, M2, M3, W2;
- D, starts at once and is independent: PLAT-N0 and PLAT-W3 (hosted spikes), and PLAT-M1 once the owner decides OW-UX2.

## 4. Interfaces

### 4.1 The seams (Rust sketches)

INIT's own APIs; std only, no async (ADR-003). Third-party calls are named in §3 and pinned in each WP.

```rust
// init-root/src/plat/mod.rs (design sketch)
use std::{collections::BTreeMap, io, path::{Path, PathBuf}, time::Duration};
use init_sandbox_profiles::{ProfileId, access::Grant, params::ArgsError};

#[cfg(unix)]    pub type OwnedIo = std::os::fd::OwnedFd;
#[cfg(windows)] pub type OwnedIo = std::os::windows::io::OwnedHandle;

/// One instance to start. Built only after V1–V6 and the class rules passed (root/root.md §3.3.4).
pub struct LaunchSpec<'a> {
    pub instance: &'a str,                       // 12 hex (root/root.md §3.5.1 step 2)
    pub profile: ProfileId,
    pub params: &'a BTreeMap<String, String>,
    pub grant: &'a Grant,                        // ADR-009; Grant::default() for non-body profiles
    pub program: &'a Path,
    pub args: &'a [String],
    pub env: &'a BTreeMap<String, String>,       // complete: nothing is inherited
    pub cwd: &'a Path,
    pub stdio: [OwnedIo; 3],                     // the child's ends; the parent's ends go to the kernel
    pub keep: &'a [OwnedIo],                     // the kernel only: its end of the control link (and fd channel)
}

pub struct Rendered { /* sandbox-exec argv | bwrap arg list + seccomp program | MXC request */ }

pub trait ConfinedLaunch {
    /// Pure: the exact text, argv or policy. Golden-tested on every host.
    fn render(&self, spec: &LaunchSpec, zone: &ZoneLayout) -> Result<Rendered, ArgsError>;
    /// Start it. The returned hold is the only way the instance can ever be signalled.
    fn start(&self, spec: LaunchSpec, r: Rendered) -> io::Result<Held>;
    /// doctor and install rows; starts only short probes that exit on their own.
    fn preflight(&self) -> Vec<Check>;
}

/// A leader the Root started and has not reaped. Not Clone, not Copy, no constructor from a number:
/// ADR-008 lock 1 as a type rule. Unix: pid (+ pid-namespace inode on Linux). Windows: job + process handles.
pub struct Held { /* private */ }

pub enum Stop { Term, Kill }
pub enum Empty { Yes, No, Unknown }            // macOS: Unknown (held only); Linux: pidns; Windows: job count

pub trait HeldKill {
    fn signal(&self, h: &Held, how: Stop) -> io::Result<usize>;   // processes reached (leader counted)
    fn wait(&self, h: &Held) -> io::Result<()>;                   // until the leader exits; does not reap
    fn reap(&self, h: Held) -> io::Result<Exit>;                  // consumes the hold; exit and CPU time
    fn empty(&self, h: &Held, within: Duration) -> Empty;
}

pub enum Endpoint { Owner, Egress, Sealed(String) }               // Root-served, named
pub enum Allow<'a> { OwnerOnly, Kernel(&'a Held), Instance(&'a Held) }
pub struct Peer { pub pid: u32, pub sandboxed: bool }

pub trait LocalIpc {
    type Stream: io::Read + io::Write + Send + 'static;
    type Listener: Send;
    fn listen(&self, ep: &Endpoint, allow: Allow) -> io::Result<Self::Listener>;
    fn accept(&self, l: &Self::Listener) -> io::Result<(Self::Stream, Peer)>;   // peer check applied
    /// The control link: the Root's end, and the kernel's end to keep across the launch.
    fn link(&self) -> io::Result<(Self::Stream, OwnedIo)>;
}

/// Unix: one SCM_RIGHTS datagram on fd 4 (today's sys::send_fds).
/// Windows: DuplicateHandle into the kernel, then a `handles` frame on the link.
/// Either way it precedes the response line and the Root keeps no copy.
pub trait HandleTx { fn give(&self, tag: &str, io: Vec<OwnedIo>) -> io::Result<()>; }
// kernel side (kernel-supervise::sys): trait HandleRx { fn take(&self, tag: &str, t: Duration) -> io::Result<Vec<OwnedIo>>; }

pub enum Put { Cloned, Copied }
pub trait Snapshot {
    fn method(&self, store: &Path, ws: &Path) -> &'static str;     // "apfs" | "reflink" | "refs" | "copy"
    fn put_object(&self, src_dirfd: &OwnedIo, rel: &Path, dst: &Path) -> io::Result<Put>;
    fn restore_tree(&self, manifest: &Manifest, objects: &Path, ws: &Path) -> io::Result<RestoreStats>;
}

pub trait Service {
    fn kind(&self) -> &'static str;                                // "launchagent" | "systemd-user" | "logon-task" | "manual"
    fn unit_path(&self, p: &Paths) -> Option<PathBuf>;
    fn render(&self, p: &Paths) -> Option<String>;                 // pure; golden
    fn write(&self, p: &Paths) -> io::Result<Option<PathBuf>>;     // never loads (root/root.md §4.3 step 9)
    fn load_cmd(&self, p: &Paths) -> Vec<Vec<String>>;             // run by the launcher, with consent
    fn unload_cmd(&self, p: &Paths) -> Vec<Vec<String>>;
}

pub struct KeyId { pub kind: &'static str, pub id: String }      // ("provider", "deepseek"), ("oauth", …)
pub trait Keystore {
    fn kind(&self) -> &'static str;                                // "file" | "secret-service" | "credman" | "dpapi-file" | "keychain"
    fn get(&self, k: &KeyId) -> io::Result<Option<Secret>>;       // Secret: no Debug/Display/Serialize; zeroed on drop
    fn set(&self, k: &KeyId, s: &Secret) -> io::Result<()>;
    fn delete(&self, k: &KeyId) -> io::Result<bool>;
}

pub struct Paths {
    pub root_home: PathBuf, pub init_home: PathBuf, pub app_dir: PathBuf, pub bin_dir: PathBuf,
    pub home_real: PathBuf, pub service: Option<PathBuf>,
    pub read_deny: Vec<PathBuf>, pub write_deny: Vec<PathBuf>,
    pub case_insensitive: bool,                                    // Windows; case-insensitive APFS volumes report it too
}
```

### 4.2 `install.json.platform` and `status.platform`

Written at install, refreshed by `doctor`, published in `status` (ICJ; shown wrapped):

```json
{"platform":{"apparmor_userns_restrict":1,"arch":"x86_64","bwrap":"/usr/bin/bwrap","bwrap_version":"0.9.0",
  "egress":"enforced","keystore":"secret-service","landlock_abi":4,"launcher":"bwrap","os":"linux",
  "platform_cap":4,"service":"systemd-user","snapshot":"copy","sweep":"held_pidns","tier":"linux","v":1}}
```

- `tier` ∈ `macos`, `linux`, `wsl2`, `win-mxc`.
- `sweep` ∈ `held_only` (macOS without `live-sweep`), `d1` (macOS with it), `held_pidns` (Linux), `held_job` (Windows).
- `egress` ∈ `enforced`, `not_enforced`.
- The Root refuses an `INIT_HOME` whose recorded `os` differs from its own (§3.8).

### 4.3 Wire and environment changes

| Item | Change | Owner doc |
|---|---|---|
| `hello.paths.egress_sock` | Linux: `/run/init-root/egress.sock` (the in-namespace path) | root/root.md §4.1.3 |
| control link on Windows | `INIT_ROOT_HANDLE=<decimal>` replaces `INIT_ROOT_FD`/`INIT_ROOT_FDPASS`; or `INIT_ROOT_CTL_TOKEN` plus the pipe name if inheritance fails | root/root.md §4.1.1, §4.4 |
| `handles` event (Root → kernel, Windows) | `{"v":1,"ev":"handles","data":{"id","h":[…]}}`, sent before the response it belongs to | root/root.md §4.1.1 |
| `INIT_KERNEL_SOCK`, `INIT_PROXY_SOCK` (Windows) | pipe names `\\.\pipe\init-<id16>-{kernel,proxy}-<lane>` | kernel/supervise.md |
| `kill{mode:"term"}` on Linux | ends the namespace with `SIGKILL`; graceful stops use the stdio `stop` frame first | kernel/supervise.md, root/root.md §4.1.3 |
| `workspace.set` refusal | new `refused.code` `contains_deny` (Linux and Windows) | root/root.md §3.17.1 |
| charter `keys.store` | `auto` (default; resolves per OS: `file` on macOS until signed, `secret-service` or `file` on Linux, `credman` on native Windows), `file`, `os` | root/egress.md §3.2, ux-v1.md §3.4.5 |

### 4.4 Files and scripts

| Path | What |
|---|---|
| `root/crates/init-root/src/plat/{mod,macos,linux,windows,unix}.rs` | the seams and backends |
| `root/crates/init-root/src/sys/{mod,unix,macos,linux,windows}.rs` | FFI; the only `unsafe` |
| `root/sandbox-profiles/src/{bwrap,seccomp_zone,homeplan,mxc,service,deny}.rs` | pure renderers and lists, compiled on every host |
| `.github/workflows/{gate,linux-full,windows-check,release,spike}.yml` | §3.7.3 |
| `ci/minutes.sh` | the budget guard (§3.7.4) |
| `ci/local.sh` | gains `INIT_CI_TIER=release` (macOS bundles) and `INIT_CI_LINUX_ARM=1` (the Docker lane) |
| `scripts/release-sign.sh` | the owner's offline signing and deterministic packing (§3.7.2) |
| `scripts/sign-macos.sh` | `codesign` and `notarytool` steps, run by the owner (§3.6.2) |
| `scripts/get-init.ps1` | gains the WSL mode (§3.5.2) |

## 5. Failure modes and recovery

| # | Failure | Detection | Recovery | Owner sees |
|---|---|---|---|---|
| F1 | user namespaces blocked (AppArmor, a hardened kernel, an unprivileged container) | the namespace probe (§3.4.6) | the zone never starts | `ux.sandbox` with §3.4.6's exact fix |
| F2 | bwrap missing or older than 0.8.0 (Ubuntu 22.04) | `bwrap --version` | refuse | the distro's install or upgrade command |
| F3 | bwrap < 0.12.0 and not on the fixed list | version and package check | run, warn | a `doctor` warning naming GHSA-pxhw-h44j-8pfx |
| F4 | a bind source swapped for a symlink | `openat2` refuses (`ELOOP`/`EXDEV`) | refuse that spawn (`root.args`), alert | an alert: the kernel tried to redirect a mount (counted as a ceiling hit when the kernel is agent code) |
| F5 | home plan over 2,048 operations | at render | the home read is dropped for that spawn | `notice` `access` |
| F6 | Landlock missing | ABI probe | run; mounts and seccomp still hold | `doctor` info |
| F7 | Secret Service locked or gone | lookup fails or times out after 2 s | that provider answers `egress.key_locked`; retry every 60 s | one alert; `doctor` names the keyring |
| F8 | `FICLONE` fails (`EXDEV`, `EOPNOTSUPP`) | the ioctl's error | copy that file | nothing (method counted) |
| F9 | checkpoint store over its cap or the disk floor | the walk's totals | ux-v1.md §3.6.5's limits | `notice` `snapshot` |
| F10 | no systemd user manager | `systemctl --user` fails | `manual` service | screen 6's "start it now only" |
| F11 | the WSL VM stops | the Root's process ends | the next `init` starts it; the latch and LKG are files | the TUI reconnects |
| F12 | MXC missing or the build too old | `doctor` (build number, `mxc-sdk` probe) | native refuses; the WSL hint | `ux.sandbox` (Windows variant) |
| F13 | a named pipe busy or squatted | `FILE_FLAG_FIRST_PIPE_INSTANCE` fails, `ERROR_PIPE_BUSY` | the Root halts `Halted{tooling}` on a squatted name; clients retry on busy | an alert naming the pipe |
| F14 | a Job Object assignment fails | the API error | kill the suspended process by its handle; refuse the spawn | `root.internal` |
| F15 | notarization rejected | `notarytool` log | fix and resubmit; the curl path is unaffected | the owner's terminal |
| F16 | the minutes budget is reached | `ci/minutes.sh` | skips per §3.7.4; never a fake pass | the job summary |
| F17 | a hosted image changes (`-latest` moves) | pinned labels | update the pin in a reviewed commit | — |
| F18 | the Docker lane runs low on disk | free-space check before and during | stop below 20 GB; remove the volume by exact name | `ci/local.sh` output |
| F19 | `repro` mismatch or local build failure | ux-v1.md §3.5.3 | `rebuilt_local` or evolution off; chat unaffected | ux-v1.md's copy |

## 6. Performance notes (spec §9)

- **Spawn latency.** macOS: unchanged. Linux: bwrap plus the home plan; the plan adds about 1–3 ms (one `readdir` per expanded directory, one `openat2` per bind). Budget: a body spawn ≤ 150 ms p50 on ubuntu-24.04 (UNVERIFIED; PL-L1 measures it). Windows MXC: ≤ 300 ms p50 is the spike's pass bar (Q13).
- **Argument size.** The home plan rides `--args`, so a few hundred mount operations cost one pipe write and never meet `ARG_MAX`.
- **Checkpoints.** ux-v1.md's target is ≤ 150 ms p50 for a 20,000-file folder with a few changes. The stat cache keeps the incremental cost proportional to changed files. A first checkpoint hashes everything: sha2 at opt-level 3 runs roughly 0.5–1.5 GB/s, so about 2–4 s for 2 GiB, plus the copies on non-reflink volumes.
- **Egress.** One Unix socket hop on Linux, as on macOS. Named pipes on Windows are expected to be within a few percent of AF_UNIX for streaming responses (UNVERIFIED; PL-N measures it).
- **CI.** About 1,470 of 2,000 billed minutes a month (§3.7.4); the Linux development loop runs in Docker on the M2 at $0.

## 7. Invariants and security (spec §2.1; v0.3 §13–§19)

The invariants do not change; their enforcement per OS is §3.5.5's table. What this doc adds or makes explicit:

1. **The Root never starts a zone it cannot confine.** No tier runs without filesystem confinement of `ROOT_HOME` and the keys (`win-none` is refused), and no tier fails open (no degraded Linux mode; no unsandboxed fallback on Windows).
2. **A bind source is never resolved through a zone-writable path at mount time** (fd-pinned binds, §3.4.2). This closes a Linux-only boundary gap in root/root.md §3.4 that exists because, on Linux, "not mounted" is the boundary.
3. **No nested namespaces on Linux**, by two independent means: `--disable-userns` and the zone seccomp's namespace denies (§3.4.2).
4. **Held-only signalling on every OS**, now also as a type rule (`Held`). On Linux and Windows the hold is complete (pid namespace, Job Object), so ADR-008's escapee class exists only on macOS.
5. **A workspace never contains a deny entry on Linux and Windows** (`contains_deny`), so no plan path inside a zone-writable tree is ever re-resolved.
6. **No signing key in CI.** Release signing and notarization run on the owner's machine with the owner's credentials; workflows have `contents: read` and no secrets.
7. **The keystore is outside the zone's reach on every OS**: the zone has no mach lookups on macOS (Z7), no session bus on Linux (mount and network namespaces), and no vault on Windows (spike Q9; native does not ship without it).
8. **Snapshots never execute or interpret workspace content** (no git process; no filters, hooks or attributes) and never follow a link the agent planted.

**New or restated risks:**
- **The Secret Service has no per-application ACL.** Any unsandboxed process of the owner can read an unlocked keyring, exactly as it can read the 0600 key file; the gain is encryption at rest and keeping keys out of home backups.
- **WSL interop.** The zone's lack of `/init`, `/run/WSL` and `/mnt/c` is what stops it from launching Windows programs. PL-W1 must pass on a real WSL before v1.0 claims it; until then the `wsl2` tier's release note marks it UNVERIFIED.
- **MXC is two weeks old.** Its enforcement details (Q5, especially reparse points and persistent ACL edits) decide whether native Windows ships at all.
- **Late-created deny entries** are invisible to running instances on Linux (§3.4.4). That is the safe direction: new owner files at expanded levels appear only at the next spawn.
- **Full mode on Linux** cannot create files directly in `~`. That is stricter than macOS, and is documented.

## 8. Test plan

Every row is $0 in money. "M2" is the owner's machine (live-safe: signals only held pids, testing.md §3.1a); "Docker" is the M2's disposable container; "hosted" is a GitHub runner VM, which is disposable.

| Id | Test | Where |
|---|---|---|
| PL-A1 | macOS goldens: composed SBPL and `sandbox-exec` argv for every profile × grant × lane, byte-identical across PLAT-0 | M2 |
| PL-A2 | tests/rsafe.rs green with the renamed call sites; no new signalling site | M2 |
| PL-A3 | the plist and the systemd unit render byte-identical to today's for fixture `Paths` | M2, hosted |
| PL-A4 | the resolved macOS deny parameters are identical; `Held` has no public constructor (a compile-fail doc test) | M2 |
| PL-S1 | checkpoint round trip on a fixture: files, empty directories, symlinks (in and out of the tree), exec bits, non-UTF-8 and Unicode names | M2, hosted |
| PL-S2 | a second checkpoint of an unchanged 10,000-file tree hashes 0 files and writes 0 objects | M2 |
| PL-S3 | exclusions: `target/` beside `Cargo.toml` and `node_modules/` are recorded, not stored, and untouched by a restore | M2 |
| PL-S4 | a restore removes files created after the checkpoint, replaces (never follows) a symlink the "agent" planted at a directory's place, and touches nothing outside the workspace (inotify or FSEvents witness on the parent) | M2, hosted |
| PL-S5 | the `pre_undo` checkpoint makes an undo undoable | M2 |
| PL-S6 | limits and GC: 2 GiB, 256 MiB per file, 200,000 files; GC removes only unreferenced objects, by exact path | M2 |
| PL-S7 | reflink objects on a btrfs loop image; `EXDEV` (store and workspace on different filesystems) falls back to copies | hosted |
| PL-K1 | the `file` backend is unchanged (egress key tests) | M2 |
| PL-K2 | `secret-tool` round trip under `dbus-run-session` with `gnome-keyring-daemon`; the secret never appears in `/proc/<pid>/cmdline` or the environment | hosted |
| PL-K3 | no Secret Service → `file`, recorded in `keys/index.json` | hosted |
| PL-K4 | a locked keyring → `egress.key_locked`, one alert, a retry 60 s later (fake clock) | hosted |
| PL-K5 | from a body instance, `/run/user/<uid>/bus` is absent and an abstract-bus connect fails | hosted |
| PL-L1 | home-plan golden from a fixture home tree (deny entries at depth 1 and 2, symlinks, a workspace two levels down, full mode); spawn time measured | M2 (pure), hosted |
| PL-L2 | inside a live body: read `~/Documents/x` ok; `~/.ssh/id` and `~/.local/share/keyrings` `ENOENT`; write the workspace ok; write `~/y` `EROFS`; full mode: write `~/proj/z` ok, create `~/new` `EROFS`, `~/.config/systemd/user/a` `EROFS`; a workspace containing `.ssh` is refused `contains_deny` | hosted |
| PL-L3 | an entry created at an expanded level after spawn is invisible until the next spawn | hosted |
| PL-L4 | `--info-fd` gives the pid-namespace inode, which equals `/proc/<child>/ns/pid` | Docker, hosted |
| PL-L5 | fd hygiene: the program sees only fds 0–2 (plus 3–4 for the kernel); bwrap's 5–7 are closed | Docker, hosted |
| PL-L6 | the bind race: a renamer swaps a probe root's `work` for a symlink to a decoy secret in a loop during 10,000 probe spawns; no instance ever sees the decoy | hosted (disposable) |
| PL-L7 | egress: only the kernel instance reaches `/run/init-root/egress.sock`; a body's connect fails; the egress server refuses a peer outside the kernel's pid namespace | hosted |
| PL-L8 | kill completeness: a body forks a `setsid` and a double-fork child; `kill{mode:"kill"}` leaves no process with the namespace inode within 1 s | hosted (disposable) |
| PL-L9 | Ubuntu 24.04: sysctl 1 without a profile → refusal with §3.4.6's exact text; with `bwrap-userns-restrict` → pass, and the inner stage works; sysctl 0 → pass | hosted (disposable) |
| PL-L10 | `systemd-analyze --user verify` on the rendered unit; `enable --now` starts the Root; `systemctl --user restart` kills the zone and the latch survives | hosted (disposable) |
| PL-L11 | no systemd user manager → `manual`; the launcher starts the Root on demand | Docker |
| PL-L12 | with the zone seccomp, `unshare(CLONE_NEWUSER)`, `clone(CLONE_NEWUSER)` and a raw `clone3` fail, while cargo, git, rustc, the body's tests and wasmtime work; repeated on bwrap 0.6.1 (ubuntu-22.04) without `--disable-userns`, to decide v1.0.x support | hosted |
| PL-L13 | the bundle on ubuntu-24.04, Debian 12 and Fedora: `doctor` rows recorded; ubuntu-22.04 refused with the bwrap message | hosted |
| PL-L14 | e2e smoke: install from the archive, first run scripted, one turn answered by `init-mockllm` that writes a file, `/undo` restores it, `audit verify --full` exits 0 | hosted, Docker |
| PL-W1 | in WSL: starting `cmd.exe` or `/mnt/c/Windows/System32/whoami.exe` from a body fails | a real WSL (UNVERIFIED until run) |
| PL-W2 | in WSL: a workspace under `/mnt/c` is refused with the hint | real WSL; logic on fixtures |
| PL-W3 | in WSL: systemd present → `systemd-user`, absent → `manual`; the VM's lifetime with only the user service running | real WSL |
| PL-W4 | `doctor` reports `tier: wsl2` from the interop marker | fixtures, real WSL |
| PL-W5 | `install.ps1` against a stub `wsl.exe`: no WSL, WSL 1 only, WSL 2 with Ubuntu; exit codes and messages | hosted |
| PL-W6 | the `init.cmd` shim passes arguments and the translated cwd | hosted (stub), real WSL |
| PL-M1 | signed binaries: `codesign --verify --strict --verbose=2`; `notarytool log` accepted | M2 (after OW-UX2) |
| PL-M2 | the extension tests and a selfmap run under the hardened-runtime signed `init-body` and `init-kernel` with the chosen entitlement | M2 (after OW-UX2) |
| PL-M3 | the A6 suite on hosted `macos-15` (and `macos-26` once available) per final tag | hosted |
| PL-M4 | `x86_64-apple-darwin` bundle under Rosetta: `doctor`, the e2e smoke with the mock | M2 |
| PL-R1 | `MANIFEST.json.sig` verifies with the public key compiled into `init-fetch`; a flipped byte in any file is refused | M2, hosted |
| PL-R2 | deterministic packing: packing the same staging tree twice gives identical archives | M2, hosted |
| PL-R3 | `ci/minutes.sh` on recorded API fixtures: rounding, multipliers, thresholds, and the skip summary | M2 |
| PL-R4 | no workflow has a secret or a write permission (a lint over `.github/workflows`) | M2 |
| PL-R5 | the release workflow on an `rc` tag produces staging trees whose manifests list every file | hosted |
| PL-B1 | the Root refuses a prebuilt body whose sha256 differs from the signed manifest | M2 |
| PL-B2 | two builds of the body on one host in different slot paths are identical (remap works) | M2, Docker, hosted |
| PL-B3 | the release-built Linux x86_64 body against a rebuild on ubuntu-24.04 outside the container: result recorded | hosted |
| PL-B4 | the extension components built on macOS, Linux and Windows hosts are byte-identical | M2, hosted |
| PL-B5 | the M2-built macOS body against a rebuild on hosted `macos-15`: result recorded (`verified` or `rebuilt_local`) | hosted (10×, once) |
| PL-N1…N10 | native Windows (v1.x): Job kill completeness; lane pipes refuse another instance's SID; the zone cannot open the owner pipe; the handle hand-over; egress (TCP, UDP, DNS, loopback all fail); the deny list and `ROOT_HOME` unreadable; the Startup folder unwritable; Credential Manager unreachable; the bind-equivalent reparse-point race; e2e smoke | hosted (disposable) |

Existing suites stay: S-M, S-B, S-G, S-Z on macOS; S-L1…S-L19 now actually run (hosted and Docker).

## 9. Open questions

### 9.1 Cross-doc

| # | To | Proposal | Recommended |
|---|---|---|---|
| X1 | root/root.md §3.4, kernel/sandbox.md §3.6, §3.13 | fd-pinned binds; the zone seccomp namespace denies; `--args`, `--info-fd`, `--assert-userns-disabled`; the split-bind home plan for ADR-009 grants; Linux `hello.paths.egress_sock`; `kill{mode:"term"}` semantics | yes (closes a Linux boundary gap) |
| X2 | root/root.md §3.13.1 | `cap = min(charter.max_level, SUPPORTED_MAX_LEVEL, platform_cap)` and the `not_enforced` handling (§3.5.5) | yes |
| X3 | ADR-009, root/root.md §3.17.1 | per-OS deny lists (§3.4.5); the COMMON additions `.git-credentials`, `.cargo/credentials.toml`, `.config/gh`, `.config/hub` on every OS, macOS included; the `contains_deny` refusal on Linux and Windows | yes |
| X4 | kernel/sandbox.md §3.8 | accept a macOS version when the install-time A6 self-test passes, instead of refusing every untested version | yes |
| X5 | kernel/supervise.md §3.6.2 | `--remap-path-prefix` for the slot and `CARGO_HOME`, `SOURCE_DATE_EPOCH` (same as ux-v1.md §9 Q4) | yes |
| X6 | ux-v1.md §3.6.5 | the checkpoint store's platform rules of §3.4.8: no git process, race-safe restore, the regenerable-cache exclusions, `.git/` included | yes |
| X7 | root/egress.md §3.2, ux-v1.md §3.4.5 | `keys.store = "auto"`: Secret Service then file on Linux (the owner's request); Credential Manager then DPAPI file on native Windows; file on macOS until signed | yes |
| X8 | ux-v1.md §3.2.1, §3.2.2, §3.11 | Windows v1.0 = `install.ps1` as a WSL bootstrapper and no native bundle; native `msvc` bundles in v1.x with a `gnullvm` body; the §3.11 rows for sandbox, owner socket (named pipes), checkpoints and `notify.cmd` (none) | yes |
| X9 | root/root.md §4.1.1, §4.4 | Windows control link (`INIT_ROOT_HANDLE`, `handles` event, the token fallback) | at PLAT-N1 |
| X10 | body/provider.md, body/tools.md, kernel/proxy.md, tui.md | named-pipe clients and servers on Windows; the bash tool via Git Bash or PowerShell; the `egress: not enforced` status field | at PLAT-N2/N3 |
| X11 | root/root.md §4.8 | budget lines for `plat/` and `sys/` (Linux ≈ 600–900 lines with the renderers counted on sandbox-profiles' own line; Windows ≈ 1,500–2,000), measured + 5% at release as for ADR-009 | yes (the total is a provisional owner decision) |
| X12 | testing.md §3.1a | hosted runner jobs write the Linux sentinel themselves with `sudo` (they are disposable by construction); Docker on the M2 counts as disposable | yes |
| X13 | CONVENTIONS.md §4 | a registry row for this doc (platform layer, tiers, platform cap, build matrix and CI budget) | yes |

### 9.2 Owner

| # | Decision | Recommended |
|---|---|---|
| O1 (= OW-UX2) | Apple Developer Program, $99/yr | enrol before the public release; v1.0 betas ship unsigned through curl and the tap |
| O2 (= OW-UX1) | a public download origin (the repo public, or a public releases-only repo) | the repo public at phase F; curl, the tap and winget all need it |
| O3 | Windows v1.0 through WSL 2, native in v1.x behind the MXC spike | yes |
| O4 | GitHub Actions minutes beyond the free tier | not needed: the plan uses about 1,470 of 2,000; the guard stops before 2,000 |
| O5 | a Windows 11 24H2 machine or VM, only if MXC does not run on the hosted Server 2025 image (spike Q7) | decide after PLAT-N0 |
| O6 | the M2's Docker lane: one ~2 GB image pull and up to ~10 GB while it runs | yes, under the 20 GB floor rule |

### 9.3 Internal

- **I1.** GHSA-pxhw-h44j-8pfx (bwrap < 0.12.0): its preconditions decide between "warn" and "refuse" for older bwrap; PLAT-L3 reads the advisory and the distros' backports.
- **I2.** Whether bwrap mounts a `/proc/self/fd/<n>` source as the pinned inode (PL-L6); the fallback is in §3.4.2.
- **I3.** `--info-fd`'s field names in the distro bwrap versions (PL-L4).
- **I4.** Hosted arm64 Linux runners for private repositories on the Free plan; if available and cheap, aarch64 Linux moves from the M2 to CI.
- **I5.** wasmtime and the body on `*-windows-gnullvm` (spike Q11).
- **I6.** Whether the 200,000-file checkpoint limit should count excluded trees (proposed: no).

## 10. Spec deltas

| Source | This doc | Why |
|---|---|---|
| root/root.md §3.4: bind sources by path; bwrap ≥ 0.8.0 is the only nested-namespace guard; no ADR-009 grants on Linux | fd-pinned binds; zone seccomp namespace denies; the split-bind home plan | Linux's boundary is the mount view, so it must not be redirectable |
| root/root.md §3.4: bwrap ≥ 0.8.0 | unchanged for v1.0; warn below 0.12.0; Ubuntu 22.04 (0.6.1) refused until PL-L12 | the distro reality and a 2026 escape fix |
| root/root.md §3.13.1: `cap = min(max_level, SUPPORTED_MAX_LEVEL)` | adds `platform_cap` | honest guarantees per tier |
| root/root.md §3.17.1 | `contains_deny` on Linux and Windows | no plan path inside a zone-writable tree |
| ADR-009 Decision 1 (macOS paths only) | per-OS deny lists; four COMMON additions | Linux and Windows have their own secret stores; tokens found missing on every OS |
| ADR-008: `status.sweep` `held_only`/`d1` | adds `held_pidns` and `held_job` | Linux and Windows holds are complete |
| kernel/sandbox.md §3.8: refuse untested macOS versions | accept a version whose install-time A6 self-test passes | users get new macOS versions before INIT's CI does |
| ux-v1.md §3.2.1: native `install.ps1` in v1.0 once lane P's sandbox lands | v1.0 Windows = WSL 2 bootstrap; native in v1.x | MXC is new, untested on available hardware, and the native port is about 110–160 agent-hours |
| ux-v1.md §3.4.5: file by default everywhere | Secret Service first on Linux; Credential Manager first on native Windows | the owner's request; the code-identity concern applies to macOS Keychain only |
| ux-v1.md §3.4.9: the MSVC linker on Windows | `gnullvm` with llvm-mingw fetched by `init-fetch` (spike Q11) | no admin rights and no Visual Studio licence |
| cross_platform.md §6: musl static Linux builds | gnu with a glibc 2.31 baseline | ux-v1.md's bundle decision; like-with-like repro |
| cross_platform.md §6: Windows Tier B (elevated) as the fallback | not planned for v1 | persistent users and firewall rules need their own consent design |

## 11. References

Checked 2026-10-20 unless stated otherwise. The research note `research_notes/INIT v1 providers UX vision/cross_platform.md` (2026-10-20) carries the full source list for §§1–6 of that note; the ones this doc leans on are repeated here.

- MXC: Windows Developer Blog, "Microsoft Execution Containers" (2026-10-07), https://blogs.windows.com/windows/2026/10/07/microsoft-execution-containers-policy-driven-containment-for-ai-agents/ (via cross_platform.md)
- MXC repository README (process container minimum Windows 11; policy areas), https://github.com/microsoft/mxc (checked 2026-10-20)
- MXC OS-version support (builds 26100.9278, 26200.9278, 26300.9550, 28000.x; no Windows Server mention), https://github.com/microsoft/mxc/blob/main/docs/backends/process-container/os-version-support.md (checked 2026-10-20)
- `mxc-sdk` on crates.io (1.0.0 on 2026-10-06, 1.0.1 on 2026-10-17), https://crates.io/crates/mxc-sdk (checked 2026-10-20)
- `mxc-sdk` API index (`v1::spawn`, `v1::ContainerRequest`, `v1::MxcProcess`; no handle or job accessor documented), https://docs.rs/mxc-sdk/latest/mxc_sdk/ (checked 2026-10-20)
- Codex Windows sandbox docs (MXC builds, `allow_local_binding`, elevated and unelevated modes), https://learn.chatgpt.com/docs/windows/windows-sandbox (via cross_platform.md)
- bwrap(1), bubblewrap 0.13.1 (`--args`, `--add-seccomp-fd`, `--disable-userns`, `--assert-userns-disabled`, `--info-fd`, `--json-status-fd`, `--symlink`, `--remount-ro`; no `--bind-fd`), https://manpages.debian.org/unstable/bubblewrap/bwrap.1.en.html (checked 2026-10-20)
- bubblewrap NEWS (0.11.2, 0.12.0: setuid removed, GHSA-pxhw-h44j-8pfx), https://cdn.jsdelivr.net/gh/containers/bubblewrap@0.13.0/NEWS.md (via cross_platform.md)
- Debian bubblewrap 0.12.0 changelog (kernel floor), https://tracker.debian.org/media/packages/b/bubblewrap/changelog-0.12.0-1deb13u1 (via cross_platform.md)
- Ubuntu AppArmor user-namespace restriction, https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007 (via cross_platform.md)
- Claude Code sandboxing docs (the bwrap AppArmor recipe; WSL 2; native Windows unsandboxed), https://code.claude.com/docs/en/sandboxing (via cross_platform.md)
- Codex linux-sandbox README, https://github.com/openai/codex/blob/main/codex-rs/linux-sandbox/README.md (via cross_platform.md)
- Landlock documentation (ABI list, limits), https://docs.kernel.org/userspace-api/landlock.html (via cross_platform.md)
- ioctl_ficlone(2), https://man7.org/linux/man-pages/man2/ioctl_ficlone.2.html (via cross_platform.md)
- ReFS block cloning, https://learn.microsoft.com/en-us/windows-server/storage/refs/block-cloning (via cross_platform.md)
- Job Objects, https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects (via cross_platform.md)
- AF_UNIX on Windows (no ancillary data, no socketpair), https://devblogs.microsoft.com/commandline/af_unix-comes-to-windows/ (via cross_platform.md)
- DuplicateHandle, https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-duplicatehandle (via cross_platform.md)
- rust-lang/rust#147335 (Windows Unix sockets, unstable), https://github.com/rust-lang/rust/pull/147335 (via cross_platform.md)
- Syncthing autostart (systemd user services, Task Scheduler options), https://docs.syncthing.net/users/autostart.html (via cross_platform.md)
- Apple Developer Program fee, https://developer.apple.com/programs/whats-included/; notarizing bare binaries (no stapling), https://developer.apple.com/forums/thread/721117 (via cross_platform.md)
- Homebrew 5.0 cask Gatekeeper deprecation, https://workbrew.com/blog/what-homebrew-5-0-0-means-for-your-mac-fleet (via cross_platform.md)
- winget submission, https://learn.microsoft.com/en-us/windows/package-manager/package/repository (via cross_platform.md)
- Rust platform support tiers, https://doc.rust-lang.org/rustc/platform-support.html (via cross_platform.md)
- cargo-dist installers, https://axodotdev.github.io/cargo-dist/book/installers/index.html (via cross_platform.md)
- GitHub Actions billing (included minutes, multipliers), https://docs.github.com/en/billing/concepts/product-billing/github-actions (via cross_platform.md)
- GitHub REST "Get workflow run usage" (`/timing`; marked as closing down), https://docs.github.com/en/rest/actions/workflow-runs?apiVersion=2022-11-28#get-workflow-run-usage (checked 2026-10-20)
- GitHub runner-images release notes for `win25` (WSL 2.7.x component listed; `windows-latest` moving to Windows Server 2025), https://newreleases.io/project/github/actions/runner-images/release/win25%2F20260803.218 (checked 2026-10-20; WSL distro support on hosted runners not confirmed)
