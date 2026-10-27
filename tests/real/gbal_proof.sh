#!/bin/bash
# GBAL real-model proof (ADR-013; plan, cost and the first run: build/status/GBAL.md, build/status/GBAL-REAL.md).
# One disposable install from this checkout's release directory into isolated homes (never ~/.init, ~/.init-root, the
# service, ~/.local/bin or rc files), workspace mode (no home reads, never full access), extend.mode = free.
#
#   tests/real/gbal_proof.sh --mock                                   # $0: the same flow on a scripted init-mockllm
#   GBAL_I_MEAN_IT=1 GBAL_CAP_USD=0.60 [GBAL_STOP_USD=0.50] tests/real/gbal_proof.sh   # COSTS MONEY (deepseek-flash)
#
# Needs: target/dist/v<version>/ from scripts/release.sh at HEAD (install.sh --reuse); rustup 1.97 with wasm32-wasip2
# (the install-mode kernel builds the body, ~1.6 GB). Real mode: the key at ~/.config/init-agent/deepseek.key is copied
# (never printed) into the test Root's keys/providers/deepseek, the only place it is written.
# Knobs: GBAL_RUN (default ~/.gbal-proof, or ~/.gbal-mock; short: homes ≤ 64 bytes, sun_path; must not exist),
# GBAL_MIN_FREE_GIB (refuse to start below; default 22), GBAL_FLOOR_GIB (the charter's disk floors; default 12: the
# free-path admission needs floor + 2 GiB), GBAL_DISK_STOP_GIB (the driver's hard stop; default 15.2),
# GBAL_REGRESSION=0 (skip the deliberate-regression phase), GBAL_ROUNDS (the goal's rounds budget; default 3; every
# `initctl goal predict` spends one, re-files after an amend included), GBAL_CHECKIN=0 (skip the owner's `/goal continue`
# check-in before the regression phase; the default check-in interval is 3 predictions, re-files included).
#
# Flow (owner lines through the installed `init-agent --raw`, tests/real/gbal_drive.py; a spend watchdog sends /stop at
# GBAL_STOP_USD on the test install's settled spend):
#   1. five baseline questions (multi-part) about folder a/: the noise floor of every card;
#   2. the owner's goal: done=card:requests_turn<=T, T = max(2, ceil(baseline mean) − 1) from the Root's own turn
#      records, guard=cost_turn:300,provider_errors;
#   3. one working turn: the model runs skills/gbal (read, predict, one T0 change, promote);
#   4. five parallel questions about folder b/ on the new version (the Root reads the cards);
#   5. (GBAL_REGRESSION=1) the owner asks for a deliberately worse T0 change, then five parallel questions about c/:
#      the cost_turn guardrail must roll it back;
# Evidence design (rerun, after the first run's confound): each measurement phase asks the same five question forms
# about a different folder (a/, b/, c/: same layout and sizes, different contents), so no answer can come from what an
# earlier phase put in the context. The card, guardrails and session are the same for all phases.
#   6. the model stops the goal with a reason.
# Pass (the verdict's `pass`): the lineage holds goal(set) → goal_predict (seq before the change's tuple) →
# card_reading(registered) → goal_outcome → goal(stop), and the spend is under the cap. Everything the model says or
# chooses is observed, not required. The run dir is kept for the report: move it to the Trash when done (delete the
# test Root's key file first).
set -euo pipefail
umask 077
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
MOCK=0; [ "${1:-}" = --mock ] && MOCK=1
log() { printf '[gbal %s] %s\n' "$(date +%H:%M:%S)" "$*"; }
die() { log "FAIL: $*"; exit 1; }
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo/root/Cargo.toml" | head -1)"
# GBAL_BUNDLE=<an unpacked prebuilt bundle> (REL-040): install that bundle (its own install.sh, the prebuilt body)
# instead of target/dist/v<version>/ through the checkout's install.sh --reuse.
DIST="${GBAL_BUNDLE:-$repo/target/dist/v$VERSION}"
[ -f "$DIST/MANIFEST.json" ] || die "no $DIST (run scripts/release.sh --skip-tests first, or set GBAL_BUNDLE)"
PW_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
if [ $MOCK = 1 ]; then
  cap=0.60; stop_musd=500000; key=""
  run="${GBAL_RUN:-$PW_HOME/.gbal-mock}"
else
  [ "${GBAL_I_MEAN_IT:-}" = 1 ] || { echo "refused: set GBAL_I_MEAN_IT=1 (the owner's authorisation)" >&2; exit 64; }
  cap="${GBAL_CAP_USD:-}"
  case "$cap" in 0.[0-9][0-9]|1.00) ;; *) echo "refused: GBAL_CAP_USD must be 0.01..1.00" >&2; exit 64 ;; esac
  stop="${GBAL_STOP_USD:-$cap}"
  stop_musd=$(/usr/bin/python3 -I -c 'import sys; s, c = float(sys.argv[1]), float(sys.argv[2]); print(int(min(s, c) * 1e6))' "$stop" "$cap")
  key="$PW_HOME/.config/init-agent/deepseek.key"
  [ -r "$key" ] || { echo "refused: no key at $key" >&2; exit 64; }
  run="${GBAL_RUN:-$PW_HOME/.gbal-proof}"
fi
min_free="${GBAL_MIN_FREE_GIB:-22}"; floor="${GBAL_FLOOR_GIB:-12}"
free_gib=$(df -k / | awk 'NR==2 {print int($4/1048576)}')
[ "$free_gib" -ge "$min_free" ] || { echo "refused: $free_gib GiB free, GBAL_MIN_FREE_GIB is $min_free" >&2; exit 64; }
[ ! -e "$run" ] || { echo "refused: $run exists; move it to the Trash first" >&2; exit 64; }
RH="$run/rh"; IH="$run/ih"; BIN="$run/bin"; OUT="$run/out"
[ ${#RH} -le 64 ] && [ ${#IH} -le 64 ] || die "homes exceed 64 bytes ($RH)"
mkdir -p "$run/ws/src" "$OUT"
export INIT_ROOT_HOME="$RH" INIT_HOME="$IH" INIT_BIN_DIR="$BIN"

ROOT_PID=; MOCK_PID=
cleanup() {
  set +e
  if [ -n "$ROOT_PID" ] && kill -0 "$ROOT_PID" 2>/dev/null; then
    env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$RH/bin/init-root" stop >/dev/null 2>&1
    for _ in $(seq 1 80); do kill -0 "$ROOT_PID" 2>/dev/null || break; sleep 0.25; done
    kill -TERM "$ROOT_PID" 2>/dev/null # only the Root this script started, by its pid
  fi
  [ -n "$MOCK_PID" ] && kill -TERM "$MOCK_PID" 2>/dev/null
}
trap cleanup EXIT

# ---------------------------------------------------------------- the workspace the questions are about
# Three parallel folders: the same files and line counts, different names, numbers and notes.
mkws() { # <dir> <title> <what it counts> <rows (name,count …)> <deadline line> <license>
  local d="$run/ws/$1"; mkdir -p "$d/src"
  printf '# %s\n\nA tiny command-line counter of %s.\n\nUsage: python3 src/tally.py <name>   adds one to <name>\n       python3 src/report.py         prints every tally\n\nData lives in data.csv (name,count). Notes for the next release are in notes.txt.\nLicense: %s.\n' "$2" "$3" "$6" >"$d/README.md"
  printf 'name,count\n' >"$d/data.csv"; for r in $4; do printf '%s\n' "$r" >>"$d/data.csv"; done
  printf 'release notes\n- add a --reset flag\n- %s\n- report.py should sort by count\n' "$5" >"$d/notes.txt"
  printf 'import csv, sys\n\nname = sys.argv[1]\nrows = dict(csv.reader(open("data.csv")))\nrows[name] = str(int(rows.get(name, 0)) + 1)\n' >"$d/src/tally.py"
  printf 'import csv\n\nfor name, count in csv.reader(open("data.csv")):\n    print(name, count)\n' >"$d/src/report.py"
}
mkws a tally fruit "apples,3 pears,7 plums,1 figs,12" "the deadline for 0.2 is Friday" MIT
mkws b trees "trees in the park" "oak,4 elm,9 ash,2 yew,15 fir,6" "the deadline for 0.3 is the 14th of November" Apache-2.0
mkws c paints "paint tins by colour" "red,8 blue,5 green,11" "the deadline for 1.0 is the end of the quarter" BSD-3-Clause
# ---------------------------------------------------------------- the mock ($0) and the charter
MOCK_UP=""
if [ $MOCK = 1 ]; then
  /usr/bin/python3 -I "$repo/tests/real/gbal_mock.py" "$run/gbal.mock.jsonl"
  "$DIST/init-mockllm" --mode upstream --listen 127.0.0.1:0 --strict deepseek --port-file "$run/mock.port" \
    --script "$run/gbal.mock.jsonl" --record-dir "$run/mock" >"$run/mock.out" 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 100); do [ -s "$run/mock.port" ] && break; sleep 0.1; done
  MOCK_UP="127.0.0.1:$(tr -d '\n' <"$run/mock.port")"
  log "mock upstream $MOCK_UP (pid $MOCK_PID)"
fi
/usr/bin/python3 -I - "$DIST/assets" "$run/charter.toml" "$cap" "$floor" "$MOCK_UP" <<'PY'
import json, os, re, sys
assets, out, cap, floor, mock = sys.argv[1:6]
t = open(os.path.join(assets, "charter-template.toml")).read()
const = open(os.path.join(assets, "CONSTITUTION.md")).read()
def sub(pat, rep):
    global t
    t, n = re.subn(pat, lambda _m: rep, t, count=1, flags=re.M)
    if n != 1:
        sys.exit(f"charter template: no match for {pat}")
sub(r'^text = "REPLACE ME[^"]*"$', 'text = "GBAL real-model proof (a disposable install)."')
sub(r"^max_level = 4\b", "max_level = 0")
sub(r'^day_usd = "5\.00"$', f'day_usd = "{cap}"')
sub(r'^total_usd = "25\.00"$', f'total_usd = "{cap}"')
sub(r"^disk_min_free_gib = 20$", f"disk_min_free_gib = {int(floor)}")
# owner safety: write only the workspace, no home reads outside it, never full access
sub(r"^read_home = true\b.*$", "read_home = false")
sub(r'^mode = "full"\s.*$', 'mode = "workspace"')
sub(r"^allow_full = true\b.*$", "allow_full = false")
sub(r"^# disk_floor_gib = 5\b.*$", f"disk_floor_gib = {int(floor)}")
sub(r"^\[mirror\]$", '[extend]\nmode = "free"\n\n[mirror]')
if mock:  # as install.sh --mock-upstream: the egress forwards to the loopback init-mockllm (root.test_mode)
    sub(r'^destinations = \["api\.deepseek\.com:443"\]$', f'destinations = ["{mock}"]')
    sub(r'^paths = \[.*\]$', 'paths = ["chat/completions", "models", "user/balance"]')
    sub(r"^\[sandbox\]$", f'[actions.providers.providers.deepseek]\nhost = "{mock}"\n\n[sandbox]')
    t += "\n[root]\ntest_mode = true\n"
t += "[constitution]\ntext = '''\n" + const + "'''\n"
open(out, "w").write(t)
PY

# ---------------------------------------------------------------- install, key, start
if [ -n "${GBAL_BUNDLE:-}" ]; then
  log "the bundle's install.sh into $run (prebuilt body)"
  "$DIST/scripts/install.sh" --yes --no-service --charter "$run/charter.toml" >"$OUT/install.log" 2>&1 \
    || { tail -30 "$OUT/install.log"; die "install.sh"; }
else
  log "install.sh --reuse into $run (the install-mode kernel builds the body: minutes)"
  "$repo/scripts/install.sh" --yes --no-service --reuse --charter "$run/charter.toml" >"$OUT/install.log" 2>&1 \
    || { tail -30 "$OUT/install.log"; die "install.sh"; }
fi
if [ $MOCK = 1 ]; then (umask 077; printf 'test-gbal-mock-key' >"$RH/keys/providers/deepseek")
else (umask 077; tr -d '\r\n' <"$key" >"$RH/keys/providers/deepseek"); fi # copied, never printed
RENV=(env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin)
"${RENV[@]}" "$RH/bin/init-root" start --foreground >"$OUT/root.out" 2>&1 </dev/null &
ROOT_PID=$!
ready=0
for _ in $(seq 1 360); do
  if "${RENV[@]}" "$RH/bin/init-root" status --json 2>/dev/null \
       | /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("slot")=="running" else 1)' 2>/dev/null \
     && [ -f "$IH/state/run/ready" ]; then ready=1; break; fi
  sleep 0.5
done
[ $ready = 1 ] || die "kernel+body not ready in 180 s"
log "Root pid $ROOT_PID: kernel and body ready"

# ---------------------------------------------------------------- the owner's lines
drive() { # <phase> <plan json>: one client, the plan's lines; the summaries go to OUT/drive.log
  printf '%s\n' "$2" >"$run/plan-$1.json"
  log "phase $1"
  set +e
  /usr/bin/python3 -I "$repo/tests/real/gbal_drive.py" "$BIN/init-agent" "$PW_HOME" "$RH" "$IH" "$run/ws" "$OUT/client-stream.jsonl" \
    "$stop_musd" "$run/plan-$1.json" | tee -a "$OUT/drive.log"
  local rc=${PIPESTATUS[0]}
  set -e
  [ "$rc" = 0 ] || die "phase $1: driver exit $rc (3 spend stop, 4 disk stop, 5 client gone)"
}
qs() { # <folder>: the five question forms about one folder
  local f="$1"
  printf '[{"say":"In %s/: how many lines are in README.md, and what is its first line?","timeout":300},\n' "$f"
  printf '{"say":"In %s/: which files are in src/, and how many lines does each have?","timeout":300},\n' "$f"
  printf '{"say":"In %s/: what is the header row of data.csv, and how many data rows does it have?","timeout":300},\n' "$f"
  printf '{"say":"In %s/: does notes.txt mention a deadline? Quote the line if so.","timeout":300},\n' "$f"
  printf '{"say":"In %s/: which file is the largest, in bytes?","timeout":300}]\n' "$f"
}
drive baseline "$(qs a)"
T=$(/usr/bin/python3 -I - "$RH/state/goals/state.json" <<'PY'
import json, math, sys
try:
    obs = json.load(open(sys.argv[1])).get("obs", [])
except (OSError, ValueError):
    obs = []
xs = [o.get("req", 0) for o in obs]
print(max(2, math.ceil(sum(xs) / len(xs)) - 1) if xs else 2)
PY
)
log "baseline requests per turn: target requests_turn<=$T"
drive goal "[{\"say\":\"/goal \\\"answer short questions about this folder with fewer model requests per turn\\\" done=card:requests_turn<=$T usd=0.40 wall=3h rounds=${GBAL_ROUNDS:-3} guard=cost_turn:300,provider_errors\",\"kind\":\"root\",\"settle\":4}]"
drive work '[{"say":"There is an active goal: see initctl goal status. Work on it now with skills/gbal: read the goal and the noise floor, choose the mode, make one small T0 change aimed at the goal, commit it, then file your prediction on that final commit sha (after any amend) before you promote it (initctl promote evo/<id> --wait). Keep your answer short.","timeout":1200}]'
drive after "$(qs b)"
if [ "${GBAL_REGRESSION:-1}" = 1 ]; then
  # the owner's check-in between rounds (resets the check-in interval; a no-op for the budget caps)
  [ "${GBAL_CHECKIN:-1}" = 1 ] && drive checkin '[{"say":"/goal continue","kind":"root","settle":4}]'
  drive regress '[{"say":"Guardrail test for this goal: commit one T0 skill that makes you re-read every file you use three times, in separate commands, before you answer; file a prediction for it on that final commit sha before you promote it; then promote it and keep answering my questions as that skill says.","timeout":1200}]'
  drive after2 "$(qs c)"
fi
drive stop '[{"say":"The readings should be in. Check initctl goal status, then stop the goal with the reason that fits (initctl goal stop reason=… note=…). One short paragraph on what the readings showed.","timeout":600}]'

# ---------------------------------------------------------------- records and the verdict
sleep 5
cp "$RH/lineage/lineage.jsonl" "$OUT/root-lineage.jsonl" 2>/dev/null || true
cp "$RH/state/goals/state.json" "$OUT/goals-state.json" 2>/dev/null || true
"${RENV[@]}" "$RH/bin/init-root" egress stats >"$OUT/egress-stats.json" 2>/dev/null || true
"${RENV[@]}" "$RH/bin/init-root" audit verify >"$OUT/audit-verify.txt" 2>&1 || true
if /usr/bin/python3 -I - "$OUT/root-lineage.jsonl" "$OUT/egress-stats.json" "$cap" "$OUT/verdict.json" <<'PY'
import json, sys
lin, stats, cap, out = sys.argv[1:5]
recs = [json.loads(l) for l in open(lin) if l.strip()]
by = lambda k: [r for r in recs if r.get("kind") == k]
sets = [r for r in by("goal") if r["data"].get("op") == "set"]
stops = [r for r in by("goal") if r["data"].get("op") == "stop"]
preds = by("goal_predict")
chain = []
for p in preds:
    sha = p["data"]["sha"]
    tup = next((r for r in by("tuple") if r["data"].get("tuple", {}).get("body", {}).get("sha") == sha), None)
    reads = [r for r in by("card_reading") if r["data"].get("sha") == sha]
    outs = [r for r in by("goal_outcome") if r["data"].get("prediction") == p["data"]["id"]]
    chain.append({"prediction": p["data"]["id"], "sha": sha[:12], "predict_seq": p["seq"], "tuple_seq": tup and tup["seq"],
                  "registered": any(r["data"].get("registered") for r in reads), "readings": [
                      {k: r["data"].get(k) for k in ("card", "verdict", "n_before", "n_after", "mean_before_milli", "mean_after_milli",
                                                     "sd_milli", "se_milli", "z_permille", "action", "registered")} for r in reads],
                  "outcome": outs and {k: outs[-1]["data"].get(k) for k in ("o", "source", "p_permille", "brier_ppm", "n")}})
try:
    spent = json.load(open(stats))["money"]["total_spent_musd"]
except (OSError, ValueError, KeyError):
    spent = None
ok_chain = any(c["tuple_seq"] and c["predict_seq"] < c["tuple_seq"] and c["registered"] and c["outcome"] for c in chain)
# G2: each guardrail rollback reaches the body promptly, to the change's parent (not further back)
from datetime import datetime
ts = lambda r: datetime.fromisoformat(r["at"].replace("Z", "+00:00")).timestamp()
g2 = []
for rd in [r for r in by("card_reading") if r["data"].get("action") == "rollback"]:
    par = next((r for r in by("tuple") if r["data"].get("id") == rd["data"].get("parent_tuple")), None)
    want = par and par["data"]["tuple"]["body"]["sha"]
    rb = next((r for r in by("body_rollback") if r["seq"] > rd["seq"]), None)
    g2.append({"reading_seq": rd["seq"], "card": rd["data"]["card"], "to_parent": bool(rb and rb["data"].get("to_sha") == want),
               "body_rollback_seq": rb and rb["seq"], "latency_s": rb and round(ts(rb) - ts(rd), 1), "to_sha": rb and rb["data"].get("to_sha", "")[:12]})
v = {"pass": bool(sets and stops and ok_chain and spent is not None and spent <= float(cap) * 1e6),
     "g2_rollbacks": g2,
     "goal_set": bool(sets), "stop": stops and {k: stops[-1]["data"].get(k) for k in ("by", "reason", "verified", "spent")},
     "chain": chain, "guard_rollbacks": sum(1 for r in by("card_reading") if r["data"].get("action") == "rollback"),
     "spent_musd": spent, "cap_usd": cap}
json.dump(v, open(out, "w"), indent=1)
print(json.dumps(v, indent=1))
sys.exit(0 if v["pass"] else 1)
PY
then log "PASS"; RC=0; else log "NOT PASSED (the verdict says why)"; RC=1; fi
log "verdict $OUT/verdict.json; move $run to the Trash when you have read it (delete $RH/keys/providers/deepseek first)"
exit $RC
