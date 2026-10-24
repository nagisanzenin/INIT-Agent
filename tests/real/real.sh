#!/bin/bash
# S-REAL: the v0.1 flows on the REAL model (and the same harness at $0 on the mock), build/status/S-REAL.md.
#
# One test home per mode, installed once through the real install path (scripts/slice1/s1.sh's steps) and kept,
# so the Root's money meter (total_spent) carries across every check of a batch: the charter's total ceiling is
# the batch's hard cap. Each `run` starts the Root, runs one check (tests/real/checks.py), collects redacted
# artifacts into build/real-artifacts/<mode>/<check>/, stops the Root and (real) removes the key.
#
# Usage: tests/real/real.sh <mock|real> install
#        tests/real/real.sh <mock|real> run <check> [args for checks.py]
#        tests/real/real.sh <mock|real> clean
# Checks: see tests/real/checks.py (vds, a1, e2e1, e2e2, e2e3, a2, a3, m3).
#
# Real mode guards (the owner's authorization for this batch, RELEASE_PLAN §3.17):
#   - S1_I_MEAN_IT=1 is required; the model is deepseek-flash only;
#   - the charter ceilings are $1.00/day and $1.00 total (Root-metered): the hard cap; a second home in the same
#     batch gets REAL_CHARTER_USD = $1.00 minus the first home's spend, and REAL_STOP_MUSD lowered by the same
#     amount, so the cap holds across homes (REAL_CACHE_READ=0 keeps the input-price meter on a newer Root);
#   - no check starts once the Root-metered total reaches REAL_STOP_MUSD (default 500000 = $0.50);
#   - inside a check the driver interrupts and /stops when the Root total passes the check's own cap
#     (S1_SPEND_MAX_MUSD above the total at its start, default 100000) or REAL_HARD_MUSD (default 950000);
#   - the key is copied from ~/.config/init-agent/deepseek.key with `tr` (never echoed, never on a command line)
#     into the test Root's key store, mode 0600, and removed in the EXIT trap and before any collection;
#   - collect.py redacts and rescans every artifact (0 key bytes, 0 `sk-` strings).
#   S1_DRY=1 runs a check on the real home with NO key (the egress refuses every model request, $0): the
#   harness plumbing is tested there before any spend.
# Safety: signals go only to PIDs this script spawned. Homes live under target/real-homes/ and are removed by
# exact path. ~/.init and ~/.init-root are never touched.
set -euo pipefail
umask 077

REPO="$(cd "$(dirname "$0")/../.." && pwd -P)"
MODE="${1:-}"
CMD="${2:-}"
case "$MODE" in mock) X=m ;; real) X=r ;; *) echo "usage: $0 <mock|real> <install|run <check>|clean>" >&2; exit 64 ;; esac
shift 2 || true
PROD=0; [ "$MODE" = real ] && PROD=1
DRY=0; [ $PROD = 1 ] && [ "${S1_DRY:-}" = 1 ] && DRY=1   # real home, no key: every model request is refused ($0)
STARTED=0

H="$REPO/target/real-homes"
D="$H/$X"
RH="$D/rh"
IH="$D/ih"
REL="$D/rel"
REAL_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
TC="$(cd "$REAL_HOME/.rustup/toolchains/stable-aarch64-apple-darwin" && pwd -P)"
DEVDIR=/Library/Developer/CommandLineTools
KEYFILE="$REAL_HOME/.config/init-agent/deepseek.key"
BIN="${REAL_BIN_DIR:-$REPO/target/s-real/bin}"   # frozen copies: init-root, init-kernel, initctl, init-mockllm
SEED_TAG=seed-v0.3.0
SEED_REV="${S1_SEED_REV:-HEAD}"
MIN_FREE_MB="${S1_MIN_FREE_MB:-5300}"
STOP_MUSD="${REAL_STOP_MUSD:-500000}"
HARD_MUSD="${REAL_HARD_MUSD:-950000}"
CHECK_MUSD="${S1_SPEND_MAX_MUSD:-100000}"

log() { printf '[real %s %s] %s\n' "$(date +%H:%M:%S)" "$MODE" "$*" >&2; }
die() { log "FAIL: $*"; exit 1; }
ms() { /usr/bin/python3 -I -c 'import time;print(int(time.time()*1000))'; }
free_mb() { df -m / | awk 'NR==2{print $4}'; }
[ ${#RH} -le 64 ] && [ ${#IH} -le 64 ] || die "homes exceed 64 bytes ($RH)"

PIDS=()
cleanup() {
  set +eu
  if [ -S "$RH/run/owner.sock" ] && [ "${STARTED:-0}" = 1 ]; then
    env -i HOME="$REAL_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$RH/bin/init-root" stop >>"$D/stop.out" 2>&1
    for _ in $(seq 1 80); do
      n=$(env -i HOME="$REAL_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$RH/bin/init-root" status --json 2>/dev/null \
        | /usr/bin/python3 -I -c 'import json,sys
try: print(len(json.load(sys.stdin).get("instances") or []))
except Exception: print(0)')
      [ "$n" = 0 ] && break
      sleep 0.25
    done
  fi
  for p in "${PIDS[@]}"; do kill -TERM "$p" 2>/dev/null; done # only our own children, by pid
  for p in "${PIDS[@]}"; do
    for _ in $(seq 1 40); do kill -0 "$p" 2>/dev/null || break; sleep 0.25; done
    kill -KILL "$p" 2>/dev/null
  done
  PIDS=()
  wait 2>/dev/null
  rm -f -- "$RH/keys/providers/deepseek" 2>/dev/null # the real key never outlives a run
}
trap cleanup EXIT

kernel_config() { # $1 = soft day, $2 = task, $3 = request_max, $4 = gate, $5 = quick gate
  cat >"$IH/kernel/config.toml" <<EOF
schema_version = 2
[model]
provider = "deepseek"
id = "deepseek-flash"

[proxy]
test_mode = false

[budget]
day_usd = "$1"
task_usd = "$2"
request_max_usd = "$3"
gate_usd = "$4"
quick_gate_usd = "$5"
probe_usd = "$5"
${REAL_KCFG_EXTRA_BUDGET:-}
[sandbox]
toolchain = "$TC"
developer_dir = "$DEVDIR"

[supervise]
build_timeout_cold_s = 3600
build_timeout_warm_s = 1800
${REAL_KCFG_EXTRA:-}
[providers.deepseek.schedule]
tz      = "+00:00"
default = "offpeak"
windows = [
  { days = "mon-fri", from = "01:00", to = "04:00", use = "peak" },
  { days = "mon-fri", from = "06:00", to = "10:00", use = "peak" },
]
default_dates_tz = "+08:00"
default_dates    = ["2026-10-01"]

[providers.deepseek.models."deepseek-flash"]
max_output            = 393216
reserve_output_tokens = 65536
reference_window      = "peak"
prices.peak    = { input = "0.30", cache_read = "0.006", output = "1.20" }
prices.offpeak = { input = "0.15", cache_read = "0.003", output = "0.60" }
EOF
  chmod 644 "$IH/kernel/config.toml"
}

# ================================================================ install (once per mode; kept)
do_install() {
  local FREE0; FREE0="$(free_mb)"
  [ "$FREE0" -ge "${S1_START_FREE_MB:-6500}" ] || die "only $FREE0 MB free; a cold install needs ${S1_START_FREE_MB:-6500} MB"
  for b in init-root init-kernel initctl; do [ -x "$BIN/$b" ] || die "missing $BIN/$b"; done
  [ $PROD = 1 ] || [ -x "$BIN/init-mockllm" ] || die "missing $BIN/init-mockllm"
  if [ -e "$D" ]; then rm -rf -- "$D"; fi
  mkdir -p "$REL" "$RH" "$D/mock"
  cp -c "$BIN/init-root" "$D/init-root"
  cp -c "$BIN/init-kernel" "$REL/init-kernel"
  cp -c "$BIN/initctl" "$REL/initctl"            # the zone's initctl (copy_release skips a missing one silently)
  cp -cR "$REPO/wit" "$REL/wit"                    # kernel/wit: the ext gate compares body/wit with it
  # G0 runs the body's tests with INIT_MOCKLLM_BIN=$INIT_HOME/kernel/bin/init-mockllm (gate.md §3.3)
  [ -x "$BIN/init-mockllm" ] && cp -c "$BIN/init-mockllm" "$REL/init-mockllm"
  [ $PROD = 1 ] || cp -c "$BIN/init-mockllm" "$D/init-mockllm"
  # the release's kernel/config.toml: the Root's [sandbox] template plus longer build timeouts (a cold seed build
  # under the build lanes' load took more than the default 900 s)
  cat >"$REL/config.toml" <<EOF
schema_version = 2

[sandbox]
toolchain = "$TC"
developer_dir = "$DEVDIR"

[supervise]
build_timeout_cold_s = 3600
build_timeout_warm_s = 1800
EOF
  log "staged: root $(shasum -a 256 "$D/init-root" | cut -c1-12), kernel $(shasum -a 256 "$REL/init-kernel" | cut -c1-12), initctl $(shasum -a 256 "$REL/initctl" | cut -c1-12); free $FREE0 MB"
  local T0; T0=$(ms)
  /usr/bin/python3 -I "$REPO/scripts/slice1/seedpack.py" --repo "$REPO" --dst "$D/seedsrc" --kind repo \
    --rev "$SEED_REV" >"$D/seedsrc.json" || die "seed packaging"
  local G=(env -i PATH=/usr/bin:/bin HOME="$D" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null TZ=UTC
     GIT_AUTHOR_DATE=2026-10-20T00:00:00Z GIT_COMMITTER_DATE=2026-10-20T00:00:00Z
     /usr/bin/git -C "$D/seedsrc" -c core.hooksPath=/dev/null -c user.name=init-maintainer -c user.email=maintainer@init.invalid
     -c commit.gpgsign=false -c tag.gpgsign=false -c gc.auto=0 -c maintenance.auto=false)
  "${G[@]}" init -q -b seed
  "${G[@]}" add -A
  "${G[@]}" commit -q -m "seed $SEED_TAG (S-REAL seed; source $SEED_REV)"
  "${G[@]}" tag "$SEED_TAG"
  local SEED_SHA; SEED_SHA="$("${G[@]}" rev-parse HEAD)"
  "${G[@]}" bundle create -q "$D/$SEED_TAG.bundle" refs/heads/seed "refs/tags/$SEED_TAG"
  rm -rf -- "$D/seedsrc"
  echo "$SEED_SHA" >"$D/seed.sha"
  log "seed ${SEED_SHA:0:12} packed in $(( $(ms) - T0 )) ms"
  local PROVIDERS TEST_MODE
  if [ $PROD = 0 ]; then
    PORT="$(/usr/bin/python3 -I -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])')"
    echo "$PORT" >"$D/mock.port.fixed"
    TEST_MODE=true
    PROVIDERS="[actions.providers]
destinations = [\"127.0.0.1:$PORT\"]
paths = [\"chat/completions\", \"models\", \"user/balance\"]
[actions.providers.providers.deepseek]
host = \"127.0.0.1:$PORT\""
  else
    TEST_MODE=false
    PROVIDERS="[actions.providers]
destinations = [\"api.deepseek.com:443\"]
paths = [\"chat/completions\", \"models\", \"user/balance\"]"
  fi
  # cache hits bill at the cache-read price (build-deviations §3.13, R-DEC cdb49e8), as in the Root's charter
# template; a Root built before that commit refuses the key, so it is written only when the binary knows it
CACHE_READ_LINE=""
if [ "${REAL_CACHE_READ:-1}" = 1 ] && /usr/bin/grep -q cache_read_musd_per_mtok "$BIN/init-root"; then
  CACHE_READ_LINE="cache_read_musd_per_mtok = 6000"
fi
cat >"$D/charter.toml" <<EOF
schema_version = 1
[purpose]
text = "S-REAL test home: prove the v0.1 flows on the real model within a \$1.00 hard cap."
[autonomy]
max_level = 4
[money]
day_usd = "${REAL_CHARTER_USD:-1.00}"
total_usd = "${REAL_CHARTER_USD:-1.00}"
day_starts_at_utc_offset = "+00:00"
[quotas]
disk_min_free_gib = ${S1_DISK_FLOOR_GIB:-5}
[sealed]
reserve_usd = "0.00"
audit_usd = "0.00"
[mirror]
url = ""
[root]
test_mode = $TEST_MODE
$PROVIDERS
[actions.providers.models."deepseek-flash"]
min_level = 0
input_musd_per_mtok = 300000
${CACHE_READ_LINE}
output_musd_per_mtok = 1200000
reserve_output_tokens = 65536
EOF
  local IENV=(env -i HOME="$REAL_HOME" INIT_ROOT_HOME="$RH" INIT_HOME="$IH" PATH=/usr/bin:/bin)
  T0=$(ms)
  set +e
  "${IENV[@]}" "$D/init-root" install --release "$REL" --charter "$D/charter.toml" --toolchain "$TC" \
    --seed-source "$D/$SEED_TAG.bundle" --no-service >"$D/install.out" 2>&1
  local IRC=$?
  set -e
  log "install exit $IRC in $(( $(ms) - T0 )) ms; free $(free_mb) MB"
  [ $IRC = 0 ] || { tail -20 "$D/install.out" >&2; tail -40 "$IH/state/kernel/logs/install-build.log" >&2 2>/dev/null; die "install"; }
  if [ -e "$IH/cache/build/slot" ]; then rm -rf -- "$IH/cache/build/slot"; fi
  [ -x "$IH/kernel/bin/initctl" ] || die "no kernel/bin/initctl after install"
  log "installed $D (INIT_HOME $(du -sm "$IH" | cut -f1) MB); free $(free_mb) MB"
}

# ================================================================ run one check
do_run() {
  local CHECK="${1:-}"; shift || true
  [ -n "$CHECK" ] || die "run needs a check name"
  [ -x "$RH/bin/init-root" ] || die "not installed: $0 $MODE install"
  if [ $PROD = 1 ] && [ $DRY = 0 ]; then
    [ -f "$KEYFILE" ] || die "no key file at ~/.config/init-agent/deepseek.key"
    [ "${S1_I_MEAN_IT:-}" = 1 ] || die "real mode spends money: set S1_I_MEAN_IT=1"
  fi
  [ "$(free_mb)" -ge "$MIN_FREE_MB" ] || die "only $(free_mb) MB free (floor $MIN_FREE_MB)"
  local SUB=$MODE; [ $DRY = 1 ] && SUB=realdry
  local OUT="$REPO/build/real-artifacts/$SUB/${REAL_TAG:-$CHECK}"
  local R="$D/runs/${REAL_TAG:-$CHECK}"
  if [ -e "$R" ]; then rm -rf -- "$R"; fi
  mkdir -p "$R/mock"
  # per-check kernel budgets (soft, under the Root's hard ceilings)
  kernel_config "${REAL_K_DAY:-0.80}" "${REAL_K_TASK:-0.20}" "${REAL_K_REQ:-0.15}" "${REAL_K_GATE:-0.30}" "${REAL_K_QUICK:-0.10}"
  # check-specific preparation outside the zone (fixtures into work/, worktrees) before the Root starts
  /usr/bin/python3 -I "$REPO/tests/real/checks.py" prepare "$CHECK" --ih "$IH" --rh "$RH" --run "$R" --repo "$REPO" --mode "$MODE" "$@" \
    || die "prepare $CHECK"
  if [ $PROD = 0 ]; then
    local PORT; PORT="$(cat "$D/mock.port.fixed")"
    local SCRIPT="$R/check.mock.jsonl"
    [ -f "$SCRIPT" ] || die "prepare wrote no mock script for $CHECK"
    "$D/init-mockllm" --mode upstream --listen "127.0.0.1:$PORT" --strict deepseek --port-file "$R/mock.port" \
      --script "$SCRIPT" --record-dir "$R/mock" >"$R/mock.out" 2>&1 &
    PIDS+=($!)
    for _ in $(seq 1 100); do [ -s "$R/mock.port" ] && break; sleep 0.1; done
    [ -s "$R/mock.port" ] || { cat "$R/mock.out" >&2; die "mock did not start"; }
    (umask 077; printf 'test-slice1-mock-key' >"$RH/keys/providers/deepseek")
  elif [ $DRY = 0 ]; then
    (umask 077; tr -d '\r\n' <"$KEYFILE" >"$RH/keys/providers/deepseek")
  fi
  [ -f "$RH/keys/providers/deepseek" ] && chmod 600 "$RH/keys/providers/deepseek"
  local RENV=(env -i HOME="$REAL_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin)
  "${RENV[@]}" "$RH/bin/init-root" keys check >"$R/keys-check.out" 2>&1 || true
  local WANT=present; [ $DRY = 1 ] && WANT=missing
  grep -q "^providers/deepseek: $WANT" "$R/keys-check.out" || { cat "$R/keys-check.out" >&2; die "key not $WANT"; }
  local T_START; T_START=$(ms)
  "${RENV[@]}" "$RH/bin/init-root" start --foreground >"$R/root.out" 2>&1 </dev/null &
  PIDS+=($!)
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
  "${RENV[@]}" "$RH/bin/init-root" status --json >"$R/status-ready.json" 2>/dev/null || true
  [ $ready = 1 ] || { tail -20 "$RH"/logs/kernel-*.stderr >&2 2>/dev/null; die "kernel+body not ready in 120 s"; }
  local READY_MS=$(( $(ms) - T_START ))
  log "$CHECK: ready after $READY_MS ms"
  # the batch line: Root-metered total spend so far
  curl -s --max-time 5 --unix-socket "$RH/run/egress.sock" http://root/_root/ceilings >"$R/ceilings-before.json" || true
  local SPENT0; SPENT0="$(/usr/bin/python3 -I -c 'import json,sys
try: print(int(json.load(open(sys.argv[1]))["money"]["total_spent_musd"]))
except Exception: print(-1)' "$R/ceilings-before.json")"
  [ "$SPENT0" -ge 0 ] || die "cannot read the Root's total spend"
  if [ $PROD = 1 ] && [ "$SPENT0" -ge "$STOP_MUSD" ]; then
    log "Root-metered total $SPENT0 µUSD >= stop line $STOP_MUSD: no new check"; exit 4
  fi
  local CAP=$(( SPENT0 + CHECK_MUSD )); [ "$CAP" -gt "$HARD_MUSD" ] && CAP=$HARD_MUSD
  log "$CHECK: Root total before $SPENT0 µUSD; this check stops above $CAP"
  balance() {
    curl -s --max-time 20 --unix-socket "$IH/run/proxy.sock" -H 'x-init-session: srealbalance' -H 'x-init-purpose: turn' \
      -o "$R/balance-$1.json" -w '%{http_code}\n' http://proxy/deepseek/user/balance >"$R/balance-$1.code" 2>/dev/null || true
  }
  curl -s --max-time 5 --unix-socket "$IH/run/proxy.sock" http://proxy/_init/stats >"$R/proxy-stats-before.json" || true
  [ $PROD = 1 ] && [ $DRY = 0 ] && balance before
  set +e
  /usr/bin/python3 -I "$REPO/tests/real/checks.py" run "$CHECK" --ih "$IH" --rh "$RH" --run "$R" --repo "$REPO" --mode "$MODE" \
    --home "$REAL_HOME" --cap-musd "$CAP" --ready-ms "$READY_MS" "$@" >"$R/result.json"
  local CRC=$?
  set -e
  if [ $PROD = 1 ] && [ $DRY = 0 ]; then sleep 3; balance after; fi
  curl -s --max-time 5 --unix-socket "$RH/run/egress.sock" http://root/_root/ceilings >"$R/ceilings-after.json" || true
  curl -s --max-time 5 --unix-socket "$IH/run/proxy.sock" http://proxy/_init/stats >"$R/proxy-stats.json" || true
  "${RENV[@]}" "$RH/bin/init-root" audit verify --full >"$R/audit-verify.out" 2>&1; echo "exit $?" >>"$R/audit-verify.out"
  "${RENV[@]}" "$RH/bin/init-root" status --json >"$R/status-end.json" 2>/dev/null || true
  # stop the Root, then remove the key, before anything is copied
  cleanup
  STARTED=0
  /usr/bin/python3 -I "$REPO/tests/real/checks.py" verdict "$CHECK" --ih "$IH" --rh "$RH" --run "$R" --repo "$REPO" --mode "$MODE" \
    --spent0 "$SPENT0" --crc "$CRC" >"$R/verdict.json" || true
  if [ -e "$OUT" ]; then rm -rf -- "$OUT"; fi
  mkdir -p "$OUT"
  local KF=-; [ $PROD = 1 ] && KF="$KEYFILE"
  /usr/bin/python3 -I "$REPO/scripts/slice1/collect.py" "$OUT" "$KF" "$R=run" \
    "$RH/lineage/lineage.jsonl=root/lineage" "$RH/logs=root/logs" "$RH/egress/usage=root/egress-usage" \
    "$IH/kernel/config.toml=init/kernel" "$IH/state/sessions=init/sessions" "$IH/state/kernel/logs=init/kernel-logs" \
    "$IH/state/kernel/gate=init/gate" "$IH/state/kernel/lkg.json=init/state" "$IH/work=init/work" >"$R/collect.json" || true
  cp "$R/collect.json" "$OUT/run/collect.json" 2>/dev/null || true
  log "$CHECK: collect $(cat "$R/collect.json")"
  log "$CHECK: verdict $(cat "$R/verdict.json")"
  /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1])).get("pass") else 1)' "$R/verdict.json"
}

case "$CMD" in
  install) do_install ;;
  run) do_run "$@" ;;
  clean) if [ -e "$D" ]; then rm -rf -- "$D"; log "removed $D"; fi ;;
  *) die "unknown command $CMD" ;;
esac
