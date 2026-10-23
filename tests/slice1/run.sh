#!/bin/bash
# Slice-1 $0 regression suite (build/status/S1.md, FIX4.md). Every case runs scripts/slice1/s1.sh, which makes a
# fresh home through the REAL install path (seed bundle → `init-root install --seed-source` → install-mode
# kernel → sandboxed seed build → genesis tuple) under target/slice1-homes/ and removes it afterwards. No case
# touches the network or spends money.
#   1 fakebody  the fake-body seed: root → kernel → init-fakebody round trip; proxy → egress → mock 200
#   2 mock      the repo seed's init-body: write tool, hello.txt = INIT, reply "done"
#   3 watchdog  a stalling model after a paid turn: the harness spend cap interrupts and /stops (rc 3)
#   4 realdry   the production config (test_mode = false, api.deepseek.com) with no key: class.disabled,
#               no upstream connection, no demotion
# Cases 2-4 each do a cold build of the repo seed (wasmtime): about 3 min and about 1.7 GB of transient disk.
# They run one after another, never in parallel; the suite needs S1_SUITE_FREE_MB (default 6500) free to begin
# and stops at the first case whose free-space sampler fell below the floor (s1.sh S1_MIN_FREE_MB, 5300).
# Each case's log: target/slice1-homes/logs/<case>.log (no secrets: the harness never prints a key).
set -uo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd -P)"
S1="$REPO/scripts/slice1/s1.sh"
V="$REPO/build/slice1-artifacts"
L="$REPO/target/slice1-homes/logs"
mkdir -p "$L"
free_mb() { df -m / | awk 'NR==2{print $4}'; }
NEED="${S1_SUITE_FREE_MB:-6500}"
[ "$(free_mb)" -ge "$NEED" ] || { echo "slice1: only $(free_mb) MB free, the suite needs $NEED MB to start"; exit 1; }
fails=0 ran=0 stop=0
check() { # name, verdict file, python predicate over v
  ran=$((ran + 1))
  if [ -f "$2" ] && /usr/bin/python3 -I -c "import json,sys; v=json.load(open(sys.argv[1])); sys.exit(0 if ($3) else 1)" "$2"; then
    echo "ok   $1: $(/usr/bin/python3 -I -c 'import json,sys; v=json.load(open(sys.argv[1])); d=v["disk"]
print("install %.1f s, owner.sock %d ms, start->ready %.1f s, free %s->%s MB (peak %s MB)" % (v["install_ms"] / 1000,
      v["owner_sock_ms"], v["start_to_ready_ms"] / 1000, d["free_start_mb"], d["free_min_mb"], d["peak_use_mb"]))' "$2")"
  else
    echo "FAIL $1: $( [ -f "$2" ] && cut -c1-400 "$2" || tail -5 "$L/$1.log")"; fails=$((fails + 1))
  fi
  # a case whose sampler saw free space below the floor stops the suite (the next cold build would go lower)
  if [ -f "$2" ] && ! /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1])).get("disk_ok") else 1)' "$2"; then
    echo "STOP free disk fell below the floor in $1"; stop=1
  fi
}
run() { # name, then env assignments and s1.sh args
  local name="$1"; shift
  [ $stop = 0 ] || return 0
  rm -f -- "$V/$name/run/verdict.json"
  env "$@" >"$L/$name.log" 2>&1
}
run fakebody "$S1" fakebody; [ $stop = 0 ] && check fakebody "$V/fakebody/run/verdict.json" 'v["pass"] and v["install_check"]["ok"]'
run mock "$S1" mock; [ $stop = 0 ] && check mock "$V/mock/run/verdict.json" 'v["pass"] and v["tool_result_sent"]'
run watchdog S1_TAG=watchdog S1_MOCK_SCRIPT="$REPO/tests/slice1/watchdog.mock.jsonl" S1_SPEND_MAX_MUSD=1000 "$S1" mock
[ $stop = 0 ] && check watchdog "$V/watchdog/run/verdict.json" 'v["drive"]["rc"] == 3 and v["drive"]["spend_cap_hit"] and v["hello_txt"] == "INIT\n" and v["install_check"]["ok"] and v["disk_ok"]'
run realdry "$S1" realdry; [ $stop = 0 ] && check realdry "$V/realdry/run/verdict.json" 'v["pass"] and v["refusals"][0]["code"] == "class.disabled"'
if ps -axo command | grep -F "$REPO/target/slice1-homes" | grep -qv grep; then echo "FAIL leftover processes"; fails=$((fails + 1)); fi
echo "slice1: $((ran - fails > 0 ? ran - fails : 0))/4 cases ok, $fails failed$( [ $ran -lt 4 ] && echo ", $((4 - ran)) not run")"
exit $((fails > 0 || ran < 4))
