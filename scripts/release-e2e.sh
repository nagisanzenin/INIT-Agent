#!/bin/bash
# The release end-to-end check, at $0 (docs/release.md step 3): install → start → `init-agent` → one line against the
# mock model → stop → uninstall, all inside target/test-homes/rel/. Nothing touches ~/.init, ~/.init-root,
# ~/Library or ~/.Trash.
#
#   scripts/release-e2e.sh [--keep] [--allow-dirty] [--reuse]   (the last two pass through to install.sh)
#
#   1. a loopback init-mockllm (this script's child), scripted like scripts/slice1/s1.sh mock but expecting only a user
#      message last (the body may add its own notes, e.g. `[init:reload]`, after the owner's line): turn 1 calls `write`
#      (hello.txt = INIT), turn 2 says "done"
#   2. scripts/install.sh --yes --no-service --disk-floor 5 --mock-upstream 127.0.0.1:<port>, with HOME, INIT_ROOT_HOME,
#      INIT_HOME and INIT_BIN_DIR in the test dir (install.sh still gives init-root the passwd HOME, which the class
#      rules need; with --no-service nothing is written there). Before it, the harness puts a foreign file named `init`
#      (not INIT's) in INIT_BIN_DIR: the install must leave it as it is (no shim over it on macOS, no removal on Linux),
#      and so must uninstall.sh (nagisanzenin/INIT-Agent#1)
#   3. a `test-` key written by this harness (install.sh never handles keys), then `init-root start` (our child)
#   4. the installed `init-agent` command (no TUI → init-root attach --raw --no-replay), one owner line, frames until idle
#   5. checks: reply "done", work/hello.txt = INIT, 2 egress requests, 2 mock requests, no ceiling hit
#   6. uninstall.sh while the Root still runs: it stops the zone (init-root stop) and refuses to move a running home;
#      then SIGTERM to our own Root pid; uninstall.sh again, confirmed by typing `uninstall` → homes in the test Trash
# Verdict: target/test-homes/rel/verdict.json, copied to build/status/rel-e2e.json. The test dir is removed (exact
# path) unless --keep. Signals go only to the PIDs this script spawned.
set -euo pipefail
umask 077

REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
KEEP=0; PASS_ARGS=()
for a in "$@"; do
  case "$a" in --keep) KEEP=1 ;; --allow-dirty|--reuse) PASS_ARGS+=("$a") ;; *) echo "usage: $0 [--keep] [--allow-dirty] [--reuse]" >&2; exit 64 ;; esac
done
D="$REPO/target/test-homes/rel"
H="$D/home"
RH="$D/r"
IH="$D/i"
BIN="$H/.local/bin"
PW_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/root/Cargo.toml" | head -1)"
MOCK_SRC="${REL_MOCK_BIN:-$REPO/target/dist/v$VERSION/init-mockllm}"   # the shipped mock, else a debug build
[ -x "$MOCK_SRC" ] || MOCK_SRC="$REPO/kernel/target/debug/init-mockllm"
log() { printf '[e2e %s] %s\n' "$(date +%H:%M:%S)" "$*"; }
die() { log "FAIL: $*"; exit 1; }
ms() { /usr/bin/python3 -I -c 'import time;print(int(time.time()*1000))'; }
[ ${#RH} -le 64 ] && [ ${#IH} -le 64 ] || die "test homes exceed 64 bytes ($RH)"
[ -x "$MOCK_SRC" ] || die "missing $MOCK_SRC (cargo build --offline -p init-testkit --bin init-mockllm in kernel/)"

PIDS=()
ROOT_PID=
cleanup() {
  set +e
  if [ -n "$ROOT_PID" ] && kill -0 "$ROOT_PID" 2>/dev/null; then
    env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$RH/bin/init-root" stop >/dev/null 2>&1
    kill -TERM "$ROOT_PID" 2>/dev/null
    for _ in $(seq 1 80); do kill -0 "$ROOT_PID" 2>/dev/null || break; sleep 0.25; done
    kill -KILL "$ROOT_PID" 2>/dev/null
  fi
  for p in ${PIDS[@]+"${PIDS[@]}"}; do kill -TERM "$p" 2>/dev/null; done # only our own children, by pid
  wait 2>/dev/null
}
trap cleanup EXIT

if [ -e "$D" ]; then rm -rf -- "$D"; fi
mkdir -p "$H" "$D/mock"
cp -c "$MOCK_SRC" "$D/init-mockllm"

# ---------------------------------------------------------------- 1. the $0 mock upstream
cat >"$D/e2e.mock.jsonl" <<'EOF'
{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"rel-e2e-write-hello","chunk":16}
{"v":1,"kind":"turn","expect":{"last_role":"user"},"reply":{"think":"Write the file.","calls":[{"name":"write","input":{"path":"hello.txt","content":"INIT\n"}}],"stop":"tool_use","usage":{"input":12000,"cache_read":0,"output":40}}}
{"v":1,"kind":"turn","expect":{"prefix":"same"},"reply":{"text":"done","stop":"end_turn","usage":{"input":12100,"cache_read":11904,"output":2}}}
EOF
"$D/init-mockllm" --mode upstream --listen 127.0.0.1:0 --strict deepseek --port-file "$D/mock.port" \
  --script "$D/e2e.mock.jsonl" --record-dir "$D/mock" >"$D/mock.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do [ -s "$D/mock.port" ] && break; sleep 0.1; done
PORT="$(tr -d '\n' <"$D/mock.port")"
log "mock upstream on 127.0.0.1:$PORT (pid ${PIDS[0]})"

# ---------------------------------------------------------------- 2. install.sh --yes --no-service
# a foreign command named init (issue #1): neither install.sh nor uninstall.sh may replace, remove or move it
mkdir -p "$BIN"
printf '#!/bin/sh\necho "a foreign init, not INIT'"'"'s"\n' >"$BIN/init"; chmod 755 "$BIN/init"
cp "$BIN/init" "$D/foreign-init"
T0=$(ms)
set +e
env HOME="$H" INIT_ROOT_HOME="$RH" INIT_HOME="$IH" INIT_BIN_DIR="$BIN" \
  "$REPO/scripts/install.sh" --yes --no-service --disk-floor 5 --mock-upstream "127.0.0.1:$PORT" ${PASS_ARGS[@]+"${PASS_ARGS[@]}"} \
  >"$D/install.log" 2>&1 </dev/null
IRC=$?
set -e
INSTALL_MS=$(( $(ms) - T0 ))
log "install.sh exit $IRC in $INSTALL_MS ms"
[ $IRC = 0 ] || { tail -40 "$D/install.log"; die "install.sh"; }
[ -x "$BIN/init-agent" ] || die "install.sh wrote no $BIN/init-agent"
cmp -s "$BIN/init" "$D/foreign-init" || die "install.sh replaced the foreign $BIN/init"
grep -qF "note: $BIN/init is not INIT's; left as it is" "$D/install.log" || die "install.sh did not report the foreign $BIN/init"
WHICH="$(env HOME="$H" "$BIN/init-agent" --which)"
if [ -x "$BIN/init-tui" ]; then # the release ships the TUI: `init-agent` opens it; the fallback is checked with it moved aside
  [ "$WHICH" = "$BIN/init-tui" ] || die "init-agent --which: $WHICH (want $BIN/init-tui)"
  mv "$BIN/init-tui" "$D/init-tui.aside"
  FALLBACK="$(env HOME="$H" "$BIN/init-agent" --which)"
  mv "$D/init-tui.aside" "$BIN/init-tui"
else
  FALLBACK="$WHICH"
fi
[ "$FALLBACK" = "$RH/bin/init-root attach" ] || die "init-agent --which without a TUI: $FALLBACK"
# T-4 (docs/tui.md): an INIT_TUI inside INIT_HOME is refused (exit 77), so the zone cannot substitute the client
printf '#!/bin/bash\necho zone-written\n' >"$IH/work/init-tui"; chmod 755 "$IH/work/init-tui"
set +e; env HOME="$H" INIT_TUI="$IH/work/init-tui" "$BIN/init-agent" --which >/dev/null 2>&1; T4=$?; set -e
rm -f -- "$IH/work/init-tui"
[ $T4 = 77 ] || die "init-agent ran with INIT_TUI inside INIT_HOME (exit $T4, want 77)"
log "launcher: init-agent opens $WHICH; without a TUI it falls back to init-root attach; INIT_TUI inside INIT_HOME refused (77); the foreign init kept"

# ---------------------------------------------------------------- 3. key (harness only) and start
(umask 077; printf 'test-rel-e2e-mock-key' >"$RH/keys/providers/deepseek")
RENV=(env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin)
"${RENV[@]}" "$RH/bin/init-root" keys check >"$D/keys-check.out" 2>&1
grep -q '^providers/deepseek: present' "$D/keys-check.out" || { cat "$D/keys-check.out"; die "key not present"; }
T1=$(ms)
"${RENV[@]}" "$RH/bin/init-root" start --foreground >"$D/root.out" 2>&1 </dev/null &
ROOT_PID=$!
log "init-root start (pid $ROOT_PID)"
st() { "${RENV[@]}" "$RH/bin/init-root" status --json 2>/dev/null; }
ready=0
for _ in $(seq 1 480); do # up to 240 s: a loaded machine
  if st | /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("slot")=="running" else 1)' 2>/dev/null \
     && [ -f "$IH/state/run/ready" ]; then ready=1; break; fi
  sleep 0.5
done
st >"$D/status-ready.json" || true
[ $ready = 1 ] || { tail -20 "$RH"/logs/*.stderr 2>/dev/null; die "kernel+body not ready in 240 s"; }
READY_MS=$(( $(ms) - T1 ))
log "ready after $READY_MS ms"

# ---------------------------------------------------------------- 4. `init-agent` and one owner line
set +e
/usr/bin/python3 -I - "$BIN/init-agent" "$H" "$D/attach.jsonl" >"$D/drive.json" <<'PY'
import json, queue, re, subprocess, sys, threading, time
init, home, out = sys.argv[1:4]
p = subprocess.Popen([init, "--raw", "--no-replay"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                     env={"HOME": home, "PATH": "/usr/bin:/bin"}, text=True, bufsize=1)
q = queue.Queue()
threading.Thread(target=lambda: ([q.put(l.rstrip("\n")) for l in p.stdout], q.put(None)), daemon=True).start()
t0 = time.monotonic()
time.sleep(0.3)
p.stdin.write("Create a file hello.txt containing the word INIT, then reply done.\n")
p.stdin.flush()
texts, busy, idle, rc = [], False, False, 2
with open(out, "w") as f:
    while time.monotonic() - t0 < 240:
        try:
            line = q.get(timeout=0.5)
        except queue.Empty:
            continue
        if line is None:
            break
        f.write(line + "\n")
        try:
            v = json.loads(line)
        except ValueError:
            continue
        d = v.get("display") if isinstance(v.get("display"), dict) else v
        if v.get("t") == "display" and (d.get("kind") in ("text", None)) and isinstance(d.get("text"), str):
            texts.append(d["text"])
        if v.get("t") == "state":
            if v.get("busy") is True:
                busy = True
            elif v.get("busy") is False and busy:
                idle = True
                break
reply = "".join(texts)
rc = 0 if idle and re.search(r"(?i)\bdone\b", reply) else (1 if idle else 2)
p.stdin.close()
try:
    p.wait(timeout=3)
except subprocess.TimeoutExpired:
    p.terminate()  # our own child
    p.wait()
print(json.dumps({"rc": rc, "idle_after_task": idle, "reply": reply[-200:], "wall_ms": int((time.monotonic() - t0) * 1000)}))
sys.exit(rc)
PY
DRC=$?
set -e
log "drive: $(cat "$D/drive.json")"
curl -s --max-time 5 --unix-socket "$RH/run/egress.sock" http://root/_root/ceilings >"$D/ceilings.json" || true
AUDIT_RC=0; "${RENV[@]}" "$RH/bin/init-root" audit verify --full >"$D/audit.out" 2>&1 || AUDIT_RC=$?
st >"$D/status-end.json" || true

# ---------------------------------------------------------------- 5. uninstall: first while the Root runs, then after it stops
set +e
printf 'uninstall\n' | env HOME="$H" INIT_ROOT_HOME="$RH" INIT_BIN_DIR="$BIN" "$REPO/scripts/uninstall.sh" >"$D/uninstall-1.log" 2>&1
U1=$?
set -e
INST_AFTER_STOP="$(st | /usr/bin/python3 -I -c 'import json,sys; print(len(json.load(sys.stdin).get("instances") or []))' 2>/dev/null || echo '?')"
log "uninstall #1 (Root running, no service): exit $U1, instances after its stop: $INST_AFTER_STOP, homes still there: $([ -d "$RH" ] && [ -d "$IH" ] && echo yes || echo no)"
kill -TERM "$ROOT_PID"
for _ in $(seq 1 120); do kill -0 "$ROOT_PID" 2>/dev/null || break; sleep 0.25; done
kill -0 "$ROOT_PID" 2>/dev/null && die "our Root (pid $ROOT_PID) did not exit on SIGTERM"
ROOT_RC=0; wait "$ROOT_PID" 2>/dev/null || ROOT_RC=$?
ROOT_PID=
log "our Root exited ($ROOT_RC) on SIGTERM"
cp "$IH/work/hello.txt" "$D/hello.txt" 2>/dev/null || true
cp -R "$RH/egress/usage" "$D/egress-usage" 2>/dev/null || true
cp "$RH/lineage/lineage.jsonl" "$D/lineage.jsonl" 2>/dev/null || true
set +e
printf 'no\n' | env HOME="$H" INIT_ROOT_HOME="$RH" INIT_BIN_DIR="$BIN" "$REPO/scripts/uninstall.sh" >"$D/uninstall-2.log" 2>&1
U2=$?
printf 'uninstall\n' | env HOME="$H" INIT_ROOT_HOME="$RH" INIT_BIN_DIR="$BIN" "$REPO/scripts/uninstall.sh" >"$D/uninstall-3.log" 2>&1
U3=$?
set -e
log "uninstall #2 (answer 'no'): exit $U2; #3 (typed uninstall): exit $U3"

# ---------------------------------------------------------------- verdict
/usr/bin/python3 -I - "$D" "$IRC" "$INSTALL_MS" "$READY_MS" "$DRC" "$AUDIT_RC" "$U1" "$INST_AFTER_STOP" "$U2" "$U3" "$RH" "$IH" "$BIN" "$WHICH" <<'PY' | tee "$D/verdict.json"
import glob, json, os, sys
d, irc, install_ms, ready_ms, drc, audit_rc, u1, inst, u2, u3, rh, ih, bindir, client = sys.argv[1:15]
def j(p):
    try: return json.load(open(p))
    except Exception: return None
usage = []
for f in sorted(glob.glob(os.path.join(d, "egress-usage/*.jsonl"))):
    usage += [json.loads(l) for l in open(f) if l.strip()]
lin = [json.loads(l) for l in open(os.path.join(d, "lineage.jsonl"))] if os.path.isfile(os.path.join(d, "lineage.jsonl")) else []
trash = os.path.join(d, "home/.Trash")
moved = sorted(os.listdir(trash)) if os.path.isdir(trash) else []
hello = open(os.path.join(d, "hello.txt")).read() if os.path.isfile(os.path.join(d, "hello.txt")) else None
c = j(os.path.join(d, "ceilings.json")) or {}
v = {"client": "init-root attach" if client.endswith(" attach") else os.path.basename(client),
     "install_rc": int(irc), "install_ms": int(install_ms), "start_to_ready_ms": int(ready_ms), "drive": j(os.path.join(d, "drive.json")),
     "hello_txt": hello, "egress_requests": len([u for u in usage if u.get("t") == "end"]),
     "mock_requests": len(glob.glob(os.path.join(d, "mock/*-turn.json"))),
     "day_spent_musd": (c.get("money") or {}).get("day_spent_musd"),
     "ceiling_hits": [r.get("data") for r in lin if r.get("kind") == "ceiling_hit"],
     "lineage_kinds": [r.get("kind") for r in lin], "audit_verify_rc": int(audit_rc),
     "uninstall_running_rc": int(u1), "instances_after_uninstall_stop": inst, "uninstall_declined_rc": int(u2),
     "uninstall_rc": int(u3), "trash": moved,
     "homes_gone": not os.path.exists(rh) and not os.path.exists(ih) and not os.path.exists(os.path.join(bindir, "init-agent")),
     "foreign_init_kept": os.path.isfile(os.path.join(bindir, "init"))
                          and open(os.path.join(bindir, "init"), "rb").read() == open(os.path.join(d, "foreign-init"), "rb").read()}
v["pass"] = (v["install_rc"] == 0 and (v["drive"] or {}).get("rc") == 0 and (hello or "").strip() == "INIT"
             and v["egress_requests"] >= 2 and v["mock_requests"] == 2 and not v["ceiling_hits"] and v["audit_verify_rc"] == 0
             and v["uninstall_running_rc"] == 1 and inst == "0" and v["uninstall_declined_rc"] == 1 and v["uninstall_rc"] == 0
             and {m.split(" ")[0] for m in moved} >= {"r", "i", "init-agent"} and "init" not in {m.split(" ")[0] for m in moved}
             and v["homes_gone"] and v["foreign_init_kept"])
print(json.dumps(v, sort_keys=True))
PY
cp "$D/verdict.json" "$REPO/build/status/rel-e2e.json"
cleanup
trap - EXIT
PASS="$(/usr/bin/python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["pass"])' "$D/verdict.json")"
if [ $KEEP = 0 ]; then rm -rf -- "$D"; log "removed $D"; fi
log "verdict: pass=$PASS"
[ "$PASS" = True ]
