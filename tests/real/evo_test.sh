#!/bin/bash
# EVO-TEST-PREP: the quick evolution test of ADR-011 (measurements/evo-test-protocol.md) on the REAL model, and the same
# harness at $0 on init-mockllm (the dry run). Logic lives in tests/real/evo_checks.py; REAL-M7's guard, watchdog and
# redaction (checks_m7.py, m7watch.py) and S-REAL's owner driver (checks.py) are reused unchanged.
#
#   tests/real/evo_test.sh selftest                  pure logic, the sandboxed scorer, the task set ($0; no Root, no key)
#   tests/real/evo_test.sh ready                     the ADR-011 blocker list for the staged binaries
#   tests/real/evo_test.sh stage-bins                clone the current debug binaries into target/evo-bin (APFS, ~0 disk)
#   tests/real/evo_test.sh <mock|real> install       one test home per mode; its pristine copy is snapshot G0
#   tests/real/evo_test.sh <mock|real> tasks         build the task set (once runs exist: only adds missing registered tasks)
#   EVO_REINSTALL=1 tests/real/evo_test.sh <mode> install   before phase 1 (protocol §10.5): new G0' from restaged bins,
#                                                    keeping the ledger, the task set and the phase-0 records
#   tests/real/evo_test.sh <mock|real> phase0|phase1|phase2   (v3: phase0 = genesis calibration)
#   EVO_I_MEAN_IT=1 EVO_GO=control tests/real/evo_test.sh real control   v3 control (Claude Code + deepseek-flash) on the
#                                                    tasks genesis failed 2/2, then the calibration classification
#   tests/real/evo_test.sh mock dry                  install (if needed) + phases 0-2 on the mock ($0; 1-2 stubbed
#                                                    until ADR-011 lands); EVO_ONLY=id,id,... limits the task set
#   tests/real/evo_test.sh mock offline-dry          $0, ~0 disk: the schedule, real task builds and sandboxed scoring,
#                                                    no home/Root/model (a stand-in agent); the dry run without an install
#   tests/real/evo_test.sh <mock|real> report|ledger|freeze|clean
#
# Layout (one set per mode x = m|r, all under target/evo/, removed by exact path):
#   x/{rh,ih}   the live test home (restored from a snapshot before each run; phase 1 keeps it)
#   x.snap/{g0,e1}   pristine genesis home, and the evolved home at the end of phase 1 (APFS clones)
#   x.tasks/    built tasks WITHOUT solution.tar (hidden checks inside: read-denied to the test home)
#   x.ws/<tag>  each run's workspace · x.runs/<tag> each run's driver files · x.ledger.json · x.attrib/<tag>.json
#
# Real-mode guards (the orchestrator gives the go per phase):
#   - EVO_I_MEAN_IT=1 and EVO_GO=p<phase> are both required; deepseek-flash only; phases 1-2 need the ADR-011 build
#     (`ready`: adr011_ready) and a home installed with extend.mode = "free";
#   - each run has a real cap (PREREG run_cap_musd); a guard prices the Root's usage every 5 s and reads the balance every
#     5 min; at the cap or the program's balance floor ($2.50 below the first balance) it freezes spend, interrupts, /stops;
#   - a run starts only if spent + its cap + the reservations of later phases <= $2.50, with its wall fully off-peak, and
#     with enough free disk; tests/real/m7watch.py runs beside every run and every scoring: free < 5500 MB stops the tree,
#     freezes spend and kills by PID; a single workload process above 4096 MB is killed alone;
#   - the key is copied from ~/.config/init-agent/deepseek.key with `tr` (never echoed, never on a command line) into the
#     test Root's key store, mode 0600, only in real mode after the guards, and removed in the EXIT trap and before any
#     collection. The mock and the dry run never touch the key file;
#   - collect.py redacts the key and `sk-` strings; evo_checks.py redact then removes e-mails and the git identity and
#     rescans (counts only). Every process runs under `env -i`.
# Safety: signals only to PIDs this script spawned. ~/.init and ~/.init-root are never touched. The test home runs with
# sandbox.read_home = false; its read_deny lists fixtures/, tests/, measurements/, root/, .git and x.tasks, x.snap.
set -euo pipefail
umask 077

REPO="$(cd "$(dirname "$0")/../.." && pwd -P)"
PY=(/usr/bin/python3 -I "$REPO/tests/real/evo_checks.py")
WATCH=(/usr/bin/python3 -I "$REPO/tests/real/m7watch.py")
BIN="${EVO_BIN_DIR:-$REPO/target/evo-bin}"
log() { printf '[evo %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { log "FAIL: $*"; exit 1; }
free_mb() { df -m / | awk 'NR==2{print $4}'; }
pre() { /usr/bin/python3 -I -B -c 'import json,sys; sys.path.insert(0, sys.argv[1]); import evo_checks as E; print(json.dumps(E.PREREG[sys.argv[2]]) if sys.argv[3] == "" else E.PREREG[sys.argv[2]][sys.argv[3]])' "$REPO/tests/real" "$1" "${2:-}"; }
jget() { /usr/bin/python3 -I -c 'import json,sys; v=json.load(open(sys.argv[1]))
for k in sys.argv[2].split("."): v = v.get(k) if isinstance(v, dict) else None
print("" if v is None else v)' "$1" "$2"; }

case "${1:-}" in
  selftest)
    "${PY[@]}" selftest ${EVO_SELFTEST_NO_TASKS:+--no-tasks}
    exec /usr/bin/python3 -I "$REPO/tests/real/m7watch.py" selftest ;;
  ready) exec "${PY[@]}" ready --bins "$BIN" ;;
  stage-bins)
    mkdir -p "$BIN"
    for b in "$REPO/root/target/debug/init-root" "$REPO/kernel/target/debug/init-kernel" "$REPO/kernel/target/debug/initctl" \
             "$REPO/kernel/target/debug/init-mockllm"; do
      [ -x "$b" ] || die "missing $b (build it first, through ci/slot.sh)"
      n="$BIN/$(basename "$b")"; if [ -e "$n" ]; then rm -f -- "$n"; fi
      cp -c "$b" "$n"
    done
    ls -la "$BIN" >&2; exit 0 ;;
esac

MODE="${1:-}"; CMD="${2:-}"
case "$MODE" in mock) X=m ;; real) X=r ;; *) echo "usage: $0 <selftest|ready|stage-bins|mock|real> ..." >&2; exit 64 ;; esac
shift 2 || true
PROD=0; [ "$MODE" = real ] && PROD=1
STARTED=0
BASE="$REPO/target/evo"
D="$BASE/$X"; RH="$D/rh"; IH="$D/ih"
SNAP="$BASE/$X.snap"; TASKS="$BASE/$X.tasks"; WSR="$BASE/$X.ws"; RUNS="$BASE/$X.runs"; ATT="$BASE/$X.attrib"
LED="$BASE/$X.ledger.json"; REL="$BASE/$X.rel"; XB="$BASE/$X.bin"; EXTF="$BASE/$X.extend"
ART="$REPO/build/real-artifacts/${EVO_ART:-evo-v3}/$MODE"   # v2's bigfile run is build/real-artifacts/evo/real
REAL_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
TC="$(cd "$REAL_HOME/.rustup/toolchains/stable-aarch64-apple-darwin" && pwd -P)"
DEVDIR=/Library/Developer/CommandLineTools
KEYFILE="$REAL_HOME/.config/init-agent/deepseek.key"
SEED_TAG=seed-v0.2.0
WATCH_MIN="$(pre watch_min_free_mb)"; MEMCAP="$(pre mem_cap_mb)"
[ ${#RH} -le 64 ] && [ ${#IH} -le 64 ] || die "homes exceed 64 bytes ($RH)"
case "$D" in "$REAL_HOME/.init"*|"$REAL_HOME/.init-root"*) die "refusing a live home path" ;; esac
RENV=(env -i HOME="$REAL_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin)

PIDS=()
cleanup() {
  set +eu
  if [ -S "$RH/run/owner.sock" ] && [ "${STARTED:-0}" = 1 ]; then
    "${RENV[@]}" "$RH/bin/init-root" stop >>"$BASE/$X.stop.out" 2>&1
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

root_live() { # a live Root accepts a connection on its owner socket (a stale socket file refuses it)
  [ -S "$RH/run/owner.sock" ] && /usr/bin/python3 -I -c 'import socket,sys
s=socket.socket(socket.AF_UNIX); s.settimeout(2)
try: s.connect(sys.argv[1]); sys.exit(0)
except OSError: sys.exit(1)' "$RH/run/owner.sock"
}

rm_exact() { # $1: a path this script created; read-only parts made writable first
  if [ -e "$1" ] || [ -L "$1" ]; then chmod -R u+w "$1" 2>/dev/null; rm -rf -- "$1"; fi
}

drop_sockets() { # stale socket files of a stopped home (recreated at start); exact names only
  local s
  for s in "$RH"/run/*.sock "$IH"/run/*.sock; do [ -S "$s" ] && rm -f -- "$s"; done
  return 0
}

snapshot() { # $1 = g0|e1: an APFS clone of the stopped live home
  root_live && die "a Root is running in $D"
  drop_sockets
  mkdir -p "$SNAP"
  rm_exact "$SNAP/$1"
  cp -cR "$D" "$SNAP/$1"
  log "snapshot $1 taken; free $(free_mb) MB"
}

restore() { # $1 = g0|e1: the live home := a fresh clone of the snapshot (same path, so every absolute path holds)
  [ -d "$SNAP/$1" ] || die "no snapshot $1"
  root_live && die "a Root is running in $D"
  rm_exact "$D"
  cp -cR "$SNAP/$1" "$D"
}

# ================================================================ install (once per mode; snapshot G0)
do_install() {
  local need; need="$(pre start_min_free_mb install)"
  [ "$(free_mb)" -ge "$need" ] || die "only $(free_mb) MB free; an install (seed body build) needs $need MB"
  for b in init-root init-kernel initctl init-mockllm; do [ -x "$BIN/$b" ] || die "missing $BIN/$b (tests/real/evo_test.sh stage-bins)"; done
  root_live && die "a Root is running in $D"
  if [ "${EVO_REINSTALL:-}" = 1 ]; then
    # protocol §10.5: before phase 1, a new genesis G0' from the restaged binaries and the seed at HEAD; the ledger,
    # the task set, the runs and the attribution files stay (phase 0 is selection only and is not re-run)
    [ -f "$LED" ] || die "reinstall needs an existing program ledger"
    [ "$(jget "$LED" p1.started)" != True ] || die "phase 1 has started: no reinstall"
    for p in "$D" "$SNAP" "$REL" "$XB" "$EXTF" "$BASE/$X.seed.bundle"; do rm_exact "$p"; done
  else
    if [ $PROD = 1 ] && [ -f "$LED" ]; then die "a real ledger exists ($LED): archive it by hand before a reinstall"; fi
    for p in "$D" "$SNAP" "$REL" "$XB" "$TASKS" "$WSR" "$RUNS" "$ATT" "$LED" "$EXTF" "$BASE/$X.seed.bundle"; do rm_exact "$p"; done
  fi
  mkdir -p "$REL" "$XB" "$RH"
  cp -c "$BIN/init-root" "$XB/init-root"; cp -c "$BIN/init-mockllm" "$XB/init-mockllm"
  for b in init-kernel initctl init-mockllm; do cp -c "$BIN/$b" "$REL/$b"; done
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
  local SRC="$BASE/$X.seedsrc"; rm_exact "$SRC"
  /usr/bin/python3 -I "$REPO/scripts/slice1/seedpack.py" --repo "$REPO" --dst "$SRC" --kind repo --rev "${EVO_SEED_REV:-HEAD}" \
    >"$BASE/$X.seedsrc.json" || die "seed packaging"
  local G=(env -i PATH=/usr/bin:/bin HOME="$BASE" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null TZ=UTC
     GIT_AUTHOR_DATE=2026-10-22T00:00:00Z GIT_COMMITTER_DATE=2026-10-22T00:00:00Z
     /usr/bin/git -C "$SRC" -c core.hooksPath=/dev/null -c user.name=init-maintainer -c user.email=maintainer@init.invalid
     -c commit.gpgsign=false -c tag.gpgsign=false -c gc.auto=0 -c maintenance.auto=false)
  "${G[@]}" init -q -b seed
  "${G[@]}" add -A
  "${G[@]}" commit -q -m "seed $SEED_TAG (EVO-TEST seed; source ${EVO_SEED_REV:-HEAD})"
  "${G[@]}" tag "$SEED_TAG"
  "${G[@]}" rev-parse HEAD >"$BASE/$X.seed.sha"
  "${G[@]}" bundle create -q "$BASE/$X.seed.bundle" refs/heads/seed "refs/tags/$SEED_TAG"
  rm_exact "$SRC"
  local PORT=""
  if [ $PROD = 0 ]; then
    PORT="$(/usr/bin/python3 -I -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])')"
    echo "$PORT" >"$BASE/$X.mock.port"
  fi
  local EXT="${EVO_EXTEND:-}"
  if [ -z "$EXT" ]; then
    EXT="$("${PY[@]}" ready --bins "$BIN" --json | /usr/bin/python3 -I -c 'import json,sys; print("free" if json.load(sys.stdin)["adr011_ready"] else "gated")')"
  fi
  echo "$EXT" >"$EXTF"
  "${PY[@]}" params --mode "$MODE" --rh "$RH" --x "$X" --cap-musd "$(pre run_cap_musd p0)" ${PORT:+--mock-port "$PORT"} \
    --toolchain "$TC" --devdir "$DEVDIR" --extend "$EXT" --out "$BASE/$X.params.json"
  "${PY[@]}" charter --params "$BASE/$X.params.json" --out "$BASE/$X.charter.toml"
  set +e
  env -i HOME="$REAL_HOME" INIT_ROOT_HOME="$RH" INIT_HOME="$IH" PATH=/usr/bin:/bin "$XB/init-root" install --release "$REL" \
    --charter "$BASE/$X.charter.toml" --toolchain "$TC" --seed-source "$BASE/$X.seed.bundle" --no-service >"$BASE/$X.install.out" 2>&1
  local IRC=$?
  set -e
  [ $IRC = 0 ] || { tail -20 "$BASE/$X.install.out" >&2; die "install (exit $IRC)"; }
  rm_exact "$IH/cache/build/slot"
  "${PY[@]}" kconfig --params "$BASE/$X.params.json" --out "$IH/kernel/config.toml"; chmod 644 "$IH/kernel/config.toml"
  [ "${EVO_SKIP_TASKS:-}" = 1 ] || [ "${EVO_REINSTALL:-}" = 1 ] || do_tasks
  snapshot g0
  if [ "${EVO_REINSTALL:-}" = 1 ]; then
    "${PY[@]}" note --ledger "$LED" --text "reinstall (protocol 10.5): G0' from $(shasum -a 256 "$BIN/init-root" "$BIN/init-kernel" | cut -c1-16 | tr '\n' ' ')seed $(cut -c1-12 "$BASE/$X.seed.sha"), extend $EXT"
  fi
  log "installed $D (extend $EXT; $(du -sm "$D" | cut -f1) MB apparent); free $(free_mb) MB"
}

do_tasks() { # the task set: references scored 1000 sandboxed, then copied WITHOUT solution.tar (outside readable paths)
  local ONLY="${EVO_ONLY:-$("${PY[@]}" roles)}"
  if [ -f "$LED" ] && [ "$(jget "$LED" runs)" != "{}" ] && [ "$(jget "$LED" runs)" != "" ]; then
    # runs exist: built tasks are frozen; only registered tasks not built yet (e.g. the spare) may be added
    local t add=""
    for t in ${ONLY//,/ }; do [ -d "$TASKS/$t" ] || add="$add${add:+,}$t"; done
    [ -n "$add" ] || { log "tasks: nothing to add"; return 0; }
    local NEW="$TASKS.add"; rm_exact "$NEW"
    "${PY[@]}" build-tasks --out "$NEW" --only "$add" >"$BASE/$X.tasks.add.out" || { cat "$BASE/$X.tasks.add.out" >&2; die "build-tasks (add)"; }
    /usr/bin/python3 -I -c 'import json,os,shutil,sys
new, old = sys.argv[1], sys.argv[2]
a, b = json.load(open(os.path.join(new, "index.json"))), json.load(open(os.path.join(old, "index.json")))
for t, v in a["tasks"].items():
    shutil.move(os.path.join(new, t), os.path.join(old, t)); b["tasks"][t] = v
json.dump(b, open(os.path.join(old, "index.json"), "w"), sort_keys=True, indent=1)' "$NEW" "$TASKS"
    rm_exact "$NEW"
    log "tasks added: $add"
    return 0
  fi
  "${PY[@]}" build-tasks --out "$TASKS" --only "$ONLY" >"$BASE/$X.tasks.out" || { cat "$BASE/$X.tasks.out" >&2; die "build-tasks"; }
  log "tasks: $(cat "$BASE/$X.tasks.out")"
}

# ================================================================ one run
start_root() { # $1 = run dir
  local R="$1"
  "${RENV[@]}" "$RH/bin/init-root" start --foreground >"$R/root.out" 2>&1 </dev/null &
  ROOT_PID=$!
  PIDS+=($ROOT_PID)
  STARTED=1
  for _ in $(seq 1 600); do [ -S "$RH/run/owner.sock" ] && break; sleep 0.05; done
  [ -S "$RH/run/owner.sock" ] || { tail -5 "$R/root.out" >&2; return 1; }
  for _ in $(seq 1 240); do
    if "${RENV[@]}" "$RH/bin/init-root" status --json 2>/dev/null \
       | /usr/bin/python3 -I -c 'import json,sys; v=json.load(sys.stdin); sys.exit(0 if v.get("slot")=="running" else 1)' 2>/dev/null \
       && [ -f "$IH/state/run/ready" ] && [ "$IH/state/run/ready" -nt "$R/root.out" ]; then return 0; fi
    sleep 0.5
  done
  return 1
}

score_run() { # $1 run dir, $2 task dir, $3 workspace: the check, sandboxed, under the disk/memory watchdog
  local R="$1"
  if [ "$(jget "$R/result.json" infra)" = "" ]; then
    set +e
    "${PY[@]}" score --task-dir "$2" --ws "$3" --out "$R/score.json" >"$R/score.out" 2>&1 &
    local SC=$!
    "${WATCH[@]}" run --root-pid "$SC" --rh "$RH" --home "$REAL_HOME" --log "$R/score-watch.jsonl" --trip-file "$R/score-watch.trip" \
      --min-free-mb "$WATCH_MIN" --mem-cap-mb "$MEMCAP" --poll-s 2 --freeze-cmd '["/usr/bin/true"]' >"$R/score-watch.out" 2>&1 &
    local SW=$!
    wait "$SC"; wait "$SW"
    set -e
    [ ! -f "$R/score-watch.trip" ] || log "scoring watchdog trip: $(cat "$R/score-watch.trip")"
  fi
  [ -f "$R/score.json" ] || printf '{"score":0,"note":"not scored"}\n' >"$R/score.json"
}

do_run_spec() { # $1 = spec file. rc 0 recorded · 5 stop the phase (budget, halt) · 7 disk
  local SPEC="$1"
  local TAG PHASE TASK RESTORE CAP
  TAG="$(jget "$SPEC" tag)"; PHASE="$(jget "$SPEC" phase)"; TASK="$(jget "$SPEC" task)"; RESTORE="$(jget "$SPEC" restore)"
  CAP="$(jget "$SPEC" cap_musd)"
  local R="$RUNS/$TAG" WS="$WSR/$TAG" OUT="$ART/$TAG"
  rm_exact "$R"; mkdir -p "$R/mock" "$R/tasks" "$ATT"
  cp "$SPEC" "$R/spec.json"
  local rc waited=0
  while :; do
    set +e; "${PY[@]}" can-start --ledger "$LED" --spec "$R/spec.json" --mode "$MODE" ${EVO_OFFLINE:+--no-disk} >"$R/can-start.out"; rc=$?; set -e
    [ $rc = 6 ] || break
    [ $waited -lt "${EVO_MAX_WAIT_S:-14400}" ] || { log "$TAG: still peak after ${waited}s; stopping (resumable)"; return 5; }
    log "$TAG: waiting for off-peak ($(cat "$R/can-start.out"))"; sleep 300; waited=$((waited + 300))
  done
  [ $rc = 0 ] || { log "$TAG: not started: $(cat "$R/can-start.out")"; return $rc; }
  if [ "${EVO_OFFLINE:-}" = 1 ]; then   # $0, no disk: no home and no Root; the stand-in agent writes one note
    "${PY[@]}" prep-ws --task-dir "$TASKS/$TASK" --ws "$WS"
    printf 'evo offline dry run\n' >"$WS/NOTES.dryrun.txt"
    /usr/bin/python3 -I -c 'import json,sys,time; t=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
json.dump({"rc": 0, "t0": t, "t1": t, "real_est_musd": 0, "offline": True, "turn": {"rc": 0, "model_turns": 0}}, open(sys.argv[1], "w"))' "$R/result.json"
    score_run "$R" "$TASKS/$TASK" "$WS"
    "${PY[@]}" record --ledger "$LED" --spec "$R/spec.json" --result "$R/result.json" --score "$R/score.json" >"$R/record.json"
    log "$TAG: $(cat "$R/record.json")"
    return 0
  fi
  # the home
  case "$RESTORE" in g0|e1) restore "$RESTORE" ;; keep) ;; *) die "bad restore $RESTORE" ;; esac
  if [ "$PHASE" = 1 ] && [ "$RESTORE" = g0 ]; then "${PY[@]}" mark --ledger "$LED" --key p1.started; fi
  # the workspace and the per-run config (charter ceilings above what the home has spent; kernel task backstop)
  "${PY[@]}" prep-ws --task-dir "$TASKS/$TASK" --ws "$WS"
  local PORT=""; [ $PROD = 0 ] && PORT="$(cat "$BASE/$X.mock.port")"
  "${PY[@]}" params --mode "$MODE" --rh "$RH" --x "$X" --cap-musd "$CAP" ${PORT:+--mock-port "$PORT"} --toolchain "$TC" \
    --devdir "$DEVDIR" --extend "$(cat "$EXTF")" --out "$R/params.json"
  "${PY[@]}" charter --params "$R/params.json" --out "$RH/charter.toml"
  chmod 600 "$RH/charter.toml"
  "${RENV[@]}" "$RH/bin/init-root" charter check --apply --yes >"$R/charter-apply.out" 2>&1 || { cat "$R/charter-apply.out" >&2; die "charter apply"; }
  "${PY[@]}" kconfig --params "$R/params.json" --out "$IH/kernel/config.toml"; chmod 644 "$IH/kernel/config.toml"
  # the key (real) or the mock upstream
  if [ $PROD = 0 ]; then
    "${PY[@]}" mock-script --out "$R/check.mock.jsonl" --name "evo-$TAG"
    printf '%s\n' '{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"evo-default","chunk":16}' \
      '{"v":1,"kind":"turn","reply":{"text":"ok","stop":"end_turn","usage":{"input":6000,"cache_read":0,"output":5}}}' >"$R/tasks/_default.mock.jsonl"
    "$XB/init-mockllm" --mode upstream --listen "127.0.0.1:$PORT" --strict deepseek --port-file "$R/mock.port" \
      --script "$R/check.mock.jsonl" --tasks-dir "$R/tasks" --record-dir "$R/mock" --record-bodies none >"$R/mock.out" 2>&1 &
    PIDS+=($!)
    for _ in $(seq 1 100); do [ -s "$R/mock.port" ] && break; sleep 0.1; done
    [ -s "$R/mock.port" ] || { cat "$R/mock.out" >&2; die "mock did not start"; }
    (umask 077; printf 'test-evo-mock-key' >"$RH/keys/providers/deepseek")
  else
    (umask 077; tr -d '\r\n' <"$KEYFILE" >"$RH/keys/providers/deepseek")
  fi
  chmod 600 "$RH/keys/providers/deepseek"
  # a fresh agent session per run (DV-3): no turn resumes from an earlier run
  if [ -f "$IH/state/session" ]; then rm -f -- "$IH/state/session"; fi
  start_root "$R" || { cleanup; log "$TAG: the Root or the body did not become ready"; printf '{"rc":4,"infra":"not_ready"}\n' >"$R/result.json"; }
  local FLOOR; FLOOR="$(jget "$LED" program.floor_musd)"; [ -n "$FLOOR" ] || FLOOR=-1
  if [ -f "$R/result.json" ]; then :; else
    set +e
    "${PY[@]}" run-one --ih "$IH" --rh "$RH" --home "$REAL_HOME" --run "$R" --mode "$MODE" --ws "$WS" --task-dir "$TASKS/$TASK" \
      --spec "$R/spec.json" --params "$R/params.json" --floor-musd "$FLOOR" >"$R/run-one.out" 2>&1 &
    local DRV=$!
    PIDS+=($DRV)
    "${WATCH[@]}" run --root-pid "$ROOT_PID" --pid "$DRV" --rh "$RH" --home "$REAL_HOME" --log "$R/watch.jsonl" \
      --trip-file "$R/watch.trip" --min-free-mb "$WATCH_MIN" --mem-cap-mb "$MEMCAP" --poll-s 5 \
      --freeze-cmd "$(/usr/bin/python3 -I -c 'import json,sys; print(json.dumps(sys.argv[1:]))' /usr/bin/python3 -I "$REPO/tests/real/evo_checks.py" freeze --rh "$RH" --home "$REAL_HOME" --params "$R/params.json")" \
      >"$R/watch.out" 2>&1 &
    PIDS+=($!)
    wait "$DRV"; local DRC=$?
    set -e
    if [ -f "$R/watch.trip" ]; then
      log "$TAG: WATCHDOG TRIP: $(cat "$R/watch.trip")"
      /usr/bin/python3 -I -c 'import json,sys; p=sys.argv[1]
try: r=json.load(open(p))
except Exception: r={}
r.update(rc=4, infra="watchdog_trip"); json.dump(r, open(p,"w"))' "$R/result.json"
    fi
    log "$TAG: driver rc $DRC"
    curl -s --max-time 5 --unix-socket "$RH/run/egress.sock" http://root/_root/ceilings >"$R/ceilings-after.json" 2>/dev/null || true
    "${RENV[@]}" "$RH/bin/init-root" audit verify --full >"$R/audit-verify.out" 2>&1; echo "exit $?" >>"$R/audit-verify.out"
    "${RENV[@]}" "$RH/bin/init-root" status --json >"$R/status-end.json" 2>/dev/null || true
  fi
  cleanup                                                         # stop the Root, remove the key, before anything is read
  rm_exact "$IH/cache/build/slot"                                 # regenerable (~1.6 GB per T2 build), by exact path
  # attribution (capabilities created and used in this run's window), with the Root stopped
  local T0 T1; T0="$(jget "$R/result.json" t0)"; T1="$(jget "$R/result.json" t1)"
  [ -n "$T0" ] || T0="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  "${PY[@]}" attrib --rh "$RH" --ih "$IH" --since "$T0" ${T1:+--until "$T1"} --seed-sha "$(cat "$BASE/$X.seed.sha")" --out "$ATT/$TAG.json" || true
  # the score: the task's check on the workspace, sandboxed, under the disk/memory watchdog
  score_run "$R" "$TASKS/$TASK" "$WS"
  # artifacts: redacted copies (never request bodies, keys, the task set or the Root's sealed state)
  rm_exact "$OUT"; mkdir -p "$OUT"
  local KF=-; [ $PROD = 1 ] && KF="$KEYFILE"
  /usr/bin/python3 -I "$REPO/scripts/slice1/collect.py" "$OUT" "$KF" "$R=run" "$ATT/$TAG.json=attrib" \
    "$RH/lineage/lineage.jsonl=root/lineage" "$RH/logs=root/logs" "$RH/egress/usage=root/egress-usage" "$RH/extend=root/extend" \
    "$IH/root=init/root-view" "$IH/kernel/config.toml=init/kernel" "$IH/state/sessions=init/sessions" \
    "$IH/state/kernel/logs=init/kernel-logs" "$IH/state/kernel/lkg.json=init/state" "$IH/state/ext/catalog.json=init/ext" >"$R/collect.json" || true
  "${PY[@]}" redact "$OUT" >"$R/redact.json" || true
  "${PY[@]}" record --ledger "$LED" --spec "$R/spec.json" --result "$R/result.json" --score "$R/score.json" >"$R/record.json"
  cp "$R/collect.json" "$R/redact.json" "$R/record.json" "$OUT/run/" 2>/dev/null || true
  log "$TAG: $(cat "$R/record.json") · redact $(cat "$R/redact.json") · free $(free_mb) MB"
  [ "$(jget "$LED" halt)" = "" ] || { log "HALT: $(jget "$LED" halt)"; return 5; }
  return 0
}

# ================================================================ the control (protocol v3 §11.3): Claude Code + deepseek-flash
do_control_spec() { # $1 = spec file. rc 0 recorded · 5 budget/halt · 7 disk
  local SPEC="$1" TAG TASK rc waited=0
  TAG="$(jget "$SPEC" tag)"; TASK="$(jget "$SPEC" task)"
  local R="$RUNS/$TAG" CW="$BASE/$X.cws/$TAG" CC="$BASE/$X.ccfg/$TAG" CT="$BASE/$X.ctmp/$TAG" OUT="$ART/$TAG"
  rm_exact "$R"; mkdir -p "$R" "$ATT"
  cp "$SPEC" "$R/spec.json"
  while :; do
    set +e; "${PY[@]}" can-start --ledger "$LED" --spec "$R/spec.json" --mode "$MODE" >"$R/can-start.out"; rc=$?; set -e
    [ $rc = 6 ] || break
    [ $waited -lt "${EVO_MAX_WAIT_S:-14400}" ] || { log "$TAG: still peak after ${waited}s; stopping (resumable)"; return 5; }
    log "$TAG: waiting for off-peak ($(cat "$R/can-start.out"))"; sleep 300; waited=$((waited + 300))
  done
  [ $rc = 0 ] || { log "$TAG: not started: $(cat "$R/can-start.out")"; return $rc; }
  for p in "$CW" "$CC" "$CT"; do rm_exact "$p"; done
  "${PY[@]}" prep-ws --task-dir "$TASKS/$TASK" --ws "$CW"
  local FLOOR; FLOOR="$(jget "$LED" program.floor_musd)"; [ -n "$FLOOR" ] || FLOOR=-1
  set +e
  "${PY[@]}" control-run --run "$R" --mode "$MODE" --ws "$CW" --cfg "$CC" --tmp "$CT" --task-dir "$TASKS/$TASK" \
    --spec "$R/spec.json" --home "$REAL_HOME" --floor-musd "$FLOOR" >"$R/control-run.out" 2>&1 &
  local DRV=$!
  "${WATCH[@]}" run --root-pid "$DRV" --rh "$RH" --home "$REAL_HOME" --log "$R/watch.jsonl" --trip-file "$R/watch.trip" \
    --min-free-mb "$WATCH_MIN" --mem-cap-mb "$MEMCAP" --poll-s 5 --freeze-cmd '["/usr/bin/true"]' >"$R/watch.out" 2>&1 &
  local W=$!
  wait "$DRV"; local DRC=$?; wait "$W"
  set -e
  [ ! -f "$R/watch.trip" ] || { log "$TAG: WATCHDOG TRIP: $(cat "$R/watch.trip")"; /usr/bin/python3 -I -c 'import json,sys; p=sys.argv[1]
try: r=json.load(open(p))
except Exception: r={}
r.update(rc=4, infra="watchdog_trip"); json.dump(r, open(p,"w"))' "$R/result.json"; }
  [ -f "$R/result.json" ] || printf '{"rc":4,"infra":"no_result"}\n' >"$R/result.json"
  log "$TAG: control rc $DRC"
  score_run "$R" "$TASKS/$TASK" "$CW"
  rm_exact "$OUT"; mkdir -p "$OUT"
  local KF=-; [ $PROD = 1 ] && KF="$KEYFILE"
  /usr/bin/python3 -I "$REPO/scripts/slice1/collect.py" "$OUT" "$KF" "$R=run" >"$R/collect.json" || true
  "${PY[@]}" redact "$OUT" >"$R/redact.json" || true
  "${PY[@]}" record --ledger "$LED" --spec "$R/spec.json" --result "$R/result.json" --score "$R/score.json" >"$R/record.json"
  cp "$R/collect.json" "$R/redact.json" "$R/record.json" "$OUT/run/" 2>/dev/null || true
  rm_exact "$CC"; rm_exact "$CT"
  log "$TAG: $(cat "$R/record.json") · redact $(cat "$R/redact.json") · free $(free_mb) MB"
  [ "$(jget "$LED" halt)" = "" ] || { log "HALT: $(jget "$LED" halt)"; return 5; }
  return 0
}

do_control() {
  [ $PROD = 1 ] || die "the control runs on the real model only (DeepSeek's Anthropic endpoint)"
  [ "${EVO_I_MEAN_IT:-}" = 1 ] || die "real mode spends money: set EVO_I_MEAN_IT=1"
  [ "${EVO_GO:-}" = control ] || die "the control needs the orchestrator's go: EVO_GO=control"
  [ -f "$KEYFILE" ] || die "no key file"
  [ -f "$TASKS/index.json" ] || die "no task set"
  local rc tag
  while :; do
    tag="$("${PY[@]}" next-control --ledger "$LED" --tasks "$TASKS" --out "$BASE/$X.next.json")" || die "control: no schedule"
    [ -n "$tag" ] || break
    log "control: run $tag"
    set +e; ( trap cleanup EXIT; do_control_spec "$BASE/$X.next.json" ); rc=$?; set -e
    case $rc in 0) ;; 5) log "control stopped (budget or halt)"; break ;; 7) log "control paused: disk"; return 7 ;; *) return $rc ;; esac
  done
  set +e; "${PY[@]}" classify --ledger "$LED" --tasks "$TASKS" >"$BASE/$X.classify.json"; rc=$?; set -e
  log "calibration: $(cat "$BASE/$X.classify.json")"
  return $rc
}

# ================================================================ phases
need_go() { # $1 = p0|p1|p2
  [ "${EVO_OFFLINE:-}" = 1 ] || [ -d "$SNAP/g0" ] || die "not installed: $0 $MODE install"
  if [ $PROD = 1 ]; then
    [ "${EVO_I_MEAN_IT:-}" = 1 ] || die "real mode spends money: set EVO_I_MEAN_IT=1"
    [ "${EVO_GO:-}" = "$1" ] || die "real phase $1 needs the orchestrator's go: EVO_GO=$1"
    [ -f "$KEYFILE" ] || die "no key file at ~/.config/init-agent/deepseek.key"
    if [ "$1" != p0 ]; then
      [ "$(cat "$EXTF")" = free ] || die "phase $1 needs a home installed with extend.mode = free (the ADR-011 build): $0 ready"
    fi
  fi
}

do_phase() { # $1 = 0|1|2
  local PH="$1" rc tag
  need_go "p$PH"
  [ -f "$TASKS/index.json" ] || die "no task set: $0 $MODE tasks"
  case "$PH" in
    1) [ "$(jget "$LED" sets)" != "" ] || die "phase 1 needs phase 0's kept sets (run phase0)" ;;
    2) [ "$(jget "$LED" p1.e1)" = True ] || die "phase 2 needs snapshot E1 (finish phase1)" ;;
  esac
  [ $PROD = 1 ] || [ "$(cat "$EXTF")" = free ] || [ "$PH" = 0 ] || log "phase $PH runs in STUB mode on the mock (ADR-011 not built: no [extend], zone-side attribution)"
  while :; do
    tag="$("${PY[@]}" next --ledger "$LED" --tasks "$TASKS" --phase "$PH" --mode "$MODE" --out "$BASE/$X.next.json")" \
      || die "phase $PH: no schedule (see above)"
    [ -n "$tag" ] || break
    log "phase $PH: run $tag"
    set +e; ( trap cleanup EXIT; do_run_spec "$BASE/$X.next.json" ); rc=$?; set -e
    case $rc in
      0) ;;
      5) log "phase $PH stopped (budget or halt)"; break ;;
      7) log "phase $PH paused: not enough free disk (resumable)"; return 7 ;;
      *) log "phase $PH: run $tag failed (rc $rc)"; return $rc ;;
    esac
  done
  if [ "$PH" = 0 ]; then
    set +e; "${PY[@]}" classify --ledger "$LED" --tasks "$TASKS" >"$BASE/$X.classify.json"; rc=$?; set -e
    log "phase 0: $(cat "$BASE/$X.classify.json")"
    return $rc
  fi
  if [ "$PH" = 1 ] && [ "$(jget "$LED" halt)" = "" ]; then
    [ "${EVO_OFFLINE:-}" = 1 ] || snapshot e1
    "${PY[@]}" mark --ledger "$LED" --key p1.done
    "${PY[@]}" mark --ledger "$LED" --key p1.e1
  fi
  return 0
}

do_report() {
  mkdir -p "$ART"
  "${PY[@]}" report --ledger "$LED" --attrib-dir "$ATT" --out "$ART/report.json"
  cp "$LED" "$ART/ledger.json"
  "${PY[@]}" redact "$ART" >/dev/null
}

case "$CMD" in
  install) do_install ;;
  tasks) do_tasks ;;
  phase0) do_phase 0 ;;
  control) do_control ;;
  phase1) do_phase 1 ;;
  phase2) do_phase 2 ;;
  dry)
    [ $PROD = 0 ] || die "dry runs on the mock only"
    export EVO_MOCK_FAST=1
    [ -d "$SNAP/g0" ] || do_install
    [ -f "$TASKS/index.json" ] || do_tasks
    do_phase 0 || die "dry: phase 0 (rc $?)"
    do_phase 1 || die "dry: phase 1"
    do_phase 2 || die "dry: phase 2"
    do_report
    log "dry: phases 0-2 done; report $ART/report.json" ;;
  offline-dry)
    # $0 and ~0 disk: the whole schedule (phases 0-2, classification, ledger, report) with real task builds and real
    # sandboxed scoring under the watchdog, but no home, no Root and no model: a stand-in agent writes one note per run
    [ $PROD = 0 ] || die "offline-dry runs on the mock set only"
    export EVO_OFFLINE=1
    for p in "$TASKS" "$WSR" "$RUNS" "$ATT" "$LED"; do rm_exact "$p"; done
    echo gated >"$EXTF"
    "${PY[@]}" build-tasks --out "$TASKS" --only "${EVO_ONLY:-$("${PY[@]}" roles)}" >"$BASE/$X.tasks.out" || { cat "$BASE/$X.tasks.out" >&2; die "build-tasks"; }
    log "offline-dry: tasks $(cat "$BASE/$X.tasks.out")"
    do_phase 0 || die "offline-dry: phase 0 (rc $?)"
    do_phase 1 || die "offline-dry: phase 1"
    do_phase 2 || die "offline-dry: phase 2"
    do_report
    log "offline-dry: phases 0-2 done; report $ART/report.json" ;;
  report) do_report ;;
  ledger) cat "$LED" ;;
  freeze) [ -f "$BASE/$X.params.json" ] && "${PY[@]}" freeze --rh "$RH" --home "$REAL_HOME" --params "$BASE/$X.params.json" ;;
  clean)
    root_live && die "a Root is running in $D"
    for p in "$D" "$SNAP" "$REL" "$XB" "$TASKS" "$WSR" "$RUNS" "$ATT" "$LED" "$EXTF"; do rm_exact "$p"; done
    for f in seed.bundle seed.sha seedsrc.json mock.port params.json charter.toml install.out tasks.out next.json classify.json stop.out; do
      rm_exact "$BASE/$X.$f"
    done
    log "removed the $MODE set under $BASE" ;;
  *) die "unknown command $CMD" ;;
esac
