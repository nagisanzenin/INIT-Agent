#!/bin/bash
# The $0 check of scripts/qualify-provider.sh on the mock model (v0.5.0 L3; lane LINUX-PROV). It tests the script's
# logic end to end on a real install: the client, the goal budget, the Root's ledger, the session log, the table.
#
#   tests/real/qualify_mock.sh [--keep] [BUNDLE.tar.gz]
#       (default: target/dist/v<version>-preview/init-v<version>-preview-<this machine>.tar.gz, any bundle >= 0.4.0)
#
# A disposable T0 install of the prebuilt bundle (get.sh --from … --no-rust -- --no-service) into /private/tmp/i5-qm
# (short homes for the Root's 64-byte limits), a strict DeepSeek-wire init-mockllm scripted for two qualify sessions,
# the Root in the foreground (our child), then:
#   1. a passing run: cache read on the second request, the tool's digest in the reply, the pause and the resume, the
#      ledger and the session log agree, cost within 5 % (DeepSeek has no write tier): exit 0, every check PASS. The
#      mock's recorded third request must hold the digest as the bash result: INIT really ran the command;
#   2. a failing run: the mock never reads the cache and its reply lacks the digest: exit 1, `cache` and
#      `tool_round_trip` FAIL, the other checks PASS;
#   3. refusals, nothing sent (the ledger does not grow): no --cap-usd (64), --cap-usd 11 (64), a provider that is not
#      the selected one (2).
# Never a live provider, never ~/.init, ~/.init-root, ~/.local/bin or ~/.config: the homes are under the run dir and
# the key is the harness's own `test-` key. Signals go only to our own children. The run dir is removed unless --keep.
set -euo pipefail
umask 077
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
KEEP=0; TGZ=""
for a in "$@"; do case "$a" in --keep) KEEP=1 ;; -*) echo "usage: $0 [--keep] [BUNDLE.tar.gz]" >&2; exit 64 ;; *) TGZ="$a" ;; esac; done
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/root/Cargo.toml" | head -1)"
case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) TARGET=macos-arm64 ;; Linux/aarch64) TARGET=linux-arm64 ;; Linux/x86_64) TARGET=linux-x86_64 ;;
  *) echo "no bundle target here" >&2; exit 64 ;;
esac
TGZ="${TGZ:-$REPO/target/dist/v$VERSION-preview/init-v$VERSION-preview-$TARGET.tar.gz}"
NAME="$(basename "$TGZ" .tar.gz)"
D=/private/tmp/i5-qm; [ "$(uname -s)" = Linux ] && D=/tmp/i5-qm
H="$D/h"; RH="$D/r"; IH="$D/i"
PW_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
log() { printf '[qm %s] %s\n' "$(date +%H:%M:%S)" "$*"; }
die() { log "FAIL: $*"; exit 1; }
[ -f "$TGZ" ] || die "no bundle $TGZ"
[ ! -e "$D" ] || die "$D exists (a run in progress, or a kept one): move it away first"
FREE_G="$(df -k / | awk 'NR==2{print int($4/1048576)}')"
[ "$FREE_G" -ge 21 ] || die "only $FREE_G GiB free; this needs the 21 GiB floor"

MOCK_PID=; ROOT_PID=
cleanup() {
  set +e
  if [ -n "$ROOT_PID" ] && kill -0 "$ROOT_PID" 2>/dev/null; then
    env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$RH/bin/init-root" stop >/dev/null 2>&1
    kill -TERM "$ROOT_PID" 2>/dev/null
    for _ in $(seq 1 80); do kill -0 "$ROOT_PID" 2>/dev/null || break; sleep 0.25; done
    kill -KILL "$ROOT_PID" 2>/dev/null
  fi
  [ -z "$MOCK_PID" ] || kill -TERM "$MOCK_PID" 2>/dev/null
  wait 2>/dev/null
  if [ $KEEP = 0 ] && [ -e "$D" ]; then chmod -R u+w "$D" 2>/dev/null; rm -rf -- "$D"; fi
}
trap cleanup EXIT
mkdir -p "$H" "$D/mock" "$D/tmp"
owner_state() { # names, sizes and mtimes of the owner's INIT paths (live timer-written dirs: names only)
  /usr/bin/python3 -I - "$PW_HOME" <<'PY'
import os, sys
h, out = sys.argv[1], []
LIVE = (".init/root", ".init/state/run", ".init/state/kernel/evo")
for top in (".init", ".init-root", ".local/bin", ".local/share/init"):
    for d, dirs, files in os.walk(os.path.join(h, top)):
        if os.path.relpath(d, h) == ".init-root":
            dirs[:] = [x for x in dirs if x != "sealed"]
        dirs.sort()
        for n in sorted(dirs) + sorted(files):
            p = os.path.join(d, n)
            try:
                st = os.lstat(p)
            except FileNotFoundError:
                continue
            rel = os.path.relpath(p, h)
            live = rel in LIVE or rel.startswith(tuple(x + "/" for x in LIVE))
            out.append(f"{rel} {st.st_mode:o}" + ("" if live else f" {st.st_size} {st.st_mtime_ns}"))
print("\n".join(out))
PY
}
owner_state >"$D/owner-before.txt"

# ---------------------------------------------------------------- the mock: two qualify sessions, in order
NONCE=0123456789abcdef
DIGEST="$(printf %s "qualify-$NONCE" | /usr/bin/python3 -I -c 'import hashlib,sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest()[:16])')"
CMD="printf %s qualify-$NONCE | { sha256sum 2>/dev/null || shasum -a 256; }"
/usr/bin/python3 -I - "$D/qm.mock.jsonl" "$CMD" "$DIGEST" <<'PY'
import json, sys
out, cmd, digest = sys.argv[1:4]
nums = " ".join(str(i) for i in range(1, 41))
facts = "\n".join(f"{i}. Rust fact number {i}." for i in range(1, 9))
def turns(read, reply):  # no `expect`: the body appends notes ([init:reload] …) after the owner's line
    r = lambda n: n if read else 0
    return [
        {"reply": {"text": nums, "stop": "end_turn", "usage": {"input": 12000, "cache_read": 0, "output": 120}}},
        {"reply": {"calls": [{"name": "bash", "input": {"command": cmd}}], "stop": "tool_use",
                   "usage": {"input": 12300, "cache_read": r(11776), "output": 30}}},
        {"reply": {"text": reply, "stop": "end_turn", "usage": {"input": 12500, "cache_read": r(12032), "output": 9}}},
        {"fault": {"kind": "stall", "after_events": 3, "ms": 30000},
         "reply": {"text": facts, "stop": "end_turn", "usage": {"input": 12700, "cache_read": r(12288), "output": 80}}},
        {"reply": {"text": "121. Rust has editions.\n122. Rust has traits.\n123. Rust has macros.", "stop": "end_turn",
                   "usage": {"input": 12900, "cache_read": r(12544), "output": 24}}},
    ]
with open(out, "w") as f:
    f.write(json.dumps({"v": 1, "kind": "meta", "provider": "deepseek", "model": "deepseek-flash", "name": "qualify-mock", "chunk": 16}) + "\n")
    for t in turns(True, digest) + turns(False, "I could not run it."):
        f.write(json.dumps(dict({"v": 1, "kind": "turn"}, **t)) + "\n")
PY
tar -xzf "$TGZ" -C "$D" "$NAME/init-mockllm"
mv "$D/$NAME/init-mockllm" "$D/init-mockllm"; rmdir "$D/$NAME"
"$D/init-mockllm" --mode upstream --listen 127.0.0.1:0 --strict deepseek --port-file "$D/mock.port" \
  --script "$D/qm.mock.jsonl" --record-dir "$D/mock" >"$D/mock.out" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 100); do [ -s "$D/mock.port" ] && break; sleep 0.1; done
PORT="$(tr -d '\n' <"$D/mock.port")"
log "mock upstream on 127.0.0.1:$PORT (pid $MOCK_PID)"

# ---------------------------------------------------------------- the install (T0, no service) and the Root
env -i HOME="$H" PATH=/usr/bin:/bin:/usr/sbin:/sbin TMPDIR="$D/tmp/" LANG=en_US.UTF-8 INIT_ROOT_HOME="$RH" INIT_HOME="$IH" \
  /bin/sh "$REPO/scripts/get.sh" --from "$TGZ" --no-rust -- --no-service --disk-floor 5 --mock-upstream "127.0.0.1:$PORT" \
  >"$D/get.log" 2>&1 </dev/null || { tail -30 "$D/get.log"; die "get.sh"; }
(umask 077; printf 'test-qualify-mock-key' >"$RH/keys/providers/deepseek")
# the Root runs with the passwd home as HOME, as scripts/bundle-e2e.sh starts it (its zones check HOME_REAL against
# passwd); INIT_ROOT_HOME and INIT_HOME stay in the run dir, and the owner's state is compared before and after
env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$RH/bin/init-root" start --foreground >"$D/root.out" 2>&1 </dev/null &
ROOT_PID=$!
ready=0
for _ in $(seq 1 240); do
  if env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$RH/bin/init-root" status --json 2>/dev/null \
       | /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("slot")=="running" else 1)' 2>/dev/null \
     && [ -f "$IH/state/run/ready" ]; then ready=1; break; fi
  sleep 0.5
done
[ $ready = 1 ] || { tail -5 "$RH"/logs/*.stderr 2>/dev/null; die "kernel+body not ready in 120 s"; }
log "install ready (Root pid $ROOT_PID)"

Q=(env -i HOME="$H" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin LANG=en_US.UTF-8 QUALIFY_NONCE="$NONCE" /bin/bash "$REPO/scripts/qualify-provider.sh")
verdict() { /usr/bin/python3 -I -c 'import json,sys; v=json.load(open(sys.argv[1])); print(" ".join(k + "=" + ("PASS" if c["pass"] else "FAIL") for k, c in sorted(v["checks"].items())))' "$1"; }

# ---------------------------------------------------------------- 1. the passing run
set +e; "${Q[@]}" --cap-usd 1 --yes --out "$D/q1" >"$D/q1.log" 2>&1; rc=$?; set -e
cat "$D/q1.log"
[ $rc = 0 ] || die "run 1 exit $rc (want 0)"
[ "$(verdict "$D/q1/verdict.json")" = "budget=PASS cache=PASS cost=PASS pause_resume=PASS streaming=PASS tokens=PASS tool_round_trip=PASS" ] \
  || die "run 1 checks: $(verdict "$D/q1/verdict.json")"
/usr/bin/python3 -I - "$D/mock" "$DIGEST" <<'PY' || die "the mock's third request does not carry the digest as the bash result"
import json, os, sys
d, digest = sys.argv[1:3]
reqs = sorted(f for f in os.listdir(d) if f.endswith(".json"))
body = json.load(open(os.path.join(d, reqs[2])))
msgs = body.get("body", body).get("messages", [])
assert any(m.get("role") == "tool" and digest in json.dumps(m.get("content")) for m in msgs), [m.get("role") for m in msgs]
PY
log "run 1: PASS as expected; INIT ran the command (the digest went back to the model)"

# ---------------------------------------------------------------- 2. the failing run
set +e; "${Q[@]}" --cap-usd 1 --yes --out "$D/q2" >"$D/q2.log" 2>&1; rc=$?; set -e
cat "$D/q2.log"
[ $rc = 1 ] || die "run 2 exit $rc (want 1)"
[ "$(verdict "$D/q2/verdict.json")" = "budget=PASS cache=FAIL cost=PASS pause_resume=PASS streaming=PASS tokens=PASS tool_round_trip=FAIL" ] \
  || die "run 2 checks: $(verdict "$D/q2/verdict.json")"
log "run 2: cache and tool_round_trip FAIL as expected"

# ---------------------------------------------------------------- 3. refusals: nothing reaches the model
size() { cat "$RH"/egress/usage/*.jsonl 2>/dev/null | wc -c | tr -d ' '; }
before="$(size)"; reqs_before="$(ls "$D/mock" | grep -c '\.json$' || true)"
for args in "" "--cap-usd 11" "--cap-usd 1 --provider anthropic --model claude-sonnet-5-5 --yes"; do
  set +e; "${Q[@]}" $args >"$D/refuse.log" 2>&1; rc=$?; set -e
  want=64; [ "${args#*--provider}" != "$args" ] && want=2
  [ $rc = $want ] || { cat "$D/refuse.log"; die "[$args] exit $rc (want $want)"; }
  grep -q "refused" "$D/refuse.log" || die "[$args] printed no refusal"
done
[ "$(size)" = "$before" ] && [ "$(ls "$D/mock" | grep -c '\.json$' || true)" = "$reqs_before" ] || die "a refusal reached the model"
log "refusals: no cap (64), cap 11 (64), another provider (2); nothing sent"
owner_state >"$D/owner-after.txt"
diff -u "$D/owner-before.txt" "$D/owner-after.txt" >"$D/owner.diff" || { head -20 "$D/owner.diff"; die "the owner's state changed"; }
log "owner state unchanged: ~/.init, ~/.init-root (sealed/ unlisted), ~/.local/bin, ~/.local/share/init"
log "PASS"
