#!/bin/bash
# Install INIT on this Mac or Linux machine with one command (root/root.md §4.3; RELEASE_PLAN.md "What v0.1.0 means" item 1).
# Two modes: from a source checkout (builds with release.sh), or from an unpacked release bundle (scripts/bundle.sh;
# scripts/get.sh runs it there), detected by SHA256SUMS and body/BODY.json next to scripts/. The bundle mode installs
# the prebuilt body (hash-equal to the maintainer's build). get.sh installs the Rust toolchain first (rustup 1.97.1 +
# wasm32-wasip2, unless --no-rust), which this script records for self-evolution T1/T2; without a toolchain
# self-evolution is T0 only.
#
#   scripts/install.sh [--yes] [--no-service] [--rebuild | --reuse] [--with-tests] [--allow-dirty]
#                      [--disk-floor GIB] [--day-usd X] [--total-usd X] [--max-level N] [--access full|workspace]
#                      [--purpose TEXT] [--charter FILE]
#   scripts/install.sh --upgrade [--yes] [--probe-timeout S]     upgrade the installed INIT to this release
#   scripts/install.sh --print-apparmor                          print the one-time sudo commands for Ubuntu's AppArmor
#
#   --yes          install with the settings shown, ask nothing (non-interactive)
#   --no-service   do not write or load the service (launchd on macOS, a systemd user unit on Linux); you run
#                  `init-root start` yourself
#   --rebuild      run scripts/release.sh even when target/dist/v<version>/ is fresh
#   --reuse        use target/dist/v<version>/ as it is (its files verified against MANIFEST.json), never rebuild
#   --with-tests   let release.sh run its release-mode test guard (root + seed); slower
#   --allow-dirty  allow building from a tree with uncommitted kernel/ root/ seed/ wit/ changes
#   The installer asks one question: it prints the settings it will write into the charter and asks "Install with these
#   settings? [Y/n]". Each setting has a flag, and each can be changed later in ROOT_HOME/charter.toml (then
#   `init-root charter check --apply`). The provider, the model and the key are chosen on the first run of init-agent.
#   --disk-floor N the free-space floor in GiB INIT keeps (default 20, the owner's rule)
#   --day-usd X    the daily spending INIT shows its spend against (default 5); tracked, not a cap
#   --total-usd X  the total spending reference (default 25, at least the daily one); tracked, not a cap. The dollar
#                  limit the Root enforces is a goal's budget (/goal ... usd=); set a hard limit at your provider too
#   --max-level N  the highest autonomy level INIT may earn, 0-4 (default 4; it starts at L0)
#   --access M     full (default, ADR-012: anything you can, except INIT's Root files) or workspace (ADR-009: writes
#                  only the folder init-agent starts in)
#   --purpose TEXT what INIT is for, one line (default: a personal agent that improves itself within your limits)
#   --charter FILE use this charter instead (it is still validated; the flags above do not apply)
#   --upgrade      INIT is installed: upgrade it to this release instead (docs/guide.md "Upgrade"). It keeps the
#                  evolved body, the extensions, the sessions, the lineage, the keys and the charter byte for byte, and
#                  replaces the Root, the kernel and the launchers: stop the Root, `init-root upgrade --release` (this
#                  release's Root: version and body checks, a snapshot, the replace), the new launchers, a health probe,
#                  then commit, or roll back to the old binaries if the probe fails. `init-agent upgrade` and
#                  `get.sh --upgrade` run this from a verified copy of the release under ROOT_HOME/cache/upgrade/.
#                  The agent may have written the owner's PATH directories, the working directory and TMPDIR (full
#                  access), so an upgrade from a bundle runs the system tools (PATH /usr/bin:/bin:/usr/sbin:/sbin) from
#                  /, keeps its scratch files under ROOT_HOME/cache/, and keeps the Rust toolchain install.json records
#                  (it runs no rustup).
#   --probe-timeout S  how long the health probe waits for the upgraded Root, kernel and body (default 120)
#   --apparmor     Linux, when AppArmor restricts unprivileged user namespaces (Ubuntu 24.04 and later,
#                  kernel.apparmor_restrict_unprivileged_userns = 1): show the one-time commands below, ask, and run them
#                  with sudo. Without this flag the installer only prints them and stops. They copy util-linux unshare
#                  and bubblewrap, owned by root, to /usr/local/lib/init-agent/ and load an AppArmor profile that lets
#                  only those two copies create a user namespace (docs/platforms.md §3.4.6). INIT never asks you to
#                  lift the restriction for every program (the sysctl), and never runs sudo without --apparmor and a yes.
#   --print-apparmor  print those commands and exit
#   (bundle mode ignores --rebuild, --reuse, --with-tests and --allow-dirty: the bundle is the release)
#
# Paths (environment): INIT_ROOT_HOME (default ~/.init-root), INIT_HOME (default ~/.init), INIT_BIN_DIR (where the
# `init-agent` command goes; default ~/.local/bin). Homes are at most 64 bytes long (the Root's sockets must fit sun_path).
# INIT_BIN_DIR is refused when it is relative or is (or resolves to) /sbin, /usr/sbin, /bin or /usr/bin: INIT never
# writes into a system bin directory (nagisanzenin/INIT-Agent#1).
#
# Steps: preflight → release binaries (release.sh, or reuse a fresh target/dist/v<version>/) → the charter (shown, one Y/n) →
# `init-root install --release … --charter … --seed-source <release>/seed/seed-v<version>.bundle` (the Root runs the
# kernel in install mode: it imports the seed, builds the body cold, and the Root forms the genesis tuple; WP21b,
# FIX4) → kernel/config.toml (model, prices, reference budgets) → the `init-agent` command → the service (launchd, or systemd --user; loaded) →
# the next steps. The command was `init` before v0.4.1 (renamed: on Linux `init` shadows /sbin/init, issue #1). No file
# named `init` is installed, and INIT's own old one (the pre-v0.4.1 launcher, or v0.4.1's macOS shim) is removed. This
# script never reads, writes or asks for a provider key: the first run of init-agent asks for it (or init-root keys set).
#
# Test hook (never for a real install): --mock-upstream 127.0.0.1:PORT writes a test-mode charter whose provider
# egress goes to a loopback init-mockllm (as scripts/slice1/s1.sh mock does). It requires --no-service.
set -euo pipefail
umask 077

YES=0; NO_SERVICE=0; REBUILD=0; REUSE=0; WITH_TESTS=0; ALLOW_DIRTY=0; FLOOR=""; CHARTER_IN=""; MOCK=""; UPGRADE=0; PROBE_S=120
APPARMOR=0; PRINT_APPARMOR=0
DAY=5; TOTAL=25; LEVEL=4; ACCESS=full; PURPOSE="My personal agent on this computer: it helps me with my work and improves itself, within my limits."
while [ $# -gt 0 ]; do
  case "$1" in
    --yes|-y) YES=1; shift ;;
    --no-service) NO_SERVICE=1; shift ;;
    --rebuild) REBUILD=1; shift ;;
    --reuse) REUSE=1; shift ;;
    --with-tests) WITH_TESTS=1; shift ;;
    --allow-dirty) ALLOW_DIRTY=1; shift ;;
    --disk-floor) FLOOR="$2"; shift 2 ;;
    --charter) CHARTER_IN="$2"; shift 2 ;;
    --day-usd) DAY="$2"; shift 2 ;;
    --total-usd) TOTAL="$2"; shift 2 ;;
    --max-level) LEVEL="$2"; shift 2 ;;
    --access) ACCESS="$2"; shift 2 ;;
    --purpose) PURPOSE="$2"; shift 2 ;;
    --mock-upstream) MOCK="$2"; shift 2 ;;
    --upgrade) UPGRADE=1; shift ;;
    --probe-timeout) PROBE_S="$2"; shift 2 ;;
    --apparmor) APPARMOR=1; shift ;;
    --print-apparmor) PRINT_APPARMOR=1; shift ;;
    -h|--help) sed -n '2,65p' "$0"; exit 0 ;;
    *) echo "install.sh: unknown argument $1 (see --help)" >&2; exit 64 ;;
  esac
done
# This script's directory, with builtins only (nothing on PATH has run yet), and whether it is an unpacked bundle
case "$0" in */*) HERE="${0%/*}" ;; *) HERE=. ;; esac
REPO="$(cd "$HERE/.." && pwd -P)"
BUNDLE=0
if [ -f "$REPO/SHA256SUMS" ] && [ -f "$REPO/body/BODY.json" ] && [ -f "$REPO/MANIFEST.json" ]; then BUNDLE=1; fi
# An upgrade from a bundle runs with INIT installed, and with full access its agent can write the owner's PATH
# directories and the working directory (ADR-012). The upgrade replaces the Root, so nothing it runs comes from them:
# the system tools, from /. (An upgrade from a source checkout builds that checkout with your cargo: it trusts the
# checkout and your PATH as they are.)
if [ $UPGRADE = 1 ] && [ $BUNDLE = 1 ]; then
  PATH=/usr/bin:/bin:/usr/sbin:/sbin; export PATH
  cd /
fi

say() { printf '%s\n' "$*"; }
step() { printf '\n==> %s\n' "$*"; }
die() { printf '\ninstall.sh: %s\n' "$*" >&2; exit 1; }
free_mb() { df -Pm "${HOME:-/}" | awk 'NR==2{print $4}'; }
OS="$(uname -s)"
# systemctl --user reaches the user's service manager through XDG_RUNTIME_DIR (or DBUS_SESSION_BUS_ADDRESS). A run
# under `env -i HOME="$HOME" …` (how the v0.5.3 release notes have an upgrade drop a planted INIT_DIST_URL) has
# neither, and every systemctl --user call failed: the upgrade stopped the agent, then waited 60 s for a Root it could
# not stop (the release review of v0.5.3). /run/user/<uid> is where systemd-logind puts it.
if [ "$OS" = Linux ]; then XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"; export XDG_RUNTIME_DIR; fi
# sha256 tools: shasum on macOS, coreutils sha256sum on Linux (same `<hash>  <file>` format)
if [ "$OS" = Darwin ]; then SHA_CHECK=(/usr/bin/shasum -a 256 -c --quiet); else SHA_CHECK=(sha256sum -c --quiet); fi
gt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 > b + 0) }'; } # decimal a > b, without bc
ge() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 >= b + 0) }'; }

# AppArmor on Ubuntu 24.04 and later (docs/platforms.md §3.4.6): the zone starts with util-linux unshare, then bubblewrap
# inside it, and both create a user namespace. Ubuntu's own profiles do not cover that nesting (REL-050: with
# bwrap-userns-restrict and unshare-userns-restrict loaded, bwrap inside unshare is still refused). So the owner copies
# both binaries, owned by root, to $HELPERS and loads a profile that grants only those two paths `userns`, Ubuntu's
# documented way for an application that needs user namespaces. The Root uses the copies only if nobody but root can
# change them (init-sandbox-profiles entry.rs); inside the zone, bwrap --disable-userns and seccomp still refuse every
# namespace. The copies are mode 0750 with your group, so other users cannot run them.
HELPERS=/usr/local/lib/init-agent
AA_PROFILE=/etc/apparmor.d/init-agent
apparmor_commands() { # the one-time commands, printed verbatim (one per line); run only with --apparmor and a yes
  local g; g="$(id -g)"
  cat <<EOF
sudo install -d -o root -g root -m 0755 $HELPERS
sudo install -o root -g $g -m 0750 /usr/bin/unshare $HELPERS/unshare
sudo install -o root -g $g -m 0750 /usr/bin/bwrap $HELPERS/bwrap
printf '%s\n' 'abi <abi/4.0>,' 'include <tunables/global>' 'profile init-agent-unshare $HELPERS/unshare flags=(unconfined) {' '  userns,' '}' 'profile init-agent-bwrap $HELPERS/bwrap flags=(unconfined) {' '  userns,' '}' | sudo tee $AA_PROFILE >/dev/null
sudo apparmor_parser -r $AA_PROFILE
EOF
}
if [ $PRINT_APPARMOR = 1 ]; then apparmor_commands; exit 0; fi

T_START=$(date +%s)
PW_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)' 2>/dev/null || true)"
HOME="${HOME:-$PW_HOME}"
# rustup and cargo keep their state in the passwd home, even when HOME is elsewhere (a test home)
export RUSTUP_HOME="${RUSTUP_HOME:-$PW_HOME/.rustup}" CARGO_HOME="${CARGO_HOME:-$PW_HOME/.cargo}"
RH="${INIT_ROOT_HOME:-$HOME/.init-root}"
IH="${INIT_HOME:-$HOME/.init}"
BIN_DIR="${INIT_BIN_DIR:-$HOME/.local/bin}"
# ROOT-MSG (v0.5.1): `init-root <args>` as you type it in a new terminal, the form the Root prints (util::root_cmd).
# init-root is not on PATH (only init-agent is), so a bare `init-root …` is "command not found"; a ROOT_HOME other than
# ~/.init-root needs INIT_ROOT_HOME= in front, because init-root finds its home there, else in ~/.init-root.
# tests/install/test_printed_commands.py checks that no command this script prints is a bare one.
shq() { case "$1" in ''|*[!A-Za-z0-9/._+,:@%=-]*) printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")" ;; *) printf '%s' "$1" ;; esac; }
# rh_env: "INIT_ROOT_HOME=<ROOT_HOME> " when a new terminal would not find this ROOT_HOME (not ~/.init-root), else ""
rh_env() { [ -n "$PW_HOME" ] && [ "$RH" = "$PW_HOME/.init-root" ] || printf 'INIT_ROOT_HOME=%s ' "$(shq "$RH")"; }
rcmd() { printf '%s%s' "$(rh_env)" "$(shq "$RH/bin/init-root")"; [ $# = 0 ] || printf ' %s' "$*"; }
# v0.5.3 (the post-release review of v0.5.2): `init-agent` is printed with its path too (get.sh's tilde and agent). A
# bare `init-agent …` is "command not found" on a fresh Mac, whose PATH lacks ~/.local/bin. Under HOME the path is
# written `~/…` (the shell expands it there), else as shq prints it.
tilde() { local h="${HOME%/}"; if [ -n "$h" ] && [[ "$1" == "$h"/?* ]]; then printf '~/%s' "$(shq "${1#"$h"/}")"; else shq "$1"; fi; }
agent() { # the init-agent command of the INIT in ROOT_HOME: in the bin_dir its install.json records, else BIN_DIR
  local b
  b="$(/usr/bin/python3 -I -c 'import json,sys
try: print(json.load(open(sys.argv[1])).get("bin_dir") or "")
except Exception: pass' "$RH/install.json" 2>/dev/null || true)"
  [ -n "$b" ] && [ -x "$b/init-agent" ] || b="$BIN_DIR"
  tilde "$b/init-agent"
}
# v0.5.3: the PATH hint names the startup file of the owner's shell ($SHELL); it always said ~/.zshrc, which a Linux
# bash user never reads. zsh: ~/.zshrc (in ZDOTDIR when set). bash: ~/.bashrc on Linux; on macOS, whose terminals open
# login shells, the file a login bash reads (~/.bash_profile, else ~/.bash_login, else ~/.profile; a new
# ~/.bash_profile would hide an existing ~/.profile). fish: fish_add_path. Any other shell: ~/.profile.
path_add() { # <dir>: the command that puts <dir> on PATH in the owner's new terminals
  local d="$1" h="${HOME%/}" rc f
  [ -n "$h" ] && [[ "$d" == "$h"/?* ]] && d="\$HOME/${d#"$h"/}"
  case "${SHELL##*/}" in
    fish) printf 'fish_add_path %s' "$(tilde "$1")"; return ;;
    zsh) rc="$(tilde "${ZDOTDIR:-$HOME}/.zshrc")" ;;
    bash)
      if [ "$OS" = Darwin ]; then
        rc="$HOME/.bash_profile"
        for f in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do [ -e "$f" ] && { rc="$f"; break; }; done
        rc="$(tilde "$rc")"
      else
        rc="$(tilde "$HOME/.bashrc")"
      fi ;;
    *) rc="$(tilde "$HOME/.profile")" ;;
  esac
  printf 'echo %s >> %s' "$(shq "export PATH=\"$d:\$PATH\"")" "$rc"
}
# The upgrade to the latest release with its get.sh and the system's curl and sh: what an install of 0.4.1 or 0.5.0 runs
# (0.5.0's own `init-agent upgrade` runs 0.5.0's get.sh, without 0.5.1's fixes; docs/guide.md "Upgrade"; get.sh's up_pipe)
up_pipe() {
  printf '/usr/bin/curl -q -fsSL %s | %s/bin/sh -s -- --upgrade' "$(shq "${INIT_DIST_URL:-https://github.com/nagisanzenin/INIT-Agent/releases/latest/download}/get.sh")" "$(rh_env)"
}
if [ $BUNDLE = 1 ]; then # an unpacked release bundle: the release dir is this directory
  VERSION="$(sed -n 's/^ "version": "\(.*\)",*$/\1/p' "$REPO/MANIFEST.json" | head -1)"
  DIST="$REPO"
else
  VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/root/Cargo.toml" | head -1)"
  DIST="$REPO/target/dist/v$VERSION"
fi

# ---------------------------------------------------------------- 1. preflight
step "1/7 preflight"
# Issue #1: INIT's commands never go into a system bin directory, even when INIT_BIN_DIR names one, directly or through
# a symlink (/bin -> /usr/bin on merged-/usr systems, or a link of your own): `-ef` compares device and inode, so links,
# `..`, doubled or trailing slashes and case-insensitive names all match. Checked before anything is written.
case "$BIN_DIR" in /*) ;; *) die "INIT_BIN_DIR ($BIN_DIR) is not an absolute path" ;; esac
for d in /sbin /usr/sbin /bin /usr/bin; do
  if [ "$BIN_DIR" -ef "$d" ] || [ "${BIN_DIR%/}" = "$d" ]; then
    die "refusing INIT_BIN_DIR=$BIN_DIR: it is the system directory $d, and INIT never writes into /sbin, /usr/sbin, /bin or /usr/bin. Use a directory of your own, such as ~/.local/bin (the default)."
  fi
done
case "$OS" in
  Darwin)
    MACOS="$(sw_vers -productVersion)"
    [ "${MACOS%%.*}" -ge 14 ] || die "macOS 14 or newer is needed (this is $MACOS)"
    [ -x /usr/bin/sandbox-exec ] || die "/usr/bin/sandbox-exec is missing"
    [ -x /usr/bin/python3 ] || die "/usr/bin/python3 is missing: install the Command Line Tools (xcode-select --install)"
    PLATFORM="macOS $MACOS" ;;
  Linux)
    # The zone (docs/platforms.md §3.4): bubblewrap >= 0.13.0 at /usr/bin/bwrap (fd-pinned binds), util-linux unshare,
    # unprivileged user namespaces. The Root checks the same at install; these say how to fix it first.
    [ -x /usr/bin/python3 ] || die "/usr/bin/python3 is missing: install python3 (apt install python3 / dnf install python3)"
    [ -x /usr/bin/unshare ] || die "/usr/bin/unshare is missing: install util-linux"
    [ -x /usr/bin/bwrap ] || die "/usr/bin/bwrap is missing: INIT needs bubblewrap 0.13.0 or newer (see docs/platforms.md, Linux install)"
    bwrap_new_enough() { # <bwrap>
      BWV="$("$1" --version 2>/dev/null | awk '{print $NF}')"
      /usr/bin/python3 -I -c 'import sys; v=tuple(int(x) for x in sys.argv[1].split(".")[:3]); sys.exit(0 if v>=(0,13,0) else 1)' "${BWV:-0}" 2>/dev/null \
        || die "bubblewrap $BWV at $1 is too old: INIT needs 0.13.0 or newer for its fd-pinned mounts (see docs/platforms.md, Linux install)"
    }
    bwrap_new_enough /usr/bin/bwrap
    # The zone's entry binaries (init-sandbox-profiles entry.rs): the root-owned copies in $HELPERS when present, else
    # /usr/bin's. A copy that anyone but root can change is refused, never skipped.
    root_only() { # <path>: its canonical file and every directory above it owned by root, writable by root alone
      local p m; p="$(readlink -f -- "$1")" && [ -f "$p" ] || return 1
      while :; do
        set -- $(stat -c '%u %a' -- "$p") && [ "$1" = 0 ] || return 1
        m=$((8#$2)); [ $((m & 8#22)) = 0 ] || return 1
        [ "$p" = / ] && return 0
        p="$(dirname -- "$p")"
      done
    }
    entries() {
      UNSHARE=/usr/bin/unshare; BWRAP=/usr/bin/bwrap
      if [ -e "$HELPERS/unshare" ] || [ -L "$HELPERS/unshare" ]; then
        root_only "$HELPERS/unshare" || die "$HELPERS/unshare is not owned by root and writable by root alone (or a directory above it is not): INIT will not start its sandbox through it. Remove it, or run the commands of install.sh --print-apparmor again"
        UNSHARE="$(readlink -f -- "$HELPERS/unshare")"
      fi
      if [ -e "$HELPERS/bwrap" ] || [ -L "$HELPERS/bwrap" ]; then
        root_only "$HELPERS/bwrap" || die "$HELPERS/bwrap is not owned by root and writable by root alone (or a directory above it is not): INIT will not start its sandbox through it. Remove it, or run the commands of install.sh --print-apparmor again"
        BWRAP="$(readlink -f -- "$HELPERS/bwrap")"; bwrap_new_enough "$BWRAP"
      fi
    }
    # The namespaces the zone needs, as it creates them: bwrap's user and pid namespaces with /proc; unshare's user and
    # network namespaces; and bwrap started inside unshare's namespace (the zone's nesting).
    ns_bwrap() { "$BWRAP" --unshare-user --unshare-pid --die-with-parent --ro-bind / / --proc /proc --dev /dev /bin/true 2>/dev/null; }
    ns_unshare() { "$UNSHARE" --user --map-root-user --net /bin/true 2>/dev/null; }
    ns_nested() { "$UNSHARE" --user --map-root-user --net -- "$BWRAP" --die-with-parent --new-session --unshare-user --unshare-pid \
      --unshare-ipc --unshare-uts --disable-userns --cap-drop ALL --ro-bind / / --proc /proc --dev /dev /bin/true 2>/dev/null; }
    entries
    if ! { ns_bwrap && ns_unshare && ns_nested; } \
      && [ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null)" = 1 ]; then
      if [ "$BWRAP" = "$HELPERS/bwrap" ] && [ "$UNSHARE" = "$HELPERS/unshare" ]; then
        die "INIT's copies in $HELPERS are there, but they still cannot create user namespaces: is the AppArmor profile loaded? Run: sudo apparmor_parser -r $AA_PROFILE (the commands of install.sh --print-apparmor; docs/platforms.md, Linux install)"
      fi
      say ""
      say "This Linux restricts unprivileged user namespaces through AppArmor (kernel.apparmor_restrict_unprivileged_userns = 1),"
      say "and INIT's sandbox needs them. Run these commands once. They copy unshare and bubblewrap, owned by root, to"
      say "$HELPERS and load an AppArmor profile that lets only those two copies create a user namespace"
      say "(docs/platforms.md, Linux install). INIT does not ask you to lift the restriction for every program."
      say ""
      apparmor_commands
      say ""
      [ $APPARMOR = 1 ] || die "run the commands above, then the installer again (or run the installer with --apparmor: it asks first, then runs them with sudo)"
      [ -r /dev/tty ] && [ -w /dev/tty ] || die "--apparmor asks before it runs sudo, and there is no terminal to ask on: run the commands above yourself"
      printf 'Run these commands now with sudo? [Y/n] ' >/dev/tty
      IFS= read -r AA_ANSWER </dev/tty || AA_ANSWER=n
      case "$AA_ANSWER" in ""|y|Y|yes|Yes|YES) ;; *) die "nothing was changed; run the commands above yourself, then the installer again" ;; esac
      apparmor_commands | /bin/bash -e || die "the commands failed (see above)"
      entries
    fi
    ns_bwrap \
      || die "bubblewrap cannot create user and pid namespaces here (unprivileged user namespaces disabled, an AppArmor/SELinux rule, or a container without them); see docs/platforms.md, Linux install"
    ns_unshare \
      || die "unshare cannot create a user and network namespace here (needed to keep the zone off the network); see docs/platforms.md, Linux install"
    ns_nested \
      || die "bubblewrap cannot start inside unshare's namespace here (the zone's nesting); see docs/platforms.md, Linux install"
    if [ $NO_SERVICE = 0 ] && [ $UPGRADE = 0 ] && ! systemctl --user show-environment >/dev/null 2>&1; then
      die "no systemd user session here (systemctl --user does not answer), so the service cannot be installed: pass --no-service and start the Root yourself after the install, in a terminal of its own ($(rcmd start --foreground)), or log in through a systemd session"
    fi
    DISTRO="$( (. /etc/os-release 2>/dev/null && printf '%s' "${PRETTY_NAME:-}") || true)" # a subshell: os-release sets VERSION
    PLATFORM="Linux $DISTRO $(uname -r) $(uname -m) · bwrap $BWV" ;;
  *) die "v$VERSION installs on macOS and Linux only (this is $OS)" ;;
esac
[ -n "$PW_HOME" ] || die "cannot read your home directory from the passwd database"
GITV="$(git --version | awk '{print $3}')"
/usr/bin/python3 -I -c 'import sys; v=tuple(int(x) for x in sys.argv[1].split(".")[:3]); sys.exit(0 if v>=(2,39,4) else 1)' "$GITV" \
  || die "git 2.39.4 or newer is needed (found $GITV)"
RUSTUP_CMD="curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.97.1 --target wasm32-wasip2 --component clippy"
if [ $UPGRADE = 1 ] && [ $BUNDLE = 1 ]; then
  # the upgrade keeps the toolchain install.json records (init-root toolchain --set changes it); a rustup found on
  # PATH would run as you, and the agent can write the directories it lives in
  TC=kept; TCV="the Rust toolchain install.json records (kept)"
elif [ $BUNDLE = 1 ]; then
  # get.sh installs Rust 1.97.1 with wasm32-wasip2 before this runs (unless --no-rust). A rustup on PATH with 1.97.x and
  # wasm32-wasip2 is recorded for self-evolution T1/T2; anything else installs without a toolchain (T0: skills, prompts,
  # extension source).
  TC=none; TCV="no Rust toolchain (self-evolution T0 only)"
  if RUSTUP="$(command -v rustup)" && RUSTC="$("$RUSTUP" which rustc 2>/dev/null)" && [ -x "$RUSTC" ]; then
    case "$("$RUSTC" -V 2>/dev/null)" in
      rustc\ 1.97.*)
        if ! [ -x "$(dirname "$RUSTC")/cargo-clippy" ]; then # the gate's G0 runs cargo clippy
          TCV="Rust without clippy (self-evolution T0 only until: rustup component add clippy, then the steps printed at the end)"
        elif "$RUSTUP" target list --installed 2>/dev/null | grep -qx wasm32-wasip2; then
          TC="$(cd "$(dirname "$RUSTC")/.." && pwd -P)"; TCV="$("$RUSTC" -V)"
        else TCV="Rust without wasm32-wasip2 (self-evolution T0 only until: rustup target add wasm32-wasip2, then the steps printed at the end)"; fi ;;
      *) TCV="$("$RUSTC" -V 2>/dev/null) is not 1.97.x (self-evolution T0 only)" ;;
    esac
  fi
else
  command -v rustup >/dev/null || [ -x "$PW_HOME/.cargo/bin/rustup" ] || die "rustup not found: install Rust from https://rustup.rs (toolchain 1.97), or install a release bundle with scripts/get.sh, which installs Rust 1.97.1 for you"
  RUSTUP="$(command -v rustup || echo "$PW_HOME/.cargo/bin/rustup")"
  RUSTC="$("$RUSTUP" which rustc 2>/dev/null)" || die "rustup has no default toolchain: rustup default 1.97.1"
  case "$("$RUSTC" -V)" in rustc\ 1.97.*) ;; *) die "need rustc 1.97.x, found: $("$RUSTC" -V)" ;; esac
  "$RUSTUP" target list --installed 2>/dev/null | grep -qx wasm32-wasip2 || die "missing target: rustup target add wasm32-wasip2"
  TC="$(cd "$(dirname "$RUSTC")/.." && pwd -P)"; TCV="$("$RUSTC" -V)"
fi
for h in "$RH" "$IH"; do
  case "$h" in /*) ;; *) die "$h is not an absolute path" ;; esac
  [ ${#h} -le 64 ] || die "$h is ${#h} bytes; a home may be at most 64 (the Root's sockets must fit sun_path)"
done
# The installed release ("" when its Root cannot say; every release prints "init-root X.Y.Z"). The upgrade starts at
# 0.4.1, the oldest release it was tested from; an older install is uninstalled first (scripts/get.sh says the same).
installed() { [ -x "$RH/bin/init-root" ] || return 0; { "$RH/bin/init-root" --version 2>/dev/null || true; } | sed -n 's/^init-root \([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' | head -n 1; }
ver_lt() { /usr/bin/python3 -I -c 'import sys; v = lambda s: tuple(int(x) for x in s.split("."))
sys.exit(0 if v(sys.argv[1]) < v(sys.argv[2]) else 1)' "$1" "$2"; }
too_old() { # <version>
  die "INIT $1 is installed ${ELSEWHERE:-in $RH}. An upgrade from a release before 0.4.1 is not supported: uninstall it first (it moves INIT, its record and your provider key to the Trash; nothing is deleted), then run this again:
  $(rh_env)bash $(shq "$REPO/scripts/uninstall.sh")$SECOND"
}
# INIT installed in another ROOT_HOME (v0.5.2; get.sh checks the same, with the same two functions). An owner who
# installed with INIT_ROOT_HOME=X and installs again without it would get a second install: step 7 rewrites the one
# service file (the LaunchAgent dev.init.root, or the systemd user unit init-root.service) for the new home and step 6
# overwrites the init-agent launcher, which leaves the INIT in X, and everything it has become, behind.
svc_file() { # the service file this install would write: none with --no-service, or with HOME not the passwd home
  # (install.sh then refuses the service)
  [ "$NO_SERVICE" = 0 ] && [ -n "$PW_HOME" ] && [ "$HOME" = "$PW_HOME" ] || return 0
  if [ "$OS" = Darwin ]; then printf '%s' "$PW_HOME/Library/LaunchAgents/dev.init.root.plist"
  else printf '%s' "$PW_HOME/.config/systemd/user/init-root.service"; fi
}
other_home() { # [<service file>]: the ROOT_HOME of an INIT this install would take over, and what names it (two
  # lines), or nothing. Read from the service file (the INIT_ROOT_HOME the Root wrote into it) and from INIT's
  # launchers in BIN_DIR (the INIT_ROOT_HOME_DEFAULT install.sh baked into them), when that home holds an install.json.
  # Run only when this ROOT_HOME has none.
  /usr/bin/python3 -I - "${1:-}" "$BIN_DIR/init-agent" "$BIN_DIR/init" <<'PY' 2>/dev/null || true
import os, plistlib, re, sys
svc, launchers = sys.argv[1], sys.argv[2:]
def found(home, by):
    if isinstance(home, str) and os.path.isabs(home) and os.path.isfile(os.path.join(home, "install.json")):
        print(home)
        print(by)
        sys.exit(0)
try:
    if svc.endswith(".plist") and os.path.isfile(svc):
        with open(svc, "rb") as f:
            found((plistlib.load(f).get("EnvironmentVariables") or {}).get("INIT_ROOT_HOME"), "the LaunchAgent dev.init.root runs it")
    elif svc and os.path.isfile(svc):
        for line in open(svc, errors="replace"):
            m = re.fullmatch(r'Environment="INIT_ROOT_HOME=(.*)"\n?', line)
            if m:  # systemd quoting (install.rs service_file): \\ and \" inside the quotes, %% for %
                found(re.sub(r'\\(.)', r'\1', m.group(1)).replace("%%", "%"), "the systemd user unit init-root.service runs it")
except Exception:
    pass
for p in launchers:
    try:
        if os.path.isfile(p) and not os.path.islink(p):
            for line in open(p, errors="replace"):
                m = re.fullmatch(r'INIT_ROOT_HOME_DEFAULT="(.*)"\n?', line)
                if m:
                    found(m.group(1), "the %s command in %s opens it" % (os.path.basename(p), os.path.dirname(p)))
    except OSError:
        pass
PY
}
ELSEWHERE=""; SECOND="" # set when the INIT found is in another ROOT_HOME than this run's
# v0.5.3 (the post-release review of v0.5.2): --upgrade looks too, as get.sh's does. Without INIT_ROOT_HOME on a
# custom-home install it said "not installed … run this without --upgrade", and only that run printed the right line.
if [ ! -e "$RH/install.json" ]; then
  OTHER="$(other_home "$(svc_file)")"
  if [ -n "$OTHER" ]; then
    NEW="$RH"; RH="$(printf '%s\n' "$OTHER" | sed -n 1p)" # every line below names that INIT's home
    if [ $UPGRADE = 1 ]; then
      ELSEWHERE="in $RH: $(printf '%s\n' "$OTHER" | sed -n 2p). It is not in $NEW, the home this upgrade looked in (INIT_ROOT_HOME was not set)"
    else
      ELSEWHERE="in $RH: $(printf '%s\n' "$OTHER" | sed -n 2p). A second install, in $NEW, would take that over and leave this one behind"
      SECOND="
(For a separate, second INIT instead: give it its own INIT_ROOT_HOME, INIT_HOME and INIT_BIN_DIR, and pass --no-service.)"
    fi
  fi
fi
if [ -e "$RH/install.json" ]; then
  HAVE="$(installed)"
  [ -z "$HAVE" ] || ! ver_lt "$HAVE" 0.4.1 || too_old "$HAVE"
fi
if [ $UPGRADE = 0 ] && [ -e "$RH/install.json" ]; then
  UP_HERE="$(rh_env)$(shq "$REPO/scripts/install.sh") --upgrade"
  if [ -n "$HAVE" ] && ver_lt "$HAVE" 0.5.1; then
    if ver_lt "$HAVE" 0.5.0; then WHY="0.4.1 has no init-agent upgrade"; else WHY="0.5.0's own init-agent upgrade runs 0.5.0's get.sh, which 0.5.1 fixed"; fi
    die "INIT $HAVE is already installed ${ELSEWHERE:-($RH/install.json exists)}. Upgrade it to v$VERSION instead, which keeps what INIT has become ($WHY):
  $UP_HERE
or, to the latest release, once from your own shell: $(up_pipe)$SECOND"
  fi
  [ -n "$HAVE" ] && ALT="" || ALT="
(from 0.4.1 or 0.5.0, init-agent upgrade does not apply: $UP_HERE, or the latest release: $(up_pipe))"
  die "INIT ${HAVE:+$HAVE }is already installed ${ELSEWHERE:-($RH/install.json exists)}. Upgrade it to v$VERSION instead, which keeps what INIT has become:
  $(rh_env)$(agent) upgrade        (or: $UP_HERE)$ALT$SECOND"
fi
if [ $UPGRADE = 1 ]; then
  [ -z "$ELSEWHERE" ] || die "INIT ${HAVE:+$HAVE }is installed $ELSEWHERE. Upgrade it to v$VERSION with its home given to install.sh:
  $(rh_env)$(shq "$REPO/scripts/install.sh") --upgrade"
  [ -e "$RH/install.json" ] || die "INIT is not installed ($RH/install.json is missing): run this without --upgrade to install it"
  [ -z "$MOCK" ] && [ -z "$CHARTER_IN" ] || die "--upgrade keeps the installed charter: --mock-upstream and --charter do not apply"
  case "$PROBE_S" in ""|*[!0-9]*) die "--probe-timeout takes a number of seconds" ;; esac
  # the install's own record wins: INIT_HOME and the launchers' directory are where the install put them
  IHB="$(/usr/bin/python3 -I -c 'import json,sys; j=json.load(open(sys.argv[1])); print(j.get("init_home") or ""); print(j.get("bin_dir") or "")' "$RH/install.json")" \
    || die "$RH/install.json is unreadable"
  IH="$(printf '%s\n' "$IHB" | sed -n 1p)"; [ -n "$IH" ] || die "$RH/install.json names no init_home"
  [ -n "${INIT_BIN_DIR:-}" ] || BIN_DIR="$(printf '%s\n' "$IHB" | sed -n 2p)"; BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
fi
if [ -n "$MOCK" ]; then
  [ $NO_SERVICE = 1 ] || die "--mock-upstream is a test hook and requires --no-service"
  case "$MOCK" in 127.0.0.1:[0-9]*) ;; *) die "--mock-upstream takes 127.0.0.1:PORT" ;; esac
fi
if [ $NO_SERVICE = 0 ] && [ $UPGRADE = 0 ] && [ "$HOME" != "$PW_HOME" ]; then
  die "HOME ($HOME) is not your passwd home ($PW_HOME); the service needs the real HOME (or pass --no-service)"
fi
say "$PLATFORM · $TCV · git $GITV · free $(( $(free_mb) / 1024 )) GiB"
[ $BUNDLE = 1 ] && say "release bundle $REPO"
say "ROOT_HOME $RH"
say "INIT_HOME $IH"

# ---------------------------------------------------------------- 2. release binaries
step "2/7 release binaries (v$VERSION)"
fresh() { # <any>: the files match their recorded sha256; unless <any>, the manifest also names HEAD, a clean build
  # and a still-clean tree
  [ -f "$DIST/MANIFEST.json" ] || return 1
  if [ "${1:-}" != any ]; then
    [ -z "$(git -C "$REPO" status --porcelain -- kernel root seed wit $([ -f "$REPO/tui/Cargo.toml" ] && echo tui))" ] || return 1
  fi
  /usr/bin/python3 -I - "$DIST" "$(git -C "$REPO" rev-parse HEAD)" "${1:-}" <<'PY'
import hashlib, json, os, sys
d, head, any_ = sys.argv[1:4]
m = json.load(open(os.path.join(d, "MANIFEST.json")))
if any_ != "any" and (m.get("commit") != head or m.get("dirty")):
    sys.exit(1)
for f, e in m["files"].items():
    p = os.path.join(d, f)
    if not os.path.isfile(p) or hashlib.sha256(open(p, "rb").read()).hexdigest() != e["sha256"]:
        sys.exit(1)
PY
}
if [ $BUNDLE = 1 ]; then
  (cd "$DIST" && "${SHA_CHECK[@]}" SHA256SUMS) || die "the bundle's files do not match its SHA256SUMS; download it again"
  say "release bundle v$VERSION: every file matches SHA256SUMS"
elif [ ! -d "$REPO/vendor" ]; then
  say "vendor/ is missing (a fresh clone): ci/vendor.sh --locked downloads the pinned crates from crates.io"
  "$REPO/ci/vendor.sh" --locked || die "ci/vendor.sh --locked failed"
fi
if [ $BUNDLE = 1 ]; then
  :
elif [ $REUSE = 1 ]; then
  fresh any || die "--reuse: $DIST has no MANIFEST.json, or its files do not match it"
  say "reusing $DIST as it is (--reuse; built from $(sed -n 's/.*"commit": "\(.\{12\}\).*/\1/p' "$DIST/MANIFEST.json"), files verified)"
elif [ $REBUILD = 0 ] && fresh; then
  say "reusing $DIST (built from HEAD, files verified)"
else
  ARGS=()
  [ $WITH_TESTS = 1 ] || ARGS+=(--skip-tests)
  [ $ALLOW_DIRTY = 1 ] && ARGS+=(--allow-dirty)
  say "building with scripts/release.sh ${ARGS[*]:-} (the first build takes about 10 minutes on an idle machine; later builds reuse cargo's cache)"
  "$REPO/scripts/release.sh" ${ARGS[@]+"${ARGS[@]}"} || die "release build failed"
fi
SEED_BUNDLE="$(ls "$DIST"/seed/seed-v*.bundle | head -1)"
for f in init-root init-kernel initctl; do [ -x "$DIST/$f" ] || die "the release has no $f"; done
[ -f "$SEED_BUNDLE" ] || die "the release has no seed bundle"

# ---------------------------------------------------------------- INIT's launchers (step 6, and --upgrade)
put_bin() { # <name> <file>: install <file> as BIN_DIR/<name>, mode 755 (a new file, then a rename)
  case "$1" in */*|""|.*) die "put_bin: bad command name '$1'" ;; esac
  # issue #1: on Linux a command named init would shadow /sbin/init
  [ "$OS" = Linux ] && [ "$1" = init ] && die "refusing to install a command named init on Linux (it would shadow /sbin/init)"
  cp "$2" "$BIN_DIR/.$1.new"
  chmod 755 "$BIN_DIR/.$1.new"
  mv "$BIN_DIR/.$1.new" "$BIN_DIR/$1"
}
old_launcher() { # <file>: INIT's launcher from before v0.4.1 (scripts/init, ROOT_HOME baked in): a regular file, not
  # a link, with that launcher's header line and the INIT_ROOT_HOME_DEFAULT line install.sh wrote
  [ -f "$1" ] && [ ! -L "$1" ] || return 1
  case "$(sed -n 2p "$1")" in '# `init`: open INIT (docs/tui.md'*) ;; *) return 1 ;; esac
  grep -q '^INIT_ROOT_HOME_DEFAULT=' "$1"
}
init_shim() { [ -f "$1" ] && [ ! -L "$1" ] && grep -qx 'INIT_AGENT_SHIM=1' "$1"; } # <file>: v0.4.1's macOS `init` shim
launchers() { # [upgrade]: init-agent (ROOT_HOME baked in) and init-tui into BIN_DIR. INIT's own `init` goes on every
  # platform: the launcher's name before v0.4.1, and v0.4.1's one-release macOS shim (checked by path, regular file and
  # content markers); anything else named init stays. An upgrade touches only BIN_DIR, which its snapshot covers.
  mkdir -p "$BIN_DIR"
  sed "s|^INIT_ROOT_HOME_DEFAULT=.*|INIT_ROOT_HOME_DEFAULT=\"$RH\"|" "$REPO/scripts/init-agent" >"$WORK/init-agent"
  put_bin init-agent "$WORK/init-agent"
  if [ -x "$DIST/init-tui" ]; then # docs/tui.md §9 item 3: next to the launcher, a new file then a rename
    put_bin init-tui "$DIST/init-tui"
    say "  $BIN_DIR/init-tui (the TUI)"
  fi
  say "  $BIN_DIR/init-agent (opens the TUI when it is installed, else init-root attach)"
  local f old=("$BIN_DIR/init")
  [ "${1:-}" = upgrade ] || [ "$BIN_DIR/init" -ef "$HOME/.local/bin/init" ] || old+=("$HOME/.local/bin/init")
  for f in "${old[@]}"; do
    if old_launcher "$f"; then rm -f -- "$f"; say "  removed $f, INIT's launcher under its old name (now init-agent)"
    elif init_shim "$f"; then rm -f -- "$f"; say "  removed $f, the v0.4.1 shim for INIT's old name (the command is init-agent)"
    elif [ -e "$f" ] || [ -L "$f" ]; then say "  note: $f is not INIT's; left as it is"; fi
  done
}
keep_get() { # get.sh into ROOT_HOME/cache/ (the zone cannot write ROOT_HOME): what `init-agent upgrade` runs
  [ -f "$REPO/scripts/get.sh" ] || return 0
  cp "$REPO/scripts/get.sh" "$RH/cache/.get.sh.new" && chmod 755 "$RH/cache/.get.sh.new" && mv "$RH/cache/.get.sh.new" "$RH/cache/get.sh"
}

# ---------------------------------------------------------------- --upgrade (instead of steps 3-7)
if [ $UPGRADE = 1 ]; then
  # the launcher is written here before it is copied into BIN_DIR: under ROOT_HOME, which the agent cannot write (it
  # can write TMPDIR, and so swap the file between the two)
  [ -d "$RH/cache" ] && [ ! -L "$RH/cache" ] || die "$RH/cache is not a directory"
  WORK="$(mktemp -d "$RH/cache/.install.XXXXXX")"
  trap 'rm -rf -- "$WORK"' EXIT
  IENV=(env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" INIT_HOME="$IH" PATH=/usr/bin:/bin)
  ROOT_BIN="$RH/bin/init-root"
  # The service, found as uninstall.sh finds it: the LaunchAgent or systemd user unit that names this ROOT_HOME.
  if [ "$OS" = Darwin ]; then SVCF="$PW_HOME/Library/LaunchAgents/dev.init.root.plist"; else SVCF="$PW_HOME/.config/systemd/user/init-root.service"; fi
  SVC=0
  if [ -f "$SVCF" ] && { grep -qF "<string>$RH</string>" "$SVCF" || grep -qxF "Environment=\"INIT_ROOT_HOME=$RH\"" "$SVCF"; }; then SVC=1; fi
  lock_free() { # 0 when no process holds ROOT_HOME/run/root.lock (the Root's flock); never waits
    /usr/bin/python3 -I -c 'import fcntl,os,sys
try: fd = os.open(sys.argv[1], os.O_RDONLY)
except FileNotFoundError: sys.exit(0)
try: fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError: sys.exit(1)
sys.exit(0)' "$RH/run/root.lock"
  }
  # The service's stop and start (v0.5.3, the release review: their errors were thrown away, so a stop that failed read
  # as a Root that would not stop): svc runs one, svc_cmd prints it as the owner types it, svc_err is what it said.
  svc() { # stop|start
    if [ "$OS" = Darwin ] && [ "$1" = stop ]; then launchctl bootout "gui/$(id -u)/dev.init.root"
    elif [ "$OS" = Darwin ]; then launchctl bootstrap "gui/$(id -u)" "$SVCF" || launchctl kickstart "gui/$(id -u)/dev.init.root"
    else systemctl --user "$1" init-root.service; fi
  }
  svc_cmd() { # stop|start
    if [ "$OS" = Darwin ] && [ "$1" = stop ]; then printf 'launchctl bootout gui/%s/dev.init.root' "$(id -u)"
    elif [ "$OS" = Darwin ]; then printf 'launchctl bootstrap gui/%s %s' "$(id -u)" "$(shq "$SVCF")"
    else printf 'systemctl --user %s init-root.service' "$1"; fi
  }
  svc_err() { # <its output file>: its last lines, on one line
    tail -n 3 "$1" | tr -s '\n' ' ' | sed 's/ *$//'
  }
  svc_start() { # the service again (if INIT has one); else how to start the Root
    if [ $SVC = 0 ]; then
      say "  start the Root again as before, in its own terminal: $(rcmd start --foreground)"
    elif ! svc start >"$WORK/svc.out" 2>&1; then
      say "  warning: $(svc_cmd start) failed: $(svc_err "$WORK/svc.out")"
      say "  The Root is not running. Start it: $(svc_cmd start)   (or, in a terminal of its own: $(rcmd start --foreground))"
    else
      for _ in $(seq 1 120); do [ -S "$RH/run/owner.sock" ] && ! lock_free && break; sleep 0.25; done
      lock_free && say "  warning: the Root did not come back under the service; see $RH/logs/" || say "  the Root runs again under the service"
    fi
  }
  # A commit or a rollback that fails leaves the upgrade half way, and the Root stopped. It is not started here: both
  # take run/root.lock and refuse a running Root, and a Root started before the commit would run the new tuple while
  # lkg and the rollback anchors still name the old one. So the message says it is stopped and how to start it after.
  left_stopped() { # <the command to run first> [<the other way>]: the rest of the die message
    local svc start alt=""
    [ -z "${2:-}" ] || alt="  (or $2)
"
    if [ $SVC = 0 ]; then
      svc="The Root is not running (it was not running before the upgrade, and has no service)"; start="$(rcmd start --foreground)   (in a terminal of its own)"
    elif [ "$OS" = Darwin ]; then
      svc="The Root's LaunchAgent stays stopped (the upgrade stopped it)"; start="launchctl bootstrap gui/$(id -u) $(shq "$SVCF")"
    else
      svc="The Root's systemd user unit stays stopped (the upgrade stopped it)"; start="systemctl --user start init-root.service"
    fi
    printf '%s, and this step needs it stopped. Run:\n  %s\n%sthen start the Root: %s' "$svc" "$1" "$alt" "$start"
  }
  # 1. stop: the zone through the Root, then the Root itself (service); a Root in a terminal is never signalled from here
  stop_root() {
    local why="" n=120 agent="its agent stopped by the upgrade (start it again: $(rcmd start))"
    [ $SVC = 1 ] || die "the Root (pid $(cat "$RH/run/root.pid" 2>/dev/null || echo '?')) runs without the service. Stop it in its terminal (Ctrl-C), then run the upgrade again. Nothing was changed."
    "${IENV[@]}" "$ROOT_BIN" stop >/dev/null 2>&1 || agent="its agent as it was (the Root did not answer the upgrade's stop)"
    # a stop the service refused stops nothing: a short wait, then what it said
    svc stop >"$WORK/svc.out" 2>&1 || { why=" ($(svc_cmd stop) failed: $(svc_err "$WORK/svc.out"))"; n=20; }
    for _ in $(seq 1 $n); do lock_free && break; sleep 0.5; done
    lock_free || die "the Root did not stop within $((n / 2)) s$why. Nothing was changed: the Root (pid $(cat "$RH/run/root.pid" 2>/dev/null || echo '?')) still runs the version you had, with $agent. To upgrade, stop the Root yourself ($(svc_cmd stop)), then run the upgrade again."
    say "  stopped (the service)"
  }
  step "upgrade 1/5 stop INIT"
  if ! lock_free; then stop_root; else say "  the Root is not running"; fi
  # 2. the replace step, by this release's own Root (refusals change nothing). --by-installer: this script runs the
  #    probe and the commit itself, so the Root prints no "next" hint for them (v0.5.2)
  step "upgrade 2/5 replace the Root and the kernel (v$VERSION)"
  set +e
  "${IENV[@]}" "$DIST/init-root" upgrade --release "$DIST" --bin-dir "$BIN_DIR" --by-installer
  URC=$?
  set -e
  case $URC in
    0) ;;
    4) svc_start; say ""; say "INIT already runs v$VERSION; nothing to do."; exit 0 ;;
    3) svc_start; die "the upgrade was refused (above); nothing was changed" ;;
    *) svc_start; die "the upgrade failed (exit $URC, above); the old install was left as it was" ;;
  esac
  # 3. the launchers
  step "upgrade 3/5 the init-agent command"
  launchers upgrade
  # 4. the health probe on the new binaries
  step "upgrade 4/5 health probe (the new Root starts the kernel and the body, then stops)"
  if "${IENV[@]}" "$ROOT_BIN" upgrade --probe --timeout "$PROBE_S"; then
    step "upgrade 5/5 commit"
    "${IENV[@]}" "$ROOT_BIN" upgrade --commit \
      || die "the commit failed (above). $(left_stopped "$(rcmd upgrade --commit)" "to go back to the old version instead: $(rcmd upgrade --rollback)")"
    keep_get
    svc_start
    cat <<EOF

INIT is upgraded to v$VERSION ($(( $(date +%s) - T_START )) s). Kept as they were: the body INIT evolved, its extensions,
the sessions, the lineage, your keys and your charter. Not done by this release: offering its new seed to INIT.
Open INIT: $(tilde "$BIN_DIR/init-agent")
EOF
    exit 0
  fi
  # the probe failed: back to the old binaries (this release's Root rolls back; an older Root that knows how is the fallback)
  step "upgrade 5/5 roll back (the health probe failed)"
  set +e
  "${IENV[@]}" "$DIST/init-root" upgrade --rollback
  RRC=$?
  if [ $RRC != 0 ]; then
    UID_="$("${IENV[@]}" "$DIST/init-root" upgrade --status --json 2>/dev/null | /usr/bin/python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("id",""))' 2>/dev/null)"
    OLD="$RH/state/upgrade/$UID_/rh/bin/init-root"
    if [ -n "$UID_" ] && [ -x "$OLD" ]; then "${IENV[@]}" "$OLD" upgrade --rollback; RRC=$?; fi
  fi
  set -e
  [ $RRC = 0 ] || die "the rollback did not finish (above). The snapshot of the old install is in $RH/state/upgrade/. $(left_stopped "INIT_ROOT_HOME=$(shq "$RH") $(shq "$DIST/init-root") upgrade --rollback")"
  svc_start
  say ""
  say "The upgrade to v$VERSION failed its health probe and was rolled back: INIT runs its old version, unchanged."
  say "The probe's log: $RH/logs/upgrade-probe.log"
  exit 1
fi

# ---------------------------------------------------------------- 3. the charter: the settings, one confirmation
step "3/7 charter (your limits; root/root.md §3.12)"
money_ok() { [[ "$1" =~ ^[0-9]{1,7}(\.[0-9]{1,6})?$ ]]; }
if [ $YES = 0 ] && ! { : </dev/tty; } 2>/dev/null; then die "no terminal to confirm the settings: pass --yes to install with them"; fi
WORK="$(mktemp -d "${TMPDIR:-/tmp}/init-install.XXXXXX")"
trap 'rm -rf -- "$WORK"' EXIT
CHARTER="$WORK/charter.toml"
MODEL=deepseek-flash # the charter's and the kernel config's starting model; the first run of init-agent picks yours
if [ -n "$CHARTER_IN" ]; then
  cp "$CHARTER_IN" "$CHARTER"
  MODEL="$(sed -n 's/^\[actions\.providers\.models\."\(.*\)"\]$/\1/p' "$CHARTER" | head -1)"
  MODEL="${MODEL:-deepseek-flash}"
  DAY="$(sed -n 's/^day_usd = "\(.*\)".*/\1/p' "$CHARTER" | head -1)"; DAY="${DAY:-5.00}"
  FLOOR="$(sed -n 's/^disk_min_free_gib = \([0-9]*\).*/\1/p' "$CHARTER" | head -1)"; FLOOR="${FLOOR:-20}"
  say "using the charter $CHARTER_IN"
else
  FLOOR="${FLOOR:-20}"
  money_ok "$DAY" && gt "$DAY" 0 || die "--day-usd takes a positive amount like 5 or 2.50"
  money_ok "$TOTAL" && ge "$TOTAL" "$DAY" || die "--total-usd takes an amount at least the daily one ($DAY)"
  [[ "$LEVEL" =~ ^[0-4]$ ]] || die "--max-level takes 0, 1, 2, 3 or 4"
  [[ "$FLOOR" =~ ^[0-9]+$ ]] && [ "$FLOOR" -ge 1 ] || die "--disk-floor takes a whole number of GiB, at least 1"
  case "$ACCESS" in full|workspace) ;; *) die "--access takes full or workspace" ;; esac
  [ -n "$PURPOSE" ] && [ ${#PURPOSE} -le 4096 ] || die "--purpose takes one line of 1-4096 bytes"
  /usr/bin/python3 -I - "$DIST/assets" "$CHARTER" "$PURPOSE" "$DAY" "$TOTAL" "$LEVEL" "$MODEL" "$FLOOR" "$MOCK" "$ACCESS" <<'PY'
import json, os, re, sys
assets, out, purpose, day, total, level, model, floor, mock, access = sys.argv[1:11]
t = open(os.path.join(assets, "charter-template.toml")).read()
const = open(os.path.join(assets, "CONSTITUTION.md")).read()
q = lambda s: json.dumps(s, ensure_ascii=False)  # a TOML basic string
fix = lambda x: f"{float(x):.2f}" if re.fullmatch(r"[0-9]+(\.[0-9]{1,2})?", x) else x
def sub(pat, rep):
    global t
    t, n = re.subn(pat, lambda _m: rep, t, count=1, flags=re.M)
    if n != 1:
        sys.exit(f"charter template: no match for {pat}")
sub(r'^text = "REPLACE ME[^"]*"$', "text = " + q(purpose))
sub(r"^max_level = 4\b", f"max_level = {int(level)}")
sub(r'^day_usd = "5\.00"$', f'day_usd = "{fix(day)}"')
sub(r'^total_usd = "25\.00"$', f'total_usd = "{fix(total)}"')
sub(r"^disk_min_free_gib = 20$", f"disk_min_free_gib = {int(floor)}")
sub(r'^mode = "full"', f'mode = "{access}"')
# the model table: the header names the model; each price line present in the template gets that model's price
prices = {"deepseek-flash": {"input": 300000, "cache_read": 6000, "output": 1200000},
          "deepseek-v4-pro": {"input": 1320000, "cache_read": 44000, "output": 3960000}}[model]
m = re.search(r'^\[actions\.providers\.models\."deepseek-flash"\]\n(?:[^\[\n].*\n)*', t, re.M)
if not m:
    sys.exit("charter template: no [actions.providers.models.\"deepseek-flash\"] table")
block = m.group(0).replace('"deepseek-flash"', f'"{model}"', 1)
for k, v in prices.items():
    block = re.sub(rf"^{k}_musd_per_mtok = [0-9]+$", f"{k}_musd_per_mtok = {v}", block, flags=re.M)
t = t[:m.start()] + block + t[m.end():]
if mock:  # test hook: the egress forwards to a loopback init-mockllm (root.test_mode; scripts/slice1/s1.sh mock)
    sub(r'^destinations = \["api\.deepseek\.com:443"\]$', f'destinations = ["{mock}"]')
    sub(r'^paths = \[.*\]$', 'paths = ["chat/completions", "models", "user/balance"]')
    sub(r"^\[sandbox\]$", f'[actions.providers.providers.deepseek]\nhost = "{mock}"\n\n[sandbox]')
    t += "\n[root]\ntest_mode = true\n"
t += "[constitution]\ntext = '''\n" + const + "'''\n"
with open(out, "w") as f:
    f.write(t)
PY
fi
"$DIST/init-root" --version >/dev/null || die "the release's init-root does not run"
set +e
CHECK="$(env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" INIT_HOME="$IH" PATH=/usr/bin:/bin "$DIST/init-root" charter check "$CHARTER" 2>&1)"
CRC=$?
set -e
[ $CRC = 0 ] || { say "$CHECK"; die "the charter is invalid (exit $CRC)"; }
val() { printf '%s\n' "$CHECK" | sed -n "s/^$1 = //p" | head -1 | tr -d '"'; } # from the validated charter itself
ACCESS_EFF="$(val sandbox.mode)"; ACCESS_EFF="${ACCESS_EFF:-full}"
say "INIT installs with these settings (its charter, $RH/charter.toml):"
say "  disk floor   keep $(val quotas.disk_min_free_gib) GiB free; INIT pauses new work below it                       (--disk-floor N)"
say "  spending     shown against \$$(val money.day_usd) a day and \$$(val money.total_usd) in total; tracked, not a cap     (--day-usd X --total-usd X)"
say "               the one dollar limit the Root enforces is a goal's budget (/goal ... usd=1.00); set a limit at your provider too"
say "  autonomy     starts at L0 and may earn up to L$(val autonomy.max_level)                                       (--max-level N)"
if [ "$ACCESS_EFF" = workspace ]; then
  say "  access       workspace: reads your home except secrets, writes the folder init-agent starts in    (--access full|workspace)"
else
  say "  access       full: reads and writes what you can, never INIT's Root files                           (--access full|workspace)"
  say "               $(tilde "$BIN_DIR/init-agent") --workspace confines a session"
fi
[ -z "$MOCK" ] || say "  TEST MODE    the provider egress goes to the mock at $MOCK"
say "Change any of them later: edit $RH/charter.toml, then run $(rcmd charter check --apply). The provider, model and key: the first run of init-agent."
if [ $YES = 0 ]; then
  printf '\nInstall with these settings? [Y/n] ' >/dev/tty
  IFS= read -r ok </dev/tty || ok=n
  case "${ok:-y}" in y|Y|yes) ;; *) die "stopped; nothing was installed" ;; esac
fi
NEED_MB=$(( FLOOR * 1024 + 2000 )); [ $BUNDLE = 1 ] && NEED_MB=$(( FLOOR * 1024 + 500 )) # the bundle builds nothing
[ "$(free_mb)" -ge "$NEED_MB" ] || die "only $(free_mb) MB free; the floor is $FLOOR GiB and the install needs about $(( NEED_MB - FLOOR * 1024 )) MB more ($NEED_MB MB). Free some disk, or choose a lower floor (--disk-floor N)."

# ---------------------------------------------------------------- 4. init-root install (minutes: the cold body build)
PRE=()
if [ $BUNDLE = 1 ]; then
  step "4/7 init-root install (the prebuilt body, hash-equal to the maintainer's build: seconds)"
  BODY_SHA="$(sed -n 's/.*"sha256": "\([0-9a-f]\{64\}\)".*/\1/p' "$DIST/body/BODY.json" | head -1)"
  BODY_COMMIT="$(sed -n 's/.*"commit": "\([0-9a-f]\{40\}\)".*/\1/p' "$DIST/body/BODY.json" | head -1)"
  [ -n "$BODY_SHA" ] && [ -n "$BODY_COMMIT" ] || die "body/BODY.json has no sha256 or commit"
  PRE=(--prebuilt-body "$DIST/body/init-body" --prebuilt-sha256 "$BODY_SHA" --prebuilt-commit "$BODY_COMMIT")
else
  step "4/7 init-root install (the kernel builds the body from the seed: about 3-4 minutes)"
fi
SVC=(); [ $NO_SERVICE = 1 ] && SVC=(--no-service)
# HOME is your passwd home for every init-root call: the broker's class rules compare the kernel's HOME_REAL (the
# passwd home) with the Root's HOME. With --no-service nothing is written under it.
IENV=(env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" INIT_HOME="$IH" PATH=/usr/bin:/bin)
LOG="$WORK/install.out"
T0=$(date +%s)
# --bin-dir: the Root write-denies the dir that holds `init`/`init-tui` to the zone in every mode (ADR-009 Z3a).
"${IENV[@]}" "$DIST/init-root" install --release "$DIST" --charter "$CHARTER" --toolchain "$TC" \
  --bin-dir "$BIN_DIR" --seed-source "$SEED_BUNDLE" ${PRE[@]+"${PRE[@]}"} ${SVC[@]+"${SVC[@]}"} >"$LOG" 2>&1 &
IPID=$!
LOWEST=$(free_mb)
while kill -0 $IPID 2>/dev/null; do
  sleep 1 # a prebuilt install takes seconds; a build prints its progress every second
  f=$(free_mb); [ "$f" -lt "$LOWEST" ] && LOWEST=$f
  kill -0 $IPID 2>/dev/null && printf '  … %3d s, free %d MB\r' $(( $(date +%s) - T0 )) "$f"
done
IRC=0; wait $IPID || IRC=$?
printf '\n'
if [ $IRC != 0 ]; then
  cat "$LOG" >&2
  tail -30 "$IH/state/kernel/logs/install-build.log" 2>/dev/null >&2 || true
  die "init-root install failed (exit $IRC) after $(( $(date +%s) - T0 )) s. Fix the cause above and run install.sh again."
fi
sed 's/^/  /' "$LOG"
say "  installed in $(( $(date +%s) - T0 )) s; lowest free space $LOWEST MB"
# The build slot is a cache that the next build resets (kernel/supervise.md §3.6.2); free it now (exact path).
if [ -e "$IH/cache/build/slot" ]; then rm -rf -- "$IH/cache/build/slot"; say "  build slot freed; free $(free_mb) MB"; fi

# ---------------------------------------------------------------- 5. kernel/config.toml (yours to edit)
step "5/7 kernel/config.toml (model, prices, reference budgets)"
/usr/bin/python3 -I - "$IH/kernel/config.toml" "$MODEL" "$DAY" <<'PY'
import os, re, sys
from decimal import Decimal, ROUND_DOWN
path, model, day = sys.argv[1:4]
old = open(path).read()
sb = re.search(r"^\[sandbox\]\n(?:[^\[\n].*\n?)*", old, re.M)  # the Root wrote [sandbox] = install.json; keep it verbatim
if not sb:
    sys.exit("kernel/config.toml has no [sandbox] table")
d = Decimal(day)
cents = lambda x: str(x.quantize(Decimal("0.01"), rounding=ROUND_DOWN))
req = min(Decimal("1.00"), d / 5)          # D6: the soft day is clamped to ceiling - request_max, so keep it small
task = min(Decimal("0.50"), d / 2)
prices = {"deepseek-flash": ('{ input = "0.30", cache_read = "0.006", output = "1.20" }', '{ input = "0.15", cache_read = "0.003", output = "0.60" }'),
          "deepseek-v4-pro": ('{ input = "1.32", cache_read = "0.044", output = "3.96" }', '{ input = "0.66", cache_read = "0.022", output = "1.98" }')}[model]
text = f"""# INIT kernel config (docs/kernel/config.md), written by scripts/install.sh. Edit freely, then restart the Root.
# The key, the day boundary and the spending references are in ROOT_HOME/charter.toml. [sandbox] must stay equal to
# ROOT_HOME/install.json, because the Root's broker compares them.
schema_version = 2

[model]
provider = "deepseek"
id = "{model}"

[budget]                 # soft budgets (config.md §4.3); the Root's money policy is observe-only (root/egress.md)
day_usd = "{cents(d)}"
task_usd = "{cents(task)}"
request_max_usd = "{cents(req)}"

{sb.group(0).rstrip()}

[providers.deepseek.schedule]
tz      = "+00:00"
default = "offpeak"
windows = [
  {{ days = "mon-fri", from = "01:00", to = "04:00", use = "peak" }},
  {{ days = "mon-fri", from = "06:00", to = "10:00", use = "peak" }},
]
default_dates_tz = "+08:00"
default_dates    = []

[providers.deepseek.models."{model}"]
max_output            = 393216
reserve_output_tokens = 65536
reference_window      = "peak"
prices.peak    = {prices[0]}
prices.offpeak = {prices[1]}
"""
tmp = path + ".new"
with open(tmp, "w") as f:
    f.write(text)
    f.flush()
    os.fsync(f.fileno())
os.chmod(tmp, 0o644)
os.replace(tmp, path)
PY
say "  $IH/kernel/config.toml: model $MODEL"

# ---------------------------------------------------------------- 6. the `init-agent` command
step "6/7 the init-agent command"
launchers
keep_get
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) say "  note: $BIN_DIR is not on your PATH; add it: $(path_add "$BIN_DIR")" ;; esac

# ---------------------------------------------------------------- 7. the service
step "7/7 the Root service"
PLIST="$PW_HOME/Library/LaunchAgents/dev.init.root.plist"
UNIT="$PW_HOME/.config/systemd/user/init-root.service"
if [ $NO_SERVICE = 1 ]; then
  say "  skipped (--no-service). Start the Root yourself, in its own terminal:"
  say "    $(rcmd start --foreground)"
elif [ "$OS" = Linux ]; then
  # LINUX: the systemd user unit init-root install wrote (Restart=always, Delegate=memory for the per-workload
  # memory.max). It runs while you are logged in; `loginctl enable-linger` keeps it running after logout.
  [ -f "$UNIT" ] || die "init-root install wrote no $UNIT"
  systemctl --user daemon-reload 2>/dev/null || die "systemctl --user is not available (no systemd user session); reinstall with --no-service and start the Root yourself, in a terminal of its own: $(rcmd start --foreground)"
  systemctl --user enable --now init-root.service || die "systemctl --user enable --now init-root.service failed; see: journalctl --user -u init-root"
  for _ in $(seq 1 120); do [ -S "$RH/run/owner.sock" ] && break; sleep 0.25; done
  [ -S "$RH/run/owner.sock" ] || die "the Root did not open $RH/run/owner.sock within 30 s; see: journalctl --user -u init-root"
  say "  enabled $UNIT (systemd --user keeps the Root running and starts it at login; for after logout: loginctl enable-linger)"
  "${IENV[@]}" "$RH/bin/init-root" status 2>/dev/null | sed 's/^/  /' | head -12 || true
else
  [ -f "$PLIST" ] || die "init-root install wrote no $PLIST"
  launchctl bootout "gui/$(id -u)/dev.init.root" 2>/dev/null || true
  launchctl bootstrap "gui/$(id -u)" "$PLIST" || die "launchctl bootstrap $PLIST failed"
  for _ in $(seq 1 120); do [ -S "$RH/run/owner.sock" ] && break; sleep 0.25; done
  [ -S "$RH/run/owner.sock" ] || die "the Root did not open $RH/run/owner.sock within 30 s; see $RH/logs/launchd.log"
  say "  loaded $PLIST (launchd keeps the Root running and starts it at login)"
  "${IENV[@]}" "$RH/bin/init-root" status 2>/dev/null | sed 's/^/  /' | head -12 || true
fi

ELAPSED=$(( $(date +%s) - T_START ))
PATH_HINT="" # the README's bare `init-agent` needs BIN_DIR on PATH; a default macOS PATH lacks ~/.local/bin (REL-051)
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) PATH_HINT="   ($BIN_DIR is not on your PATH; to type just init-agent, add: $(path_add "$BIN_DIR"))" ;; esac
cat <<EOF

INIT v$VERSION is installed ($(( ELAPSED / 60 )) min $(( ELAPSED % 60 )) s).

Next: $(tilde "$BIN_DIR/init-agent")$PATH_HINT
  The first run picks the provider and the model and asks for your key (hidden; stored 0600 in $RH/keys/, never by
  this script). Then type a task. /stop stops the agent's current body; /panic stops everything and demotes one level.

Your limits: $RH/charter.toml (edit, then: $(rcmd charter check --apply)).
             Spending is tracked, not capped: /usage in the chat, or $(rcmd egress stats);
             a goal's usd= budget is enforced; set a limit at your provider.
Logs:        $RH/logs/ (the Root), $IH/state/kernel/logs/ (the kernel and the body)
Upgrade:     $(rh_env)$(tilde "$BIN_DIR/init-agent") upgrade (keeps what INIT has become: its body, lineage, keys and charter)
Uninstall:   $(rh_env)bash $(shq "$REPO/scripts/uninstall.sh")   (moves both homes to the Trash)
EOF
if [ "$ACCESS_EFF" = workspace ]; then
  cat <<EOF
Access:      WORKSPACE: the agent writes only the folder you start init-agent in. Full access for a session: /full on
             (or sandbox.mode = "full" in the charter; docs/adr/009-codex-style-permissions.md).
EOF
else
  cat <<EOF
Access:      FULL (default): the agent can read and write your files and use the network; INIT's Root ($RH) stays
             off-limits. To confine it: $(tilde "$BIN_DIR/init-agent") --workspace, /full off, or sandbox.mode = "workspace" in the charter
             (docs/adr/012-full-access-by-default.md).
EOF
fi
if [ "$TC" = none ]; then
  # `init-root toolchain --set` needs the Root's lock, and `init-root stop` (/root stop) stops only the agent: stop the
  # Root itself, as the Root's own hint says (install.rs `evolution`, `stop_hint`, `start_hint`)
  if [ $NO_SERVICE = 1 ]; then
    RSTOP="Ctrl-C in the Root's terminal (if you started it)"; RSTART="$(rcmd start --foreground)   (in a terminal of its own)"
  elif [ "$OS" = Darwin ]; then
    RSTOP="launchctl bootout gui/$(id -u)/dev.init.root"; RSTART="launchctl bootstrap gui/$(id -u) $(shq "$PLIST")"
  else
    RSTOP="systemctl --user stop init-root.service"; RSTART="systemctl --user start init-root.service"
  fi
  cat <<EOF

Self-evolution: T0 only (skills, prompts, extension source). Rebuilding itself (T1/T2) needs Rust 1.97.1 with
wasm32-wasip2. get.sh installs it unless you passed --no-rust (or rustup failed). To add it now:
  $RUSTUP_CMD
then record it with the Root stopped (/root stop stops only the agent, and the Root holds the lock):
  $RSTOP
  $(rcmd toolchain --set)
  $RSTART
EOF
fi
