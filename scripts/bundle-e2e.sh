#!/bin/bash
# The prebuilt-bundle end-to-end check, at $0 (docs/release.md §2b, §5b; lane UX-BUNDLE): a user without Rust runs
# get.sh on the bundle, then `init`, sends one line and gets the mock model's reply. Everything lives in
# target/test-homes/ube/; the owner's ~/.init, ~/.init-root, ~/.local/bin, ~/.local/share/init,
# ~/Library/LaunchAgents and shell startup files are snapshotted before and after and must not change.
#
#   scripts/bundle-e2e.sh [--keep] [TARBALL]     (default: target/dist/v<version>-preview/init-…-macos-arm64.tar.gz)
#
#   1. a loopback init-mockllm from the bundle (one turn: "hello from the mock")
#   2. get.sh --from TARBALL -- --no-service --disk-floor 5 --mock-upstream 127.0.0.1:<port>, with HOME,
#      INIT_ROOT_HOME, INIT_HOME in the test dir and PATH=/usr/bin:/bin:/usr/sbin:/sbin (no ~/.cargo/bin: no Rust)
#   3. a `test-` key written by this harness, `init-root start --foreground` (our child)
#   4. the installed `init`, one owner line, frames until idle; the reply must be the mock's
#   5. checks: the genesis body is the bundle's prebuilt one; `init-root status` says self-evolution T0 with the
#      rustup command; INIT_HOME/root/status.json carries the same `evolution` object; times
# Verdict: build/status/bundle-e2e.json. Signals go only to the PIDs this script spawned. The test dir is removed
# (exact path) unless --keep.
set -euo pipefail
umask 077

REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
KEEP=0; TGZ=""
for a in "$@"; do case "$a" in --keep) KEEP=1 ;; -*) echo "usage: $0 [--keep] [TARBALL]" >&2; exit 64 ;; *) TGZ="$a" ;; esac; done
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/root/Cargo.toml" | head -1)"
NAME="init-v$VERSION-preview-macos-arm64"
TGZ="${TGZ:-$REPO/target/dist/v$VERSION-preview/$NAME.tar.gz}"
NAME="$(basename "$TGZ" .tar.gz)"
D="$REPO/target/test-homes/ube"
H="$D/home"; RH="$D/r"; IH="$D/i"; BIN="$H/.local/bin"
PW_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
NORUST_PATH=/usr/bin:/bin:/usr/sbin:/sbin
log() { printf '[ube %s] %s\n' "$(date +%H:%M:%S)" "$*"; }
die() { log "FAIL: $*"; exit 1; }
ms() { /usr/bin/python3 -I -c 'import time;print(int(time.time()*1000))'; }
[ -f "$TGZ" ] || die "no bundle $TGZ (scripts/bundle.sh)"
[ ${#RH} -le 64 ] && [ ${#IH} -le 64 ] || die "test homes exceed 64 bytes ($RH)"
if env -i PATH="$NORUST_PATH" /bin/sh -c 'command -v cargo || command -v rustup || command -v rustc' >/dev/null; then
  die "cargo/rustup/rustc is on $NORUST_PATH: cannot simulate a Mac without Rust"
fi

# ---------------------------------------------------------------- 0. the owner's state, before
snap() { # metadata only (names, sizes, mtimes, modes) and rc-file hashes; ROOT_HOME/sealed is not listed (M7 sealed S)
  /usr/bin/python3 -I - "$PW_HOME" <<'PY'
import hashlib, os, sys
h = sys.argv[1]
out = []
for top in (".init", ".init-root", ".local/bin", ".local/share/init", "Library/LaunchAgents"):
    root = os.path.join(h, top)
    if not os.path.lexists(root):
        out.append(f"{top} ABSENT"); continue
    for d, dirs, files in os.walk(root):
        if os.path.relpath(d, h) == ".init-root":
            dirs[:] = [x for x in dirs if x != "sealed"]
        dirs.sort()
        for n in sorted(dirs) + sorted(files):
            p = os.path.join(d, n)
            try:
                st = os.lstat(p)
            except FileNotFoundError:
                continue
            out.append(f"{os.path.relpath(p, h)} {st.st_mode:o} {st.st_size} {st.st_mtime_ns}")
for rc in (".zshrc", ".zprofile", ".zshenv", ".bashrc", ".bash_profile", ".profile", ".config/fish/config.fish"):
    p = os.path.join(h, rc)
    out.append(f"{rc} " + (hashlib.sha256(open(p, "rb").read()).hexdigest() if os.path.isfile(p) else "ABSENT"))
print("\n".join(out))
PY
}
PIDS=(); ROOT_PID=
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
  if [ $KEEP = 0 ] && [ -e "$D" ]; then chmod -R u+w "$D" 2>/dev/null; rm -rf -- "$D"; fi
}
trap cleanup EXIT
if [ -e "$D" ]; then chmod -R u+w "$D"; rm -rf -- "$D"; fi
mkdir -p "$H" "$D/mock" "$D/tmp"
snap >"$D/owner-before.txt"

# ---------------------------------------------------------------- 1. the $0 mock upstream (from the bundle itself)
/usr/bin/tar -xzf "$TGZ" -C "$D" "$NAME/init-mockllm"
mv "$D/$NAME/init-mockllm" "$D/init-mockllm"; rmdir "$D/$NAME"
cat >"$D/ube.mock.jsonl" <<'EOF'
{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"bundle-e2e-hello","chunk":16}
{"v":1,"kind":"turn","expect":{"last_role":"user"},"reply":{"text":"hello from the mock","stop":"end_turn","usage":{"input":12000,"cache_read":0,"output":5}}}
EOF
"$D/init-mockllm" --mode upstream --listen 127.0.0.1:0 --strict deepseek --port-file "$D/mock.port" \
  --script "$D/ube.mock.jsonl" --record-dir "$D/mock" >"$D/mock.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do [ -s "$D/mock.port" ] && break; sleep 0.1; done
PORT="$(tr -d '\n' <"$D/mock.port")"
log "mock upstream on 127.0.0.1:$PORT (pid ${PIDS[0]})"

# ---------------------------------------------------------------- 2. get.sh, no Rust on PATH
T0=$(ms)
set +e
env -i HOME="$H" PATH="$NORUST_PATH" TMPDIR="$D/tmp/" LANG=en_US.UTF-8 INIT_ROOT_HOME="$RH" INIT_HOME="$IH" \
  /bin/sh "$REPO/scripts/get.sh" --from "$TGZ" -- --no-service --disk-floor 5 --mock-upstream "127.0.0.1:$PORT" \
  >"$D/get.log" 2>&1 </dev/null
GRC=$?
set -e
GET_MS=$(( $(ms) - T0 ))
log "get.sh exit $GRC in $GET_MS ms"
[ $GRC = 0 ] || { tail -40 "$D/get.log"; die "get.sh"; }
[ -x "$BIN/init" ] || die "no $BIN/init"
grep -q 'self-evolution: T0 only' "$D/get.log" || die "the install did not report T0-only self-evolution"
grep -q 'sh.rustup.rs' "$D/get.log" || die "the install printed no rustup command"
grep -q 'prebuilt body, verified bit for bit' "$D/get.log" || die "the install did not use the prebuilt body"

# ---------------------------------------------------------------- 3. key (harness only) and start
(umask 077; printf 'test-ube-mock-key' >"$RH/keys/providers/deepseek")
RENV=(env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin)
T1=$(ms)
"${RENV[@]}" "$RH/bin/init-root" start --foreground >"$D/root.out" 2>&1 </dev/null &
ROOT_PID=$!
st() { "${RENV[@]}" "$RH/bin/init-root" status --json 2>/dev/null; }
ready=0
for _ in $(seq 1 240); do
  if st | /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("slot")=="running" else 1)' 2>/dev/null \
     && [ -f "$IH/state/run/ready" ]; then ready=1; break; fi
  sleep 0.5
done
[ $ready = 1 ] || { tail -20 "$RH"/logs/*.stderr 2>/dev/null; die "kernel+body not ready in 120 s"; }
READY_MS=$(( $(ms) - T1 ))
log "init-root start (pid $ROOT_PID): ready after $READY_MS ms"

# ---------------------------------------------------------------- 4. `init` and one owner line
set +e
/usr/bin/python3 -I - "$BIN/init" "$H" "$D/attach.jsonl" >"$D/drive.json" <<'PY'
import json, queue, subprocess, sys, threading, time
init, home, out = sys.argv[1:4]
p = subprocess.Popen([init, "--raw", "--no-replay"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                     env={"HOME": home, "PATH": "/usr/bin:/bin"}, text=True, bufsize=1)
q = queue.Queue()
threading.Thread(target=lambda: ([q.put(l.rstrip("\n")) for l in p.stdout], q.put(None)), daemon=True).start()
t0 = time.monotonic()
time.sleep(0.3)
p.stdin.write("Say hello.\n")
p.stdin.flush()
texts, busy, idle = [], False, False
with open(out, "w") as f:
    while time.monotonic() - t0 < 120:
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
rc = 0 if idle and "hello from the mock" in reply else (1 if idle else 2)
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
FIRST_MS=$(( $(ms) - T0 ))
log "drive: $(cat "$D/drive.json")"
"${RENV[@]}" "$RH/bin/init-root" status >"$D/status.txt" 2>&1 || true
st >"$D/status.json" || true
TOOLCHAIN_REC="$(/usr/bin/python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1])).get("toolchain"))' "$RH/install.json")"

# ---------------------------------------------------------------- 5. stop our Root and mock, then the checks
"${RENV[@]}" "$RH/bin/init-root" stop >/dev/null 2>&1 || true
kill -TERM "$ROOT_PID"
for _ in $(seq 1 120); do kill -0 "$ROOT_PID" 2>/dev/null || break; sleep 0.25; done
kill -0 "$ROOT_PID" 2>/dev/null && die "our Root (pid $ROOT_PID) did not exit on SIGTERM"
wait "$ROOT_PID" 2>/dev/null || true
ROOT_PID=
set +e
APP_VER="$(cat "$H/.local/share/init/"*/VERSION 2>/dev/null | head -1)"
/usr/bin/python3 -I - "$D" "$RH" "$IH" "$H/.local/share/init/$APP_VER/body/BODY.json" "$GET_MS" "$READY_MS" "$FIRST_MS" "$DRC" \
  "$TOOLCHAIN_REC" "$TGZ" "$REPO/build/status/bundle-e2e.json" <<'PY'
import json, os, re, sys
d, rh, ih, bodyrec, get_ms, ready_ms, first_ms, drc, tc, tgz, verdict = sys.argv[1:12]
body = json.load(open(bodyrec))
tuples = [json.load(open(os.path.join(rh, "store/tuples", f))) for f in os.listdir(os.path.join(rh, "store/tuples")) if f.endswith(".json")]
genesis_bodies = sorted({t["body"]["binary"] for t in tuples if t["body"]["binary"]})
status_txt = open(os.path.join(d, "status.txt")).read()
status = json.load(open(os.path.join(d, "status.json")))
view = json.load(open(os.path.join(ih, "root/status.json")))
drive = json.load(open(os.path.join(d, "drive.json")))
mock_reqs = len(os.listdir(os.path.join(d, "mock")))
checks = {
    "reply_is_the_mocks": int(drc) == 0 and "hello from the mock" in drive["reply"],
    "genesis_body_is_prebuilt": genesis_bodies == [body["sha256"]],
    "no_toolchain_recorded": tc == "None",
    "status_says_T0_with_rustup": "self-evolution T0 · " in status_txt and "sh.rustup.rs" in status_txt,
    "status_json_evolution": status.get("evolution", {}).get("tiers") == ["T0"],
    "published_view_evolution": view.get("evolution", {}).get("tiers") == ["T0"],
    "install_plus_first_reply_le_90s": int(first_ms) <= 90000,
}
v = {"v": 1, "bundle": os.path.basename(tgz), "bundle_bytes": os.path.getsize(tgz), "body_sha256": body["sha256"],
     "times_ms": {"get_sh_install": int(get_ms), "start_to_ready": int(ready_ms), "get_sh_to_first_reply": int(first_ms),
                  "drive_wall": drive.get("wall_ms")},
     "mock_requests": mock_reqs, "reply": drive["reply"], "checks": checks, "ok": all(checks.values())}
json.dump(v, open(verdict, "w"), indent=1, sort_keys=True)
open(verdict, "a").write("\n")
print(json.dumps(v, sort_keys=True))
sys.exit(0 if v["ok"] else 1)
PY
VRC=$?
set -e
for p in ${PIDS[@]+"${PIDS[@]}"}; do kill -TERM "$p" 2>/dev/null || true; done
wait 2>/dev/null || true
PIDS=()
snap >"$D/owner-after.txt"
if ! diff -u "$D/owner-before.txt" "$D/owner-after.txt" >"$D/owner.diff"; then
  head -20 "$D/owner.diff"; die "the owner's state changed (see $D/owner.diff)"
fi
log "owner state unchanged: ~/.init, ~/.init-root (sealed/ unlisted), ~/.local/bin, ~/.local/share/init, ~/Library/LaunchAgents, shell rc files"
[ $VRC = 0 ] || die "checks failed (build/status/bundle-e2e.json)"
log "PASS"
