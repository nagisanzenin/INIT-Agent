# INIT v1 UX: install, first run, daily use, update and uninstall

```
Status: draft v0.1 · 2026-10-20 · Area: ux (owner-facing lifecycle; V1_PLAN phase C, lane DES-UX)
Depends on: V1_PLAN.md, CONVENTIONS.md (§1, §3, §5, §6 OW-TUI, OW-PERM), tui.md, root/root.md (§3.2, §3.12,
            §3.13, §3.15, §3.17, §4.2, §4.3, §4.8, §9.3), root/egress.md (§3.2, §3.3, §4.2),
            kernel/supervise.md (§3.6.2, §3.6.3, §4.6), body/log-context.md (§3.5, §4.5), evolution.md (upstream),
            adr/009-codex-style-permissions.md, release.md, scripts/{install.sh, uninstall.sh, init},
            research_notes/INIT v1 providers UX vision/{install_daily_ux, subscription_plans,
            proxies_gateways_local, llm_api_providers}.md; the providers v2 design doc (V1_PLAN phase C, not yet
            written); cross_platform.md (not yet written, §3.11)
Defines:    the release bundle and its signed manifest; the one-line installers (install.sh, install.ps1 at the
            release host); the app directory; the `init` launcher binary (crate `init-cli`) and its subcommands;
            the first-run flow (six screens) and its setup mode; the background warm-up and repro check (owner
            side); `init update`, `init uninstall`, `init doctor`; the error copy ids `ux.*`; test ids UX-T1…UX-T23;
            work packages UX-1…UX-16.
Proposes:   (cross-doc, §9) owner-socket requests to root/root.md §4.2; key storage and plan metering to
            root/egress.md; the v1 command set, key map, status bar, notifications and trust-rule changes to tui.md;
            body commands and display fields to body/log-context.md; build and warm-up changes to
            kernel/supervise.md; per-turn checkpoints and per-folder trust to ADR-009.
```

---

## 1. Purpose

The owner's mandate for v1: "UIUX tuyệt vời từ cài đặt tới sử dụng hàng ngày, hiện tại đang quá clunky lúc setup" (great UI/UX from install to daily use; setup is too clunky today), on Linux, macOS and Windows (V1_PLAN.md items 1 and 4).

The owner's own first run on 2026-10-20 is the baseline this doc fixes:

| What happened | Cause | v1 fix |
|---|---|---|
| `scripts/install.sh` failed when run from `~` | the installer is a repo script and must be run from the checkout | one hosted command that works from any directory (§3.2) |
| `init: command not found` afterwards | `~/.local/bin` was not on PATH; the hint was one line at the end of a long log | the installer prints a path that works at once; the first screen fixes PATH with one key (§3.2.5) |
| about 5 minutes building the body, plus about 10 minutes of release build | the body and the release binaries were compiled on the owner's Mac | a prebuilt, signed bundle that includes the body; the self-build moves to a background warm-up that gates only self-change (§3.5) |
| the agent could not touch the owner's project | v0.1's sandbox wrote only `INIT_HOME` | ADR-009 (v0.1.1), plus a per-folder trust prompt and a checkpoint before every turn (§3.4.2 screen 5, §3.6.5) |
| two more manual steps: `~/.init-root/bin/init-root keys set deepseek` and a five-question shell wizard | key entry and the charter lived outside the app | both move into the first run of `init` (§3.4) |

**Goals** (each is tested, §8):
- **G1.** One command per OS. No Rust toolchain, compiler or repo checkout is needed to start chatting. It works from any directory.
- **G2.** Time to first prompt: at most 60 s from typing `init` to a reply when the owner takes the defaults, not counting the download. The machine's share is at most 20 s.
- **G3.** Six first-run screens, each answered by one keypress with a default, so pressing Enter six times gives a working agent.
- **G4.** The money ceilings and the Root's authority are visible at every step, in plain words.
- **G5.** No daily task needs a text editor, a path that is not on PATH, or a restart by hand.

## 2. Responsibilities / non-goals

**Responsibilities**
- The install path: the bundle, the installers, the app directory, PATH, and migration from v0.1.
- The `init` launcher and its subcommands.
- The first-run flow and the setup protocol it needs (proposed to the Root).
- The owner-visible side of the warm-up and the repro check.
- The daily-use contract: commands, keys, status bar, permission display, checkpoints, mentions, the away digest, notifications, sessions, cost views, headless mode, `/config`.
- Update, uninstall, doctor.
- Error copy and empty states, cross-platform differences, the tests and the work breakdown.

**Non-goals** (owned elsewhere; used here by name)
- Provider adapters, the model catalog and prices, auth kinds: the providers v2 doc. This doc only names the catalog fields it reads (§9 Q6).
- Sandbox mechanics on Linux and Windows: platform lane P and cross_platform.md.
- Root internals beyond the request shapes proposed in §4.3, which root/root.md owns once it accepts them.
- IDE and ACP integration, MCP, a GUI, and telemetry. INIT has no telemetry: it sends nothing anywhere except to the model provider the owner picks and, for updates, to the release host.

## 3. Design

### 3.1 Principles

1. **Install silently, onboard in the app.** The installer asks nothing. Every question is asked in `init`'s first run, where the defaults are visible and can be changed later in `/config`.
2. **One keypress per decision, with safe defaults:** Standard limits ($5 a day, $25 in total), start at L0, read-only for folders that are not git repositories, network only to the model provider through the Root.
3. **Chat never waits for evolution.** The self-build runs in the background and gates only self-change.
4. **Authority stays visible.** Root-sourced facts (money, level, access, panic) are shown as the Root's. The TUI gains no trust except forwarding one secret, once, to the Root.
5. **Consent before persistent changes.** Shell startup files, the background service, the toolchain download and OS keychain use each take one explicit keypress, are listed, and are undone by `init uninstall`.
6. **Plain copy.** Every message says what happened, whether money or files are affected, and the one command that fixes it (§3.10).

### 3.2 Install

#### 3.2.1 One command per OS

| OS | Command | When |
|---|---|---|
| macOS 14+ (arm64, x86_64); Linux (x86_64, aarch64; glibc ≥ 2.31) | `curl -fsSL <DIST>/install.sh \| sh` | v1.0 |
| Windows 10 1809+ and 11 (x86_64, arm64) | `irm <DIST>/install.ps1 \| iex` (PowerShell 5.1+) | v1.0, once the Windows sandbox from lane P lands (§3.11) |
| macOS, Linux | `brew install nagisanzenin/tap/init` (Homebrew tap) | v1.1 |
| Windows | `winget install` and `scoop install` manifests | v1.1 |

`<DIST>` is the release host. **PENDING OWNER (OW-UX1, §9):** the recommendation is GitHub Releases on the repo once it is public (V1_PLAN phase F): `https://github.com/nagisanzenin/INIT-Agent/releases/latest/download`. A private repo cannot serve anonymous downloads. The scripts honour `INIT_VERSION` (pin a version) and `INIT_CHANNEL` (`stable`, the default, or `latest`).

Package-manager installs use the same bundle. Under them INIT's own updater only notifies, and the package manager updates (§3.7).

#### 3.2.2 The bundle

One archive per target: `init-<version>-<target>.tar.gz`, or `.zip` on Windows. The targets are `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`. gzip is used because every target unpacks it with stock tools (macOS bsdtar, GNU tar, and PowerShell's `Expand-Archive` for the zip). Linux builds use the `gnu` target, not `musl`, because the local toolchain builds `gnu` and the repro check (§3.5.3) compares like with like.

```
init-<version>-<target>/
  bin/init                       the launcher (crate init-cli, §3.3)
  bin/init-tui                   the TUI (tui.md)
  bin/init-root                  the Root
  bin/init-kernel  bin/initctl  bin/init-mockllm
  bin/init-fetch                 downloads and verifies releases and the toolchain (§3.7, §3.4.9); the only INIT
                                 binary besides the Root that opens network connections
  body/init-body                 the prebuilt genesis body, built exactly as the kernel's G0 build (§3.5.1)
  body/ext/<name>.wasm           the seed's prebuilt extension components (today: count-lines)
  seed/seed-v<version>.bundle    src/<version>.bundle    docs/    kupgrade-smoke/    wit/
  assets/charter-template.toml   assets/CONSTITUTION.md  assets/catalog.json (the model catalog, providers v2)
  MANIFEST.json  MANIFEST.json.sig
```

**Size.** `target/dist/v0.1.0` is 86 MB unpacked (`init-kernel` 38 MB, a release-profile `init-body` 23 MB). The bundle's body is the dev-profile body the kernel builds, 37 MB as installed in `~/.init/bin` today, so the bundle is about 100 MB unpacked and an estimated 30–40 MB as tar.gz (UNVERIFIED; UX-1 measures it).

**`MANIFEST.json`** (ICJ, shown wrapped):

```json
{"v":1, "version":"1.0.0", "target":"aarch64-apple-darwin", "channel":"stable", "released_at":"2026-…Z",
 "commit":"<sha40>", "min_upgrade_from":"0.1.0", "notes_url":"…",
 "files":{"bin/init":{"mode":493,"sha256":"…"}, "…":{}},
 "body":{"commit":"<seed commit>", "profile":"dev", "rustc":"1.97.1", "sha256":"…"},
 "toolchain":{"rustc":"1.97.1", "channel_manifest_sha256":"…", "targets":["wasm32-wasip2"]}}
```

`MANIFEST.json.sig` is an Ed25519 signature in minisign format, made with the release key. The public key is compiled into `init` and `init-fetch` and printed in the README. The private key stays offline with the maintainer; lane F decides where.

**Signing of binaries** (Apple Developer ID and notarization; Windows Authenticode) is **PENDING OWNER (OW-UX2)**. The curl path works without it: curl does not set the `com.apple.quarantine` attribute, so Gatekeeper does not check binaries the script installs (UNVERIFIED on macOS 15; UX-T3 checks it). Browser downloads and Homebrew casks are quarantined and need signing.

#### 3.2.3 What the installer does

The hosted `install.sh` lives in the repo as `scripts/get-init.sh` (and `install.ps1` as `scripts/get-init.ps1`), so it is not confused with the from-source `scripts/install.sh`. It is POSIX `sh`: it must run under `sh`, dash and busybox ash, so it uses no bashisms. Each step prints one line.

1. **Who and where.** It refuses to run as root (`ux.as_root`). It maps `uname -s`/`uname -m` to a target and refuses anything else with the list of supported targets (`ux.platform`). On Linux it reads the glibc version (`ldd --version`) and refuses musl systems such as Alpine in v1.
2. **Which release.** It uses `curl`, or `wget` if curl is missing, to fetch `<DIST>/<channel>.json` (the version, the archive URL, its sha256 and size).
3. **Disk.** It needs about 250 MB free on the app directory's filesystem (the archive plus two copies while unpacking) and refuses with `ux.disk` below that.
4. **Download** to `<app>/update/<file>.partial` with a progress bar (`curl --progress-bar`, `wget --show-progress`), resuming (`curl -C -`) on a retry.
5. **Verify.** The archive's sha256 (`shasum -a 256` on macOS, `sha256sum` on Linux) must equal the channel file's. If `minisign` is installed it also checks `MANIFEST.json.sig`. **Trust on first install rests on HTTPS to `<DIST>`**, as with rustup, Homebrew and Claude Code: a tampered bundle could carry a tampered verifier, so the script does not pretend otherwise. The signature protects every later update, because `init-fetch` checks it with the key the installed binary already holds (§3.7).
6. **Unpack** into `<app>/versions/<version>.tmp/`, check every file against `MANIFEST.json`, rename to `versions/<version>/`, make the tree read-only, and point `<app>/current` at it (`ln -sfn`).
7. **Commands.** It links `~/.local/bin/init` and `~/.local/bin/init-agent` to `<app>/current/bin/init`. If `~/.local/bin/init` exists and is not INIT's, it stops and asks the owner to move that file; it never overwrites a foreign file. A v0.1 launcher (a bash script containing `INIT_ROOT_HOME_DEFAULT=`) counts as INIT's and is replaced (§3.2.8).
8. **PATH** (§3.2.5), then the closing lines.

The installer asks nothing, never runs `sudo`, writes nothing outside the app directory and `~/.local/bin`, starts no process, and does not touch `ROOT_HOME` or `INIT_HOME`.

```
$ curl -fsSL https://…/install.sh | sh
INIT installer · macOS 15.1 arm64 · channel stable
  downloading init-1.0.0-aarch64-apple-darwin.tar.gz (34.2 MB)
######################################################################## 100.0%
  ✓ checksum matches the release
  ✓ installed INIT 1.0.0 in ~/.local/share/init
  ✓ command: ~/.local/bin/init
  ! ~/.local/bin is not on your PATH yet. For this terminal, run:
        export PATH="$HOME/.local/bin:$PATH"
    INIT will offer to make this permanent when it starts.

Start INIT:  ~/.local/bin/init
```

When PATH already has `~/.local/bin`, the last line is `Start INIT:  init`.

**`install.ps1`** follows the same steps with `Invoke-WebRequest` (with `$ProgressPreference` set so the transfer is not slowed by the progress UI), `Get-FileHash` and `Expand-Archive`. On Windows the installer does update PATH, because that is the platform convention (rustup, scoop and winget shims do it): it adds `%LOCALAPPDATA%\Programs\INIT\bin` to the user PATH (`HKCU\Environment`) and to the current session, then says "open a new terminal for other windows".

#### 3.2.4 Directory layout

| What | macOS | Linux | Windows (provisional, §3.11) |
|---|---|---|---|
| app directory: versions, toolchain, update staging | `~/.local/share/init` | `${XDG_DATA_HOME:-~/.local/share}/init` | `%LOCALAPPDATA%\Programs\INIT` |
| commands | `~/.local/bin/{init,init-agent}` (symlinks) | the same | `%LOCALAPPDATA%\Programs\INIT\bin\{init,init-agent}.exe` (copies, on the user PATH) |
| `ROOT_HOME` | `~/.init-root` (unchanged) | `~/.init-root` | `%LOCALAPPDATA%\INIT\root` |
| `INIT_HOME` | `~/.init` (unchanged) | `~/.init` | `%LOCALAPPDATA%\INIT\home` |
| service | `~/Library/LaunchAgents/dev.init.root.plist` | `~/.config/systemd/user/init-root.service` | Task Scheduler task `\INIT\Root` |

```
~/.local/share/init/
  versions/<version>/        one unpacked bundle, read-only
  current -> versions/<v>    a symlink (Windows: a file current.txt naming the version)
  toolchain/rust-1.97.1/     only after `init setup evolution` downloads it (§3.4.9)
  update/                    init-fetch's staging (.partial files) and status.json
  install-source.json        {"channel":"stable","source":"script"|"brew"|"winget"|"scoop"}
```

- `~/.init-root` and `~/.init` keep their names: the Root's 64-byte socket-path rule (root.md §3.2.1), the owner's existing install and every other doc depend on them.
- **The app directory and `~/.local/bin` hold code that runs as the owner, outside every sandbox** (`init`, `init-tui`, `init-fetch`, the Root's install source). The zone must never write them. Proposed (§9 Q1): the Root refuses a workspace inside or containing either (reason `root_path`), and denies writes to both in every mode, which also closes tui.md T-4's full-mode caveat and root.md §9.3 Q5's `INIT_BIN_DIR` item.
- **Retention.** `versions/` keeps the current version, the previous one (for `init update --rollback`), and any version a running process still uses. `init-fetch` removes the rest by exact path after a successful update.

#### 3.2.5 PATH fix-ups

- **Changed in v0.5.4 (owner's decision, rustup and uv style):** the installer edits the shell's startup file by default, and `--no-modify-path` opts out. The earlier rule ("never edits shell startup files: under `| sh` its stdin is the script, so it cannot ask") left every new user one copied line short of a working `init-agent`, and the README's install block carried that line for every shell. Not asking is what rustup and uv do too: the edit is a marked block, made only when the command's directory is not on `PATH`, made once, named in the install output, and removed by the uninstaller. As built (scripts/install.sh `path_step`): zsh `${ZDOTDIR:-~}/.zshrc`, bash `~/.bashrc` on Linux or the login file on macOS (`~/.bash_profile`, else `~/.bash_login` or `~/.profile` when one exists), fish `~/.config/fish/conf.d/init-agent.fish`, any other shell `~/.profile`; the real `INIT_BIN_DIR`, quoted for that shell. The plan below (a first-run offer in the TUI, a backup file) is not built.
- It prints the absolute path that works in the current terminal.
- The first run checks PATH. The launcher knows whether it was found through PATH (it compares how it was invoked with a PATH lookup of `init`) and passes `--path-missing` to the TUI. Screen 1 then offers `p`: "Add ~/.local/bin to your PATH in ~/.zshrc". The edit is a marked block, appended through a temp file and a rename, with a one-time backup `~/.zshrc.init-backup`:
  ```sh
  # >>> INIT (added by init; `init uninstall` removes this block) >>>
  export PATH="$HOME/.local/bin:$PATH"
  # <<< INIT <<<
  ```
- Which file, from `$SHELL`: zsh → `~/.zshrc`; bash → `~/.bashrc`, plus `~/.bash_profile` on macOS when it exists and does not source `~/.bashrc`; fish → `~/.config/fish/conf.d/init.fish` containing `fish_add_path -g $HOME/.local/bin`. For any other shell INIT prints the line and edits nothing.
- **The Linux name clash.** If `init` resolves to `/sbin/init`, `/usr/sbin/init` or `/usr/bin/init` (systemd), the installer and screen 1 say so, put `~/.local/bin` first in the PATH block, and recommend the alias `init-agent`, which always works. INIT never suggests `sudo init`.

#### 3.2.6 Time to first prompt

| Step | Machine time (target) | Owner |
|---|---|---|
| installer after the download: verify, unpack, link | ≤ 3 s | — |
| `init` → screen 1: setup-mode Root up, preflight including the sandbox self-test | ≤ 2 s | reads |
| screen 2: detection and the live key test | ≤ 3 s | Enter, or pastes a key |
| screens 3–5 | 0 | three Enters |
| screen 6 → apply: charter, install with the prebuilt body, service | ≤ 8 s | Enter |
| first boot: daemon Root, kernel ready, body ready | ≤ 4 s | — |
| **total** | **≤ 20 s** | 30–40 s of reading and six keys |

The target is at most 60 s from typing `init` to a prompt that answers, p50 on the M2, without the download. UX-T13 measures the machine time with the owner's keys scripted.

#### 3.2.7 Migration from a v0.1 install

The owner's Mac has a v0.1 source install.
1. `install.sh` replaces the bash launcher in `~/.local/bin/init` with the link to the new binary.
2. (As built in v0.5.0: `init-agent upgrade`, §3.7's note.) The first `init` finds a configured `ROOT_HOME` (`install.json` exists), so there is no first run. It runs `init-root install --upgrade --release <app>/current` (root.md §4.3): new Root, kernel swap through `system.swap{source:"maintainer"}`, and the new seed bundle. **The body the agent has evolved is kept.** The agent considers the new seed through its own upstream merge flow (evolution.md), and the TUI says so.
3. It shows one "What's new in 1.0" panel, then the folder trust prompt for the cwd (§3.6.4).

The charter, keys, lineage, level and sessions are untouched. `scripts/install.sh` stays as the from-source path for maintainers (release.md).

### 3.3 The launcher `init`

Today `init` is a bash script (`scripts/init`). v1 replaces it with a small binary, because Windows has no bash and the launcher now has subcommands. It is the crate `init-cli` in the `tui/` workspace: maintainer code, never evolvable, std plus `serde_json`, `#![forbid(unsafe_code)]` except a Windows platform module that calls the OS through `windows-sys` (version pinned in UX-3; UNVERIFIED here).

**Rules** (the launcher's equivalent of tui.md T-1…T-5):
- **L-1, no network.** Downloads belong to `init-fetch`.
- **L-2, no secrets.** `init keys set` hands the terminal to `init-root keys set`, which reads the key itself.
- **L-3, fixed execs.** It runs only `init-tui` and `init-fetch` next to itself; `ROOT_HOME/bin/init-root` (or the app's `init-root` before the first install); the platform service manager by absolute path (`/bin/launchctl`; `/usr/bin/systemctl` or `/bin/systemctl`; `%SystemRoot%\System32\schtasks.exe`); the OS URL opener (§3.4.6); and `xcode-select --install` on consent (§3.4.9). It never looks a program up in PATH.
- **L-4, not as root.** It refuses to run as uid 0, or elevated on Windows (`ux.as_root`).
- **L-5, bounded writes.** It writes only the app directory, the `~/.local/bin` links, the marked PATH block (with consent), the service file through the Root, and the Trash (uninstall).
- **T-4 kept.** It refuses an `init-tui` or `init-root` inside `INIT_HOME`, as `scripts/init` does today.

| Invocation | Does |
|---|---|
| `init [flags]` | not set up → the first run (§3.4). Set up → make sure the Root runs (start it through the service manager, or detached when there is no service; at most 10 s, else `ux.root_down`), then exec `init-tui` with the flags |
| `init -p …` · `-r` · `--resume <id>` · `--full` · `--plain` · `--raw` · `--verbose` | passed to `init-tui` (§4.1) |
| `init start` · `stop` · `status` · `panic` · `ledger` · `audit …` · `charter …` | exec `init-root <the same>` |
| `init keys set <provider> [--stdin]` · `keys list` · `keys remove <provider>` | exec `init-root keys …` |
| `init login chatgpt` | exec `init-tui --login chatgpt` (the flow needs a screen) |
| `init setup` | on a configured install, screens 2–6 as a review; otherwise the first run |
| `init setup evolution [--toolchain PATH] [--now]` | §3.4.9 |
| `init update [--check] [--now] [--channel stable\|latest] [--rollback] [--repair]` | §3.7 |
| `init doctor [--fix] [--online]` | §3.9 |
| `init uninstall [--keep-data]` | §3.8 |
| `init --version` | launcher, TUI, Root, kernel and body versions, and the channel |
| `init --which` | what `init` would run (kept from `scripts/init`) |

On Windows, "exec" means spawn, wait, and pass on the exit code; Ctrl-C reaches the child because console control events go to the whole process group.

### 3.4 First run

#### 3.4.1 Setup mode

1. The launcher finds no `ROOT_HOME/install.json`. It spawns `init-root setup --serve` from the app directory, detached in its own session (`setsid`; on Windows `DETACHED_PROCESS`), waits up to 5 s for `owner.sock`, then execs `init-tui --first-run [--path-missing]`.
2. **Setup mode is the Root binary without a charter and without a zone.** It creates the `ROOT_HOME` skeleton and the lineage key (root.md §4.3 steps 1–3), opens `owner.sock` with the usual peer check, and answers only the setup requests of §4.3 plus `status` (with `slot:"setup"`). It never spawns a zone process.
3. It keeps the choices made so far, never secrets, in `ROOT_HOME/state/setup.json`. If the owner quits, the next `init` resumes at the same screen. It exits 30 min after the last client detaches without applying.
4. `setup.apply` finishes the install (§3.4.7) and hands off. With the service, it writes and loads the service, releases `run/root.lock` and exits; the service manager starts the daemon Root, and the TUI's ordinary reconnect (tui.md §3.11) attaches to it. Without the service, it execs `init-root start` in place, in the same detached session.

Why the Root and not a separate setup daemon: keys, the charter and the install are the Root's business, and the TUI keeps its single endpoint (T-1). The new code goes on its own line in root.md §4.8 (`setup`, an estimated 350–450 lines, UNVERIFIED), outside the core subtotal (§6, OW-UX4).

#### 3.4.2 The six screens

Every screen has a title, `n/6`, a body and a hint line. Enter takes the highlighted default; ↑/↓ or a digit picks; Esc goes back one screen; Ctrl-C twice quits (choices are kept). The mockups are for 80×24 and wrap down to 60 columns. Dollar figures and counts in the mockups are illustrative.

**Screen 1: welcome and checks**

```
 INIT 1.0.0 · first-time setup                                          1/6
 ─────────────────────────────────────────────────────────────────────────
 INIT is an agent that improves itself. A separate supervisor, the Root,
 holds your keys and your spending limits and can always roll the agent
 back. The agent can never change the Root.

 Checks
   ✓ macOS 15.1, Apple silicon        ✓ sandbox works
   ✓ git 2.46                         ✓ 112 GB free (INIT keeps 20 GB free)
   ! ~/.local/bin is not on PATH      p: fix it in ~/.zshrc
   · Rust is not set up: chat works now; self-improvement turns on later
     with `init setup evolution`.
 Theme: auto (dark)    t: change

 Enter continue · p fix PATH · q quit
```

| Check (`setup.preflight`; Root side unless noted) | Pass | On failure |
|---|---|---|
| OS and architecture | a supported target | `ux.platform`, blocking |
| sandbox | a one-instance self-test: Seatbelt on macOS; bwrap with user namespaces, seccomp and Landlock on Linux; lane P's design on Windows | `ux.sandbox`, blocking |
| git ≥ 2.39.4 | found by absolute path (`/usr/bin/git`, Homebrew's, Git for Windows) | `ux.git_missing`, blocking (the body syncs itself with git at boot) |
| disk | free ≥ the floor (default 20 GB) + 1 GB | `ux.disk`: offers a lower floor; blocking until resolved |
| home path length | `ROOT_HOME`, `INIT_HOME` ≤ 64 bytes | blocking; the copy says how to set `INIT_ROOT_HOME` |
| PATH (launcher) | `init` resolves to INIT's | not blocking; `p` fixes it (§3.2.5) |
| Rust toolchain | rustc 1.97.x with `wasm32-wasip2`, and a C linker | not blocking: "chat works now" (§3.4.9) |
| terminal (TUI) | colours, kitty keyboard protocol | information only, e.g. "Shift+Enter: in Terminal.app use Ctrl+J or \ then Enter" |

The theme is detected with an OSC 11 background query; `t` cycles auto, dark, light, ANSI-only. The choice is saved with `prefs.set` at apply time.

**Screen 2: model provider**

```
 Model provider                                                         2/6
 ─────────────────────────────────────────────────────────────────────────
   ● DeepSeek         DEEPSEEK_API_KEY is set in your environment
   ○ Ollama           running on this computer · 3 models can use tools
   ○ ChatGPT plan     sign in with your ChatGPT account (Plus, Pro)
   ○ Another provider Anthropic, OpenAI, OpenRouter, Gemini, … (paste a key)

 INIT tests the key once (cost under $0.001) and the Root stores it.
 Enter: use the DeepSeek key · ↑↓ choose
```

- Rows come from detection (§3.4.3), ordered: an environment key for a supported provider, a key the Root already holds, a local server with a tool-capable model, the ChatGPT plan, then "Another provider". The first row is the default.
- **Enter on a key row** reads the value (from the environment, or from a masked field for "Another provider": provider list → masked paste field) and sends `keys.set{test:true}` (§3.4.4). On success the screen shows `✓ deepseek-flash answered in 410 ms · $0.00002 · key kept by the Root` and moves to screen 3 by itself. On failure it shows the copy (`ux.key_invalid`, `ux.key_no_credit`, `ux.provider_down`) and stays.
- **Enter on a local server** lists its tool-capable models with the recommended one highlighted, and warns once: "Local models cost nothing per token but are weaker at long tasks. INIT checks tool calling first (about 10 s)." The tool-call conformance probe belongs to providers v2.
- **Enter on ChatGPT plan** starts the sign-in (§3.4.6).
- `m` picks a model other than the provider's recommended one. The list is the catalog's models for that provider, with prices.
- When the owner looks for Claude or Gemini subscriptions, the list says why they are not there: "Claude Pro/Max and Google AI subscriptions can't be used by other apps. Use an Anthropic or Gemini API key." (subscription_plans.md Q0.)

**Screen 3: spending limits**

```
 Spending limits                                                        3/6
 ─────────────────────────────────────────────────────────────────────────
 The Root enforces these at the network edge. The agent cannot raise them,
 and no request goes out once a limit is reached.

   ○ Frugal      $2 a day ·  $10 in total     about 20 tasks a day
   ● Standard    $5 a day ·  $25 in total     about 50 tasks a day
   ○ Generous   $20 a day · $100 in total     about 200 tasks a day
   ○ Custom…

 Includes INIT improving itself, which may use up to 60% of a day's limit.
 Estimates use deepseek-flash prices. Lowering a limit is instant; raising
 one needs you to type the new amount.
 Enter: Standard · ↑↓ choose
```

- "About N tasks a day" is `floor(day_usd / ref_task_usd)`, rounded to two significant figures, where `ref_task_usd` is the catalog's measured median task cost for the chosen model (providers v2; M7 measures it). Without a measured value the column is left out rather than guessed.
- Custom asks for the daily and total amounts with charter rule C5 (positive, total ≥ day).
- For a ChatGPT plan or a local server the subtitle changes: "Your ChatGPT plan's own weekly cap applies to plan usage. These limits cover any pay-per-token provider you add later." The screen is still shown, because the limits guard every paid provider.

**Screen 4: autonomy cap**

```
 How far may INIT grow?                                                 4/6
 ─────────────────────────────────────────────────────────────────────────
 INIT always starts at L0 and earns each level from its measured record.
 A panic drops it one level. You set the highest level it may reach.

   ○ L0  chat, tools, and improving its own code through its gate
   ○ L1  + picks among the models you allow, writes its own practice tasks
   ○ L2  + several copies working in parallel
   ○ L3  + improves its own kernel (the Root still checks every change)
   ● L4  + adapts or fine-tunes models, inside your money limits
 This version supports up to L1. A higher cap takes effect after updates.
 Enter: L4 (a cap; INIT starts at L0) · ↑↓ choose
```

The lines summarize root.md §3.13.4. The default is the charter template's `max_level = 4` (the owner's decision CH-1, D13); PENDING OWNER OW-UX5 asks whether new users should get the same default. The "supports up to" line shows the build's `SUPPORTED_MAX_LEVEL` (L1 above is illustrative).

**Screen 5: this folder**

```
 This folder: ~/code/myapp (a git repository)                           5/6
 ─────────────────────────────────────────────────────────────────────────
   ● Let INIT edit files here    reads your home, writes only this folder
   ○ Read only                   reads, changes nothing
   ○ Not now                     exit; run init in a project folder

 Never readable: ~/.ssh, cloud credentials, keychains, browser and mail
 data, ~/.init-root. INIT saves a checkpoint before every turn; /undo puts
 files back, including changes made by shell commands.
 Enter: let INIT edit here
```

- The default is "Let INIT edit" in a git repository and "Read only" elsewhere (the Codex pattern; install_daily_ux.md §2).
- The cwd is the home folder: "You started INIT in your home folder. It can read your files but will edit none until you run init inside a project folder." Enter continues read-only. The question is never asked for `~` again (avoiding Claude Code's repeated home prompt, issue #97726).
- The cwd is refused by the Root's workspace rules (root.md §3.17.1): the reason in plain words (`ux.folder_refused`), and Enter continues read-only.
- The answer is stored by the Root per folder (`workspace.trust`, §4.3) and asked again only for a new folder.

**Screen 6: ready**

```
 Ready                                                                  6/6
 ─────────────────────────────────────────────────────────────────────────
   Provider    DeepSeek · deepseek-flash              /model to change
   Limits      $5.00 a day · $25.00 in total          /config
   Autonomy    starts at L0 · may reach L4
   This folder ~/code/myapp · INIT can edit           Shift+Tab
   Improving   off until Rust is set up (init setup evolution)

   ● Run INIT in the background and start it when you log in
   ○ Start it now only (after a restart, run: init start)
   Notifications: when INIT needs you or finishes while you are away

 Nothing is sent anywhere except to your model provider. No telemetry.
 Enter: start INIT
```

- On macOS, when the service is chosen: "macOS will show 'Background Items Added'; INIT appears in Login Items as init-root."
- Enter sends `setup.apply`. Progress is polled from `setup.state` and shown in place: `✓ limits saved ✓ agent installed (prebuilt, signature ✓) ✓ background service ✓ INIT started · 6.8 s`. Then the chat screen opens with the first-chat empty state (§3.10).
- The charter's `purpose.text` gets a default sentence (§3.4.7); `/config purpose` changes it.

#### 3.4.3 Provider detection (`providers.detect` and the TUI)

1. **Environment, TUI side.** The Root's environment is the service manager's, not the owner's shell, so only the TUI can see `DEEPSEEK_API_KEY` and its kind. The TUI checks a fixed list of names for presence: the catalog's `env` names per provider, compiled into the TUI at build time (for example `DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`, `GROQ_API_KEY`, `MISTRAL_API_KEY`, `DASHSCOPE_API_KEY`). A value is read only after the owner picks that row.
2. **Keys the Root holds:** `keys.list` gives provider ids and status, never values.
3. **Local servers, Root side.** Parallel GETs with a 300 ms connect-and-read timeout, to these loopback addresses only: `127.0.0.1:11434` (`/api/version`, `/api/tags`: Ollama), `127.0.0.1:1234` (`/v1/models`: LM Studio), `127.0.0.1:8080` and `127.0.0.1:9931` (`/v1/models`: llama.cpp's server, MLX-LM, LocalAI). A server is identified by its response's shape, never by the port alone (ports collide: proxies_gateways_local.md). For Ollama, `POST /api/show` per model reads `capabilities` and keeps the models that list `tools`. Nothing else is probed, nothing off loopback, and only during setup or `/login`.
4. **ChatGPT plan:** always offered.

**A key goes only to the provider the owner confirmed.** A pasted key's prefix preselects the provider (`sk-ant-` Anthropic, `sk-or-` OpenRouter, `AIza` Gemini, `xai-` xAI, `gsk_` Groq, `sk-proj-` OpenAI). An ambiguous prefix (`sk-` alone could be DeepSeek or an older OpenAI key) asks. INIT never tries one key against several providers, because that would hand the key to the wrong company.

#### 3.4.4 The live key test

`keys.set{provider, secret, test:true}`. The Root stores nothing until the test passes:
1. **Format:** 1–512 bytes of UTF-8 with no control characters (root/egress.md §3.2).
2. **One authenticated request** through the egress code path to the provider's own host, using the catalog's `test` recipe: a free `GET /models` where the provider has one, else a one-token completion on the default model. Timeout 10 s. Cost at most $0.001, booked against the ceilings as purpose `setup`.
3. **Result.** OK (model, latency, cost) → store, `egress.reload()`. 401 or 403 → `ux.key_invalid`. 402 or the provider's insufficient-balance error → `ux.key_no_credit`. A network error or 5xx → `ux.provider_down`, with "Save the key anyway? [y/N]" (the key may be right while the provider is down).

In test mode the Root accepts only `test-` keys and loopback upstreams, as today (root/egress.md §3.2).

#### 3.4.5 Key storage: `keys.set` over `owner.sock`

**Decision:** the TUI reads the key in a masked field and forwards it in one `keys.set` request on its request connection. The Root validates, tests and stores it. The TUI never stores it. This is the research's recommendation (install_daily_ux.md §6.2), adopted for v1 because pasting a key into a second command in another directory was the clunkiest step. It is a **provisional owner decision 2026-10-20 (owner may override; OW-UX3)**, taken because the owner said not to wait. It changes tui.md T-2 and the invariant "keys never leave the Root" to "keys pass through the TUI's memory once, on their way to the Root" (§9 Q3).

**TUI rules for the secret**
- The key field is a separate input mode. Its bytes never enter the input history, the transcript, a `line` frame, the screen (it shows `sk-…` and the length), or any log.
- The field's buffer is preallocated (1 KiB), so it never reallocates and leaves no stale copy. After the request is written, the buffer is overwritten with zeros in a byte loop followed by `std::hint::black_box` (stable since Rust 1.66), so the compiler keeps the stores. The request bytes are built in a preallocated buffer and zeroed the same way. No new crate is needed.
- An environment value is read with `std::env::var_os` only after the owner picks its row, and is handled the same way.
- **Secret-paste guard.** A chat line that matches a key pattern (`sk-[A-Za-z0-9_-]{20,}`, `sk-ant-`, `AIza[0-9A-Za-z_-]{35}`, `ghp_`, `github_pat_`, `xai-`, `gsk_`) is held with: "This looks like an API key. Sending it gives it to the agent and your model provider. Send anyway? [y/N]". The default is No.

**Root rules.** `keys.set` is accepted only on `owner.sock`, from a peer that is not sandboxed, like `charter.apply`. The secret is held in the existing secret type (no `Debug`, `Display` or `Serialize`; zeroed on drop; root/egress.md §3.2).

**Where keys live** (root/egress.md §3.2 owns it; proposed in §9 Q2):
- **Default everywhere:** the 0600 file `ROOT_HOME/keys/providers/<id>`, as today. On Windows the same file, encrypted with DPAPI (`CryptProtectData`, user scope) under an owner-only ACL.
- **Opt-in `keys.store = "os"`** in the charter, once releases are signed with a stable identity (OW-UX2): macOS Keychain (a generic password, service `INIT Root`, account `<provider_id>`), the Linux Secret Service when a session bus exists, Windows Credential Manager. The macOS Keychain is not the default because unsigned or ad-hoc-signed builds change identity with each update, which would make the Keychain prompt after every update (install_daily_ux.md §6, Gaps).

`init keys set <id>` stays for shells. v1 adds `--stdin`, so a script can pipe a key in (`printenv OPENAI_API_KEY | init keys set openai --stdin`); a key is never accepted as an argument or an environment variable.

#### 3.4.6 ChatGPT sign-in (plan usage)

INIT uses OpenAI's "Sign in with ChatGPT" path for open-source local apps, which is ALLOWED (preview) for a project like INIT (subscription_plans.md Q0, Q1). It never reuses Codex CLI's client id or the `chatgpt.com/backend-api` endpoint (GRAY), and never offers Claude.ai or Google consumer sign-in (PROHIBITED).

The flow runs in the Root; credentials never leave it:
1. The TUI sends `auth.start{provider:"openai-chatgpt"}`. The Root creates a PKCE verifier (S256), `state` and `nonce`; binds a loopback listener on `127.0.0.1:1455`, path `/auth/callback`, for at most 10 minutes; and returns `{url, expires_at}`. The first time, the URL uses `client_id=dynamic_agent_client` with `agent_name_hint=INIT` and a persistent random `ext_agent_host_id` kept in `ROOT_HOME`. The scopes are `openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`, with `resource=https://api.openai.com/v1`.
2. The TUI shows the URL as text and as an OSC 8 link. Enter opens it with the OS opener (`/usr/bin/open`; `xdg-open` by absolute path; `explorer.exe` on Windows). This is a second permitted exec, which amends tui.md T-3 (§9 Q3). Only a URL that starts with exactly `https://auth.openai.com/` is ever passed to it.
3. The browser comes back to the listener. The Root checks `state`, exchanges the code, verifies the ID token against OpenAI's JWKS (`nonce`, `aud`, `iss`, `exp`), confirms `chatgpt.tokens.use.direct` was granted, and stores the issued client id and the tokens in `ROOT_HOME/keys/oauth/openai-chatgpt.json` (0600). It then closes the listener. The TUI polls `auth.status` every 500 ms and shows `✓ Signed in · ChatGPT Plus`.
4. **Refresh** is the Root's alone, one at a time, before expiry: access tokens last 1 h, and refresh tokens rotate with a 30-day life.
5. **Use.** The egress injects the bearer for provider `openai-chatgpt`. The body's Responses adapter sends `store:false` and `stream:true` (providers v2). Plan-funded requests are metered in tokens and booked at $0 against the money ceilings. The status bar shows `ChatGPT plan` instead of a price. A 429 `subscription_sharing_usage_limit_exceeded` becomes `ux.plan_limit`.
6. `/logout chatgpt` and `init uninstall` revoke the refresh token at OpenAI's revocation endpoint, then delete the file.

Over SSH the listener is on the machine that runs the Root, so the copy says: "Open this link in a browser on this computer, or forward the port first: ssh -L 1455:127.0.0.1:1455 <host>". If port 1455 is busy (for example Codex signing in at the same time): `ux.auth_port`. Whether OpenAI accepts other loopback ports is UNVERIFIED (§9). The Root's `oauth` module is an estimated 350–450 lines on its own budget line (UNVERIFIED).

#### 3.4.7 Charter creation, without a wizard script (`setup.apply`)

The six screens fill `setup.apply` (§4.3). The Root then:
1. **Renders `charter.toml` from its embedded template** (root.md §3.12.2), substituting:
   - `purpose.text`: "My personal agent on this computer. It helps me with my work and improves itself, within my limits." (`/config purpose` changes it; charter rule C2 rejects the template's REPLACE ME text, so a default sentence is needed);
   - `autonomy.max_level`, `money.day_usd`, `money.total_usd`;
   - `money.day_starts_at_utc_offset`: the machine's current UTC offset, which the Root reads from the OS;
   - `quotas.disk_min_free_gib`;
   - the provider action tables and model prices for the chosen provider, from the catalog (providers v2 owns their shape);
   - `notify.cmd` for the OS (§3.6.8), and the `sandbox.*` defaults.
2. **Validates** with `charter::load` (C1–C15), writes the file 0600, and continues as `init-root install` does from step 4 (root.md §4.3), with two changes: the body is the bundle's prebuilt body (§3.5.1), and the service is written and loaded as screen 6 chose.
3. Stores the folder trust from screen 5 and the preferences (theme, notifications).
4. Reports progress through `setup.state{phase, step, steps, text}`.

The owner sees TOML only on request: `/config` has "show the file", and hand edits with `init charter check --apply` still work.

#### 3.4.8 Service install

| OS | "Run in the background and start at login" (default) | "Start it now only" |
|---|---|---|
| macOS | LaunchAgent `dev.init.root` (`KeepAlive`, root.md §4.3 step 9), loaded with `launchctl bootstrap gui/<uid>`. macOS 13+ shows "Background Items Added"; screen 6 warns about it. | the Root detached with `setsid`; `init start` after a reboot |
| Linux | `~/.config/systemd/user/init-root.service`, then `systemctl --user enable --now`. It runs while the owner is logged in. Starting at boot without a login needs `loginctl enable-linger`, which may need admin rights; INIT never runs it and the copy mentions it. With no systemd user session (WSL without systemd, containers, OpenRC), INIT falls back to "start it now only" and says so. | the same |
| Windows | a Task Scheduler task `\INIT\Root`: trigger at logon, restart on failure every minute (up to 999 times), run only while the owner is logged on, limited rights, no admin | the Root detached |

Loading the service is the owner's own action on screen 6, as root.md §4.3 step 9 requires. `init uninstall` removes it.

#### 3.4.9 The toolchain check and `init setup evolution`

Self-evolution builds the body from source. It needs Rust 1.97.1 with the `wasm32-wasip2` target, plus a C linker: the Command Line Tools on macOS, `cc` on Linux, and the MSVC linker on Windows (§3.11). Chat and tools need none of these.

Screens 1 and 6 report what exists. Then:

```
$ init setup evolution
INIT needs Rust 1.97.1 (with wasm32-wasip2) to rebuild and improve itself.
  ● Download it into ~/.local/share/init/toolchain  (about 150 MB, 600 MB on disk)
  ○ Use the toolchain you have: ~/.rustup/toolchains/1.97.1-aarch64-apple-darwin
  ○ Not now
Nothing is installed system-wide, and `init uninstall` removes it.
```

- **Download.** `init-fetch toolchain` reads `channel-rust-1.97.1.toml` from `static.rust-lang.org`, checks its sha256 against the value pinned in INIT's signed `MANIFEST.json`, checks each component against the channel manifest, and unpacks into `toolchain/rust-1.97.1/`, read-only. It is outside `INIT_HOME`, so the agent can never alter its own compiler. No rustup, and no shell profile edits. The sizes are UNVERIFIED (UX-7 measures them).
- **No C linker.** On macOS INIT offers `xcode-select --install`, Apple's own installer dialog. On Linux it prints the distro's command (`sudo apt install build-essential`, `sudo dnf install gcc`). INIT never runs anything with admin rights.
- Then the launcher sends `toolchain.set{path}` (§4.3). The Root checks `rustc -V` and the installed targets, updates `install.json` and the kernel config's `[sandbox]` together (the broker compares them), and the warm-up starts (§3.5).
- `--toolchain PATH` and `--now` (start even on battery power) are for scripts.

#### 3.4.10 Non-interactive setup and line mode

- **`init --yes`** takes every default. The provider must be known without input: an environment key named by `--key-env NAME`, a key the Root already holds, or `--provider local`. Folder access defaults to read-only unless `--workspace rw`. The service follows `--service login|now|none`. If anything needs a choice, it exits 64 and names the flag.
- **Line mode** (`--plain`, or no TTY): each screen prints its text and `[Enter = <default>]`, then reads one line: empty for the default, a digit for an option, `q` to quit. The key prompt turns echo off when stdin is a TTY. From a pipe it accepts a key only in a test build with `root.test_mode` (UX-T13); otherwise it asks for `--key-env`.

### 3.5 Background warm-up and the repro check

#### 3.5.1 Install with the prebuilt body

`init-root install` gains `--prebuilt-body <dir>` (root.md §4.3 step 8; §9 Q1). The kernel's install mode imports the seed and clones `body/` as today, checks that `body/`'s HEAD equals `MANIFEST.body.commit`, then `store.put`s the prebuilt body and components instead of building them. The Root checks every blob's sha256 against the signed manifest, forms the genesis tuple with `built:"prebuilt"`, and records the manifest's hash in the `genesis` lineage record. This takes seconds instead of about 4 minutes.

The release builds the prebuilt body with exactly G0's build command (kernel/supervise.md §3.6.2: `cargo build --frozen --bin init-body --profile dev`, vendored sources, the same environment) in a build-slot path of the same length and name pattern, so a local rebuild can match it.

#### 3.5.2 The warm-up

After the first turn ends (never before the first reply), when a toolchain is configured and the kernel is idle, the kernel runs a `warmup` job: a cold build of the genesis commit with G0's build command in the build slot.
- **Priority.** The Root's broker spawns the build instance at background priority: background QoS on macOS; nice 19, `SCHED_IDLE` and the idle I/O class on Linux; `PROCESS_MODE_BACKGROUND_BEGIN` on Windows. `CARGO_BUILD_JOBS = max(1, cores / 2)`.
- **Pauses.** While a turn runs (it resumes at idle); on battery power (the Root reads the power source: `pmset -g ps` on macOS, `/sys/class/power_supply/*/online` on Linux, `GetSystemPowerStatus` on Windows); and when free disk would drop below the floor plus 2 GB.
- It also fills `cache/deps/`, so the first real self-change builds warm.
- About 4 minutes cold on an idle M2 by an early README figure that has no receipt, and more at background priority. The measured cold times, with their load, are in docs/guide.md "Time". It is not on the critical path.

#### 3.5.3 The repro check

*Not built (2026-10-29): no `repro` record or `root.repro_pending` refusal exists in the Root. A bundle install checks only that the prebuilt body is hash-equal to the maintainer's build (release.md §5b).*

The kernel reports the built body's sha256 (`repro.report{tuple, sha256}`), and the Root compares it with the genesis tuple's body blob.
- **Match:** lineage `repro{result:"verified"}`, and one dim notice: `▲ self-improvement ready · this computer's build is hash-equal to the shipped agent (3f9c0a1b)`.
- **Mismatch:** the locally built body goes through the same staging probe the gate uses for a new body (kernel/supervise.md §3.6.3). If it passes, it becomes the LKG, with lineage `repro{result:"rebuilt_local", shipped, local}`, and the notice reads `▲ self-improvement ready · rebuilt from source on this computer (the shipped build differs)`. `init doctor` shows both hashes. If the probe fails, evolution stays off, chat keeps the prebuilt body, and the owner sees `ux.repro_failed`.
- **The promotion gate.** Until a `repro` record exists, the Root refuses `promotion` with `root.repro_pending`, which holds even against an agent-written kernel, and the kernel starts no evolution job. If the owner asks for a self-change meanwhile: "INIT will start improving itself after its build check (about 3 min). Chat is unaffected."

**Why.** The agent edits source. The body it runs must be one this computer can build from that source; otherwise the first self-change would silently replace a maintainer build with a different local one. Reproducibility across machines is UNVERIFIED: absolute paths and Xcode or Command Line Tools linker versions can change the bytes. Two mitigations are proposed: the kernel's build adds `--remap-path-prefix` for the slot and `CARGO_HOME` (which also keeps the owner's home path out of binaries, a privacy fix; §9 Q4), and the mismatch path above, so non-reproducibility costs a minute, not a disabled feature.

#### 3.5.4 What the owner sees

| State | Status bar (dim, rightmost, dropped first) | `/status` |
|---|---|---|
| no toolchain | `evolve off` | "Self-improvement is off: Rust isn't set up. Run init setup evolution." |
| waiting (first turn not done, a turn running, battery) | `evolve waiting` | the reason |
| building | `evolve 41%` (units built over units planned, from cargo's JSON messages) | an ETA from the observed rate |
| verified or rebuilt_local | nothing | the result and the hashes |
| failed | `evolve ✗` | the copy, and `init doctor` |

### 3.6 Daily use

#### 3.6.1 Commands

v1 amends tui.md §3.8. The TUI handles a fixed set itself (panels and pickers built from requests, with no authority of their own); Root, kernel and body lines are sent unchanged.

| Command | Handled by | Does |
|---|---|---|
| **Conversation** | | |
| `/new` (alias `/clear`) | body | start a new session; the old one stays in `/resume` |
| `/resume` | TUI picker, then body `/resume <id>` | reopen a past session (§3.6.9). **Renamed:** it no longer restarts a stopped agent |
| `/rename <title>` | body | name this session |
| `/compact [focus]` | body | as today |
| `/rewind` (also Esc Esc) | TUI picker → Root `rewind` and body `/rewind-to` | go back to before a turn: files, conversation, or both (§3.6.5) |
| `/undo` | Root | put the folder back to before the last turn; `/undo confirm` as today |
| `/diff` | body | what changed in this folder in this session |
| `/copy` | TUI | copy the last reply to the clipboard (OSC 52, written by the TUI itself) |
| **Money and model** | | |
| `/usage` (alias `/cost`) | TUI panel from Root `egress.stats` | §3.6.10 |
| `/ceiling` | TUI panel, then `charter.set` | see, lower (instant) or raise (typed amount) the limits |
| `/model` | TUI picker → Root `config.set{model}` | the models your charter allows at this level, with prices |
| `/context` | TUI panel from body fields | context used and left, and what fills it |
| **Folder and permissions** | | |
| `/permissions` (alias `/workspace`) | TUI panel from Root `access.get` | §3.6.4 |
| `/full on` · `/full off` | Root | as today (ADR-009) |
| **The agent** | | |
| `/status` | TUI panel from Root `status`, then the kernel's `/status` | Root state, level, body, warm-up, update, INIT's disk use |
| `/level` | TUI panel from Root `ledger` | level, cap, progress toward the next level |
| `/evolution` | TUI panel from Root `lineage.recent` | self-changes kept and rolled back, and why (§3.6.7) |
| `/stop` | kernel | pause the agent's current body; no demotion |
| `/start` | Root or kernel | **renamed from `/resume`**: start again after `/stop`, `init stop` or a panic |
| `/panic` | Root | as today |
| `/later`, `/accept`, `/bless`, `/reject`, `/kernel`, `/branch`, `/label` | kernel or body | as today; listed under "more" in the popup |
| **Setup** | | |
| `/config` | TUI form → Root requests | §3.6.12 |
| `/login` · `/logout <provider>` | TUI, screen 2's flow | add or replace a key; ChatGPT sign-in and sign-out |
| `/doctor` | TUI → Root `doctor` | §3.9 |
| `/update` · `/release-notes` | TUI → Root `update.*` | §3.7 |
| `/theme` | TUI → `prefs.set` | auto, dark, light, ANSI-only, ASCII |
| `/help` | TUI | commands and keys |

**The `/resume` → `/start` transition.** For one minor release the Root and the kernel still accept a bare `/resume` while stopped, panicked or halted, and answer with the notice "Use /start. /resume now reopens past sessions." The TUI never sends a bare `/resume` to the Root: it opens the picker. `init-root attach` users get the notice.

#### 3.6.2 Keys (compatible with Claude Code)

tui.md §3.7 stays. v1 adds or changes:

| Key | INIT v1 | Claude Code |
|---|---|---|
| Shift+Tab | cycle this folder's access: `rw → ro`, plus `FULL` when this session allows it (§3.6.4) | cycles permission modes |
| Esc Esc, idle, empty box | the rewind picker | the same |
| Ctrl+R | search earlier messages (this session's replay and this process's history) | the same |
| `@` | the file mention picker (§3.6.6) | the same |
| `!` at the start of the box | run a shell command in the agent's sandbox without the model (`!ls`); its output joins the conversation | the same |
| Ctrl+V | paste; a pasted image path becomes an image chip (§3.6.6) | also pastes clipboard images |
| `?`, Enter, Shift/Alt+Enter, Ctrl+J, `\` then Enter, Esc, Ctrl+C, Ctrl+C twice, Ctrl+D, Ctrl+O, Ctrl+T, Ctrl+L, Up/Down | as today | the same |
| Ctrl+G (`$EDITOR`) | **not in v1**: it needs the TUI to write a temp file and exec an editor found in PATH, which T-2 and T-3 forbid (§9) | opens the editor |

#### 3.6.3 Status bar v1

```
 L0 · rw ~/code/app · deepseek-flash · ctx 72% left · $0.04 · $1.23/$5.00 today · running   evolve 41% ↑1.0.3
```

| Field | Source | Notes |
|---|---|---|
| level | Root `status` | as today |
| access | Root `status.access` and the folder's trust | `rw <path>`; `ro <path>` in yellow; `no folder` dim; `FULL` red reverse video; `↻` while `restart_pending` |
| model | kernel `state.model` | `· ChatGPT plan` when plan-funded |
| context left | body `turn_end.ctx` (§4.4) | yellow at ≤ 20%, red at ≤ 10% with a `/compact` hint |
| run cost | kernel `state.task_musd` | `$0.04` |
| today against the ceiling | Root `egress.stats` | yellow at ≥ 80%, red at the ceiling |
| Root state | Root | as today |
| busy, queued | kernel and Root | as today |
| evolve | Root `status.warmup` (§4.3) | only while not ready (§3.5.4) |
| update | Root `update.status` | `↑1.0.3` in blue when an update is ready |

When the terminal is narrow, fields drop in this order: queued, busy, update, evolve, run cost, context, model, the folder path (the `rw`/`ro` badge stays), today. A Root state other than running, `FULL` and a red ceiling are never dropped. The authority rule of tui.md §3.9 holds: Root-sourced fields stay visually distinct from kernel and body fields.

#### 3.6.4 Permissions display

INIT asks no per-action approvals: ADR-007 removed human review, and ADR-009 chose scope plus undo. "Permissions" therefore means showing the scope clearly:
- **The badge** in the status bar.
- **Trust per folder.** The first `init` in a folder asks screen 5's question inline before the first line is sent. The Root remembers the answer (`workspace.trust`). Until it is answered, access is `read_home`.
- **Shift+Tab** switches this folder between rw and ro (remembered), and includes `FULL` only when the session started with `--full` or `/full on` was confirmed in it. Each switch applies at the agent's next idle point (`↻`).
- **Blocked actions.** When a tool fails because the sandbox denied it, the body adds `denied{op, path, reason}` to `tool_end` (§4.4), and the TUI renders `✗ write ~/notes/todo.md · outside this folder (Shift+Tab or /full on)`. A deny-list hit reads `· protected: secrets are never readable`.
- **`/permissions`:**

```
 This folder   ~/code/app · INIT can edit (rw) · checkpoint before every turn
 Reads         your home, except 23 protected paths      /permissions protected
 Writes        this folder, and ~/.init (INIT's own world)
 Network       your model provider only, through the Root
 Full mode     off · your charter allows it               /full on
```

#### 3.6.5 Checkpoints, `/undo` and `/rewind` (shell changes included)

v1 extends ADR-009's session snapshot to **a checkpoint before every turn** (§9 Q7):
- When the Root relays an owner line that starts a turn (the kernel's last `state` was idle), it first checkpoints the folder, then delivers the line. A steer during a run belongs to the current turn's checkpoint.
- **What a checkpoint is.** A manifest (path → sha256, mode, symlink target) plus a content-addressed store `ROOT_HOME/snapshots/<folder-id>/objects/`. Only files whose size, mtime, ctime or inode changed since the previous checkpoint are read and stored. Stored files are cloned where the filesystem allows (APFS `clonefile`, `FICLONE` on Linux btrfs and XFS, ReFS block cloning) and copied otherwise. Links are recorded, never followed.
- **Shell changes are included** because the checkpoint records the folder's state, not the tool calls. Claude Code's checkpoints miss changes made by shell commands (install_daily_ux.md §3).
- **Limits.** As ADR-009 (2 GiB, same volume as `ROOT_HOME`), plus at most 200,000 files. Above a limit the folder gets only the session snapshot, and a `snapshot` notice says so. Retention: 100 checkpoints or 30 days per folder, at most 5 GiB, within the disk floor; pruning is by exact path.
- **`/undo`** (with `/undo confirm`) puts the folder back to before the last turn, saving the current state first, so an undo can be undone. Each further `/undo` steps back one more turn. As root.md §9.3 Q4 recommends, a restore waits for the agent to be idle.
- **`/rewind`** (Esc Esc) lists the turns: time, the first 60 characters of the owner's line (the Root's own copy of the owner's text, never agent text) and the number of files changed. It offers "files and conversation", "files only" or "conversation only". The conversation part is a body command, `/rewind-to <qid>`, which branches the session log to just before that input (body/log-context.md §3.5; §9 Q5).
- **Target:** a checkpoint of a 20,000-file folder with a few changed files takes ≤ 150 ms p50 on the M2, so the start of a turn does not visibly lag (UNVERIFIED; UX-T15).

#### 3.6.6 `@` file mentions and images

- **`@`** opens a fuzzy picker of paths under the folder. The TUI lists directory entries (`read_dir`, never file contents), at most 50,000, skipping `.git`, `node_modules`, `target`, `.venv`, `dist` and `build`. File names pass the sanitizer, because the agent can create them. The chosen path is inserted as `@path/to/file`, and the body expands it, reading the file under its own sandbox rules (§9 Q5).
- **Images.** Dragging a file into the terminal pastes its path in most terminals (macOS, Linux and Windows Terminal), and the owner can also paste a path. When the pasted text is a single existing `.png`, `.jpg`, `.jpeg`, `.gif` or `.webp` path of at most 5 MB (one metadata call), the TUI shows `[Image #1 screenshot.png]` and sends `@image:"<absolute path>"`. The body attaches the image when the model accepts images, and otherwise replies with a `/model` hint.
- **Pasting a screenshot from the clipboard** (Ctrl+V of image data) is deferred: the TUI would have to read the OS clipboard and write a file, which T-2 and T-3 forbid (§9).

#### 3.6.7 The away digest

On the first attach of a TUI process, if the owner was away at least 30 minutes and something happened, the TUI shows one block from the Root's `digest` (since the Root's `last_detach_at`):

```
 While you were away (6 h 12 min)
   ▲ 2 self-changes kept · faster file search · shorter plans          /evolution
   ▼ 1 rolled back · gate G2: slower on 3 of 12 tasks
   $0.84 spent ($0.31 your tasks · $0.53 self-improvement) · level L0 → L1
   ↑ INIT updated to 1.0.3                                           /release-notes
```

- It comes only from lineage and the egress ledger, which the Root owns, so it is authoritative. Change subjects are agent-written text: sanitized and cut to 40 columns.
- Maintainer releases show as `↑` in blue, separate from the agent's own `▲`/`▼`.
- If nothing happened, nothing is shown.
- `/evolution` lists the last 20 self-change records with their subject, `Why` trailer, gate result and cost. `d` on a row asks the body to show that change's diff (`/show <sha>`): informative, since it comes from the agent's own repository.

#### 3.6.8 Notifications

| Event | TUI attached, terminal focused | attached, unfocused | no TUI attached |
|---|---|---|---|
| turn finished after ≥ 20 s | — | desktop notification | Root `notify.cmd` |
| ceiling reached | notice | notice + notification | `notify.cmd` |
| panic, demotion, halt | notice | notice + notification | `notify.cmd` (as today) |
| level up; self-change kept or rolled back | notice | — (the digest covers it) | — (the digest) |
| update applied | notice | — | — |

- **TUI side.** Focus comes from crossterm's focus events (`EnableFocusChange`, `Event::FocusGained`/`FocusLost`; UNVERIFIED in 0.29.0 here, checked in UX-9); unknown focus counts as unfocused. The mechanism depends on the terminal: OSC 9 (iTerm2, WezTerm, Ghostty), OSC 99 (kitty), OSC 777 (foot, rxvt), and BEL elsewhere (Terminal.app bounces the Dock icon, Windows Terminal flashes the taskbar). The text is a fixed TUI string ("INIT finished · 2 min 14 s", "INIT: daily limit reached"). Agent text never goes inside an escape sequence. `prefs.notifications = auto|bell|off`.
- **Root side** (proposed, §9 Q1). `notify.cmd` arguments may contain `{title}` and `{text}`, which the Root fills with fixed Root strings, and the Root also calls it for `turn_done` when no client is attached. Defaults: macOS `osascript` (the template's); Linux `/usr/bin/notify-send` when it exists; Windows none until cross_platform.md picks a mechanism.

#### 3.6.9 Sessions

- `init` attaches to the live conversation, because INIT is one running agent. `-c` is accepted and does nothing, for muscle memory.
- **`/resume` or `init -r`** opens a picker from the body's `/sessions` answer (§4.4): title, folder, last activity, turns, and an approximate cost. ↑/↓ and Enter open one; `/` filters; `r` renames. `init --resume <id>` opens one directly.
- `/new` starts fresh. Nothing is deleted.
- With no earlier sessions: "No earlier sessions yet."

#### 3.6.10 Cost transparency

The hard ceilings are INIT's clearest difference from tools whose limits are opaque (the top complaint in install_daily_ux.md §5), so money is visible without asking: the status bar always shows today against the ceiling.

```
/usage
 Today                          $1.23 of $5.00   ██▌░░░░░░░ 25%   resets in 5 h 12 min
   your tasks                   $0.70
   self-improvement             $0.53   (may use up to 60% of the day; /config)
 Since install                  $8.40 of $25.00
 This session                   $0.31 · 42 model calls · 1.2 M tokens, 91% from cache
 deepseek-flash, per M tokens   input $0.30 · cached $0.006 · output $1.20
 The Root enforces these limits outside the agent. Nothing is sent once one is reached.
```

- The split by purpose needs `egress.stats` to add `by_purpose` (task, evolution, gate, setup) (§9 Q2).
- `/ceiling` raises a limit only with the new amount typed (the Root's `root.confirm`, with the amount as the phrase) and lowers it at once.
- `init -p --max-usd` adds a soft stop per run (§3.6.11).

#### 3.6.11 Headless: `init -p`

```
init -p "<prompt>" [--output-format text|json|stream-json] [--workspace rw|ro] [--session new|current]
                   [--max-usd N] [--timeout S] [--quiet]
```

1. Attach without replay; `workspace.set` with the cwd. An untrusted folder is read-only unless `--workspace rw`, which is remembered like a trust answer.
2. `--session new` (the default) sends `/new` first and waits for its echo; `current` continues the live session. If the agent is busy, the prompt waits in the queue (up to `--timeout`, default 600 s).
3. Send the prompt. If stdin is a pipe, its content is appended after a blank line (capped at 900 KiB in total). If no prompt is given, stdin is the prompt.
4. Stream: in `text` mode the assistant text goes to stdout and tool rows to stderr (none with `--quiet`); `stream-json` writes each display frame as an NDJSON line with C1 and bidi characters re-escaped (as `--raw` does); `json` prints one ICJ object at the end: `{"cost_musd","duration_ms","exit","result","session","tool_calls"}`.
5. The run ends at the turn's `turn_end` followed by `state{busy:false}`. The TUI then detaches.

- `--max-usd` is a soft stop: the TUI watches `state.task_musd` and sends `interrupt` once it passes the amount, which may overshoot by one model call. The Root's hard ceilings always apply as well.
- Exit codes: 0 done; 2 timeout; 3 a ceiling was reached or `--max-usd` stopped the run; 4 stopped by a panic, a stop or a halt; 5 the turn ended with a provider error; 64 usage; 69 the Root is unreachable.

#### 3.6.12 `/config`

A form with one row per setting. Each change goes through a Root request; nothing is written by the TUI.

| Row | Request | Rule |
|---|---|---|
| purpose | `charter.set` | 1–4,096 bytes (C2) |
| daily and total limits | `charter.set` | raising needs the new amount typed (`root.confirm`); lowering is instant |
| day starts at (UTC offset) | `charter.set` | C5 |
| autonomy cap | `charter.set` | raising needs `L<n>` typed |
| disk floor | `charter.set` | at least 1 GB |
| model | `config.set{model}` (`/model`) | only models the charter allows at this level |
| self-improvement share | `config.set{evolution_share}` | soft, under the ceilings |
| notifications, theme | `prefs.set` | — |
| updates: auto, notify or off; channel | `prefs.set` | §3.7 |
| key storage: file or OS store | `charter.set{keys.store}` | only once releases are signed (OW-UX2) |
| show the files | — | prints the paths for hand editing |

`charter.set` edits only the named keys in place, then runs the same validate-and-apply path as `charter check --apply` (root.md §3.12.4). If the owner has restructured the file so a key is not on its own line, it refuses with `charter.not_editable{key}`, and the copy says to edit the file and run `init charter check --apply`. `config.set` rewrites the kernel config's `[model]` or `[budget]` keys and asks the kernel to pick them up at its next idle point (§9 Q4).

### 3.7 Update

**As built in v0.5.0 (lane UPGRADE; build/status/UPGRADE.md), a reduced version of this section.** There is no
`init-fetch`, no background check, no channels and no apply-when-idle: the owner runs `init-agent upgrade [--bundle F]
[--version V]` (or, from 0.4.1, `get.sh --upgrade`), which downloads and checks the release (sha256 only: OW-UX2's
signature is not built), unpacks it under `ROOT_HOME/cache/upgrade/` and runs its `install.sh --upgrade`. That is "stop
then replace": the Root stops, the new `init-root upgrade --release <dir>` checks the version (no downgrade) and the
body (the installed body's `--manifest` against the release's own body, both in a sealed sandbox; a mismatch is
refused, never migrated), snapshots what it replaces, installs the new Root, kernel, kernel source and launchers and
forms a tuple {new kernel, current body, current ext}; then a health probe, and `--commit` (`lkg` moves) or `--rollback`
(the snapshot comes back, `rollback` to the tuple from before). Not built: the kernel swap through
`system.swap{source:"maintainer"}` (step 3 below; unqualified), offering the new seed to the agent (item 6), `init
update --rollback` to an older version (item 7) and the lineage record `release{from, to}` (the upgrade's `tuple` record
carries `upgrade{from, id, to}` instead, so a Root that never heard of it still verifies the chain).

1. **Check.** The Root runs `<app>/current/bin/init-fetch check --channel <c>` once a day at a random minute, and on `init update --check`. It runs as the owner, outside any sandbox, with a 60 s timeout. `init-fetch` fetches `<DIST>/<channel>.json` and its signature, checks the signature with its embedded key, compares versions, and writes `<app>/update/status.json`.
2. **Download.** If a newer version exists, it is downloaded in the background at low priority, at most once a day, then its signature and every file's sha256 are checked and it is unpacked to `versions/<new>/`. The status becomes `ready`, and the status bar shows `↑1.0.3`.
3. **Apply when idle.** The Root waits until no turn has run for 2 minutes and no gate job is in its G4 run; it defers at most 24 hours, after which the next idle moment wins and any interrupted gate job restarts. It then runs `install --upgrade --release <app>/versions/<new>` (root.md §4.3): a new Root binary and restart, the kernel swap through `system.swap{source:"maintainer"}`, and the new seed bundle offered to the agent. It points `current` at the new version. TUIs reconnect by themselves (tui.md §3.11). A TUI older than the Root shows `↑ restart init to use the new version of this screen`. `init update --now` applies at once.
4. **Channels.** `stable` (the default) gets a release after it has spent 7 days on `latest` without a blocking regression; `latest` gets every release. `init update --channel latest` switches.
5. **Modes** (`prefs.updates`): `auto` (the default; PENDING OWNER OW-UX6), `notify` (download, then ask) and `off`. Homebrew, winget and scoop installs are always `notify`, and the notice names the package manager's command.
6. **Two kinds of change, shown differently.** Maintainer releases are `↑` in blue and say what changed: "INIT 1.0.3 installed: Root, kernel and this screen · /release-notes". The agent's own changes are `▲` (kept, green) and `▼` (rolled back, yellow). A release never resets the level, the lineage, the charter, keys or sessions. It appends a `release{from, to}` lineage record (§9 Q1). The new seed reaches the agent as an upstream it may merge through its own gate (evolution.md): "New seed 1.0.3 is available to INIT; it decides whether to adopt it."
7. **Rollback.** `init update --rollback` reinstalls the previous version kept in `versions/`, by the same path.
8. **Repair.** `init update --repair` downloads the current version again when `init doctor` finds a damaged install.
9. **Failure:** `ux.update_failed`. Nothing changes; INIT retries the next day, or now with `init update`.

### 3.8 Uninstall

```
$ init uninstall
This stops INIT and removes it from this computer.
  Stops      the Root and the agent (no demotion)
  Removes    the background service (LaunchAgent dev.init.root)
  Signs out  of ChatGPT (INIT's token is revoked)
  Deletes    the provider keys INIT holds: deepseek (key file), openai-chatgpt (token)
             Secrets are deleted, not moved to the Trash. Your keys stay valid at the providers.
  Removes    the PATH lines INIT added to ~/.zshrc
  Moves to the Trash (nothing else is deleted):
             ~/.init-root   ~/.init   ~/.local/share/init   ~/.local/bin/init   ~/.local/bin/init-agent
Type uninstall to continue: _
  ✓ moved to the Trash as "… 2026-10-20 21.04.05" · empty the Trash yourself
```

- `--keep-data` removes the program, the service and the PATH block, and keeps `ROOT_HOME` (charter, keys, lineage) and `INIT_HOME`, so a reinstall continues where it left off.
- **Secrets are deleted, not trashed:** a key file is overwritten with zeros, fsynced and unlinked, and OS store items are deleted, because a secret in the Trash is a leak. The lineage signing key is not a provider secret and may be the only copy, so it goes to the Trash with `ROOT_HOME`.
- **The Trash:** `~/.Trash` on macOS; the freedesktop Trash (`$XDG_DATA_HOME/Trash/files` plus a `.trashinfo` file) on Linux; the Recycle Bin on Windows (`IFileOperation` with `FOFX_RECYCLEONDELETE`). A path on a different volume from its Trash stops the uninstall with instructions, as `uninstall.sh` does today.
- It never signals a Root that the service does not manage; it asks the owner to stop that Root first (`uninstall.sh` step 3).
- It is the launcher's code, so it works when the Root is broken.

### 3.9 Doctor

`init doctor` and `/doctor` run read-only checks and print one fix per failure. `--fix` applies the listed safe fixes one at a time, each with consent. `--online` adds a live test of each provider key, each costing at most $0.001 against the ceilings.

```
✓ Install     INIT 1.0.2 (stable) · files match the signed manifest
✗ PATH        ~/.local/bin is not on your PATH
              Fix: init doctor --fix path
✓ Root        running · 1.0.2 · up 3 d · service loaded
```

| Id | Check | Fix offered |
|---|---|---|
| D1 install | the files in `current` match the signed manifest; the launcher and TUI versions match | `init update --repair` |
| D2 PATH | `init` resolves to INIT's and is not shadowed by `/sbin/init` | add the PATH block; use `init-agent` |
| D3 Root | running, reachable, its version | `init start` |
| D4 service | installed and loaded, or "start now only" was chosen | reinstall and load it |
| D5 charter | valid (`charter check`) | show the errors and the file's path |
| D6 keys | each key present and 0600 (or in the OS store); accepted, with `--online` | `/login` |
| D7 sandbox | `init-root status --selftest` | the platform's copy (`ux.sandbox`) |
| D8 disk | free space against the floor; INIT's own use (homes, app directory, checkpoints, caches) | prune checkpoints and caches by exact path |
| D9 self-improvement | toolchain, warm-up, repro result | `init setup evolution` |
| D10 lineage | `audit verify` on the tail | report only |
| D11 update | last check, last result, channel | `init update` |
| D12 terminal | colours, keyboard protocol, glyphs | tips, e.g. "Use Option as Meta key" in Terminal.app |
| D13 clock | the system clock is within 5 minutes of the release host's `Date` header (otherwise TLS fails) | the OS time settings |

Root-side checks come from a `doctor` request (`{checks:[{id, ok, text, fix}]}`); D1, D2 and D12 run in the launcher. The exit code is 0 when everything passes and 1 otherwise.

### 3.10 Errors and empty states

**Copy rules.** The first sentence says what happened, in plain words. The next says whether money or files are affected. The last says the one thing to do, as a command or a key. No stack traces: details live in `init doctor`. Each message has an id for tests (UX-T23). Values in angle brackets are filled in; the examples use DeepSeek.

| Id | When | Text |
|---|---|---|
| `ux.no_key` | no provider is set up | INIT has no model provider yet, so it can't answer. Nothing was sent or spent. Add one with /login (or: init keys set <provider>). |
| `ux.key_invalid` | 401 or 403 from the provider | DeepSeek rejected the key. It may be mistyped or revoked. Nothing was spent. Paste it again with /login, or make a new key on the provider's site. |
| `ux.key_no_credit` | 402, or the provider's insufficient-balance error | DeepSeek says your account has no credit left. INIT's own limits are not the cause. Top up with the provider, or switch with /model. |
| `ux.ceiling_day` | the daily ceiling is reached | Daily limit reached: $5.00 of $5.00. INIT has stopped calling models until the day resets in 5 h 12 min, and nothing above the limit was charged. To keep going today, raise it with /ceiling (you type the new amount). |
| `ux.ceiling_total` | the total ceiling is reached | Total limit reached: $25.00 of $25.00. INIT stays stopped, and nothing above the limit was charged. Raise the total with /ceiling when you are ready. |
| `ux.disk_low` | free space under the floor | Your disk is almost full: 18 GB free, under the 20 GB you asked INIT to keep free. INIT paused builds and self-improvement; chat still works, and in 10 minutes it pauses entirely. Free some space, or lower the floor in /config; init doctor shows what INIT itself uses. |
| `ux.provider_down` | timeouts or 5xx after the retries | DeepSeek isn't responding (it timed out 3 times). Your message is kept. Try again in a minute, check the provider's status page, or switch with /model. |
| `ux.sandbox` | the sandbox self-test fails | INIT can't run the agent safely on this system: <reason>. INIT never runs the agent without its sandbox. Fix: <platform command>, then run init doctor. |
| `ux.toolchain` | evolution needs a toolchain | Self-improvement is off because Rust 1.97 isn't set up. Chat and tools work normally. Turn it on with init setup evolution (it asks before downloading about 150 MB). |
| `ux.not_trusted` | a write in a read-only folder | INIT can read this folder but not change it. Press Shift+Tab to let it edit here, or see /permissions. |
| `ux.home_folder` | `init` started in `~` | You started INIT in your home folder. It can read your files but will edit none. cd into a project and run init there. |
| `ux.folder_refused` | the Root refuses the cwd | INIT won't edit <path>: <it holds INIT's own files \| it is on your protected list \| it is outside your home>. It can still read it. |
| `ux.root_down` | the Root can't be reached or started | INIT's supervisor (the Root) isn't running, so the agent is stopped. Your files and limits are safe. Start it with init start; if it won't start, init doctor explains why. |
| `ux.update_failed` | an update could not be verified or applied | The update to INIT 1.0.3 didn't finish: <the download's signature didn't match \| the download was interrupted \| not enough disk>. You're still on 1.0.2, unchanged. INIT will try again tomorrow, or run init update now. |

`<reason>` and `<platform command>` for `ux.sandbox`: "bubblewrap is not installed" → `sudo apt install bubblewrap` (or the distro's equivalent); "unprivileged user namespaces are turned off" → on Ubuntu 24.04+ an AppArmor rule allows bwrap (the exact command is cross_platform.md's; UNVERIFIED); "seccomp is unavailable" → a kernel with seccomp; "this Windows version has no INIT sandbox yet" → use WSL2.

More ids, in the same style: `ux.plan_limit` (ChatGPT plan cap used up; "raise INIT's cap at chatgpt.com/settings/usage, or /model"), `ux.auth_port` (port 1455 busy), `ux.as_root`, `ux.platform`, `ux.git_missing` (macOS: `xcode-select --install`; Linux: the distro command; Windows: `winget install Git.Git`), `ux.repro_failed`, `ux.disk` (the installer and preflight).

**Empty states**

| Where | Text |
|---|---|
| the first chat screen | INIT is ready in ~/code/app (it can edit here). Try: "what does this project do?" · "fix the failing test" · /help for commands · ? for shortcuts · /usage for spending |
| `/resume` with none | No earlier sessions yet. |
| `/evolution` with none | No self-changes yet. INIT proposes them as it works, and each must pass its gate first. (With evolution off, it adds: Self-improvement is off: init setup evolution.) |
| `/usage` at $0 | Nothing spent yet today. Limits: $5.00 a day · $25.00 in total. |
| `/level` at L0 | the progress toward L1, criterion by criterion |
| `/rewind` with none | No checkpoints in this folder yet. |
| `@` with no match | No files match. |
| the digest | nothing happened → nothing is shown |

### 3.11 Cross-platform differences

**Provisional.** `research_notes/INIT v1 providers UX vision/cross_platform.md` did not exist on 2026-10-20; refine this table when it lands, and when platform lane P settles the Linux and Windows sandboxes.

| Topic | macOS | Linux | Windows |
|---|---|---|---|
| install command | `curl … \| sh` | `curl … \| sh` (or `wget`) | `irm … \| iex` |
| archive | tar.gz | tar.gz | zip |
| app directory | `~/.local/share/init` | `$XDG_DATA_HOME/init` | `%LOCALAPPDATA%\Programs\INIT` |
| homes | `~/.init-root`, `~/.init` | the same | `%LOCALAPPDATA%\INIT\{root,home}` |
| PATH | marked block in the shell's rc file, with consent | the same, plus the `/sbin/init` clash and the `init-agent` alias | the installer sets the user PATH |
| service | LaunchAgent | systemd user unit; fallback "start now only" | Task Scheduler logon task |
| key storage default | 0600 file | 0600 file | DPAPI-encrypted file, owner-only ACL |
| optional OS store | Keychain (when signed) | Secret Service (needs a session bus) | Credential Manager |
| sandbox | Seatbelt (`sandbox-exec`) | bwrap ≥ 0.8 with user namespaces, seccomp, Landlock; Ubuntu 24.04+ restricts user namespaces through AppArmor | lane P (AppContainer, Job Objects); none yet |
| owner socket | Unix socket | Unix socket | AF_UNIX (Windows 10 1803+) or named pipe: lane P; the peer check needs a Windows equivalent |
| checkpoints | APFS `clonefile` | `FICLONE` on btrfs and XFS, else copies | ReFS or Dev Drive block cloning, else copies |
| Trash | `~/.Trash` | freedesktop Trash | Recycle Bin |
| notifications (TUI) | OSC 9 in iTerm2, Ghostty, WezTerm; BEL in Terminal.app | OSC 99 (kitty), OSC 777 (foot); BEL elsewhere | BEL (Windows Terminal flashes the taskbar) |
| `notify.cmd` default | `osascript` | `/usr/bin/notify-send` if present | none yet |
| URL opener | `/usr/bin/open` | `xdg-open` by absolute path | `explorer.exe` |
| power source | `pmset -g ps` | `/sys/class/power_supply/*/online` | `GetSystemPowerStatus` |
| git | Command Line Tools (`xcode-select --install`) or Homebrew | the distro's package | Git for Windows |
| linker for evolution | Command Line Tools | `cc` (gcc or clang) | MSVC build tools (large); the choice is lane P's |
| binary signing | Developer ID and notarization (OW-UX2); the curl path works without | none needed | Authenticode (OW-UX2); SmartScreen may warn without it |

**Terminals**
- **macOS.** Terminal.app has no kitty keyboard protocol, so Shift+Enter cannot be told apart from Enter; Ctrl+J and `\` then Enter always work, and Option+Enter works once "Use Option as Meta key" is on. iTerm2 (with CSI u enabled), Ghostty, kitty, WezTerm and Alacritty support the protocol.
- **Linux.** GNOME Terminal and Konsole: BEL notifications. kitty, foot, Alacritty, WezTerm and Ghostty: richer support. The Linux console (`TERM=linux`) gets the ASCII glyph set and 8 colours.
- **Windows.** Windows Terminal is recommended. crossterm reads keys through the console input API there, so Shift+Enter arrives with its modifier. The classic console host works on Windows 10 1809+ with VT processing, using ASCII glyphs. mintty (Git Bash) is not a Windows console, and crossterm's support there is UNVERIFIED, so the copy recommends Windows Terminal.

**Fonts and glyphs.** The TUI uses `▲ ▼ ■ ✓ ✗ › · ↻ ↑`, box drawing (`╭ ╮ ╰ ╯ │ ─`) and a braille spinner (`⠋ ⠙ ⠹`). These are in SF Mono and Menlo, DejaVu Sans Mono and Cascadia Mono; Consolas may lack the braille block (UNVERIFIED). An ASCII set (`^ v # + x > . * | -` and a `- \ | /` spinner) is used when `TERM=linux`, on the classic Windows console (`WT_SESSION` unset), or with `/theme ascii`. To pick it, the TUI also reads `WT_SESSION` and `TERM_PROGRAM` (§9 Q3). INIT's own UI uses no emoji, because emoji widths differ between terminals. Emoji in agent text are measured with `unicode-width`, which can disagree with some terminals on variation-selector sequences; the effect is cosmetic.

## 4. Interfaces

### 4.1 The `init` command line

```
init [--full] [--plain | --raw] [--verbose] [--no-replay] [-r | --resume <id>] [-c] [--yes [setup flags]]
init -p "<prompt>" [--output-format text|json|stream-json] [--workspace rw|ro] [--session new|current]
                   [--max-usd N] [--timeout S] [--quiet]
init start | stop | status [--json] | panic | ledger [--json] | audit verify [--full] | charter check [--apply]
init keys set <provider> [--stdin] | keys list | keys remove <provider>
init login chatgpt | logout <provider>
init setup | setup evolution [--toolchain PATH] [--now]
init update [--check] [--now] [--channel stable|latest] [--rollback] [--repair]
init doctor [--fix] [--online]
init uninstall [--keep-data]
init --version | --which | --help

setup flags (first run without screens): --key-env NAME | --provider <id>|local  --model <id>
            --limits frugal|standard|generous|<day>/<total>  --max-level 0-4  --workspace rw|ro
            --service login|now|none
```

`init-tui` gains `--first-run`, `--path-missing`, `--login <provider>`, `-p` and its flags, `-r`/`--resume`. The other tui.md §4.1 flags are unchanged.

### 4.2 Installer contract

| Variable | Meaning | Default |
|---|---|---|
| `INIT_VERSION` | install this exact version | the channel's current |
| `INIT_CHANNEL` | `stable` or `latest` | `stable` |
| `INIT_INSTALL_DIR` | the app directory | per OS, §3.2.4 |
| `INIT_BIN_DIR` | where `init` and `init-agent` go | `~/.local/bin` |
| `INIT_DIST_BASE` | the release host; anything but `https://` is refused unless `INIT_INSTALL_TEST=1` (tests only) | `<DIST>` |

Exit codes: 0 installed; 1 a download or verification error (nothing left behind except `.partial` files in `update/`); 2 unsupported platform; 3 not enough disk; 4 a foreign file is in the way; 5 refused to run as root.

### 4.3 Owner-socket requests (proposed to root/root.md §4.2)

Request framing as today: `{"v":1,"id","cmd","args"}`. "setup" means the request is answered in setup mode; "daemon" means by the running Root.

| `cmd` | `args` | `result` | Errors | Mode |
|---|---|---|---|---|
| `setup.state` | `{}` | `{phase:"fresh"\|"choosing"\|"applying"\|"handoff", screen, choices, step, steps, text}` (no secrets) | — | setup |
| `setup.preflight` | `{}` | `{checks:[{id, ok, blocking, text, data}]}` | — | setup |
| `setup.apply` | `{day_usd, total_usd, max_level, provider, model, disk_min_free_gib, purpose?, service:"login"\|"now"\|"none", trust:{cwd, mode}, prefs:{theme, notifications}}` | `{job}`; progress in `setup.state` | `charter.invalid{errors}`, `setup.blocked{check}` | setup |
| `providers.detect` | `{}` | `{stored:[{provider, status}], local:[{kind, base, models:[{id, tools}]}]}` | — | both |
| `keys.set` | `{provider, secret, test:bool, save_untested?:bool}` | `{store:"file"\|"os", test:{ok, model, latency_ms, cost_musd}\|null}` | `keys.invalid{status}`, `keys.no_credit`, `keys.unreachable`, `root.args` | both |
| `keys.list` | `{}` | `{keys:[{provider, status:"present"\|"insecure", store}]}` | — | both |
| `keys.remove` | `{provider, confirm?}` | `{removed}` | `root.confirm` | daemon |
| `auth.start` | `{provider:"openai-chatgpt"}` | `{url, expires_at}` | `auth.port_busy`, `auth.unsupported` | both |
| `auth.status` | `{}` | `{state:"waiting"\|"ok"\|"failed"\|"expired", plan?, error?}` | — | both |
| `auth.cancel` | `{}` | `{}` | — | both |
| `auth.logout` | `{provider}` | `{revoked:bool}` | — | daemon |
| `toolchain.set` | `{path}` | `{rustc, targets}` | `toolchain.invalid` | daemon |
| `workspace.trust` | `{cwd, mode:"rw"\|"ro"}` | `{access: AccessView}` | `workspace.set`'s refusals | daemon |
| `checkpoints.list` | `{limit?}` | `{workspace, checkpoints:[{id, at, qid, line_head, files_changed}]}` | `root.mode{no_workspace}` | daemon |
| `rewind` | `{checkpoint, files:bool, conversation:bool, confirm?}` | `{restored, saved, qid}` | `root.confirm`, `root.not_found` | daemon |
| `digest` | `{}` | `{since, now, kept:[{sha12, subject}], rolled_back:[{gate, reason}], demotions, level:{from, to}, ceiling_hits, spend:{total_musd, by_purpose}, releases:[{from, to}]}` | — | daemon |
| `lineage.recent` | `{kinds?, limit ≤ 50}` | `{records:[…]}` (published-view fields) | — | daemon |
| `charter.set` | `{changes:{<key>: <value>}, confirm?}` | `{applied, diff, digest}` | `root.confirm` (a raise), `charter.invalid`, `charter.not_editable{key}` | daemon |
| `config.set` | `{model?, evolution_share?}` | `{applied, restart:"idle"}` | `config.model_not_allowed` | daemon |
| `prefs.get` · `prefs.set` | `{}` · `{theme?, notifications?, updates?, channel?}` | the prefs | — | both |
| `doctor` | `{online?}` | `{checks:[{id, ok, text, fix}]}` | — | both |
| `update.status` · `update.check` · `update.apply` | `{}` · `{}` · `{now?}` | `{channel, current, available?, state, last_check, error?}` | `update.*` | daemon |
| `status` (existing) | | gains `warmup{state, done, total}`, `repro`, `clients:[{client, workspace}]`, `slot:"setup"` | | both |

Root lines: `/start` is new (§3.6.1), and a bare `/resume` is a deprecated alias for one minor release. Lineage kinds: `repro`, `release`. Error `root.repro_pending` on `promotion` (§3.5.3).

### 4.4 Display and state fields (proposed; they ride tui.md §10 Q2(b)'s pass-through)

| Frame | Field | Meaning |
|---|---|---|
| body `turn_end` | `ctx:{used_tokens, window_tokens}`, `cost_musd` | context use after the turn; the turn's cost by the body's count |
| body `tool_end` | `denied:{op:"read"\|"write"\|"exec", path, reason:"outside_workspace"\|"protected"\|"root_path"}` | a sandbox refusal, for §3.6.4 |
| body `info` (answer to `/sessions`) | `sessions:[{id, title, workspace, started_at, last_at, turns, cost_musd}]` | the `/resume` picker; newest 50 |

**Body commands** (proposed to body/log-context.md §4.5): `/sessions`, `/resume <id>`, `/rename <title>`, `/rewind-to <qid>`, `/diff`, `/show <sha>`, `!<command>`, and the expansion of `@path` and `@image:"<path>"`.

### 4.5 Files

| Path | Owner | Contents |
|---|---|---|
| `<app>/versions/<v>/`, `current`, `install-source.json` | installer, `init-fetch` | §3.2.4 |
| `<app>/update/status.json` | `init-fetch` | `{channel, current, available, state, last_check, error}` |
| `<app>/toolchain/rust-1.97.1/` | `init-fetch` | the downloaded toolchain, read-only |
| `ROOT_HOME/state/setup.json` | Root (setup mode) | choices so far, no secrets; removed after handoff |
| `ROOT_HOME/state/workspaces.json` | Root | `{"<canonical path>": {"at", "mode":"rw"\|"ro"}}`, at most 1,000 entries, least recently used dropped |
| `ROOT_HOME/state/prefs.json` | Root | theme, notifications, updates, channel |
| `ROOT_HOME/keys/oauth/openai-chatgpt.json` | Root | the issued client id and tokens, 0600 |
| `ROOT_HOME/keys/host_id` | Root | the persistent `ext_agent_host_id`, 0600 |
| `ROOT_HOME/snapshots/<folder-id>/{manifests,objects}/` | Root | per-turn checkpoints (§3.6.5), next to ADR-009's session snapshots |
| rc-file block | launcher | §3.2.5 |

### 4.6 Exit codes of the launcher

0 success; 1 error (message on stderr); 64 usage, or `--yes` missing a needed choice; 69 the Root is unreachable and could not be started; 77 a refused binary inside `INIT_HOME` (as `scripts/init`); 78 refused to run as root. Subcommands that exec `init-root` or `init-tui` return their codes (tui.md §4.2, root.md §4.3); `init -p` returns §3.6.11's.

## 5. Failure modes and recovery

| Failure | Detection | Behaviour |
|---|---|---|
| download interrupted | curl or wget fails | the `.partial` file stays; a re-run resumes it |
| checksum or signature mismatch | step 5 / `init-fetch` | nothing is installed or changed; `ux.update_failed` or the installer's message |
| foreign `~/.local/bin/init` | no INIT marker | the installer stops and names the file; nothing is overwritten |
| setup-mode Root does not open its socket in 5 s | the launcher's wait | the Root's stderr tail, plus `init doctor` |
| the owner quits during setup | detach before apply | choices kept in `setup.json`; the next `init` resumes at the same screen |
| `setup.apply` fails during install | the install step's error | as root.md §4.3 step 8: `install.json` is removed by exact path, every earlier step is idempotent, and screen 6 offers a retry |
| the service fails to load | the service manager's exit code | falls back to "start now only" with a notice; doctor D4 |
| port 1455 busy | bind fails | `ux.auth_port` |
| the browser never returns | 10-minute expiry | `auth.status` says `expired`; Esc cancels at any time |
| key test cannot reach the provider | network error, 5xx | "Save the key anyway? [y/N]" |
| the warm-up build fails | job error | `ux.repro_failed` or `ux.toolchain`; chat unaffected |
| an update while a turn runs | the idle rule | deferred (§3.7) |
| an update fails half-way | the install's error | the Root's upgrade replaces binaries as new inodes; on failure `init-fetch` points `current` back and the Root restarts on the previous version; `ux.update_failed` |
| a second `init` window in a different folder | `status.clients` lists another workspace | "Another init window is using ~/code/a. Switch INIT to ~/code/b? [y/N]"; No keeps the first folder and makes this window read-only |
| `init` run as root | uid 0 | `ux.as_root`, exit 78 |
| `init` shadowed by the system's on Linux | the PATH check | screen 1 and doctor D2; the `init-agent` alias |
| uninstall finds a Root not managed by the service | the lock is held | stop with the copy; nothing is moved |

## 6. Performance notes (spec §9)

| Metric | Target | Measured by |
|---|---|---|
| time to first prompt, defaults, without the download | ≤ 60 s p50 with owner time; ≤ 20 s machine time | UX-T13 |
| installer after the download | ≤ 3 s | UX-T3 |
| `providers.detect` | ≤ 600 ms (300 ms per probe, in parallel) | UX-T8 |
| key test | ≤ 10 s timeout; typically ≤ 1.5 s | UX-T7 |
| `setup.apply` with the prebuilt body | ≤ 8 s p50 on the M2, ≤ 30 s p99 | UX-T13 |
| a checkpoint, 20k files, few changed | ≤ 150 ms p50 | UX-T15 |
| warm-up | background priority, at most half the cores; pauses while a turn runs | UX-T12 |
| update checks | one request a day | UX-T20 |

**Disk** (estimates, UNVERIFIED until UX-1 and UX-7): about 100 MB per installed version, two or three kept; the toolchain about 600 MB; `INIT_HOME` after install about 200 MB; the warm-up build about 2 GB while it runs (the slot is freed after), plus the dependency cache; checkpoints at most 5 GiB per folder. `/status` and doctor D8 show INIT's own use.

**Root size** (root.md §4.8; OW-UX4). Estimated additions, all on their own budget lines outside the core: `setup` 400, keys over the socket and the live test 150, local detection 120, `oauth` 400, per-turn checkpoints and rewind 450, digest and `lineage.recent` 120, `charter.set`, `config.set` and prefs 250, update scheduling 120, doctor 150, the repro gate 40. That is about 2,200 lines (UNVERIFIED), on top of the ≈ 7,500 budget. Downloads, signature checks and unpacking stay in `init-fetch`, outside the Root.

## 7. Invariants and security (spec §2.1)

| Invariant | How v1 keeps it | Test |
|---|---|---|
| keys never reach the agent zone | a key travels TUI → `owner.sock` → Root only; never in a `line` frame, the transcript or a log; the TUI zeroes its buffers | UX-T7, UX-T10 |
| a key reaches only the provider the owner confirmed | no cross-provider probing (§3.4.3) | UX-T7 |
| OAuth tokens stay in the Root | the Root runs PKCE, exchange, refresh and revocation; the TUI sees only the URL and the status | UX-T11 |
| the agent cannot change the owner's programs | the app directory and `~/.local/bin` are outside `INIT_HOME` and write-denied in every mode (proposed, §9 Q1); the toolchain lives in the app directory | UX-T5, sandbox self-test rows |
| updates are authentic | `init-fetch` verifies the Ed25519 signature with the embedded key before unpacking | UX-T2, UX-T20 |
| nothing runs in the background before the owner agrees | the installer starts nothing; the service is loaded only from screen 6 | UX-T3, UX-T13 |
| the money ceilings hold everywhere | setup key tests, plan-funded usage and `-p` runs all go through the egress meter; `--max-usd` only adds a stop | UX-T7, UX-T19 |
| a release never changes what the agent earned | an update does not touch the ledger, the lineage, the charter or sessions; it appends `release` | UX-T20 |
| self-change waits for a build this computer can reproduce | `root.repro_pending` until a `repro` record exists | UX-T12 |
| agent text is inert in notifications | notification text is fixed TUI or Root strings, never agent text | UX-T17 |
| no telemetry | only the provider and, for updates, the release host are contacted | review; UX-T3 (no other host in the fixture's log) |
| INIT never uses admin rights | no `sudo`, no elevation; admin-only fixes are printed for the owner | UX-T3, UX-T5 |

## 8. Test plan and work breakdown

Every test runs at $0 with `init-mockllm` and fixture servers. All of them use scratch homes and never touch the owner's `ROOT_HOME`, `INIT_HOME`, app directory, shell files or real service manager. Service tests use stub service managers, or `--service none`; loading a real LaunchAgent or systemd unit is tested only in the disposable environment (testing.md §3.1a). Every process a test starts is held by PID (CONVENTIONS §1).

### 8.1 Tests

| Id | Case | Expectation |
|---|---|---|
| UX-T1 | the bundle: contents against the manifest; the prebuilt body against a rebuild with G0's command on the release machine | every file listed and matching; the body's sha256 equal (the release gate) |
| UX-T2 | signatures: good; one flipped byte; the wrong key | accepted; rejected; rejected (`init-fetch` and the launcher) |
| UX-T3 | `install.sh` on macOS and Linux, scratch HOME, fixture host on 127.0.0.1 (`INIT_INSTALL_TEST=1`): fresh; re-run; foreign `init` present; checksum mismatch; interrupted download; PATH variants; under dash and busybox ash (shellcheck `-s sh`); no quarantine attribute on macOS | as §3.2.3; nothing outside the app directory and `~/.local/bin`; no process left running |
| UX-T4 | `install.ps1` on Windows CI (later, with lane P) | the same as UX-T3 |
| UX-T5 | the launcher: dispatch table; refuses binaries inside `INIT_HOME`; refuses root; PATH block added and removed idempotently on fixture rc files; `/sbin/init` shadowing with a fake PATH; Root auto-start against a stub `init-root` that records argv | as §3.3 |
| UX-T6 | setup mode: preflight with fakes; `setup.json` persistence and resume; no zone process ever spawned (broker count 0); exit after 30 min idle on the fake clock | as §3.4.1 |
| UX-T7 | `keys.set` against the mock: OK, 401, 402, timeout; nothing stored before a pass; save-untested; the key bytes absent from every Root log and alert (grep); `keys.list` returns no values; a key is never sent to a second provider | as §3.4.4, §3.4.5 |
| UX-T8 | `providers.detect` against fixture Ollama, LM Studio and llama.cpp servers (in test builds the fixed port list can point at scratch ports); the 300 ms timeout; a listener on any other port sees no connection | as §3.4.3 |
| UX-T9 | first-run snapshots: each screen in each state (detection results, key OK and failed, ChatGPT waiting, OK and failed, local server, limits per provider kind, autonomy, trust for git, non-git, home and refused folders, ready with each service choice, apply progress, each blocking preflight failure) at 80×24, 120×40 and 60×20; line-mode goldens | golden grids match |
| UX-T10 | the secret in the TUI, against the fixture server | the key appears in exactly one frame (`keys.set`), never in a `line`, in `--raw` output or in the transcript view; the paste guard triggers on each pattern and defaults to No |
| UX-T11 | ChatGPT sign-in against a mock OAuth/OIDC server on loopback (test mode): PKCE S256; wrong `state`; wrong `nonce` or `aud`; missing scope; ten concurrent requests during a refresh; logout | rejected where wrong; exactly one refresh in flight; the rotated token saved; revocation called before deletion; a 429 plan limit maps to `ux.plan_limit` |
| UX-T12 | warm-up and repro: matching blob; different blob with the probe passing; with the probe failing; a promotion before any `repro`; no toolchain; on battery (fake power source) | `verified`; `rebuilt_local`; evolution off, chat on; `root.repro_pending`; `evolve off`; paused |
| UX-T13 | **first-run end to end**: the installer from a fixture host → `init --plain` with scripted Enter ×6 and a `test-` key → setup-mode Root in test mode, mock upstream, `--service none` → the first prompt answered by the mock → `init uninstall` into a scratch Trash | charter values as chosen; the genesis tuple `built:"prebuilt"`; machine time ≤ 20 s; nothing left running |
| UX-T14 | daily snapshots: the status bar at 40, 80, 120 and 200 columns with every field state; the drop order; the `/usage`, `/permissions`, `/status`, `/level`, `/evolution` panels; the digest; the pickers | golden grids match |
| UX-T15 | checkpoints: a mock turn edits a file with `edit` and runs `bash -c 'echo x > a; rm b; mv c d'`; `/undo`; `/undo` again; `/rewind` files only, conversation only, both; a folder over the file limit; retention; timing | all three shell changes restored; each step back one turn; session-only notice over the limit; pruning by exact path; §6 timing |
| UX-T16 | command routing: each v1 command reaches its handler; a bare `/resume` never reaches the Root from the TUI; the deprecated alias notice; `/start` after `/stop` and after a panic | as §3.6.1 |
| UX-T17 | notifications: focus events; mechanism chosen per `TERM_PROGRAM` fixture; Root `turn_done` with no client, through a stub `notify.cmd` that records argv and stdin | no agent text inside any escape sequence; placeholders filled with fixed strings |
| UX-T18 | digest on a test ledger (root.md §3.16) with promotions, rollbacks, ceiling hits, a level change and a release, plus an egress test ledger | counts, subjects (sanitized) and spend by purpose as recorded |
| UX-T19 | headless: text, json and stream-json goldens; exit codes 0, 2, 3, 4, 5, 69; stdin with a prompt; `--max-usd`; `--session new` and `current` | as §3.6.11 |
| UX-T20 | update against a fixture release host: nothing new; newer and ready; bad signature; checksum mismatch; low disk; apply while busy; apply at idle with a TUI attached; rollback; a Homebrew-sourced install | as §3.7; the TUI reconnects; level and lineage unchanged; `release` appended |
| UX-T21 | uninstall in scratch homes: stub service; key files; lineage key; PATH block; `--keep-data`; a cross-volume path | service removed; key bytes gone from disk; lineage key in the scratch Trash; block removed; data kept with the flag; refusal with copy |
| UX-T22 | doctor: each check passing and failing on fixtures; `--fix` | each fix applied only after consent |
| UX-T23 | copy lint: every `ux.*` id rendered at 60 and 80 columns | each message ends with a command or a key |

`ci/local.sh` runs UX-T1, T2, T5–T12, T14, T16–T19, T22 and T23 in the fast tier; UX-T3, T13, T15, T20 and T21 in the full tier; UX-T4 joins with the Windows runners.

### 8.2 Work breakdown (proposed to roadmap.md)

| WP | Scope | Depends on | Agent-hours | Tests |
|---|---|---|---|---|
| UX-1 bundle | `release.sh` builds one bundle per target, including the dev-profile prebuilt body and components built with G0's command in a normalized slot path; `MANIFEST.json`, the Ed25519 signature, channel files; measures the sizes | — | 8 | T1, T2 |
| UX-2 installers | `install.sh` (POSIX) and `install.ps1` | UX-1's format (a fixture bundle meanwhile) | 8 | T3, T4 |
| UX-3 launcher | crate `init-cli`: dispatch, first-run detection, Root auto-start, PATH block, shadow check, exec rules, Windows spawn | — | 12 | T5 |
| UX-4 Root setup mode | `setup --serve`, the `setup.*` requests, `providers.detect`, `keys.set`/`list`/`remove` with the live test, `toolchain.set`, `install --prebuilt-body`, writing and loading the service (macOS first) | UX-1's manifest format | 16 | T6, T7, T8 |
| UX-5 TUI first run | the six screens, the masked key field, environment detection, line mode, `--yes`, the paste guard, theme detection | UX-4's protocol (a fixture server meanwhile) | 14 | T9, T10 |
| UX-6 ChatGPT sign-in | the Root's `oauth` module, egress injection, plan metering at $0, the TUI flow, logout and revocation | providers v2 (the Responses adapter), UX-4 | 14 | T11 |
| UX-7 warm-up and repro | the kernel's `warmup` job, `--remap-path-prefix`, the Root's `repro` record and promotion gate, power and disk pauses, `init-fetch toolchain`, `init setup evolution` | UX-1, UX-4 | 16 | T12 |
| UX-8 checkpoints | per-turn incremental checkpoints, `/undo` per turn, `rewind` (files) and the body's `/rewind-to` | the ADR-009 code (SBX-009) | 14 | T15 |
| UX-9 daily TUI | command set and routing, the `/resume` picker and `/start`, status bar v1, Shift+Tab, Esc Esc, `@` picker, image chips, the panels, notifications, the digest, `/config` | UX-10's data (fixture server meanwhile) | 20 | T14, T16, T17 |
| UX-10 daily protocol | Root: `digest`, `lineage.recent`, `workspace.trust`, `charter.set`, `config.set`, prefs, `by_purpose`, `notify.cmd` placeholders and `turn_done`, `/start`. Kernel: `/start`, config pickup. Body: `/sessions`, `/resume <id>`, `/rename`, `/rewind-to`, `/diff`, `/show`, `!`, `@` expansion, the `ctx` and `denied` fields | §9 settled | 18 | T16, T18 |
| UX-11 headless | `init -p` | UX-9 | 5 | T19 |
| UX-12 update | `init-fetch` (TLS, signature, unpack, retention), the Root's daily check and idle apply, rollback, repair, `/release-notes` | UX-1, UX-3 | 12 | T20 |
| UX-13 uninstall and doctor | the launcher's uninstall (service, secrets, PATH block, Trash); doctor in the launcher and the Root, `--fix` | UX-3, UX-4 | 10 | T21, T22 |
| UX-14 copy | one `copy` module with the `ux.*` ids, shared by the TUI and the launcher; snapshots of every state | UX-5, UX-9 | 4 | T23 |
| UX-15 first-run e2e | the scripted first run against a test-mode Root and the mock, from the installer to the first answer; the time-to-first-prompt measurement | UX-1…UX-5 | 6 | T13 |
| UX-16 Linux and Windows | paths, services, key storage, Trash, notifications and terminals for UX-2, 3, 4, 12 and 13 | cross_platform.md; lane P's sandboxes | 24 | the per-OS runs of T3–T13, T21 |

**Total ≈ 220 agent-hours.** Critical path: UX-1 → UX-4 → UX-5 → UX-15, about 44 agent-hours. Parallel lanes: A (UX-1, UX-2, UX-12), B (UX-3, UX-13), C (UX-4, UX-6), D (UX-5, UX-9, UX-11, UX-14), E (UX-8, UX-10), F (UX-7), G (UX-16, after lane P). UX-3, UX-8 and UX-10 can start at once; the rest start against fixture servers before their protocol lands.

## 9. Open questions

**Cross-doc** (each proposal is specified in §3 and §4)
1. **→ root/root.md** (§3.2, §3.12, §3.17, §4.2, §4.3, §4.8, §9.3): setup mode and the setup requests; `install --prebuilt-body`; `keys.set`, `keys.list`, `keys.remove`; `providers.detect` (loopback probes from the Root); `auth.*`; `toolchain.set`; `doctor`; `update.*` and running `init-fetch`; `digest`; `lineage.recent`; `workspace.trust` and `status.clients`; `charter.set`; `config.set` (the Root writes `INIT_HOME/kernel/config.toml`, which R-F1 allows); prefs; per-turn checkpoints and `rewind`; `/start` and the `/resume` alias; the `repro` and `release` lineage kinds and `root.repro_pending`; the app directory and `INIT_BIN_DIR` as `root_path` refusals and write-denied in every mode (closes §9.3 Q5's `INIT_BIN_DIR` item); `notify.cmd` placeholders and `turn_done`; new budget lines in §4.8. **Recommended: yes**, in UX-4, UX-6, UX-8, UX-10 and UX-12.
2. **→ root/egress.md** (§3.2, §4.2): keys arriving over `owner.sock` and the live-test path; the storage backends (file by default, DPAPI on Windows, an opt-in OS store); OAuth tokens and bearer injection; plan-funded usage booked at $0; `by_purpose` in `egress.stats`; purpose `setup`. **Recommended: yes.**
3. **→ tui.md** (T-2, T-3, §3.7, §3.8, §3.9, §3.10, §4.1): T-2 allows one transient secret under §3.4.5's rules; T-3 adds the OS URL opener for an `https://auth.openai.com/` URL; the environment read list adds the catalog's key names, `WT_SESSION` and `TERM_PROGRAM`; the TUI-handled commands; the key map additions; status bar v1; notifications; `read_dir` and one `metadata` call in the cwd for `@` and image chips; `--first-run`, `--login` and `-p`; the ASCII glyph set. Ctrl+G and clipboard images stay out unless T-2 and T-3 are relaxed further: recommended to wait for v1.1.
4. **→ kernel/supervise.md** (§3.6.2, §4.6): `/start`; `--remap-path-prefix` for the slot and `CARGO_HOME` in the build command; the `warmup` job and `repro.report`; background priority for warm-up builds; picking up `config.set` at the next idle point. **Recommended: yes.**
5. **→ body/log-context.md, body/loop.md, body/tools.md:** the body commands of §4.4; the `turn_end.ctx` and `tool_end.denied` fields; expanding `@path` and `@image`; session titles. **Recommended: yes.**
6. **→ providers v2 (not yet written):** the catalog fields this doc reads (`env` names, the `test` recipe, `ref_task_usd`, `tools`, `vision`, prices), the local-loopback egress class (production C11 rejects loopback today), the `openai-chatgpt` provider and its Responses constraints.
7. **→ ADR-009:** a checkpoint before every turn extends Decision 4; per-folder trust refines Decision 2 (the first write in a new folder needs a remembered yes). Needs an addendum, or ADR-010.
8. **→ release.md, RELEASE_PLAN.md:** the bundle, signing and channels; `scripts/install.sh` becomes the from-source path.
9. **→ CONVENTIONS.md:** a §4 registry row for this doc (the install path, the launcher, the first run, update, uninstall, doctor, the `ux.*` copy); the app directory in §3; OW-UX1…OW-UX6 in §6.

**Owner** (each has a recommended default that this doc uses; `PENDING OWNER (CONVENTIONS §6 <id>)`)
- **OW-UX1** the release host and making the repo public (V1_PLAN phase F). Recommended: GitHub Releases on the repo once public.
- **OW-UX2** code signing: Apple Developer ID ($99/yr) and Windows signing (paid). Recommended: decide before v1.0 publicity. The curl path works without it; it is needed for quiet Homebrew and browser installs and before the OS key store can be the default.
- **OW-UX3** the TUI forwarding keys to the Root. Taken provisionally (§3.4.5).
- **OW-UX4** Root size: about 2,200 more lines on non-core budget lines (§6). Recommended: accept, re-measured at release under the same rule.
- **OW-UX5** the screen 4 default for new users. Recommended: keep L4, the charter template's value (CH-1), since the agent starts at L0 and earns each level.
- **OW-UX6** updates apply automatically at idle on `stable`. Recommended: yes, with `notify` and `off` in `/config`.

**Internal**
- Whether OpenAI's sign-in accepts a loopback port other than 1455 (UNVERIFIED).
- The bundle size, the toolchain download size and `init-root install --prebuilt-body` time: measured in UX-1, UX-7 and UX-15.
- Bit-for-bit reproducibility of the body across two Macs with different Command Line Tools: measured in UX-7; §3.5.3 already handles a mismatch.
- The exact Ubuntu 24.04+ user-namespace fix for bwrap, and the Windows owner-socket peer check: cross_platform.md and lane P.

## 10. Spec deltas

| Source | This doc | Why |
|---|---|---|
| README, release.md: install = clone, Command Line Tools, Rust 1.97, `scripts/install.sh` (source build and shell wizard), then `init-root keys set`, then `init` | one hosted command per OS installs a signed prebuilt bundle; every question moves into `init`'s first run; `scripts/install.sh` stays for maintainers | the owner's mandate; the owner's own first run (§1) |
| root.md §4.3 step 8: the kernel builds the body cold at install | the bundle's prebuilt body is installed; the cold build becomes a background warm-up; self-change waits for the repro check | time to first prompt (G2) |
| root/egress.md §3.2: keys only through `init-root keys set` on a TTY | also `keys.set` over `owner.sock` from the TUI, tested live before storing; `--stdin` for scripts | one first-run flow (OW-UX3, provisional) |
| tui.md T-2, T-3, §4.1 environment list | one transient secret; the URL opener; more environment names read | §3.4.5, §3.4.6, §3.11 |
| tui.md §3.8: the TUI sends every line unchanged except `/panic` | the TUI handles a fixed set of panel and picker commands itself | §3.6.1 |
| root.md §4.2, kernel/supervise.md §4.6: `/resume` restarts a stopped or panicked system | `/start` does that; `/resume` reopens past sessions (a deprecated alias for one minor release) | every major tool uses `/resume` for sessions (install_daily_ux.md §3) |
| ADR-009 Decision 4: one snapshot per session, `/undo` restores it | a checkpoint before every turn; `/undo` steps back one turn; `/rewind` | shell-inclusive undo per turn |
| ADR-009 Decision 2: any valid cwd is writable at once | the first write in a new folder needs the owner's remembered yes (default yes in git repos) | the trust screen the owner asked for |
| `scripts/init` (bash launcher) | the `init` binary (`init-cli`), plus the `init-agent` alias | Windows; subcommands; the Linux name clash |
| `scripts/uninstall.sh`: provider keys go to the Trash with `ROOT_HOME` | provider keys and tokens are deleted (tokens revoked first); the rest goes to the Trash as before; plus the service, the PATH block and the app directory | a secret in the Trash is a leak |

## 11. References

Checked 2026-10-20, through the research notes listed (read-only web research done that day) and the repo itself.
- Repo: V1_PLAN.md; README.md; RELEASE_PLAN.md; `scripts/install.sh`, `scripts/uninstall.sh`, `scripts/init`; docs/tui.md; docs/root/root.md (§3.2, §3.12–§3.17, §4.2, §4.3, §9.3); docs/root/egress.md (§3.2); docs/kernel/supervise.md (§3.6.2); docs/body/log-context.md (§4.5); docs/adr/009-codex-style-permissions.md; docs/release.md; `target/dist/v0.1.0/` sizes (`du`, 86 MB; `init-body` 22.7 MB release, 37 MB dev-profile in `~/.init/bin`).
- research_notes/INIT v1 providers UX vision/install_daily_ux.md: §1 (installers, channels, doctor, uninstall), §2 (first-run sequences, trust prompts, key storage), §3 (commands, keys, checkpoints, headless), §5 (complaints: opaque limits, permission fatigue), §6 (the recommended spec this doc builds on).
- research_notes/INIT v1 providers UX vision/subscription_plans.md: Q0 (the verdict matrix), Q1 (Sign in with ChatGPT: PKCE, loopback `127.0.0.1:1455`, scopes, token lifetimes, `store:false`/`stream:true`, the 429 error), Q11 (credential broker pattern).
- research_notes/INIT v1 providers UX vision/proxies_gateways_local.md: local servers' default ports, endpoints and fingerprinting (Ollama 11434, LM Studio 1234, llama.cpp 9931/8080, MLX-LM 8080).
- research_notes/INIT v1 providers UX vision/llm_api_providers.md: provider key environment variable names.
- Claude Code setup (installer, channels, `doctor`, uninstall) — https://code.claude.com/docs/en/setup ; checkpointing — https://code.claude.com/docs/en/checkpointing ; interactive mode (keys) — https://code.claude.com/docs/en/interactive-mode (via install_daily_ux.md).
- Codex approvals and security (trust and permission presets) — https://learn.chatgpt.com/docs/agent-approvals-security (via install_daily_ux.md).
- Sign in with ChatGPT for open-source apps — https://developers.openai.com/siwc/token-sharing-open-source/sign-in (via subscription_plans.md).
- Rust release channel manifests — https://static.rust-lang.org/dist/channel-rust-1.97.1.toml (format as used by rustup; UNVERIFIED for this version until UX-7 fetches it).
- `std::hint::black_box` (stable since 1.66) — https://doc.rust-lang.org/std/hint/fn.black_box.html ; crossterm 0.29.0 focus events — https://docs.rs/crossterm/0.29.0/crossterm/event/ (UNVERIFIED here; UX-9 checks).
- freedesktop Trash specification — https://specifications.freedesktop.org/trash-spec/ ; minisign signature format — https://jedisct1.github.io/minisign/ (UNVERIFIED here; UX-1 checks).
