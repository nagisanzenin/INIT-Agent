#!/bin/bash
# The prebuilt-bundle end-to-end check, at $0 except --live (docs/release.md §2b, §5b; lane UX-BUNDLE): a user without Rust runs
# get.sh on the bundle, then `init-agent`, sends one line and gets the mock model's reply. Everything lives in
# target/test-homes/ube/; the owner's ~/.init, ~/.init-root, ~/.local/bin, ~/.local/share/init,
# ~/Library/LaunchAgents and shell startup files are snapshotted before and after and must not change.
#
#   scripts/bundle-e2e.sh [--keep] [--t2 | --pause | --live] [TARBALL]
#                         (default TARBALL: target/dist/v<version>-preview/init-…-macos-arm64.tar.gz)
#
#   1. a loopback init-mockllm from the bundle (one turn: "hello from the mock")
#   2. get.sh --from TARBALL -- --no-service --disk-floor 5 --mock-upstream 127.0.0.1:<port>, with HOME,
#      INIT_ROOT_HOME, INIT_HOME, RUSTUP_HOME and CARGO_HOME in the test dir and PATH=/usr/bin:/bin:/usr/sbin:/sbin
#      (no ~/.cargo/bin: no Rust). get.sh installs Rust 1.97.1 + wasm32-wasip2 + clippy into the test home (its default; a
#      download of about 150 MB), so the install records that toolchain (T1/T2). UBE_NO_RUST=1 passes --no-rust
#      instead and checks the T0-only install (no toolchain, the rustup hint).
#      Before get.sh the harness plants INIT's launcher of v0.4.0 and earlier at ~/.local/bin/init (the command's old
#      name, nagisanzenin/INIT-Agent#1): the install must remove it and install no `init` (macOS too since v0.5.0, which
#      dropped v0.4.1's one-release shim).
#   3. a `test-` key written by this harness, `init-root start --foreground` (our child)
#   4. the installed `init-agent`, one owner line, frames until idle; the reply must be the mock's
#   5. checks: the genesis body is the bundle's prebuilt one; the recorded toolchain is the test home's (or, with
#      UBE_NO_RUST, `init-root status` says self-evolution T0 with the rustup command); `init-root status --json` and
#      INIT_HOME/root/status.json carry the same `evolution` tiers; times
#   --t2 (REL-G0; about 20-30 min, $0; not with UBE_NO_RUST): the toolchain get.sh installed into the test home makes
#      the install T2. After step 4 the harness commits a no-op change (one
#      comment in src/main.rs, `refactor`, by the agent identity) on evo/noop in INIT_HOME/body and runs
#      `initctl promote evo/noop --wait`: the unmodified seed must pass G0-G3 under the shipped gate (the Root's
#      Seatbelt zones, the shipped init-mockllm, the install's real path lengths). Verdict:
#      build/status/bundle-e2e-t2.json. Knobs (all opt-in, each logged): UBE_MOCK_BIN=<path> replaces the bundle's
#      upstream mock; UBE_COLD_BUILD_S=<s> sets [supervise] build_timeout_cold_s in the test install's kernel config
#      (to check a bundle cut before the 3600 s default); UBE_T2_FROM=<git repo> UBE_T2_REF=<branch> promotes that
#      commit (it must be by the agent identity, on the same seed) instead of the no-op one; UBE_T2_REPRO_EVENT=1 points
#      its Repro trailer at an event of this install (the original names the source install's trace).
#   --pause (REL-032/REL-033; $0, about 30 s; a T0-only install as with UBE_NO_RUST=1): the owner's interrupt is a
#      pause (docs/body/loop.md §3.5.1) on the strict DeepSeek mock. The mock stalls the first turn before any event;
#      2 s after `busy` the harness sends SIGINT to `init-agent --raw` (our child; line mode sends `interrupt`), expects the
#      display `paused · type to continue, or say what to do instead`, no `aborted`, idle within 1 s; then "Please
#      continue." and the mock's second reply. Checks: exactly 2 model requests, no 400 or protocol error in the strict
#      mock's log, the `[init:paused]` note in the resume request and not in the first, and the paused turn replayed as
#      `[no visible output]` with `"reasoning_content": ""` (provider.md §3.8.1). Verdict: build/status/bundle-pause.json.
#   --live (REL-030/REL-033; COSTS MONEY, about $0.0005): one turn against the live provider in the bundle's charter
#      (api.deepseek.com), no mock. UBE_LIVE_KEYFILE=<file> (required) holds the DeepSeek key; it is copied into the
#      test Root's keys/providers/deepseek and never printed or logged. A watchdog stops the turn when the test install's
#      settled spend (INIT_HOME/root/ceilings.json) reaches UBE_LIVE_CAP_MUSD (default 10000 µUSD = $0.01) or 4 provider
#      requests. The reply to "Reply with the word ok." must contain `ok`; the harness waits up to 75 s for the Root to
#      meter the request and records requests, tokens and µUSD. Key hygiene: before the test dir is removed, the key may
#      be in exactly one file of it (the test Root's key file). Verdict: build/status/bundle-live.json.
# Verdict: build/status/bundle-e2e.json. Signals go only to the PIDs this script spawned. The test dir is removed
# (exact path) unless --keep.
set -euo pipefail
umask 077

REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
KEEP=0; T2=0; PAUSE=0; LIVE=0; TGZ=""
USAGE="usage: $0 [--keep] [--t2 | --pause | --live] [TARBALL]"
for a in "$@"; do case "$a" in --keep) KEEP=1 ;; --t2) T2=1 ;; --pause) PAUSE=1 ;; --live) LIVE=1 ;; -*) echo "$USAGE" >&2; exit 64 ;; *) TGZ="$a" ;; esac; done
[ $(( T2 + PAUSE + LIVE )) -le 1 ] || { echo "$USAGE (one mode at most)" >&2; exit 64; }
CAND=evo/noop
if [ -n "${UBE_T2_FROM:-}" ]; then
  case "${UBE_T2_REF:-}" in ""|*[!A-Za-z0-9/_.-]*) echo "UBE_T2_FROM needs UBE_T2_REF=<branch> ([A-Za-z0-9/_.-])" >&2; exit 64 ;; esac
  CAND="evo/${UBE_T2_REF#evo/}"
fi
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/root/Cargo.toml" | head -1)"
NAME="init-v$VERSION-preview-macos-arm64"
TGZ="${TGZ:-$REPO/target/dist/v$VERSION-preview/$NAME.tar.gz}"
NAME="$(basename "$TGZ" .tar.gz)"
D="$REPO/target/test-homes/ube"; VERDICT="$REPO/build/status/bundle-e2e.json"
if [ $T2 = 1 ]; then D="$REPO/target/test-homes/ub2"; VERDICT="$REPO/build/status/bundle-e2e-t2.json"; fi
if [ $PAUSE = 1 ]; then D="$REPO/target/test-homes/ubp"; VERDICT="$REPO/build/status/bundle-pause.json"; fi
if [ $LIVE = 1 ]; then D="$REPO/target/test-homes/rels"; VERDICT="$REPO/build/status/bundle-live.json"; fi
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
NO_RUST="${UBE_NO_RUST:-0}"
if [ $PAUSE = 1 ] || [ $LIVE = 1 ]; then NO_RUST=1; fi # neither needs a toolchain; skip the Rust download
KEYFILE=; CAP_MUSD="${UBE_LIVE_CAP_MUSD:-10000}"
if [ $LIVE = 1 ]; then
  KEYFILE="${UBE_LIVE_KEYFILE:-}"
  [ -n "$KEYFILE" ] && [ -s "$KEYFILE" ] || die "--live needs UBE_LIVE_KEYFILE=<a file holding the DeepSeek key>"
  case "$CAP_MUSD" in ""|*[!0-9]*) die "UBE_LIVE_CAP_MUSD must be a whole number of µUSD" ;; esac
  log "--live: one live DeepSeek turn (costs money); cap $CAP_MUSD µUSD or 4 requests"
fi
[ "$NO_RUST" = 0 ] || [ $T2 = 0 ] || die "--t2 needs the toolchain get.sh installs (unset UBE_NO_RUST)"
GET_ARGS=()
[ "$NO_RUST" = 0 ] || { GET_ARGS=(--no-rust); log "UBE_NO_RUST=1: get.sh --no-rust (a T0-only install)"; }

# ---------------------------------------------------------------- 0. the owner's state, before
snap() { # metadata only (names, sizes, mtimes, modes) and rc-file hashes; ROOT_HOME/sealed is not listed (M7 sealed S)
  /usr/bin/python3 -I - "$PW_HOME" <<'PY'
import hashlib, os, sys
h = sys.argv[1]
out = []
# Written on timers by an idle, running owner install: INIT_HOME/root/ (the Root's published view, ceilings.json),
# state/run/ (the body's heartbeat), state/kernel/evo/ (the kernel's friction miner). Only names and modes there.
LIVE = (".init/root", ".init/state/run", ".init/state/kernel/evo")
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
            rel = os.path.relpath(p, h)
            if rel in LIVE or rel.startswith(tuple(x + "/" for x in LIVE)):
                out.append(f"{rel} {st.st_mode:o}")
            else:
                out.append(f"{rel} {st.st_mode:o} {st.st_size} {st.st_mtime_ns}")
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
# an upgrade from v0.4.0 or earlier: INIT's launcher under its old name (the install checks its header and marker lines)
mkdir -p "$BIN"
cat >"$BIN/init" <<'EOF'
#!/bin/bash
# `init`: open INIT (docs/tui.md §2). Installed by scripts/install.sh into INIT_BIN_DIR (default ~/.local/bin).
# (a test fixture: the launcher as v0.4.0 and earlier installed it, planted by scripts/bundle-e2e.sh)
set -euo pipefail
INIT_ROOT_HOME_DEFAULT="/nonexistent/old-root-home"
echo "the old launcher ran" >&2; exit 1
EOF
chmod 755 "$BIN/init"

# ---------------------------------------------------------------- 1. the $0 mock upstream (from the bundle itself)
# (--live: none; the charter's provider is the live api.deepseek.com)
MOCK_ARGS=()
if [ $LIVE = 0 ]; then
# UBE_MOCK_BIN: another upstream mock (for example kernel/target/debug/init-mockllm, to check a bundle cut before a
# mock fix); the installed one under INIT_HOME/kernel/bin, which the seed's own tests use in G0, is always the bundle's.
if [ -n "${UBE_MOCK_BIN:-}" ]; then
  cp "$UBE_MOCK_BIN" "$D/init-mockllm"; log "upstream mock: $UBE_MOCK_BIN (not the bundle's)"
else
  /usr/bin/tar -xzf "$TGZ" -C "$D" "$NAME/init-mockllm"
  mv "$D/$NAME/init-mockllm" "$D/init-mockllm"; rmdir "$D/$NAME"
fi
if [ $PAUSE = 1 ]; then # the first turn stalls before any event (the pause lands before the first token)
  cat >"$D/ube.mock.jsonl" <<'EOF'
{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"bundle-e2e-pause","chunk":16}
{"v":1,"kind":"turn","expect":{"last_role":"user"},"fault":{"kind":"stall","after_events":0,"ms":30000},"reply":{"text":"never shown","stop":"end_turn"}}
{"v":1,"kind":"turn","expect":{"last_role":"user"},"reply":{"text":"resumed after the pause","stop":"end_turn","usage":{"input":12000,"cache_read":0,"output":5}}}
EOF
else
  cat >"$D/ube.mock.jsonl" <<'EOF'
{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"bundle-e2e-hello","chunk":16}
{"v":1,"kind":"turn","expect":{"last_role":"user"},"reply":{"text":"hello from the mock","stop":"end_turn","usage":{"input":12000,"cache_read":0,"output":5}}}
EOF
fi
if [ $T2 = 1 ]; then cat >>"$D/ube.mock.jsonl" <<EOF
{"v":1,"kind":"turn","expect":{"last_role":"user","last_text_contains":"Promote $CAND"},"reply":{"calls":[{"name":"bash","input":{"command":"initctl promote $CAND"}}],"stop":"tool_use","usage":{"input":12100,"cache_read":11776,"output":9}}}
{"v":1,"kind":"turn","reply":{"text":"promote submitted","stop":"end_turn","usage":{"input":12300,"cache_read":11776,"output":3}}}
EOF
fi
"$D/init-mockllm" --mode upstream --listen 127.0.0.1:0 --strict deepseek --port-file "$D/mock.port" \
  --script "$D/ube.mock.jsonl" --record-dir "$D/mock" >"$D/mock.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do [ -s "$D/mock.port" ] && break; sleep 0.1; done
PORT="$(tr -d '\n' <"$D/mock.port")"
log "mock upstream on 127.0.0.1:$PORT (pid ${PIDS[0]})"
MOCK_ARGS=(--mock-upstream "127.0.0.1:$PORT")
fi

# ---------------------------------------------------------------- 2. get.sh, no Rust on PATH
T0=$(ms)
set +e
env -i HOME="$H" PATH="$NORUST_PATH" TMPDIR="$D/tmp/" LANG=en_US.UTF-8 INIT_ROOT_HOME="$RH" INIT_HOME="$IH" \
  RUSTUP_HOME="$H/.rustup" CARGO_HOME="$H/.cargo" \
  /bin/sh "$REPO/scripts/get.sh" --from "$TGZ" --no-modify-path ${GET_ARGS[@]+"${GET_ARGS[@]}"} -- --no-service --disk-floor 5 \
  ${MOCK_ARGS[@]+"${MOCK_ARGS[@]}"} \
  >"$D/get.log" 2>&1 </dev/null
GRC=$?
set -e
GET_MS=$(( $(ms) - T0 ))
log "get.sh exit $GRC in $GET_MS ms"
[ $GRC = 0 ] || { tail -40 "$D/get.log"; die "get.sh"; }
[ -x "$BIN/init-agent" ] || die "no $BIN/init-agent"
# the old name (issue #1): the install removes INIT's old launcher and installs no init (v0.5.0 dropped v0.4.1's macOS shim)
[ ! -e "$BIN/init" ] && [ ! -L "$BIN/init" ] || die "$BIN/init is still there"
grep -qF "removed $BIN/init, INIT's launcher under its old name" "$D/get.log" || die "the install did not report removing $BIN/init"
RENAME=old-launcher-removed
log "old name: $RENAME"
if [ "$NO_RUST" = 1 ]; then
  grep -q 'self-evolution: T0 only' "$D/get.log" || die "the install did not report T0-only self-evolution"
  grep -q 'sh.rustup.rs' "$D/get.log" || die "the install printed no rustup command"
else
  grep -q 'with wasm32-wasip2 and clippy: INIT can rewrite its tools (T1) and its core (T2)' "$D/get.log" \
    || die "get.sh did not install Rust 1.97.1 with wasm32-wasip2 and clippy"
  [ -x "$H/.cargo/bin/rustup" ] || die "rustup is not in the test home ($H/.cargo/bin)"
fi
# the "installed (prebuilt body, …)" line, not install.sh's step title; v0.5.3's wording, or an older bundle's
grep -qE "installed \(prebuilt body, (hash-equal to the maintainer's build|verified bit for bit)\)" "$D/get.log" \
  || die "the install did not use the prebuilt body"

if [ -n "${UBE_COLD_BUILD_S:-}" ]; then
  printf '\n[supervise]\nbuild_timeout_cold_s = %d\n' "$UBE_COLD_BUILD_S" >>"$IH/kernel/config.toml"
  log "test install: [supervise] build_timeout_cold_s = $UBE_COLD_BUILD_S (UBE_COLD_BUILD_S)"
fi

# ---------------------------------------------------------------- 3. key (harness only) and start
if [ $LIVE = 1 ]; then # the owner's key: copied, never printed
  (umask 077; tr -d '\r\n' <"$KEYFILE" >"$RH/keys/providers/deepseek")
else
  (umask 077; printf 'test-ube-mock-key' >"$RH/keys/providers/deepseek")
fi
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
cat >"$D/drive.py" <<'PY'
import json, queue, subprocess, sys, threading, time
init, home, out, say, want = sys.argv[1:6]
# --live: the test install's ceilings.json and a µUSD cap; the turn stops at the cap or at 4 provider requests
ceil, cap = (sys.argv[6], int(sys.argv[7])) if len(sys.argv) > 7 else (None, None)
def metered():
    try:
        c = json.load(open(ceil))
    except (OSError, ValueError):
        return 0, 0
    reqs = sum(x.get("requests_today") or 0 for x in c.get("classes", []) if x.get("id") == "providers")
    return c.get("money", {}).get("total_spent_musd") or 0, reqs
p = subprocess.Popen([init, "--raw", "--no-replay"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                     env={"HOME": home, "PATH": "/usr/bin:/bin"}, text=True, bufsize=1)
q = queue.Queue()
threading.Thread(target=lambda: ([q.put(l.rstrip("\n")) for l in p.stdout], q.put(None)), daemon=True).start()
t0 = time.monotonic()
time.sleep(0.3)
p.stdin.write(say + "\n")
p.stdin.flush()
texts, busy, idle, fired, turn_end, polled = [], False, False, None, None, 0.0
with open(out, "w") as f:
    while time.monotonic() - t0 < 120:
        if ceil and time.monotonic() - polled >= 0.5: # the spend watchdog
            polled = time.monotonic()
            spent, reqs = metered()
            if spent >= cap or reqs >= 4:
                fired = {"spent_musd": spent, "requests": reqs}
                break
        try:
            line = q.get(timeout=0.25 if ceil else 0.5)
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
        if v.get("t") == "display" and d.get("kind") == "turn_end":
            turn_end = d.get("text")
        if v.get("t") == "state":
            if v.get("busy") is True:
                busy = True
            elif v.get("busy") is False and busy:
                idle = True
                break
reply = "".join(texts)
rc = 0 if idle and not fired and want.lower() in reply.lower() else (1 if idle or fired else 2)
p.stdin.close()
try:
    p.wait(timeout=3)
except subprocess.TimeoutExpired:
    p.terminate()  # our own child
    p.wait()
r = {"rc": rc, "idle_after_task": idle, "reply": reply[-200:], "wall_ms": int((time.monotonic() - t0) * 1000)}
if ceil:
    r.update({"watchdog": fired, "turn_end": turn_end})
print(json.dumps(r))
sys.exit(rc)
PY
cat >"$D/pause.py" <<'PY'
import json, queue, signal, subprocess, sys, threading, time
init, home, out = sys.argv[1:4]
p = subprocess.Popen([init, "--raw", "--no-replay"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                     env={"HOME": home, "PATH": "/usr/bin:/bin"}, text=True, bufsize=1)
q = queue.Queue()
threading.Thread(target=lambda: ([q.put(l.rstrip("\n")) for l in p.stdout], q.put(None)), daemon=True).start()
f = open(out, "w")
infos, texts = [], []
def until(want_busy, limit):  # the next state frame with busy == want_busy; display frames are collected on the way
    t = time.monotonic()
    while time.monotonic() - t < limit:
        try:
            line = q.get(timeout=0.2)
        except queue.Empty:
            continue
        if line is None:
            return False
        f.write(line + "\n")
        try:
            v = json.loads(line)
        except ValueError:
            continue
        d = v.get("display") if isinstance(v.get("display"), dict) else v
        if v.get("t") == "display" and isinstance(d.get("text"), str):
            if d.get("kind") == "info":
                infos.append(d["text"])
            elif d.get("kind") in ("text", None):
                texts.append(d["text"])
        if v.get("t") == "state" and v.get("busy") is want_busy:
            return True
    return False
t0 = time.monotonic()
time.sleep(0.3)
p.stdin.write("Say hi.\n"); p.stdin.flush()
r = {"busy1": until(True, 30)}
time.sleep(2)
p.send_signal(signal.SIGINT)  # our own child: line mode sends `interrupt`
ts = time.monotonic()
r["idle1"] = until(False, 15)
r["idle_after_sigint_ms"] = int((time.monotonic() - ts) * 1000)
r["infos1"] = list(infos)
texts.clear()
time.sleep(0.5)
p.stdin.write("Please continue.\n"); p.stdin.flush()
r["busy2"] = until(True, 30)
r["idle2"] = until(False, 60)
r["reply"] = "".join(texts)[-200:]
r["wall_ms"] = int((time.monotonic() - t0) * 1000)
p.stdin.close()
try:
    p.wait(timeout=3)
except subprocess.TimeoutExpired:
    p.terminate()  # our own child
    p.wait()
f.close()
ok = r["busy1"] and r["idle1"] and r["busy2"] and r["idle2"] and "resumed after the pause" in r["reply"]
print(json.dumps(r))
sys.exit(0 if ok else 1)
PY
set +e
if [ $PAUSE = 1 ]; then
  /usr/bin/python3 -I "$D/pause.py" "$BIN/init-agent" "$H" "$D/attach.jsonl" >"$D/drive.json"
elif [ $LIVE = 1 ]; then
  /usr/bin/python3 -I "$D/drive.py" "$BIN/init-agent" "$H" "$D/attach.jsonl" "Reply with the word ok." "ok" \
    "$IH/root/ceilings.json" "$CAP_MUSD" >"$D/drive.json"
else
  /usr/bin/python3 -I "$D/drive.py" "$BIN/init-agent" "$H" "$D/attach.jsonl" "Say hello." "hello from the mock" >"$D/drive.json"
fi
DRC=$?
set -e
FIRST_MS=$(( $(ms) - T0 ))
log "drive: $(cat "$D/drive.json")"
if [ $LIVE = 1 ]; then # the Root republishes ceilings.json every 60 s and on changes: wait (75 s at most) for the metering
  for _ in $(seq 1 150); do
    /usr/bin/python3 -I -c 'import json,sys; c=json.load(open(sys.argv[1])); sys.exit(0 if sum(x.get("requests_today") or 0 for x in c.get("classes",[]) if x.get("id")=="providers")>=1 and (c.get("money",{}).get("total_spent_musd") or 0)>0 else 1)' "$IH/root/ceilings.json" 2>/dev/null && break
    sleep 0.5
  done
  cp "$IH/root/ceilings.json" "$D/ceilings.json" 2>/dev/null || true
fi

# ---------------------------------------------------------------- 4b. (--t2) a no-op T2 promote of the unmodified seed
PRC=-; PROMOTE_MS=0
if [ $T2 = 1 ]; then
  BODY="$IH/body"
  AGENT_GIT=(env -i HOME="$H" PATH=/usr/bin:/bin git -C "$BODY" -c user.name='INIT Agent' -c user.email=agent@init.local
             -c commit.gpgsign=false)
  if [ -n "${UBE_T2_FROM:-}" ]; then
    "${AGENT_GIT[@]}" fetch -q "$UBE_T2_FROM" "$UBE_T2_REF:$CAND"
    log "candidate $CAND fetched from $UBE_T2_FROM (UBE_T2_FROM)"
    if [ "${UBE_T2_REPRO_EVENT:-0}" = 1 ]; then # its Repro names the source install's trace/event: point it at ours
      SF="$(ls "$IH"/state/sessions/*.jsonl | head -1)"
      EID="$(head -1 "$SF" | /usr/bin/python3 -I -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
      "${AGENT_GIT[@]}" switch -q "$CAND"
      "${AGENT_GIT[@]}" log -1 --format=%B "$CAND" | sed "s|^Repro: .*|Repro: event:$(basename "$SF" .jsonl)#$EID|" >"$D/tmp/cand.msg"
      "${AGENT_GIT[@]}" commit -q --amend -F "$D/tmp/cand.msg"
      log "candidate Repro rewritten to $(grep '^Repro:' "$D/tmp/cand.msg") (UBE_T2_REPRO_EVENT; the tree is unchanged)"
    fi
  else
    "${AGENT_GIT[@]}" switch -q -c evo/noop
    printf '\n// bundle-e2e --t2: a no-op change for the T2 regression check (REL-G0)\n' >>"$BODY/src/main.rs"
    # self.json must match (G1): regenerate it as the agent would (`init-kernel selfmap --write`), without the cold
    # cargo build: a comment changes no manifest or component, so the genesis body and the bundle's ext/ describe it
    # (the same call with --check reproduces the seed's committed self.json byte for byte).
    APP_DIR="$(cd "$H/.local/share/init/"*/ && pwd -P)"
    mkdir -p "$D/tmp/selfmap"
    env -i HOME="$H" PATH=/usr/bin:/bin CARGO_TARGET_DIR="$D/tmp/selfmap" "$IH/kernel/bin/init-kernel" selfmap --root "$BODY" \
      --write --no-build --binary "$IH/bin/init-body" --components "$APP_DIR/ext" >"$D/selfmap.out" 2>&1 \
      || { cat "$D/selfmap.out"; die "selfmap --write"; }
    "${AGENT_GIT[@]}" commit -q -a -F - <<'MSG'
refactor(main): a no-op comment for the T2 regression check

Why: bundle-e2e --t2 proves that the shipped gate passes the unmodified seed on a fresh install.
Predicts: pass_rate >= parent
Compensates-for: none (release regression check)
Prefix-change: no
MSG
  fi
  # The kernel socket refuses peers outside the body's zone (E_PEER), so the live agent starts the command (the
  # mock's turns 2-3, as S-REAL and REAL-M7 did) and the harness waits on the kernel's gate report, never on the reply.
  log "$CAND at $("${AGENT_GIT[@]}" rev-parse --short "$CAND"); the agent runs initctl promote $CAND (cold build, about 20-30 min)"
  T3=$(ms)
  set +e
  /usr/bin/python3 -I "$D/drive.py" "$BIN/init-agent" "$H" "$D/attach2.jsonl" "Promote $CAND." "promote submitted" >"$D/drive2.json"
  PRC=$?
  set -e
  log "drive 2: $(cat "$D/drive2.json")"
  if [ $PRC = 0 ]; then # a refused promote (validator, E_*) makes no job: fail now, with the agent's tool output
    JOB=
    for _ in $(seq 1 60); do
      for j in "$IH"/state/kernel/jobs/j-*.json; do [ -f "$j" ] && JOB="$j"; done
      [ -n "$JOB" ] && break
      sleep 1
    done
    if [ -z "$JOB" ]; then
      PRC=3
      log "no promote job; the agent's initctl output: $(/usr/bin/python3 -I - "$D/mock" <<'PY'
import json, os, sys
d = sys.argv[1]
out = ""
for f in sorted(os.listdir(d)):
    if f.endswith(".json"):
        try:
            rec = json.load(open(os.path.join(d, f)))
            msgs = rec.get("body", rec).get("messages", [])
        except (ValueError, AttributeError):
            continue
        out = next((m.get("content") for m in reversed(msgs) if m.get("role") == "tool"), out) or out
print(str(out)[:600].replace("\n", " "))
PY
)"
    fi
  fi
  # The kernel's job record is the result (a passing promote writes no gate report.json; a G4 job follows it).
  if [ $PRC = 0 ]; then
    for _ in $(seq 1 2700); do # 90 min
      /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1])).get("state") in ("succeeded","failed","cancelled") else 1)' "$JOB" 2>/dev/null && break
      sleep 2
    done
    cp "$JOB" "$D/promote-job.json"
    JID="$(basename "$JOB" .json)"
    [ -f "$IH/state/kernel/gate/$JID/report.json" ] && cp "$IH/state/kernel/gate/$JID/report.json" "$D/gate-report.json"
  fi
  PROMOTE_MS=$(( $(ms) - T3 ))
  log "promote job after $(( PROMOTE_MS / 1000 )) s: $([ -f "$D/promote-job.json" ] && /usr/bin/python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["id"], d["state"], [(s["stage"], s.get("pass"), s.get("wall_ms")) for s in d.get("stages", [])], (d.get("error") or ""))' "$D/promote-job.json" || echo none)"
fi
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
MODE=hello; [ $PAUSE = 1 ] && MODE=pause; [ $LIVE = 1 ] && MODE=live
KEYHITS= # --live: the test dir's files that hold the key (names only), before anything is removed
[ $LIVE = 0 ] || KEYHITS="$(grep -rlF -f "$KEYFILE" "$D" 2>/dev/null | sed "s|^$D/||" | tr '\n' ' ' || true)"
/usr/bin/python3 -I - "$D" "$RH" "$IH" "$H/.local/share/init/$APP_VER/body/BODY.json" "$GET_MS" "$READY_MS" "$FIRST_MS" "$DRC" \
  "$TOOLCHAIN_REC" "$TGZ" "$VERDICT" "$T2" "$PRC" "$PROMOTE_MS" "$NO_RUST" "$H" "$MODE" "$CAP_MUSD" "$KEYHITS" "$RENAME" <<'PY'
import json, os, re, sys
d, rh, ih, bodyrec, get_ms, ready_ms, first_ms, drc, tc, tgz, verdict, t2, prc, promote_ms, no_rust, home, mode, cap, keyhits, rename = sys.argv[1:21]
t2 = t2 == "1"
no_rust = no_rust == "1"
body = json.load(open(bodyrec))
tuples = [json.load(open(os.path.join(rh, "store/tuples", f))) for f in os.listdir(os.path.join(rh, "store/tuples")) if f.endswith(".json")]
genesis_bodies = sorted({t["body"]["binary"] for t in tuples if t["body"]["binary"]})
status_txt = open(os.path.join(d, "status.txt")).read()
status = json.load(open(os.path.join(d, "status.json")))
view = json.load(open(os.path.join(ih, "root/status.json")))
drive = json.load(open(os.path.join(d, "drive.json")))
mock_reqs = len(os.listdir(os.path.join(d, "mock")))
extra = {}
if mode == "pause":
    rec = os.path.join(d, "mock")
    bodies = []
    for n in sorted(os.listdir(rec)):
        if n.endswith(".json"):
            b = json.load(open(os.path.join(rec, n)))
            bodies.append(b.get("body", b) if isinstance(b, dict) else {})
    logp = os.path.join(rec, "mock.log.jsonl")
    mlog = [json.loads(l) for l in open(logp) if l.strip()] if os.path.isfile(logp) else []
    has = lambda b, t: any(t in json.dumps(m) for m in b.get("messages", []))
    resume = bodies[-1] if bodies else {}
    asst = [m for m in resume.get("messages", []) if m.get("role") == "assistant"]
    infos = drive.get("infos1", [])
    checks = {
        "paused_display": "paused · type to continue, or say what to do instead" in infos,
        "no_aborted_display": "aborted" not in infos,
        "idle_after_sigint_le_1s": drive.get("idle1") is True and drive.get("idle_after_sigint_ms", 99999) <= 1000,
        "resume_reply": int(drc) == 0 and "resumed after the pause" in drive.get("reply", ""),
        "two_requests_only": len(bodies) == 2,
        "note_absent_from_first_request": len(bodies) == 2 and not has(bodies[0], "[init:paused]"),
        "note_in_resume_request": has(resume, "[init:paused]"),
        "paused_turn_reasoning_empty": any(m.get("content") == "[no visible output]" and m.get("reasoning_content") == ""
                                           for m in asst),
        "no_400_no_protocol_error": all(not l.get("protocol_error") and l.get("status") != 400 for l in mlog),
    }
    extra = {"requests": len(bodies), "mock_log_lines": len(mlog), "drive": drive}
elif mode == "live":
    cp = os.path.join(d, "ceilings.json")
    c = json.load(open(cp)) if os.path.isfile(cp) else {}
    m = c.get("money", {})
    reqs = sum(x.get("requests_today") or 0 for x in c.get("classes", []) if x.get("id") == "providers")
    checks = {
        "reply_ok": int(drc) == 0 and "ok" in drive.get("reply", "").lower(),
        "watchdog_quiet": drive.get("watchdog") is None,
        "live_request_metered": reqs >= 1 and (m.get("total_spent_musd") or 0) > 0,
        "spend_below_cap": (m.get("total_spent_musd") or 0) < int(cap),
        "key_only_in_the_test_keyfile": keyhits.split() == ["r/keys/providers/deepseek"],
    }
    extra = {"live": {"requests": reqs, "spent_musd": m.get("total_spent_musd"), "reserved_musd": m.get("total_reserved_musd"),
                      "input_tokens": m.get("total_input_tokens"), "output_tokens": m.get("total_output_tokens"),
                      "cap_musd": int(cap), "turn_end": drive.get("turn_end"), "key_files": len(keyhits.split())}}
else:
    checks = {
        "reply_is_the_mocks": int(drc) == 0 and "hello from the mock" in drive["reply"],
    }
# issue #1: the install handled the command's old name (the harness died otherwise; recorded here)
checks["old_name_" + rename.replace("-", "_")] = rename == "old-launcher-removed"
# without Rust the bound is the UX-BUNDLE target; get.sh's Rust install (a ~150 MB download) is allowed 5 min more
if no_rust:
    checks["install_plus_first_reply_le_90s"] = int(first_ms) <= 90000
else:
    checks["install_with_rust_plus_first_reply_le_390s"] = int(first_ms) <= 390000
gate = {}
if not t2 and not no_rust:
    checks.update({
        "genesis_body_is_prebuilt": genesis_bodies == [body["sha256"]],
        "toolchain_in_test_home": tc.startswith(os.path.join(home, ".rustup") + "/"),
        "status_json_evolution_t2": "T2" in (status.get("evolution", {}).get("tiers") or []),
        "published_view_evolution_t2": "T2" in (view.get("evolution", {}).get("tiers") or []),
    })
elif not t2:
    checks.update({
        "genesis_body_is_prebuilt": genesis_bodies == [body["sha256"]],
        "no_toolchain_recorded": tc == "None",
        "status_says_T0_with_rustup": "self-evolution T0 · " in status_txt and "sh.rustup.rs" in status_txt,
        "status_json_evolution": status.get("evolution", {}).get("tiers") == ["T0"],
        "published_view_evolution": view.get("evolution", {}).get("tiers") == ["T0"],
    })
else:
    # the promote moved the tuple, so the genesis body is one of the tuples' bodies, not the only one
    jp, rp = os.path.join(d, "promote-job.json"), os.path.join(d, "gate-report.json")
    job = json.load(open(jp)) if os.path.isfile(jp) else {}
    gate = json.load(open(rp)) if os.path.isfile(rp) else {}
    stages = {s.get("stage"): s for s in job.get("stages", [])}
    checks.update({
        "genesis_body_is_prebuilt": body["sha256"] in genesis_bodies,
        "toolchain_in_test_home": tc.startswith(os.path.join(home, ".rustup") + "/"),
        "status_json_evolution_t2": "T2" in (status.get("evolution", {}).get("tiers") or []),
        "promote_started_by_agent": prc == "0",
        "gate_g0_to_g3_pass": all(stages.get(g, {}).get("pass") is True for g in ("G0", "G1", "G2", "G3")),
        "promote_job_succeeded": job.get("state") == "succeeded",
    })
v = {"v": 1, "bundle": os.path.basename(tgz), "rust": "none (--no-rust)" if no_rust else "installed by get.sh into the test home", "toolchain": tc, "bundle_bytes": os.path.getsize(tgz), "body_sha256": body["sha256"],
     "times_ms": {"get_sh_install": int(get_ms), "start_to_ready": int(ready_ms), "get_sh_to_first_reply": int(first_ms),
                  "drive_wall": drive.get("wall_ms")},
     "mock_requests": mock_reqs, "reply": drive["reply"], "checks": checks, "ok": all(checks.values())}
if mode != "hello":
    v["mode"] = mode
    v.update(extra)
if t2:
    v["times_ms"]["promote_wait"] = int(promote_ms)
    v["promote"] = {"job": job.get("id"), "sha": job.get("sha"), "state": job.get("state"), "error": job.get("error"),
                    "stages": [{k: s.get(k) for k in ("stage", "pass", "wall_ms")} for s in job.get("stages", [])]}
    if gate.get("failure"):
        v["promote"]["failure_head"] = gate["failure"][:1500]
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
[ $VRC = 0 ] || die "checks failed ($VERDICT)"
log "PASS"
