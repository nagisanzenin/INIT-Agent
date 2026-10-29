#!/bin/sh
# Install INIT from a prebuilt release bundle; it installs the Rust toolchain itself (docs/ux-v1.md §3.2.3; lane UX-BUNDLE).
#
#   curl -fsSL "$INIT_DIST_URL/get.sh" | sh                       download the current bundle and install it
#   sh scripts/get.sh --from init-v0.5.0-preview-macos-arm64.tar.gz [--sha256 HEX] [-- <install.sh args>]
#   (Linux: init-v<version>-linux-arm64.tar.gz on aarch64, init-v<version>-linux-x86_64.tar.gz on x86_64)
#
#   --from PATH|URL   a local bundle (.tar.gz) or a URL; default "$INIT_DIST_URL/init-v<version>-<target>.tar.gz", the
#                     version from "$INIT_DIST_URL/latest.json" and the target from this machine
#   --sha256 HEX      the bundle's expected sha256; default: latest.json's, or the <bundle>.sha256 next to it (a URL's,
#                     or a local file's on a fresh install). An upgrade from a local file needs --sha256
#   --unpack-only     verify and unpack, do not install
#   --no-rust         skip the Rust toolchain (INIT then rewrites only its skills and prompts, tier T0)
#   --upgrade         upgrade the installed INIT to this bundle (docs/guide.md "Upgrade"). With INIT installed, its
#                     agent may have written whatever the owner can (full access): the PATH directories, the working
#                     directory, the folder a bundle sits in. So an upgrade runs the system tools (PATH
#                     /usr/bin:/bin:/usr/sbin:/sbin) from /; the download, or a copy of the local bundle, its check and
#                     the unpacked copy go under ROOT_HOME/cache/upgrade/ (the agent cannot write ROOT_HOME in any
#                     access mode); a local bundle's sha256 comes from --sha256 only. Then that copy's
#                     scripts/install.sh --upgrade runs. No Rust step. From 0.5.1 `init-agent upgrade` runs this; from
#                     0.4.1 or 0.5.0: /usr/bin/curl -q -fsSL <get.sh URL> | /bin/sh -s -- --upgrade
#   --version V       that release instead of the latest: the tag v<V>, else v<V>-preview (0.5.0 finds v0.5.0-preview;
#                     0.5.0-preview is taken as it is)
#   --allow-unsigned  install a download whose signature cannot be checked (no SHA256SUMS.sig, a release before 0.5.3,
#                     no ssh-keygen): its sha256 is then all that is checked, which proves integrity, not origin
#   --allow-expired   install a release whose signed SHA256SUMS has expired (what a replayed old release looks like)
#   -- ARGS           passed to the bundle's scripts/install.sh, after --yes (for example --no-service, --disk-floor 10)
#
# The signature (v0.5.3; docs/release-signing.md): a downloaded bundle is installed only when the release's
# SHA256SUMS, which lists it under the release's version and an expiry date, verifies under INIT's release key
# (RELEASE_KEYS below). On an upgrade the installed Root checks it (from 0.5.3: `init-root release verify`, with the key
# compiled into it); otherwise `ssh-keygen -Y verify` does (OpenSSH 8.1 or newer: macOS, and Linux's openssh-client).
# A local bundle (--from FILE) is checked by its sha256 only, as before 0.5.3.
#
# Steps: macOS 14+ on Apple silicon or Linux on aarch64/x86_64, not root → download (curl) → signature → sha256 → unpack into
# ~/.local/share/init/<version>/ (INIT_APP_DIR overrides ~/.local/share/init) → check every file against the
# bundle's SHA256SUMS → the bundle's scripts/install.sh --yes, which installs with the defaults (ROOT_HOME
# ~/.init-root, INIT_HOME ~/.init, the `init-agent` command in ~/.local/bin, the LaunchAgent or the systemd user unit unless
# --no-service). Self-rewrite is the point of INIT, so get.sh also makes sure the Rust toolchain it rebuilds itself
# with is present: rustup's toolchain 1.97.1 with the wasm32-wasip2 target and clippy, which the gate runs (installed with rustup if missing, without
# touching your shell files or your default toolchain). The provider, key and model are chosen on the first run of
# `init-agent` (the TUI's setup). Nothing here needs sudo or a key.
#
# INIT_DIST_URL defaults to the latest GitHub release of nagisanzenin/INIT-Agent. POSIX sh: it runs under sh, dash and
# zsh's sh mode.
set -eu
umask 022

DIST_URL="${INIT_DIST_URL:-https://github.com/nagisanzenin/INIT-Agent/releases/latest/download}"
DIST0="$DIST_URL" # as given: --version changes DIST_URL to that release's directory
# The release key (v0.5.3; docs/release-signing.md): the OpenSSH form of the ed25519 key the maintainer signs each
# release's SHA256SUMS with, one line per key, the newest first. The installed Root pins the same lines (release.rs
# RELEASE_KEYS; a test checks that they agree). Empty in a copy made before the key ceremony: it verifies no download.
RELEASE_KEYS='ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKMhJt8xvzNDPIJZg1WUaKzOPzqdtZj66Paubkbgqjf+ INIT release key 1 (2026-10)'
FROM=""; WANT=""; WANT_FROM=""; UNPACK_ONLY=0; NO_RUST=0; UPGRADE=0; RELEASE=""; ALLOW_UNSIGNED=0; ALLOW_EXPIRED=0
while [ $# -gt 0 ]; do
  case "$1" in
    --from) FROM="$2"; shift 2 ;;
    --sha256) WANT="$2"; WANT_FROM=owner; shift 2 ;;
    --unpack-only) UNPACK_ONLY=1; shift ;;
    --no-rust) NO_RUST=1; shift ;;
    --upgrade) UPGRADE=1; shift ;;
    --version) RELEASE="$2"; shift 2 ;;
    --allow-unsigned) ALLOW_UNSIGNED=1; shift ;;
    --allow-expired) ALLOW_EXPIRED=1; shift ;;
    --) shift; break ;;
    -h|--help) sed -n '2,45p' "$0"; exit 0 ;;
    *) echo "get.sh: unknown argument $1 (see --help)" >&2; exit 64 ;;
  esac
done
NO_SERVICE=0 # install.sh's --no-service, among the arguments after --: then the install writes no service file
for a in "$@"; do [ "$a" != --no-service ] || NO_SERVICE=1; done

say() { printf '%s\n' "$*"; }
ok() { printf '  ok  %s\n' "$*"; }
die() { printf 'get.sh: %s\n' "$*" >&2; exit 1; }
shq() { # a word as the shell reads it back: as it is when plain, else in single quotes
  case "$1" in *[!A-Za-z0-9._/+=:,@%-]*|"") printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")" ;; *) printf '%s' "$1" ;; esac
}
if [ $UPGRADE = 1 ]; then
  # INIT is installed: with full access its agent can write the owner's PATH directories and the working directory
  # (ADR-012). The upgrade replaces the Root, so it runs no tool from them and reads no file there: the system tools,
  # from / (a relative --from is made absolute first).
  case "$FROM" in ""|http://*|https://*|/*) ;; *) FROM="$(pwd -P)/$FROM" ;; esac
  PATH=/usr/bin:/bin:/usr/sbin:/sbin; export PATH
  cd / || die "cannot change to /"
fi
# This run's arguments as the owner gave them, quoted (FROM, WANT and DIST_URL change below), for the line that runs it
# again with one flag more (again). v0.5.3 (the release review): that line dropped them, so `--version 0.5.2 --
# --no-service` with its own homes was offered a line that installs the latest release, with the service, in the
# default homes. AGAIN_P: get.sh's (the pipe's); AGAIN_A: init-agent upgrade's (its --bundle is --from); AGAIN_I: -- and
# install.sh's.
AGAIN_P=""; AGAIN_A=""; AGAIN_I=""
[ -z "$RELEASE" ] || { AGAIN_P="$AGAIN_P --version $(shq "$RELEASE")"; AGAIN_A="$AGAIN_A --version $(shq "$RELEASE")"; }
[ -z "$FROM" ] || { AGAIN_P="$AGAIN_P --from $(shq "$FROM")"; AGAIN_A="$AGAIN_A --bundle $(shq "$FROM")"; }
[ -z "$WANT" ] || { AGAIN_P="$AGAIN_P --sha256 $(shq "$WANT")"; AGAIN_A="$AGAIN_A --sha256 $(shq "$WANT")"; }
[ $UNPACK_ONLY = 0 ] || AGAIN_P="$AGAIN_P --unpack-only"
[ $NO_RUST = 0 ] || AGAIN_P="$AGAIN_P --no-rust"
[ $ALLOW_UNSIGNED = 0 ] || { AGAIN_P="$AGAIN_P --allow-unsigned"; AGAIN_A="$AGAIN_A --allow-unsigned"; }
[ $ALLOW_EXPIRED = 0 ] || { AGAIN_P="$AGAIN_P --allow-expired"; AGAIN_A="$AGAIN_A --allow-expired"; }
[ $# = 0 ] || { AGAIN_I=" --"; for a in "$@"; do AGAIN_I="$AGAIN_I $(shq "$a")"; done; }
T0=$(date +%s)

# ---------------------------------------------------------------- who and where
[ "$(id -u)" != 0 ] || die "run this as yourself, not as root (INIT never needs sudo)"
OS="$(uname -s)"
case "$OS" in
  Darwin)
    [ "$(uname -m)" = arm64 ] || die "this bundle is for Apple silicon (arm64); this Mac is $(uname -m)"
    MACOS="$(sw_vers -productVersion)"
    [ "${MACOS%%.*}" -ge 14 ] || die "macOS 14 or newer is needed (this is $MACOS)"
    [ -x /usr/bin/python3 ] && /usr/bin/python3 -c '' 2>/dev/null \
      || die "the Command Line Tools are needed (git, python3): run xcode-select --install, then this again"
    TARGET=macos-arm64; PLATFORM="macOS $MACOS arm64"
    sha256() { shasum -a 256 "$@"; }
    sha_check() { shasum -a 256 -c --quiet "$@"; } ;;
  Linux)
    case "$(uname -m)" in
      aarch64|arm64) TARGET=linux-arm64 ;;
      x86_64|amd64) TARGET=linux-x86_64 ;;
      *) die "no Linux bundle for $(uname -m) (aarch64 and x86_64 only)" ;;
    esac
    for t in git tar sha256sum; do command -v "$t" >/dev/null || die "$t is missing: install it with your package manager"; done
    [ -x /usr/bin/python3 ] || die "/usr/bin/python3 is missing: install python3 with your package manager"
    [ -x /usr/bin/bwrap ] || die "/usr/bin/bwrap is missing: INIT needs bubblewrap 0.13.0 or newer (docs/platforms.md, Linux install)"
    PLATFORM="Linux $(uname -r) $(uname -m)"
    sha256() { sha256sum "$@"; }
    sha_check() { sha256sum -c --quiet "$@"; } ;;
  *) die "INIT bundles exist for macOS and Linux only (this is $(uname -s); on Windows use WSL2, docs/platforms.md)" ;;
esac
APP="${INIT_APP_DIR:-$HOME/.local/share/init}"
RH="${INIT_ROOT_HOME:-$HOME/.init-root}"
BIN_DIR="${INIT_BIN_DIR:-$HOME/.local/bin}" # where install.sh puts the init-agent command
PW_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)' 2>/dev/null || true)"
# ROOT-MSG (v0.5.1): `init-root <args>` as you type it in a new terminal, the form the Root prints (util::root_cmd; the
# same rcmd as install.sh's). init-root is not on PATH (only init-agent is), so a bare `init-root …` is "command not
# found"; a ROOT_HOME other than ~/.init-root needs INIT_ROOT_HOME= in front (init-root finds its home there).
rh_env() { # "INIT_ROOT_HOME=<ROOT_HOME> " when a new terminal would not find this ROOT_HOME (not ~/.init-root), else ""
  pw="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)' 2>/dev/null || true)"
  [ -n "$pw" ] && [ "$RH" = "$pw/.init-root" ] || printf 'INIT_ROOT_HOME=%s ' "$(shq "$RH")"
}
rcmd() { printf '%s%s' "$(rh_env)" "$(shq "$RH/bin/init-root")"; [ $# = 0 ] || printf ' %s' "$*"; }
# v0.5.3 (the post-release review of v0.5.2): `init-agent` is printed with its path too. A bare `init-agent …` is
# "command not found" on a fresh Mac, whose PATH lacks ~/.local/bin. The path is written `~/…` under HOME (the shell
# expands it there), else as shq prints it; scripts/install.sh, the launcher and the TUI print the same form.
tilde() { # <path>: as the owner types it
  h="${HOME%/}"
  case "$1" in "$h"/?*) [ -n "$h" ] && { printf '~/%s' "$(shq "${1#"$h"/}")"; return; } ;; esac
  shq "$1"
}
agent() { # the init-agent command of the INIT in ROOT_HOME: in the bin_dir its install.json records, else BIN_DIR
  b="$(/usr/bin/python3 -I -c 'import json,sys
try: print(json.load(open(sys.argv[1])).get("bin_dir") or "")
except Exception: pass' "$RH/install.json" 2>/dev/null || true)"
  [ -n "$b" ] && [ -x "$b/init-agent" ] || b="$BIN_DIR"
  tilde "$b/init-agent"
}
# The upgrade as the owner types it, with get.sh's arguments after --upgrade (already quoted). From 0.5.1:
# `init-agent upgrade` (up_agent; --from is its --bundle), which runs the get.sh this install kept. From 0.4.1 (no
# init-agent upgrade) or 0.5.0 (whose init-agent upgrade runs 0.5.0's get.sh, without 0.5.1's fixes): this get.sh,
# fetched again and run with the system's curl and sh (up_pipe; docs/guide.md "Upgrade"). INIT_ROOT_HOME goes to sh.
up_agent() { printf '%s%s upgrade' "$(rh_env)" "$(agent)"; [ $# = 0 ] || printf ' %s' "$*"; }
up_pipe() { # the get.sh this run came from (DIST0: --version points DIST_URL at an older release, whose get.sh may
  # not take the flags again adds)
  printf '/usr/bin/curl -q -fsSL %s | %s/bin/sh -s -- --upgrade' "$(shq "$DIST0/get.sh")" "$(rh_env)"
  [ $# = 0 ] || printf ' %s' "$*"
}
# --version V: the release's tags carry a label (v0.5.0-preview); `init-root --version` and the site say 0.5.0. So a
# plain V tries the tag vV, then vV-preview; a V with a label is that tag. The download base is the releases' (a
# DIST_URL ending in /latest/download); another INIT_DIST_URL is a mirror of one release, which --version leaves as it is.
TAGS=""; RV="${RELEASE#v}"
case "$RELEASE" in
  "") ;;
  *[!0-9A-Za-z.-]*) die "--version takes a version such as 0.5.0 or 0.5.0-preview" ;;
  *) case "$DIST_URL" in */latest/download) case "$RV" in *-*) TAGS="v$RV" ;; *) TAGS="v$RV v$RV-preview" ;; esac ;; esac ;;
esac
# The installed release ("" when its Root cannot say): `init-root --version` prints "init-root X.Y.Z" in every release.
installed() { "$RH/bin/init-root" --version 2>/dev/null | sed -n 's/^init-root \([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' | head -n 1; }
ver_lt() { /usr/bin/python3 -I -c 'import sys; v = lambda s: tuple(int(x) for x in s.split("."))
sys.exit(0 if v(sys.argv[1]) < v(sys.argv[2]) else 1)' "$1" "$2"; }
# <a> <b>: a is below b by SemVer precedence: the core, then a pre-release label below no label, labels field by field
# (numeric fields numerically and below alphanumeric ones); build metadata ignored. The Root's upgrade::precedence (v0.5.3)
ver_older() { /usr/bin/python3 -I -c 'import sys
def k(s):
    c, _, l = s.split("+")[0].partition("-")
    f = tuple((0, len(x), x) if x.isascii() and x.isdigit() else (1, 0, x) for x in l.split(".")) if l else ()
    return (tuple(int(x) for x in c.split(".")), not l, f)
sys.exit(0 if k(sys.argv[1]) < k(sys.argv[2]) else 1)' "$1" "$2"; }
installed_release() { # install.json `release.version` (with its label), else the core `init-root --version` gave (HAVE)
  /usr/bin/python3 -I -c 'import json, sys
try: v = json.load(open(sys.argv[1])).get("release", {}).get("version", "")
except Exception: v = ""
print(v if isinstance(v, str) else "")' "$RH/install.json" 2>/dev/null | grep -E '^[0-9]+[.][0-9]+[.][0-9]+([-+][0-9A-Za-z.+-]*)?$' || printf '%s\n' "$HAVE"
}
tname() { # a bundle target as a platform name
  case "$1" in
    macos-arm64) echo "macOS 14+ on Apple silicon ($1)" ;; linux-arm64) echo "Linux aarch64 ($1)" ;;
    linux-x86_64) echo "Linux x86_64 ($1)" ;; *) echo "$1" ;;
  esac
}
page() { # the releases page of DIST_URL (a /latest/download base: the releases; a /download/<tag> base: that release)
  pg="${DIST_URL%/latest/download}"; case "$pg" in */download/v*) pg="${pg%/download/*}/tag/${pg##*/}" ;; esac
  printf '%s' "$pg"
}
# INIT installed in another ROOT_HOME (v0.5.2; the post-release review of v0.5.1). An owner who installed with
# INIT_ROOT_HOME=X and then pastes the one-liner without it got a second install: install.sh rewrote the one service
# file (the LaunchAgent dev.init.root, or the systemd user unit init-root.service) for the new home and overwrote the
# init-agent launcher, which left the INIT in X, and everything it had become, behind. install.sh checks the same.
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
# v0.5.3 (the post-release review of v0.5.2): --upgrade looks too. Run without INIT_ROOT_HOME on a custom-home install,
# it said "not installed … run this without --upgrade", and only that second run printed the line that works.
if [ $UNPACK_ONLY = 0 ] && [ ! -f "$RH/install.json" ]; then
  OTHER="$(other_home "$(svc_file)")"
  if [ -n "$OTHER" ]; then
    NEW="$RH"; RH="$(printf '%s\n' "$OTHER" | sed -n 1p)" # every line below names that INIT's home
    if [ $UPGRADE = 1 ]; then
      ELSEWHERE="in $RH: $(printf '%s\n' "$OTHER" | sed -n 2p). It is not in $NEW, the home this upgrade looked in (INIT_ROOT_HOME was not set)"
    else
      ELSEWHERE="in $RH: $(printf '%s\n' "$OTHER" | sed -n 2p). A second install, in $NEW, would take that over and leave this one behind"
      SECOND="
(For a separate, second INIT instead: give it its own INIT_ROOT_HOME, INIT_HOME and INIT_BIN_DIR, without the service: sh -s -- -- --no-service.)"
    fi
  fi
fi
# Before it advises an upgrade, get.sh checks that the latest release has a bundle for this machine: a Linux aarch64
# install of 0.4.1 was told to upgrade, and the upgrade then stopped on a bundle that does not exist (v0.5.2).
latest_without_target() { # the latest release's version when its latest.json lists no bundle for TARGET; else nothing
  # (also when latest.json cannot be fetched or read: then the upgrade itself says what it finds)
  lj="$(mktemp "${TMPDIR:-/tmp}/init-get.XXXXXX")" || return 0
  if curl -q -fsSL --proto '=https' --tlsv1.2 --retry 2 --max-time 60 "$DIST_URL/latest.json" -o "$lj" 2>/dev/null; then
    lv="$(sed -n 's/^ *"version": "\([0-9A-Za-z.-]*\)".*/\1/p' "$lj" | head -n 1)"
    if [ -n "$lv" ] && ! grep -q "^ *\"sha256_$TARGET\": \"[0-9a-f]\{64\}\"" "$lj" \
      && ! { grep -q "^ *\"target\": \"$TARGET\"" "$lj" && grep -q '^ *"sha256": "[0-9a-f]\{64\}"' "$lj"; }; then
      printf '%s' "$lv"
    fi
  fi
  rm -f -- "$lj"
}
UNINSTALL='f="$(mktemp)" && curl -fsSL https://raw.githubusercontent.com/nagisanzenin/INIT-Agent/main/scripts/uninstall.sh -o "$f" && '"$(rh_env)"'bash "$f"'
too_old() { # <version>: the upgrade starts at 0.4.1, the oldest release it was tested from
  die "INIT $1 is installed ${ELSEWHERE:-in $RH}. An upgrade from a release before 0.4.1 is not supported: uninstall it first, then run this again. The uninstaller moves INIT, its record and your provider key to the Trash (nothing is deleted):
  $UNINSTALL$SECOND"
}
if [ $UPGRADE = 1 ]; then
  if [ -n "$ELSEWHERE" ]; then # the INIT this upgrade is for, in another ROOT_HOME: its line, with that home filled in
    HAVE="$(installed)"
    [ -z "$HAVE" ] || ! ver_lt "$HAVE" 0.4.1 || too_old "$HAVE"
    UPA=""; UPB="" # this run's arguments after --upgrade, quoted: for the pipe, and for init-agent upgrade (--bundle)
    [ -z "$RELEASE" ] || { UPA="$UPA --version $(shq "$RELEASE")"; UPB="$UPB --version $(shq "$RELEASE")"; }
    [ -z "$FROM" ] || { UPA="$UPA --from $(shq "$FROM")"; UPB="$UPB --bundle $(shq "$FROM")"; }
    [ -z "$WANT" ] || { UPA="$UPA --sha256 $(shq "$WANT")"; UPB="$UPB --sha256 $(shq "$WANT")"; }
    [ $ALLOW_UNSIGNED = 0 ] || { UPA="$UPA --allow-unsigned"; UPB="$UPB --allow-unsigned"; }
    [ $ALLOW_EXPIRED = 0 ] || { UPA="$UPA --allow-expired"; UPB="$UPB --allow-expired"; }
    if [ $# -gt 0 ]; then UPA="$UPA --"; UPB="$UPB --"; for a in "$@"; do UPA="$UPA $(shq "$a")"; UPB="$UPB $(shq "$a")"; done; fi
    if [ -n "$HAVE" ] && ver_lt "$HAVE" 0.5.1; then # 0.4.1 and 0.5.0: this get.sh, piped, as the owner ran it
      die "INIT $HAVE is installed $ELSEWHERE. Upgrade it with its home given to sh:
  $(up_pipe ${UPA:+"${UPA# }"})"
    fi
    die "INIT ${HAVE:+$HAVE }is installed $ELSEWHERE. Upgrade it with:
  $(up_agent ${UPB:+"${UPB# }"})
(or, as you ran it, with its home given to sh: $(up_pipe ${UPA:+"${UPA# }"}))"
  fi
  [ -f "$RH/install.json" ] || die "INIT is not installed in $RH (no install.json): run this without --upgrade to install it"
  [ -d "$RH/cache" ] && [ ! -L "$RH/cache" ] || die "$RH/cache is not a directory"
  HAVE="$(installed)"
  [ -z "$HAVE" ] || ! ver_lt "$HAVE" 0.4.1 || too_old "$HAVE"
  APP="$RH/cache/upgrade" # the download, its check and the unpacked release live where the zone cannot write
  case "$FROM" in
    ""|http://*|https://*) ;;
    *) # a local bundle: its sha256 comes from the owner, never from a file beside it, which the agent can write
      [ -n "$WANT" ] || {
        FILE="$(basename "$FROM")"; LV="$(printf '%s\n' "$FILE" | sed -n "s/^init-v\([0-9A-Za-z.-]*\)-$TARGET\.tar\.gz\$/\1/p")"
        HEX='<the 64 hex digits>'; ALT=""
        if [ -n "$HAVE" ] && ver_lt "$HAVE" 0.5.1; then # 0.4.1 and 0.5.0: this get.sh again, piped (the owner may have no file of it)
          RUN="$(up_pipe --from "$(shq "$FROM")" --sha256 "$HEX")"; RUNV="$(up_pipe --version "$LV")"
        else
          RUN="$(up_agent --bundle "$(shq "$FROM")" --sha256 "$HEX")"; RUNV="$(up_agent --version "$LV")"
          [ -n "$HAVE" ] || ALT="
(from INIT 0.4.1 or 0.5.0: $(up_pipe --from "$(shq "$FROM")" --sha256 "$HEX"))"
        fi
        die "an upgrade from a local bundle needs its sha256 on the command line. A $FILE.sha256 beside it is not used: with
full access the agent can write that folder, so a checksum from there proves nothing. Copy the sha256 that the release
page lists for $FILE (https://github.com/nagisanzenin/INIT-Agent/releases${LV:+/tag/v$LV}), then run:
  $RUN$ALT${LV:+
Or let INIT download and check the release itself:
  $RUNV}"
      } ;;
  esac
elif [ -f "$RH/install.json" ] && [ $UNPACK_ONLY = 0 ]; then
  HAVE="$(installed)"
  [ -z "$HAVE" ] || ! ver_lt "$HAVE" 0.4.1 || too_old "$HAVE"
  LV="$(latest_without_target)"
  if [ -n "$LV" ]; then
    UP="The latest release (INIT $LV) has no bundle yet for this machine, $(tname "$TARGET"), so an upgrade has nothing to install here: the INIT you have keeps working as it is. When a $TARGET bundle is released ($(page)), upgrade it, which keeps what INIT has become"
  else
    UP="Upgrade it instead, which keeps what INIT has become"
  fi
  if [ -z "$HAVE" ]; then
    die "INIT is already installed ${ELSEWHERE:-in $RH}, and its version could not be read. $UP. From 0.5.1: $(up_agent). From 0.4.1 or 0.5.0: $(up_pipe). Before 0.4.1, uninstall it first: $UNINSTALL$SECOND"
  elif ver_lt "$HAVE" 0.5.1; then
    if ver_lt "$HAVE" 0.5.0; then WHY="0.4.1 has no init-agent upgrade"; else WHY="0.5.0's own init-agent upgrade runs 0.5.0's get.sh, which 0.5.1 fixed"; fi
    die "INIT $HAVE is already installed ${ELSEWHERE:-in $RH}. $UP. Run this once, from your own shell ($WHY):
  $(up_pipe)$SECOND"
  else
    die "INIT $HAVE is already installed ${ELSEWHERE:-in $RH}. $UP:
  $(up_agent)$SECOND"
  fi
fi
mkdir -p "$APP"
say "INIT installer · $PLATFORM · $APP"
if [ -n "$RELEASE" ] && { [ -n "$FROM" ] || [ -z "$TAGS" ]; }; then # --version picks a tag of the releases' download base only
  if [ -n "$FROM" ]; then printf 'get.sh: note: --version %s is ignored: --from names the bundle\n' "$RELEASE" >&2
  else printf 'get.sh: note: --version %s is ignored: INIT_DIST_URL (%s) does not end in /latest/download, so it is one release\n' "$RELEASE" "$DIST_URL" >&2; fi
fi

# ---------------------------------------------------------------- download (or a local file)
TMP="$(mktemp -d "$APP/.get.XXXXXX")"
trap 'rm -rf -- "$TMP"' EXIT INT TERM
# -q first: no ~/.curlrc (the agent can write it in full access, and a config line there can turn off the TLS check)
fetch() { curl -q -fsSL --proto '=https' --tlsv1.2 --retry 2 "$1" -o "$2"; }
no_bundle() { # <url> <http code>: the release has no bundle for this machine (until v0.5.0: every Linux x86_64 host)
  LISTED=""
  [ ! -f "$TMP/latest.json" ] || LISTED="$(sed -n 's/^ *"sha256_\([a-z0-9_-]*\)": .*/\1/p; s/^ *"target": "\([a-z0-9_-]*\)".*/\1/p' \
    "$TMP/latest.json" | sort -u)"
  case " $(echo $LISTED) " in *" $TARGET "*) # listed but missing: an upload in progress, or a broken release
    die "$1 answered HTTP $2, although latest.json lists a $TARGET bundle; the release may still be uploading. Try again in a few minutes, or use --from <bundle.tar.gz>" ;;
  esac
  {
    printf 'get.sh: there is no INIT bundle for this machine (%s, target %s) in this release:\n' "$PLATFORM" "$TARGET"
    printf '  %s answered HTTP %s.\n' "$1" "$2"
    if [ -n "$LISTED" ]; then
      printf '  This release%s has bundles for:\n' "${LVER:+ (INIT $LVER)}"
      for t in $LISTED; do printf '    %s\n' "$(tname "$t")"; done
    fi
    if [ $UPGRADE = 1 ]; then # an upgrade: the install stays as it is, and the way on is a later release
      printf '  Nothing was changed: the INIT you have keeps working as it is. Run this upgrade again when a %s\n' "$TARGET"
      printf '  bundle is released: %s\n' "$(page)"
    else
      printf '  What you can do:\n'
      printf '    - install on %s, or\n' "$([ -n "$LISTED" ] && echo 'one of the platforms above' || echo 'a platform the release page lists')"
      printf '    - build from source here: clone the repository and run scripts/install.sh (Rust 1.97.1; on Linux also\n'
      printf '      bubblewrap >= 0.13, docs/platforms.md section 3.4.0)%s, or\n' \
        "$([ "$TARGET" = linux-x86_64 ] && echo '; on Linux x86_64 this compiles but is not tested yet')"
      printf '    - run this installer again when a %s bundle is released: %s\n' "$TARGET" "$(page)"
    fi
  } >&2
  exit 1
}
# ---------------------------------------------------------------- the signature (v0.5.3; docs/release-signing.md)
RISK_UNSIGNED="the bundle is then checked by its sha256 alone, which comes from the same server as the bundle: that proves the download is intact, not that the INIT maintainer made it"
again() { # <flag>: this run with <flag> added, as the owner types it: its arguments (AGAIN_*), and on a fresh install
  # the homes it was given (on an upgrade they come from the install)
  if [ $UPGRADE = 0 ]; then
    printf '/usr/bin/curl -q -fsSL %s | %s%s%s%s/bin/sh -s -- %s' "$(shq "$DIST0/get.sh")" "$(rh_env)" \
      "${INIT_HOME:+INIT_HOME=$(shq "$INIT_HOME") }" "${INIT_BIN_DIR:+INIT_BIN_DIR=$(shq "$INIT_BIN_DIR") }" \
      "${INIT_APP_DIR:+INIT_APP_DIR=$(shq "$INIT_APP_DIR") }" "${AGAIN_P# }${AGAIN_P:+ }$1$AGAIN_I"
  elif [ -n "${HAVE:-}" ] && ! ver_lt "$HAVE" 0.5.3 && [ $UNPACK_ONLY = 0 ]; then up_agent "${AGAIN_A# }${AGAIN_A:+ }$1$AGAIN_I"
  else up_pipe "${AGAIN_P# }${AGAIN_P:+ }$1$AGAIN_I"; fi
}
no_verifier() { # <why>: get.sh has no way to check the signature here
  case "$OS" in
    Darwin) HOW="macOS ships it as /usr/bin/ssh-keygen; put /usr/bin on your PATH" ;;
    *) HOW="install OpenSSH's client (Debian, Ubuntu: sudo apt install openssh-client; Fedora, RHEL: sudo dnf install openssh-clients; Arch: sudo pacman -S openssh; Alpine: sudo apk add openssh-keygen)" ;;
  esac
  die "cannot check this release's signature: $1. get.sh checks it with ssh-keygen -Y verify (OpenSSH 8.1 or newer): $HOW, then run this again. Nothing was installed. To install without the check, knowing that $RISK_UNSIGNED, add --allow-unsigned:
  $(again --allow-unsigned)"
}
sig_fetch() { # <base URL>: the release's SHA256SUMS and SHA256SUMS.sig, from beside the bundle, into TMP
  for f in SHA256SUMS SHA256SUMS.sig; do
    CODE=0
    HTTP="$(curl -q -fsL --proto '=https' --tlsv1.2 --retry 2 -w '%{http_code}' "$1/$f" -o "$TMP/$f")" || CODE=$?
    case "$CODE/$HTTP" in
      0/*) ;;
      22/404|22/410) die "this release${LVER:+ (INIT $LVER)} is not signed: $1/$f answered HTTP $HTTP. INIT releases are signed from 0.5.3 on (SHA256SUMS and SHA256SUMS.sig beside the bundles). Nothing was installed. To install it anyway, knowing that $RISK_UNSIGNED, add --allow-unsigned:
  $(again --allow-unsigned)" ;;
      *) die "cannot fetch $1/$f (curl exit $CODE, HTTP ${HTTP:-none}); check the connection" ;;
    esac
  done
}
expired() { # the message for a signed SHA256SUMS past its expiry date
  printf 'the signed SHA256SUMS of INIT %s expired on %s (today is %s, UTC). An expired signature is what a replayed old release looks like (a server, or a takeover of the release page, offering an old release with a known flaw as the newest); it is also what a release page with nothing newer since %s looks like. Nothing was installed. Check %s for a newer release; to install this one anyway, add --allow-expired:\n  %s' \
    "$SIGNED" "$EXP" "$(date -u +%Y-%m-%d)" "$EXP" "$(page)" "$(again --allow-expired)"
}
SIGNED=""; EXP=""; SUMSHA=""; KEYFP=""; BY="" # the release, expiry, sha256 and key a verified SHA256SUMS names
sig_by_root() { # the installed Root (0.5.3 or later) checks SHA256SUMS, its signature and the bundle with the key
  # compiled into it; false when that Root has no `release` command (an install before 0.5.3)
  K="$("$RH/bin/init-root" --root-home "$RH" release keys --json 2>/dev/null)" || return 1
  case "$K" in *'"keys":[]'*) die "the installed INIT pins no release key (a development build, made before the release key existed), so it can verify no release. Nothing was changed. To upgrade anyway, knowing that $RISK_UNSIGNED, add --allow-unsigned:
  $(again --allow-unsigned)" ;; esac
  AE=""; [ $ALLOW_EXPIRED = 0 ] || AE=--allow-expired
  CODE=0
  OUT="$("$RH/bin/init-root" --root-home "$RH" release verify --sums "$TMP/SHA256SUMS" --sig "$TMP/SHA256SUMS.sig" --file "$TGZ" $AE 2>"$TMP/verify.err")" || CODE=$?
  WHY="$(sed 's/^init-root release: //' "$TMP/verify.err" | head -n 3 | sed '$ s/\.$//')"
  case $CODE in
    0) [ -z "$WHY" ] || printf 'get.sh: %s\n' "$WHY" >&2 ;; # its note: the installed release, expired
    5) SIGNED="$(sed -n 's/.*of INIT \([0-9A-Za-z.+-]*\) expired on \([0-9-]*\) .*/\1/p' "$TMP/verify.err")"
       EXP="$(sed -n 's/.* expired on \([0-9-]*\) .*/\1/p' "$TMP/verify.err")"
       die "$(expired)" ;;
    *) die "the installed INIT refused this release: $WHY. Nothing was changed." ;;
  esac
  SIGNED="$(printf '%s' "$OUT" | sed -n 's/.*"version":"\([0-9A-Za-z.+-]*\)".*/\1/p')"
  EXP="$(printf '%s' "$OUT" | sed -n 's/.*"expires":"\([0-9-]*\)".*/\1/p')"
  SUMSHA="$(printf '%s' "$OUT" | sed -n 's/.*"sha256":"\([0-9a-f]\{64\}\)".*/\1/p')"
  KEYFP="$(printf '%s' "$OUT" | sed -n 's/.*"key":"\(SHA256:[A-Za-z0-9+/]*\)".*/\1/p')"
  [ -n "$SIGNED" ] && [ -n "$SUMSHA" ] || die "the installed INIT's check printed no release and sha256: $OUT"
  # only with the flag: the installed release itself, expired, passes without it (its note is printed above)
  [ $ALLOW_EXPIRED = 0 ] || case "$OUT" in *'"expired":true'*) printf 'get.sh: warning: --allow-expired: the signed SHA256SUMS of INIT %s expired on %s; installing it anyway, as you asked\n' "$SIGNED" "$EXP" >&2 ;; esac
  BY="the installed INIT's own key"
}
sig_by_ssh() { # ssh-keygen -Y verify with this get.sh's RELEASE_KEYS (a fresh install, or an upgrade of an INIT before
  # 0.5.3, whose Root cannot check it), then the version, the expiry and this bundle's line, read here
  [ -n "$RELEASE_KEYS" ] || die "this get.sh pins no release key (a copy made before the release key existed), so it can verify no download. Nothing was installed. Use the get.sh of a published release, or, knowing that $RISK_UNSIGNED, add --allow-unsigned:
  $(again --allow-unsigned)"
  command -v ssh-keygen >/dev/null 2>&1 || no_verifier "ssh-keygen is not installed"
  printf '%s\n' "$RELEASE_KEYS" | sed -n 's|^\(ssh-ed25519 [A-Za-z0-9+/=]*\).*|init-release namespaces="init-release" \1|p' >"$TMP/allowed_signers"
  if ! ssh-keygen -Y verify -f "$TMP/allowed_signers" -I init-release -n init-release -s "$TMP/SHA256SUMS.sig" \
    <"$TMP/SHA256SUMS" >"$TMP/verify.out" 2>&1; then
    case "$(cat "$TMP/verify.out")" in
      *"illegal option"*|*"unknown option"*|*"invalid option"*) no_verifier "this ssh-keygen has no -Y verify (OpenSSH before 8.1)" ;;
    esac
    die "the signature of SHA256SUMS does not verify with INIT's release key (ssh-keygen: $(head -n 2 "$TMP/verify.out" | tr '\n' ' ')): the files on the server are not the ones the maintainer signed, or they were signed with another key. Nothing was installed. Please report it: https://github.com/nagisanzenin/INIT-Agent/issues"
  fi
  KEYFP="$(sed -n 's/.* key \(SHA256:[A-Za-z0-9+/]*\).*/\1/p' "$TMP/verify.out" | head -n 1)"
  [ "$(sed -n 1p "$TMP/SHA256SUMS")" = "# INIT release SHA256SUMS v1" ] \
    || die "SHA256SUMS is in a format this get.sh does not read (from a newer release?): use the current get.sh"
  SIGNED="$(sed -n '2s/^# version: \([0-9A-Za-z.+-]*\)$/\1/p' "$TMP/SHA256SUMS")"
  EXP="$(sed -n '3s/^# expires: \([0-9]\{4\}-[0-9][0-9]-[0-9][0-9]\)$/\1/p' "$TMP/SHA256SUMS")"
  [ -n "$SIGNED" ] && [ -n "$EXP" ] || die "the signed SHA256SUMS has no valid version or expires line"
  SUMSHA="$(awk -v f="$FILE" 'NR > 3 && NF == 2 && $2 == f && $1 ~ /^[0-9a-f]+$/ && length($1) == 64 { print $1 }' "$TMP/SHA256SUMS")"
  [ -n "$SUMSHA" ] && [ "$(printf '%s\n' "$SUMSHA" | wc -l | tr -d ' ')" = 1 ] || die "the signed SHA256SUMS of INIT $SIGNED does not list $FILE once"
  STATE=newer
  if [ $UPGRADE = 1 ] && [ -n "${HAVE:-}" ]; then # never down; only the installed release itself (the same version
    # string) installs nothing new, so its expiry does not count; another label of the same core is another release
    HAVEV="$(installed_release)"
    if ver_older "$SIGNED" "$HAVEV"; then
      die "this signed release is INIT $SIGNED, older than the installed $HAVEV: INIT does not downgrade. An older release offered as the newest is what a replayed download looks like. Nothing was changed."
    elif [ "$SIGNED" = "$HAVEV" ]; then STATE=current; fi
  fi
  if [ "$(date -u +%Y%m%d)" -gt "$(printf '%s' "$EXP" | tr -d -)" ] && [ $STATE != current ]; then
    [ $ALLOW_EXPIRED = 1 ] || die "$(expired)"
    printf 'get.sh: warning: --allow-expired: the signed SHA256SUMS of INIT %s expired on %s; installing it anyway, as you asked\n' "$SIGNED" "$EXP" >&2
  fi
  BY="ssh-keygen"
}
signature() { # <base URL>: fetch and verify; sets SIGNED, EXP, SUMSHA, KEYFP, BY
  sig_fetch "$1"
  if [ $UPGRADE = 0 ] || ! sig_by_root; then sig_by_ssh; fi
}

LOCAL=0 # 1: --from a local file, checked by its sha256 only
case "$FROM" in
  http://*) die "refusing a plain-http URL: $FROM" ;;
  https://*|"")
    command -v curl >/dev/null || die "curl is missing"
    if [ -z "$FROM" ] && [ -n "$TAGS" ]; then # --version: the first of its tags that the releases have
      BASE="${DIST_URL%/latest/download}"; DIST_URL=""
      for t in $TAGS; do
        CODE=0 # silent (no -S): a 404 for the first tag is expected; the errors are ours, below
        HTTP="$(curl -q -fsL --proto '=https' --tlsv1.2 --retry 2 -w '%{http_code}' "$BASE/download/$t/latest.json" -o "$TMP/latest.json")" || CODE=$?
        case "$CODE/$HTTP" in
          0/*) DIST_URL="$BASE/download/$t"; break ;;
          22/404) ;;
          *) die "cannot fetch $BASE/download/$t/latest.json (curl exit $CODE, HTTP ${HTTP:-none}); check the connection, or use --from <bundle.tar.gz>" ;;
        esac
      done
      if [ -z "$DIST_URL" ]; then
        case "$TAGS" in
          *" "*) die "there is no INIT release $RV: neither the tag ${TAGS%% *} nor ${TAGS#* } exists (each answered HTTP 404 for latest.json). The releases: $BASE" ;;
          *) die "there is no INIT release $RV: the tag $TAGS does not exist (HTTP 404 for latest.json). The releases: $BASE" ;;
        esac
      fi
    elif [ -z "$FROM" ]; then
      fetch "$DIST_URL/latest.json" "$TMP/latest.json" || die "cannot fetch $DIST_URL/latest.json (or use --from <bundle.tar.gz>)"
    fi
    if [ -z "$FROM" ]; then
      LVER="$(sed -n 's/^ *"version": "\([0-9A-Za-z.-]*\)".*/\1/p' "$TMP/latest.json")"
      LTARGET="$(sed -n 's/^ *"target": "\([^"]*\)".*/\1/p' "$TMP/latest.json")"
      [ -n "$LVER" ] || die "$DIST_URL/latest.json names no version"
      FILE="init-v$LVER-$TARGET.tar.gz"
      # latest.json carries "sha256_<target>" per target (and "sha256" for its "target"); a target it does not
      # list takes the checksum from the .sha256 next to the bundle
      [ -n "$WANT" ] || WANT="$(sed -n "s/^ *\"sha256_$TARGET\": \"\([0-9a-f]\{64\}\)\".*/\1/p" "$TMP/latest.json")"
      [ -n "$WANT" ] || [ "$LTARGET" != "$TARGET" ] || WANT="$(sed -n 's/^ *"sha256": "\([0-9a-f]\{64\}\)".*/\1/p' "$TMP/latest.json")"
      [ -z "$WANT" ] || [ -n "$WANT_FROM" ] || WANT_FROM=latest.json
      FROM="$DIST_URL/$FILE"
    fi
    FILE="$(basename "$FROM")"
    say "  downloading $FILE"
    CODE=0
    HTTP="$(curl -q -fL --progress-bar --proto '=https' --tlsv1.2 --retry 2 -w '%{http_code}' "$FROM" -o "$TMP/$FILE")" || CODE=$?
    case "$CODE/$HTTP" in
      0/*) ;;
      22/404|22/410) no_bundle "$FROM" "$HTTP" ;;
      *) die "download failed: $FROM (curl exit $CODE, HTTP ${HTTP:-none}); check the connection, or use --from <bundle.tar.gz>" ;;
    esac
    [ -n "$WANT" ] || { fetch "$FROM.sha256" "$TMP/$FILE.sha256" && WANT="$(cut -d' ' -f1 "$TMP/$FILE.sha256")" && WANT_FROM="$FILE.sha256"; } || true
    TGZ="$TMP/$FILE" ;;
  *)
    [ -f "$FROM" ] || die "no such bundle: $FROM"
    LOCAL=1
    SRC="$(cd "$(dirname "$FROM")" && pwd -P)/$(basename "$FROM")"
    FILE="$(basename "$FROM")"
    # a fresh install (no INIT, so no agent yet) may take the .sha256 beside the bundle; an upgrade stopped above without --sha256
    [ -n "$WANT" ] || [ $UPGRADE = 1 ] || { [ -f "$SRC.sha256" ] && WANT="$(cut -d' ' -f1 "$SRC.sha256")"; } || true
    # The bytes checked are the bytes unpacked: a copy in this run's own directory (in an upgrade under
    # ROOT_HOME/cache/upgrade/, where the agent cannot write), so the file at $SRC cannot be swapped between the two.
    cp -- "$SRC" "$TMP/$FILE" || die "cannot copy $SRC into $TMP"
    TGZ="$TMP/$FILE" ;;
esac

if [ $LOCAL = 0 ] && [ $ALLOW_UNSIGNED = 1 ]; then
  printf 'get.sh: warning: --allow-unsigned: the signature is not checked; %s (download base: %s).\n' "$RISK_UNSIGNED" "${FROM%/*}" >&2
elif [ $LOCAL = 0 ]; then
  signature "${FROM%/*}"
  [ -z "${LVER:-}" ] || [ "$LVER" = "$SIGNED" ] || die "latest.json names INIT $LVER, and the signed SHA256SUMS beside it INIT $SIGNED: the release page is inconsistent. Nothing was installed."
  if [ -n "$WANT" ] && [ "$WANT" != "$SUMSHA" ]; then
    if [ "$WANT_FROM" = owner ]; then die "the sha256 you gave (--sha256 $WANT) is not the one the signed SHA256SUMS lists for $FILE ($SUMSHA). Nothing was installed."; fi
    die "$WANT_FROM lists sha256 $WANT for $FILE, and the signed SHA256SUMS $SUMSHA: the release page is inconsistent, or the bundle was swapped. Nothing was installed."
  fi
  WANT="$SUMSHA"
  ok "signature: INIT $SIGNED, signed by INIT's release key ${KEYFP:-?}, valid until $EXP (checked with $BY)"
fi

# ---------------------------------------------------------------- verify
case "$WANT" in
  "") die "no checksum to verify $FILE against: pass --sha256 HEX, or keep $FILE.sha256 next to it" ;;
  *[!0-9a-f]*) die "the expected sha256 is not 64 lowercase hex digits: $WANT" ;;
  *) [ ${#WANT} = 64 ] || die "the expected sha256 is not 64 hex digits: $WANT" ;;
esac
GOT="$(sha256 "$TGZ" | cut -d' ' -f1)"
[ "$GOT" = "$WANT" ] || die "checksum mismatch for $FILE: got $GOT, expected $WANT. Download it again."
ok "checksum matches ($(printf '%.16s' "$GOT")…)"

# ---------------------------------------------------------------- unpack
mkdir -p "$TMP/x"
tar -xzf "$TGZ" -C "$TMP/x" || die "cannot unpack $FILE"
TOP="$(ls "$TMP/x")"
[ "$(printf '%s\n' "$TOP" | wc -l | tr -d ' ')" = 1 ] && [ -f "$TMP/x/$TOP/VERSION" ] || die "$FILE is not an INIT bundle (one top directory with VERSION)"
case "$TOP" in *-"$TARGET") ;; *) die "$FILE is not a $TARGET bundle (its top directory is $TOP); download the one for this machine" ;; esac
VERSION="$(cat "$TMP/x/$TOP/VERSION")"
case "$VERSION" in *[!0-9A-Za-z.-]*|"") die "bad VERSION in the bundle: $VERSION" ;; esac
[ -z "$SIGNED" ] || [ "$VERSION" = "$SIGNED" ] || die "$FILE holds INIT $VERSION, but its signed SHA256SUMS is for INIT $SIGNED. Nothing was installed."
(cd "$TMP/x/$TOP" && sha_check SHA256SUMS) || die "the unpacked files do not match the bundle's SHA256SUMS"
DEST="$APP/$VERSION"
if [ -e "$DEST" ]; then
  if cmp -s "$DEST/SHA256SUMS" "$TMP/x/$TOP/SHA256SUMS" && (cd "$DEST" && sha_check SHA256SUMS); then
    ok "INIT $VERSION is already unpacked in $DEST (verified)"
  elif [ $UPGRADE = 1 ]; then # our own staging copy under ROOT_HOME/cache/upgrade/: replaced (exact path)
    chmod -R u+w "$DEST" && rm -rf -- "$DEST" && mv "$TMP/x/$TOP" "$DEST" && chmod -R a-w "$DEST"
    ok "unpacked INIT $VERSION in $DEST (replacing an older copy of it)"
  else
    die "$DEST exists and differs from this bundle; move it away (for example to the Trash), then run this again"
  fi
else
  mv "$TMP/x/$TOP" "$DEST"
  chmod -R a-w "$DEST" # read-only: the code INIT runs as you (docs/ux-v1.md §3.2.3 step 6)
  ok "unpacked INIT $VERSION in $DEST"
fi
say "  $(( $(date +%s) - T0 )) s so far"
[ $UNPACK_ONLY = 0 ] || { say "Unpacked only. Install: $DEST/scripts/install.sh --yes"; exit 0; }
if [ $UPGRADE = 1 ]; then
  say ""
  "$DEST/scripts/install.sh" --upgrade --yes "$@" || exit $?
  exit 0
fi

# ---------------------------------------------------------------- the Rust toolchain INIT rebuilds itself with
RUST_TC=1.97.1
if [ $NO_RUST = 0 ]; then
  RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"; CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"; export RUSTUP_HOME CARGO_HOME
  CARGO_BIN="$CARGO_HOME/bin"
  RUSTUP="$(command -v rustup 2>/dev/null || true)"
  [ -n "$RUSTUP" ] || { [ -x "$CARGO_BIN/rustup" ] && RUSTUP="$CARGO_BIN/rustup"; } || true
  if [ -z "$RUSTUP" ]; then
    say "  ..  installing Rust $RUST_TC with rustup (self-rewrite needs it; about 1-2 min; --no-rust skips it)"
    curl -q --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --quiet --no-modify-path --profile minimal --component clippy --default-toolchain "$RUST_TC" --target wasm32-wasip2 \
        >"$TMP/rustup.log" 2>&1 \
      || { tail -5 "$TMP/rustup.log" >&2; die "installing Rust failed; rerun with --no-rust to install without self-rewrite (T0 only), or install rustup yourself"; }
    RUSTUP="$CARGO_BIN/rustup"
  else
    say "  ..  making sure rustup has Rust $RUST_TC with wasm32-wasip2 (your default toolchain is left alone)"
    "$RUSTUP" toolchain install "$RUST_TC" --profile minimal --component clippy --target wasm32-wasip2 --no-self-update >/dev/null 2>&1 \
      || die "rustup could not install toolchain $RUST_TC; rerun with --no-rust to skip self-rewrite"
  fi
  "$RUSTUP" target add --toolchain "$RUST_TC" wasm32-wasip2 >/dev/null 2>&1 || die "rustup could not add wasm32-wasip2"
  # the gate's G0 runs cargo clippy; a toolchain installed earlier with the minimal profile lacks it
  "$RUSTUP" component add --toolchain "$RUST_TC" clippy >/dev/null 2>&1 || die "rustup could not add clippy (the gate's G0 runs it)"
  ok "Rust $("$RUSTUP" run "$RUST_TC" rustc -V 2>/dev/null | awk '{print $2}') with wasm32-wasip2 and clippy: INIT can rewrite its tools (T1) and its core (T2)"
  # install.sh records the toolchain rustup resolves; pin it to the one above for this run only.
  PATH="$(dirname "$RUSTUP"):$PATH"; RUSTUP_TOOLCHAIN="$RUST_TC"; export PATH RUSTUP_TOOLCHAIN
else
  say "  --  no Rust (--no-rust): INIT will rewrite only its skills and prompts (T0). Later: install Rust $RUST_TC, then, with the Root stopped, $(rcmd toolchain --set) (the install's last lines give the steps)"
fi

# ---------------------------------------------------------------- install (non-interactive, defaults)
say ""
"$DEST/scripts/install.sh" --yes "$@" || die "install failed; fix the cause above, then run: $DEST/scripts/install.sh --yes $*"
RH="${INIT_ROOT_HOME:-$HOME/.init-root}"
BIN="${INIT_BIN_DIR:-$HOME/.local/bin}"
cat <<EOF

INIT $VERSION installed in $(( $(date +%s) - T0 )) s (after the download).
Start it:
  $(tilde "$BIN/init-agent")
The first run asks for the provider, your key (hidden) and the model, then opens the chat.
From a shell instead: $(rcmd keys set deepseek)   (the key is never echoed or logged)
EOF
case ":$PATH:" in *":$BIN:"*) ;; *) say "($BIN is not on your PATH; for this terminal: export PATH=\"$BIN:\$PATH\")" ;; esac
