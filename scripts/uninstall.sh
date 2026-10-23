#!/bin/bash
# Uninstall INIT: stop it, unload the service, and MOVE its homes to the Trash. Nothing is deleted: you empty
# the Trash yourself (or put things back from it).
#
#   scripts/uninstall.sh
#
# Paths (environment, as for install.sh): INIT_ROOT_HOME (default ~/.init-root), INIT_HOME (default: the one
# ROOT_HOME/install.json names, else ~/.init), INIT_BIN_DIR (default ~/.local/bin). The Trash is ~/.Trash.
#
# Steps:
#   1. `init-root stop` through the running Root (the zone stops: kernel and bodies, no demotion).
#   2. Only for the real install (the LaunchAgent dev.init.root names this ROOT_HOME): `launchctl bootout`, which
#      stops the Root itself; the plist then goes to the Trash too.
#   3. A Root still running without the service (`init-root start` in a terminal) is never signalled from here:
#      stop it there (Ctrl-C), then run this again. Nothing is moved while a Root holds run/root.lock.
#   4. After you type `uninstall`, ROOT_HOME, INIT_HOME and the `init` command (with init-tui) move to ~/.Trash, each with a
#      timestamp in its name. Never `rm`.
set -euo pipefail
umask 077

say() { printf '%s\n' "$*"; }
die() { printf 'uninstall.sh: %s\n' "$*" >&2; exit 1; }

PW_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
HOME="${HOME:-$PW_HOME}"
RH="${INIT_ROOT_HOME:-$HOME/.init-root}"
IH_REC="$(/usr/bin/python3 -I -c 'import json,sys
try: print(json.load(open(sys.argv[1])).get("init_home") or "")
except Exception: print("")' "$RH/install.json")"
IH="${INIT_HOME:-${IH_REC:-$HOME/.init}}"
BIN_DIR="${INIT_BIN_DIR:-$HOME/.local/bin}"
TRASH="$HOME/.Trash"
PLIST="$PW_HOME/Library/LaunchAgents/dev.init.root.plist"
ROOT_BIN="$RH/bin/init-root"
RENV=(env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin)

[ -e "$RH" ] || [ -e "$IH" ] || die "nothing to uninstall: neither $RH nor $IH exists"
REAL=0
if [ -f "$PLIST" ] && grep -q "<string>$RH</string>" "$PLIST"; then REAL=1; fi

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
if [ $REAL = 1 ]; then
  say "unloading the launchd service dev.init.root …"
  launchctl bootout "gui/$(id -u)/dev.init.root" 2>/dev/null || true
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
if [ -f "$BIN_DIR/init" ] && grep -q '^INIT_ROOT_HOME_DEFAULT=' "$BIN_DIR/init"; then
  MOVE+=("$BIN_DIR/init")
  [ -f "$BIN_DIR/init-tui" ] && MOVE+=("$BIN_DIR/init-tui")
fi
[ $REAL = 1 ] && MOVE+=("$PLIST")
mkdir -p "$TRASH"
for p in "${MOVE[@]}"; do
  [ "$(stat -f %d "$p")" = "$(stat -f %d "$TRASH")" ] || die "$p is on another volume than $TRASH; move it to the Trash in Finder instead"
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
  say "  moved $p → $dst"
done
say "INIT is uninstalled. To undo: move the items back from the Trash. To free the space: empty the Trash."
