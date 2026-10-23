#!/bin/bash
# Slice-1 end-to-end harness through the REAL install path (build/status/S1.md, WP21b.md, FIX4.md):
#   seed source → git bundle (branch `seed`, tag seed-v0.1.0)
#   → init-root install --seed-source <bundle>: the Root runs `init-kernel install --from-bundle` in the zone;
#     the kernel imports the seed into lineage/seed.git and lineage/body.git, clones body/, builds the seed
#     under the `build` profile, store.puts the body and reports promotion{lkg:true}; the Root forms the
#     genesis tuple (real seed sha, the built body) and appends tuple, lkg, audited
#   → init-root start → init-kernel supervise (`kernel` profile) → init-body (`body` profile, built from the
#     seed) → kernel proxy → Root egress → model → `write` tool → reply back to attach.
# Nothing writes state/kernel/lkg.json, bin/init-body or body/ by hand.
#
# Usage: scripts/slice1/s1.sh <fakebody|mock|realdry|real> [--keep]
#   fakebody  the fake-body seed (seedpack.py --kind fakebody: init-body = the kernel test kit's scripted fake
#             body); the model path is exercised with one request to the proxy socket; $0
#   mock      the repo seed + init-mockllm (loopback upstream, root.test_mode = true, a `test-` key); $0
#   realdry   the real run's production config (test_mode = false, api.deepseek.com) with NO key: the egress
#             refuses with class.disabled before any connection; $0, no network
#   real      the repo seed + api.deepseek.com, production egress (test_mode = false); key from
#             ~/.config/init-agent/deepseek.key, never printed. Needs S1_I_MEAN_IT=1. ONE task, hard ceiling
#             $1.00, kernel soft cap $0.20 (day, task, request), harness stop at S1_SPEND_MAX_MUSD of Root spend.
#
# The repo seed: S1_SEED_REV=<rev> (default HEAD, the committed seed/) or `worktree` (tracked plus untracked, not
# ignored files). See seedpack.py for the vendor cut and the stubs. The fake-body seed takes kernel/crates at the
# same rev.
# Binaries are frozen copies (other lanes rebuild the shared target dirs): S1_ROOT_BIN, S1_KERNEL_BIN, S1_MOCK_BIN.
# The body is never staged: the install-mode kernel builds it. Debug builds only (disk).
# Disk: a cold build of the repo seed (wasmtime) is the big transient cost (about 1.7 GB at the lowest point:
# the build slot plus the new home). The case needs S1_START_FREE_MB free to begin (default 6500; 5800 for the
# small fake-body seed); a sampler logs free space every 2 s and the verdict fails if it fell below
# S1_MIN_FREE_MB (default 5300). Inside the install the Root itself kills the kernel and the build (only
# instances it holds) below the charter's quotas.disk_min_free_gib (S1_DISK_FLOOR_GIB, default 5 = 5.37 GB).
# The build slot is removed after the install and the run dir at the end, both by exact path.
# Safety: every process signalled here is one this script spawned, by its recorded pid. install and start run
# with the owner's real HOME (the class rules compare HOME_REAL with the kernel's passwd home); install has
# --no-service, so nothing is written under HOME.
set -euo pipefail
umask 077

REPO="$(cd "$(dirname "$0")/../.." && pwd -P)"
MODE="${1:-}"
KEEP="${2:-}"
case "$MODE" in
  fakebody) X=f ;;
  mock) X=m ;;
  realdry) X=d ;;
  real) X=r ;;
  *) echo "usage: $0 <fakebody|mock|realdry|real> [--keep]" >&2; exit 64 ;;
esac
PROD=0; case "$MODE" in real|realdry) PROD=1 ;; esac
SPEND_MAX_MUSD="${S1_SPEND_MAX_MUSD:-200000}"

S1="$REPO/target/slice1-homes"
D="$S1/$X"
RH="$D/rh"
IH="$D/ih"
REL="$D/rel"
OUT="$REPO/build/slice1-artifacts/${S1_TAG:-$MODE}"
REAL_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
TC="$(cd "$REAL_HOME/.rustup/toolchains/stable-aarch64-apple-darwin" && pwd -P)"
DEVDIR=/Library/Developer/CommandLineTools
KEYFILE="$REAL_HOME/.config/init-agent/deepseek.key"
ROOT_BIN="${S1_ROOT_BIN:-$REPO/root/target/debug/init-root}"
KERNEL_BIN="${S1_KERNEL_BIN:-$REPO/kernel/target/k1/debug/init-kernel}"
MOCK_BIN="${S1_MOCK_BIN:-$REPO/kernel/target/k1/debug/init-mockllm}"
TASK="${S1_TASK:-Create a file hello.txt containing the word INIT, then reply done.}"
SEED_TAG=seed-v0.1.0
SEED_REV="${S1_SEED_REV:-HEAD}"
SEED_KIND=repo; [ "$MODE" = fakebody ] && SEED_KIND=fakebody
MIN_FREE_MB="${S1_MIN_FREE_MB:-5300}"
START_DEFAULT=6500; [ "$SEED_KIND" = fakebody ] && START_DEFAULT=5800
START_FREE_MB="${S1_START_FREE_MB:-$START_DEFAULT}"

log() { printf '[s1 %s %s] %s\n' "$(date +%H:%M:%S)" "$MODE" "$*"; }
die() { log "FAIL: $*"; exit 1; }
ms() { /usr/bin/python3 -I -c 'import time;print(int(time.time()*1000))'; }
free_mb() { df -m / | awk 'NR==2{print $4}'; }

FREE0="$(free_mb)"
[ "$FREE0" -ge "$START_FREE_MB" ] || die "only $FREE0 MB free; a $SEED_KIND-seed install needs $START_FREE_MB MB to start (floor $MIN_FREE_MB MB)"
[ ${#RH} -le 64 ] && [ ${#IH} -le 64 ] || die "homes exceed 64 bytes ($RH)"
for b in "$ROOT_BIN" "$KERNEL_BIN"; do [ -x "$b" ] || die "missing $b"; done
[ $PROD = 1 ] || [ -x "$MOCK_BIN" ] || die "missing $MOCK_BIN"
if [ "$MODE" = real ]; then
  [ -f "$KEYFILE" ] || die "no key file at ~/.config/init-agent/deepseek.key"
  [ -z "${S1_I_MEAN_IT:-}" ] && die "real mode spends money: set S1_I_MEAN_IT=1"
fi

PIDS=()
WATCH=
cleanup() {
  set +eu
  if [ -S "$RH/run/owner.sock" ]; then
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
  for p in "${PIDS[@]}" $WATCH; do # only our own children, by pid
    kill -TERM "$p" 2>/dev/null
  done
  for p in "${PIDS[@]}" $WATCH; do
    for _ in $(seq 1 40); do kill -0 "$p" 2>/dev/null || break; sleep 0.25; done
    kill -KILL "$p" 2>/dev/null
  done
  WATCH=
  wait 2>/dev/null
  if [ $PROD = 1 ]; then rm -f -- "$RH/keys/providers/deepseek"; fi # the real key never outlives the run
}
trap cleanup EXIT

# ---------------------------------------------------------------- fresh run dir (exact path)
if [ -e "$D" ]; then rm -rf -- "$D"; fi
mkdir -p "$REL" "$RH" "$D/mock"
# free space every 2 s for the whole case (the sampler is this script's child)
( while :; do printf '%s %s\n' "$(date +%H:%M:%S)" "$(free_mb)"; sleep 2; done ) >"$D/free.log" 2>/dev/null &
WATCH=$!
cp -c "$ROOT_BIN" "$D/init-root"       # APFS clones: frozen, no second copy of the debug binaries
cp -c "$KERNEL_BIN" "$REL/init-kernel"
[ $PROD = 1 ] || cp -c "$MOCK_BIN" "$D/init-mockllm"
log "staged binaries: root $(shasum -a 256 "$D/init-root" | cut -c1-12), kernel $(shasum -a 256 "$REL/init-kernel" | cut -c1-12); free $FREE0 MB"

# ---------------------------------------------------------------- the seed source → a git bundle
# The maintainer's packaging step (root/root.md §4.3 step 7 takes a repo with branch `seed` and tags seed-v*,
# or such a bundle). The temp repo and the temp CARGO_HOME are removed once the bundle exists.
T0=$(ms)
/usr/bin/python3 -I "$REPO/scripts/slice1/seedpack.py" --repo "$REPO" --dst "$D/seedsrc" --kind "$SEED_KIND" \
  --rev "$SEED_REV" --cargo "$TC/bin/cargo" --cargo-home "$D/cargo-home" >"$D/seedsrc.json" || die "seed packaging"
G=(env -i PATH=/usr/bin:/bin HOME="$D/cargo-home" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null TZ=UTC
   GIT_AUTHOR_DATE=2026-10-20T00:00:00Z GIT_COMMITTER_DATE=2026-10-20T00:00:00Z
   /usr/bin/git -C "$D/seedsrc" -c core.hooksPath=/dev/null -c user.name=init-maintainer -c user.email=maintainer@init.invalid
   -c commit.gpgsign=false -c tag.gpgsign=false -c gc.auto=0 -c maintenance.auto=false) # no detached gc racing the rm below
"${G[@]}" init -q -b seed
"${G[@]}" add -A
"${G[@]}" commit -q -m "seed $SEED_TAG (Slice-1 $SEED_KIND seed; source $SEED_REV)"
"${G[@]}" tag "$SEED_TAG"
SEED_SHA="$("${G[@]}" rev-parse HEAD)"
"${G[@]}" bundle create -q "$D/$SEED_TAG.bundle" refs/heads/seed "refs/tags/$SEED_TAG"
SEED_MB=$(du -sm "$D/seedsrc" 2>/dev/null | cut -f1 || true)
rm -rf -- "$D/seedsrc"
if [ -e "$D/cargo-home" ]; then rm -rf -- "$D/cargo-home"; fi
log "seed $SEED_TAG = ${SEED_SHA:0:12}: $(cat "$D/seedsrc.json"); tree+git ${SEED_MB} MB, bundle $(du -sm "$D/$SEED_TAG.bundle" | cut -f1) MB, $(( $(ms) - T0 )) ms"

# ---------------------------------------------------------------- the $0 mock upstream
PORT=0
if [ "$MODE" = fakebody ]; then
  # the fake body never calls a model; the harness sends one request through the proxy socket
  cat >"$D/s1.mock.jsonl" <<'EOF'
{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"slice1-proxy-probe","chunk":16}
{"v":1,"kind":"turn","reply":{"text":"pong from the mock","stop":"end_turn","usage":{"input":10,"cache_read":0,"output":4}}}
EOF
fi
if [ "$MODE" = mock ]; then
  # turn 1: the model calls `write` (relative path: the body's cwd is INIT_HOME/work); turn 2: "done".
  # Turn 1 expects only a user message last: the body may append its own notes (e.g. `[init:reload] ext
  # generation 1 · not built: count-lines`) after the owner's line, as REL's release-e2e.sh found.
  cat >"$D/s1.mock.jsonl" <<'EOF'
{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"slice1-write-hello","chunk":16}
{"v":1,"kind":"turn","expect":{"last_role":"user"},"reply":{"think":"Write the file.","calls":[{"name":"write","input":{"path":"hello.txt","content":"INIT\n"}}],"stop":"tool_use","usage":{"input":12000,"cache_read":0,"output":40}}}
{"v":1,"kind":"turn","expect":{"prefix":"same"},"reply":{"text":"done","stop":"end_turn","usage":{"input":12100,"cache_read":11904,"output":2}}}
EOF
fi
if [ -n "${S1_MOCK_SCRIPT:-}" ]; then cp "$S1_MOCK_SCRIPT" "$D/s1.mock.jsonl"; fi # tests/slice1 cases
if [ $PROD = 0 ]; then
  "$D/init-mockllm" --mode upstream --listen 127.0.0.1:0 --strict deepseek --port-file "$D/mock.port" \
    --script "$D/s1.mock.jsonl" --record-dir "$D/mock" >"$D/mock.out" 2>&1 &
  PIDS+=($!)
  for _ in $(seq 1 100); do [ -s "$D/mock.port" ] && break; sleep 0.1; done
  PORT="$(tr -d '\n' <"$D/mock.port")"
  log "mock upstream on 127.0.0.1:$PORT (pid ${PIDS[0]})"
fi

# ---------------------------------------------------------------- charter (mirror off, L4 cap, $1/$1, disk floor)
if [ $PROD = 0 ]; then
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
if /usr/bin/grep -q cache_read_musd_per_mtok "$ROOT_BIN"; then CACHE_READ_LINE="cache_read_musd_per_mtok = 6000"; fi
cat >"$D/charter.toml" <<EOF
schema_version = 1
[purpose]
text = "Slice-1 integration home: prove the owner-to-model path end to end, from a real install."
[autonomy]
max_level = 4
[money]
day_usd = "1.00"
total_usd = "1.00"
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

# ---------------------------------------------------------------- install: the real path (owner's HOME, no service file)
IENV=(env -i HOME="$REAL_HOME" INIT_ROOT_HOME="$RH" INIT_HOME="$IH" PATH=/usr/bin:/bin)
T0=$(ms)
set +e
"${IENV[@]}" "$D/init-root" install --release "$REL" --charter "$D/charter.toml" --toolchain "$TC" \
  --seed-source "$D/$SEED_TAG.bundle" --no-service >"$D/install.out" 2>&1
IRC=$?
set -e
INSTALL_MS=$(( $(ms) - T0 ))
INSTALL_MIN_FREE=$(awk 'NR==1||$2<m{m=$2} END{print m}' "$D/free.log")
SLOT_MB=$(du -sm "$IH/cache/build/slot" 2>/dev/null | cut -f1)
IH_MB=$(du -sm "$IH" 2>/dev/null | cut -f1)
log "install exit $IRC in $INSTALL_MS ms; lowest free so far ${INSTALL_MIN_FREE:-?} MB (start $FREE0); build slot ${SLOT_MB:-0} MB; INIT_HOME ${IH_MB:-?} MB"
[ $IRC = 0 ] || { cat "$D/install.out"; tail -40 "$IH/state/kernel/logs/install-build.log" 2>/dev/null; tail -20 "$RH"/logs/kernel-*.stderr 2>/dev/null; die "install"; }
cat "$D/install.out"
# The build slot is a cache (kernel/supervise.md §3.6.2): reset by the next build. Free it now (exact path).
if [ -e "$IH/cache/build/slot" ]; then rm -rf -- "$IH/cache/build/slot"; fi
log "build slot removed; free $(free_mb) MB"

# what the install made, read back (no hand-written state)
/usr/bin/python3 -I - "$RH" "$IH" "$SEED_SHA" >"$D/install-check.json" <<'PY'
import hashlib, json, os, sys
rh, ih, seed = sys.argv[1:4]
recs = [json.loads(l) for l in open(os.path.join(rh, "lineage/lineage.jsonl"))]
t = next(r for r in recs if r["kind"] == "tuple")["data"]
lkg = json.load(open(os.path.join(ih, "state/kernel/lkg.json")))
body = hashlib.sha256(open(os.path.join(ih, "bin/init-body"), "rb").read()).hexdigest()
v = {"kinds": [r["kind"] for r in recs], "tuple": t["id"], "body_sha": t["tuple"]["body"]["sha"], "body_binary": t["tuple"]["body"]["binary"],
     "lkg_sha": lkg["sha"], "lkg_reason": lkg["reason"], "genesis_md": os.path.isfile(os.path.join(ih, "body/GENESIS.md")),
     "body_git": os.path.isdir(os.path.join(ih, "body/.git")), "body_lineage": os.path.isdir(os.path.join(ih, "lineage/body.git")),
     "seed_mirror": os.path.isdir(os.path.join(ih, "lineage/seed.git"))}
v["ok"] = (v["body_sha"] == seed == v["lkg_sha"] and v["lkg_reason"] == "seed" and v["body_binary"] == body == lkg["binary_sha256"]
           and v["genesis_md"] and v["body_git"] and v["body_lineage"] and v["seed_mirror"] and v["kinds"][-3:] == ["tuple", "lkg", "audited"])
print(json.dumps(v, sort_keys=True))
PY
log "install check: $(cat "$D/install-check.json")"
/usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))["ok"] else 1)' "$D/install-check.json" \
  || die "the install did not produce the genesis tuple, lkg.json and body/ that agree"

KBUDGET=0.50; [ $PROD = 1 ] && KBUDGET=0.20
# kernel/config.toml: the owner's edit of the Root's template ([sandbox] unchanged, the class rule compares it
# with install.json); proxy prices and soft budgets. A $1 request_max would make the D6 clamp ceiling − request_max $0.
cat >"$IH/kernel/config.toml" <<EOF
schema_version = 2
[model]
provider = "deepseek"
id = "deepseek-flash"

[proxy]
test_mode = false

[budget]
day_usd = "$KBUDGET"
task_usd = "$KBUDGET"
request_max_usd = "0.20"

[sandbox]
toolchain = "$TC"
developer_dir = "$DEVDIR"

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

# ---------------------------------------------------------------- the provider key (never printed)
KDEST="$RH/keys/providers/deepseek"
if [ "$MODE" = real ]; then
  (umask 077; tr -d '\r\n' <"$KEYFILE" >"$KDEST")
elif [ "$MODE" = realdry ]; then
  : # no key: class.disabled
else
  (umask 077; printf 'test-slice1-mock-key' >"$KDEST")
fi
[ -f "$KDEST" ] && chmod 600 "$KDEST"
"${IENV[@]}" "$D/init-root" keys check >"$D/keys-check.out" 2>&1 || true
WANT=present; [ "$MODE" = realdry ] && WANT=missing
grep -q "^providers/deepseek: $WANT" "$D/keys-check.out" || { cat "$D/keys-check.out"; die "key not $WANT"; }

# ---------------------------------------------------------------- start the Root (owner's real HOME)
RENV=(env -i HOME="$REAL_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin)
T_START=$(ms)
"${RENV[@]}" "$RH/bin/init-root" start --foreground >"$D/root.out" 2>&1 </dev/null &
PIDS+=($!)
log "init-root start (pid $!)"
for _ in $(seq 1 600); do [ -S "$RH/run/owner.sock" ] && break; sleep 0.05; done # owner.sock opens before the tuple verify
[ -S "$RH/run/owner.sock" ] || { cat "$D/root.out"; die "owner.sock never appeared"; }
SOCK_MS=$(( $(ms) - T_START ))
st() { "${RENV[@]}" "$RH/bin/init-root" status --json 2>/dev/null; }
ready=0
for _ in $(seq 1 240); do
  [ -f "$IH/state/kernel/logs/body-3.stderr" ] && break  # the body crashed three times: report now
  if st | /usr/bin/python3 -I -c 'import json,sys; v=json.load(sys.stdin); sys.exit(0 if v.get("slot")=="running" else 1)' 2>/dev/null \
     && [ -f "$IH/state/run/ready" ]; then ready=1; break; fi
  sleep 0.5
done
st >"$D/status-ready.json" || true
[ $ready = 1 ] || { log "status: $(cat "$D/status-ready.json")"; tail -20 "$RH"/logs/kernel-*.stderr 2>/dev/null; tail -c 2000 "$IH"/state/kernel/logs/*.stderr 2>/dev/null; die "kernel+body not ready in 120 s"; }
READY_MS=$(( $(ms) - T_START ))
log "owner.sock after $SOCK_MS ms; kernel running and body ready after $READY_MS ms"
st | /usr/bin/python3 -I "$REPO/scripts/slice1/zonecheck.py" >"$D/zone.json" \
  || { cat "$D/zone.json"; die "an instance is not sandboxed"; }
log "zone: $(cat "$D/zone.json")"

# the provider's own account balance (GET user/balance: unmetered, $0) before and after the task, so the
# real cost is also measured outside our meters; in realdry it must stop at the key check (class.disabled)
balance() {
  curl -s --max-time 20 --unix-socket "$IH/run/proxy.sock" -H 'x-init-session: s1balance' -H 'x-init-purpose: turn' \
    -o "$D/balance-$1.json" -w '%{http_code}\n' http://proxy/deepseek/user/balance >"$D/balance-$1.code" 2>/dev/null || true
}
[ $PROD = 1 ] && balance before

# ---------------------------------------------------------------- the owner task
if [ "$MODE" = fakebody ]; then
  LINE="hello from slice-1"; EXPECT='fake reply: hello from slice-1'; TMO=30; UNTIL=--until-text
else
  LINE="$TASK"; EXPECT='(?i)\bdone\b'; TMO=240; UNTIL=
fi
[ "$MODE" = realdry ] && TMO=90
set +e
/usr/bin/python3 -I "$REPO/scripts/slice1/drive.py" --bin "$RH/bin/init-root" --rh "$RH" --home "$REAL_HOME" \
  --line "$LINE" --expect "$EXPECT" --timeout "$TMO" --out "$D/attach.jsonl" $UNTIL \
  --spend-sock "$RH/run/egress.sock" --spend-max-musd "$SPEND_MAX_MUSD" >"$D/drive.json"
DRC=$?
set -e
log "drive: $(cat "$D/drive.json")"

# the model path for the fake body: one request through the proxy socket (body-side contract)
if [ "$MODE" = fakebody ]; then
  # forwarded by the Root egress to the loopback mock (test mode); expect 200 and the mock's text
  curl -s --max-time 20 --unix-socket "$IH/run/proxy.sock" -o "$D/proxy-probe.body" -w '%{http_code}\n' \
    -H 'content-type: application/json' -H 'x-init-session: s1probe' -H 'x-init-purpose: turn' \
    --data '{"model":"deepseek-flash","stream":true,"max_tokens":16,"messages":[{"role":"user","content":"hi"}]}' \
    http://proxy/deepseek/chat/completions >"$D/proxy-probe.code" 2>/dev/null || true
  log "proxy probe http $(cat "$D/proxy-probe.code" 2>/dev/null)"
fi

[ $PROD = 1 ] && sleep 5 && balance after

# ---------------------------------------------------------------- evidence
curl -s --max-time 5 --unix-socket "$RH/run/egress.sock" http://root/_root/ceilings >"$D/ceilings.json" || true
curl -s --max-time 5 --unix-socket "$IH/run/proxy.sock" http://proxy/_init/stats >"$D/proxy-stats.json" || true
"${RENV[@]}" "$RH/bin/init-root" egress stats >"$D/egress-stats.json" 2>&1 || true
"${RENV[@]}" "$RH/bin/init-root" ledger >"$D/ledger.json" 2>&1 || true
"${RENV[@]}" "$RH/bin/init-root" audit verify --full >"$D/audit-verify.out" 2>&1; echo "exit $?" >>"$D/audit-verify.out"
st >"$D/status-end.json" || true
cp "$IH/state/kernel/lkg.json" "$D/lkg.json" 2>/dev/null || true
printf '%s %s\n' "$(date +%H:%M:%S)" "$(free_mb)" >>"$D/free.log"

/usr/bin/python3 -I - "$D" "$IH" "$RH" "$MODE" "$DRC" "$SPEND_MAX_MUSD" "$SEED_SHA" "$SEED_KIND" "$SEED_REV" \
  "$INSTALL_MS" "$SOCK_MS" "$READY_MS" "$FREE0" "${INSTALL_MIN_FREE:-0}" "$MIN_FREE_MB" "${SLOT_MB:-0}" "${IH_MB:-0}" <<'PY' | tee "$D/verdict.json"
import json, os, sys, glob
(d, ih, rh, mode, drc, cap, seed, seed_kind, seed_rev, install_ms, sock_ms, ready_ms, free0, install_min_free,
 min_free, slot_mb, ih_mb) = sys.argv[1:18]
def j(p):
    try:
        with open(p) as f: return json.load(f)
    except Exception: return None
def jl(p):
    out = []
    for f in sorted(glob.glob(p)):
        for l in open(f):
            try: out.append(json.loads(l))
            except ValueError: pass
    return out
lin = jl(os.path.join(rh, "lineage/lineage.jsonl"))
kinds = [r.get("kind") for r in lin]
usage = jl(os.path.join(rh, "egress/usage/*.jsonl"))
ends = [u for u in usage if u.get("t") == "end"]
c = j(os.path.join(d, "ceilings.json")) or {}
ps = j(os.path.join(d, "proxy-stats.json")) or {}
st = j(os.path.join(d, "status-end.json")) or {}
hello = os.path.join(ih, "work/hello.txt")
sess = jl(os.path.join(ih, "state/sessions/*.jsonl"))
msgs = [e for e in sess if e.get("type") == "message" and e.get("role") == "assistant"]
boots = [e for e in sess if e.get("type") == "boot"]
us = [e.get("usage") or {} for e in msgs]
frees = [int(l.split()[1]) for l in open(os.path.join(d, "free.log")) if len(l.split()) == 2 and l.split()[1].isdigit()]
v = {
  "mode": mode,
  "path": "install",
  "seed_kind": seed_kind,
  "seed_rev": seed_rev,
  "seed_sha": seed,
  "install_ms": int(install_ms),
  "owner_sock_ms": int(sock_ms),
  "start_to_ready_ms": int(ready_ms),
  "disk": {"free_start_mb": int(free0), "free_min_install_mb": int(install_min_free),
           "free_min_mb": min(frees) if frees else None, "peak_use_mb": int(free0) - min(frees) if frees else None,
           "floor_mb": int(min_free), "build_slot_mb": int(slot_mb), "init_home_after_install_mb": int(ih_mb)},
  "install_check": j(os.path.join(d, "install-check.json")),
  "tuples": st.get("tuples"),
  "boot_sha": [b.get("sha") or (b.get("data") or {}).get("sha") for b in boots][:3],
  "drive_rc": int(drc),
  "reply_ok": int(drc) == 0,
  "hello_txt": open(hello).read() if os.path.isfile(hello) else None,
  "lineage_kinds": sorted(set(kinds)),
  "lineage_n": len(lin),
  "ceiling_hits": [r.get("data") for r in lin if r.get("kind") == "ceiling_hit"],
  "egress_requests": len(ends),
  "egress_usage": [{k: e.get(k) for k in ("status", "usage", "cost_musd", "finish", "flags")} for e in ends],
  "day_spent_musd": (c.get("money") or {}).get("day_spent_musd"),
  "total_spent_musd": (c.get("money") or {}).get("total_spent_musd"),
  "model_turns": len(msgs),
  "stop_reasons": [e.get("stop_reason") for e in msgs],
  "cache_hit_tokens": sum(u.get("cache_read_tokens", 0) for u in us),
  "cache_miss_tokens": sum(u.get("input_tokens", 0) for u in us),
  "output_tokens": sum(u.get("output_tokens", 0) for u in us),
  "reasoning_tokens": sum(u.get("reasoning_tokens", 0) for u in us),
  "per_turn_usage": us,
  "proxy_spend_today_musd": ((ps.get("spend") or {}).get("today_musd") if isinstance(ps, dict) else None),
  "proxy_requests_today": (ps.get("requests_today") if isinstance(ps, dict) else None),
  "proxy_cache": (ps.get("cache") if isinstance(ps, dict) else None),
  "refusals": [{k: x for k, x in u.items() if k != "v"} for u in usage if u.get("t") == "refuse"],
  "drive": j(os.path.join(d, "drive.json")),
  "spend_cap_musd": int(cap),
  "balance_before": j(os.path.join(d, "balance-before.json")),
  "balance_after": j(os.path.join(d, "balance-after.json")),
}
if mode == "mock":
    # the second request carries the tool result for the write call (strict mode checks the id pairing)
    rec = sorted(glob.glob(os.path.join(d, "mock/*-turn.json")))
    tr = []
    if len(rec) >= 2:
        b = json.load(open(rec[1])); b = b.get("body", b)
        tr = [m.get("content") for m in b.get("messages", []) if m.get("role") == "tool"]
    v["mock_requests"] = len(rec)
    v["tool_result_sent"] = tr[:1]
v["spend_within_cap"] = (v["day_spent_musd"] or 0) <= int(cap)
v["disk_ok"] = bool(frees) and min(frees) >= int(min_free)
ok_install = bool((v["install_check"] or {}).get("ok"))
if mode == "realdry":
    ok = len(v["refusals"]) >= 3 and all(r.get("code") == "class.disabled" for r in v["refusals"]) and not ends and not v["ceiling_hits"]
elif mode != "fakebody":
    ok = v["reply_ok"] and (v["hello_txt"] or "").strip() == "INIT" and v["egress_requests"] >= 2 and not v["ceiling_hits"] and v["spend_within_cap"]
else:
    probe = open(os.path.join(d, "proxy-probe.code")).read().strip() if os.path.isfile(os.path.join(d, "proxy-probe.code")) else ""
    v["proxy_probe_http"] = probe
    ok = v["reply_ok"] and probe == "200" and v["egress_requests"] == 1
v["pass"] = bool(ok and ok_install and v["disk_ok"])
print(json.dumps(v, sort_keys=True))
PY

# ---------------------------------------------------------------- artifacts (redacted), then stop
if [ -e "$OUT" ]; then rm -rf -- "$OUT"; fi
mkdir -p "$OUT"
KF=-; [ "$MODE" = real ] && KF="$KEYFILE"
[ $PROD = 1 ] && rm -f -- "$RH/keys/providers/deepseek" # before anything is copied
/usr/bin/python3 -I "$REPO/scripts/slice1/collect.py" "$OUT" "$KF" \
  "$D/verdict.json=run" "$D/drive.json=run" "$D/attach.jsonl=run" "$D/ceilings.json=run" "$D/proxy-stats.json=run" \
  "$D/egress-stats.json=run" "$D/ledger.json=run" "$D/audit-verify.out=run" "$D/status-ready.json=run" \
  "$D/status-end.json=run" "$D/root.out=run" "$D/install.out=run" "$D/install-check.json=run" "$D/seedsrc.json=run" \
  "$D/free.log=run" "$D/charter.toml=run" "$D/mock=mock" "$D/zone.json=run" "$D/keys-check.out=run" "$D/lkg.json=run" \
  "$D/balance-before.json=run" "$D/balance-after.json=run" "$D/balance-before.code=run" "$D/balance-after.code=run" \
  "$D/proxy-probe.code=run" "$D/proxy-probe.body=run" \
  "$RH/lineage/lineage.jsonl=root/lineage" "$RH/logs=root/logs" "$RH/egress/usage=root/egress-usage" \
  "$RH/state/root.json=root/state" "$RH/install.json=root" \
  "$IH/kernel/config.toml=init/kernel" "$IH/root=init/root" "$IH/state=init/state" "$IH/vault/lineage.jsonl=init/vault" \
  "$IH/work/hello.txt=init/work" | sed 's/^/[s1 collect] /'
cleanup
trap - EXIT
log "stopped; verdict: $(cat "$D/verdict.json")"
if [ "$KEEP" != --keep ]; then rm -rf -- "$D"; log "removed $D"; fi
/usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))["pass"] else 1)' "$OUT/run/verdict.json"
