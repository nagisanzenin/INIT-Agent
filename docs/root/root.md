# The Root: `init-root`, whole-system recovery, lineage, charter and autonomy ledger

```
Status: v0.3 + build sync 2026-10-20 · Area: root
        v0.3 reconciliation applied: reconciliation/v03-decisions.md D1, D4, D6–D9, D13 (ledger: v03-root.md)
        ADR-009 (Codex-style permissions, v0.1.1, owner decision 2026-10-20): §3.17, §3.12, §4.2
        M7 fold (m7-design.md §10; provisional owner decisions 2026-10-20): §3.7, §3.10.5, §3.12.2, §3.13.2, §4.1.3, §4.8
Depends on: INIT Agent spec.md (v0.3 Part II §12–§19; Part I §2.1, §7), adr/007-root-and-autonomy-ladder.md,
            adr/009-codex-style-permissions.md,
            CONVENTIONS.md (v0.3, incl. §1 live-system safety), reconciliation/v03-decisions.md,
            kernel/supervise.md, kernel/sandbox.md, kernel/proxy.md, kernel/config.md, kernel/initctl.md,
            kernel/gate.md, kernel/selfmap.md, body/log-context.md (ICJ v1, §3.8.1), evolution.md, awareness.md,
            root/egress.md, root/sealed-eval.md, testing.md
Defines:    the Root process and trust model; ROOT_HOME layout and the Root file rules (R-F1–R-F6, R-X);
            agent-zone confinement (profile composition, the zone tail, the `kernel` profile, Linux zone
            arguments); the spawn broker for every agent-zone process and the instance sweep (D1); the Root
            supervisor state machine; whole-system tuples, the LKG and audited-good pointers, the blob store;
            whole-system rollback; system crash-loop escalation, system safe mode, the R1 pause and Halted;
            the Root side of kernel upgrades; `lineage/kernel.git` and the kernel source bundles (D7); panic
            channels, latch, effects and resume; `stop`; R4 lineage record format, kinds, dedup key, signing,
            verification, published view and off-machine mirror; R5 charter schema, validation and apply flow;
            R7 level computation, unlock criteria, demotion triggers and procedure, per-level permissions, the
            L2 evolution-share bound; the parallel-body and instance quotas; the owner channel
            (ROOT_HOME/run/owner.sock, `init-root attach`); the control link (Root ↔ kernel); the `init-root`
            CLI; the fake clock and test ledgers (test builds only); Root alerts and fault points; the size
            budget and dependency list; (v0.1.1, ADR-009) workspace validation, the access state, workspace
            snapshots and `/undo`, the charter keys `sandbox.read_home`, `read_deny`, `allow_full`
```

---

## 1. Purpose

`init-root` is the small, immutable program that keeps control of INIT while the agent owns everything else (spec §12–§14, ADR-007). It runs as the owner's OS user, outside every sandbox. The boundary between the Root and the agent zone (the kernel plus everything the kernel runs) is the only security boundary in v0.3.

This doc owns four of the Root's seven responsibilities:
- **R1**, stop and roll back the whole system;
- **R4**, tamper-evident lineage;
- **R5**, the charter;
- **R7**, the autonomy ledger.

It also owns the parts every responsibility rests on: the process model, how the agent zone is confined, the Root ↔ kernel control link, the owner channel and the CLI. R2 and R3 (egress, keys, metering, money ceilings, external-action classes) belong to root/egress.md. R6 (sealed tasks, mutation suite, gate-of-gates, sealed audit) belongs to root/sealed-eval.md. Both run inside the same `init-root` process and are reached through the internal calls named here.

One measured fact shapes the design: **macOS Seatbelt sandboxes do not nest.** On this machine (macOS 14.6, 2026-10-20), `sandbox-exec` run inside even an `(allow default)` sandbox fails with `sandbox_apply: Operation not permitted` (exit 71). A sandboxed kernel therefore cannot put its bodies into their own sandboxes. So **the Root starts every agent-zone process itself**, on the kernel's request, and hands the stdio pipes to the kernel. The kernel still decides what runs and when. The Root decides what any of it can touch.

## 2. Responsibilities / non-goals

### 2.1 Responsibilities

1. **Process and trust model.** Run as the owner, own `ROOT_HOME`, confine the agent zone (§3.1–§3.4).
2. **Spawn broker.** Start, reap, kill and sweep every agent-zone process. Enforce the instance, parallel-body, CPU and disk quotas (§3.5, §3.14).
3. **R1:**
   - supervise the kernel process;
   - keep whole-system **tuples** (kernel build + body binary + extension set) with LKG and audited-good pointers;
   - perform whole-system rollback;
   - run crash-loop escalation, system safe mode and `Halted`;
   - drive the Root side of kernel upgrades;
   - handle panic and stop (§3.6–§3.9).
4. **R4:** the signed, hash-chained lineage: its verification, its published view and its off-machine mirror (§3.10–§3.11).
5. **R5:** the charter schema, validation, apply flow and publication (§3.12).
6. **R7:** the autonomy level, the unlock criteria, demotion and the per-level permissions (§3.13).
7. The **owner channel**, the CLI and owner alerts (§3.15, §4.2, §4.3, §4.6).

### 2.2 Non-goals (owned elsewhere; used here by name)

| Concern | Owner |
|---|---|
| Egress socket protocol, provider keys and action credentials (`keys/providers/`, `keys/actions/`), the usage ledger (`egress/`), metering, money-ceiling enforcement, the `demotes` classification of each egress refusal (D6), sealed lanes, external-action classes (R2, R3) | root/egress.md |
| Sealed task set S, mutation suite M, gate-of-gates, sealed audit, δ_S, task admission (R6) | root/sealed-eval.md |
| Body supervision: body state machine, overlapped handoff, body LKG, probes, G4, body safe mode, git authority over `lineage/body.git` | kernel/supervise.md |
| Text of the `body`, `build`, `check` and `git-read` profiles; the Linux inner stage `init-kernel sandbox-inner` | kernel/sandbox.md |
| Model routing, response caching, traces, the kernel's soft caps | kernel/proxy.md |
| ICJ v1 (canonical JSON) | body/log-context.md §3.8.1 |
| Gate stages and the gate record | kernel/gate.md |

### 2.3 Split of duties with kernel/supervise.md

The Root supervises the kernel, and the kernel supervises the bodies. The kernel keeps all decisions about bodies. The Root performs the mechanisms that a sandboxed process cannot perform, and holds the state that must survive a bad kernel.

| Concern | Root (this doc) | Kernel (kernel/supervise.md) |
|---|---|---|
| Who is supervised | the one kernel process | live bodies, probes, builds, git reads |
| Spawning | performs every spawn (sandbox launch, marks, fd hand-over, reaping) | decides when and what; sends `spawn` (§4.1) |
| Killing | performs kills and instance sweeps | requests them (`kill`) |
| Known-good state | whole-system tuples; LKG and audited-good pointers; the blob store | body LKG (`lkg.json`, `bin/init-body.lkg`), provisional chain, G4 |
| Rollback | whole-system (installs a tuple's kernel and hands the kernel its body and extension set) | body rollback (handoff to body LKG) |
| Crash loops | kernel crash loop → tuple rollback → demotion and system safe mode → `Halted` (retried automatically, D4) | body crash loop → body safe mode, reported as `body_safe{on:true}`; it ends when the Root restarts the kernel in normal mode |
| Panic | every channel, the latch, killing everything | none: the kernel is simply killed |
| Owner I/O | `owner.sock`, `init-root attach`, `/panic`, relay, replay ring | inbox, relay to bodies, the kernel's own owner lines (`/stop`, `/resume`, `/status`, `/accept`, `/bless`, `/reject`, `/later`, `/kernel`; D4) |
| Lineage | R4, the authoritative signed chain; `lineage/kernel.git`, the kernel source the zone can only read (D7, §3.8.1) | `lineage/body.git` (objects and refs), the kernel's own candidate area, `vault/lineage.jsonl` (kernel-local detail, not authoritative) |
| Service manager | `dev.init.root` LaunchAgent / `init-root.service` | none: the kernel is no longer a launchd or systemd job |
| Owner alerts | delivery (notices, `alerts.jsonl`, `notify.cmd`) | produces kernel alerts and sends them as `alert` |

## 3. Design

### 3.1 Process and trust model

```
launchd (LaunchAgent dev.init.root) / systemd --user (init-root.service)
└── init-root start                          owner uid · no sandbox · cwd ROOT_HOME
    ├── threads  supervisor · control-link reader · owner server (+1 per client) · signal · watcher (1 s)
    │            · reaper (1 per instance) · mirror · egress (root/egress.md) · sealed (root/sealed-eval.md)
    └── children  one sandbox instance each, own process group, always started via `init-root exec-clean`
        ├── kernel    profile `kernel`    $INIT_HOME/kernel/bin/init-kernel supervise --under-root   (exactly 1)
        ├── body      profile `body`      live bodies (≤ parallel-body quota of the level, §3.14)
        ├── probe     profile `body`      candidate bodies for G2, G3 and G4 runs
        ├── build     profile `build`     cargo for candidates and stub builds
        ├── check     profile `check`     gate acceptance checks
        ├── git_read  profile `git-read`  sandboxed git-upload-pack
        └── sealed    profile `body`/`check` + the sealed tail (sealed audits, gate-of-gates, family evaluations)
```

| Zone | Processes | Trusted as | Writes | Network |
|---|---|---|---|---|
| **Root** | `init-root` and its own children: `git`/`ssh` for the mirror, `ssh-keygen` at install, `notify.cmd` | the owner | `ROOT_HOME`, plus the Root-written parts of `INIT_HOME` (R-F1) | unrestricted: egress upstreams (root/egress.md), mirror pushes |
| **Agent zone: kernel** | `init-kernel` and its children (git, tar) | maintainer code at L0–L2; agent code from L3 | `INIT_HOME` minus the Root-written parts (§3.3.2) | `ROOT_HOME/run/egress.sock` only |
| **Agent zone: everything else** | bodies, probes, builds, checks, git reads | agent | per kernel/sandbox.md | per kernel/sandbox.md (kernel sockets only) |

**Rules.**
- **Same uid.** The Root and the zone share the owner's uid, so Unix modes are hygiene only. The boundary is the OS sandbox: Seatbelt on macOS; bubblewrap, seccomp and namespaces on Linux. It holds because every zone process is started by the Root under a policy that the Root composes and the zone never authors.
- **R-X.** The Root never executes a zone-supplied program outside a zone sandbox. Programs the Root runs unsandboxed are fixed:
  - `ROOT_HOME/bin/init-root`
  - `/usr/bin/sandbox-exec`
  - `/usr/bin/bwrap`
  - the configured `git`
  - `/usr/bin/ssh` and `/usr/bin/ssh-keygen`
  - `/usr/bin/tmutil`
  - `notify.cmd`
- **No inherited fds.** A zone process inherits only the fds named in its spawn: 0–2, plus 3–4 for the kernel (§3.5.2).
- **Zone data reaches the Root only two ways:** as bytes on the control link, or as fds opened *inside* the zone and passed over the fd channel (R-F2). So a symlink planted by the zone can never make the Root read or write the wrong file.

A separate uid for the Root was rejected. It would need administrator rights at install, contradict spec §13 ("the owner's OS user"), and add nothing the tested sandbox does not already give.

### 3.2 Files

#### 3.2.1 `ROOT_HOME` (default `~/.init-root`)

`ROOT_HOME` must be:
- an absolute, symlink-free path of at most 64 bytes, so `ROOT_HOME/run/egress.sock` fits `sun_path` (macOS 104 bytes, Linux 108). A `test-fixtures` build allows 80 bytes, because the repo's test homes are 66 bytes or more and 80 still fits `sun_path` for `run/*.sock`;
- outside `INIT_HOME`, and `INIT_HOME` must be outside it;
- outside every read root of the zone's inner profiles (the system paths of kernel/sandbox.md §3.3.1 section 4, `/opt/homebrew`, the toolchain, the developer directory) and outside every charter `sandbox.extra_read` entry. Tail rule Z1 denies `ROOT_HOME` anyway; this placement rule is a second layer. From v0.1.1 the live body and fleet members read the owner's home, which contains `ROOT_HOME` (ADR-009), so for them Z1 and the deny list (Z2, which names `ROOT_HOME` again) are the layers that hold.

`init-root install` and every start check this (R-F5).

The environment variable `INIT_ROOT_HOME` and the CLI flag `--root-home` override the default location.

```
$ROOT_HOME/                          0700 · agent zone: nothing, not even metadata (tail rule Z1)
├── bin/init-root                    0755  the binary launchd/systemd runs; also the exec-clean shim
├── charter.toml                     0600  the owner's working copy (edited by the owner, applied explicitly)
├── install.json                     0600  paths fixed at install: init_home, toolchain, devdir, git, platform
├── keys/                            0700  excluded from Time Machine (tmutil addexclusion)
│   ├── lineage.ed25519              0600  32-byte Ed25519 seed (R4)
│   ├── lineage.pub                  0644  64 lowercase hex + "\n"
│   ├── mirror_ed25519{,.pub}        0600/0644  the Root's deploy key for the off-machine mirror (§3.11)
│   ├── known_hosts                  0600  host key of the mirror remote (trust on first use)
│   ├── providers/<provider_id>      0600  provider API keys        ┐ owned by root/egress.md
│   └── actions/<class_id>           0600  action-class credentials ┘ (§3.2 there)
├── egress/                          0700  root/egress.md (usage ledger, sealed-lane usage)
├── lineage/                         0700
│   ├── lineage.jsonl                0600  the authoritative R4 chain (append-only)
│   ├── head.json                    0600  {seq, hash, verified_full_at}
│   ├── idem.jsonl                   0600  dedup keys of kernel appends (§3.10.3)
│   └── mirror.git/                  bare repo pushed to the owner's private remote
├── store/                           0700
│   ├── blobs/<sha256 hex>           0444  kernel binaries, body binaries, Wasm components, kernel-state snapshots
│   └── tuples/<tuple id>.json       0444
├── state/                           0700
│   ├── root.json                    0600  supervisor snapshot (§4.5)
│   ├── charter.active.toml          0600  the applied charter
│   ├── ledger.json                  0600  R7 state (derivable from lineage)
│   ├── owner-queue.jsonl            0600  owner input not yet acknowledged by a kernel
│   └── proposals/<id>.md            0600  agent proposals (never applied automatically)
├── sealed/                          0700  root/sealed-eval.md (sealed runs use private roots here, §3.3.6)
├── snapshots/                       0700  v0.1.1: APFS clones of the workspace, `<ts>-<name>/`, last 5 per workspace (§3.17.4)
├── cache/git-home/                  0700  HOME for the Root's git (empty)
├── logs/                            0700  root.jsonl, kernel-<seq>.stderr (4 MiB × 10), launchd.log
├── alerts.jsonl                     0600
└── run/                             0700
    ├── owner.sock                   0600  owner channel (§4.2); the zone can never connect (tail Z6)
    ├── egress.sock                  0600  the zone's only network path (given to the kernel as `hello.paths.egress_sock`); protocol: root/egress.md
    ├── canary                       0400  empty; every zone instance is denied it (sweep condition c, §3.5.4)
    ├── root.lock  root.pid
    └── panic                        the panic latch; exists only while panicked (§3.9)
```

#### 3.2.2 The Root's part of `INIT_HOME`

kernel/supervise.md §3.1 remains authoritative for `INIT_HOME`. These rows change ownership in v0.3:

| Path | v0.2 | v0.3 | Zone access |
|---|---|---|---|
| `kernel/` | kernel, owner-installed | **Root-written** from the release (§4.3): `bin/`, `docs/` (the kernel docs the agent reads, including the selfmap recipes), `kupgrade-smoke/` fixtures, `wit/`, `CONSTITUTION.md` from the charter. The owner still edits `kernel/config.toml` directly. | read-only (tail Z4) |
| `kernel/seed/<tag>.bundle` (new) | — | Root-written git bundles of the maintainer's seed, the kernel's only seed source | read-only |
| `kernel/src/<version>.bundle` (new, D7) | — | Root-written git bundle of the maintainer's kernel *source* for each installed kernel release | read-only |
| `lineage/kernel.git` (new, D7) | — | **Root-written** bare repo of the kernel source: the maintainer's releases and every agent kernel the Root has installed (§3.8.1). Created at install, at every level, so the kernel the system runs is always readable. | read-only (tail Z4) |
| `root/` (new) | — | **Root-written view**: `charter.json`, `lineage.jsonl`, `lineage.pub`, `ledger.json`, `ceilings.json`, `status.json` (§4.5) | read-only (tail Z4); the `body` profile reads it too (kernel/sandbox.md, X2 settled) |
| `run/mark/<id>`, `run/mark/decoy`, `run/zone-mark` | kernel | **Root-written**. Each instance may read only its own mark. The decoy is a sibling every instance is denied (sweep condition c, §3.5.4). Every instance may read the zone mark. | tail Z4, Z5 |
| `run/owner.sock`, `run/panic` | kernel | **removed**: they move to `ROOT_HOME/run/` | — |
| `vault/keys/` | kernel | **removed**: they move to `ROOT_HOME/keys/providers/<provider_id>` (root/egress.md §3.2) | — |
| everything else | kernel or agent | unchanged: internal to the zone (fleet roots, `run/fleet/`, `run/spawn.sock`, the kernel's own kernel-candidate area included) | per kernel/sandbox.md |

#### 3.2.3 File rules

- **R-F1, writes.** The Root writes only in these places:
  - `ROOT_HOME`;
  - `INIT_HOME/kernel/`, `INIT_HOME/root/`, `INIT_HOME/lineage/kernel.git`, `INIT_HOME/run/mark/` and `INIT_HOME/run/zone-mark`, all of which the zone can read but not write (tail Z4);
  - during `install` only, and only before any zone process has existed, the empty `INIT_HOME` skeleton.

  It never writes a path the zone can write. The one exception, from v0.1.1, is `/undo` restoring the owner's workspace, under §3.17.4's rules.
- **R-F2, reads of zone data.** The Root reads zone data only as control-link bytes, through fds passed over the fd channel (§4.1.1), and through the sandboxed upload-pack that fetches an admitted kernel commit (§3.8.1). There is one exception: the disk quota walks `INIT_HOME` reading metadata only (`symlink_metadata`, never following links) and calls `statvfs` (§3.14). From v0.1.1 there is a second: the workspace's size walk and its snapshot clone (§3.17.4).
- **R-F3, Root-written files.** Files are written with `write_atomic` (kernel/supervise.md §3.12), which uses `O_NOFOLLOW`. Binaries are always written as a new inode and then renamed into place, because of macOS code-signing caching (kernel/supervise.md §3.6.2). Directories are fsynced after each rename.
- **R-F4, secrets.** This covers the lineage key, the mirror key and egress's provider keys and credentials. Each is a 0600 regular file, opened with `O_NOFOLLOW`, and must satisfy `st_uid == geteuid()` and `st_mode & 0o077 == 0` (the rule of kernel/proxy.md §3.3). Secrets are never printed, never passed as argv and never put in environment variables.
- **R-F5, layout check at every start.** Every one of these must hold:
  - `ROOT_HOME` and its subdirectories are real directories owned by the uid, mode 0700;
  - the path-length and nesting rules of §3.2.1 hold;
  - every top-level directory of the `INIT_HOME` skeleton, and `lineage/kernel.git`, is a real directory. Other entries in `INIT_HOME` are not checked, so a file the zone creates there cannot halt the Root (a denial of service).

  Any violation → `Halted{layout}`.
- **R-F6, JSON.** Every Root JSON file and frame is ICJ (body/log-context.md §3.8.1): sorted keys, integers only, one line plus `\n`.

### 3.3 Confining the agent zone on macOS

#### 3.3.1 Profile composition

```
profile(id) = PRELUDE          kernel/sandbox.md §3.3.1 sections 1–4 (identical for every profile)
            + SECTION(id)      `kernel`: §3.3.3 below · `body`: kernel/sandbox.md §3.3.1 sections 5–8 ·
                               `build`/`check`: §3.4 there · `git-read`: §3.5 there
            + ROOT_ALLOWS      the instance mark, the zone mark, and one line per charter `sandbox.extra_read`
            + TAIL             the zone tail (§3.3.2); deny-only, always last
profile(kind sealed) = PRELUDE + the sealed text (§3.3.6): no SECTION and no ROOT_ALLOWS
```

Every text is compiled into `init-root` with `include_str!`. The kernel chooses a profile **by id** and supplies **parameter values**. It never supplies profile text. Two tests run on 2026-10-20 make this mandatory:

- **SBPL is evaluated as Scheme.** A profile containing `(define (deny . args) #t)` turns every later `deny` into a no-op: a file denied after that line was still readable. A zone that could author profile text could therefore switch off the tail.
- **The last matching rule wins.** `(deny file-read* X)(allow file-read* X)` allows X. The tail therefore comes last, and nothing follows it.

As a consequence, any change to a profile text, including the texts kernel/sandbox.md owns, ships as a Root release. Kernel versus body is a stability boundary, not a security one (spec §13). Even so, keeping its text in the Root costs nothing at runtime and removes a whole class of zone-authored policy.

v0.1.1 (ADR-009) adds the last three lines: the Root-generated grants of §3.17.2, a home read (`Z_HOME`) for a body that has one and a write grant (`Z_WRITE`) for the live body, both excluding `Z_INIT_HOME`. The block is `root/sandbox-profiles/profiles/root-allows.template` at SBX-009's 8570638, copied verbatim; `tests/doc_drift.rs` compares it byte for byte, so change the file and this block together.

```scheme
; ---- R. init-root instance allows (rendered between SECTION(id) and the tail) ----
(allow file-read-data (literal (param "Z_MARK")) (literal (param "Z_ZONE_MARK")))
; one line per charter sandbox.extra_read entry i (0 ≤ i < 8), only when the list is non-empty (D9):
(allow file-read* file-map-executable (subpath (param "Z_EXTRA_READ_<i>")))
(allow file-read-metadata (path-ancestors (param "Z_EXTRA_READ_<i>")))
; ADR-009 read grant, a body only: the owner's home except INIT_HOME (the tail still denies the Root's paths and the read deny list):
(allow file-read* file-map-executable (require-all (subpath (param "Z_HOME")) (require-not (subpath (param "Z_INIT_HOME")))))
; ADR-009 write grant, the live body only: Z_WRITE is the workspace, or the home in full mode; never INIT_HOME:
(allow file-read* file-write* file-map-executable (require-all (subpath (param "Z_WRITE")) (require-not (subpath (param "Z_INIT_HOME")))))
(allow file-read-metadata (path-ancestors (param "Z_WRITE")))
```

The `extra_read` lines are Root-generated from the owner's charter (§3.12.1), never from the kernel. The paths travel as `-D` parameters, so no path is ever spliced into profile text and no escaping is needed. kernel/config.md's `[sandbox] extra_read` is replaced by this charter key (§9.1 X2).

```rust
// init-root/src/zone.rs
const PRELUDE: &str = include_str!("../profiles/prelude.sb");   // kernel/sandbox.md §3.3.1 §§1–4
const ZONE_TAIL: &str = include_str!("../profiles/zone-tail.sb");
const SEALED_TAIL: &str = include_str!("../profiles/sealed-tail.sb"); // as built: `TAIL` in sealed/run.rs (§3.3.6)
pub enum ProfileId { Kernel, Body, Build, Check, GitRead }
fn render(id: ProfileId, sealed: bool, extra_read: usize) -> String {
    if sealed { return [PRELUDE, SEALED_TAIL].concat(); }          // instance kind `sealed` (§3.3.6)
    [PRELUDE, section(id), &root_allows(extra_read), ZONE_TAIL].concat()
}
// argv: ["/usr/bin/sandbox-exec", "-p", &text, "-DNAME=value"…, "--", program, args…]
```

#### 3.3.2 The zone tail (exact text)

Z2 changed in the v0.3 reconciliation (D9): it now denies only the Root's own paths, so the owner's `extra_read` paths work. Reads of everything else are confined by the inner profiles, which are Root-embedded and `deny default`, together with parameter validation (V3, §3.3.4). The zone cannot widen either. The tail keeps its full strength for the Root's home, writes, the network, marks, XPC and signals. Over an inner profile of `(version 1)(allow default)`, the worst possible, those rules all held (§3.3.5).

**v0.1.1 (ADR-009): Z2 and Z3 change.**
- **Z2** gains one deny line per deny-list entry (`Z_DENY_<i>`: ADR-009 Decision 1's built-in list plus the charter's `sandbox.read_deny`), denying reads and writes in every access mode, and a write deny on the service file's directory (`Z_ROOT_SVC_DIR`, `~/Library/LaunchAgents`), so no mode can install a login job. The home read is a ROOT_ALLOWS grant (§3.3.1), so this tail rule is what keeps `~/.ssh` and the other entries out of reach, even under an inner `(allow default)`. It applies to every zone profile, the kernel's included.
- **Z3** gains `(subpath (param "Z_WRITE"))`: writes are allowed inside `INIT_HOME`, the devices and `Z_WRITE`, which is the Root-validated workspace (`workspace` mode) or the owner's home (`full` mode). With no write grant (`read_home` and `none`, members, probes, the kernel and gate instances) the scope is v0.1.0's. Z1, Z2 and Z4 still deny `ROOT_HOME`, the deny list and the Root-written parts of `INIT_HOME` inside that scope: the tail is deny-only and nothing follows it, so Z3 not matching a path never allows it, and any other tail rule that matches denies it.
- **Z3a** (new): one write deny per entry (`Z_WDENY_<i>`), in every mode, for what the owner's own tools run or load unsandboxed: the shell startup files (`.zshrc`, `.zprofile`, `.zshenv`, `.zlogin`, `.zlogout`, `.bashrc`, `.bash_profile`, `.bash_login`, `.bash_logout`, `.profile`, `.config/fish`), git's user config (`.gitconfig`, `.config/git`), the PATH directories `.local/bin` and `bin` (whole, so `init` and `init-tui` too), login items (`Library/LaunchAgents`, `.config/autostart`, `.config/systemd/user`, `.config/environment.d`, `.pam_environment`), and `install.json`'s `bin_dir` (`init-root install --bin-dir`, §4.3).
- **Z3b** (new): no writes to `.git/hooks`, `.git/config`, or `.git/modules/**/{hooks,config}` in any repository outside `INIT_HOME` (regex rules). The rest of `.git` stays writable, so the agent can commit; `INIT_HOME` is exempt, because the kernel clones and configures its own repositories there.
- **ROOT_ALLOWS** (§3.3.1) gains two Root-generated grant lines: a read of the home (`Z_HOME`) and a write of `Z_WRITE`. Both exclude `Z_INIT_HOME`, so inside `INIT_HOME` the instance SECTION still rules; even in full mode a body cannot write `bin/`, `lineage/` or `state/kernel/`.
- The parameters are the Root's, like every `Z_*` value (V1). The kernel sends the same `spawn` parameters as in v0.1.0 and no new one. The workspace comes from the TUI's cwd after §3.17.1's validation, the mode from the Root's access state (§3.17.2), and the deny entries from the built-in list and the charter. Which instance gets which grant is §3.17.2's table.

The block is `root/sandbox-profiles/profiles/zone-tail.sb` at SBX-009's cc16577, copied verbatim; `tests/doc_drift.rs` compares it byte for byte, so change the file and this block together.

```scheme
; ==== Z. init-root zone tail (deny-only; always appended last) ====
; Z1. the Root's home is invisible (connecting to the egress socket is a network operation; Z6 leaves it open)
(deny file-read* file-write* (literal (param "Z_ROOT_HOME")) (subpath (param "Z_ROOT_HOME")))
; Z2. the Root's paths outside ROOT_HOME (its service file, no writes in its directory) and the owner's secrets
;     (the ADR-009 read deny list, also in full mode); every other read is the inner profile's (D9)
(deny file-read* file-write* (literal (param "Z_ROOT_SVC")))
(deny file-write* (literal (param "Z_ROOT_SVC_DIR")) (subpath (param "Z_ROOT_SVC_DIR")))
; one line per read deny entry i: the built-in list, then the charter's sandbox.read_deny:
(deny file-read* file-write* (literal (param "Z_DENY_<i>")) (subpath (param "Z_DENY_<i>")))
; Z3. writes only inside INIT_HOME and the instance's write grant Z_WRITE (ADR-009: the live body's workspace, or the
;     home in full mode; INIT_HOME for every other instance), plus the devices every profile needs
(deny file-write*
  (require-not (require-any
    (subpath (param "Z_INIT_HOME")) (subpath (param "Z_WRITE")) (literal "/dev/null") (literal "/dev/zero")
    (literal "/dev/dtracehelper") (literal "/dev/ptmx") (regex #"^/dev/ttys[0-9]+") (subpath "/dev/fd"))))
; Z3a. never what the owner's own tools run or load unsandboxed (ADR-009 §9.3 Q5), in every mode: shell startup
;     files, git's user config, INIT's launchers and PATH directories, login items; one line per write deny entry i:
(deny file-write* (literal (param "Z_WDENY_<i>")) (subpath (param "Z_WDENY_<i>")))
; Z3b. nor git's hooks and config in any repository outside INIT_HOME (the rest of .git stays writable, so the agent can commit)
(deny file-write* (require-all (require-not (subpath (param "Z_INIT_HOME")))
  (require-any (regex #"/\.git/hooks(/|$)") (regex #"/\.git/config$") (regex #"/\.git/modules/.+/hooks(/|$)") (regex #"/\.git/modules/.+/config$"))))
; Z4. never into the Root-written parts of INIT_HOME; INIT_HOME and run/ cannot be renamed or replaced
(deny file-write* (literal (param "Z_INIT_HOME")) (literal (param "Z_RUN")) (literal (param "Z_ZONE_MARK"))
  (literal (param "Z_KERNEL")) (subpath (param "Z_KERNEL"))
  (literal (param "Z_PUB")) (subpath (param "Z_PUB"))
  (literal (param "Z_MARKS")) (subpath (param "Z_MARKS"))
  (literal (param "Z_KGIT")) (subpath (param "Z_KGIT")))
; Z5. an instance reads only its own mark (other marks and the decoy are denied)
(deny file-read* (require-all (subpath (param "Z_MARKS")) (require-not (literal (param "Z_MARK")))))
; Z6. no IP at all; Unix sockets only inside INIT_HOME, plus the Root egress socket
(deny network-outbound
  (require-not (require-any
    (remote unix-socket (path-literal (param "Z_EGRESS_SOCK")))
    (remote unix-socket (subpath (param "Z_INIT_HOME"))))))
(deny network-bind (require-not (local unix-socket (subpath (param "Z_INIT_HOME")))))
(deny network-inbound (require-not (local unix-socket (subpath (param "Z_INIT_HOME")))))
; Z7. no XPC services; no signals or inspection outside the instance's own sandbox
(deny mach-lookup (xpc-service-name-prefix ""))
(deny signal (require-not (target same-sandbox)))
(deny process-info* (require-not (target same-sandbox)))
```

All tail parameters are chosen by the Root. A kernel-supplied parameter whose name starts with `Z_` is refused (V1, §3.3.4).

| Param | Value |
|---|---|
| `Z_ROOT_HOME` · `Z_ROOT_SVC` | `ROOT_HOME` · the service file (`~/Library/LaunchAgents/dev.init.root.plist`; Linux `~/.config/systemd/user/init-root.service`) |
| `Z_INIT_HOME` · `Z_RUN` · `Z_KERNEL` · `Z_PUB` · `Z_KGIT` | `INIT_HOME` · `/run` · `/kernel` · `/root` · `/lineage/kernel.git` |
| `Z_MARKS` · `Z_MARK` · `Z_ZONE_MARK` | `INIT_HOME/run/mark` · `INIT_HOME/run/mark/<instance id>` · `INIT_HOME/run/zone-mark` |
| `Z_EGRESS_SOCK` | `ROOT_HOME/run/egress.sock` |
| `Z_EXTRA_READ_<i>` | the charter's `sandbox.extra_read[i]` (ROOT_ALLOWS only) |
| `Z_ROOT_SVC_DIR` (v0.1.1) | the service file's directory (`~/Library/LaunchAgents`; Linux `~/.config/systemd/user`): write-denied by Z2 for every instance |
| `Z_DENY_<i>` (v0.1.1) | the deny list: ADR-009's built-in entries resolved under the owner's home (a symlinked entry also by its literal path), then the charter's `sandbox.read_deny`; at most 128 lines. Read- and write-denied by Z2 |
| `Z_WDENY_<i>` (v0.1.1) | the write deny list of Z3a: the built-in entries under the owner's home, then `install.json`'s `bin_dir`. Write-denied for every instance in every mode |
| `Z_HOME` (v0.1.1) | the owner's home (`HOME_REAL`); only for an instance with a read grant (§3.17.2), and only in ROOT_ALLOWS |
| `Z_WRITE` (v0.1.1) | the write grant: the workspace, or the home in full mode, for the live body; `INIT_HOME` for every instance without a grant, so Z3 then reduces to v0.1.0's |

Z3 and Z4 together mean the zone can write only zone-internal paths, which the Root never opens by name (R-F2). From v0.1.1 the live body can also write the workspace (or the home, under `--full`), which the Root opens for snapshots and `/undo` under the rules of §3.17.4. Z4's literals on `INIT_HOME` and `run/` stop the zone from replacing `run/` with a symlink before the Root creates a mark in it. Z4's `run/` literal also denies `file-write-mode`, so the zone cannot chmod `run/` itself, even to its current mode (measured, FIX1). The Root therefore keeps creating `INIT_HOME/run` at 0700 at install, and the kernel chmods a lane directory only when its mode differs, so it never touches `run/`. The `/opt/homebrew/var` and `/opt/homebrew/etc` deny (ESC-2) is not a Root path. It now lives in each inner profile's section 8: in kernel/sandbox.md's `body` profile, and in §3.3.3 for `kernel`.

#### 3.3.3 The `kernel` profile (sections 5–8)

```scheme
; ---- 5. INIT_HOME: read everything except other instances' marks; write everything
;         (the tail removes kernel/, root/, lineage/kernel.git, run/mark/, run/zone-mark and the anchors) ----
(allow file-read-metadata (path-ancestors (param "INIT_HOME")))
(allow file-read* (require-all (subpath (param "INIT_HOME")) (require-not (subpath (param "MARKS")))))
(allow file-map-executable (subpath (param "INIT_HOME")))
(allow file-write* (subpath (param "INIT_HOME")))
; ---- 6. network: serve sockets under run/ (live, fleet and spawn lanes) and probe/; connect only to the Root egress socket ----
(allow system-socket (socket-domain AF_UNIX))
(allow network-bind (local unix-socket (subpath (param "RUN"))) (local unix-socket (subpath (param "PROBE"))))
(allow network-inbound (local unix-socket (subpath (param "RUN"))) (local unix-socket (subpath (param "PROBE"))))
(allow network-outbound (remote unix-socket (path-literal (param "EGRESS_SOCK"))))
; ---- 6a. G0's commands run as the kernel's children: Unix sockets under the build slot's OUT only, as in `build` ----
(allow network-bind (local unix-socket (subpath (param "SLOT_OUT"))))
(allow network-outbound (remote unix-socket (subpath (param "SLOT_OUT"))))
; ---- 8. hard denies (the tail follows) ----
(deny file-read* file-write* (subpath "/opt/homebrew/var") (subpath "/opt/homebrew/etc"))
(deny mach-lookup (xpc-service-name-prefix ""))
```

The Root sets the kernel's parameters itself. The kernel cannot spawn a `kernel` instance.

| Param | Value |
|---|---|
| `INIT_HOME` · `RUN` · `PROBE` · `MARKS` | `INIT_HOME` · `/run` · `/probe` · `/run/mark` |
| `EGRESS_SOCK` | `ROOT_HOME/run/egress.sock` (the kernel learns it from `hello.paths.egress_sock`) |
| `SLOT_OUT` | `INIT_HOME/cache/build/slot/out`, the G0 build slot's `OUT` (kernel/sandbox.md §3.4) |
| `TOOLCHAIN` · `DEVDIR` · `HOME_REAL` | as in kernel/sandbox.md §3.3.2 (the prelude uses them) |

**Section 6a (provisional owner decision, 2026-10-20).** G0's commands (kernel/gate.md §3.3) do not yet run in a separate `build` instance: kernel-gate starts them as the kernel's own children, so they inherit this profile (WP8b deviation 1). The seed's tests serve and connect Unix sockets (the `init-mockllm` provider tests, in-test stubs), so section 6a gives this profile the same socket rule the `build` profile has (kernel/sandbox.md §3.4): bind and connect Unix sockets whose path lies under the slot's `OUT`, nothing else. No IP, no other socket. Remove 6a once G0 runs in `build` instances through the broker.

What the kernel cannot do:
- Reach any IP address.
- Talk to the keychain or to any XPC service.
- Signal a body, because each body is a different sandbox. It asks the Root (`kill`) instead.
- Read the Root's files, another instance's mark or the decoy.
- Write `lineage/kernel.git`, `kernel/` or the published view.

What it can do:
- Run `git` (the hardened prefix of kernel/supervise.md §3.11.2) and `tar`.
- Read `lineage/kernel.git`, including the source of the kernel it runs.
- Serve `kernel.sock`, `proxy.sock`, `spawn.sock`, the fleet lane sockets under `run/fleet/<bid>/`, and the probe lane sockets.
- Serve and connect Unix sockets under `cache/build/slot/out/` (section 6a: the tests G0 runs as its children).
- Reach the model providers, but only through the egress socket. Under v0.3 the kernel's proxy sends its upstream requests to the Root egress gateway as HTTP/1.1 over that socket (root/egress.md); TLS and keys live only in the Root.

#### 3.3.4 Parameters and their validation

The kernel's `spawn` request names a profile id and a parameter map (§4.1.3). The Root validates every value. For writes, the network and the Root's paths, containment does not depend on this check, because the tail holds whatever the parameters are. For reads it does (D9): V3 is what keeps a parameter from pointing an inner profile's read rule outside `INIT_HOME`. On Linux the parameters also become bind mounts (§3.4).

| Rule | Check | Error |
|---|---|---|
| V1 | Name matches `^[A-Z][A-Z0-9_]{0,31}$`, does not start with `Z_`, and is not `MARK` (the Root sets `MARK = Z_MARK`). At most 32 parameters. | `root.args` |
| V2 | Value is absolute and UTF-8, ≤ 1024 bytes, with no NUL and no `.` or `..` component. It is canonical: an existing path equals `fs::canonicalize(value)`, and for a missing path its parent does. **Exception for `LINK_*` (D8):** both `LINK_*` forms are accepted. A `LINK_*` equal to its target (`KERNEL`, `LINEAGE` or `BIN`) is a real canonical directory and passes the ordinary rule. A symlink is accepted only inside a probe or member root: there the *parent* must be canonical, the last component is kept literal, and `lstat` must show a symlink or nothing. The live root's `LINK_*` must equal its target. A symlink's target does not matter: Seatbelt checks resolved paths, so the rules for the target apply anyway. | `root.args` |
| V3 | Value lies under `INIT_HOME`, `TOOLCHAIN` or `DEVDIR`, or under one of the charter's `sandbox.extra_read` entries, or equals the owner's home (`HOME_REAL`, used only by a metadata rule) | `root.args` |
| V4 | `program` is absolute. `cwd` lies under `INIT_HOME`. `env` has ≤ 128 entries of ≤ 32 KiB each and no `INIT_ROOT_*` key. `args` ≤ 256 entries of ≤ 32 KiB each. | `root.args` |
| V5 | The (kind, profile) pair is allowed: `body`/`probe` → `body`; `build` → `build`; `check` → `check`; `git_read` → `git-read`. The profile `kernel` and the kind `sealed` are never available to the kernel. | `root.args` |
| V6 | Linux only: a parameter used as a *writable* bind lies under `INIT_HOME` and outside `kernel/`, `root/`, `lineage/kernel.git` and `run/mark/` | `root.args` |

#### 3.3.5 Test evidence (macOS 14.6, 23G80, M2, 2026-10-20)

All experiments used scratch files and processes this session created. Every process was stopped by the PID it was started with. No sweep, pattern kill or broad predicate ran on the live system (CONVENTIONS §1).

| Check | Profile | Result |
|---|---|---|
| nested `sandbox-exec` inside `(allow default)` | — | fails, `sandbox_apply: Operation not permitted`, exit 71 |
| `(define (deny . args) #t)` before a deny | `(allow default)` | the denied file is readable: redefinition defeats later rules |
| deny then allow of the same path | `(allow default)` | allowed: the last match wins |
| read and list `ROOT_HOME`; read the service file | tail; `kernel` | `EPERM` |
| read a file outside `INIT_HOME` that is not a Root path; read `~/.ssh` | tail over `(allow default)` / `kernel` | allowed (D9: reads are the inner profile's) / `EPERM` |
| read `/etc/hosts`, `INIT_HOME/kernel/*`, `lineage/kernel.git/*` | tail; `kernel` | allowed |
| write `INIT_HOME/work/*`, `INIT_HOME/state/*`, `INIT_HOME/lineage/<other>` | tail; `kernel` | allowed |
| write or rename `lineage/kernel.git`; write `INIT_HOME/kernel/*`, `INIT_HOME/root/*`, `run/mark/*`, `run/zone-mark`; write outside `INIT_HOME` | tail; `kernel` | `EPERM` |
| rename `INIT_HOME/kernel`, rename `INIT_HOME/run` | tail; `kernel` | `EPERM` |
| read own mark / the decoy / another mark / the zone mark | tail; `kernel` | allowed / `EPERM` / `EPERM` / allowed |
| connect to the egress socket inside the hidden `ROOT_HOME` | tail; `kernel` | allowed (a `file-read*` deny does not block `connect`) |
| connect to another socket inside `ROOT_HOME` (the `owner.sock` stand-in) | tail; `kernel` | `EPERM` |
| bind a Unix socket inside / outside `INIT_HOME` | tail | allowed / `EPERM` |
| TCP connect and bind on 127.0.0.1; UDP send to 127.0.0.1:53 | tail; `kernel` | `EPERM` |
| `kill -0` on a process outside the sandbox | tail | `EPERM` |
| `git init --bare` in `INIT_HOME/lineage/`, `tar --version` | `kernel` | allowed |
| inherited `SOCK_STREAM` and `SOCK_DGRAM` socketpairs; `SCM_RIGHTS` in both directions | `(allow default)(deny network*)` | work |
| `sandbox_check(pid, NULL, 0)` on another sandbox's process, called from inside `kernel` | `kernel` | returns 1 (works). The path-filter form also works from inside a sandbox (kernel/sandbox.md S-Z10, compiled helper). |

The rows for Z2, `Z_KGIT` and the decoy were re-run after the reconciliation edits. They are re-run as RR-1 and RR-2 (§8) against the real install, never on the owner's live session for anything that kills.

#### 3.3.6 Sealed runs: the sealed tail

Sealed audits, gate-of-gates runs and L3+ family evaluations (root/sealed-eval.md §3.2) run an isolated reference body or evaluator. That process must neither persist anything the zone can read nor pass through the kernel (D5). The sealed module asks the broker, in-process, for an instance of kind **`sealed`**. That kind is never available to the kernel (V5).
- **Profile.** PRELUDE (kernel/sandbox.md §3.3.1 sections 1–4) plus the sealed text below, with no SECTION and no ROOT_ALLOWS. The zone tail cannot be reused, because its Z1 hides all of `ROOT_HOME`. The run's private root is `ROOT_HOME/sealed/run/<id>/` (root/sealed-eval.md §3.2), listed in `sealed/runs.json` before anything spawns.
- **Binaries.** It runs the tuple's binaries, copied from the store into that root.
- **Network.** Its only network path is its sealed egress lane (`egress::sealed_lane`, root/egress.md §3.8), the socket `ROOT_HOME/run/sl-<run id>.sock`. A socket inside the run root would overflow macOS `sun_path` (104 bytes) for a 64-byte `ROOT_HOME`.
- **Clean-up.** The root is deleted by exact path afterwards.

The **sealed text** (exact text, `TAIL` in `sealed/run.rs`). It opens with three allows for the run root and the lane, then only denies. It denies all of `INIT_HOME` and the rest of `ROOT_HOME`, writes outside the run root, every other socket, binds, IP, XPC and signals outside the instance's own sandbox. So nothing the run produces lands where the zone can read it, and it cannot reach `kernel.sock`, `proxy.sock` or `egress.sock`.

```scheme
; ---- S. sealed run (root/sealed-eval.md §3.2, D5): the run root only, the lane socket only ----
(allow file-read* file-write* file-map-executable (subpath (param "S_RUN")))
(allow file-read-metadata (path-ancestors (param "S_RUN")))
(allow network-outbound (remote unix-socket (path-literal (param "S_LANE"))))
(deny file-read* file-write* (subpath (param "S_INIT_HOME")))
(deny file-read* file-write* (require-all (subpath (param "S_ROOT_HOME")) (require-not (subpath (param "S_RUN")))
  (require-not (path-ancestors (param "S_RUN")))))
(deny file-write* (require-not (require-any (subpath (param "S_RUN")) (literal "/dev/null") (literal "/dev/zero")
  (literal "/dev/dtracehelper") (subpath "/dev/fd"))))
(deny network-outbound (require-not (remote unix-socket (path-literal (param "S_LANE")))))
(deny network-bind) (deny network-inbound)
(deny mach-lookup (xpc-service-name-prefix ""))
(deny signal (require-not (target same-sandbox)))
(deny process-info* (require-not (target same-sandbox)))
```

| Param | Value |
|---|---|
| `S_RUN` · `S_LANE` | the run root · `ROOT_HOME/run/sl-<run id>.sock` |
| `S_ROOT_HOME` · `S_INIT_HOME` | `ROOT_HOME` · `INIT_HOME` |
| `HOME_REAL` · `DEVDIR` · `TOOLCHAIN` | as for the zone (the prelude uses them), from `install.json` |

The environment is fresh (`HOME` and `TMPDIR` inside the run root) and each run has its own process group. This block is drift-tested against the code like the zone texts: it is `SEALED_TAIL` (`root/sandbox-profiles/profiles/sealed-tail.sb`), checked by `doc_drift.rs` (FIX9).

The zone, in turn, can never read a run root, because Z1 of the zone tail hides all of `ROOT_HOME`. Sealed instances count toward `max_instances` within the `sealed.parallel` reserve (§3.14).

### 3.4 Confining the agent zone on Linux (UNVERIFIED: no Linux host was available; S-L checks at M7)

Bubblewrap nests in principle, but every zone process is still started by the Root. One spawn path on both platforms keeps the kernel's code identical, and the Root always knows every instance. The Root builds each bwrap argv from three parts:

1. **Zone arguments** (fixed, prepended to every instance):

   ```
   /usr/bin/bwrap --die-with-parent --new-session --unshare-user --unshare-pid --unshare-ipc --unshare-net
     --unshare-uts --unshare-cgroup-try --disable-userns --cap-drop ALL --add-seccomp-fd 5
   ```

   - `--disable-userns` (bwrap ≥ 0.8.0) needs `--unshare-user`. It stops the zone from making further user namespaces and so from rearranging its mounts.
   - Unix sockets: abstract ones are per network namespace, and a pathname socket exists only where it is bind-mounted.
2. **The mounts of the profile.**
   - `body`/`build`/`check`/`git-read`: kernel/sandbox.md §3.6.1 and §3.6.4, Root-embedded, with parameters validated by V1–V6.
   - `kernel`: the system view of kernel/sandbox.md §3.6.1, plus `--bind $INIT_HOME $INIT_HOME`, then read-only re-binds of `INIT_HOME/{kernel,root,lineage/kernel.git,run/mark}` and `--ro-bind` of the toolchain, then `--tmpfs /run/init-root --bind ROOT_HOME/run/egress.sock /run/init-root/egress.sock`. Nothing else of `ROOT_HOME` exists inside.
   - every profile: `--ro-bind-try <p> <p>` for each charter `sandbox.extra_read` entry (D9).
   - kind `sealed`: the `body`/`check` mounts rooted at its private root under `ROOT_HOME/sealed/run/<id>/`, with that root as the only writable bind and its sealed lane socket as the only socket (§3.3.6).
3. **The zone seccomp program** on fd 5 (`--add-seccomp-fd`, cBPF). Built at start with `seccompiler` 0.5.0 (Linux-only dependency).
   - Calls that fail with `EPERM`:
     - `io_uring_setup`, `io_uring_enter`, `io_uring_register` (io_uring can create sockets without `socket()`; kernel/sandbox.md §3.2);
     - `ptrace`, `process_vm_readv`, `process_vm_writev`;
     - `bpf`, `perf_event_open`, `userfaultfd`;
     - `keyctl`, `add_key`, `request_key`;
     - `kexec_load`, `kexec_file_load`, `open_by_handle_at`;
     - `socket` and `socketpair` with a domain other than `AF_UNIX`, which closes `AF_VSOCK` on VMs.
   - Construction: `SeccompFilter::new(rules, SeccompAction::Allow /*mismatch*/, SeccompAction::Errno(EPERM) /*match*/, ARCH.try_into()?)` then `.try_into::<BpfProgram>()`. The argument order of `new` is UNVERIFIED against the 0.5.0 source. The program is written to a pipe, and the pipe's read end becomes fd 5.

Instance identity is the pid-namespace inode, and a sweep is a `SIGKILL` to the bwrap pid (kernel/supervise.md §3.8.2, now executed by the Root). For bodies, the kernel's inner stage `init-kernel sandbox-inner` (Landlock plus the body seccomp filter) still runs inside, as kernel/sandbox.md §3.6.2 specifies. It is a stability layer; the zone guarantee comes from the Root's arguments.

Prerequisites, checked by `install` and `status --selftest`:
- `bwrap` ≥ 0.8.0;
- unprivileged user namespaces available;
- seccomp available.

Failing any one → `Halted{tooling}`.

### 3.5 The spawn broker

#### 3.5.1 Procedure

1. **Validate.** Check the request (§3.3.4, §4.1.3), the state (refused while `Panicked`, `Halted` or stopping; safe-mode limits per §3.6.4) and the quotas (§3.14).
2. **Allocate.** Make an instance id from 12 hex of 6 bytes of `/dev/urandom`. On macOS, create the mark `INIT_HOME/run/mark/<id>`: empty, 0400, `O_CREAT|O_EXCL|O_NOFOLLOW`.
3. **Render** the launcher argv: §3.3.1 on macOS, §3.4 on Linux.
4. **Spawn** `ROOT_HOME/bin/init-root exec-clean [--keep 3 --keep 4] -- <launcher argv> -- <program> <args>` as follows:

```rust
let (in_r, in_w) = std::io::pipe()?;                  // std 1.87+; From<PipeReader|PipeWriter> for Stdio
let (out_r, out_w) = std::io::pipe()?;
let (err_r, err_w) = std::io::pipe()?;
let mut cmd = Command::new(&self.root_bin);           // absolute path
cmd.arg("exec-clean").arg("--").args(&launcher)
   .env_clear().envs(&req.env).current_dir(&req.cwd)
   .process_group(0)                                  // own process group (supervise.md §3.4)
   .stdin(in_r).stdout(out_w).stderr(err_w);
let child = cmd.spawn()?;
drop(cmd);                                            // closes the child ends Command still holds
```

5. **Record** the instance in `state/root.json` (fsync) *before* answering, so that a Root crash can always find and sweep it.
6. **Hand over.** Send `[in_w, out_r, err_r]` to the kernel on the fd channel, tagged with the request id (§4.1.1). Then answer `{instance, pid}` and close the Root's copies.
7. **Reap.** Start the instance's reaper thread (§3.5.4).

The kernel instance itself is spawned the same way, with three differences:
- its stdout and stderr go to `ROOT_HOME/logs/kernel-<seq>.stderr` (capped at 4 MiB, 10 files kept; as built (WP21 D12), through a pipe a Root thread copies into the file, keeping the newest half behind a marker line when a write would pass the cap, and never blocking the kernel);
- its stdin is `/dev/null`;
- a `pre_exec` closure `dup2`s the control socket to fd 3 and the fd channel to fd 4, and exec-clean keeps them with `--keep 3 --keep 4`.

`pre_exec` is unsafe and forces std's fork path. `dup2` is async-signal-safe, and this happens once per kernel start.

#### 3.5.2 `exec-clean` and signals

`init-root exec-clean [--keep N]… -- argv…` runs unsandboxed for well under 1 ms, then `execv`s the launcher. It does three things:

1. Sets the signal mask to empty: `pthread_sigmask(SIG_SETMASK, &empty, null)`.
2. Closes every fd ≥ 3 except the kept ones: `close_range` on Linux ≥ 5.9, otherwise a loop up to `getdtablesize()`.
3. `execv`s the launcher.

Why each step:
- **Closing fds.** std and socket libraries set `FD_CLOEXEC` *after* `socket()` returns, not atomically. An fd that leaks during that window gets past Seatbelt's path checks (kernel/supervise.md §3.4 step 4, tested there). The Root holds TLS upstream sockets (egress), so a leaked fd would be a direct network path.
- **Clearing the mask.** std 1.97.1 deliberately *inherits* the parent's signal mask into children. Its source says: "Inherit the signal mask from the parent rather than resetting it". So the Root never blocks signals:
  - it handles `SIGUSR1`, `SIGTERM` and `SIGINT` with `sigaction` handlers that write one byte to a self-pipe, and a signal thread reads that pipe;
  - caught signals reset to their default on `exec`;
  - `exec-clean` clears the mask anyway, as defense in depth.

```rust
static SIG_W: AtomicI32 = AtomicI32::new(-1);
extern "C" fn on_signal(sig: libc::c_int) {            // async-signal-safe: one write(2)
    let b = sig as u8;
    unsafe { libc::write(SIG_W.load(Ordering::Relaxed), (&b as *const u8).cast(), 1) };
}
// install: sa.sa_sigaction = on_signal as usize; sa.sa_flags = libc::SA_RESTART; sigaction(SIGUSR1|SIGTERM|SIGINT)
```

#### 3.5.3 Handing stdio to the kernel

The fd channel is a `SOCK_DGRAM` socketpair (`UnixDatagram::pair()`), inherited by the kernel as fd 4. One datagram carries one hand-over: a payload `{"id":"k-42","n":3}` (ICJ) plus one `SCM_RIGHTS` control message with `n` ≤ 16 fds. Datagram boundaries keep each set of fds attached to its tag. Send and receive go through `libc::sendmsg`/`recvmsg` using `CMSG_FIRSTHDR`, `CMSG_DATA`, `CMSG_SPACE` and `CMSG_LEN` (all exported by libc 0.2.191 for Apple and Linux). std's `SocketAncillary` is still unstable. On Linux the Root receives with `MSG_CMSG_CLOEXEC`. On macOS there is no such flag, so the Root sets `FD_CLOEXEC` right after receipt; because of exec-clean the race is harmless. All of this lives in `sys.rs`, the only module allowed to use `unsafe` (§4.8).

Once the hand-over is done, the Root holds no copy of a body's pipes. The contract of kernel/supervise.md §4.5 therefore stays exact: when the kernel dies, its bodies see EOF on stdin and exit within 10 s.

#### 3.5.4 Reaping, end of an instance, sweeps

- **Reaper.** One thread per instance calls `libc::wait4(pid, &mut status, 0, &mut rusage)`. std's `Child::wait` does not return `rusage`.
- **End of an instance.** An instance ends when its leader exits. The leader is the pid the Root spawned: the body on macOS (`exec-clean` → `sandbox-exec` → program, all one pid), and bwrap on Linux. The Root then:
  1. sweeps whatever the instance left behind, such as `setsid` daemons;
  2. deletes the mark by its exact path;
  3. adds `ru_utime + ru_stime` to the CPU account (§3.14);
  4. sends `exited{instance, code, signal, rusage_ms}` to the kernel.

  kernel/supervise.md's "sweep the instance after exit" therefore becomes the Root's automatic behaviour (adopted there).
- **Membership rule (D1).** The Root signals the instance's leader by the pid it holds. It may signal any *other* process as a member of an instance only when **all three** conditions hold:
  - **(a)** the process is sandboxed: `sandbox_check(pid, NULL, 0) == 1`;
  - **(b)** it may read the instance mark: `sandbox_check(pid, "file-read-data", SANDBOX_FILTER_PATH | SANDBOX_CHECK_NO_REPORT, mark) == 0`;
  - **(c)** it is **denied** both the sibling decoy `INIT_HOME/run/mark/decoy` and the Root canary `ROOT_HOME/run/canary`: `sandbox_check(…) != 0` for each.

  Every zone instance meets (c) by construction: tail Z5 denies the decoy and Z1 denies the canary. A foreign sandbox permissive enough to read the mark can read its sibling decoy too, and almost always the canary, so (c) excludes it. The v0.2 rule, (a) and (b) alone, matched 31 of the owner's processes on 2026-10-20 and a test sweep killed them, Docker Desktop's VM among them (kernel/supervise.md §9). `sandbox_check` is resolved with `dlsym`. If the symbol is missing, the Root signals only the leader and its process group, never by scan, and `status.sweep = "degraded"` (RF13).
- **Sweep.** This is the algorithm of kernel/supervise.md §3.8.2 with the membership rule above. It is executed only by the Root.
  - **macOS:** `killpg` on the leader's group, then up to 50 passes, 20 ms apart. Each pass lists the uid's pids with `proc_listpids` (exported by libc 0.2.191 for Apple) and kills every pid that meets (a), (b) and (c).
  - **Linux:** `SIGKILL` to the bwrap pid (the Root's own child), then confirm that no process still has the instance's pid-namespace inode. A pid namespace contains only the instance, so no predicate over foreign processes is needed.
- **Generic sweep** (cold start without records; the `init-root panic` fallback).
  - **macOS:** (a); (b) with the zone mark `INIT_HOME/run/zone-mark` in place of an instance mark; and (c).
  - **Linux:** as kernel/supervise.md §3.8.2, with `INIT_HOME/kernel/bin` added to the executable paths.
- **Live-system safety (CONVENTIONS §1).** These rules are product behaviour. *Tests* of any sweep, kill or panic path run only in a disposable environment: a VM, a container, or a separate macOS user account with no other processes of its own. On the owner's machine, tests and self-tests signal only processes they spawned, by the pid they hold. `init-root status --selftest` therefore never sweeps. It runs short-lived probe instances that exit on their own, and it kills only their leaders, by pid.
- **As built (owner-machine build).** The D1 scan over the live process table is compiled only with the cargo feature `live-sweep`, which is enabled only in a disposable environment. Without it, every kill, sweep and panic signals only what the Root holds: each unreaped instance leader and its own process group (a zombie leader keeps the pgid the Root's). `status.sweep` is `"held_only"`, or `"d1"` with the feature.
- **Owner-machine mode (v0.1, ADR-008).** v0.1 runs on the owner's live account without a disposable test account. It rests on four locks:
  - `sys::signal`, the only `kill`/`killpg` in a default build, itself refuses any pid that is not an unreaped child of the Root (`waitid(P_PID, pid, WEXITED|WNOHANG|WNOWAIT)`).
  - `live-sweep` is in no default feature, and a release build that names it fails to compile.
  - Even a `live-sweep` build scans only where the disposable sentinel of testing.md §3.1a exists.
  - tests/rsafe.rs audits the source for every signalling call site.

  `init-root doctor --sweep-dry-run` runs the D1 rule (`sweep::member`) over the real process table and prints the would-kill set. It lists every instance mark plus the zone mark, says for each pid whether `state/root.json` records it, and counts the rejects by the condition they fail. It never signals. When the zone empties (panic, stop, crash) and at cold start, the Root runs the same rule on the zone mark. Any process outside its hold that still matches on two passes 300 ms apart gets an `escapee{after, mark, pids, signalled:false}` record and an owner alert, and is never signalled. The residual risk is a `setsid` escapee, which survives a panic until `live-sweep` is enabled (ADR-008).

### 3.6 R1: supervising the kernel

#### 3.6.1 State machine

The machine is a pure function `next(state, event) -> (state, Vec<Action>)`, unit-tested without processes, as in kernel/supervise.md §3.3. As built (WP21), `sup.rs` has pure, unit-tested `escalate()` (§3.6.4) and `backoff_ms()` functions plus an imperative supervisor loop. The `ColdStart`, `Swapping` and `Paused` slots are not built yet.

```rust
// init-root/src/sup.rs
pub enum Slot {
    ColdStart,                                            // §3.6.5 recovery; no kernel
    Starting { since: Instant, hello: bool },             // kernel spawned: hello ≤ 30 s, then ready ≤ 1200 s
    Running,
    Stopping { why: StopWhy, deadline: Instant },         // kernel told `stop` (≤ stop_grace_s), then killed
    Swapping(SwapPhase),                                  // kernel upgrade (§3.8)
    Backoff { until: Instant, attempt: u32 },
    Stopped,                                              // owner `stop`; waits for `start` or `/resume`
    Paused { reason: PauseReason, since: SystemTime },    // R1 pause (§3.6.6): resumes by itself
    Panicked { at: SystemTime, channel: Channel },        // latch present (§3.9)
    Halted { reason: HaltReason, retry_at: Option<SystemTime> },  // §3.6.6
}
pub enum Mode { Normal, Safe { since: SystemTime, reason: SafeReason } }
pub enum StopWhy { Owner, Rollback { to: TupleId, reason: RollbackReason }, SafeClear, Pause(PauseReason), Shutdown }
pub enum PauseReason { Disk }                             // §3.14: free space below the floor past the grace
pub enum HaltReason { Config, CrashLoopSafe, Disk, Layout, Tooling, Lineage, NoTuple }
pub enum SwapPhase { Gog { job: JobId }, Preparing { job: JobId, deadline: Instant },
                     Installing { job: JobId }, AwaitReady { job: JobId, deadline: Instant },
                     Probation { job: JobId, until: Instant } }
```

#### 3.6.2 Transition table (`*` = any state not otherwise listed)

| State | Event | Guard | Next | Actions |
|---|---|---|---|---|
| `*` | panic (any channel, §3.9.1) | — | `Panicked` | §3.9.2 |
| `ColdStart` | recovery done | latch absent; a tuple is usable | `Starting` | install the tuple's kernel if its hash differs (§3.7.3 step 3); spawn the kernel |
| `ColdStart` | recovery done | latch present | `Panicked` | sweep; spawn nothing |
| `Starting` | `hello` | `proto == 1` | `Starting{hello:true}` | answer with the boot block (§4.1.3) |
| `Starting` | `ready` | — | `Running` | redeliver the owner queue; `zone_start` record |
| `Starting` | exit 78 before `ready` | `config_error_exits` (2) in a row | `Halted{config}` | alert `config_error` with the stderr tail; not a crash; retried as §3.6.6 says |
| `Starting`/`Running` | crash (§3.6.3) | probation of an upgrade | `Starting` | rollback to the previous tuple (§3.8 step 6) |
| `Starting`/`Running` | crash | otherwise | `Backoff` or escalation | §3.6.4 |
| `Running` | `stop` (owner) | — | `Stopping{Owner}` | send `stop{grace_ms:30000}` |
| `Running` | watcher: free space below `disk_min_free_gib` for `disk_pause_grace_s` | — | `Stopping{Pause}` → `Paused` | send `stop{grace_ms:30000}`; `pause` record |
| `Paused` | free space ≥ floor + 2 GiB (checked every 60 s) | — | `Starting` | reason `cold`; `unpause` record |
| `Running` | demotion, crash-loop rollback | — | `Stopping{Rollback}` | send `stop{grace_ms:10000}` |
| `Running` | `system.swap` accepted | level ≥ 3, or source `maintainer` | `Swapping(Gog)` | §3.8 |
| `Running(Safe)` | `safe_auto_clear_s` healthy (D4: no owner step) | — | `Stopping{SafeClear}` | restart in normal mode |
| `Stopping` | kernel exit, or deadline then kill | — | `Starting` / `Stopped` | sweep every instance; for `Rollback`, install the target tuple first |
| `Backoff` | timer | — | `Starting` | spawn on the same tuple |
| `Stopped` | `start` or `/resume` | — | `Starting` | reason `cold` |
| `Panicked` | owner `resume` | no watch path or panic ref still present | `Starting` | §3.9.5 |
| `Halted` | `retry_at` reached, or the condition cleared (§3.6.6), or owner `resume` | — | `Starting` in `Safe` | counters reset; `halt{on:false}` record |
| `*` with a zone | free space < 512 MiB | — | `Halted{disk}` | orderly stop; leaves automatically once free ≥ 2 GiB |

#### 3.6.3 Exit codes, readiness and heartbeats

| Kernel exit | Meaning | Root reaction |
|---|---|---|
| `0` after `stop` | stopped on request | `Stopped`, or the next step of the rollback or safe-clear |
| `75` after `prepare_swap` | ready to be replaced | §3.8 step 4 |
| `78` before `ready` | owner configuration error (`kernel/config.toml`) | twice in a row → `Halted{config}` |
| anything else | crash | §3.6.4 |

A **crash** is any of the following:
- an exit not covered by the table above (including 0 or 75 when the Root did not ask);
- death by a signal the Root did not send;
- no `hello` within `kernel_hello_timeout_s` (30 s);
- no `ready` within `kernel_ready_timeout_s` (1200 s). This is long because a kernel cold start may rebuild the body LKG from lineage, a build of up to 15 minutes (kernel/supervise.md §3.9 step 6);
- no `hb` event for `hb_stale_s` (30 s), counted from `hello`. Before `hello` only `kernel_hello_timeout_s` applies, because the first exec of a freshly installed binary under Seatbelt took more than 3 s. The kernel sends one every `hb_interval_s` (5 s) while starting and running;
- three malformed control frames in a row, a line over 1 MiB, or more than 10,000 frames in a minute.

When the kernel crashes, the Root kills its instance and then sweeps **every** zone instance, because their stdio belonged to the dead kernel. Heartbeats are a liveness hint, not a security property: a kernel that fakes them only keeps itself from being restarted.

**Backoff** is `min(backoff_max_ms, backoff_initial_ms × 2^(n−1))`: 1, 2, 4, …, 60 s. `n` resets once a kernel has been `ready` for `backoff_reset_after_s` (300 s).

#### 3.6.4 Crash-loop escalation and system safe mode

The Root keeps the timestamps of kernel crashes on the current tuple in a sliding window of `crash_window_s` (600 s).

| Situation | Action | Demotes |
|---|---|---|
| crash with the count ≤ `crash_limit` (3) | backoff, respawn the same tuple | no |
| crash during an upgrade's ready wait or probation (§3.8) | immediate rollback to the previous tuple (`kernel_upgrade{stage:"boot_fail"\|"probation_fail"}`) | no |
| count > 3 and the current tuple is **not** the LKG tuple | whole-system rollback to the LKG tuple (`crash_loop{action:"rollback_lkg"}`) | no |
| count > 3 **on the LKG tuple** (spec §15 "a crash loop on LKG") | demotion (§3.13.3): audited-good tuple, **system safe mode** | yes |
| in system safe mode, count > `safe_crash_limit` (2) | `Halted{crash_loop_safe}` (`crash_loop{action:"halt"}`) | no (already demoted) |

**System safe mode** runs the audited-good tuple with these restrictions:
- **Root:** the broker allows only one `body` and `git_read` instances, and refuses `build`, `check` and `probe` (`root.mode`). `system.swap` is refused. Effective permissions are L0's minus evolution.
- **Egress:** `egress::set_level(level, safe_mode = true)`: the `providers` class with L0 models only (root/egress.md §4.3).
- **Kernel:** it gets `mode:"safe"` in `hello` and runs its body in the kernel's own safe mode (kernel/supervise.md §3.7: LKG body, no T1; `promote`, `gate`, `reload`, `vendor` and `rollback` refused).

It ends **automatically**, after `safe_auto_clear_s` (86,400 s) with no kernel crash and no `body_safe{on:true}` report. There is no owner command for it: D4 removes `/clear-safe` and every other required owner step. The kernel restarts on the same tuple in normal mode (reason `safe_clear`), which also ends the kernel's own body safe mode (kernel/supervise.md §3.7), and a `safe{on:false}` record is written.

#### 3.6.5 Cold start (Root start)

These steps run in order, before anything that can fail for other reasons:

1. **Lock and pid.** Take `run/root.lock` (`File::try_lock`, stable since 1.89); `WouldBlock` → print `already running` and exit 0. Write `run/root.pid`.
2. **Layout:** the R-F5 checks → `Halted{layout}`. Then the tool checks: `/usr/bin/sandbox-exec` on macOS; `bwrap` ≥ 0.8.0 on Linux; `git` ≥ 2.39.4 → `Halted{tooling}`.
3. **Lineage:** load the key (R-F4), verify the chain and `head.json` (§3.10.4; signatures over genesis and the last 1,000 records), and repair a torn tail (§3.10.3) → `Halted{lineage}`.
4. **Charter:** load `state/charter.active.toml`. If `charter.toml` has a different digest, raise a `charter_pending` notice. Its effects do not apply until the owner applies it.
5. **Orphan sweep:** every instance recorded in `state/root.json`, then the generic sweep (§3.5.4, D1 rule). As built, the Root never signals a previous Root's recorded pids, because a pid may have been reused. It deletes each recorded mark by its exact path; the D1 scans of those marks and of the zone mark run only with `live-sweep` (§3.5.4). The kernel and its bodies exit on their own when the control link and their stdin close (kernel/supervise.md §4.5). The Root never re-adopts zone processes after its own restart. It sweeps them and starts a fresh kernel (testing.md F-kill-root invariant 4).
6. **Panic latch** present → `Panicked`. Stop here, with the owner server up.
7. **Ledger:** fold the lineage, or load `state/ledger.json` and verify that its `seq` matches the head (§3.13).
8. **Services:** start egress (root/egress.md), sealed (root/sealed-eval.md), the owner server, the watcher and the signal thread. Rebuild the published view (§3.10.6).
9. **Choose a tuple:** the `state/root.json` current tuple, in its recorded mode, if its blobs verify. Otherwise LKG, then audited-good, then genesis, with an alert. None usable → `Halted{no_tuple}`. With no kernel installed at all, the Root stays `Stopped` with a notice (§3.7.1).

As built, step 9 and the first `start` run on the supervisor thread, and `owner.sock` is served at once. Verifying the blobs hashes the whole kernel binary, which took about 25 s for a 154 MB debug kernel in a debug Root. A panic in that window stays safe: `panic` sets the slot before it kills, and `start` re-checks the slot after the spawn, so a kernel spawned into `Panicked` or `Halted` is killed and never set running. The dev profile also builds `sha2` and its hashing crates (`digest`, `block-buffer`, `crypto-common`, `hybrid-array`, `cpufeatures`) at `opt-level = 3` (`root/Cargo.toml` `[profile.dev.package.*]`), so a debug Root's start → ready fell from 9.8 s to 1.7 s. Release builds and the hash checks are unchanged.

#### 3.6.6 The R1 pause and `Halted`, without an owner step (D4)

**Pause.** When free space stays below `quotas.disk_min_free_gib` for `disk_pause_grace_s` (600 s), the watcher pauses the zone (§3.14):
1. it stops the zone in order (`stop`, 30 s grace, then the instances the broker holds are stopped by id);
2. it appends `pause{reason:"disk"}`;
3. it enters `Paused`.

Every 60 s it checks again. Once free space is back at the floor plus 2 GiB, it starts the same tuple (reason `cold`) and appends `unpause`. A pause is not a demotion, and nothing is lost: the session log and every file stay as they were. A compute overage never pauses; it only refuses heavy spawns until the next owner day.

**Halted.** Each reason has a way out that needs no owner:

| Reason | Retry |
|---|---|
| `crash_loop_safe` | after `halt_retry_s` (21,600 s), in safe mode with fresh counters; at most 4 retries per owner day (the charter's `money.day_starts_at_utc_offset`), then the next day. As built (WP21 D9), the retries are counted from the `halt{on:false}` records of the owner day (an owner `resume` counts too), and a halt the records leave standing survives a Root restart with its `retry_at` |
| `config` | when the digest of `kernel/config.toml` changes (checked every 60 s), and otherwise after `halt_retry_s` |
| `disk` | when free space is back at ≥ 2 GiB |
| `layout`, `tooling`, `lineage`, `no_tuple` | **none.** These are integrity failures: the Root cannot run the zone safely, and retrying cannot fix them. The Root stays up (status, attach, `audit verify`) and alerts. A fail-safe stop is not an approval step. |

Every halt and retry is recorded (`halt{reason, on}`) and alerted. The owner may still `resume` early. That is a convenience, never a requirement.

### 3.7 R1: whole-system tuples, LKG and rollback

#### 3.7.1 Tuples and the blob store

A **tuple** is one runnable system state:

```json
{"body":{"binary":"5c1f…64hex","sha":"4b1e0c2d…40hex"},
 "ext":{"components":[{"name":"semver-sort","sha256":"9a07…"}],"set":"b81c0e5a9d27"},
 "kernel":{"built_from":null,"init_kernel":"e3b0…","initctl":"7d86…","source":"maintainer","version":"0.1.0"},
 "v":1}
```

(Shown wrapped; on disk it is one ICJ line.)

- The tuple id is `"t"` followed by the first 12 hex of `SHA-256(ICJ(tuple))`, so equal tuples get equal ids.
- `kernel.source` ∈ `maintainer` | `agent`.
- `kernel.built_from` is the kernel source commit in `lineage/kernel.git`: for a maintainer release, the commit of its source bundle (`refs/upstream/<version>`); for an agent kernel, the candidate commit the Root fetched at install (§3.8.1). As built, the Root reads it from `lineage/kernel.git` `refs/heads/main` when it forms the kernel half of a tuple, the genesis tuple included (`""` if `main` does not exist).
- `ext.set` uses the set-id rule of kernel/supervise.md §3.6.5.
- **Tuple v2 (M7; m7-design.md §3.2.3).** `body` gains `t0`: the sha256 of a `t0` blob, a pax tar of the body's `GENESIS.md`, `SELF.md` and `skills/**` (relative paths, ≤ 8 MiB), stored with `store.put kind:"t0"` (§4.1.3). Sealed runs, slices and audits alike, extract it into the run root's `body/`, so they see the candidate's real head. This also closes the gap that audits had no defined T0 source.

**Store.**
- Blobs are `store/blobs/<sha256>`, mode 0444, each at most 512 MiB. Kinds: `init-kernel`, `initctl`, `init-body`, `ext`, and `kstate`, a snapshot of the old kernel's `state/kernel/` taken at a kernel swap (§3.8). A `kstate` blob is kept only until that swap's probation ends. They are written as a temp file, fsynced, then renamed (R-F3). The same bytes are never stored twice.
- **Retention:** the tuples referenced by `current`, `lkg`, `audited` and (M7) `confirmed`, plus the newest `store_keep_tuples` (5) tuples. After each pointer change, blobs that no kept tuple references are deleted by exact path.
- **Size:** `quotas.store_max_gib` (8). Older, unreferenced tuples are pruned first. If the store is still too big, `store.put` is refused with `root.store` and an alert.

**How blobs enter the store.** The kernel opens the file inside its sandbox and passes the fd (`store.put`, §4.1.3). The Root reads the fd to EOF while hashing, compares the hash with the claimed `sha256`, and stores the blob. Under R-F2 the Root never opens `INIT_HOME/bin/…` by name. Because Seatbelt checked the open, the kernel cannot feed in a file it cannot read (for example `~/.ssh/id_ed25519`).

**When tuples are formed.**
- **Body promotion.** When the kernel reports `promotion{lkg:true}` (§3.10.5), the binary and the extension components must already be stored. The Root forms `{current kernel, new body, new ext}` and appends `tuple`, then `lkg` (§3.7.2).
- **Kernel upgrade.** §3.8.
- **Install.** The genesis tuple is the maintainer kernel plus the seed body built at install (§4.3 step 8).
- **Seedless homes.** A home installed without `--seed-source` (the Root's test homes) forms genesis at its first start from the Root-written `kernel/bin` binaries and an empty body (`binary:""`, `sha:""`, ext `set:null`), which kernel/supervise.md treats as genesis. With no kernel installed at all (no tuple and no `kernel/bin/init-kernel`), the Root enters `Stopped` with a notice, not `Halted{no_tuple}`; `Halted{no_tuple}` is kept for tuples that exist but none of which verifies. Running the install-mode kernel at first start is not built: a failed seed install is re-run with `install`.

#### 3.7.2 Pointers

| Pointer | Meaning | Moves when |
|---|---|---|
| `current` | the tuple the zone runs | every rollback, swap or promotion |
| `lkg` | the newest tuple known good at system level | a body promotion while the current kernel is LKG-grade (not inside an upgrade's probation); an upgrade's probation passes; a sealed audit passes on a newer tuple |
| `audited` | the newest tuple that passed a sealed audit | a sealed audit on that tuple passes (root/sealed-eval.md writes `audit{regression:false, tuple}`; the Root then appends `audited` and, if the tuple is newer, `lkg`). From M7 also when a G4 slice whose incumbent is the `audited` tuple ends `superior`, or `tie` with m ≤ `margin_max`, and its candidate becomes `lkg` (root/sealed-eval.md §3.5.1); a milestone regression resets it to `confirmed` |
| `confirmed` (M7) | the last tuple whose lineage gain a milestone check confirmed on the untouched reserve (m7-design.md §3.2.13; provisional owner decision 2026-10-20) | a milestone check with outcome `confirmed` (root/sealed-eval.md §3.5.1). A milestone `regression` rolls the system back to it (§3.7.3), with a demotion |

The genesis tuple is `lkg` and `audited` by definition, and from M7 also `confirmed`. Its first sealed audit sets the baseline (root/sealed-eval.md). The pointers are recorded in lineage, so they can be rebuilt from it. `state/root.json` caches them.

#### 3.7.3 Whole-system rollback procedure (target T)

1. **Stop the zone.** If the kernel is alive and not the cause, send `stop{reason:"rollback", grace_ms:10000}` and wait at most `stop_grace_s`. Then sweep every instance with `SIGKILL`.
2. **Verify T's blobs.** If any is missing or corrupt, take the next fallback: LKG, then audited-good, then genesis. None usable → `Halted{no_tuple}`.
3. **Install T's kernel.** For `init-kernel` and `initctl`: create `INIT_HOME/kernel/bin/.<name>.tmp` with `O_CREAT|O_EXCL|O_NOFOLLOW` and mode 0755, copy the blob in, `sync_all`, rename over the old file, fsync `kernel/bin/`. Each install is a new inode (R-F3).
4. **Record.** Set `state/root.json` `current = T` and append `rollback{from, to:T, target, reason}`.
5. **Spawn the kernel** with a boot reason of `rollback_system`, `demotion` or `panic_resume`. The `hello` answer carries `tuple.body` and `tuple.ext`. When its `bin/init-body`, `bin/init-body.lkg` or `bin/ext` hashes differ, the kernel restores them from `store.get` fds. It then resets lineage `main` and `lkg` to `tuple.body.sha` and rewrites `lkg.json` (adopted by kernel/supervise.md). If `boot.kstate` is set, it first restores `state/kernel/` from that snapshot (§3.8). The Root never writes `bin/` (R-F1).

The conversation survives a rollback, because the body's session log under `state/` is untouched and the new body rehydrates it (kernel/supervise.md). Rolled-back versions are not lost: they stay in `lineage/body.git` under `refs/init/archive/*`, and the agent may promote them again through the gate.

### 3.8 R1: kernel upgrades (the Root side)

A kernel upgrade replaces the kernel build in the running tuple. There are two sources:
- **agent:** at L3 and above, the kernel sends `system.swap` with an agent-built kernel;
- **maintainer:** at any level, the owner runs `init-root install --upgrade --release <dir>`.

Both go through the same steps:

| Phase | What happens | On failure |
|---|---|---|
| 1 request | Blobs are already stored (`store.put`). Checks: level ≥ 3 or source `maintainer` (`root.level`); not safe mode (`root.mode`); no swap in progress (`root.busy`). Job `s-NNNNNN`. Record `kernel_upgrade{stage:"requested"}`. | refusal; a `root.level` refusal is a `ceiling_hit{ceiling:"level", demotes:true}` (D6 (b), §3.13.3) |
| 2 gate-of-gates | `sealed::gog(candidate_tuple)` (root/sealed-eval.md) runs on Root-spawned `sealed` instances (§3.3.6) while the zone keeps running. It records `mutation{…}` and `kernel_upgrade{stage:"gog_pass"\|"gog_fail"}`. | agent source: demotion trigger "mutation-suite failure in a new gate"; maintainer source: alert only |
| 3 prepare | Root → kernel `prepare_swap{job, deadline_s:1800}`. The kernel pauses every body at a step boundary and stops it (exit 0). It `store.put`s a `kstate` snapshot of `state/kernel/` (a tar stream on an fd, logs excluded), flushes, answers `ok` and exits 75 within 10 s. | timeout or refusal → the swap is aborted and the old kernel keeps running (a failed swap is a non-event, ADR-001) |
| 4 install | Sweep leftovers. Fetch the candidate source into `lineage/kernel.git` (§3.8.1). Install the new kernel (§3.7.3 step 3). Append `tuple` and `kernel_upgrade{stage:"installed", source_sha}`; `current = new`. | a failed fetch aborts the swap before the install, and the old kernel restarts |
| 5 ready | Spawn the new kernel with reason `kernel_upgrade`; it must be `ready` within 1200 s. Its bodies boot with reason `cold` on the same session. | rollback to the previous tuple, `stage:"boot_fail"`, no demotion |
| 6 probation | `kernel_probation_s` (1800 s) with no kernel crash | rollback to the previous tuple, `stage:"probation_fail"`, no demotion. The restored kernel gets `boot.kstate` (the snapshot's sha256) and restores `state/kernel/` from `store.get` before anything else, so it never runs on state the newer kernel migrated. kernel/supervise.md still requires the newer kernel to keep the older one's formats readable; the snapshot is a second layer. |
| 7 LKG and audit | Append `lkg`; move `lineage/kernel.git` `refs/heads/lkg`; drop the `kstate` blob. The sealed audit for "every kernel upgrade" (spec §16) is triggered through root/sealed-eval.md; in Phase 0 the owner runs it manually (spec §19). | pass → `audited`; regression beyond δ_S → demotion (§3.13.3) |

After a demotion below L3, the audited-good tuple keeps whatever kernel it has, an agent-written one included, and `system.swap` from an agent source is refused until L3 is earned again (evolution.md §3.15.3).

#### 3.8.1 `lineage/kernel.git` and the kernel source (D7)

`INIT_HOME/lineage/kernel.git` is a bare repository that **only the Root writes** and the zone can only read (tail Z4). The Root creates it at install, at every level, so the source of the kernel the system runs is always readable and can be inventoried (kernel/selfmap.md).

| Ref | Written | Meaning |
|---|---|---|
| `refs/upstream/<version>` | at install and at each maintainer kernel release | the commit of the release's source bundle `INIT_HOME/kernel/src/<version>.bundle` |
| `refs/heads/main` | at each install of a kernel (phase 4), and moved back on rollback | the source of the current tuple's kernel (`tuple.kernel.built_from`) |
| `refs/heads/lkg` | phase 7 | the source of the LKG tuple's kernel |
| `refs/init/karchive/<sha>` | phase 4, create-only | every agent kernel the Root has installed |
| `refs/init/rolled-back/<sha>` | at a rollback, create-only | a kernel source that was rolled back |

- **Maintainer source.** The release carries `src/<version>.bundle` (§4.3). The bundle holds exactly one ref, `refs/heads/release`. The Root copies it to `INIT_HOME/kernel/src/` and imports it with `$RK fetch --no-tags <bundle> "refs/heads/release:refs/upstream/<version>"` (create-only: an existing `refs/upstream/<version>` that differs is refused with an alert; the same commit again is a no-op). At install, `refs/heads/main` is created only when `kernel/src/` holds exactly one bundle. Here `$RK` is the hardened Root git prefix of §3.11 with `--git-dir=$INIT_HOME/lineage/kernel.git`.
- **Agent source.** The kernel keeps candidates in its own candidate area and never writes `lineage/kernel.git` (evolution.md §3.15.6). `system.swap` names `source_repo` (a bare repo under `INIT_HOME`) and `source_sha`. At phase 4 the Root fetches exactly that commit through a **sandboxed upload-pack**, because running git as the owner on a repo the zone can write would execute zone-chosen configuration (kernel/supervise.md §3.11.3):

  ```sh
  $RK fetch --no-tags --no-write-fetch-head --no-recurse-submodules \
     --upload-pack="$ROOT_HOME/bin/init-root sandboxed-upload-pack" \
     "$INIT_HOME/<source_repo>" "+$SOURCE_SHA:refs/init/karchive/$SOURCE_SHA"
  $RK merge-base --is-ancestor refs/heads/main "$SOURCE_SHA"     # must descend from the running kernel's source
  $RK update-ref refs/heads/main "$SOURCE_SHA" "$OLD_MAIN"        # CAS
  ```

  `init-root sandboxed-upload-pack <repo>` runs `git upload-pack` inside a Root-spawned `git-read` instance (kernel/sandbox.md §3.5), with `-c uploadpack.allowAnySHA1InWant=true`. `transfer.fsckObjects=true` rejects malformed objects. The Root performs no other git operation on zone data. The fetch comes before the ancestry check, so a refused non-descendant keeps its create-only `karchive` ref.
- **Rollback.** The Root moves `refs/heads/main` back to the target tuple's `built_from` and keeps the removed commit under `refs/init/rolled-back/`.

### 3.9 R1: panic and stop

`/panic` means: stop every action and all spend now, keep everything for inspection, and restart nothing until the owner says so. It must not depend on the kernel, which is agent code from L3. None of its channels passes through the zone.

#### 3.9.1 Channels

| Channel | How the owner uses it | Reaches the Root through | Reaction | Always on |
|---|---|---|---|---|
| owner socket | the line `/panic` in `init-root attach`, or `init-root panic` (frame `{"t":"panic"}`) | `ROOT_HOME/run/owner.sock`; the reader checks for `/panic` before anything else | < 100 ms | yes |
| signal | `kill -USR1 $(cat ~/.init-root/run/root.pid)` | self-pipe → signal thread | < 100 ms | yes |
| latch file | `touch ~/.init-root/run/panic` | the watcher's 1 s `lstat` | ≤ 1 s | yes |
| watch path | an owner-chosen file appears, for example in a synced folder (`panic.watch_paths`) | the watcher's 1 s `lstat` | ≤ 1 s | optional |
| mirror ref | the owner creates branch `panic` on the mirror remote, for example from a phone (`panic.mirror_ref = true`) | `git ls-remote <url> refs/heads/panic` every `panic.mirror_poll_s` (300 s), on the mirror thread. It fires once per commit id, so after `resume` a stale `panic` branch does not panic again until it changes. | ≤ 300 s | optional |

The zone can trigger none of them:
- `owner.sock` and the latch are inside `ROOT_HOME` (tail Z1, Z6);
- signals to processes outside the instance's own sandbox are denied (Z7);
- watch paths must lie outside `INIT_HOME` (C8), and Z3 stops the zone writing anywhere outside `INIT_HOME`;
- the mirror's deploy key lives in `ROOT_HOME/keys`.

#### 3.9.2 Actions, in order

Steps 1–3 must finish within 100 ms of receipt, and steps 4–6 within 2 s.

1. Set the in-memory panic flag. Create the latch `run/panic` containing `{"at","channel","v":1}` and fsync it. If the owner created the file by hand, it is left as is.
2. `egress::panic()`: refuse new requests and drop in-flight ones; spend stops (root/egress.md).
3. The broker refuses every spawn (`root.mode`), and `sealed::cancel_all()` cancels sealed jobs.
4. Sweep **every** zone instance (kernel, bodies, probes, builds, checks, git reads, sealed runs) with `SIGKILL` and no grace. In the owner-machine build (no `live-sweep`, §3.5.4) this kills only what the Root holds: each unreaped leader and its own process group, plus the sealed runs' held pids. A `setsid` escapee survives until the D1 scan is enabled, which happens only in a disposable environment (CONVENTIONS §1).

   **Owner-machine mode (v0.1, ADR-008).** Panic, stop and the resume rollback send signals only through `sys::signal`, which refuses any pid that is not an unreaped child. Cold start never signals a pid recorded by a previous Root: it deletes their marks by exact path and logs them. The panic result carries `sweep:"held_only"`, and its message says that a process which left its group is reported, not killed. Once the zone is empty, the Root runs the D1 rule on the zone mark. A survivor gets an `escapee` record and an alert naming its pids (§3.5.4), and ending it is the owner's call. RT-P1, RT-P3, RT-P4, RT-P6, RT-P7, RT-P9 and RT-P11 run live in this mode (tests/wp21.rs, tests/rsafe.rs).
5. Append `panic{channel, killed:{instances, processes}}`. Then demote one level (§3.13.3; the owner's decision CH-2, D13): append `ladder{direction:"down", cause:"demotion", trigger:{kind:"panic", seq}}`. The rollback happens at resume.
6. Report `PANIC at <ts> via <channel>: killed <p> processes in <i> instances; egress closed; level <a>→<b>. init-root resume to restart.` to every attached client, `alerts.jsonl` and `notify.cmd`. Push the mirror at once.

#### 3.9.3 What panic kills and what it preserves

| Killed or stopped | Preserved |
|---|---|
| every agent-zone process, including ones that left their process group or session (identified by sandbox or pid namespace, not by process tree; with the D1 scan, §3.9.2 step 4) | every file in `INIT_HOME` and `ROOT_HOME`: no git resets, no deletions, no binary installs |
| all egress, including in-flight streams | the session logs, `lineage/body.git`, the kernel's `state/` and `vault/`, for inspection |
| sealed audit and gate-of-gates runs | the Root daemon itself: `status`, `attach`, `ledger`, `audit verify` keep working |
| new spawns, until `resume` | the egress spend ledger and the lineage, including the `panic` record |
| the kernel's pending jobs (lost with the process; the kernel marks them failed at its next cold start) | mirror pushes, which continue so the panic reaches off-machine evidence |

#### 3.9.4 The latch survives restarts

A Root that starts and finds `run/panic` enters `Panicked`, runs the orphan sweep and spawns nothing (§3.6.5 step 6). So neither a Root crash, a launchd restart, a reboot nor a Root upgrade undoes a panic. Only `resume` deletes the latch.

#### 3.9.5 Resume

`init-root resume`, or the line `/resume` while the Root is `Panicked`:
1. Refused while any `panic.watch_paths` file still exists, or the mirror's `refs/heads/panic` still exists. The message names the file or ref to remove. *As built (build-deviations §5 L5):*
   - The ref check uses the mirror thread's last `ls-remote` rather than a fresh one, because a git call may take up to 120 s and the CLI waits 10 s. A refused resume moves the next poll to now, so the owner can delete the branch and resume about a second later.
   - With no mirror (`mirror.url = ""` until M0) or with `panic.mirror_ref = false`, nothing is polled and there is no ref to check.
2. Delete the latch.
3. Run the rollback procedure (§3.7.3) to the **audited-good** tuple, as the demotion requires (spec §15).
4. Start in the mode the system had at the time of the panic, at the demoted level, with boot reason `panic_resume`.
5. Append `resume{tuple, mode, level}`.

#### 3.9.6 Panic without a working Root

`init-root panic` connects to `owner.sock` and waits up to 2 s for confirmation. If none comes, it does the panic itself, as the owner, with the same code:
1. creates the latch;
2. sends `SIGUSR1` to the Root pid;
3. runs the generic sweep (§3.5.4) directly;
4. kills the Root (`SIGKILL`) if it is still unresponsive.

launchd restarts the Root, which finds the latch and sweeps again. The CLI exits 0 once no zone process remains. An offline Root, a wedged Root and a deadlocked supervisor are all covered.

**As built (WP21), the fallback writes the latch only** (channel `cli_fallback`) and exits 0. It sends no `SIGUSR1`, does no self-sweep and does not `SIGKILL` the Root, because a stale `root.pid` could name one of the owner's processes. A running Root's watcher takes the latch within 1 s; a dead Root takes it at its next start, and records the panic once.

#### 3.9.7 Stop (cheap, no demotion)

`init-root stop` (or `/root stop`):
1. The Root sends the kernel `stop{reason:"owner", grace_ms:30000}`. The kernel stops its bodies (kernel/supervise.md `stop` frame) and exits 0.
2. The Root sweeps and enters `Stopped`, and appends `stop{by:"owner"}`.

The tuple and level are unchanged. `init-root start` (or `/resume` while `Stopped`) starts the same tuple. This is the owner's everyday "halt"; panic is the emergency and demotes one level (CH-2, decided by the owner, D13).

### 3.10 R4: lineage

#### 3.10.1 Record format

`ROOT_HOME/lineage/lineage.jsonl` is append-only, one ICJ object per line:

```json
{"at":"2026-10-20T08:01:02.345Z","by":"root","data":{"channel":"owner_socket","killed":{"instances":3,"processes":41}},"hash":"sha256:5d2a…64hex","kind":"panic","prev":"sha256:9c41…64hex","seq":311,"sig":"ed25519:1f0b…128hex","v":1}
```

| Field | Type | Meaning |
|---|---|---|
| `v` | int | 1 |
| `seq` | u64 | 0 for `genesis`, then strictly +1 |
| `at` | string | RFC 3339 UTC with milliseconds, from the Root's clock. It is never taken from the kernel, and never decreases: the Root writes `max(previous at, now)`. |
| `by` | string | `root`, or `kernel:<first 12 hex of the reporting kernel build's sha256>` for kernel-reported kinds (§3.10.5) |
| `kind` | string | §3.10.5 |
| `data` | object | per kind; ICJ: integers only, ≤ 16 KiB serialized, depth ≤ 8 |
| `prev` | string | the previous record's `hash`; for `genesis`, `"sha256:"` followed by 64 zeros |
| `hash` | string | `"sha256:" + hex(SHA-256(B))` |
| `sig` | string | `"ed25519:" + hex(Ed25519(sk, "INIT-ROOT-LINEAGE-V1\n" ‖ raw 32 hash bytes))` |

`B` is the ICJ serialization of the object with exactly the keys `at, by, data, kind, prev, seq, v` (no `hash`, no `sig`). The stored line is the ICJ serialization of all nine keys, plus `\n`. Because ICJ is canonical, a verifier recomputes `B` from the parsed line, and also checks that the line itself re-serializes byte for byte.

#### 3.10.2 Signing

The Root uses **`ed25519-dalek` 3.0.0** (released 2026-07-06; default features `fast` and `zeroize`; MSRV 1.85; depends on `curve25519-dalek` ^5.0.0, `ed25519` ^3 and `sha2` ^0.11, which is the same `sha2` the Root already uses). Its standing:
- The dalek libraries were audited by Quarkslab in 2019. The audit focused on `curve25519-dalek` and reviewed `ed25519-dalek` more lightly.
- The known advisories are fixed in these major versions: RUSTSEC-2022-0093 (double-public-key oracle, fixed in ≥ 2.0) and RUSTSEC-2024-0344 (`curve25519-dalek` timing, fixed in ≥ 4.1.3).
- Verification uses `verify_strict`, which rejects malleable signatures and weak keys.

```rust
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
const DOMAIN: &[u8] = b"INIT-ROOT-LINEAGE-V1\n";

// install: seed from /dev/urandom (no rand dependency); SigningKey implements ZeroizeOnDrop
let sk = SigningKey::from_bytes(&seed);                         // seed: [u8; 32]
let pk_hex = hex(&sk.verifying_key().to_bytes());               // keys/lineage.pub

fn seal(sk: &SigningKey, b: &[u8]) -> ([u8; 32], Signature) {
    let h = Sha256::digest(b);
    let hash: [u8; 32] = h[..].try_into().expect("32 bytes");
    (hash, sk.sign(&[DOMAIN, &hash[..]].concat()))
}
fn check(vk: &VerifyingKey, hash: &[u8; 32], sig: &[u8; 64]) -> bool {
    vk.verify_strict(&[DOMAIN, &hash[..]].concat(), &Signature::from_bytes(sig)).is_ok()
}
```

- **Key.** `keys/lineage.ed25519` holds the 32 raw seed bytes (R-F4), generated at install. `install` prints the fingerprint `sha256(pubkey)[..16]` for the owner to keep, and the public key is also published in the view and the mirror.
- **Rotation.** Phase 0 has one key. The format reserves kind `key_rotate{new_pubkey}`, signed by the old key, for a later Root release.
- **Lost or corrupt key.** → `Halted{lineage}`; the chain cannot be extended.

#### 3.10.3 Appending and durability

1. Build the record and compute `hash` and `sig` (§3.10.1).
2. Write the line with one `write_all` to the file opened with `O_APPEND`, then `sync_data` (`F_FULLFSYNC` on Apple). Append to the view copy (§3.10.6).
3. Update `head.json` with `write_atomic`.

All appends are serialized under one mutex, owned by the supervisor thread's lineage handle.

**Torn tail.** At start, a final line without `\n` (a crash mid-append) is copied to `lineage/torn-<ts>.bin` and the file is truncated to the end of the last complete line. This is the only rewrite ever made, and only of an unterminated tail. A complete line that fails verification is *not* repaired → `Halted{lineage}` with alert `lineage_corrupt`. The owner compares the local file with the mirror (§3.11).

**Dedup key (D7).** Every kernel `lineage.append` carries `idem`, which is the kernel's boot id plus its outbox sequence number (`"<boot_id>:<outbox_seq>"`, ≤ 64 bytes, kernel/supervise.md §3.13.3). Before appending, the Root looks the key up in `lineage/idem.jsonl`, which keeps the keys of the last 7 days. If the key is there, the Root appends nothing and answers with the original `{seq, hash}` plus `dup:true`. A resend after a lost response therefore never duplicates a record. The key is indexed **before** the record is written (write-ahead), so a resend after a crash at any point cannot duplicate either. At open the index keeps only the keys of the last 7 days whose record is in the chain with the same hash; a key whose record never landed is dropped, and its resend appends once. The `idem` key is stored in the index, not in the record, so the chain stays free of kernel bookkeeping.

**Volume.** Kernel-reported records are limited to `kernel_lineage_per_hour` (600); beyond that the kernel gets `root.rate` and the owner an alert. Root-written kinds are rare (tens per day). root/egress.md keeps model calls and read-only class calls out of the lineage entirely (they are usage-ledger records). It writes one `egress_action` per effectful action, demoting `ceiling_hit` records capped at `kernel_lineage_per_hour` an hour (beyond the cap the record is refused with `root.rate` and logged, and only the egress usage ledger has it; one incident costs one level anyway), and at most one non-demoting `ceiling_hit` per ceiling per owner day. The chain, and the mirror, therefore stay small.

#### 3.10.4 Verification

`init-root audit verify [--full]` checks, for every line:
1. It is canonical ICJ, and `v == 1`.
2. `seq` is the previous `seq` + 1, starting at 0. A gap means truncation or deletion.
3. `prev` equals the previous `hash`.
4. `hash` recomputes from `B`.
5. `sig` verifies with `verify_strict` under the genesis public key, or the newest `key_rotate` key.
6. `at` does not decrease.
7. `head.json` equals the last record. A head one record behind (an append in progress, or a crash after the line) is accepted by `audit verify` and rewritten at open. A head ahead of the chain (truncation) is invalid.
8. If a mirror is configured, the mirror's `HEAD.json.seq` is ≤ the local `seq`, and the local record at that `seq` has the same `hash`. This proves the remote is a prefix of the local chain, so the local file was not rewritten. The Root fetches the remote branch into `refs/mirror/remote` for this check; an unreachable mirror prints `not checked` and does not fail the run.

When it runs:
- at every start, over the tail: rules 1–4 and 6 on every line, and signatures (rule 5) on genesis and the last 1,000 records. At open this is a constant 1,000, the charter default; `audit verify` without `--full` reads `root.lineage_verify_tail`;
- in full once a day (on the mirror thread; a failure alerts `lineage_corrupt` and enters `Halted{lineage}` through a hook into the supervisor);
- in full on demand. `audit verify --view` checks `INIT_HOME/root/lineage.jsonl` under `INIT_HOME/root/lineage.pub`, as the agent can.

The agent can verify its own copy with the published public key (§3.10.6). Exit codes: 0 valid; 1 invalid, printing the first bad `seq` and the rule it broke.

#### 3.10.5 Kinds

**Written by the Root** (`by:"root"`). The Root refuses to append a Root kind that is not in this table. `key_rotate` is reserved: verification supports it, and nothing writes it yet.

| Kind | `data` |
|---|---|
| `genesis` | `pubkey`, `root_version`, `install_id` (16 hex, random). Test builds may add `test:true` (§3.16), which a production Root refuses to run on. |
| `root_start` | `root_version`, `root_sha256` |
| `charter` | `digest`, `changed` (sorted key paths), `max_level`, `money{day_musd,total_musd}`, `ladder` (the effective criteria), `source` (`install`\|`owner`) |
| `tuple` | `id`, `tuple` (§3.7.1) |
| `lkg` · `audited` · `confirmed` (M7) | `tuple` |
| `zone_start` | `tuple`, `reason`, `mode`, `level` |
| `zone_exit` | `tuple`, `code`, `signal`, `class` (`stop`\|`swap`\|`crash`\|`config`\|`panic`\|`pause`). As built, no `zone_exit` is written for a panic (the `panic` record covers it), and `signal` is always null. The install-mode kernel (§4.3 step 8) gets neither `zone_start` nor `zone_exit`. |
| `crash_loop` | `tuple`, `crashes`, `window_s`, `action` (`rollback_lkg`\|`demote`\|`halt`) |
| `safe` | `on`, `reason` |
| `halt` | `on`, `reason`, `retry_at` |
| `pause` · `unpause` | `reason` (`disk`) (§3.6.6) |
| `rollback` | `from`, `to`, `target` (`lkg`\|`audited`\|`previous`\|`genesis`; from M7 also `confirmed`, after a milestone regression), `reason` |
| `kernel_upgrade` | `job`, `source`, `from`, `to`, `stage`, `source_sha` |
| `ladder` | `from`, `to`, `direction` (`up`\|`down`\|`cap`), `cause` (`unlock`\|`demotion`\|`charter_max`), and either `criteria` (up) or `trigger{kind, seq}` (down, cap). `trigger.kind` ∈ `audit`, `mutation`, `ceiling_hit`, `crash_loop`, `body_safe`, `panic`, `gate` (a canary trip) (down) and `charter` (cap); test ledgers may also use `test_fixture` (§3.16). |
| `share` | `evolution_share_max_permille`, `window`, `reason` (§3.13.5) |
| `ceiling_hit` | `ceiling`, `code`, `limit`, `attempted`, `demotes`, `rule` (`a`\|`b`\|`c`\|`none`, D6), `source` (`egress`\|`broker`). Money and action-class hits are written by root/egress.md through `lineage::append`; the broker writes `root.level` and quota refusals. Demoting records are capped at `kernel_lineage_per_hour` an hour (§3.10.3). |
| `egress_action` | written by root/egress.md (§3.6 there) for each request of a class with external effects: `class`, `verb`, `destination`, `bytes`, `outcome`; no content, query strings, addresses or keys |
| `family_eval` | written by root/sealed-eval.md (§4.1 there) |
| `panic` · `resume` · `stop` | §3.9 |
| `access` (v0.1.1) | Root: `access` (`none`\|`read_home`\|`workspace`\|`full`), `event`, `full`, `workspace` (a boolean, never a path, because the lineage is mirrored) on every workspace, full-mode or charter-driven access change (§3.17) |
| `escapee` | `after` (`panic`\|`stop`\|`cold_start`\|a slot name), `mark` (`zone-mark`), `pids`, `signalled` (always false). Written by a held-only Root (ADR-008, §3.5.4) when processes outside its hold still meet D1 after the zone empties. |
| `proposal` | `id`, `title`, `sha256`, `bytes`; the digest and size are of the proposal's `text`, which never enters the lineage (§4.1.3) |
| `audit` · `mutation` | owned by root/sealed-eval.md §4.1. The Root reads: `audit{tuple, score_permille, regression, trigger}` and `mutation{gate, tuple, total, rejected, controls_ok, pass}` |
| `sealed_draw` · `sealed_calibrate` · `sealed_g4` · `sealed_rotate` · `milestone` (M7) | owned by root/sealed-eval.md §4.1 (formats: m7-design.md §4.2). The Root reads `milestone{outcome, current}` to move `confirmed` or roll back (§3.7.2), and `sealed_g4{status, incumbent, candidate}` for the slice-based `audited` move |

**Reported by the kernel** (`lineage.append{kind, data, idem}`, `by:"kernel:<12>"`). The Root accepts every kind the kernel and evolution docs use (D7). Each record passes the generic checks: ICJ, ≤ 16 KiB, depth ≤ 8, and the privacy rules of §3.10.6. The Root checks required fields only for the kinds it interprets. "Required" means the key is present; null is allowed, because handoff promotions send `gate_job`, `scope` and `ext_set` as null. For the others, the fields belong to the doc named.

| Kind | Fields owned by | Required by the Root | Effect in the Root |
|---|---|---|---|
| `promotion` | kernel/supervise.md, evolution.md | `sha`, `from_sha`, `gate_job`, `scope`, `lkg` (bool), `body_binary` (sha256), `ext_set`, `components` [{name, sha256}]; optional `audit_required` (bool); from M7 optional `t0` (sha256, tuple v2, §3.7.1) and `milestone_required` (bool, set on a `lineage_gain` accept, kernel/gate.md §3.9) | `lkg:true` → tuple formed (§3.7.1); counted by the ledger |
| `gate` | kernel/gate.md §3.13 (agent-safe subset) | `gate_id`, `gate_kind`, `sha`, `verdict`, `reason` | recorded; a G4 canary trip (`reason:"canary"`) is a demotion trigger (§3.13.3) |
| `task_registered` · `task_admit` | kernel/gate.md, root/sealed-eval.md §3.6 rule 6 | `task_admit`: `family`, `label`, `n`, `n_sealed`, `form`, `author_sha`, `sealed_by`, `sealed_commit`, `model`; `task_registered`: `task_id` | recorded; the returned `seq` is the registration anchor (D3) |
| `task_suspended` | evolution.md | `task_id`, `reason` | recorded |
| `owner` | kernel/gate.md, kernel/supervise.md | `signal` (`accept`\|`bless`\|`reject`), `qid`, `target` | the Root checks that `qid` is an owner line it relayed (§3.15) whose first word is that signal. A kernel cannot invent an owner signal. Owner signals are optional and gate nothing (D4). |
| `body_rollback` | kernel/supervise.md | `from_sha`, `to_sha`, `reason` | recorded |
| `body_safe` | kernel/supervise.md | `on`, `sha`, `crashes` | `on:true` is a demotion trigger ("a crash loop on LKG", §3.13.3) |
| `fleet_spawn` · `fleet_stop` · `qualified` · `adopt` · `fleet_campaign` | kernel/supervise.md, evolution.md | — | recorded |
| `value_window` | evolution.md §3.14 | `window_start`, `window_end`, `evolution_spend_musd`, `accepted_tasks` | input to the share bound (§3.13.5) |
| `kernel_candidate` · `kernel_main` | kernel/supervise.md, evolution.md | — | recorded |
| `x_<name>` (`^x_[a-z0-9_]{1,40}$`) | the kernel that writes it | — | recorded. This lets an L3 kernel add a kind without a Root release; the Root never interprets it. |

The trust level of kernel-reported records is explicit in `by`. At L0–L2 the reporter is a maintainer kernel. From L3 it is agent code that passed the gate-of-gates. Either way, the honesty of promotions rests on the sealed audit (R6), not on these reports.

#### 3.10.6 Published view and privacy

`INIT_HOME/root/` is Root-written and read-only for the zone. It holds:
- `lineage.jsonl`, a byte-identical copy (rebuilt at start if it differs);
- `lineage.pub`;
- `charter.json` (§3.12.5);
- `ledger.json` (§3.13);
- `ceilings.json`, refreshed every 60 s;
- `status.json`, written on every change.

The agent reads its lineage, level and ceilings there, and can verify the chain. That is spec R4: "the agent can read it but not write it".

**Privacy.** The lineage is mirrored off the machine, so its records never contain:
- an absolute path, host name, user name, email, or anything read from git config;
- key material;
- the purpose text or the mirror URL;
- sealed-set content (root/sealed-eval.md keeps details in `sealed/`).

The Root refuses a kernel-reported record with `root.lineage` if any string contains the owner's home path or `@`-addresses outside a fixed allowlist (`agent@init.local`).

### 3.11 R4: the off-machine mirror

- **Target (CH-3, decided by the owner, D13).** The new private repository `nagisanzenin/INIT-Lineage`, as `[mirror] url = "git@github.com:nagisanzenin/INIT-Lineage.git"`, branch `lineage`. It is created at milestone M0. Only the Root pushes to it, with its own deploy key; nothing else has write access. Until then the charter template leaves `url = ""`, so the mirror is off (§3.12.2). A URL set before the repository exists makes pushes fail harmlessly and raise `mirror_failing` after 24 h. A `file://` URL is accepted only with `root.test_mode = true`, and in test mode any other URL counts as off, so a test never reaches a network. Test `file://` URLs percent-encode spaces, because C9 refuses whitespace.
- **Credentials.** A dedicated deploy key, generated at install:

  ```sh
  /usr/bin/ssh-keygen -q -t ed25519 -N "" -C init-root -f "$ROOT_HOME/keys/mirror_ed25519"
  ```

  The comment `-C init-root` replaces ssh-keygen's default comment, which is `user@host`. At M0 the `.pub` file is added as a deploy key with write access on `nagisanzenin/INIT-Lineage` only (GitHub deploy keys are scoped to one repository; root/egress.md §11). The Root never uses the owner's ssh-agent or SSH config, and the zone can reach none of them.
- **Repository.** The Root writes `ROOT_HOME/lineage/mirror.git` (bare) with plumbing only. Each commit's tree is:
  - `HEAD.json` (`{seq, hash, at}`);
  - `lineage.pub`;
  - `lineage-<NNNNNN>.jsonl`, chunks of 10,000 records.

  Only the last chunk changes, so each push adds at most one chunk-sized blob. `git gc --auto` runs after each push.
- **Hardened Root git prefix** `$RG`. The `$GIT` value is the same as kernel/supervise.md §3.11.2's, and it is recorded in `install.json`:

  ```sh
  env -i PATH=/usr/bin:/bin HOME="$ROOT_HOME/cache/git-home" LC_ALL=C TZ=UTC \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=/usr/bin/false \
    "$GIT" -c core.hooksPath=/dev/null -c user.useConfigOnly=true -c user.name=init-root -c user.email=root@init.invalid \
           -c core.sshCommand="ssh -F /dev/null -i $ROOT_HOME/keys/mirror_ed25519 -o IdentitiesOnly=yes -o BatchMode=yes \
                              -o ConnectTimeout=20 -o UserKnownHostsFile=$ROOT_HOME/keys/known_hosts -o StrictHostKeyChecking=accept-new" \
           -c protocol.allow=never -c protocol.ssh.allow=always -c protocol.file.allow=always \
           --git-dir="$ROOT_HOME/lineage/mirror.git"
  ```

  Why the variables are set as they are:
  - `GIT_CONFIG_GLOBAL=/dev/null` and an explicit identity: no data from the owner's git config reaches a commit or a request.
  - `TZ=UTC`: commit timestamps do not reveal the owner's time zone.
  - `ssh -F /dev/null`: the owner's SSH config is ignored.

  The SSH public-key exchange sends the key blob, not its comment. Git's protocol-v2 `agent=` string carries only git's version.
- **Push sequence:**

  ```sh
  C1=$(… | $RG hash-object -w --stdin)                               # current chunk, cut from lineage.jsonl (no temp file)
  T=$(printf '100644 blob %s\tHEAD.json\n100644 blob %s\tlineage-%06d.jsonl\n…' … | $RG mktree)
  C=$($RG commit-tree "$T" ${PARENT:+-p "$PARENT"} -m "lineage $SEQ $HASH12")
  $RG update-ref refs/heads/lineage "$C" ${PARENT:-""}
  $RG push --porcelain "$URL" "refs/heads/lineage:refs/heads/$BRANCH"   # never --force
  $RG ls-remote "$URL" "refs/heads/$BRANCH"                            # must equal $C
  ```

- **Schedule.**
  - Every `push_interval_s` (600) when there are new records.
  - At once after any of these kinds: `panic`, `ladder`, `rollback`, `kernel_upgrade`, `audit`, `charter`, `halt`, `crash_loop`. These urgent pushes bypass the backoff.
  - Pushes run on the mirror thread, with a timeout of 120 s per git call (the call is killed by its own handle).
- **Failures.**
  - A transient error retries with backoff from 60 s to 3600 s. 24 h after the first failure that follows the last success → alert `mirror_failing`.
  - A non-fast-forward rejection means the remote history was rewritten → alert `mirror_diverged`, and the Root stops pushing until the owner resolves it. It never force-pushes. As built, the `diverged` state lasts until the Root restarts.
  - Mirror events are not lineage records, which avoids a push feedback loop. Their state is in `status`.

### 3.12 R5: the charter

#### 3.12.1 Schema

The charter is `ROOT_HOME/charter.toml`, TOML 1.1, parsed with `toml` 1.1.8 into `serde` structs with `#[serde(deny_unknown_fields)]` everywhere. Money values are decimal strings (CONVENTIONS §5), parsed exactly as kernel/config.md §3.1 specifies. The egress parses them to integer micro-USD, which `GET /_root/ceilings` serves as `money.day_limit_musd` and `money.total_limit_musd` (root/egress.md §4.2). The `quotas.*` keys and their enforcement are this doc's (§3.14; root/egress.md §3.9 contributes platform notes).

| Key | Type | Default | Meaning · enforced by |
|---|---|---|---|
| `schema_version` | int | required | `1` |
| `purpose.text` | string | **required** | the owner's statement of what INIT is for; 1–4096 bytes. Published; the agent reads it in its head, after the constitution (awareness.md §3.3) |
| `constitution.text` | string | the built-in text (awareness.md §3.3, **PENDING OWNER (CONVENTIONS §6 OW-1)**) | installed byte-identical as `INIT_HOME/kernel/CONSTITUTION.md`; 1–32,768 bytes |
| `autonomy.max_level` | int 0–4 | **required**; the template writes **`4`** (CH-1, decided by the owner, D13) | cap on the level (R7). The effective cap is `min(max_level, SUPPORTED_MAX_LEVEL)`; the Phase 0 Root build has `SUPPORTED_MAX_LEVEL = 0` (spec §19). The agent always starts at L0 and climbs only on its measured record (§3.13). |
| `money.day_usd` | money | `"5.00"` (template value, D13) | hard daily ceiling · root/egress.md §3.3 |
| `money.total_usd` | money | `"25.00"` (template value, D13) | hard lifetime ceiling; `sealed.reserve_usd` of it is kept for the Root's sealed runs · root/egress.md §3.3 |
| `money.day_starts_at_utc_offset` | string | `"+00:00"` | the owner day for every Root and kernel daily window (kernel/proxy.md §3.8 rule) |
| `quotas.max_instances` | int | 12 | concurrent zone instances of every kind · broker (§3.14) |
| `quotas.parallel_bodies` | int | 4 | the fleet size N at L2+; L0 and L1 always have 1 · broker, with egress's session count as a cross-check |
| `quotas.cpu_core_hours_day` | int | 48 | zone CPU per owner day; 0 = unmetered · broker (§3.14) |
| `quotas.disk_min_free_gib` | int | 20 | free-space floor (the owner's 20 GB rule) · broker and watcher (§3.14) |
| `quotas.init_home_max_gib` | int | 80 | size ceiling of `INIT_HOME` · watcher |
| `quotas.store_max_gib` | int | 8 | the Root's blob store · Root |
| `quotas.disk_pause_grace_s` | int | 600 | how long free space may stay below the floor before the R1 pause (§3.6.6) |
| `actions.<class_id>` | tables | the built-in `providers` (DeepSeek only, spec §19) and `crates` (D10; built in with `paths = ["*"]` on both hosts; egress.md splits the paths per host) | **schema owned by root/egress.md §3.5** (one table per class, with `min_level` and, for model classes, the per-level model allowlist); validated by `egress::validate_actions` |
| `sandbox.extra_read` | [string] | `[]` | extra read-only paths for every zone profile (D9; §3.3.1). Replaces kernel/config.md's `[sandbox] extra_read` |
| `sandbox.read_home` | bool | `true` | v0.1.1 (ADR-009): the live body and fleet members read the owner's home except the deny list. With `false` there is no home read outside full mode, and with no workspace the access is `none`, the v0.1.0 behaviour (§3.17.2) |
| `sandbox.read_deny` | [string] | `[]` | v0.1.1: extra deny-list entries, added to the built-in list; denied for reads and writes in every mode (tail Z2) |
| `sandbox.allow_full` | bool | `true` | v0.1.1: whether `init --full` and `/full on` are accepted (§3.17.2) |
| `panic.watch_paths` | [string] | `[]` | extra panic files (§3.9.1) |
| `panic.mirror_ref` | bool | `false` | panic when the mirror has `refs/heads/panic` |
| `panic.mirror_poll_s` | int | 300 | poll interval for the above |
| `notify.cmd` | [string] | `[]` | argv run on every alert, outside any sandbox, with the alert JSON on stdin, a 10 s timeout and no shell; e.g. `["/usr/bin/osascript","-e","display notification \"INIT needs you\" with title \"INIT\""]` |
| `mirror.url` | string | `""` (off) when the key is absent. The template writes `""` until M0, with `"git@github.com:nagisanzenin/INIT-Lineage.git"` (CH-3, D13) as a comment. | off-machine mirror target (§3.11) |
| `mirror.branch` | string | `"lineage"` | remote branch |
| `mirror.push_interval_s` | int | 600 | |
| `ladder.l1.*` … `ladder.l4.*` | ints | §3.13.2 | unlock criteria; may only be made **stricter** than the built-in values |
| `sealed` | table | root/sealed-eval.md | **schema owned by root/sealed-eval.md** (schedule, money cap, δ_S) |
| `root.test_mode` | bool | false | enables `file://` mirrors, `INIT_FAULT`, timings below the production minimums, and, in test builds only, the fake clock and test ledgers (§3.16) |
| `root.kernel_hello_timeout_s` · `kernel_ready_timeout_s` | int | 30 · 1200 | §3.6.3 |
| `root.hb_interval_s` · `hb_stale_s` | int | 5 · 30 | |
| `root.backoff_initial_ms` · `backoff_max_ms` · `backoff_reset_after_s` | int | 1000 · 60000 · 300 | |
| `root.crash_window_s` · `crash_limit` · `safe_crash_limit` | int | 600 · 3 · 2 | §3.6.4 |
| `root.config_error_exits` · `halt_retry_s` | int | 2 · 21600 | §3.6.6 |
| `root.kernel_probation_s` | int | 1800 | §3.8 |
| `root.safe_auto_clear_s` | int | 86400 | §3.6.4 |
| `root.stop_grace_s` · `swap_prepare_timeout_s` | int | 40 · 1800 | |
| `root.store_keep_tuples` | int | 5 | §3.7.1 |
| `root.kernel_lineage_per_hour` · `lineage_verify_tail` | int | 600 · 1000 | §3.10 |
| `root.replay_frames` · `replay_bytes` · `owner_queue_max` | int | 2000 · 262144 · 200 | §3.15 |

#### 3.12.2 Template (written by `install` when no charter exists)

The template carries the owner's decisions of 2026-10-20 (D13): `max_level = 4`, panic demotes one level (fixed behaviour, no key), the mirror `nagisanzenin/INIT-Lineage` (as a comment: `url = ""` keeps the mirror off until the repository exists at M0, an orchestrator decision), $5 a day and $25 in total. The soft budgets stay in `kernel/config.toml` (kernel/config.md), below these ceilings.

```toml
# ~/.init-root/charter.toml — the owner's charter. The agent can read a published copy and can never write it.
# After editing: `init-root charter check` (validate + diff), then `init-root charter check --apply`.
schema_version = 1

[purpose]
text = "REPLACE ME: what INIT is for, in your words."

[autonomy]
max_level = 4          # the cap. The agent starts at L0 and climbs only on its measured record (R7).

[money]                # hard ceilings (root/egress.md); soft budgets stay in kernel/config.toml
day_usd = "5.00"
total_usd = "25.00"
day_starts_at_utc_offset = "+00:00"

[quotas]
max_instances = 12
parallel_bodies = 4
cpu_core_hours_day = 48
disk_min_free_gib = 20
init_home_max_gib = 80
store_max_gib = 8

[actions.providers]    # schema: root/egress.md §3.5; `crates` is built in (D10)
kind = "model"
min_level = 0
destinations = ["api.deepseek.com:443"]
methods = ["GET", "POST"]
paths = ["chat/completions", "beta/chat/completions", "models", "user/balance"]
requests_per_day = 100000
bytes_per_day = 8000000000
rate_per_min = 600

[actions.providers.models."deepseek-flash"]
min_level = 0
input_musd_per_mtok = 300000
cache_read_musd_per_mtok = 6000
output_musd_per_mtok = 1200000
reserve_output_tokens = 65536

[sandbox]
extra_read = []
read_home = true       # v0.1.1: read your home, except secrets (ADR-009); false = no home reads outside a workspace or full mode
read_deny = []         # extra paths the agent may never read or write, e.g. "/Users/you/Private"
allow_full = true      # allow `init --full` / `/full on`: write the whole home except secrets and the Root

[mirror]
url = ""   # off until M0. Then: "git@github.com:nagisanzenin/INIT-Lineage.git", with keys/mirror_ed25519.pub as its deploy key
branch = "lineage"

[notify]
cmd = ["/usr/bin/osascript", "-e", "display notification \"INIT needs you\" with title \"INIT\""]

[constitution]
text = '''
…the built-in text (awareness.md §3.3), shown verbatim so you can read and change it…
'''
```

**Dev-checkout installs (M7; provisional owner decision 2026-10-20, owner may override; CONVENTIONS §6; m7-design.md §3.1.2).** When `install` runs with `--seed-source` pointing into a dev checkout, the template's `read_deny` lists the checkout's top-level directory (the git work tree that holds the seed source) as a canonical absolute path. Otherwise, under ADR-009's `read_home`, the live body could read the S v1 pack generators under `root/sealed/packs/` and their `.git` objects. Tail Z2 then denies the checkout for reads and writes in every mode, and §3.17.1's `deny_list` rule refuses it as an `init` workspace, which is acceptable because INIT is developed with Claude Code, not with itself. The owner may remove the entry. On the owner's machine the entry is the dev checkout `~/Documents/INIT Agent`, written as an absolute path (C15); an existing charter gets it through the owner-only edit flow (§3.12.4).

#### 3.12.3 Validation

`charter::load(path) -> Result<(Charter, Digest), Vec<CharterError>>` reports every error together, each with its TOML key path:

| Rule | Check |
|---|---|
| C1 | `schema_version == 1`; unknown keys are errors |
| C2 | `purpose.text` is 1–4096 bytes of UTF-8 and is not the template's `REPLACE ME` text |
| C3 | `constitution.text` is 1–32,768 bytes of UTF-8 |
| C4 | `autonomy.max_level` ∈ 0..=4 |
| C5 | money strings match `^[0-9]{1,7}(\.[0-9]{1,6})?$`; `day_usd > 0`; `total_usd ≥ day_usd`; the offset matches `^[+-](0[0-9]\|1[0-4]):[0-5][0-9]$` |
| C6 | `max_instances` 2–64; `parallel_bodies` 1–32; `disk_min_free_gib` ≥ 1 (0 in test mode); `init_home_max_gib` ≥ 10; `store_max_gib` 1–64; `cpu_core_hours_day` 0 or ≥ 1. Warning if `parallel_bodies + 7 > max_instances` (a full fleet plus a G4 gate would not fit). |
| C7 | every `ladder.*` value is at least as strict as the built-in value of §3.13.2 → otherwise `charter.ladder.looser` |
| C8 | each `panic.watch_paths` entry is absolute and lies **outside `INIT_HOME`**, because the zone must never be able to create a panic file; at most 8 entries |
| C9 | `mirror.url` is `""`, `ssh://…`, or `user@host:path`, with no whitespace; `file://` only in test mode. `mirror.branch` passes `git check-ref-format`. `push_interval_s` and `panic.mirror_poll_s` 60–86,400 (from 1 in test mode) |
| C10 | `notify.cmd[0]` is absolute; at most 16 elements |
| C11 | `actions` passes `egress::validate_actions` (root/egress.md) |
| C12 | `sealed` passes `sealed::validate` (root/sealed-eval.md) |
| C13 | `root.*` values stay within the production ranges, unless `test_mode` (then each is only ≥ 0). The production minimums are: `kernel_hello_timeout_s` ≥ 10, `kernel_ready_timeout_s` ≥ 60, `hb_interval_s` ≥ 1, `hb_stale_s` ≥ 3, `backoff_initial_ms` ≥ 100, `backoff_max_ms` ≥ 1000, `backoff_reset_after_s` ≥ 60, `crash_window_s` ≥ 60, `crash_limit` ≥ 1, `safe_crash_limit` ≥ 1, `config_error_exits` ≥ 1, `halt_retry_s` ≥ 600, `kernel_probation_s` ≥ 300, `safe_auto_clear_s` ≥ 3600, `stop_grace_s` ≥ 5, `swap_prepare_timeout_s` ≥ 60, `store_keep_tuples` ≥ 2, `kernel_lineage_per_hour` ≥ 1, `lineage_verify_tail` ≥ 100, `replay_frames` ≥ 100, `replay_bytes` ≥ 65,536, `owner_queue_max` ≥ 10. Also `hb_stale_s ≥ 3 × hb_interval_s` and `backoff_max_ms ≥ backoff_initial_ms`. |
| C14 | each `sandbox.extra_read` entry is absolute, canonical and existing. It is not `/`, not the owner's home itself, and neither inside nor an ancestor of `ROOT_HOME` or `INIT_HOME`. At most 8 entries. Warning if it contains `~/.ssh`, `~/.aws`, `~/.gnupg` or `~/Library/Keychains`. |
| C15 | v0.1.1: each `sandbox.read_deny` entry is an absolute path (no `~/` form), at most 1,024 bytes, with no NUL and no `.` or `..` component; it need not exist (a deny on a missing path still blocks its creation). It is not `/` and not the owner's home itself. An entry that is an ancestor of `INIT_HOME`, equal to it or inside it is refused, because it would lock the agent out of its own world. At most 32 entries. `read_home` and `allow_full` are booleans. |

`charter_sha256 = hex(SHA-256(ICJ(serde_json::to_value(&charter_with_defaults))))`. It is the same rule as kernel/config.md's digest, so comments and formatting do not change it. One exception (v0.1.1, SBX-009): the three ADR-009 keys `sandbox.read_home`, `read_deny` and `allow_full` are left out of the effective charter while they hold their defaults, so a v0.1.0 charter keeps its digest (its `charter` records and `hello.charter_sha256`). The digest is published in `charter.json` and `status.json` and given to the kernel in `hello` (kernel/gate.md records it).

#### 3.12.4 Owner-only edit flow

1. **Edit.** The owner edits `ROOT_HOME/charter.toml` with any editor. Editing alone changes nothing.
2. **Check.** `init-root charter check` validates the file, prints the effective values (defaults filled in) and a diff against `state/charter.active.toml`, and exits 0 (valid) or 2 (invalid).
3. **Apply.** `init-root charter check --apply` shows the diff and asks `apply? [y/N]` on a TTY; a non-TTY run needs `--yes`. It then sends `charter.apply` over `owner.sock`. The Root does not trust the client: it re-reads the file itself, re-validates it, and then:
   - writes `state/charter.active.toml` with `write_atomic`;
   - appends `charter{digest, changed, …}`;
   - republishes the view;
   - applies the effects listed below.

   If the file is invalid, nothing changes and the errors are returned.

Only the owner can apply:
- the file lives in `ROOT_HOME`, which the zone can neither read nor write (Z1);
- `charter.apply` is accepted only on `owner.sock`, which the zone cannot reach (Z6), from a peer that is not sandboxed;
- the agent may file a proposal (`proposal.file`, §4.1.3), which is never applied automatically (spec §13). It is only stored in `state/proposals/`, recorded and alerted; nothing in the Root reads it back.

| Change | Effect |
|---|---|
| `constitution.text` | `kernel/CONSTITUTION.md` is rewritten atomically. The kernel tells the body, and the new text enters the head at the next boot or reload: a deliberate prefix change (body/log-context.md §3.8.2). |
| `autonomy.max_level` below the current level | `ladder{direction:"cap"}`. No rollback; this is an owner choice, not a bad signal. Climbing resumes under the new cap, with hold times counted from the cap. |
| money ceilings | `egress::reload()`. A lowered ceiling applies at the next admission. The kernel clamps its soft budgets to the new ceiling at its next `ceilings` read; nothing refuses to start and nothing halts (D6). |
| quotas | apply at the next spawn. A lowered `parallel_bodies` is sent to the kernel (`ceilings` event), and the kernel retires bodies; the Root does not kill them. |
| `ladder.*` | apply at the next evaluation |
| `mirror.*` | the next push goes to the new target, starting with a full push |
| `root.*` | apply at the next kernel start |
| `sandbox.read_home`, `sandbox.read_deny`, `sandbox.allow_full` (v0.1.1) | the access state is recomputed (§3.17.2), and the live body gets a new profile through a graceful restart at the next idle boundary (§3.17.3). `allow_full = false` while full mode is on ends it. The new deny entries reach every other instance at its next spawn. |

#### 3.12.5 Published view

`INIT_HOME/root/charter.json` is the active charter as ICJ, with defaults filled in, plus `charter_sha256` and `supported_max_level`. Some fields are replaced:
- `mirror.url` becomes `"set"` or `""`;
- `panic.watch_paths` and `notify.cmd` become their counts;
- `constitution.text` is replaced by `constitution: {"sha256": "sha256:<hex>"}` (awareness.md's `constitution.sha256`; the text itself is `kernel/CONSTITUTION.md`);
- each `actions.<class_id>` table keeps its id, `kind` and `min_level`, and drops `destinations` and prices (awareness.md prints class ids only; prices are in `ceilings.json`).

`sealed.margin_max` is a ratio in TOML. ICJ allows integers only, so it is published, and digested, as `sealed.margin_max_permille`.

The other view files are specified in §4.5.

### 3.13 R7: the autonomy ledger

#### 3.13.1 How the level is computed

The level is a **pure function of the lineage** plus the current time. No human input enters it, except through the charter's cap.

```text
state  = { level, entered_seq, entered_at, incident: Option<seq>, mode, panicked }
fold(state, record)      updates counters since entered_seq; applies `ladder` records
evaluate(state, now)     → Up(level+1, criteria) | Down(trigger) | Cap(max) | None
cap                      = min(charter.max_level, SUPPORTED_MAX_LEVEL)
```

- **Evaluation** runs after every appended record and every 60 s. A transition is appended as a `ladder` record first. Then, within the same second, three things happen: the kernel gets a `level` event on the control link (kernel/proxy.md and kernel/selfmap.md switch on it); the Root calls `egress::set_level(level, safe_mode)`, which it also calls on every mode change, so egress gates classes and models and `GET /_root/ceilings` carries the new `level`; `ledger.json` and `status.json` in the view are rewritten. That answers proxy.md Q-RT1 and selfmap.md Q-RT1.
- **Climbing** needs all of these at once:
  - no open incident;
  - `mode = normal`;
  - not panicked;
  - `level + 1 ≤ cap`;
  - the criteria for `level + 1`.

  The level rises by at most one step per evaluation.
- **Counts** run from the level's entry record (`entered_seq`), which is the latest `ladder` record or the genesis record. A demotion therefore restarts the track record.
- **Verification.** `init-root ledger --verify` replays the lineage and checks four things:
  - every `ladder up` record's criteria held at its `at`, given only the records before it;
  - every demotion trigger is followed by a `ladder down` in the same incident;
  - no `ladder` record lacks a justification;
  - the final level equals `ledger.json`.

  A forged but validly signed `ladder up` therefore still fails verification. The agent can run the same check on the published view.

#### 3.13.2 Unlock criteria

These are placeholders until Phase 1 calibrates them (spec §15). Each is a charter key under `[ladder.lN]` that may only be made stricter (C7).

| To | Key | Built-in value | Definition over records since `entered_seq` |
|---|---|---|---|
| L1 | `min_promotions` | 20 | `promotion{lkg:true}` records with distinct `sha` |
| L1 | `audit_clean_days` · `min_audits_in_window` | 14 · 1 | in `[now − 14 d, now]`: at least 1 `audit` record and 0 with `regression:true` (an empty window does not count as clean). This window spans the whole chain, not only the records since `entered_seq`: every regression demotes and resets `entered_seq`, so a count from it would be vacuous (RT-L3). |
| L1 | `max_demoting_hits` | 0 | `ceiling_hit{demotes:true}` records |
| L2 | `min_days` | 30 | `now − entered_at` ≥ 30 days |
| L2 | `min_audits` · `trend_min_permille` | 4 · 0 | audits A₁…Aₙ, n ≥ 4, with scores sᵢ (permille); `h = ⌊n/2⌋`; `trend = mean(s_{h+1..n}) − mean(s_{1..h})` in integer permille; requires `trend ≥ 0` and no `regression:true` |
| L3 | `min_days` | 60 | `now − entered_at` ≥ 60 days |
| L3 | `min_mutation_runs` · `mutation_pass_permille` | 1 · 1000 | `mutation` records: at least 1, and every one has `pass:true` (rejected = total, controls accepted) |
| L4 | `min_days` | 60 | spec §15 says only "L3 held". 60 days is this doc's placeholder, matching L2→L3. Also requires `charter.max_level = 4`. |

**M7 proposal for L1 (m7-design.md §3.4; for the Root release that raises `SUPPORTED_MAX_LEVEL` to 1, which stays gated by roadmap R14).** T0 promotions cost $0 and skip G4, so 20 of them prove nothing. The proposed L1 criteria: ≥ 20 promotions **of which ≥ 3 passed an S-backed G4** (any accept reason, from `sealed_g4` records); ≥ 1 audit **or slice-based `audited` move** (§3.7.2) in the 14-day window and no regression in it; **0 canary, tamper or leak trips** at the current level; no ceiling violations (`max_demoting_hits` 0). They are tested at $0 on test ledgers (M7-L04). L1 cannot be reached live inside the $5 program; the release that ships these criteria calibrates them first.

#### 3.13.3 Demotion

**Triggers** (spec §15) and the records that carry them:

| Trigger | Detected by | Record |
|---|---|---|
| sealed-audit regression beyond δ_S | root/sealed-eval.md | `audit{regression:true}` |
| mutation-suite failure in a new gate | root/sealed-eval.md | `mutation{pass:false}`, for an agent-built candidate kernel or the periodic run against the live gate. As built (WP24 N), R7 demotes only for `source:"agent"`; a maintainer-built candidate's failure alerts (`gog_fail`). An underpowered gog (`mutation{pass:null, underpowered:true}`, S below 12 tasks; provisional owner decision 2026-10-20, build-deviations §3.15) is neither a trigger nor a mutation run for L3 |
| an attempt to exceed a ceiling or a policy class, as D6 defines it (below) | root/egress.md (classes, money); the broker (level-gated requests) | `ceiling_hit{demotes:true, rule}` |
| a crash loop on LKG | the Root (kernel crash loop on the LKG tuple, §3.6.4); the kernel (body crash loop on body LKG → `body_safe{on:true}`) | `crash_loop{action:"demote"}`, `body_safe`. A `body_safe{on:true}` while the system is already in safe mode is not a trigger. |
| a panic (CH-2, decided by the owner: demotes one level) | the Root | `panic`. It always costs one level, even inside an open incident. |
| a canary trip: a G4 canary scored 1000 in any gate (evolution.md (d), root/sealed-eval.md §9); a canary in S already counts as an audit regression | the kernel's gate | `gate{reason:"canary"}` |

**What counts as an "attempt to exceed" (D6).** Only these three demote. root/egress.md §3.6 classifies each egress refusal and writes `ceiling_hit{demotes, source:"egress"}`. The broker classifies its own refusals. R7 consumes only the `demotes` flag and the `rule`.

| Rule | Refusal | Reported by |
|---|---|---|
| (a) | an R3 action-class or policy violation: `class.unknown`, `class.destination`, `class.method`, `class.path`, `class.provider_tool`; and `class.volume` or `class.rate` on a class with external effects (`git-push`, `email`, a non-GET `http` class) | root/egress.md |
| (b) | a level-gated request refused by the Root: `class.level` or `class.model` above the level at egress; at the broker, a second live `body` at L0–L1, `system.swap` from an agent source below L3, or `sealed.family_eval` below L3 (`root.level`). The broker's record for a level refusal is `ceiling_hit{ceiling:"level", code:"root.level", limit:<current level>, attempted:<level needed>, demotes:true, rule:"b", source:"broker"}`. | root/egress.md; the broker |
| (c) | a money-ceiling refusal (`ceiling.day`, `ceiling.total`) where the kernel **bypassed its guard**. root/egress.md §3.6 defines it: the `x-egress-guard: epoch=<E>` header is missing, or `E` is stale (out of the 1,024-epoch ring, or more than 30 s behind), or at epoch `E` the request already did not fit, so the guard should have refused it | root/egress.md |

Everything else is logged and does not demote:
- ordinary ceiling hits, including the peak-price gap and `ceiling.lane` on sealed lanes, logged once per ceiling per owner day as `ceiling_hit{demotes:false}`;
- `class.volume` and `class.rate` on read-only classes;
- the quotas: CPU, disk, `INIT_HOME`, `max_instances`, and parallel bodies above N at L2+.

When a ceiling is lowered, the kernel clamps its soft budgets to it, and nothing refuses to start (D6, §3.12.4).

**Procedure** (one demotion per incident):

1. **Coalesce.** If an incident is open, link the trigger to it and stop. As built, an incident opens at its `ladder down` record and runs until the next `zone_start` (the rollback target's kernel is `ready`; "its body runs" is not observed separately), or 600 s on the Root clock, whichever comes first. A trigger stays pending until a `ladder down` answers it, and a pending trigger also absorbs later ones. A burst of ceiling hits therefore costs one level, not five; each coalesced hit still counts against L1's `max_demoting_hits`.
2. **Record.** Open the incident and append `ladder{direction:"down", cause:"demotion", from, to:max(from−1, 0), trigger:{kind, seq}}`. At L0 the record still says `0→0`, and the rollback still happens.
3. **Roll back** to the **audited-good** tuple (§3.7.3). For a panic this step runs at resume (§3.9.5). A mutation-suite failure does not roll back, because nothing was swapped (testing.md RT-L7). While the Root is `Stopped` or `Halted`, the pointers are rolled back now and nothing is started. While it is `Panicked`, only the `ladder` record is written, because resume rolls back.
4. **Safe mode,** if the trigger was a crash loop on LKG, or if this is the third demotion at L0 within 24 h (`ladder down` records from 0, folded from the lineage, so a Root restart keeps the count; the `demotion_loop` alert). **Provisional owner decision 2026-10-20 (build-deviations §3.9):** a kernel that repeats a violation after every restart enters system safe mode on the crash-loop path instead of driving a rollback loop; from there safe mode clears after 24 h healthy, or a crash loop ends in `Halted{crash_loop_safe}` (§3.6.4).
5. **Apply the new level's permissions:** egress classes, the broker's quotas, and the `level` event to the kernel.
6. **Restart the hold time:** `entered_seq` becomes this record.
7. **Alert** `demoted`, and push the mirror at once.

#### 3.13.4 Permissions per level

Legend: ✓ granted · — not granted. **Bold** marks the security-relevant enforcement point, the one that holds even against a kernel that is agent code (L3+).

| Capability | L0 | L1 | L2 | L3 | L4 | Enforced by |
|---|---|---|---|---|---|---|
| Evolve the body (T0, T1, T2) through the kernel gate | ✓ | ✓ | ✓ | ✓ | ✓ | kernel (gate, supervise) |
| Add crates without approval (ESC-1) | ✓ | ✓ | ✓ | ✓ | ✓ | kernel; the network fetch through **egress**, built-in class `crates` (D10, root/egress.md) |
| Read `/opt/homebrew` except `var/` and `etc/` (ESC-2) | ✓ | ✓ | ✓ | ✓ | ✓ | **sandbox**: the Root-embedded `body` and `kernel` profiles (section 8) |
| Model calls to the charter's providers | ✓ | ✓ | ✓ | ✓ | ✓ | **egress** |
| Route models per subtask among charter-allowed models | — (the L0 model list) | ✓ | ✓ | ✓ | ✓ | **egress** model allowlist per level (`actions.providers.models.*.min_level`, root/egress.md §3.5) and the kernel proxy |
| Allocate its own daily budget across tasks and gates | — | ✓ | ✓ | ✓ | ✓ | kernel soft caps; **egress** hard ceilings above them |
| Write its own tasks into the pool (spec §17) | — | ✓ | ✓ | ✓ | ✓ | kernel gate + admission rules (root/sealed-eval.md); provenance in **lineage** |
| Live bodies at once | 1 | 1 | N = `quotas.parallel_bodies` | N | N | **Root broker** (§3.14) |
| Archive and HGM-style parent selection; budget share grows with value delivered | — | — | ✓ | ✓ | ✓ | kernel, within the Root's share bound (§3.13.5) and the **egress** ceilings |
| Owner signals `/accept`, `/bless`, `/reject` | optional at every level; they never gate anything (D4) | | | | | kernel; the Root verifies their origin (§3.10.5 `owner`) |
| Evolve the kernel (`system.swap`, gate-of-gates) | — | — | — | ✓ | ✓ | **Root** (level check, §3.8) + **R6** gate-of-gates |
| Model as a component (adapters, fine-tunes on owned infrastructure) | — | — | — | — | ✓ | **egress** classes with `min_level = 4`, inside the money ceilings |
| External-action classes | classes with `min_level` ≤ the level (root/egress.md §3.5), up to the charter's list | | | | | **egress (R3)** |
| Zone instances at once | `quotas.max_instances` at every level | | | | | **Root broker** |
| Lineage, panic, charter, sealed audit | unaffected by level; never reachable from the zone | | | | | **Root** |

In system safe mode, the effective permissions are L0's minus evolution (§3.6.4), whatever the ledger level.

#### 3.13.5 The evolution-share bound (L2+)

Spec §15 grows the L2 budget share "with value delivered (cost per accepted task)". The Root fixes the formula, using evolution.md §3.14's proposal; evolution.md defines the inputs.
- The kernel reports one `value_window` record for each 30-day window: evolution spend and accepted tasks. Here an accepted task is one whose frozen check scored 1000 on a version that is now audited-good. Then `cpat = evolution_spend_musd ÷ max(accepted_tasks, 1)`.
- `evolution_share_max_permille` starts at 600 when L2 is entered.
  - It rises by 50 for each window in which `cpat` fell or stayed flat against the previous window and the sealed-audit trend (§3.13.2) was non-negative.
  - It falls by 100 on each demotion.
  - It is clamped to [600, 900].
- The Root appends a `share` record on each change, publishes the bound in `ledger.json`, and sends it in the `level` event. The kernel enforces it as a soft budget. The hard ceilings never move. A kernel that over-reports value can therefore raise only its share of money the charter already allows.

### 3.14 Quotas: enforced by the broker and the watcher

The quota keys (`quotas.*`, §3.12.1) and their enforcement are this doc's. Every zone process is started and reaped by the broker, which makes it the one enforcement point. root/egress.md §3.9 records platform notes and keeps no duplicate keys. Quota enforcement refuses spawns and, when it must stop the zone, stops the instances the broker holds by id. It never signals by pattern (CONVENTIONS §1).

| Quota | Default | Counted how | When exceeded | Demotes (D6) |
|---|---|---|---|---|
| `max_instances` | 12, of which `sealed.parallel` (2) are reserved for sealed runs, so the zone may hold 10 | live instances of every kind (kernel, bodies, probes, builds, checks, git reads; sealed runs in their reserve) | `spawn` → `root.quota{quota:"instances"}` | no |
| parallel bodies | L0–L1: 1; L2+: `quotas.parallel_bodies` (4) | instances of kind `body`. A `spawn` with `replaces: <instance>` (the B of an overlapped handoff) may exceed the limit by one while the replaced instance lives. | L0–L1: `root.level{quota:"parallel_bodies"}`, rule (b); L2+ above N: `root.quota{quota:"parallel_bodies"}` | L0–L1 yes; L2+ no |
| CPU per owner day | `cpu_core_hours_day` 48 | `ru_utime + ru_stime` from `wait4` at each instance's end, attributed to the day of the end. Long-lived bodies are counted when they exit; Phase 1 may sample live leaders with `proc_pid_rusage` (root/egress.md §3.9). | `build`, `check` and `probe` refused (`root.quota{quota:"cpu_day"}`) until the next owner day | no |
| disk floor | `disk_min_free_gib` 20 (`statvfs` on the `INIT_HOME` filesystem) | checked at each `build`/`check`/`probe` spawn and every 60 s | those spawns refused (`root.quota{quota:"disk"}`), alert `disk_low`. Below the floor for `disk_pause_grace_s` (600 s): the R1 pause (§3.6.6). Below 512 MiB: `Halted{disk}`, because the Root could no longer record lineage. | no |
| `INIT_HOME` size | `init_home_max_gib` 80 | metadata-only walk every 600 s (R-F2 exception) | `build`/`check`/`probe` refused (`root.quota{quota:"init_home"}`); alert | no |
| store size | `store_max_gib` 8 | blob bytes | `store.put` refused after pruning (`root.store`) | no |

The broker is the authoritative count of live bodies. Every live body is a `spawn{kind:"body"}`, so the Root learns each start and stop without help (asked by root/egress.md §9). `ceilings.get` (§4.1.3) and `INIT_HOME/root/ceilings.json` return all of these, embedding egress's money block (`egress::ceilings()`, root/egress.md §4.2). The kernel uses them to stay below the limits.

#### 3.14.1 Memory watchdog and workload disk floor (MEM-RUNAWAY, 2026-10-22)

Seatbelt has no memory limit and macOS does not enforce `RLIMIT_AS`, so one sandboxed process can grow through RAM into swap, and swap grows on the system volume until the disk is full. In M7 D one `python3.9` reached 43.9 GB of compressed memory and free disk fell to 0.24 GB (measurements/m7-results.md DV-4). The Root therefore polls its workloads (`memwatch.rs`).

- **Keys** (charter, Root-owned, so the agent cannot raise them): `sandbox.mem_cap_mib`, default 4096, bounds 512–65536; `sandbox.disk_floor_gib`, default 5, bounds 1–1024 (rule C16; test mode lowers the minimums to 64 MiB and 0, and 0 turns the floor off). At their defaults both are left out of the effective JSON, so existing charters keep their digest. `quotas.disk_min_free_gib` stays the install-time floor.
- **Who is watched.** A *leader* is a process the Root holds: every zone instance (kernel, body, probe, build, check, git read) and every sealed run. A *unit* is the process tree of one direct child of a leader: one tool call of a body, one `rustc` of a build. A member stays in its unit after its parent exits (it is remembered by pid and start time), and an orphan still in the unit root's process group joins it. Each unit's physical footprint (`proc_pid_rusage(RUSAGE_INFO_V4).ri_phys_footprint`, resident plus compressed) is summed every second.
- **Memory cap.** A unit over the cap is stopped (SIGSTOP, top-down, then once more for children born meanwhile), recorded, and killed (SIGKILL, bottom-up), member by member. Its leader and the sibling units keep running. A leader whose own footprint passes the cap is killed as a whole instance (`sys::signal`, held), except the kernel: only the kernel's child units can be killed. In a sealed run that ends the run as `mem_cap`, which scores like a crash in either arm (DV-6).
- **Disk floor.** Below the floor (`statvfs` on `INIT_HOME`'s volume, where swap also grows), the broker refuses every spawn except a body, with `root.quota{quota:"disk_floor", free_mib, floor_mib}`. A sealed run is refused before it spawns, and a running one is stopped. The zone watchdog kills every unit, and every leader that is a workload itself (build, check, git read). The kernel and the bodies stay up, so the agent can tell the owner. The alert `disk_floor` is raised once per low spell, and the notice `disk_floor` (info) when free space is back.
- **Reporting.** Each kill is appended, *before* the signal, to the events file of the instance: `INIT_HOME/root/mem-events.jsonl` for the zone (Root-written, readable by every body through `PUB`), or `<run root>/mem-events.jsonl` for a sealed run (created before the spawn and written only through the Root's own handle). One ICJ line per event: `{v, event: mem_cap|disk_floor|disk_low|disk_ok, wall_ms, instance, kind, pgid, scope: unit|instance, footprint_mib, cap_mib, free_mib, floor_mib, pids}`, where `pgid` is the unit root's pid, which is the tool call's process group. While the disk is low, `disk_low` is repeated every 5 s. A body gets `INIT_MEM_CAP_MIB` and `INIT_MEM_EVENTS` from the Root at spawn (tools.md §3.5.8). The owner gets an alert `mem_cap` (a notice, `alerts.jsonl`, `notify.cmd`) naming the instance, the process group, the footprint and the cap.
- **Signals (the ADR-008 amendment).** Leaders are signalled only through `sys::signal` (held pids). Unit members are signalled only through `sys::signal_tracked`, whose one caller is `memwatch::kill_members`. Right before each syscall it re-reads the pid's start time and uid and requires them unchanged, our uid, and `sandbox_check` = sandboxed. pid ≤ 1 and the Root itself are never signalled, and only SIGSTOP and SIGKILL are sent. tests/rsafe.rs audits all of this.
- **Residual gap.** A process that calls `setsid` and loses its parent within one poll (1 s), before it was ever seen, is in no unit and is not measured. It is still an `escapee` (§3.5.4).
- **Linux.** Not built. TODO: a cgroup v2 per zone instance and sealed run, with `memory.max` = the cap, `memory.swap.max` = 0, and `memory.events` (`oom_kill`) for the report.

### 3.15 Owner I/O moves to the Root (decision)

**Decision: the owner channel moves from the kernel to the Root**, as recommended:
- **Panic must not depend on an evolvable kernel.** From L3 the kernel is agent code. A channel served by the kernel could drop, delay or forge `/panic` and the owner's view.
- **The socket must live where the zone can neither reach nor replace it.** `run/owner.sock` was in the kernel's own directory.
- **Kernel restarts and upgrades should not cut the owner off.** The Root keeps the client connected, keeps the replay ring and queues input.

The cost is one relay hop of about 50 µs per frame and about 100 lines of Root code.

**What moves** from kernel/supervise.md §3.10 and §4.6:
- `owner.sock` → `ROOT_HOME/run/owner.sock`;
- `init-kernel attach` → `init-root attach`;
- `init-kernel panic` → `init-root panic`;
- the attach client's rendering rules (§4.6 of supervise.md, unchanged);
- the replay ring (2,000 frames or 256 KiB);
- the parsing of `/panic`.

**What stays in the kernel:**
- the durable inbox `state/kernel/inbox.jsonl`;
- relaying to bodies, with ack and redelivery;
- the kernel's own owner lines, as kernel/supervise.md §4.6 lists them after D4: `/stop` (body), `/resume` (the kernel's own `Stopped`), `/status`, `/accept`, `/bless`, `/reject`, `/later`, `/kernel`. `/approve`, `/deny` and `/clear-safe` no longer exist (D4). `/accept`, `/bless` and `/reject` are optional signals that never gate anything, and the kernel records them as `owner` lineage records that the Root checks against the `qid` it relayed (§3.10.5). The Root keeps the first word and the `qid` of the last 1,000 relayed lines for that check;
- the body's slash commands.

**Relay.**
- **Owner → kernel.** Each owner line is first checked for `/panic`, then for the Root lines of §4.2. Any other line is appended to `state/owner-queue.jsonl` (fsync), echoed to clients, and sent as `owner.in{qid, frame}`. As built, the Root echoes a line only when it queues it with no kernel running; a running kernel echoes the line itself (kernel/supervise.md §3.10), so there is no double echo. The kernel answers `ok` once the line is durably in its inbox. The Root then drops the entry; the queue is compacted when it passes 1 MiB.
- **While no kernel runs** (backoff, swap, panic, stop), lines wait in the queue, up to `owner_queue_max` (200). The client sees `[root] the system is restarting; your message is queued (n)`.
- **Redelivery.** After each `ready`, unacknowledged entries are resent in order. The kernel deduplicates by `qid` (kernel/supervise.md, adopted).
- **Kernel → owner.** The kernel sends `owner.out{frame}` events: `display`, `echo`, `notice`, `state`. The Root forwards them to every attached client and keeps them in the replay ring. Root notices (alerts) are added as `notice{level, text}`.

### 3.16 Test support: fake clock, test ledgers, fakes

These exist only in a binary built with the cargo feature `test-fixtures`. Release builds compile them out. Even then, the Root honours them only when the active charter has `root.test_mode = true`.

- **Fake clock.** `INIT_ROOT_TEST_NOW=<RFC 3339>` in the Root's environment at start sets the Root clock. The owner-socket command `test.clock{advance_ms}` or `{set}` (CLI: `init-root test clock --advance <s>`) moves it forward, never back. Everything the clock drives follows it:
  - lineage `at`;
  - the ledger's day-based rules;
  - day boundaries;
  - `safe_auto_clear_s`, `kernel_probation_s` and `halt_retry_s`;
  - mirror intervals.

  root/egress.md uses the same clock for its day windows. Short timers (hello, ready, heartbeat, backoff, grace) stay on the monotonic clock and are shortened through `[root]` keys instead.
- **Test ledger.** `init-root test ledger --root-home <dir> --spec <file.json>` writes a signed R4 chain into a *test* `ROOT_HOME` before the Root starts. It refuses unless that home's charter has `test_mode = true` and its lineage is empty. It uses a throwaway key and writes `genesis{…, test:true}`. The spec gives `start_level` and a list of records on the fake clock: promotions, audits with verdicts, mutation runs, ceiling hits, crash loops. The start level is reached by `ladder` records with `trigger.kind = "test_fixture"`. `ledger --verify` accepts those only in a `test-fixtures` build, and a production Root halts (`Halted{lineage}`) on any chain whose genesis says `test:true`. RT-L1…RT-L9 and RR-23 run on such ledgers.
- **Fakes.** The kernel-facing contract of the Root is §4.1 together with §3.5. `init-fakeroot` (testing.md §3.5b) implements it for kernel-only tests. It is a stand-in and never a security test. `init-fakekernel` implements the kernel side of §4.1 for Root tests (testing.md §3.13).
- **Live-system safety.** Sweep, kill and panic tests run only in a disposable environment (§3.5.4, CONVENTIONS §1).

### 3.17 Workspace, access modes and `/undo` (v0.1.1, ADR-009)

The owner chose Codex-style permissions on 2026-10-20 (ADR-009, CONVENTIONS §6 OW-PERM). The Root owns every part that decides what the zone may write: workspace validation, the access state, the typed confirmations, snapshots and `/undo`. The TUI only reports its cwd and forwards lines. The kernel only restarts the live body when the Root asks. The body learns its access from two environment variables. The wire protocol is lane SBX-009's (build/status/SBX-009.md, "Protocol for WIRE-009"); §4.1 and §4.2 list its frames.

#### 3.17.1 Workspace validation

`init` sends `workspace.set{cwd}` once per launch, right after its first attach and before any owner line (§4.2). Each call starts a new **owner session**: full mode turns off and its confirmation is forgotten. The TUI sends the cwd as the OS gives it; the Root canonicalizes it (symlinks resolved) and checks it. A refused cwd is not an error: the workspace becomes none, the answer's `refused` names the reason, and the Root's notice says `[root] no workspace: cd into a project and run init to let me edit it`.

| Reason (`refused.code`) | The cwd … |
|---|---|
| `relative` | is not absolute |
| `too_long` | is longer than the Root's parameter limit (V2) |
| `missing` | does not exist |
| `not_dir` | is not a directory |
| `outside_home` | does not lie inside the owner's home as the Root knows it (`HOME_REAL`: the Root process's `$HOME`, which launchd sets to the owner's home) |
| `home` | is the home itself |
| `root_path` | lies inside, or contains, `ROOT_HOME`, `INIT_HOME` or `~/Library/LaunchAgents` |
| `deny_list` | is on the read deny list (ADR-009 Decision 1, plus the charter's `sandbox.read_deny`), or lies inside a Z3a write-deny entry (for example `~/bin/x` or `~/.config/fish`) |

Every `workspace.set` that sets a workspace takes the session snapshot (§3.17.4) before it answers, even when the path is unchanged, because each `init` launch is a new session. So the first agent turn that may write the workspace always comes after it.

#### 3.17.2 The access state

The Root keeps one access state and publishes it as `AccessView`, in the answers of §4.2, in `status.access`, and in `access.get`:

```text
AccessView = {"access": "none"|"read_home"|"workspace"|"full", "allow_full": bool, "full": bool, "read_home": bool,
              "restart_pending": bool, "snapshot": Snap|null, "workspace": "<canonical path>"|null}
Snap       = {"at": "<RFC 3339>", "id": "<snapshot dir name>", "reason": "session"|"full_on"|"pre_undo", "workspace": "<path>"}
```

`access` is what a new live body gets: `full` when full mode is on, else `workspace` when a workspace is set, else `read_home` when the charter's `sandbox.read_home` is true (the default), else `none` (the v0.1.0 behaviour). `restart_pending` is true from a change until the next live body spawns with the new grant. `snapshot` is the latest snapshot of the current workspace.

**Full mode.** `full.set{on:true}` (or the line `/full on`) turns it on. The first time in an owner session, the Root answers `root.confirm` instead and acts only when the request comes back with the typed phrase (§4.2). With the charter's `sandbox.allow_full = false` it answers `root.mode` with `data.reason = "charter"`. Turning full mode on snapshots the workspace (`reason: "full_on"`); with no workspace, the notice warns that full mode has no snapshot. `full.set{on:false}` (`/full off`) never needs a confirmation. While full mode is on, the TUI's status bar shows `FULL`.

**Who gets what.** The Root decides from the spawn's kind and the lane of its `INIT_HOME` parameter. The kernel sends the same `spawn` parameters as in v0.1.0 and no new one.

| Instance | Read (beyond v0.1.0) | Write (beyond v0.1.0) | `INIT_ACCESS` · `INIT_WORKSPACE` |
|---|---|---|---|
| live body (kind `body`, the live lane, safe mode included) | the home minus the deny list, when `read_home` is true or full mode is on | the workspace; in full mode, the whole home minus the deny list and the Root's paths | `AccessView.access` · the workspace, or `""` |
| fleet member (kind `body`, `fleet/<bid>`) | the home minus the deny list, when `read_home` is true | none | `read_home` or `none` · `""` |
| probe (kind `probe`) | none: hermetic, as in v0.1.0 | none | `none` · `""` |
| kernel, `build`, `check`, `git-read` | unchanged; tail Z2's deny list applies to them too | unchanged | — |

For every `body`-profile spawn the Root **sets `INIT_ACCESS` and `INIT_WORKSPACE`** in the child's environment, overwriting any value the kernel sent, so they always match that instance's sandbox. `cwd` still lies under `INIT_HOME` (V4 unchanged); the body's tools `chdir` to `$INIT_WORKSPACE` when it is not empty (body/boot.md §3.2). The grants themselves are §3.3.1's ROOT_ALLOWS lines and §3.3.2's Z2 and Z3.

#### 3.17.3 Applying a change

A body's profile is fixed at spawn. The kernel learns the access state from `hello` (which gains `access`) and from the Root → kernel event `access{access, full, restart, workspace}` (§4.1.4). The Root sends it after every `ready` with `restart: null`, and on every change, with `restart: "idle"` when the live body's grant changed: a new workspace, full mode on or off, or a charter edit to `sandbox.*`. On `"idle"`, the kernel restarts the live body gracefully at its next idle boundary. The body resumes its session log, so the conversation continues, and its new boot event shows the new access line (awareness.md §3.5). Every change also sends attached clients a `notice` with `code: "access"`: `[root] workspace: <path>`, the no-workspace text of §3.17.1, `[root] full mode on: …` or `[root] full mode off …`.

#### 3.17.4 Snapshots and `/undo`

- **When.** The Root takes a snapshot of the workspace at every `workspace.set` that sets one (`reason: "session"`, before it answers), when full mode turns on (`"full_on"`), and right before an undo restores (`"pre_undo"`).
- **How.** An APFS clone of the whole tree (`clonefile(2)`, as `cp -c -R` does), into `ROOT_HOME/snapshots/<id>/` plus `<id>.json`, where `id` is `<UTC ts>-<name>`. Clones copy symbolic links as links and never follow them. Clones share blocks with the workspace, so a snapshot costs almost nothing until one side changes. The zone can never read or alter a snapshot (tail Z1).
- **Skips.** A workspace over 2 GiB, one not on APFS, or one on another volume than `ROOT_HOME` gets no snapshot; a `notice` with `code: "snapshot"` says so. Taken snapshots and `/undo` restores are announced the same way.
- **Retention.** The last 5 snapshots per workspace are kept, and older ones are removed by their exact path. Snapshots never leave the machine: they are not mirrored and not put in lineage.
- **`/undo`.** The request `undo` (or the line `/undo`) is refused with `root.busy` (`data.reason = "body_busy"`) while the live body is busy, as its last heartbeat says. That check comes before the confirmation. Otherwise it always needs a typed confirmation: the first answer is `root.confirm`, whose `data.snapshot` names what will be restored. With the phrase, the Root clones the current workspace into a `pre_undo` snapshot. It then removes the workspace's entries (never following a link) and clones the snapshot's entries back, keeping the workspace directory itself, so an owner shell inside it keeps a valid cwd. It answers `{restored, saved}`. Because the replaced state is saved first, a second `/undo` undoes the first. With no workspace it answers `root.mode` (`data.reason = "no_workspace"`); with no snapshot, `root.not_found`.
- **R-F1 and R-F2 exception.** The workspace is zone-writable, and the snapshot walk and `/undo` are the only places where the Root reads or writes such a path. The Root re-validates the workspace (§3.17.1) right before each snapshot and restore, never follows a link inside it, and touches nothing outside it and `ROOT_HOME/snapshots/`. The busy check reads the last heartbeat (every 5 s), so a turn that starts during a restore can still write (§9.3 Q4).
- **Lineage.** Every access change is a Root record `access{access, event, full, workspace}`, where `workspace` is a boolean: the lineage is mirrored off the machine, so it never holds a path (§3.10.5).

## 4. Interfaces

### 4.1 The control link (Root ↔ kernel)

#### 4.1.1 Transport and framing

- **fd 3** is a `SOCK_STREAM` socketpair (`UnixStream::pair()`) inherited by the kernel. `INIT_ROOT_FD=3`.
  - There is no socket file, so nothing else in the zone can connect, and no peer authentication is needed.
  - A kernel that sees EOF on fd 3 must stop its bodies and exit within 10 s.
- **fd 4** is the fd channel, a `SOCK_DGRAM` socketpair (§3.5.3). `INIT_ROOT_FDPASS=4`.
- **Framing** follows kernel/initctl.md §3.1–§3.2:
  - one ICJ object per line, `"v":1`, ≤ 1 MiB;
  - unknown fields ignored, unknown `cmd` → `root.unknown_cmd`.
- **Messages.** Both sides send requests, responses and events:
  - request: `{"v":1,"id","cmd","args"}`;
  - response: `{"v":1,"id","ok":true,"result"}` or `{"v":1,"id","ok":false,"error":{"code","message","details":[],"data"}}`;
  - event: `{"v":1,"ev","data"}`, with no id and no response.
- **Ids.** `k-<n>` from the kernel, `r-<n>` from the Root. Requests may be pipelined, and responses may arrive out of order (correlated by `id`).
- **Ordering of fds.** Whenever fds accompany a message, the datagram tagged with that message's `id` is sent **before** the line. The receiver keeps unmatched datagrams for 60 s (at most 32), then closes them.

#### 4.1.2 Error codes (dotted, CONVENTIONS §4)

| Code | Meaning | Retry |
|---|---|---|
| `root.proto` · `root.version` · `root.unknown_cmd` | malformed frame, unsupported `v`, unknown command | no |
| `root.args` | invalid arguments (V1–V6, schema) | no |
| `root.level` | the level does not grant this (`data.need`, `data.level`); also counted as a ceiling hit | after promotion |
| `root.mode` | refused in the current mode (`data.mode` ∈ `safe` `panicked` `stopping` `swapping`) | later |
| `root.quota` | `data.quota` ∈ `instances` `parallel_bodies` (L2+ above N) `cpu_day` `disk` `init_home`; `data.limit`, `data.used`, `data.resets_at` | after reset or cleanup |
| `root.busy` | a swap or rollback is in progress; v0.1.1 owner socket: `undo` while the live body is busy (`data.reason = "body_busy"`) | yes |
| `root.rate` | lineage, alert or proposal rate limit | after 1 h (proposals: once the rolling 24 h window allows) |
| `root.store` | hash mismatch, too large, store full | no |
| `root.not_found` | unknown instance, blob or tuple | no |
| `root.lineage` | kind not allowed for the kernel, missing fields, non-ICJ, size, privacy (§3.10.6) | no |
| `root.internal` | Root bug; details in `logs/root.jsonl` | maybe |
| `root.confirm` | v0.1.1, owner socket only: the request needs a typed confirmation (§4.2); `data.phrase` is what the owner must type, and for `undo` `data.snapshot` is what will be restored | resend with `args.confirm` |
| `root.sealed_none` · `root.sealed_quota` · `root.budget` | M7, `sealed.g4_slice` only: no S v1 drawn (G4 uses the kernel fallback); looks exhausted and the rotation failed; `budget_musd` below the slice's estimate. `root.busy` also covers a slice already running | no · after the next rotation · with more budget |

#### 4.1.3 Kernel → Root requests

| `cmd` | `args` | `result` | Notes |
|---|---|---|---|
| `hello` | `kernel_version`, `proto` (1) | the boot block below | first frame; within 30 s of spawn |
| `ready` | `{}` | `{}` | the kernel's supervisor and servers are up |
| `spawn` | `kind`, `profile`, `params`, `program`, `args`, `env`, `cwd`, `lane`?, `replaces`? | `instance`, `pid`, `started_at`; fds `[stdin_w, stdout_r, stderr_r]` | §3.5; quotas §3.14 |
| `kill` | `instance`, `mode` (`term`: SIGTERM to the process group; `kill`: full sweep) | `processes` (as built 0 or 1: the leader is counted, members of its group are not) | the instance then ends normally (`exited` event) |
| `instances` | `{}` | `[{instance, kind, lane, pid, started_at}]` | for kernel cold start |
| `store.put` | `kind` (`init-kernel`\|`initctl`\|`init-body`\|`ext`\|`kstate`\|`t0` (M7)), `name`?, `sha256`, `bytes`, `job`? (for `kstate`); 1 fd | `sha256` | the Root hashes while reading; mismatch → `root.store`. A `t0` blob is a pax tar with relative paths, ≤ 8 MiB, holding only `GENESIS.md`, `SELF.md` and `skills/**`; any other path → `root.store` (§3.7.1) |
| `store.get` | `sha256` | `bytes`; 1 read-only fd | used after a whole-system rollback |
| `tuple.get` | `{}` | `current`, `lkg`, `audited` (ids and objects) | |
| `lineage.append` | `kind`, `data`, `idem` (`"<boot_id>:<outbox_seq>"`) | `seq`, `hash`, `dup` (true when `idem` was seen before; the original `seq` and `hash` are returned) | kinds and fields: §3.10.5; dedup: §3.10.3 (D7) |
| `ledger.get` | `{}` | the content of `ledger.json` | |
| `ceilings.get` | `{}` | the object of `ceilings.json` (§4.5): egress's `GET /_root/ceilings` blocks plus the broker's `instances` and `parallel_bodies` | integers only |
| `system.swap` | `version`, `source_repo` (a bare repo under `INIT_HOME`), `source_sha`, `init_kernel` (sha256), `initctl` (sha256), `report` (the K-stage summary, kernel/gate.md) | `job` | §3.8, §3.8.1; progress via `swap` events |
| `proposal.file` | `title` (1–200 B, not blank, no control characters), `text` (≤ 64 KiB) | `id`: `p-000001`, `p-000002`, … | Stored as `ROOT_HOME/state/proposals/<id>.md` (0600): `# <title>`, a blank line, then the text. Recorded as the Root record `proposal{id, title, sha256, bytes}`, where the digest and size are of `text`; the text never enters the lineage. Alert `proposal`. Never applied. Accepted at every level and in every mode. Refusals: `root.args` (a missing or non-string field; a title that is empty, blank, over 200 bytes or holds control characters; text over 64 KiB); `root.lineage` (a title that fails the §3.10.6 privacy rule, since it is mirrored); `root.rate` (a sixth proposal within a rolling 24 h on the Root clock; the counter and the window are seeded from the lineage, so a restart resets neither). A refusal stores and records nothing. |
| `sealed.family_eval` | the request of root/sealed-eval.md §4.2 (`FamilyEvalReq`) | the `family_eval` record and its `seq` | L3+ only (`root.level` otherwise, rule b); runs on sealed instances |
| `sealed.g4_slice` (M7) | `gate_id`, `candidate` `{body, ext:{set, components[]}, t0}` (sha256s already stored), `incumbent` (tuple id; must be `lkg`), `tier` `{n, repeats}`, `budget_musd`, `wall_s` | `{seq, status, ratchet_signal, unconfirmed, ratchet_k, n, repeats, d_permille (±50), m_permille (±10), cpp_ratio_permille (±50), looks_left, s_version, dup, spent_musd}` | G4 v2's sealed slice (root/sealed-eval.md §3.5.1; kernel/gate.md §3.7.1a). Every level; normal mode only (`root.mode` in safe mode). Refusals: `root.sealed_none` (no S v1), `root.sealed_quota` (looks exhausted and the rotation failed), `root.busy` (a slice is running), `root.budget` (budget below the estimate), `root.store` (missing blob), `root.args`. `error` statuses are not looks |
| `sealed.holdout_rotate` | `{}` | `count`; one read-only fd per task bundle | Phase 1, L0–L2 only: hands retired public S tasks to the kernel, which imports them into its holdout itself (the Root never writes zone paths, R-F1; root/sealed-eval.md §3.6) |
| `alert` | `code`, `level` (`info`\|`warn`\|`alert`), `message` | `{}` | ≤ 30 per hour; delivered as `source:"kernel"` |

**The `hello` result** (the boot block):

```json
{"boot":{"cause":null,"kstate":null,"level":0,"mode":"normal","panic":null,"reason":"cold",
         "tuple":{"audited":"t3f09c1e5b7d2","current":"t9a07c2e91d04","lkg":"t9a07c2e91d04"},
         "tuple_body":{"binary":"5c1f…","ext":{"components":[],"set":"b81c0e5a9d27"},"sha":"4b1e…"},
         "tuple_kernel":{"built_from":"a1c4…","init_kernel":"e3b0…","source":"maintainer","version":"0.1.0"}},
 "charter_sha256":"7d1e…",
 "ladder":{"cap":0,"evolution_share_max_permille":null,"level":0},
 "limits":{"hb_interval_s":5,"max_instances":12,"parallel_bodies":1},
 "paths":{"egress_sock":"/Users/<owner>/.init-root/run/egress.sock","kernel_git":"/Users/<owner>/.init/lineage/kernel.git"},
 "root":{"proto":1,"version":"0.1.0"}}
```

From v0.1.1 the boot block also carries `"access": {"access", "full", "workspace"}` (§3.17.3; `workspace` is a path or null).

`reason` ∈ `cold`, `crash`, `rollback_system`, `demotion`, `panic_resume`, `safe_clear`, `kernel_upgrade`, `install`. `mode` ∈ `normal`, `safe`. `cause` is set for `demotion` and `rollback_system`: `{trigger, seq}`, which awareness.md and kernel/supervise.md copy into `boot.json.cause`. `kstate` is the sha256 of a state snapshot to restore first (§3.8, phase 6), else null. `tuple_kernel.init_kernel` is the admitted build digest kernel/selfmap.md reports as `live.kernel.verified`. The boot block carries no `ROOT_HOME`. The kernel derives the Root canary `ROOT_HOME/run/canary` (§3.2.1) as the file `canary` in the directory of `paths.egress_sock`, and uses it in its socket peer checks.

#### 4.1.4 Root → kernel requests and events

| Kind | Name | Fields | Kernel must |
|---|---|---|---|
| request | `stop` | `reason`, `grace_ms` | stop its bodies (kernel/supervise.md `stop` frame), flush, answer `ok`, exit 0 |
| request | `prepare_swap` | `job`, `deadline_s` | pause the live body at a step boundary, stop it, flush, answer `ok`, exit 75 within 10 s |
| request | `owner.in` | `qid`, `frame` (`line`\|`interrupt`, as supervise.md §4.6) | append durably to its inbox (dedup by `qid`), then answer `ok` |
| event | `exited` | `instance`, `code`, `signal`, `rusage_ms`, `at` | treat it as the instance's exit (its process supervision) |
| event | `level` | `from`, `to`, `cap`, `cause` (`unlock`\|`demotion`\|`charter_max`), `trigger`, `r4_seq`, `evolution_share_max_permille` | enable or disable level features (§3.13.4); relay the notice to the body (evolution.md §4.5) |
| event | `mode` | `mode` | — (it arrives only together with a restart) |
| event | `ceilings` | the `ceilings.get` result | adapt; e.g. retire bodies above a lowered `parallel_bodies` |
| event | `swap` | `job`, `stage`, `msg` | report progress to its own initctl job |
| event | `access` (v0.1.1) | `access` (`none` \| `read_home` \| `workspace` \| `full`), `full`, `restart` (`"idle"` \| null), `workspace` (path or null) | sent after every `ready` (`restart: null`) and on every change; on `"idle"`, restart the live body gracefully at its next idle boundary (§3.17.3). Ignore unknown fields. |

**Kernel → Root events:**
- `hb` with `{slot, mode, busy, sha12}`, every 5 s;
- `owner.out` with `{frame}`.

**Example** (a body spawn, abbreviated; `→` is kernel to Root):

```
→ {"args":{"cwd":"/Users/<owner>/.init/work","env":{…},"kind":"body","lane":"b0","params":{"BODY":"/Users/<owner>/.init/body",…},"profile":"body","program":"/Users/<owner>/.init/bin/init-body.staged","replaces":"7c2e91d04b3a"},"cmd":"spawn","id":"k-42","v":1}
← (fd channel) {"id":"k-42","n":3} + SCM_RIGHTS[stdin_w, stdout_r, stderr_r]
← {"id":"k-42","ok":true,"result":{"instance":"a3f09c1e5b7d","pid":48311,"started_at":"2026-10-20T08:01:02.345Z"},"v":1}
…
← {"data":{"at":"2026-10-20T08:03:10.002Z","code":75,"instance":"7c2e91d04b3a","rusage_ms":41230,"signal":null},"ev":"exited","v":1}
```

### 4.2 The owner socket (`ROOT_HOME/run/owner.sock`)

- **Socket.** Mode 0600 in the 0700 `run/`. Each accepted connection passes the peer check of kernel/supervise.md §4.6: same uid, and the peer is not sandboxed (macOS `sandbox_check(pid, NULL, 0) == 0`; Linux: the peer's pid-namespace inode equals the Root's). Tail Z6 also forbids the zone from connecting. A rejected peer is closed without a reply (logged); the CLI then reports a protocol error.
- **Channel frames** are those of kernel/supervise.md §4.6, unchanged:
  - client → Root: `attach{client, replay}`, `line{text}`, `interrupt`, `panic`, `detach`;
  - Root → client: `display`, `echo`, `notice`, `state`, `replay_end`.
- **Request frames** (`{"v":1,"id","cmd","args"}`) carry the CLI commands: `status`, `panic`, `resume`, `stop`, `start`, `charter.apply`, `ledger`, `audit.run`, `audit.verify`, `keys.reload`, `upgrade`, and in test builds `test.clock` (§3.16).
- **v0.1.1 request frames (ADR-009, §3.17)**, lane SBX-009's protocol:

  | `cmd` | `args` | `result` | Errors |
  |---|---|---|---|
  | `workspace.set` | `{"cwd": "<the TUI's cwd, absolute>"}` | `{"access": AccessView, "changed": bool, "refused": null \| {"code": Reason, "message"}}` | `root.args` (`cwd` missing or not a string). A refused cwd is not an error (§3.17.1). |
  | `full.set` | `{"on": bool, "confirm"?: "<what the owner typed>"}` | `{"access": AccessView, "changed": bool, "snapshot": Snap \| null}` | `root.mode` with `data.reason = "charter"` (`sandbox.allow_full = false`); `root.confirm` |
  | `undo` | `{"confirm"?: "<what the owner typed>"}` | `{"restored": Snap, "saved": Snap}` | `root.busy` with `data.reason = "body_busy"` (checked before the confirmation); `root.mode` with `data.reason = "no_workspace"`; `root.not_found` (no snapshot); `root.confirm`; `root.internal` |
  | `access.get` | `{}` | `AccessView` | — |
  | `status` (existing) | `{}` | gains `"access": AccessView` | — |

  `workspace.set` is sent once per `init` launch, after the first attach and before any owner line. **Typed confirmation:** `full.set{on:true}` the first time in an owner session, and every `undo`, answer `root.confirm` with `message` (what will happen) and `data.phrase` (today `"confirm"`) unless `args.confirm` equals `data.phrase` exactly. The client shows `message`, reads one line from the owner, and resends the same request with `"confirm": <that line>`. Clients take the phrase from `data.phrase` and never hard-code it.
- **v0.1.1 notices** (Root → every attached client, `src:"root"`): `code:"access"` on every workspace or full-mode change, and `code:"snapshot"` when a snapshot is taken, skipped or restored (§3.17.3, §3.17.4).

**Lines the Root handles** (exact match after trimming; everything else goes to the kernel):

| Line | Effect |
|---|---|
| `/panic` | §3.9; checked before anything else |
| `/resume` | when the Root is `Panicked`, `Halted` or `Stopped`: resume (§3.9.5, §3.6.6). Otherwise the line goes to the kernel, which leaves its own `Stopped`. System safe mode has no owner command (D4). |
| `/root status` · `/root ledger` · `/root stop` · `/root help` | the CLI equivalents |
| `/workspace` · `/full` (v0.1.1) | a notice with the `AccessView` as text |
| `/full on` · `/full on confirm` · `/full off` (v0.1.1) | `full.set`. `/full on` answers with a notice that says what will happen and asks for `/full on confirm` (the first time in an owner session; later it acts at once) |
| `/undo` · `/undo confirm` (v0.1.1) | `undo`. `/undo` answers with a notice naming the snapshot that would be restored and asks for `/undo confirm` |

### 4.3 The `init-root` CLI

| Command | Does | Exit |
|---|---|---|
| `install --release <dir> [--home <INIT_HOME>] [--root-home <ROOT_HOME>] --seed-source <repo\|bundle> [--seed-tag <tag>] [--charter <file>] [--with-testkit] [--no-service] [--bin-dir <DIR>]` | first install; see below. `--bin-dir` (v0.1.1) records where the `init` launcher and `init-tui` are installed (`install.json` `bin_dir`), so tail Z3a write-denies it in every mode; `scripts/install.sh` passes it when `INIT_BIN_DIR` is not `~/.local/bin`, which Z3a already covers. `--no-service` skips the service file (step 9), so a harness that runs `install` with the owner's real HOME (`HOME_REAL` must equal the kernel's passwd home) writes nothing under `~/Library`. `--with-testkit` adds `init-fakebody`; `init-mockllm` is installed either way. | 0; 1 error (including a release without `init-mockllm`); 2 invalid charter |
| `install --upgrade --release <dir>` | new `init-root`: replaces `ROOT_HOME/bin/init-root` (new inode) and asks launchd/systemd to restart it, appending `root_start` on the next start. New kernel: imports `src/<version>.bundle` into `lineage/kernel.git` (§3.8.1), refreshes `kernel/docs/` and `kernel/kupgrade-smoke/`, then `system.swap{source:"maintainer"}` through `owner.sock` (§3.8). New seed: a new bundle in `kernel/seed/`. | 0/1 |
| `start [--foreground]` | the daemon (what launchd/systemd run). It always runs in the calling process; `--foreground` only echoes the boot status to stderr. If one is already running: start the zone if it is `Stopped`, and exit 0 | 0/1 |
| `attach [--no-replay] [--raw]` | the owner terminal (§3.15); rendering as kernel/supervise.md §4.6 | 0 detached; 69 Root unreachable; 76 protocol |
| `panic` | §3.9; falls back to writing the latch (§3.9.6, as built) | 0 once confirmed or latched |
| `resume` | §3.9.5, §3.6.6 (optional; never required except after the owner's own panic) | 0; 77 refused (state or watch path) |
| `stop` | §3.9.7 | 0 |
| `keys set <provider_id>` · `keys set --action <class_id>` | read the secret from the TTY with echo off (never from argv or env), write it as root/egress.md §3.2 specifies, send `keys.reload` | 0; 2 not a TTY |
| `keys check` | present / insecure / missing per provider; never prints a key | 0 |
| `status [--json] [--selftest] [--proposals]` | slot, mode, tuples, level and cap, ceilings, panic latch, mirror state, last audit, kernel pid and uptime. `--selftest` runs the RR-1 and RR-2 probes in short-lived instances that exit on their own; it never sweeps (§3.5.4). `--proposals` lists agent proposals, one line each, sorted: `proposal <id> · <title> · <path>`. | 0; 1 self-test failed |
| `charter check [<path>] [--apply [--yes]]` | §3.12.4 | 0 valid; 2 invalid |
| `ledger [--json] [--verify]` | level, cap, progress against the next level's criteria, history | 0; 1 verify failed |
| `audit run [--usd <cap>]` | runs the sealed audit now (root/sealed-eval.md; manual in Phase 0, spec §19) | 0 pass; 1 regression; 2 error |
| `sealed draw`, `sealed calibrate [--repeats 6] [--offpeak] [--probe 2]`, `sealed rotate`, `sealed show [--g4 <gate_id>\|--partition\|--templates]`, `audit run --reason milestone --set reserve` (M7) | S v1's draw, calibration and cost probe, rotation, owner-only detail, and the milestone check (root/sealed-eval.md §3.1.1, §3.5.1, §4.4). Output never shows task content | as root/sealed-eval.md |
| `audit verify [--full]` | lineage verification (§3.10.4) | 0 valid; 1 invalid |
| `exec-clean [--keep N]… -- argv…` · `sandboxed-upload-pack <repo>` | internal (§3.5.2, §3.8.1) | — |
| `test ledger --root-home <dir> --spec <file>` · `test clock --advance <s>` | test builds only (feature `test-fixtures`) and `root.test_mode` only (§3.16) | 0; 2 refused |

**The release directory** (from the maintainer) holds: `init-root`, `init-kernel`, `initctl`, `init-mockllm`, `src/<version>.bundle` (the kernel source, one ref `refs/heads/release`), `docs/` (the kernel docs the agent reads, including the selfmap recipes), `kupgrade-smoke/` (kernel-upgrade fixtures), `wit/`, and with `--with-testkit` `init-fakebody`. The Root installs all of it, because `kernel/` is Root-written (kernel/supervise.md §9 item 8).

**`init-mockllm` is in every install** (provisional owner decision, 2026-10-20). It is the $0 local mock model and makes no network call. G0 runs the body's tests with `INIT_MOCKLLM_BIN=$INIT_HOME/kernel/bin/init-mockllm` (kernel/gate.md §3.3), so without it every T2 promotion fails G0 `test`. `install` copies it into `kernel/bin/` with `init-kernel` and `initctl`, whatever `--with-testkit` says, and refuses a release directory that has `init-kernel` but no `init-mockllm` (only a warning under `root.test_mode`). Like the rest of `kernel/` it is read-only to the zone (Z4). It is not part of a tuple: tuples name `init-kernel` and `initctl` only (§3.7), so neither the genesis tuple's form nor the kernel's `built_from` changes, and a rollback leaves the installed mock in place.

**`install` (first time), in order:**
1. Check the platform tools: `sandbox-exec`; or `bwrap` ≥ 0.8.0 with user namespaces and seccomp; `git` ≥ 2.39.4. Check the paths (§3.2.1).
2. Warn if free space is below `quotas.disk_min_free_gib`. This is a warning, not a refusal; the broker and watcher enforce the floor later.
3. Create `ROOT_HOME` (§3.2.1) and `run/canary`. Generate the lineage key, the mirror deploy key and an empty `known_hosts` in `keys/`, and exclude `keys/` from Time Machine.
4. Write `charter.toml` from `--charter`, or the template (§3.12.2). If the charter is invalid, stop with exit 2 and the message `edit ~/.init-root/charter.toml, then run install again`.
5. Create the `INIT_HOME` skeleton (kernel/supervise.md §3.1, with §3.2.2's changes), including `run/mark/decoy` and `run/zone-mark`. Install `init-kernel`, `initctl`, `docs/`, `kupgrade-smoke/` and `wit/` from the release into `kernel/`. Write `CONSTITUTION.md` from the charter and the `config.toml` template. When the release ships no `config.toml`, the template is only `schema_version = 2` plus `[sandbox] toolchain` and `developer_dir` taken from `install.json`, because the broker's class rules compare them exactly; the kernel's defaults fill the rest.
6. Create `lineage/kernel.git` (bare), copy `src/<version>.bundle` to `kernel/src/`, and import it as `refs/upstream/<version>`; `refs/heads/main` and `refs/heads/lkg` point at it (§3.8.1; `lkg` as built since build-deviations §5 L4, 2026-10-20).
7. Bundle the seed: `$RG` (§3.11) with `--git-dir=<seed-source>/.git` (or the source itself when it is bare) in place of the mirror's, running `bundle create INIT_HOME/kernel/seed/<tag>.bundle refs/heads/seed --tags=seed-v*`. The branch is named in full: `--branches=seed` would mean `refs/heads/seed/*`, because git appends `/*` to a pattern with no glob character, so the branch would be left out. The source must be a local path. It is the owner's own repository, run with the hardened prefix. A source that is a file must already be a bundle, and is copied. Either way, `git bundle list-heads` must show `refs/heads/seed`, and the work goes through a temp file and a rename. The tag is `--seed-tag`, or the highest-semver `seed-v<x.y.z>` (numeric, so 0.10 > 0.9).
8. Append `genesis` and `charter`. Before the kernel spawn, write `ROOT_HOME/bin/init-root` (a new inode) and `install.json`, because exec-clean and the broker's V1–V5 and class rules need them. Spawn the kernel in install mode (`init-kernel install --from-bundle $INIT_HOME/kernel/seed/<tag>.bundle --seed-tag <tag>`, reason `install`) on a cold-started Root under `run/root.lock`, with no daemon and no egress. The kernel imports the seed, clones `body/`, builds the seed through `spawn{kind:"build"}`, `store.put`s the body binary and its components, and reports `promotion{lkg:true}`. The Root forms the genesis tuple and appends `tuple`, `lkg` and `audited`; it writes no `zone_start` or `zone_exit` for this kernel.
   - **Bounds:** `kernel_hello_timeout_s`; `hb_stale_s` after `hello`; 1,800 s in total (`INSTALL_TIMEOUT_S`; the kernel bounds a cold build at 900 s); and free space at or above `quotas.disk_min_free_gib`. Any bound kills the instances the Root holds (the kernel and its build).
   - The step succeeds only on exit 0 with a current tuple that verifies. Afterwards nothing is held and the slot is `Stopped`.
   - On failure the error names the exit code, the kernel log tail and `state/kernel/logs/install-build.log`. `install.json` is removed by its exact path, so `install` can be re-run; every earlier step is idempotent.
9. Write the service file, unless `--no-service`. It is not loaded, because loading it is an owner action.
   - macOS: `~/Library/LaunchAgents/dev.init.root.plist` with `KeepAlive=true`, `ThrottleInterval=10`, `ExitTimeOut=60`, `ProgramArguments=[ROOT_HOME/bin/init-root, start]`, and stdout/stderr to `ROOT_HOME/logs/launchd.log`.
   - Linux: `~/.config/systemd/user/init-root.service` with `Restart=always`, `RestartSec=2`, `KillMode=mixed`, `TimeoutStopSec=60`.

   If a v0.2 `dev.init.kernel` job exists, print the command that unloads it.
10. Print the lineage fingerprint, the path of `keys/mirror_ed25519.pub`, which is added at M0 as the deploy key of `nagisanzenin/INIT-Lineage` (§3.11), and the next steps.

### 4.4 Environment of the kernel process

The Root calls `env_clear()` and sets exactly these. Everything else reaches the kernel through `hello`.

| Variable | Value |
|---|---|
| `INIT_HOME` | absolute path |
| `INIT_ROOT_FD` · `INIT_ROOT_FDPASS` | `3` · `4` |
| `PATH` | `$INIT_HOME/kernel/bin:$TOOLCHAIN/bin:$DEVDIR/usr/bin:/usr/bin:/bin:/usr/sbin:/sbin` (macOS); `$INIT_HOME/kernel/bin:$TOOLCHAIN/bin:/usr/bin:/bin` (Linux) |
| `HOME` · `TMPDIR` | `$INIT_HOME/cache/kernel-home` · `$INIT_HOME/cache/tmp/` |
| `LANG` | `en_US.UTF-8` (macOS) · `C.UTF-8` (Linux) |
| `SDKROOT` · `DEVELOPER_DIR` | macOS only; from `install.json` |
| `INIT_TEST` · `INIT_TEST_*` · `INIT_FAULT` | only with `root.test_mode = true` (testing.md §3.4.6) |

Arguments: `init-kernel supervise --under-root`, or `init-kernel install --from-bundle <path> --seed-tag <tag>` at install.

### 4.5 Root files (formats)

**`state/root.json`** (shown wrapped; one ICJ line on disk):

```json
{"counters":{"crashes_total":2,"rollbacks":1,"swaps_failed":0,"swaps_ok":1},
 "crash":{"backoff_until":null,"consecutive":0,"window":[]},
 "instances":[{"id":"9d1c0e5a7b23","kind":"kernel","lane":null,"mark":"run/mark/9d1c0e5a7b23","pgid":47002,"pid":47002,"pidns_ino":null,"started_at":"2026-10-20T08:00:00.000Z"}],
 "kernel":{"boot_seq":12,"ready_at":"2026-10-20T08:00:04.120Z"},
 "mode":"normal","slot":"running","swap":null,
 "tuples":{"audited":"t3f09c1e5b7d2","current":"t9a07c2e91d04","lkg":"t9a07c2e91d04"},"v":1}
```

As built (WP21 D10): `counters` are folded from the records with the level (`zone_exit` class `crash`, `rollback`, `kernel_upgrade` stages), and `crash` is the supervisor's backoff and crash window (`window` as RFC 3339 times). A Root restart reads the previous `crash` block, keeps the window entries inside `crash_window_s` and waits out a pending `backoff_until`. The file also carries `cpu_ms`.

**`state/ledger.json`** and its view `INIT_HOME/root/ledger.json`. The field names follow awareness.md §4.5:

```json
{"cap":0,"entered_at":"2026-10-20T07:55:00.000Z","entered_seq":2,"evolution_share_max_permille":null,"incident":null,
 "last_change":null,"level":0,"max_level":4,
 "next":{"criteria":[{"have":7,"id":"min_promotions","met":false,"need":20},{"have":1,"id":"audits_in_window","met":true,"need":1},{"have":0,"id":"regressions_in_window","met":true,"need":0},{"have":0,"id":"demoting_hits","met":true,"need":0}],"level":1},
 "seq":311,"supported_max_level":0,"v":1}
```

`next` is null at the cap. `last_change` is `{from, to, direction, cause, trigger, seq, at}` from the newest `ladder` record.

**`INIT_HOME/root/status.json`**: `{"audited_good":{"body_sha","tuple"},"charter_sha256","kernel_build","level","mode","panicked","slot","tuples":{"audited","current","lkg"},"v":1}`. Here `kernel_build` is the sha256 of the running `init-kernel`.

**`INIT_HOME/root/ceilings.json`**, rewritten every 60 s: the object root/egress.md §4.3 serves as `GET /_root/ceilings` (`money`, `compute`, `disk`, `classes`, `level`), plus `instances{active, limit}` and `parallel_bodies{active, limit}` from the broker.

**`INIT_HOME/root/charter.json`**: §3.12.5.

**`head.json`**: `{"hash","seq","v":1,"verified_full_at"}`.

**`install.json`**: `{"devdir","git","init_home","platform","root_home","sdkroot","toolchain","v":1}`; v0.1.1 adds `bin_dir` when `install --bin-dir` was given (§4.3, tail Z3a).

### 4.6 Alerts

Every alert is delivered three ways: as a `notice{level:"alert"}` to every attached client, as a line in `ROOT_HOME/alerts.jsonl` (`{"v":1,"ts","source","code","message",…}`), and through `notify.cmd`. `source` ∈ `root`, `kernel`, `egress`, `sealed`.

Root codes:
- state: `panic`, `resume`, `safe_mode`, `halted`, `halt_retry`, `paused`;
- ladder: `demoted`, `level_up`;
- recovery: `rollback`, `kernel_crash_loop`, `kernel_upgrade`;
- mirror and lineage: `mirror_failing`, `mirror_diverged`, `lineage_corrupt`;
- resources: `disk_low`, `quota`;
- owner-facing: `proposal`, `charter_pending`, `config_error`.

Alert text never contains key material, paths outside `INIT_HOME`, or upstream error bodies.

### 4.7 Fault-injection points (`INIT_FAULT`, honoured only with `root.test_mode = true`)

| Point | Placed |
|---|---|
| `root.spawn.after_record` | instance recorded, before the fds are handed over |
| `root.lineage.mid_append` | half a line written; then `abort` |
| `root.lineage.after_append` | line synced, before the view copy and `head.json` |
| `root.rollback.after_stop` | zone swept, before the kernel install |
| `root.rollback.after_install` | new `kernel/bin/init-kernel` in place, before `state/root.json` |
| `root.swap.after_prepare` | old kernel exited 75 and its `kstate` is stored, before the install |
| `root.kgit.after_fetch` | candidate commit fetched into `lineage/kernel.git`, before `main` moves |
| `root.lineage.after_idem` | `idem` key indexed, before the record is written (write-ahead, §3.10.3): the resend appends once, because at open a key whose record never landed is dropped |
| `root.panic.after_latch` | latch written, before egress closes |
| `root.panic.after_sweep` | zone dead, before the records |
| `root.charter.after_active_write` | active charter written, before the `charter` record |
| `root.mirror.after_commit` | commit made, before the push |

Expected recovery after each, checked by RR-35 and testing.md F-kill-root (in a disposable environment): one Root, a verifying chain, no orphan zone process, pointers consistent with the lineage.

### 4.8 Size budget and dependencies

**Budget.**
- **Provisional owner decision 2026-10-20 (owner may override; build-deviations.md §3.1, RELEASE_PLAN.md):** `init-root` is at most **≈ 7,500 lines** of Rust, as the sum of the per-module sub-budgets below (7,540 at cdb49e8). Each sub-budget is the measured size plus 5%, rounded up to a multiple of 10, and the numbers are re-measured at release. The v0.3 design figure of 1,500 lines (core ≤ 995) predates sealed evaluation, D1–D14 and the money layer, and no WP cut a specified feature to meet it.
- **Counting method.** Non-blank lines of `root/crates/init-root/src/**/*.rs` after `//` and `/* */` comments (doc comments included) are removed, excluding every item under `#[cfg(test)]` (a `#[cfg(test)] mod x;` excludes its file, for example `lineage/tests.rs`), `tests/`, `testfix.rs` (feature `test-fixtures` only) and the `.sb` profile files. `root/sandbox-profiles/src` is counted the same way, on its own lines. The 2026-10-20 count is pinned to a commit (it reads `git show <sha>:<path>`), and its comment stripping is string-aware for ordinary string literals only, so it is approximate by a few lines.
- `ci/local.sh` check `rootloc` (WP17, not built yet) fails when any module exceeds its sub-budget or the total exceeds the budget.
- **Core** (for the owner's hand review): `zone.rs`, `sup.rs`, `panic.rs`, `egress/money.rs` and `lineage/verify.rs`, counted as a separate subtotal.
- `#![deny(unsafe_code)]` at the crate root. Only `sys.rs` (FFI: `sendmsg`/`recvmsg`, `sigaction`, `wait4`, `kill`/`killpg`, `proc_listpids`, `dlsym(sandbox_check)`, `statvfs`, `close_range`, `dup2`) carries `#[allow(unsafe_code)]`, with a `SAFETY` comment per block.

| Module | Content | v0.3 estimate | Measured (cdb49e8) | Budget |
|---|---|---|---|---|
| `main.rs`, `cli.rs` | dispatch, the attach client, the panic fallback, `exec-clean`, `sandboxed-upload-pack`, `audit run`, `sealed …`, `test ledger` | 85 | 415 | 440 |
| `charter.rs` | schema, C1–C14, digest, view | 75 | 319 | 340 |
| `install.rs` | layouts, keys, seed and kernel-source bundles, `lineage/kernel.git`, service files, the kernel `config.toml` template | 70 | 377 | 400 |
| `zone.rs` (core) | profile composition (zone and sealed tails, `extra_read`), V1–V6, spawn, fd hand-over, reapers, Linux argv and seccomp | 180 | 288 | 310 |
| `sweep.rs` | the D1 membership rule (fake tables; live only with `live-sweep`) | 50 | 34 | 40 |
| `sup.rs` (core) | state machine, quotas and watcher, pause, `Halted` retries, rollback, safe mode, the kernel control link | 165 | 822 | 870 |
| `tuple.rs` | tuples, the blob store, the `current`/`lkg`/`audited` pointers, rollback targets | (in `sup.rs`) | 223 | 240 |
| `panic.rs` (core) | signals, latch, watch paths, mirror ref, resume | 35 | 151 | 160 |
| `lineage/` except `verify.rs` (`mod.rs`, `kinds.rs`, `kgit.rs`, `mirror.rs`, `fault.rs`) | ICJ append, `idem` index, chain, sign, view, mirror, `kernel.git` refs | 130 (all of lineage) | 842 | 890 |
| `lineage/verify.rs` (core) | chain and signature verification | (in lineage) | 125 | 140 |
| `ledger.rs` | fold, criteria, demotion, share bound, verify | 95 | 162 | 180 |
| `link.rs` | control link, owner server, relay, replay ring, queue, `owner` signal check, `proposal.file` | 100 | 441 | 470 |
| `sys.rs` | FFI, the only `unsafe` module | — | 250 | 270 |
| `util.rs` | ICJ, hex, `/dev/urandom`, clock and RFC 3339, atomic writes, secrets, the Root log | — | 160 | 170 |
| egress (R2, R3) except `money.rs` (`egress.rs`, `classes.rs`, `http.rs`, `lane.rs`, `tap.rs`) | root/egress.md §6 | 330 (all of egress) | 1,411 | 1,490 |
| `egress/money.rs` (core) | metering, reservations, hard ceilings | (in egress) | 270 | 290 |
| sealed (R6, `sealed/`) | root/sealed-eval.md §4.2 | 175 | 795 | 840 |
| **total `init-root`** | | **≤ 1,490** | **7,085** | **7,540** |
| of which **core** (zone, sup, panic, egress money, lineage verify) | | ≤ 995 (a different set) | 1,656 | 1,770 |
| `testfix.rs` | fake clock and test ledgers; feature `test-fixtures` only, not counted, because release builds do not contain it | (≤ 80) | 103 | (≤ 110) |
| `root/sandbox-profiles` profile code (`lib.rs`, `params.rs`, `probe.rs`, `seatbelt.rs`, `sys.rs`), own line | V1–V5 parameter validation, Seatbelt composition, the in-sandbox probe | — | 821 | 870 |
| `root/sandbox-profiles` self-test runner (`selftest.rs`, `bin/init-sbx-selftest.rs`), own line | the A6 sandbox self-test (kernel/sandbox.md §8) | — | 1,454 | 1,530 |

**v0.1.1 (ADR-009).** Workspace validation, the access state, typed confirmations, snapshots, `/undo`, three charter keys, the owner-socket frames and the `access` event are Root code, because each decides what the zone may write. They get their own sub-budget line once SBX-009 lands, set by the same rule (measured + 5%, rounded up to 10) at the v0.1.1 release. As landed at cc16577, `src/access.rs` is about 400 raw lines, with smaller additions to `link.rs`, `zone.rs`, `sup.rs`, `charter.rs` and `sys.rs`, before this section's counting rule. Because the ≈ 7,500 total is a provisional owner decision, the release raises it by that line rather than squeezing other modules.

**M7 (lane ROOT-S1; provisional owner decision 2026-10-20, owner may override; m7-design.md §6, §10).** ROOT-S1 adds about 360 lines to `sealed/`: draw, the held-out reserve, template probes and rotation (90); `g4_slice` on the audit driver (70); the exact sign-flip (35); the incumbent cache and sentinel (45); scans (25); the milestone check, the `confirmed` pointer and the span counter (50); records and CLI (45). The `sealed/` line rises from 840 to ≈ 1,100 (m7-design.md §10), and the release sets it by this section's rule (measured + 5%, rounded up to 10). The design's own count estimate is about 1,160 lines measured (sealed/ 795 → ≈ 1,160; `init-root` ≈ 7,540 → ≈ 7,910; UNVERIFIED until measured), which would put the line near 1,220, so the release measurement decides. As for v0.1.1, the total rises by that line; the core subtotal is unchanged.

**Measured (build, 2026-10-20).** WP24's earlier ≈ 6,450 used a different count. This method gave 7,053 for `init-root` at dd5c358. At cdb49e8, after R-DEC's money-meter commit, it gave 7,085, the column above. Both counts include WP21b's install path, FIX5's `proposal.file` and the later fixes. R-SAFE and R-DEC are still adding Root code for v0.1.0, so the table is re-measured at release.

**Dependencies of the Root core** (versions checked on crates.io, 2026-10-20; vendored like the kernel's):

| Crate | Version | Why | Rejected alternative |
|---|---|---|---|
| `serde` (derive) | 1.0.229 | typed charter and frames, with no hand-written parsers | — |
| `serde_json` | 1.0.154, never `preserve_order` | ICJ (sorted keys), control link | — |
| `toml` | 1.1.8 | the charter (TOML 1.1), the same as the kernel's | — |
| `sha2` | 0.11.1 | chain hashes, content addressing, digests; shared with `ed25519-dalek` | `ring`: larger, C and assembly |
| `ed25519-dalek` | 3.0.0 (default features `fast`, `zeroize`) | lineage signatures; `verify_strict`; audited family (§3.10.2) | `ring::signature` (C/asm; no strict-verification API); `ssh-keygen -Y` (an external process per record) |
| `libc` | 0.2.191 | FFI listed above; `proc_listpids` and the `CMSG_*` helpers are exported for Apple | `nix`: more surface than needed |
| `seccompiler` (Linux only) | 0.5.0 | the zone seccomp program (§3.4) | hand-written cBPF |

The transitive dependencies are those of the crates above (`curve25519-dalek` 5.0.0, `ed25519` 3.0.0, `subtle`, `zeroize`, `digest`, and toml's parser stack). The core uses **no** async runtime, no HTTP client, no git library (the `git` CLI is used for the mirror), no time crate (RFC 3339 formatting and parsing take about 30 lines), no rand crate (`/dev/urandom`) and no hex crate. Egress adds `ureq` 3.4.2 (rustls with ring, no gzip) and `httparse` 1.10.1, listed and justified in root/egress.md §6; it also uses std threads only.

## 5. Failure modes and recovery

| # | Failure | Detection | Recovery | Agent sees |
|---|---|---|---|---|
| RF1 | kernel crashes | reaper / heartbeat / timeouts | sweep the zone; backoff; respawn | body boot `cold`; restart |
| RF2 | kernel crash loop, not on LKG | window count | rollback to LKG, no demotion | boot after `rollback_system` |
| RF3 | kernel crash loop on LKG | window count | demotion, audited-good, system safe mode | level −1; safe mode |
| RF4 | crash loop in system safe mode | window count | `Halted{crash_loop_safe}`; alert; retried after `halt_retry_s` (§3.6.6) | nothing runs until the retry |
| RF5 | upgraded kernel fails to boot, or crashes in probation | ready timeout / reaper | rollback to the previous tuple | `kernel_upgrade` record |
| RF6 | old kernel will not prepare a swap | `swap_prepare_timeout_s` | abort; the old kernel keeps running | `swap` event `aborted` |
| RF7 | Root crashes or is killed | launchd/systemd | the kernel exits on EOF; cold start sweeps orphans; latch honoured | restart (boot `cold`) |
| RF8 | lineage tail torn | missing `\n` | truncate after copying to `torn-*.bin` | — |
| RF9 | lineage line invalid, key missing | verification | `Halted{lineage}`; the owner compares with the mirror | nothing runs |
| RF10 | store blob missing or corrupt | hash check | next fallback tuple; else `Halted{no_tuple}` | boot after rollback |
| RF11 | disk below 512 MiB | watcher | `Halted{disk}`; leaves at ≥ 2 GiB free | nothing runs |
| RF11b | free space below the floor for 600 s | watcher | R1 pause; resumes at floor + 2 GiB (§3.6.6) | the body stops; it restarts `cold` later |
| RF12 | mirror unreachable or diverged | git exit, ls-remote | backoff; alert; never force | — |
| RF13 | `sandbox_check` missing (future macOS) | `dlsym` null | degraded sweep: the leader and its process group only, never a scan (D1); `status.sweep = "degraded"`, alert once | — |
| RF14 | `sandbox-exec` removed by Apple | install / start check | `Halted{tooling}`; a Root release is needed (§9.3 Q3) | nothing runs |
| RF15 | owner applies an invalid charter | validation | active charter unchanged; errors printed | — |
| RF16 | kernel floods the link or the lineage | framing limits / rate | crash or `root.rate`; alert | refusals |
| RF17 | owner client wedged | — | `kill -USR1`, latch file, or the CLI fallback | — |
| RF18 | clock jumps backwards | `at` would decrease | `at = max(prev, now)`; durations never negative | — |
| RF19 | exit 78 twice at kernel boot | exit code | `Halted{config}`, alert with the stderr tail; retried when `kernel/config.toml` changes or after `halt_retry_s` | nothing runs until the retry |
| RF20 | a `lineage.append` response is lost (kernel crash) | the kernel's outbox resends | the `idem` key returns the original `seq`, `dup:true`; nothing is duplicated (D7) | — |
| RF21 | an upgraded kernel crashes in probation after migrating `state/kernel/` | probation crash | rollback; the restored kernel restores the `kstate` snapshot first (§3.8) | boot after `rollback_system` |

## 6. Performance notes (spec §9)

- **The Root is off the hot path** of model calls except for egress (root/egress.md). Per body spawn it adds profile rendering (microseconds), `exec-clean` (< 1 ms) and one control-link round trip (≈ 50 µs), on top of the `sandbox-exec` cost measured in kernel/supervise.md §6 (≈ 22 ms median, once per spawn, never per tool call).
- **Owner relay:** one extra Unix-socket hop per frame, about 50 µs. The replay ring is capped at 256 KiB.
- **Lineage:** one `F_FULLFSYNC` per record (milliseconds), with tens to a few hundred records per day once egress aggregates (§3.10.3); Ed25519 signing costs tens of µs. A full verification of 100k records takes seconds (UNVERIFIED timing; RR-16 measures it) and runs once a day in the background.
- **Mirror:** pushes on a background thread; chunked files keep each push to one changed blob.
- **Disk walk:** metadata only, every 600 s, on the watcher thread. CPU is read from `wait4` at reaping, at no extra cost.
- **Memory target:** Root core RSS < 30 MB, excluding egress (RR-34 measures it).
- **Persistent upstream connections** now live in the Root egress gateway. They survive body restarts and also kernel restarts and upgrades, which helps spec §9 rank 2 (root/egress.md).

## 7. Invariants and security (spec §2.1, mapped onto the Root by spec §19)

| Invariant | How this doc enforces it |
|---|---|
| 1 Recoverability (R1) | Whole-system tuples live in a store the zone cannot see. Rollback installs the kernel from that store and hands the kernel its body. Crash loops end in safe mode, then `Halted`, never in a loop. Panic is owner-only, reachable without the kernel, latched, and survives every restart. `stop` gives a cheap non-emergency stop. |
| 2 Honest measuring stick (R6) | Through root/sealed-eval.md: audits and gate-of-gates run on Root-spawned instances. Their results reach the ledger only as Root-written records. |
| 3 Blast radius (R2, R3) | Every zone process is started under a Root-composed profile, with the zone tail last. For the Root's paths, writes, the network, marks and signals, the tail was tested against `(allow default)`. Reads outside the Root's paths are confined by the Root-embedded inner profiles and V3 (D9). There is no IP, and the only way out is the egress socket. The zone authors no profile text (Scheme redefinition, tested). exec-clean prevents fd leaks. The Root never opens zone paths by name (R-F2) and never runs zone code unsandboxed (R-X). |
| 4 Lineage (R4) | A hash-chained, Ed25519-signed, append-only log. The agent can read and verify it but cannot write it. It is mirrored off the machine with a deploy key the zone cannot reach and never force-pushed. Ladder changes can be verified mechanically. |
| 5 Humans only where irreversible | The charter's action classes, set once (root/egress.md). No owner step is required anywhere (D4, spec §18): safe mode clears itself, a pause resumes itself, `Halted` retries itself except on integrity failures, and owner signals are optional. |

**Threats and mitigations:**
- **A kernel (L3, agent code) tries to widen a sandbox.**
  - Profile text is never accepted.
  - Parameters cannot touch `Z_*` or the `extra_read` slots. The tail holds whatever the parameters are, and V3 keeps read parameters inside `INIT_HOME`, the toolchain, the developer directory and the owner's `extra_read` list.
  - `kernel` and sealed profiles cannot be requested (V5).
- **It forges a promotion or verdict.** The record carries `by:"kernel:<build>"`. The sealed audit catches a regression, and `ladder up` needs Root-written audit records.
- **It tries to stop the Root or forge owner input.** Signals outside its own sandbox are denied (Z7). `owner.sock` is unreachable (Z6), owner frames never come from the kernel, and a kernel-reported `owner` record must name a `qid` the Root actually relayed (§3.10.5).
- **It plants symlinks or hard links for the Root to follow.** R-F1 and R-F2: the Root reads zone data only through fds opened inside the zone, and fetches kernel source only through the sandboxed upload-pack. Z4's anchors stop the zone from replacing `run/`, `kernel/`, `root/` or `lineage/kernel.git`.
- **A sweep kills the owner's own processes.** The D1 three-condition rule (§3.5.4), and the CONVENTIONS §1 rule that sweep tests run only in disposable environments.
- **It exhausts resources.** Instance, compute, disk and store quotas apply (§3.14), and a persistent overage pauses the zone. The lineage is rate-limited and deduplicated, and frames are capped. Below 512 MiB free the zone is halted.
- **It triggers a panic or demotion to deny service to the owner.** It cannot reach any panic channel (§3.9.1). Demotion triggers come from Root-observed events. Self-inflicted ceiling hits only demote the agent itself, which gains it nothing.
- **It exfiltrates through the mirror.** The lineage holds only ICJ records that passed the §3.10.6 privacy checks. Kernel strings are bounded.
- **A malicious `notify.cmd` or mirror URL.** Both are owner-set in the charter, which is owner-only.
- **Residual risks:**
  - the Root runs as the owner and parses zone input (frames, egress requests); mitigated by serde, size caps and `deny(unsafe_code)` outside `sys.rs`;
  - `sandbox-exec` is deprecated by Apple (§9.3 Q3);
  - a pid can be reused during a sweep or peer check (kernel/initctl.md §3.3, accepted there).

## 8. Test plan

The end-to-end Root scenarios are testing.md §3.13's, and this doc adopts their IDs:
- RT-P1…RT-P11: panic from every channel, plus stop;
- RT-W1…RT-W5: whole-system rollback;
- RT-L1…RT-L9: the ladder, on test ledgers and the fake clock;
- RT-C1…RT-C8: ceilings at the Root;
- RT-E*: action classes;
- F-kill-root: the `root.*` fault points.

The Root-local unit and integration cases below are `RR-*`, so they do not collide with testing.md's `RT-*` IDs.

**Live-system safety (CONVENTIONS §1, D1).** Every case that signals, kills, sweeps or panics runs only in a disposable environment: a VM, a container, or a separate macOS user account. These are marked **D** below and include RT-P*, RT-W*, RR-6 and RR-11–RR-13. On the owner's machine, a test may signal only processes it spawned, by the pid it holds. RT-P* also have an **owner-machine variant** (tests/wp21.rs), built without `live-sweep`: the Root kills only the pids it holds (§3.5.4), and the test checks the outcome with read-only `ps` and `sandbox_check` queries instead of a process scan. The full RT-P2 and RT-P5 need the disposable environment with `--features live-sweep`.

| ID | Case | Expect | Maps to |
|---|---|---|---|
| RR-1 | zone tail over `(allow default)`: every row of §3.3.5 that names the tail | identical results | A6, S-Z |
| RR-2 | `kernel` profile probes: git, tar, writes, `lineage/kernel.git`, marks and decoy, egress, sockets, TCP/UDP, `~/.ssh`, an `extra_read` path | as §3.3.5; the `extra_read` path is readable, a sibling path is not | A6 |
| RR-3 | nested `sandbox-exec` inside a zone instance | fails (documents why the broker exists) | — |
| RR-4 | fuzz `spawn` args: `Z_` names, `MARK`, relative paths, `..`, NUL, paths outside the V3 roots, a `LINK_*` whose parent is not canonical, profile `kernel` or `sealed` | `root.args`; nothing spawned | A6 |
| RR-4b | `LINK_*` naming a real symlink inside a probe root (D8) | accepted; the probe can traverse it | kernel/sandbox.md S-M |
| RR-5 | an instance lists `/dev/fd`; sends itself `SIGTERM` | only 0–2 (the kernel: 0–4); default disposition (it dies) | A6 |
| RR-6 **D** | after a body spawn, list the Root's open fds; kill the kernel | no copy of the body's pipes; the body sees EOF and exits within 10 s | — |
| RR-7 **D** | the D1 membership rule against a decoy foreign process: a sandboxed process (not a zone instance) whose profile allows reading the mark, the decoy and the canary; and one that allows the mark only | the first is never matched; zone members are; the second is matched only if it is also denied the decoy and the canary | D1, S-M47 |
| RR-11 **D** | `/panic` with a `setsid` escapee and a `posix_spawn(SETSID)` daemon inside the zone | all killed in < 2 s; no process outside the disposable account's zone is touched | RT-P1, RT-P5 |
| RR-12 **D** | `kill -9` the Root while panicked; start it | `Panicked`, orphan sweep, no spawn | RT-P6 |
| RR-13 **D** | `init-root panic` with no Root daemon | the CLI sweeps by itself, exit 0. As built: the CLI writes the latch only (channel `cli_fallback`), exit 0; the next start is `Panicked` and records the panic once (§3.9.6) | RT-P2 |
| RR-14 | resume after panic on a test ledger at L1 | audited-good tuple runs; L0; the session continues | RT-P7 |
| RR-15 | stop then start | same tuple, same level, no `ladder` record | RT-P11 |
| RR-16 | 10k records: verify; flip a byte; delete a line; swap two lines; a non-canonical line; a wrong key | ok, or the bad `seq` reported for each; timing measured | — |
| RR-17 | `root.lineage.mid_append` abort; restart | torn tail copied and truncated; chain verifies | F-kill-root |
| RR-18 | kernel `lineage.append` with: an unknown kind, an `x_` kind, a float, 17 KiB, a home path, 601 records in an hour | `root.lineage` / accepted / `root.lineage` / `root.lineage` / `root.lineage` / `root.rate` | — |
| RR-18b | the same `idem` twice; a resend after `root.lineage.after_idem` | one record; the second answer has `dup:true` and the original `seq` | D7 |
| RR-18c | a kernel `owner` record naming a `qid` the Root never relayed, or a line whose first word differs | `root.lineage` | D4 |
| RR-19 | mirror to a local bare repo (`file://`, test mode) | commits by `init-root <root@init.invalid>`, TZ UTC; the owner email absent from all objects (the test reads it from git config locally only); non-fast-forward → `mirror_diverged`; never forced | — |
| RR-20 | charter: each C-rule violated; an unknown key; a looser ladder; a watch path under `INIT_HOME`; an `extra_read` of `/` or of the owner's home | each error with its key path; exit 2 | — |
| RR-21 | edit without apply; apply; lower `max_level`; lower `money.day_usd` below the kernel's soft day | no effect; `charter` record and view; `ladder cap`, no rollback; the kernel clamps, nothing halts (D6) | RT-L4, RT-L5, RT-C1 |
| RR-22 | the zone reads `charter.toml`, `keys/*`, `owner.sock`, the service file; reads `INIT_HOME/root/*` | the first four denied; the view readable | A6 |
| RR-23 | ledger fold over synthetic test ledgers; `--verify`; a forged signed `ladder up`; a `test:true` genesis on a production build | transitions at the expected `at` (fake clock); verify passes; the forgery fails; the production Root halts | RT-L1–RT-L9 |
| RR-24 | each demotion trigger; a burst of 5 demoting hits; a trigger at L0 | one level each; the burst costs one; rollback at L0 | RT-L7 |
| RR-24b | D6 classification: `class.path` (a), `class.level` and `root.level` (b), `ceiling.day` with a fresh guard snapshot that showed no headroom or with no header (c), `ceiling.day` on an honest stale snapshot, `class.volume`, `quota.compute` | demote / demote / demote / no / no / no | RT-C2, RT-C5, RT-L9 |
| RR-25 | at L0, a second `body` spawn; with `replaces` | `root.level`, a demoting `ceiling_hit`; the replacement is allowed | RT-C3 |
| RR-26 | `max_instances` (zone limit 10 with 2 reserved for sealed runs), CPU per day, the disk floor on a small volume (then the pause and auto-resume), `INIT_HOME` size, store full | refusals; R1 pause and auto-resume; no demotion | RT-C6 |
| RR-27 | `system.swap` from an agent source at L2 | `root.level`, demotion | RT-L9 |
| RR-28 | swap at test-L3 with a stub gog: pass → install (`lineage/kernel.git` `main` moves, `karchive` ref) → probation; a crash in probation (`kstate` restored); gog fail | LKG and `refs/heads/lkg`; rollback with `main` moved back; demotion | RT-W2 |
| RR-29 | owner types during a kernel restart and a swap | each line delivered exactly once (`qid` dedup) | A3 |
| RR-30 | the kernel floods `owner.out`; the owner types `/panic` | panic handled first | RT-P1 |
| RR-31 | a sandboxed process connects to `owner.sock` | Seatbelt `EPERM`; the peer check also rejects it through a test hole | A6 |
| RR-32 | `store.put` with a wrong hash; the kernel tries to open `~/.ssh/id_ed25519` to pass it | `root.store`; the open is denied | A6, RT-C8 |
| RR-33 | `Halted{crash_loop_safe}` with `halt_retry_s = 5` (test); `Halted{config}` and then a fixed `config.toml` | retried in safe mode; retried on the digest change | D4; RT-W5, A5-halt |
| RR-34 | `rootloc` within §4.8's per-module sub-budgets and their total of 7,540 at cdb49e8, about 7,500 (core subtotal ≤ 1,770; `root/sandbox-profiles` on its own lines). This is a provisional owner decision of 2026-10-20 that the owner may override. `cargo tree` lists only §4.8's crates and egress's; Root RSS. | pass by hand: 7,085 lines at cdb49e8 (core 1,656) by §4.8's method. `rootloc` itself is not built yet (WP17). | roadmap.md R13 |
| RR-35 **D** | every fault point of §4.7 | one Root; chain valid; no orphan; pointers consistent | F-kill-root |
| RR-36 | v0.1.1 workspace validation table, in a test home: a project dir; a symlink to it; a relative path; a missing path; a file; a dir outside the home; the home; `ROOT_HOME`, `INIT_HOME`, a parent of either, `~/Library/LaunchAgents`; a deny-list dir; an over-long path | accepted (canonical path), the same canonical path, then `refused.code` = `relative` `missing` `not_dir` `outside_home` `home` `root_path` ×4 `deny_list` `too_long`; access `read_home` for every refusal | ADR-009 |
| RR-37 | snapshot and `/undo` round trip on a small fixture dir on APFS: `workspace.set` (session snapshot), change and delete files, `undo` without, then with, the phrase; `undo` again | `root.confirm` first; then the tree equals the session snapshot byte for byte and a `pre_undo` snapshot holds the changed state; the second undo restores that state; at most 5 snapshots kept; a symlink in the tree is cloned as a link | ADR-009 |
| RR-37b | `undo` while the last heartbeat says busy | `root.busy` with `data.reason = "body_busy"`, before any `root.confirm`; nothing restored | ADR-009 |
| RR-38 | the size and volume skips: a workspace over 2 GiB (sparse files), one on a non-APFS volume (a disk image), one on another volume | no snapshot; a `snapshot` notice each; `undo` answers `root.not_found` | ADR-009 |
| RR-39 | full mode: `full.set{on:true}` twice in one session; after a new `workspace.set`; with `sandbox.allow_full = false`; `full.set{on:false}` | `root.confirm`, then on at once; `root.confirm` again (new session); `root.mode` `data.reason = "charter"`; off with no confirmation; `access` events with `restart: "idle"`, and `restart_pending` until the next live spawn | ADR-009 |
| RR-40 | `spawn` of each kind while a workspace is set, with the kernel sending its own `INIT_ACCESS=full` and `INIT_WORKSPACE=/` | the child's environment holds the Root's values (live body: the access and workspace; member: `read_home` or `none` and `""`; probe: `none` and `""`); the composed profiles carry `Z_HOME`/`Z_WRITE` only as §3.17.2's table says | ADR-009, kernel/sandbox.md S-A1–S-A26b |

## 9. Open questions

### 9.1 Cross-doc

Settled in the v0.3 round (ledger: reconciliation/v03-root.md): the sweep rule (D1); owner commands (D4); ceilings and demotion (D6); the lineage interface, the fleet kinds and `lineage/kernel.git` (D7); `LINK_*` (D8); `extra_read` (D9); the charter template (D13); egress's names (`money.*`, `keys/providers/`, `keys/actions/`, `egress_action`, `set_level`, the egress socket delivered in `hello`) and its TLS dependencies; sealed runs (kind `sealed`, `family_eval`, the `sealed.parallel` reserve); the release layout (`docs/`, `kupgrade-smoke/`, `src/`); the `kstate` snapshot; peer attribution from inside the sandbox (kernel/sandbox.md S-Z10); the fake clock and test ledgers; the view schemas for awareness.md; the level change notice; the share-bound formula; sweep-not-re-adopt after a Root restart.

Still open:
- **X2. kernel/sandbox.md and kernel/config.md:** take `extra_read` from the charter (`charter.json` view; §3.3.1) and drop `[sandbox] extra_read` from `config.toml`. Give the `body` profile one read rule for `INIT_HOME/root/` (agreed by awareness.md).
- **X4. kernel/proxy.md:** send `x-egress-guard: epoch=<E>` on every metered request (root/egress.md §3.6), which makes D6 rule (c) mechanical.
- **X7. root/sealed-eval.md:** the sealed run uses kind `sealed` with the `body`/`check` profile and the sealed tail (§3.3.6); confirm that it needs nothing more of the Root (a run reads `INIT_HOME` read-only; it writes and serves sockets only in its root).
- **X8. evolution.md:** define the `value_window` fields (§3.13.5).
- **X11. testing.md:** adopt the `RR-*` IDs, and mark the sweep, kill and panic scenarios as disposable-environment only (CONVENTIONS §1).
- **X12. kernel/gate.md:** report a G4 canary trip as `gate{reason:"canary"}`, which §3.13.3 now treats as a demotion trigger.

### 9.2 Owner

None left for this doc. CH-1, CH-2 and CH-3 and the template money were decided on 2026-10-20 (D13). OW-1 (the constitution text) stays with awareness.md.

### 9.3 Internal

- **Q1.** Should signing move to one signature per checkpoint (every N records) if lineage volume grows? Not needed at a few hundred records a day.
- **Q3.** `sandbox-exec` is deprecated. If Apple removes it, the Root moves to `sandbox_init_with_parameters` called from `exec-clean`. That is the same SBPL with no launcher, and `libsandbox` still exports it (UNVERIFIED for future macOS).
- **Q4 (v0.1.1).** `/undo` is refused while the live body is busy (`root.busy`), but the check reads the last heartbeat (every 5 s), so a turn that starts during a restore can still write. Recommended: ask the kernel to hold the body at its idle boundary for the length of the restore (the boundary §3.17.3 uses).
- **Q5 (v0.1.1, ADR-009 residual risk). Settled in part** by tails Z3a and Z3b (SBX-009, cc16577): shell startup files, git's user config, `~/.local/bin`, `~/bin`, `bin_dir`, login items, and every repository's hooks and config outside `INIT_HOME` are write-denied in every mode. Still writable, and possibly run later by the owner: the workspace's build and task files, a `.git` *file* (`gitdir:`) pointing at a git directory the regex does not cover, and in full mode editor startup files, language user-site and tool bin directories, tools that run configured commands, app plugin folders and other PATH directories (ADR-009, Threat model delta). Open: whether to deny more of these in full mode.

## 10. Spec deltas

| Spec v0.3 | This doc | Why |
|---|---|---|
| §13, R1: the Root "starts the agent zone" | The Root starts **every** agent-zone process (the spawn broker) and hands stdio to the kernel | Seatbelt does not nest (tested). One spawn path on both platforms. |
| §14 R1, §7.1: safe mode is terminal until the owner acts | System safe mode clears itself after 24 h healthy; a pause resumes itself; `Halted` retries itself, except on integrity failures (layout, tooling, lineage, no tuple), which stay stopped as a fail-safe | spec §18; D4: no owner step is required anywhere |
| §15: L4 "L3 held" | 60 days (placeholder) | a mechanical criterion needs a number |
| §15: "0 sealed-audit regressions in 14 days" | also needs ≥ 1 audit in the window | an empty window is not evidence |
| §15: "sealed-audit trend non-negative" | defined as halves-mean difference ≥ 0 over ≥ 4 audits | mechanical and verifiable |
| §15: "drops at least one level" | exactly one level per incident; triggers within an incident coalesce | a burst of refusals is one event |
| §15: "an attempt to exceed a ceiling" | only D6's three rules demote: (a) an R3 class or policy violation (including volume and rate on effectful classes), (b) a level-gated request the Root refuses, (c) a money-ceiling refusal that bypassed the kernel's guard. Ordinary ceiling hits, read-only volume and rate limits and quotas are logged only. | D6: metering lag and peak-price estimates must not demote |
| §15: the demotion triggers | adds a G4 canary trip in any gate | a canary scoring 1000 is evidence of cheating at any level (evolution.md, root/sealed-eval.md) |
| §16: audit "on every kernel upgrade" | after the upgrade's probation; manual in Phase 0 | a crashing kernel needs no audit; spec §19 |
| §14 R4: "optionally mirrored" | mirrored to `nagisanzenin/INIT-Lineage` from M0, pushed only by the Root with its own deploy key | owner decision CH-3 (D13) |
| v0.2 §7.1, supervise.md: `/panic` parsed by the kernel | parsed by the Root; the owner channel moves to the Root | panic must not depend on an evolvable kernel |
| §15: panic triggers demotion | kept: panic demotes one level; `init-root stop` does not demote | owner decision CH-2 (D13); a cheap stop keeps §12's asymmetry argument practical |
| §15: the L2 share grows "with value delivered" | the bound formula of §3.13.5 (+50‰ per good 30-day window, −100‰ per demotion, within 600–900‰) | a mechanical rule, proposed by evolution.md |
| §13: lineage the agent cannot write | adds `lineage/kernel.git`, Root-written and zone read-only, holding every installed kernel's source | D7 |
| §13: "Root changes come only from the maintainer or the owner" | the sandbox profile texts (kernel/sandbox.md) also ship inside the Root | the zone must not author SBPL (Scheme redefinition, tested) |
| §13: the agent zone reads and writes only its own world | v0.1.1: the live body reads the owner's home minus a deny list and writes the Root-validated workspace (or the home, in full mode); the Root takes APFS snapshots of the workspace and restores them on `/undo` (§3.17) | owner decision OW-PERM, ADR-009. The Root boundary itself is unchanged |

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.3):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP21b D1 | step 7 bundles `refs/heads/seed`, because git appends `/*` to a glob-free `--branches` pattern; a bundle file is copied; `list-heads` must show the branch; highest-semver tag | §4.3 |
| build: WP21b D2, D3, D6 | `install --seed-source <repo\|bundle> … [--no-service]`; `bin/init-root` and `install.json` are written before the install-mode kernel (`--seed-tag <tag>`); `--no-service` skips the service file; that kernel gets no `zone_start`/`zone_exit` and is bounded by hello, heartbeat, 1,800 s and the disk floor; a failure removes `install.json` | §3.10.5, §4.3, §4.4 |
| build: WP21b D4, D5 | the Root's `config.toml` template is `schema_version = 2` plus `[sandbox]` from `install.json`; `kernel.built_from` is read from `lineage/kernel.git` `main` | §3.7.1, §4.3 step 5 |
| build: WP21b Done (faster start) | `owner.sock` is served before tuple verification; the first `start` runs on the supervisor thread and re-checks the slot, so a kernel spawned into `Panicked`/`Halted` is killed | §3.6.5 |
| build: FIX4 | the dev profile builds `sha2`, `digest`, `block-buffer`, `crypto-common`, `hybrid-array` and `cpufeatures` at `opt-level = 3`; debug start → ready fell from 9.8 s to 1.7 s; release and hash checks unchanged | §3.6.5 |
| build: WP20 D3 | `ROOT_HOME` may be 80 bytes in `test-fixtures` builds | §3.2.1 |
| build: WP20 D4, D5, D11 | `start` always runs in-process (`--foreground` only echoes); R-F5 checks only the skeleton's top-level directories; a rejected owner-socket peer is closed without a reply | §3.2.3, §4.2, §4.3 |
| build: WP20 D6 | C13 lists the code's production minimums (≥ 0 in test mode); C9 intervals go down to 1 s in test mode | §3.12.3 |
| build: WP20 D7 | `mirror.url` defaults to `""`; built-in `crates` has `paths = ["*"]`; `margin_max_permille`; the view's `constitution: {sha256}` | §3.12.1, §3.12.5 |
| build: WP23 D10, WP25 D11 | the template writes `[mirror] url = ""` until M0, with the GitHub URL as a comment (owner ratification: build-deviations.md §3.5) | §3.11, §3.12.1, §3.12.2 |
| build: WP21 D1, D2 | without `live-sweep`, kills, sweeps and panic signal only held leaders and their groups; cold start never signals a previous Root's pids; the CLI panic fallback writes the latch only; the RT-P* owner-machine variant | §3.5.4, §3.6.5, §3.9.2, §3.9.3, §3.9.6, §4.3, §8 |
| build: WP21 D3, D4, WP21b D7 | a seedless home forms genesis from `kernel/bin` and an empty body; with no kernel installed the Root is `Stopped` with a notice | §3.6.5, §3.7.1 |
| build: WP21 D5 | the supervisor is pure `escalate()`/`backoff_ms()` plus an imperative loop; `ColdStart`, `Swapping` and `Paused` are not built | §3.6.1 |
| build: WP21 D6, D7, D8, D11 | heartbeat staleness counts from `hello`; the Root echoes only lines it queues; no `zone_exit` for a panic, `signal` null; `kill` counts the leader only | §3.6.3, §3.10.5, §3.15, §4.1.3 |
| build: WP4 D1 | the spawn example uses the body lane `"b0"` | §4.1.4 |
| build: WP7b D2 | the kernel derives the Root canary from the directory of `paths.egress_sock` | §4.1.3 |
| build: WP23 D2 | `idem` is indexed before the record (write-ahead); `root.lineage.after_idem` sits between the two | §3.10.3, §4.7 |
| build: WP23 D4 | demoting `ceiling_hit` records are capped at `kernel_lineage_per_hour` an hour | §3.10.3, §3.10.5 |
| build: WP23 D5, D6 | a Root-kind allowlist (`key_rotate` reserved); a kernel record's required field may be null | §3.10.5 |
| build: WP23 D7, D8, D9, D11, D12 | the open-time tail check is a constant 1,000; a head one record behind is tolerated; mirror details; `main` is created only with one bundle; the daily check runs on the mirror thread and halts through a hook | §3.6.5, §3.8.1, §3.9.1, §3.10.4, §3.11 |
| build: WP25 D1 | the L1 audit window spans the whole chain | §3.13.2 |
| build: WP25 D2, D4, D5, D7, D8 | an incident runs from its `ladder down` to the next `zone_start` or 600 s; a panic always costs one level; `body_safe` in safe mode is not a trigger; demotion while Stopped, Halted or Panicked; the level-refusal `ceiling_hit` fields | §3.13.3 |
| build: WP25 D3 | a mutation-suite failure does not roll back | §3.13.3 step 3 |
| build: WP6a N, WP24 N, WP24 D5 | sealed runs use PRELUDE + WP24's exact sealed text (to be drift-tested); the lane is `ROOT_HOME/run/sl-<run id>.sock` | §3.3.1, §3.3.6 |
| build: WP6a D2 | both `LINK_*` forms are accepted (D8); a symlink only inside a probe or member root; the live root's `LINK_*` equals its target | §3.3.4 (V2) |
| build: FIX1 N | the kernel never chmods `run/` (Z4); the Root keeps creating `INIT_HOME/run` at 0700 | §3.3.2 |
| build: FIX5 | `proposal.file` is built: the file format, the record (digest of `text`), the alert, `p-NNNNNN` ids, the refusals, a rolling 24 h seeded from lineage, the `status --proposals` line | §3.10.5, §3.12.4, §4.1.2, §4.1.3, §4.3 |
| build: WP24 Size (owner review) | measured ≈ 6,450 lines; the budget is under owner review (build-deviations.md §3.1) | §4.8, §8 (RR-34) |
| build-deviations §3.1 (provisional owner decision 2026-10-20, owner may override) | the budget is ≈ 7,500: per-module sub-budgets of measured + 5%, rounded up to 10, re-measured at release. The core subtotal (zone, sup, panic, egress money, lineage verify) is counted separately, and `root/sandbox-profiles` has its own lines. The count is 7,053 at dd5c358 and 7,085 at cdb49e8 (core 1,656), where the table totals 7,540. | §4.8, §8 (RR-34) |
| build-deviations §5 L8 | RR-21 and RR-33 "Maps to" now name RT-L4 and RT-W5/A5-halt, matching testing.md §3.18 | §8 |

**M7 design 2026-10-20** (m7-design.md §10, folded by lane DOC-M7; rows marked provisional are provisional owner decisions 2026-10-20 that the owner may override, CONVENTIONS §6):

| Was | This doc now says | Why | Section |
|---|---|---|---|
| §3.7.1 tuple v1 | tuple v2 with `body.t0`; `store.put kind:"t0"` | sealed runs (slices and audits) need the candidate's T0 head | §3.7.1, §4.1.3 |
| §3.7.2 pointers `current`, `lkg`, `audited` | a fourth pointer `confirmed`, moved only by a confirmed milestone check on the untouched reserve; a milestone regression rolls back to it (**provisional**); `audited` also moves on a qualifying G4 slice | the lineage's cumulative claim is tested once, exactly, on data no gate has seen; no second payment for the same sealed comparison | §3.7.2, §3.10.5 |
| §4.1.3 control link | `sealed.g4_slice` and its refusals; the M7 sealed record kinds | G4's gain decision runs in the Root (D5) | §4.1.2, §4.1.3, §3.10.5 |
| §3.13.2 L1 criteria (placeholders) | proposal: ≥ 3 of the 20 promotions passed an S-backed G4; an audit or slice-based `audited` move in the window; 0 canary, tamper or leak trips | T0 promotions cost $0 and skip G4, so 20 of them prove nothing | §3.13.2 |
| §4.8 `sealed/` budget 840 | ≈ 1,100, set at release as measured + 5% (**provisional**) | ROOT-S1's added duties | §4.8 |
| ADR-009 deny list | a dev-checkout install adds the checkout to `sandbox.read_deny` (**provisional**; the owner may remove it) | the S pack generators and `.git` would otherwise be readable by the live body | §3.12.2 |

## 11. References

All checked 2026-10-20.

- reconciliation/v03-decisions.md (D1, D4, D6–D9, D13) and CONVENTIONS.md §1 (live-system safety), read 2026-10-20.
- Local experiments on this machine (macOS 14.6 23G80, M2), 2026-10-20. They were run from a scratch directory that was then deleted by exact path. They touched only files and processes the session created, and processes were stopped by their own pid. A second run after the reconciliation covered the revised Z2, `Z_KGIT` and the decoy:
  - nested `sandbox-exec` (exit 71);
  - SBPL `define` override;
  - last-match-wins;
  - connecting to a socket in a `file-read*`-denied directory;
  - the zone tail over `(allow default)` and the composed `kernel` profile (every §3.3.5 row);
  - inherited socketpairs and `SCM_RIGHTS` under `(deny network*)`;
  - `sandbox_check(pid, NULL, 0)` from inside a sandbox; the path-filter form is covered by kernel/sandbox.md S-Z10.
- Rust std 1.97.1 source:
  - child signal mask inherited — https://raw.githubusercontent.com/rust-lang/rust/1.97.1/library/std/src/sys/process/unix/unix.rs ;
  - `std::io::pipe` stable since 1.87.0 — https://raw.githubusercontent.com/rust-lang/rust/1.97.1/library/std/src/io/pipe.rs ;
  - `impl From<io::PipeReader|PipeWriter> for Stdio` — https://raw.githubusercontent.com/rust-lang/rust/1.97.1/library/std/src/process.rs .
- libc 0.2.191 (Apple: `proc_listpids`, `CMSG_*`, `LOCAL_PEERPID`) — https://raw.githubusercontent.com/rust-lang/libc/0.2.191/src/unix/bsd/apple/mod.rs ; crates.io versions (serde 1.0.229, serde_json 1.0.154, toml 1.1.8, sha2 0.11.1, libc 0.2.191, seccompiler 0.5.0) — https://crates.io/api/v1/crates/<name>
- ed25519-dalek 3.0.0 (dependencies, features, release date) — https://crates.io/api/v1/crates/ed25519-dalek/3.0.0 ; API — https://docs.rs/ed25519-dalek/3.0.0/ed25519_dalek/struct.SigningKey.html , https://docs.rs/ed25519-dalek/3.0.0/ed25519_dalek/struct.VerifyingKey.html ; `Signature::from_bytes` — https://docs.rs/ed25519/3.0.0/ed25519/struct.Signature.html
- Quarkslab, "Security audit of dalek libraries" (2019) — https://blog.quarkslab.com/security-audit-of-dalek-libraries.html ; RustSec advisories — https://github.com/rustsec/advisory-db/tree/main/crates/ed25519-dalek (RUSTSEC-2022-0093), https://github.com/rustsec/advisory-db/blob/main/crates/curve25519-dalek/RUSTSEC-2024-0344.md
- sha2 0.11.1 usage — https://docs.rs/sha2/0.11.1/sha2/ ; seccompiler 0.5.0 — https://docs.rs/seccompiler/0.5.0/seccompiler/
- bubblewrap manual (`--disable-userns`, `--seccomp`, `--add-seccomp-fd`, `--die-with-parent`, `--new-session`) — https://raw.githubusercontent.com/containers/bubblewrap/main/bwrap.xml
- git: git-push, git-commit-tree, git-mktree, git-bundle, git-ls-remote, git-config (`GIT_CONFIG_GLOBAL`) — https://git-scm.com/docs
- The INIT docs this one builds on: kernel/supervise.md (§3.1, §3.4, §3.8.2, §3.11, §3.12, §3.13.3, §4.5–§4.7, §9), kernel/sandbox.md (§3.2–§3.6, §9), kernel/proxy.md (§3.3, §3.8, §9), kernel/config.md (§3.1–§3.2, §9), kernel/initctl.md (§3.1–§3.3, §9), kernel/gate.md (§3.13, §9), kernel/selfmap.md (§9), evolution.md (§3.14, §3.15, §4.4, §9), awareness.md (§4.5, §9), testing.md (§3.5b, §3.13, §9), root/egress.md (§3.3–§3.9, §4.3, §9), root/sealed-eval.md (§9), body/log-context.md (§3.8.1).
