#!/bin/bash
# REAL-M7: the lean M7 proof lines (measurements/m7-runbook.md; build/specs/PROOF-LEAN.md) on the REAL model, and the
# same harness at $0 on init-mockllm (the dry run). Logic lives in tests/real/checks_m7.py; S-REAL's checks.py and
# scripts/slice1/collect.py are reused unchanged.
#
#   tests/real/m7.sh selftest                        pure logic at $0 (no Root, no network, no key)
#   tests/real/m7.sh ready                           which interfaces each line needs are present (blocker list)
#   tests/real/m7.sh stage-bins                      clone the current debug binaries into target/m7-bin (APFS, ~0 disk)
#   tests/real/m7.sh <mock|real> install             one test home per mode, kept across lines (S is drawn once)
#   tests/real/m7.sh <mock|real> run <P|C|D|M|G1L|G1D>
#   tests/real/m7.sh mock dry                        every line in order on the mock home ($0), stopping at the first block
#   tests/real/m7.sh <mock|real> ledger | clean | freeze   (mock also: refresh)
#   tests/real/m7.sh watch-test                      $0, live: the disk/memory watchdog on a Root-only test home
#
# One home per mode (target/m7-homes/{m,r}); the ledger in it orders the lines (P → C → D → M → G1L; G1D on the mock
# before any real D), carries the plan picked from P's measured mean, the halts, and the program's real spend.
#
# Real-mode guards (the orchestrator gives the go per line):
#   - M7_I_MEAN_IT=1 and M7_GO=<the line> are both required; deepseek-flash only;
#   - the line's real cap comes from the ledger (checks_m7.py line_budget); the charter's money ceilings are set per
#     line (each pool: its share of cap × Root/real factor + 4 reservations) and frozen to "spent" on any stop;
#   - a guard polls the priced Root usage every 30 s and the free balance every 5 min, and stops at the real cap;
#   - tests/real/m7watch.py polls free disk every 5 s beside every line: below M7_WATCH_MIN_FREE_MB (6000) it stops the
#     Root's process tree (SIGSTOP), freezes spend, then kills the tree and the driver by PID; with M7_MEM_CAP_MB > 0 it
#     also kills a single workload process whose memory footprint passes the cap (D's first G4: a runaway python3.9
#     grew to 43.9 GB and its swap filled the disk). A real line starts only with M7_START_MIN_FREE_MB (10000) free;
#   - the key is copied from ~/.config/init-agent/deepseek.key with `tr` (never echoed, never on a command line) into
#     the test Root's key store, mode 0600, only in real mode after both guards, and removed in the EXIT trap and before
#     any collection. The mock and the dry run never touch the key file.
#   - collect.py redacts the key and `sk-` strings; checks_m7.py redact then removes e-mails and the git identity and
#     rescans (counts only). The owner's e-mail and git config never reach a request: every process runs under
#     `env -i`, and git under GIT_CONFIG_GLOBAL=/dev/null.
# Safety: signals only to PIDs this script spawned. Homes live under target/m7-homes/ and are removed by exact path.
# ~/.init and ~/.init-root are never touched. ROOT_HOME/sealed/{S,seeds,g4} are never read or collected.
set -euo pipefail
umask 077

REPO="$(cd "$(dirname "$0")/../.." && pwd -P)"
PY=(/usr/bin/python3 -I "$REPO/tests/real/checks_m7.py")
BIN="${M7_BIN_DIR:-$REPO/target/m7-bin}"
log() { printf '[m7 %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { log "FAIL: $*"; exit 1; }
free_mb() { df -m / | awk 'NR==2{print $4}'; }

case "${1:-}" in
  selftest) "${PY[@]}" selftest --repo "$REPO" && exec /usr/bin/python3 -I "$REPO/tests/real/m7watch.py" selftest ;;
  ready) exec "${PY[@]}" ready --repo "$REPO" --bins "$BIN" ${2:+--json} ;;
  stage-bins)
    mkdir -p "$BIN"
    for b in "$REPO/root/target/debug/init-root" "$REPO/kernel/target/debug/init-kernel" "$REPO/kernel/target/debug/initctl" \
             "$REPO/kernel/target/debug/init-mockllm"; do
      [ -x "$b" ] || die "missing $b (build it first, through ci/slot.sh)"
      n="$BIN/$(basename "$b")"; if [ -e "$n" ]; then rm -f -- "$n"; fi
      cp -c "$b" "$n"
    done
    ls -la "$BIN" >&2; exit 0 ;;
  freeze-test)
    # $0, live: a Root-only test home (no seed, ~170 MB), the P charter applied, then the guard's freeze; the Root's
    # /_root/ceilings must show both pools at what they spent (0 here). Removed by exact path afterwards.
    T="$REPO/target/m7-homes/t"; TR="$T/rh"; TI="$T/ih"
    TENV=(env -i HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')" INIT_ROOT_HOME="$TR" INIT_HOME="$TI" PATH=/usr/bin:/bin)
    if [ -e "$T" ]; then rm -rf -- "$T"; fi
    mkdir -p "$T/rel" "$TR"
    for b in init-kernel initctl init-mockllm; do cp -c "$BIN/$b" "$T/rel/$b"; done
    cp -cR "$REPO/wit" "$T/rel/wit"
    "${PY[@]}" params P --mode mock --rh "$TR" --repo "$REPO" --bins "$BIN" --mock-port 9 >/dev/null || true
    "${PY[@]}" charter --params "$T/charter.params.json" --out "$T/charter.toml"
    TC="$(cd "$("${TENV[@]}" /usr/bin/printenv HOME)/.rustup/toolchains/stable-aarch64-apple-darwin" && pwd -P)"
    "${TENV[@]}" "$BIN/init-root" install --release "$T/rel" --charter "$T/charter.toml" --toolchain "$TC" --no-service >"$T/install.out" 2>&1 \
      || { tail -5 "$T/install.out" >&2; rm -rf -- "$T"; die "freeze-test install"; }
    "${TENV[@]}" "$TR/bin/init-root" start --foreground >"$T/root.out" 2>&1 </dev/null &
    RP=$!
    for _ in $(seq 1 200); do [ -S "$TR/run/egress.sock" ] && break; sleep 0.05; done
    before="$(curl -s --max-time 5 --unix-socket "$TR/run/egress.sock" http://root/_root/ceilings)"
    "${PY[@]}" freeze --rh "$TR" --home "$("${TENV[@]}" /usr/bin/printenv HOME)" >"$T/freeze.json" || true
    sleep 1
    after="$(curl -s --max-time 5 --unix-socket "$TR/run/egress.sock" http://root/_root/ceilings)"
    "${TENV[@]}" "$TR/bin/init-root" stop >/dev/null 2>&1 || true
    kill -TERM "$RP" 2>/dev/null || true
    for _ in $(seq 1 40); do kill -0 "$RP" 2>/dev/null || break; sleep 0.25; done
    kill -KILL "$RP" 2>/dev/null || true; wait "$RP" 2>/dev/null || true
    res="$(/usr/bin/python3 -I -c 'import json,sys
b=json.loads(sys.argv[1] or "{}").get("money",{}); a=json.loads(sys.argv[2] or "{}").get("money",{}); f=json.load(open(sys.argv[3]))
ok = f.get("rc")==0 and a.get("total_limit_musd")==f.get("ceiling_musd") and a.get("sealed_reserve_musd")==f.get("reserve_musd") and b.get("total_limit_musd",0)>a.get("total_limit_musd",-1)
print(json.dumps({"pass": ok, "before": {k: b.get(k) for k in ("total_limit_musd","sealed_reserve_musd","total_spent_musd")}, "after": {k: a.get(k) for k in ("total_limit_musd","sealed_reserve_musd","total_spent_musd")}, "freeze": f}))' "$before" "$after" "$T/freeze.json")"
    echo "$res"
    rm -rf -- "$T"
    /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1])["pass"] else 1)' "$res"; exit $? ;;
  watch-test)
    # $0, live: the watchdog on a Root-only test home (as freeze-test) with its trip level above the current free disk,
    # so it trips at once without filling anything: the real `checks_m7.py freeze` must run while the tree is stopped
    # (rh/charter.toml then carries the frozen ceilings), and the Root must be gone by PID. Removed by exact path.
    T="$REPO/target/m7-homes/t"; TR="$T/rh"; TI="$T/ih"
    TENV=(env -i HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')" INIT_ROOT_HOME="$TR" INIT_HOME="$TI" PATH=/usr/bin:/bin)
    if [ -e "$T" ]; then rm -rf -- "$T"; fi
    mkdir -p "$T/rel" "$TR"
    for b in init-kernel initctl init-mockllm; do cp -c "$BIN/$b" "$T/rel/$b"; done
    cp -cR "$REPO/wit" "$T/rel/wit"
    "${PY[@]}" params P --mode mock --rh "$TR" --repo "$REPO" --bins "$BIN" --mock-port 9 >/dev/null || true
    "${PY[@]}" charter --params "$T/charter.params.json" --out "$T/charter.toml"
    TC="$(cd "$("${TENV[@]}" /usr/bin/printenv HOME)/.rustup/toolchains/stable-aarch64-apple-darwin" && pwd -P)"
    "${TENV[@]}" "$BIN/init-root" install --release "$T/rel" --charter "$T/charter.toml" --toolchain "$TC" --no-service >"$T/install.out" 2>&1 \
      || { tail -5 "$T/install.out" >&2; rm -rf -- "$T"; die "watch-test install"; }
    "${TENV[@]}" "$TR/bin/init-root" start --foreground >"$T/root.out" 2>&1 </dev/null &
    RP=$!
    for _ in $(seq 1 200); do [ -S "$TR/run/egress.sock" ] && break; sleep 0.05; done
    before="$(curl -s --max-time 5 --unix-socket "$TR/run/egress.sock" http://root/_root/ceilings)"
    /bin/sleep 600 & SP=$!
    t0=$(date +%s); wrc=0
    /usr/bin/python3 -I "$REPO/tests/real/m7watch.py" run --root-pid "$RP" --pid "$SP" --rh "$TR" \
      --home "$("${TENV[@]}" /usr/bin/printenv HOME)" --log "$T/watch.jsonl" --trip-file "$T/watch.trip" \
      --min-free-mb $(( $(free_mb) + 100000 )) --poll-s 5 >"$T/watch.out" 2>&1 || wrc=$?
    t1=$(date +%s)
    kill -0 "$RP" 2>/dev/null && rgone=0 || rgone=1
    kill -0 "$SP" 2>/dev/null && sgone=0 || sgone=1
    kill -KILL "$RP" "$SP" 2>/dev/null || true; wait "$RP" "$SP" 2>/dev/null || true
    res="$(/usr/bin/python3 -I -c 'import json,sys,re
evs=[json.loads(l) for l in open(sys.argv[1])]; k=[e["ev"] for e in evs]
fz=next((e for e in evs if e["ev"]=="freeze"),{}); out=json.loads(fz.get("out") or "{}")
ch=open(sys.argv[2]).read(); b=json.loads(sys.argv[3] or "{}").get("money",{})
m=re.search(r"reserve_usd\s*=\s*\"([0-9.]+)\"", ch)
ok = sys.argv[4]=="3" and k[:1]==["start"] and "trip" in k and k.index("trip")<k.index("freeze")<k.index("stopped") and fz.get("rc")==0 \
     and m is not None and round(float(m.group(1))*1e6)==out.get("reserve_musd") and b.get("total_limit_musd",0)>out.get("ceiling_musd",-1) \
     and sys.argv[5]=="1" and sys.argv[6]=="1"
print(json.dumps({"pass": ok, "rc": int(sys.argv[4]), "events": k, "freeze": out, "ceilings_before": {x: b.get(x) for x in ("total_limit_musd","sealed_reserve_musd")}, "root_gone": sys.argv[5]=="1", "extra_pid_gone": sys.argv[6]=="1", "trip_to_stopped_s": int(sys.argv[7])}))' \
      "$T/watch.jsonl" "$TR/charter.toml" "$before" "$wrc" "$rgone" "$sgone" "$((t1 - t0))")"
    echo "$res"
    rm -rf -- "$T"
    /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1])["pass"] else 1)' "$res"; exit $? ;;
esac

MODE="${1:-}"; CMD="${2:-}"
case "$MODE" in mock) X=m ;; real) X=r ;; *) echo "usage: $0 <selftest|ready|stage-bins|mock|real> …" >&2; exit 64 ;; esac
shift 2 || true
PROD=0; [ "$MODE" = real ] && PROD=1
STARTED=0
D="$REPO/target/m7-homes/$X"
RH="$D/rh"; IH="$D/ih"; REL="$D/rel"
REAL_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
TC="$(cd "$REAL_HOME/.rustup/toolchains/stable-aarch64-apple-darwin" && pwd -P)"
DEVDIR=/Library/Developer/CommandLineTools
KEYFILE="$REAL_HOME/.config/init-agent/deepseek.key"
SEED_TAG=seed-v0.3.2
MIN_FREE_MB="${M7_MIN_FREE_MB:-5300}"
START_MIN_FREE_MB="${M7_START_MIN_FREE_MB:-10000}"   # real lines only (orchestrator, 2026-10-21)
WATCH_MIN_FREE_MB="${M7_WATCH_MIN_FREE_MB:-6000}"
MEM_CAP_MB="${M7_MEM_CAP_MB:-0}"
WATCH=(/usr/bin/python3 -I "$REPO/tests/real/m7watch.py")
[ ${#RH} -le 64 ] && [ ${#IH} -le 64 ] || die "homes exceed 64 bytes ($RH)"
RENV=(env -i HOME="$REAL_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin)

PIDS=()
cleanup() {
  set +eu
  if [ -S "$RH/run/owner.sock" ] && [ "${STARTED:-0}" = 1 ]; then
    "${RENV[@]}" "$RH/bin/init-root" stop >>"$D/stop.out" 2>&1
    for _ in $(seq 1 120); do
      n=$("${RENV[@]}" "$RH/bin/init-root" status --json 2>/dev/null | /usr/bin/python3 -I -c 'import json,sys
try: print(len(json.load(sys.stdin).get("instances") or []))
except Exception: print(0)')
      [ "$n" = 0 ] && break
      sleep 0.25
    done
  fi
  for p in "${PIDS[@]}"; do kill -TERM "$p" 2>/dev/null; done   # only our own children, by pid
  for p in "${PIDS[@]}"; do
    for _ in $(seq 1 40); do kill -0 "$p" 2>/dev/null || break; sleep 0.25; done
    kill -KILL "$p" 2>/dev/null
  done
  PIDS=()
  wait 2>/dev/null
  rm -f -- "$RH/keys/providers/deepseek" 2>/dev/null   # the real key never outlives a run
  STARTED=0
}
trap cleanup EXIT

rm_home() { # the home by exact path; the pool mirror and the installed pack are read-only by design
  # a live Root accepts a connection on its owner socket (a stale socket file refuses it)
  if [ -S "$RH/run/owner.sock" ] && /usr/bin/python3 -I -c 'import socket,sys
s=socket.socket(socket.AF_UNIX); s.settimeout(2)
try: s.connect(sys.argv[1]); sys.exit(0)
except OSError: sys.exit(1)' "$RH/run/owner.sock"; then die "a Root is running in $D"; fi
  if [ -e "$D" ]; then chmod -R u+w "$D" && rm -rf -- "$D"; fi
}

params() { # $1 line → charter.params.json in $D; exit 5 when the line has no budget
  local port=""; [ -f "$D/mock.port.fixed" ] && port="--mock-port $(cat "$D/mock.port.fixed")"
  # shellcheck disable=SC2086
  "${PY[@]}" params "$1" --mode "$MODE" --rh "$RH" --repo "$REPO" --bins "$BIN" --toolchain "$TC" --devdir "$DEVDIR" \
    --wall-s "${M7_WALL_S:-3600}" $port >"$D/params.out"
}

write_configs() { # charter (ROOT_HOME) + kernel config from charter.params.json
  "${PY[@]}" charter --params "$D/charter.params.json" --out "$1"
  [ -d "$IH/kernel" ] && "${PY[@]}" kconfig --params "$D/charter.params.json" --out "$IH/kernel/config.toml" && chmod 644 "$IH/kernel/config.toml"
  return 0
}

# ================================================================ install (once per mode; kept across lines)
do_install() {
  local FREE0; FREE0="$(free_mb)"
  [ "$FREE0" -ge "${M7_START_FREE_MB:-6500}" ] || die "only $FREE0 MB free; a cold install (seed body build, ~600 MB peak) needs ${M7_START_FREE_MB:-6500} MB"
  for b in init-root init-kernel initctl init-mockllm; do [ -x "$BIN/$b" ] || die "missing $BIN/$b (tests/real/m7.sh stage-bins)"; done
  rm_home
  mkdir -p "$REL" "$RH"
  cp -c "$BIN/init-root" "$D/init-root"
  cp -c "$BIN/init-kernel" "$REL/init-kernel"
  cp -c "$BIN/initctl" "$REL/initctl"
  cp -c "$BIN/init-mockllm" "$REL/init-mockllm"
  [ $PROD = 1 ] || cp -c "$BIN/init-mockllm" "$D/init-mockllm"
  cp -cR "$REPO/wit" "$REL/wit"
  cat >"$REL/config.toml" <<EOF
schema_version = 2

[sandbox]
toolchain = "$TC"
developer_dir = "$DEVDIR"

[supervise]
build_timeout_cold_s = 3600
build_timeout_warm_s = 1800
EOF
  /usr/bin/python3 -I "$REPO/scripts/slice1/seedpack.py" --repo "$REPO" --dst "$D/seedsrc" --kind repo --rev "${M7_SEED_REV:-HEAD}" \
    >"$D/seedsrc.json" || die "seed packaging"
  local G=(env -i PATH=/usr/bin:/bin HOME="$D" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null TZ=UTC
     GIT_AUTHOR_DATE=2026-10-21T00:00:00Z GIT_COMMITTER_DATE=2026-10-21T00:00:00Z
     /usr/bin/git -C "$D/seedsrc" -c core.hooksPath=/dev/null -c user.name=init-maintainer -c user.email=maintainer@init.invalid
     -c commit.gpgsign=false -c tag.gpgsign=false -c gc.auto=0 -c maintenance.auto=false)
  "${G[@]}" init -q -b seed
  "${G[@]}" add -A
  "${G[@]}" commit -q -m "seed $SEED_TAG (REAL-M7 seed; source ${M7_SEED_REV:-HEAD})"
  "${G[@]}" tag "$SEED_TAG"
  "${G[@]}" rev-parse HEAD >"$D/seed.sha"
  "${G[@]}" bundle create -q "$D/$SEED_TAG.bundle" refs/heads/seed "refs/tags/$SEED_TAG"
  rm -rf -- "$D/seedsrc"
  if [ $PROD = 0 ]; then
    /usr/bin/python3 -I -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])' >"$D/mock.port.fixed"
  fi
  params P || die "P has no budget: $(cat "$D/params.out")"
  write_configs "$D/charter.toml"
  local IENV=(env -i HOME="$REAL_HOME" INIT_ROOT_HOME="$RH" INIT_HOME="$IH" PATH=/usr/bin:/bin)
  set +e
  "${IENV[@]}" "$D/init-root" install --release "$REL" --charter "$D/charter.toml" --toolchain "$TC" \
    --seed-source "$D/$SEED_TAG.bundle" --no-service >"$D/install.out" 2>&1
  local IRC=$?
  set -e
  [ $IRC = 0 ] || { tail -20 "$D/install.out" >&2; die "install (exit $IRC)"; }
  if [ -e "$IH/cache/build/slot" ]; then rm -rf -- "$IH/cache/build/slot"; fi
  # the visible pool v1 (POOL-LITE): full validation (cargo for the Rust tasks), frozen into vault/gate/pool/ and
  # mirrored read-only at kernel/pool/ (pool.py refuses ~/.init and ~/.init-root)
  env -i HOME="$REAL_HOME" PATH="$TC/bin:/usr/bin:/bin" TMPDIR="${TMPDIR:-/tmp}" \
    /usr/bin/python3 -I "$REPO/fixtures/pool-v1/tools/pool.py" import --home "$IH" >"$D/pool-import.out" 2>&1 \
    || { tail -20 "$D/pool-import.out" >&2; die "pool import"; }
  log "installed $D (INIT_HOME $(du -sm "$IH" | cut -f1) MB; pool $(ls "$IH/vault/gate/pool" | wc -l | tr -d ' ') tasks); free $(free_mb) MB"
}

# ================================================================ one line
mock_script() { # the owner turns the mock plays for a line (the G4/probe/G3 sessions use the mock's own responders)
  /usr/bin/python3 -I - "$2" "$1" <<'PYEOF'
import json, sys
run, line = sys.argv[1], sys.argv[2]
U = {"input": 6000, "cache_read": 0, "output": 30}
meta = {"v": 1, "kind": "meta", "provider": "deepseek", "model": "deepseek-flash", "name": f"m7-{line}", "chunk": 16}
def turn(reply): return {"v": 1, "kind": "turn", "reply": reply}
lines = [meta]
cal = {"P": "initctl evo calibrate --repeats 1 --limit 2 --no-wait", "C": "initctl evo calibrate --repeats 2 --limit 0 --no-wait"}
if line in cal:
    lines += [turn({"think": "Run it.", "calls": [{"name": "bash", "input": {"command": cal[line], "timeout": 600}}], "stop": "tool_use", "usage": U}),
              turn({"text": "done", "stop": "end_turn", "usage": U})]
if line in ("G1D", "G1L"):
    lines += [turn({"think": "Promote.", "calls": [{"name": "bash", "input": {"command": "cd \"$INIT_HOME/body\" && initctl promote evo/m7-null --wait", "timeout": 3600}}], "stop": "tool_use", "usage": U}),
              turn({"text": "Promoted.", "stop": "end_turn", "usage": U})]
elif line == "D":
    lines += [turn({"think": "Read the frictions.", "calls": [{"name": "bash", "input": {"command": "initctl evo frictions --json"}}], "stop": "tool_use", "usage": U}),
              turn({"think": "File the canned dry-run proposal.", "calls": [{"name": "bash", "input": {"command": "sh \"$INIT_HOME/work/m7dry/propose.sh\"", "timeout": 3600}}], "stop": "tool_use", "usage": U}),
              turn({"text": "Proposal filed and screened (see work/m7dry/p.id).", "stop": "end_turn", "usage": U}),
              turn({"think": "Promote.", "calls": [{"name": "bash", "input": {"command": "sh \"$INIT_HOME/work/m7dry/promote.sh\"", "timeout": 3600}}], "stop": "tool_use", "usage": U}),
              turn({"text": "Promoted.", "stop": "end_turn", "usage": U})]
lines += [turn({"text": "ok", "stop": "end_turn", "usage": U})] * 4
open(f"{run}/check.mock.jsonl", "w").write("".join(json.dumps(l, sort_keys=True) + "\n" for l in lines))
PYEOF
}

do_run() {
  local LINE="${1:-}"
  [ -n "$LINE" ] || die "run needs a line (P C D M G1L G1D)"
  [ -x "$RH/bin/init-root" ] || die "not installed: $0 $MODE install"
  if [ $PROD = 1 ]; then
    [ "${M7_I_MEAN_IT:-}" = 1 ] || die "real mode spends money: set M7_I_MEAN_IT=1"
    [ "${M7_GO:-}" = "$LINE" ] || die "real line $LINE needs the orchestrator's go: M7_GO=$LINE"
    [ "$LINE" != G1D ] || die "G1D runs on the mock only"
    [ -f "$KEYFILE" ] || die "no key file at ~/.config/init-agent/deepseek.key"
  fi
  local NEED="$MIN_FREE_MB"; case "$MODE:$LINE" in *:G1L|*:G1D|mock:D) NEED=$((MIN_FREE_MB + 1100)) ;; esac   # selfmap's body build (~1 GB)
  [ "$(free_mb)" -ge "$NEED" ] || die "only $(free_mb) MB free (line $LINE needs $NEED)"
  if [ $PROD = 1 ] && [ "$(free_mb)" -lt "$START_MIN_FREE_MB" ]; then die "only $(free_mb) MB free (a real line starts at $START_MIN_FREE_MB)"; fi
  local OUT="$REPO/build/real-artifacts/m7/$MODE/${M7_TAG:-$LINE}"
  local R="$D/runs/${M7_TAG:-$LINE}"
  if [ -e "$R" ]; then rm -rf -- "$R"; fi
  mkdir -p "$R/mock"
  "${PY[@]}" offpeak --wall-s "${M7_WALL_S:-3600}" >"$R/offpeak.json"
  log "$LINE: price window $(cat "$R/offpeak.json")"
  if ! params "$LINE"; then log "$LINE refused: no budget: $(cat "$D/params.out")"; return 5; fi
  write_configs "$RH/charter.toml"
  "${RENV[@]}" "$RH/bin/init-root" charter check --apply --yes >"$R/charter-apply.out" 2>&1 || { cat "$R/charter-apply.out" >&2; die "charter apply"; }
  log "$LINE: $(/usr/bin/python3 -I -c 'import json,sys;p=json.load(open(sys.argv[1]));print("real cap $%.2f, tier %s, ceilings: sealed $%.2f, total $%.2f (factor %s)"%(p["cap_real_musd"]/1e6,p["tier"],p["reserve_musd"]/1e6,p["ceiling_musd"]/1e6,p["factor"]))' "$D/charter.params.json")"
  # before the Root starts ($0, local): G1's null commit (+ selfmap, ~1 GB transient); the mock's canned D files
  "${PY[@]}" prepare "$LINE" --ih "$IH" --rh "$RH" --run "$R" --repo "$REPO" --mode "$MODE" >"$R/prepare.json" || die "prepare $LINE"
  if [ $PROD = 0 ]; then
    mock_script "$LINE" "$R"
    local PORT; PORT="$(cat "$D/mock.port.fixed")"
    # no request bodies: a sealed run's prompt is S content (BUILD_AGENT_BRIEF); the log keeps hashes and headers only
    # task sessions (G4 runs, screens, Root sealed runs) without their own script take this default: it writes
    # ok.txt = OK, which passes the health canary on both arms and fails every real task and the other canaries
    mkdir -p "$R/tasks"
    printf '%s\n' '{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"m7-default-task","chunk":16}' \
      '{"v":1,"kind":"turn","reply":{"think":"Write it.","calls":[{"name":"write","input":{"path":"ok.txt","content":"OK\n"}}],"stop":"tool_use","usage":{"input":6000,"cache_read":0,"output":30}}}' \
      '{"v":1,"kind":"turn","reply":{"text":"done","stop":"end_turn","usage":{"input":6000,"cache_read":5900,"output":5}}}' >"$R/tasks/_default.mock.jsonl"
    "$D/init-mockllm" --mode upstream --listen "127.0.0.1:$PORT" --strict deepseek --port-file "$R/mock.port" \
      --script "$R/check.mock.jsonl" --tasks-dir "$R/tasks" --record-dir "$R/mock" --record-bodies none >"$R/mock.out" 2>&1 &
    PIDS+=($!)
    for _ in $(seq 1 100); do [ -s "$R/mock.port" ] && break; sleep 0.1; done
    [ -s "$R/mock.port" ] || { cat "$R/mock.out" >&2; die "mock did not start"; }
    (umask 077; printf 'test-m7-mock-key' >"$RH/keys/providers/deepseek")
  else
    (umask 077; tr -d '\r\n' <"$KEYFILE" >"$RH/keys/providers/deepseek")
  fi
  chmod 600 "$RH/keys/providers/deepseek"
  "${RENV[@]}" "$RH/bin/init-root" keys check >"$R/keys-check.out" 2>&1 || true
  grep -q '^providers/deepseek: present' "$R/keys-check.out" || { cat "$R/keys-check.out" >&2; die "key not present"; }
  # a fresh agent session per line: a turn interrupted by an earlier stop must not resume at boot (it would spend
  # outside the protocol, and interrupting it makes the Root bill an estimated full stream). The old log stays.
  if [ -f "$IH/state/session" ]; then rm -f -- "$IH/state/session"; fi
  "${RENV[@]}" "$RH/bin/init-root" start --foreground >"$R/root.out" 2>&1 </dev/null &
  ROOT_PID=$!
  PIDS+=($ROOT_PID)
  STARTED=1
  for _ in $(seq 1 600); do [ -S "$RH/run/owner.sock" ] && break; sleep 0.05; done
  [ -S "$RH/run/owner.sock" ] || { cat "$R/root.out" >&2; die "owner.sock never appeared"; }
  local ready=0
  for _ in $(seq 1 240); do
    if "${RENV[@]}" "$RH/bin/init-root" status --json 2>/dev/null \
       | /usr/bin/python3 -I -c 'import json,sys; v=json.load(sys.stdin); sys.exit(0 if v.get("slot")=="running" else 1)' 2>/dev/null \
       && [ -f "$IH/state/run/ready" ]; then ready=1; break; fi
    sleep 0.5
  done
  [ $ready = 1 ] || die "kernel+body not ready in 120 s"
  if [ "$LINE" = D ]; then "${PY[@]}" prereg --ih "$IH" --rh "$RH" --repo "$REPO" >"$R/prereg.json" || die "D-0 freeze"; fi
  # the S pack for P's draw (`sealed draw --pack-dir`; passed, never read): real = pack s1, mock = ROOT-S1a's fixture pack
  local PACK; if [ $PROD = 1 ]; then PACK="${M7_PACK_DIR:-$REPO/root/sealed/packs/s1}"; else PACK="${M7_FIXTURE_PACK:-$REPO/root/sealed/packs/s1}"; fi
  local CAP_ARGS=(--ih "$IH" --rh "$RH" --run "$R" --repo "$REPO" --mode "$MODE" --home "$REAL_HOME" --wall-s "${M7_WALL_S:-3600}"
                  --pack-dir "$PACK")
  if [ $PROD = 1 ]; then
    curl -s --max-time 20 --unix-socket "$IH/run/proxy.sock" -H 'x-init-session: m7balance' -H 'x-init-purpose: turn' \
      -o "$R/balance-before.json" http://proxy/deepseek/user/balance || true
  fi
  set +e
  "${PY[@]}" run "$LINE" "${CAP_ARGS[@]}" >"$R/result.json" &
  local DRV=$!
  PIDS+=($DRV)
  # the disk/memory watchdog: the Root's tree and the driver, by PID (m7watch.py); a trip leaves $R/watch.trip
  "${WATCH[@]}" run --root-pid "$ROOT_PID" --pid "$DRV" --rh "$RH" --home "$REAL_HOME" --log "$R/watch.jsonl" \
    --trip-file "$R/watch.trip" --min-free-mb "$WATCH_MIN_FREE_MB" --mem-cap-mb "$MEM_CAP_MB" --poll-s 5 \
    >"$R/watch.out" 2>&1 &
  PIDS+=($!)
  wait "$DRV"
  local CRC=$?
  set -e
  if [ -f "$R/watch.trip" ]; then log "$LINE: WATCHDOG TRIP: $(cat "$R/watch.trip")"; fi
  if [ $CRC = 5 ]; then log "$LINE refused: $(cat "$R/result.json")"; cleanup; return 5; fi
  if [ $PROD = 1 ] && [ ! -f "$R/watch.trip" ]; then   # after a trip the Root (and its proxy) is gone
    # billing lags by minutes: wait for the balance to settle (two equal reads 60 s apart, at most 10 min)
    local prev="" cur
    for _ in $(seq 1 10); do
      sleep 60
      curl -s --max-time 20 --unix-socket "$IH/run/proxy.sock" -H 'x-init-session: m7balance' -H 'x-init-purpose: turn' \
        -o "$R/balance-after.json" http://proxy/deepseek/user/balance || true
      cur="$(cat "$R/balance-after.json" 2>/dev/null)"; [ -n "$cur" ] && [ "$cur" = "$prev" ] && break; prev="$cur"
    done
  fi
  curl -s --max-time 5 --unix-socket "$RH/run/egress.sock" http://root/_root/ceilings >"$R/ceilings-after.json" || true
  "${RENV[@]}" "$RH/bin/init-root" audit verify --full >"$R/audit-verify.out" 2>&1; echo "exit $?" >>"$R/audit-verify.out"
  "${RENV[@]}" "$RH/bin/init-root" status --json >"$R/status-end.json" 2>/dev/null || true
  cleanup                                                     # stop the Root, remove the key, before anything is copied
  # the kernel's build slot (G0/G3/E0 builds, ~1.6 GB each) is regenerable: removed by exact path after every line
  if [ -e "$IH/cache/build/slot" ]; then rm -rf -- "$IH/cache/build/slot"; log "$LINE: build slot removed; free $(free_mb) MB"; fi
  # finished kernel probes can leave their root: each clones INIT_HOME/kernel, whose imported pool is read-only, so the
  # kernel's remove_dir_all stops at the first EACCES (error ignored). With the Root stopped none is live: exact paths.
  local pr n=0
  for pr in "$IH"/probe/*; do
    case "${pr##*/}" in [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;; *) continue ;; esac
    [ -d "$pr" ] && [ ! -L "$pr" ] || continue
    chmod -R u+w "$pr" && rm -rf -- "$pr" && n=$((n + 1))
  done
  [ $n = 0 ] || log "$LINE: $n leftover probe roots removed; free $(free_mb) MB"
  "${PY[@]}" verdict "$LINE" "${CAP_ARGS[@]}" >"$R/verdict.json" || true
  if [ -e "$OUT" ]; then rm -rf -- "$OUT"; fi
  mkdir -p "$OUT"
  local KF=-; [ $PROD = 1 ] && KF="$KEYFILE"
  # never collected: ROOT_HOME/sealed (S instances, seeds, per-task slice detail), keys, request bodies
  /usr/bin/python3 -I "$REPO/scripts/slice1/collect.py" "$OUT" "$KF" "$R=run" "$D/ledger.json=ledger" "$D/charter.params.json=params" \
    "$RH/lineage/lineage.jsonl=root/lineage" "$RH/logs=root/logs" "$RH/egress/usage=root/egress-usage" \
    "$IH/kernel/config.toml=init/kernel" "$IH/state/sessions=init/sessions" "$IH/state/kernel/logs=init/kernel-logs" \
    "$IH/state/kernel/gate=init/gate" "$IH/state/kernel/evo=init/evo" "$IH/state/kernel/lkg.json=init/state" >"$R/collect.json" || true
  "${PY[@]}" redact "$OUT" >"$R/redact.json" || true
  cp "$R/collect.json" "$R/redact.json" "$OUT/run/" 2>/dev/null || true
  log "$LINE: collect $(cat "$R/collect.json") redact $(cat "$R/redact.json")"
  log "$LINE: verdict $(head -c 1500 "$R/verdict.json")"
  /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1])).get("pass") else 1)' "$R/verdict.json"
}

do_settle() { # mock only: boot the home with an "ok"-only script so a turn resumed from an interrupted line ends at once
  [ $PROD = 0 ] || die "settle is for the mock home only"
  local R="$D/runs/settle"; if [ -e "$R" ]; then rm -rf -- "$R"; fi; mkdir -p "$R/mock"
  /usr/bin/python3 -I -c 'import json,sys
U={"input":6000,"cache_read":0,"output":5}
L=[{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"m7-settle","chunk":16}]+[{"v":1,"kind":"turn","reply":{"text":"ok","stop":"end_turn","usage":U}}]*20
open(sys.argv[1],"w").write("".join(json.dumps(l)+"\n" for l in L))' "$R/check.mock.jsonl"
  "$D/init-mockllm" --mode upstream --listen "127.0.0.1:$(cat "$D/mock.port.fixed")" --strict deepseek --port-file "$R/mock.port" \
    --script "$R/check.mock.jsonl" --record-dir "$R/mock" --record-bodies none >"$R/mock.out" 2>&1 &
  PIDS+=($!)
  for _ in $(seq 1 100); do [ -s "$R/mock.port" ] && break; sleep 0.1; done
  (umask 077; printf 'test-m7-mock-key' >"$RH/keys/providers/deepseek")
  "${RENV[@]}" "$RH/bin/init-root" start --foreground >"$R/root.out" 2>&1 </dev/null &
  PIDS+=($!); STARTED=1
  for _ in $(seq 1 600); do [ -S "$RH/run/owner.sock" ] && break; sleep 0.05; done
  for _ in $(seq 1 240); do   # a fresh boot: the slot runs and `ready` is newer than this Root's start
    if "${RENV[@]}" "$RH/bin/init-root" status --json 2>/dev/null \
       | /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("slot")=="running" else 1)' 2>/dev/null \
       && [ "$IH/state/run/ready" -nt "$R/root.out" ]; then break; fi
    sleep 0.5
  done
  /usr/bin/python3 -I - "$REPO" "$IH" "$RH" "$R" "$REAL_HOME" <<'PYEOF'
import sys, types
sys.path.insert(0, sys.argv[1] + "/tests/real")
import checks as S
a = types.SimpleNamespace(ih=sys.argv[2], rh=sys.argv[3], run=sys.argv[4], home=sys.argv[5])
o = S.Owner(a)
o.drain(30)
print("busy after 30 s:", o.busy)
o.close()
PYEOF
  cleanup
  log "settled $D"
}

do_dry() {
  [ $PROD = 0 ] || die "dry runs on the mock only"
  [ -x "$RH/bin/init-root" ] || do_install
  local rc=0
  for L in P C G1D D M G1L; do
    log "dry: line $L"
    set +e; ( trap cleanup EXIT; do_run "$L" ); rc=$?; set -e
    if [ $rc = 5 ]; then log "dry: $L refused by the ledger/budget (expected for G1L unless D was accepted)"; continue; fi
    [ $rc = 0 ] || { log "dry: $L failed (rc $rc); see build/real-artifacts/m7/mock/$L and tests/real/m7.sh ready"; return $rc; }
  done
  log "dry: all lines green"
}

case "$CMD" in
  install) do_install ;;
  run) do_run "$@" ;;
  dry) do_dry ;;
  refresh) # mock home only: the current staged init-root and init-mockllm (new inodes); the kernel stays, since the Root
           # checks it against the tuple's hashes (a kernel change needs a reinstall)
    [ $PROD = 0 ] || die "refresh is for the mock home only"
    for pair in "$BIN/init-root:$RH/bin/init-root" "$BIN/init-mockllm:$D/init-mockllm"; do
      src="${pair%%:*}"; dst="${pair#*:}"; rm -f -- "$dst"; cp -c "$src" "$dst"
    done
    log "refreshed init-root and init-mockllm in $D" ;;
  settle) do_settle ;;
  ledger) "${PY[@]}" ledger --rh "$RH" ;;
  freeze) "${PY[@]}" freeze --rh "$RH" --home "$REAL_HOME" ;;   # emergency stop: every pool's ceiling := what it spent
  clean) rm_home; log "removed $D" ;;
  *) die "unknown command $CMD" ;;
esac
