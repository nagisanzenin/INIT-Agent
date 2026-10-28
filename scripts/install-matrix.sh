#!/bin/bash
# The install matrix (docs/release-protocol.md): what a user sees when they run the one-line installer on a machine in
# each state a real user can be in, at $0 on the mock model, in test homes. Written after v0.5.0-preview shipped a get.sh
# that told a 0.3.1 owner to run `init-agent upgrade`, a command that install does not have (build/status/REL-050.md,
# "After the release"): the release had checked fresh installs and the upgrade from 0.4.1, never get.sh on an older
# install.
#
#   scripts/install-matrix.sh --bundle NEW_TARBALL [--get NEW_GET_SH] [--old VERSION ...] [--keep]
#   scripts/install-matrix.sh --published [--get NEW_GET_SH]
#
#   NEW_TARBALL  the release under test, its .sha256 next to it
#   NEW_GET_SH   the get.sh the release will publish as its asset (default: this tree's scripts/get.sh); the uninstaller
#                under test is this tree's scripts/uninstall.sh (the one the printed command fetches from main)
#   --old V      an older release to start from (repeatable; default: every public release from 0.3.1 on, except
#                NEW). Its bundle, .sha256 and get.sh come from `gh release download v<V>-preview` into
#                MX_CACHE (default /private/tmp/i5-mx-cache, reused across runs, sha256-checked)
#   --keep       keep the test homes (MX_DIR, default /private/tmp/i5-mx/run)
#   --published  no installs: the release's get.sh asset (releases/latest/download/get.sh) must be NEW_GET_SH byte for
#                byte, and the uninstaller the printed command fetches (main's scripts/uninstall.sh) must be this tree's
#
# Rows (each in its own test home DIR/{h,r,i}; the Root runs as our child with --no-service, never as a service):
#   fresh    NEW's get.sh on a clean home → one owner line answered by the mock → `init-agent --version` names NEW →
#            NEW's get.sh again says `init-agent upgrade`, and that command exists
#   old V    V installed with its own get.sh and running, then NEW's get.sh with no flags, as a user pastes it. The
#            message must take the branch get.sh promises and every command it prints must exist in that state:
#              V < 0.4.1   names V, prints the uninstall command; `--upgrade` refuses before any download; the printed
#                          command (its curl replaced by a copy of this tree's uninstall.sh, recorded in the verdict)
#                          refuses while the Root runs and moves nothing, then, after Ctrl-C to the Root, moves
#                          ROOT_HOME, INIT_HOME and INIT's launchers to the test home's Trash; a fresh NEW install in the
#                          same home answers, and init-tui is NEW's
#              V >= 0.4.1  prints the `--upgrade` command (0.4.1) or `init-agent upgrade` (0.5.0+); the upgrade with
#                          NEW's get.sh is refused while the foreground Root runs, then, after Ctrl-C, upgrades: the
#                          Root is NEW's, `audit verify --full` passes, the old `init` is gone, the install answers
# The owner's ~/.init, ~/.init-root, ~/.local/bin, ~/.local/share/init and ~/Library/LaunchAgents (t1_snap) must not
# change, except the data the owner's own running INIT writes while it is used (counted in the verdict). Signals go only to the PIDs this script started. Verdict: build/status/install-matrix.json; one PASS/FAIL
# line per row on stderr; exit 1 on any FAIL. Needs gh (for --old downloads), 21 GiB free on /, rustup at
# ~/.cargo/bin (installs use --no-rust with the owner's toolchain on PATH, as scripts/upgrade-e2e-t1.sh does).
set -euo pipefail
umask 077

MX_REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
# shellcheck source=scripts/upgrade-e2e-t1.sh
. "$MX_REPO/scripts/upgrade-e2e-t1.sh"
NEW=""; GET_NEW="$MX_REPO/scripts/get.sh"; OLDS=(); KEEP=0; PUBLISHED=0
MX_DIR="${MX_DIR:-/private/tmp/i5-mx/run}"; MX_CACHE="${MX_CACHE:-/private/tmp/i5-mx-cache}"
while [ $# -gt 0 ]; do
  case "$1" in
    --bundle) NEW="$2"; shift 2 ;;
    --get) GET_NEW="$2"; shift 2 ;;
    --old) OLDS+=("$2"); shift 2 ;;
    --keep) KEEP=1; shift ;;
    --published) PUBLISHED=1; shift ;;
    -h|--help) sed -n '2,37p' "$0"; exit 0 ;;
    *) echo "install-matrix.sh: unknown argument $1 (see --help)" >&2; exit 64 ;;
  esac
done
log() { printf '[mx %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { log "FAIL: $*"; exit 1; }
sha() { /usr/bin/shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1; }
UNI_URL="https://raw.githubusercontent.com/nagisanzenin/INIT-Agent/main/scripts/uninstall.sh"
GET_URL="https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh"
[ -f "$GET_NEW" ] || die "no get.sh at $GET_NEW"

# ---------------------------------------------------------------- --published: the URLs users run serve what we tested
if [ $PUBLISHED = 1 ]; then
  T="$(mktemp -d /private/tmp/i5-mx-pub.XXXXXX)"
  /usr/bin/curl -fsSL "$GET_URL" -o "$T/get.sh" || die "cannot fetch $GET_URL"
  /usr/bin/curl -fsSL "$UNI_URL" -o "$T/uninstall.sh" || die "cannot fetch $UNI_URL"
  rc=0
  if cmp -s "$T/get.sh" "$GET_NEW"; then log "PASS published get.sh == $GET_NEW ($(sha "$GET_NEW"))"
  else log "FAIL published get.sh ($(sha "$T/get.sh")) != $GET_NEW ($(sha "$GET_NEW"))"; rc=1; fi
  if cmp -s "$T/uninstall.sh" "$MX_REPO/scripts/uninstall.sh"; then log "PASS main's uninstall.sh == this tree's ($(sha "$T/uninstall.sh"))"
  else log "FAIL main's uninstall.sh ($(sha "$T/uninstall.sh")) != this tree's ($(sha "$MX_REPO/scripts/uninstall.sh"))"; rc=1; fi
  rm -rf -- "$T"
  exit $rc
fi

[ -f "$NEW" ] && [ -f "$NEW.sha256" ] || die "--bundle NEW_TARBALL (with its .sha256 next to it) is required"
(cd "$(dirname "$NEW")" && /usr/bin/shasum -a 256 -c --quiet "$(basename "$NEW").sha256") || die "$NEW does not match its .sha256"
[ "$(t1_free_gib)" -ge 21 ] || die "$(t1_free_gib) GiB free on /, below 21 GiB"
[ -x "$T1_PW_HOME/.cargo/bin/rustup" ] || die "no rustup at $T1_PW_HOME/.cargo/bin"
member() { /usr/bin/tar -xzOf "$1" "$(basename "$1" .tar.gz)/$2"; }
NV="$(member "$NEW" VERSION | tr -d '\n')"; NVN="${NV%%-*}"
case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) TARGET=macos-arm64 ;;
  Linux/x86_64) TARGET=linux-x86_64 ;;
  Linux/aarch64) TARGET=linux-arm64 ;;
  *) die "no INIT bundle target for $(uname -s)/$(uname -m)" ;;
esac
case "$(basename "$NEW")" in *"-$TARGET.tar.gz") ;; *) die "$(basename "$NEW") is not a $TARGET bundle" ;; esac
ver_lt() { /usr/bin/python3 -I -c 'import sys; v = lambda s: tuple(int(x) for x in s.split("-")[0].split("."))
sys.exit(0 if v(sys.argv[1]) < v(sys.argv[2]) else 1)' "$1" "$2"; }
if [ ${#OLDS[@]} = 0 ]; then
  while IFS= read -r v; do OLDS+=("$v"); done < <(gh release list -R nagisanzenin/INIT-Agent --limit 100 --json tagName --jq '.[].tagName' \
    | /usr/bin/python3 -I -c 'import sys
nv = sys.argv[1]; key = lambda s: tuple(int(x) for x in s.split("."))
vs = sorted({t.strip()[1:].split("-")[0] for t in sys.stdin if t.startswith("v")}, key=key)
print("\n".join(v for v in vs if key(v) >= (0, 3, 1) and v != nv))' "$NVN")
fi
log "NEW $(basename "$NEW") ($NV) · get.sh $GET_NEW ($(sha "$GET_NEW" | cut -c1-12)) · old: ${OLDS[*]:-none} · $TARGET"

mkdir -p "$MX_DIR" "$MX_CACHE"
W="$MX_DIR/w"; mkdir -p "$W"
STEPS="$W/steps.jsonl"; : >"$STEPS"
cp "$MX_REPO/scripts/uninstall.sh" "$W/uninstall.sh"   # a path without spaces, for the substituted command
member "$NEW" init-mockllm >"$W/init-mockllm"; chmod 700 "$W/init-mockllm"
NEW_TUI="$(member "$NEW" init-tui | /usr/bin/shasum -a 256 | cut -d' ' -f1)"
NEW_ROOT="$(member "$NEW" init-root | /usr/bin/shasum -a 256 | cut -d' ' -f1)"
# the mock: hello, as many times as any row asks (no other turn is scripted; a row that drives more fails visibly)
/usr/bin/python3 -I - "$W/mx.mock.jsonl" <<'PY'
import json, sys
u = lambda n: {"input": 12000 + 100 * n, "cache_read": 11776 if n else 0, "output": 9}
lines = [{"v": 1, "kind": "meta", "provider": "deepseek", "model": "deepseek-flash", "name": "install-matrix", "chunk": 16}]
lines += [{"v": 1, "kind": "turn", "expect": {"last_role": "user"}, "reply": {"text": "hello from the mock", "stop": "end_turn", "usage": u(n)}} for n in range(4)]
open(sys.argv[1], "w").write("".join(json.dumps(l) + "\n" for l in lines))
PY

# The old releases from the cache (downloaded once, checked every run)
fetch_old() { # V → prints the directory with the bundle, its .sha256 and its get.sh
  local v="$1" d="$MX_CACHE/v$1-preview" tgz="init-v$1-preview-$TARGET.tar.gz"
  if [ ! -f "$d/$tgz" ] || [ ! -f "$d/$tgz.sha256" ] || [ ! -f "$d/get.sh" ]; then
    mkdir -p "$d"
    gh release download "v$v-preview" -R nagisanzenin/INIT-Agent -p "$tgz" -p "$tgz.sha256" -p get.sh -D "$d" --clobber >/dev/null 2>&1 || return 1
  fi
  (cd "$d" && /usr/bin/shasum -a 256 -c --quiet "$tgz.sha256") >/dev/null || return 1
  printf '%s\n' "$d"
}

T1_OWNER_BEFORE="$W/owner-before.txt"; t1_snap >"$T1_OWNER_BEFORE"

# ---------------------------------------------------------------- one row (runs in a subshell: a failure ends the row)
ROW=""; ROW_T0=0
step() { # NAME PASS NOTE: one line of the verdict, and a FAIL ends the row
  /usr/bin/python3 -I -c 'import json,sys; print(json.dumps({"row": sys.argv[1], "step": sys.argv[2], "pass": sys.argv[3] == "1", "note": sys.argv[4]}))' \
    "$ROW" "$1" "$2" "$3" >>"$STEPS"
  if [ "$2" = 1 ]; then log "  ok   $ROW · $1${3:+ · $3}"; else log "  FAIL $ROW · $1 · $3"; exit 1; fi
}
row_env() { # the environment a user's shell gives get.sh, plus the test home's paths and the owner's rustup (--no-rust)
  GET_ENV=(env -i HOME="$H" PATH="$BIN:$T1_PW_HOME/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin" TMPDIR="$T1_DIR/tmp/" LANG=en_US.UTF-8
    INIT_ROOT_HOME="$RH" INIT_HOME="$IH" INIT_BIN_DIR="$BIN"
    RUSTUP_HOME="$T1_PW_HOME/.rustup" CARGO_HOME="$T1_PW_HOME/.cargo" RUSTUP_TOOLCHAIN="$T1_RUST")
}
row_cleanup() {
  set +e
  if [ -n "${ROOT_PID:-}" ] && kill -0 "$ROOT_PID" 2>/dev/null; then
    kill -INT "$ROOT_PID" 2>/dev/null
    for _ in $(seq 1 120); do kill -0 "$ROOT_PID" 2>/dev/null || break; sleep 0.25; done
    kill -0 "$ROOT_PID" 2>/dev/null && kill -KILL "$ROOT_PID" 2>/dev/null
    wait "$ROOT_PID" 2>/dev/null
  fi
  if [ -n "${MOCK_PID:-}" ] && kill -0 "$MOCK_PID" 2>/dev/null; then kill -TERM "$MOCK_PID"; wait "$MOCK_PID" 2>/dev/null; fi
  ROOT_PID=""; MOCK_PID=""
}
row_start() { # NAME DIR
  ROW="$1"; t1_paths "$2"; T1_MOCK_SCRIPT="$W/mx.mock.jsonl"
  if [ -e "$T1_DIR" ]; then chmod -R u+w "$T1_DIR"; rm -rf -- "$T1_DIR"; fi
  mkdir -p "$H" "$T1_DIR/tmp" "$T1_DIR/mock"
  [ ${#RH} -le 64 ] && [ ${#IH} -le 64 ] || step "test home paths at most 64 bytes" 0 "$RH"
  t1_write_driver; row_env
  trap row_cleanup EXIT
  "$W/init-mockllm" --mode upstream --listen 127.0.0.1:0 --strict deepseek --port-file "$T1_DIR/mock.port" \
    --script "$T1_MOCK_SCRIPT" --record-dir "$T1_DIR/mock" >"$T1_DIR/mock.out" 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 100); do [ -s "$T1_DIR/mock.port" ] && break; sleep 0.1; done
  PORT="$(tr -d '\n' <"$T1_DIR/mock.port" 2>/dev/null || true)"
  [ -n "$PORT" ] || step "mock upstream" 0 "$(tail -3 "$T1_DIR/mock.out")"
}
install_with() { # GET_SH TARBALL LOG: an install as the e2e checks make it (no service, the mock upstream)
  "${GET_ENV[@]}" /bin/sh "$1" --from "$2" --no-rust -- --no-service --disk-floor 5 --mock-upstream "127.0.0.1:$PORT" >"$3" 2>&1 </dev/null
}
start_root() { # LABEL: the Root as our child, until it says running and the body is ready
  (umask 077; printf 'test-mx-mock-key' >"$RH/keys/providers/deepseek")
  env -i HOME="$T1_PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$RH/bin/init-root" start --foreground >"$T1_DIR/root-$1.out" 2>&1 </dev/null &
  ROOT_PID=$!
  for _ in $(seq 1 240); do
    t1_renv "$RH/bin/init-root" status --json 2>/dev/null \
      | /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("slot")=="running" else 1)' 2>/dev/null \
      && [ -f "$IH/state/run/ready" ] && return 0
    sleep 0.5
  done
  tail -15 "$T1_DIR/root-$1.out" >&2; return 1
}
stop_root() { # Ctrl-C, as the owner would in the Root's terminal
  kill -INT "$ROOT_PID" 2>/dev/null || true
  for _ in $(seq 1 120); do kill -0 "$ROOT_PID" 2>/dev/null || break; sleep 0.25; done
  kill -0 "$ROOT_PID" 2>/dev/null && return 1
  wait "$ROOT_PID" 2>/dev/null || true; ROOT_PID=""
}
hello() { # LABEL: one owner line through the installed init-agent
  /usr/bin/python3 -I "$T1_DIR/drive.py" "$BIN/init-agent" "$H" "$T1_DIR/attach-$1.jsonl" "Say hello." "hello from the mock" 120 >"$T1_DIR/drive-$1.json"
}
run_get() { # LOG ARGS...: NEW's get.sh as a user runs it; prints its exit code
  set +e; "${GET_ENV[@]}" /bin/sh "$GET_NEW" "${@:2}" >"$1" 2>&1 </dev/null; local rc=$?; set -e; echo $rc
}
first_line() { head -n 1 "$1" | cut -c1-200; }
# Every command line get.sh prints (indented by two spaces) names commands that exist here, and every URL it prints
# answers. The state is the test home's: PATH is the user's (their bin dir first).
printed_commands_exist() { # LOG
  /usr/bin/python3 -I - "$1" "$BIN:/usr/bin:/bin:/usr/sbin:/sbin" <<'PY'
import os, re, shlex, subprocess, sys
text, path = open(sys.argv[1]).read(), sys.argv[2].split(":")
lines = [l.strip() for l in text.splitlines() if l.startswith("  ") and l.strip()]
if not lines:
    print("no command line in the message"); sys.exit(1)
bad = []
for l in lines:
    for part in re.split(r"&&|\|\||\||;", l):
        try:
            w = shlex.split(part)
        except ValueError:
            w = part.split()
        while w and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", w[0]):
            w = w[1:]
        if not w:
            continue
        c = w[0]
        ok = os.access(c, os.X_OK) if "/" in c else any(os.access(os.path.join(d, c), os.X_OK) for d in path)
        if not ok:
            bad.append(f"{c!r} (in: {l})")
for u in sorted(set(re.findall(r"https://[^\s\"']+", text))):
    r = subprocess.run(["/usr/bin/curl", "-fsSIL", "-o", "/dev/null", "-w", "%{http_code}", u], capture_output=True, text=True)
    if r.stdout.strip() != "200":
        bad.append(f"{u} answers {r.stdout.strip() or r.returncode}")
print("; ".join(bad) if bad else f"{len(lines)} command line(s), every command and URL exists")
sys.exit(1 if bad else 0)
PY
}

row_fresh() { # DIR
  row_start fresh "$1"
  local t0; t0=$(t1_ms)
  install_with "$GET_NEW" "$NEW" "$T1_DIR/get.log" && step "fresh install with NEW's get.sh" 1 "$(( $(t1_ms) - t0 )) ms" \
    || step "fresh install with NEW's get.sh" 0 "$(tail -5 "$T1_DIR/get.log" | tr '\n' ' ')"
  [ "$(sha "$RH/bin/init-root")" = "$NEW_ROOT" ] && [ "$(sha "$BIN/init-tui")" = "$NEW_TUI" ] && step "Root and TUI are NEW's" 1 "" || step "Root and TUI are NEW's" 0 ""
  start_root fresh && step "Root ready" 1 "" || step "Root ready" 0 "$(tail -3 "$T1_DIR/root-fresh.out" | tr '\n' ' ')"
  hello fresh && step "first reply" 1 "$(cat "$T1_DIR/drive-fresh.json")" || step "first reply" 0 "$(cat "$T1_DIR/drive-fresh.json")"
  local v; v="$("${GET_ENV[@]}" "$BIN/init-agent" --version 2>&1 | head -n 1)"
  case "$v" in *"$NVN"*) step "init-agent --version names $NVN" 1 "$v" ;; *) step "init-agent --version names $NVN" 0 "$v" ;; esac
  local rc; rc=$(run_get "$T1_DIR/get-again.log")
  [ "$rc" = 1 ] && grep -q "INIT $NVN is already installed" "$T1_DIR/get-again.log" && grep -qx '  init-agent upgrade' "$T1_DIR/get-again.log" \
    && step "get.sh again: init-agent upgrade" 1 "$(first_line "$T1_DIR/get-again.log")" || step "get.sh again: init-agent upgrade" 0 "rc $rc: $(first_line "$T1_DIR/get-again.log")"
  local pc; pc="$(printed_commands_exist "$T1_DIR/get-again.log")" && step "printed commands exist" 1 "$pc" || step "printed commands exist" 0 "$pc"
}

row_old() { # V DIR
  local v="$1" d tgz t0 rc pc cmd cmd2
  row_start "old-$v" "$2"
  d="$(fetch_old "$v")" || step "fetch v$v-preview ($TARGET)" 0 "no $TARGET bundle or a sha256 mismatch"
  tgz="$d/init-v$v-preview-$TARGET.tar.gz"
  t0=$(t1_ms)
  install_with "$d/get.sh" "$tgz" "$T1_DIR/old-get.log" && step "install $v with its own get.sh" 1 "$(( $(t1_ms) - t0 )) ms" \
    || step "install $v with its own get.sh" 0 "$(tail -5 "$T1_DIR/old-get.log" | tr '\n' ' ')"
  [ "$(t1_renv "$RH/bin/init-root" --version 2>&1)" = "init-root $v" ] && step "init-root --version is $v" 1 "" || step "init-root --version is $v" 0 "$(t1_renv "$RH/bin/init-root" --version 2>&1)"
  start_root old && step "$v Root ready" 1 "" || step "$v Root ready" 0 "$(tail -3 "$T1_DIR/root-old.out" | tr '\n' ' ')"

  # NEW's get.sh with no flags, as a user pastes the README's line
  rc=$(run_get "$T1_DIR/get-plain.log")
  if ver_lt "$v" 0.4.1; then
    [ "$rc" = 1 ] && grep -q "INIT $v is installed" "$T1_DIR/get-plain.log" && grep -q "before 0.4.1 is not supported" "$T1_DIR/get-plain.log" \
      && grep -qF "$UNI_URL" "$T1_DIR/get-plain.log" && ! grep -q 'init-agent upgrade' "$T1_DIR/get-plain.log" \
      && step "get.sh: names $v, prints the uninstall" 1 "$(first_line "$T1_DIR/get-plain.log")" || step "get.sh: names $v, prints the uninstall" 0 "rc $rc: $(cat "$T1_DIR/get-plain.log")"
  elif ver_lt "$v" 0.5.0; then
    [ "$rc" = 1 ] && grep -q "INIT $v is already installed" "$T1_DIR/get-plain.log" && grep -qF "get.sh | sh -s -- --upgrade" "$T1_DIR/get-plain.log" \
      && step "get.sh: prints the --upgrade command" 1 "$(first_line "$T1_DIR/get-plain.log")" || step "get.sh: prints the --upgrade command" 0 "rc $rc: $(cat "$T1_DIR/get-plain.log")"
  else
    [ "$rc" = 1 ] && grep -qx '  init-agent upgrade' "$T1_DIR/get-plain.log" \
      && step "get.sh: init-agent upgrade" 1 "$(first_line "$T1_DIR/get-plain.log")" || step "get.sh: init-agent upgrade" 0 "rc $rc: $(cat "$T1_DIR/get-plain.log")"
  fi
  pc="$(printed_commands_exist "$T1_DIR/get-plain.log")" && step "printed commands exist" 1 "$pc" || step "printed commands exist" 0 "$pc"

  if ver_lt "$v" 0.4.1; then
    rc=$(run_get "$T1_DIR/get-upgrade.log" --upgrade)
    [ "$rc" = 1 ] && grep -q "before 0.4.1 is not supported" "$T1_DIR/get-upgrade.log" && [ ! -e "$RH/cache/upgrade" ] \
      && step "--upgrade refuses, nothing downloaded" 1 "" || step "--upgrade refuses, nothing downloaded" 0 "rc $rc: $(cat "$T1_DIR/get-upgrade.log")"
    cmd="$(grep -E '^  f="\$\(mktemp\)"' "$T1_DIR/get-plain.log" | sed 's/^  //')"
    [ -n "$cmd" ] || step "the uninstall command line" 0 "not found in the message"
    local pat="curl -fsSL $UNI_URL -o" with="cp $W/uninstall.sh"
    cmd2="${cmd//"$pat"/$with}"
    [ "$cmd2" != "$cmd" ] || step "substitute the uninstaller's download" 0 "$cmd"
    step "the printed uninstall command, its curl replaced by a copy of this tree's uninstall.sh" 1 "$cmd2"
    set +e; printf 'uninstall\n' | "${GET_ENV[@]}" /bin/bash -c "$cmd2" >"$T1_DIR/uninstall-1.log" 2>&1; rc=$?; set -e
    [ "$rc" != 0 ] && grep -q 'still running without the service' "$T1_DIR/uninstall-1.log" && [ -d "$RH" ] && [ -d "$IH" ] \
      && step "uninstall refuses while the Root runs; nothing moved" 1 "" || step "uninstall refuses while the Root runs; nothing moved" 0 "rc $rc: $(cat "$T1_DIR/uninstall-1.log")"
    stop_root && step "Ctrl-C stops the $v Root" 1 "" || step "Ctrl-C stops the $v Root" 0 ""
    local launchers; launchers="$(ls "$BIN" 2>/dev/null | tr '\n' ' ')"
    set +e; printf 'uninstall\n' | "${GET_ENV[@]}" /bin/bash -c "$cmd2" >"$T1_DIR/uninstall-2.log" 2>&1; rc=$?; set -e
    [ "$rc" = 0 ] && [ ! -e "$RH" ] && [ ! -e "$IH" ] && [ ! -e "$BIN/init" ] && [ ! -e "$BIN/init-agent" ] && ls "$H/.Trash" | grep -q '^r ' \
      && step "uninstall moves $v to the Trash" 1 "bin before: $launchers; Trash: $(ls "$H/.Trash" | tr '\n' ',')" || step "uninstall moves $v to the Trash" 0 "rc $rc: $(cat "$T1_DIR/uninstall-2.log")"
    t0=$(t1_ms)
    install_with "$GET_NEW" "$NEW" "$T1_DIR/new-get.log" && step "fresh $NVN install in the same home" 1 "$(( $(t1_ms) - t0 )) ms" \
      || step "fresh $NVN install in the same home" 0 "$(tail -5 "$T1_DIR/new-get.log" | tr '\n' ' ')"
    [ "$(sha "$BIN/init-tui")" = "$NEW_TUI" ] && [ "$(sha "$RH/bin/init-root")" = "$NEW_ROOT" ] && step "init-tui and the Root are NEW's" 1 "" || step "init-tui and the Root are NEW's" 0 ""
    start_root new && step "Root ready" 1 "" || step "Root ready" 0 "$(tail -3 "$T1_DIR/root-new.out" | tr '\n' ' ')"
  else
    rc=$(run_get "$T1_DIR/get-upgrade-fg.log" --upgrade --from "$NEW" -- --probe-timeout 120)
    [ "$rc" != 0 ] && [ "$(sha "$RH/bin/init-root")" != "$NEW_ROOT" ] && kill -0 "$ROOT_PID" 2>/dev/null \
      && step "upgrade refused while the foreground Root runs (not signalled)" 1 "$(grep -m1 -iE 'foreground|Ctrl-C|terminal' "$T1_DIR/get-upgrade-fg.log" | cut -c1-160)" \
      || step "upgrade refused while the foreground Root runs (not signalled)" 0 "rc $rc: $(tail -5 "$T1_DIR/get-upgrade-fg.log" | tr '\n' ' ')"
    stop_root && step "Ctrl-C stops the $v Root" 1 "" || step "Ctrl-C stops the $v Root" 0 ""
    t0=$(t1_ms)
    rc=$(run_get "$T1_DIR/get-upgrade.log" --upgrade --from "$NEW" -- --probe-timeout 120)
    [ "$rc" = 0 ] && step "upgrade $v → $NVN with NEW's get.sh" 1 "$(( $(t1_ms) - t0 )) ms" || step "upgrade $v → $NVN with NEW's get.sh" 0 "rc $rc: $(tail -8 "$T1_DIR/get-upgrade.log" | tr '\n' ' ')"
    [ "$(sha "$RH/bin/init-root")" = "$NEW_ROOT" ] && [ ! -e "$BIN/init" ] && grep -q '= upgrade ]; then' "$BIN/init-agent" \
      && step "the Root and launcher are NEW's; the old init is gone" 1 "" || step "the Root and launcher are NEW's; the old init is gone" 0 "$(ls "$BIN" | tr '\n' ' ')"
    t1_renv "$RH/bin/init-root" audit verify --full >"$T1_DIR/verify.txt" 2>&1 && step "audit verify --full" 1 "$(tail -1 "$T1_DIR/verify.txt" | cut -c1-120)" \
      || step "audit verify --full" 0 "$(tail -3 "$T1_DIR/verify.txt" | tr '\n' ' ')"
    start_root up && step "upgraded Root ready" 1 "" || step "upgraded Root ready" 0 "$(tail -3 "$T1_DIR/root-up.out" | tr '\n' ' ')"
    rc=$(run_get "$T1_DIR/get-after.log")
    [ "$rc" = 1 ] && grep -qx '  init-agent upgrade' "$T1_DIR/get-after.log" && step "get.sh after the upgrade: init-agent upgrade" 1 "" \
      || step "get.sh after the upgrade: init-agent upgrade" 0 "rc $rc: $(cat "$T1_DIR/get-after.log")"
    pc="$(printed_commands_exist "$T1_DIR/get-after.log")" && step "printed commands exist after the upgrade" 1 "$pc" || step "printed commands exist after the upgrade" 0 "$pc"
  fi
  hello "$v" && step "the install answers" 1 "$(cat "$T1_DIR/drive-$v.json")" || step "the install answers" 0 "$(cat "$T1_DIR/drive-$v.json")"
}

# ---------------------------------------------------------------- run every row
RESULTS=(); ROWDIRS=()
run_row() { # NAME FUNCTION [ARG] DIR
  local t0 rc; t0=$(t1_ms)
  ROWDIRS+=("${!#}")
  set +e; ( set -e; "${@:2}" ); rc=$?; set -e
  RESULTS+=("$1 $rc $(( $(t1_ms) - t0 ))")
  if [ $rc = 0 ]; then log "PASS $1 ($(( ($(t1_ms) - t0) / 1000 )) s)"; else log "FAIL $1 ($(( ($(t1_ms) - t0) / 1000 )) s)"; fi
}
run_row fresh row_fresh "$MX_DIR/f"
for v in ${OLDS[@]+"${OLDS[@]}"}; do run_row "old-$v" row_old "$v" "$MX_DIR/o$(echo "$v" | tr -d .)"; done

t1_snap >"$W/owner-after.txt"
# The owner's own INIT may be in use during the run: its sessions, traces, work folder, spend ledgers and goals, and a
# key set in the TUI, change under it. Those paths are counted, not failed. Everything an installer, an upgrade or an uninstaller
# touches (the launchers, ~/.local/share/init, the LaunchAgent, rc files, the Root's and the kernel's binaries,
# install.json, the charter, the homes themselves) must not change; and a key file the size of this script's test key
# fails too (metadata only: no key is read).
OWNER_OK=1
OWNER_NOTE="$(/usr/bin/python3 -I - "$T1_OWNER_BEFORE" "$W/owner-after.txt" <<'PY'
import sys
ACTIVITY = (".init/state", ".init/vault", ".init/root", ".init/work", ".init-root/state", ".init-root/egress",
            ".init-root/extend", ".init-root/logs", ".init-root/keys/providers")
TEST_KEY_BYTES = len("test-mx-mock-key")
def load(p):
    d = {}
    for l in open(p):
        f = l.rstrip("\n").split(" ")
        d[f[0]] = f[1:]
    return d
a, b = load(sys.argv[1]), load(sys.argv[2])
changed = sorted(k for k in set(a) | set(b) if a.get(k) != b.get(k))
act = lambda k: any(k == x or k.startswith(x + "/") for x in ACTIVITY)
strict = [k for k in changed if not act(k)]
strict += [k for k in changed if k.startswith(".init-root/keys/providers/") and k in b and len(b[k]) > 1 and b[k][1] == str(TEST_KEY_BYTES)]
print(f"{len(changed) - len(strict)} change(s) in the owner's INIT data while it ran (not ours)" + (f"; OUTSIDE it: {strict[:10]}" if strict else ""))
sys.exit(1 if strict else 0)
PY
)" || OWNER_OK=0
if [ $OWNER_OK = 1 ]; then log "owner state: installs, launchers and service unchanged; $OWNER_NOTE"
else log "FAIL the owner's state changed: $OWNER_NOTE"; diff "$T1_OWNER_BEFORE" "$W/owner-after.txt" | head -20 >&2 || true; fi

mkdir -p "$MX_REPO/build/status"
/usr/bin/python3 -I - "$STEPS" "$MX_REPO/build/status/install-matrix.json" "$NEW" "$NV" "$GET_NEW" "$OWNER_OK" "$OWNER_NOTE" "${RESULTS[@]}" <<'PY'
import hashlib, json, os, sys, time
steps_path, out, new, nv, get_new, owner_ok, owner_note, *results = sys.argv[1:]
steps = [json.loads(l) for l in open(steps_path) if l.strip()]
sha = lambda p: hashlib.sha256(open(p, "rb").read()).hexdigest()
rows = []
for r in results:
    name, rc, ms = r.split(" ")
    mine = [s for s in steps if s["row"] == name]
    msg = next((s["note"] for s in mine if s["step"].startswith("get.sh")), None)
    rows.append({"row": name, "pass": rc == "0", "ms": int(ms), "get_sh_message": msg,
                 "steps": [{k: s[k] for k in ("step", "pass", "note")} for s in mine]})
v = {"v": 1, "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "bundle": os.path.basename(new), "version": nv,
     "bundle_sha256": sha(new), "get_sh_sha256": sha(get_new), "owner_state_unchanged": owner_ok == "1", "owner_state_note": owner_note,
     "ok": owner_ok == "1" and all(r["pass"] for r in rows), "rows": rows}
json.dump(v, open(out, "w"), indent=1)
open(out, "a").write("\n")
PY
OK="$(/usr/bin/python3 -I -c 'import json,sys; print(1 if json.load(open(sys.argv[1]))["ok"] else 0)' "$MX_REPO/build/status/install-matrix.json")"
[ $KEEP = 1 ] || { for d in "${ROWDIRS[@]}" "$W"; do [ -e "$d" ] && { chmod -R u+w "$d"; rm -rf -- "$d"; }; done; }
log "verdict: build/status/install-matrix.json · $([ "$OK" = 1 ] && echo PASS || echo FAIL)"
[ "$OK" = 1 ]
