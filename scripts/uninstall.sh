#!/bin/bash
# Uninstall INIT: stop it, unload the service, and MOVE its homes to the Trash. Nothing is deleted: you empty
# the Trash yourself (or put things back from it).
#
#   scripts/uninstall.sh [--apparmor]
#
# Paths (environment, as for install.sh): INIT_ROOT_HOME (default ~/.init-root), INIT_HOME (default: the one
# ROOT_HOME/install.json names, else ~/.init), INIT_BIN_DIR (default: the one install.json names, else ~/.local/bin).
# The Trash is ~/.Trash on macOS and the freedesktop.org Trash (${XDG_DATA_HOME:-~/.local/share}/Trash, with a
# .trashinfo per item) on Linux.
#
# Steps:
#   1. `init-root stop` through the running Root (the zone stops: kernel and bodies, no demotion).
#   2. Only for the real install (the LaunchAgent dev.init.root, or on Linux the systemd user unit init-root.service,
#      names this ROOT_HOME): `launchctl bootout` / `systemctl --user disable --now`, which stops the Root itself; the
#      plist or unit then goes to the Trash too.
#   3. A Root still running without the service (`init-root start` in a terminal) is never signalled from here:
#      stop it there (Ctrl-C), then run this again. Nothing is moved while a Root holds run/root.lock.
#   4. After you type `uninstall`, ROOT_HOME, INIT_HOME and the `init-agent` command (with init-tui, and INIT's `init`:
#      the launcher's name before v0.4.1, or the macOS shim of v0.4.1) move to the Trash, each with a timestamp in its
#      name. A file named init that is not INIT's stays. Never `rm`.
#   5. Linux, when install.sh's AppArmor step was run (Ubuntu 24.04 and later; docs/platforms.md §3.4.6): the root-owned
#      copies in /usr/local/lib/init-agent/ and the profile /etc/apparmor.d/init-agent need root to remove, so this
#      prints the commands. With --apparmor it asks, then runs them with sudo. It never runs sudo otherwise.
set -euo pipefail
umask 077

say() { printf '%s\n' "$*"; }
die() { printf 'uninstall.sh: %s\n' "$*" >&2; exit 1; }
APPARMOR=0
for a in "$@"; do
  case "$a" in
    --apparmor) APPARMOR=1 ;;
    -h|--help) sed -n '2,27p' "$0"; exit 0 ;;
    *) die "unknown argument $a (scripts/uninstall.sh [--apparmor])" ;;
  esac
done

PW_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
HOME="${HOME:-$PW_HOME}"
RH="${INIT_ROOT_HOME:-$HOME/.init-root}"
# INIT_HOME and the launchers' directory as the install recorded them (install.json "init_home", "bin_dir"); an
# install with INIT_BIN_DIR set keeps its init-agent there, and before v0.5.2 this looked only in ~/.local/bin
REC="$(/usr/bin/python3 -I -c 'import json,sys
try:
    j = json.load(open(sys.argv[1]))
    print(j.get("init_home") or ""); print(j.get("bin_dir") or "")
except Exception: print("")' "$RH/install.json")"
IH_REC="$(printf '%s\n' "$REC" | sed -n 1p)"; BIN_REC="$(printf '%s\n' "$REC" | sed -n 2p)"
IH="${INIT_HOME:-${IH_REC:-$HOME/.init}}"
BIN_DIR="${INIT_BIN_DIR:-${BIN_REC:-$HOME/.local/bin}}"
OS="$(uname -s)"
if [ "$OS" = Darwin ]; then
  TRASH="$HOME/.Trash"
  PLIST="$PW_HOME/Library/LaunchAgents/dev.init.root.plist"
  devno() { stat -f %d "$1"; }
else
  TRASH="${XDG_DATA_HOME:-$HOME/.local/share}/Trash/files"
  PLIST="$PW_HOME/.config/systemd/user/init-root.service" # the systemd user unit init-root install writes
  devno() { stat -c %d "$1"; }
fi
ROOT_BIN="$RH/bin/init-root"
RENV=(env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin)

[ -e "$RH" ] || [ -e "$IH" ] || die "nothing to uninstall: neither $RH nor $IH exists"
REAL=0
if [ -f "$PLIST" ] && { grep -qF "<string>$RH</string>" "$PLIST" || grep -qxF "Environment=\"INIT_ROOT_HOME=$RH\"" "$PLIST"; }; then REAL=1; fi

lock_free() { # 0 when no process holds ROOT_HOME/run/root.lock (the Root's flock); checks without waiting
  /usr/bin/python3 -I -c 'import fcntl,os,sys
try: fd = os.open(sys.argv[1], os.O_RDONLY)
except FileNotFoundError: sys.exit(0)
try: fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError: sys.exit(1)
sys.exit(0)' "$RH/run/root.lock"
}
instances() {
  "${RENV[@]}" "$ROOT_BIN" status --json 2>/dev/null | /usr/bin/python3 -I -c 'import json,sys
try: v = json.load(sys.stdin); print(len(v.get("instances") or []) if v.get("slot") != "down" else 0)
except Exception: print(0)'
}

# ---------------------------------------------------------------- 1. stop the zone through the Root
if [ -x "$ROOT_BIN" ] && [ -S "$RH/run/owner.sock" ] && ! lock_free; then
  say "stopping INIT (init-root stop) …"
  "${RENV[@]}" "$ROOT_BIN" stop >/dev/null 2>&1 || say "  (init-root stop did not answer)"
  for _ in $(seq 1 120); do [ "$(instances)" = 0 ] && break; sleep 0.5; done
  [ "$(instances)" = 0 ] || die "the zone did not stop within 60 s; try: $ROOT_BIN panic"
  say "  the zone is stopped"
fi

# ---------------------------------------------------------------- 2. the service (real install only)
if [ $REAL = 1 ] && [ "$OS" = Darwin ]; then
  say "unloading the launchd service dev.init.root …"
  launchctl bootout "gui/$(id -u)/dev.init.root" 2>/dev/null || true
  for _ in $(seq 1 120); do lock_free && break; sleep 0.5; done
elif [ $REAL = 1 ]; then
  say "stopping and disabling the systemd user service init-root.service …"
  systemctl --user disable --now init-root.service 2>/dev/null || true
  for _ in $(seq 1 120); do lock_free && break; sleep 0.5; done
fi

# ---------------------------------------------------------------- 3. never move a running Root's home
if ! lock_free; then
  pid="$(cat "$RH/run/root.pid" 2>/dev/null || echo '?')"
  die "the Root (pid $pid) is still running without the service. Stop it in its terminal (Ctrl-C), then run uninstall.sh again. Nothing was moved."
fi

# ---------------------------------------------------------------- 4. confirm, then move to the Trash
MOVE=()
for p in "$RH" "$IH"; do [ -e "$p" ] && MOVE+=("$p"); done
ours() { [ -f "$1" ] && [ ! -L "$1" ] && grep -qE '^(INIT_ROOT_HOME_DEFAULT=|INIT_AGENT_SHIM=1$)' "$1"; } # a launcher or the shim
LAUNCHER=0
for c in init-agent init; do
  if ours "$BIN_DIR/$c"; then MOVE+=("$BIN_DIR/$c"); LAUNCHER=1; fi
done
[ $LAUNCHER = 1 ] && [ -f "$BIN_DIR/init-tui" ] && MOVE+=("$BIN_DIR/init-tui")
[ $REAL = 1 ] && MOVE+=("$PLIST")
mkdir -p "$TRASH"
for p in "${MOVE[@]}"; do
  [ "$(devno "$p")" = "$(devno "$TRASH")" ] || die "$p is on another volume than $TRASH; move it to the Trash in your file manager instead"
done
say ""
say "This moves to $TRASH (nothing is deleted):"
for p in "${MOVE[@]}"; do say "  $p"; done
say "Your provider key ($RH/keys/providers/) goes with ROOT_HOME. Empty the Trash to delete it for good."
printf 'Type uninstall to continue: '
IFS= read -r answer || answer=""
[ "$answer" = uninstall ] || die "not confirmed; nothing was moved"
STAMP="$(date '+%Y-%m-%d %H.%M.%S')"
for p in "${MOVE[@]}"; do
  base="$(basename "$p")"
  dst="$TRASH/$base $STAMP"
  n=1; while [ -e "$dst" ]; do n=$((n + 1)); dst="$TRASH/$base $STAMP $n"; done
  mv -- "$p" "$dst"
  if [ "$OS" != Darwin ]; then # freedesktop.org Trash: the file manager can restore it from this record
    mkdir -p "$TRASH/../info"
    printf '[Trash Info]\nPath=%s\nDeletionDate=%s\n' "$p" "$(date +%Y-%m-%dT%H:%M:%S)" >"$TRASH/../info/$(basename "$dst").trashinfo"
  fi
  say "  moved $p → $dst"
done
if [ $REAL = 1 ] && [ "$OS" != Darwin ]; then systemctl --user daemon-reload 2>/dev/null || true; fi
say "INIT is uninstalled. To undo: move the items back from the Trash. To free the space: empty the Trash."

# ---------------------------------------------------------------- 5. the AppArmor step's files (Linux, root-owned)
HELPERS=/usr/local/lib/init-agent; AA_PROFILE=/etc/apparmor.d/init-agent
if [ "$OS" != Darwin ] && { [ -e "$AA_PROFILE" ] || [ -e "$HELPERS" ]; }; then
  aa_remove() {
    [ -e "$AA_PROFILE" ] && printf 'sudo apparmor_parser -R %s\nsudo rm -f %s\n' "$AA_PROFILE" "$AA_PROFILE"
    [ -e "$HELPERS" ] && printf 'sudo rm -f %s/unshare %s/bwrap\nsudo rmdir %s\n' "$HELPERS" "$HELPERS" "$HELPERS"
    return 0
  }
  say ""
  say "The AppArmor step left root-owned files that only root can remove (copies of unshare and bwrap, and the profile"
  say "that lets only them create user namespaces). Other INIT installs of this account would use them too. To remove:"
  say ""
  aa_remove
  say ""
  if [ $APPARMOR = 1 ]; then
    if [ -r /dev/tty ] && [ -w /dev/tty ]; then
      printf 'Run these commands now with sudo? [Y/n] ' >/dev/tty
      IFS= read -r aa </dev/tty || aa=n
      case "$aa" in ""|y|Y|yes|Yes|YES) aa_remove | /bin/bash -e && say "removed" || say "the commands failed; run them yourself" ;; *) say "left in place" ;; esac
    else
      say "(--apparmor asks before it runs sudo, and there is no terminal to ask on: run the commands above yourself)"
    fi
  fi
fi
