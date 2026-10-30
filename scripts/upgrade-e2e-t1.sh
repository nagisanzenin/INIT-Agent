#!/bin/bash
# The T1 half of the upgrade end-to-end check (lane UPGRADE, v0.5.0), at $0: install a published bundle into a short
# temporary home, then have INIT keep a new Wasm extension (a T1 self-change) through the gate, on the mock model. After
# it, the Root's current tuple has a different `ext` set than the genesis tuple. The upgrade lane then installs a newer
# bundle over this home and checks that the evolved body and ext survive.
#
#   scripts/upgrade-e2e-t1.sh [--keep] [TARBALL] [DIR]
#       TARBALL  default target/dist/v0.4.1-preview/init-v0.4.1-preview-macos-arm64.tar.gz (its .sha256 next to it)
#       DIR      default /private/tmp/i5-upg-t1 (ROOT_HOME and INIT_HOME must stay at most 64 bytes)
#   Run it inside ci/slot.sh, after `df -g /` (21 GiB free at least; it stops when free space drops under
#   T1_DISK_MIN, default 20 GiB). The promote takes about 25-35 min (a cold body build in G0, G3's builds).
#
#   . scripts/upgrade-e2e-t1.sh      sourced: defines the functions and runs nothing
#   t1_install TARBALL DIR   mock + get.sh install + test key + `init-root start --foreground` as our child, then one
#                            owner line ("Say hello."). Leaves the Root and the mock running; exports ROOT_PID,
#                            MOCK_PID, RH, IH, H, BIN, PORT, T1_DIR, T1_TC (the recorded toolchain) and T1_MOCK_SCRIPT.
#   t1_change DIR            one owner line; the mock agent, through its bash tool: scaffolds ext/upgrade-probe from
#                            skills/ext-authoring's template, builds the ext workspace for wasm32-wasip2, refreshes
#                            self.json (`init-kernel selfmap --write --no-build`), commits on evo/upgrade-probe as the
#                            agent identity and runs `initctl promote evo/upgrade-probe`. The harness waits for the
#                            promote job and the G4 job that follows it, then checks that the Root's current tuple
#                            names upgrade-probe and differs in `ext` from genesis. Prints the job id, stage times and
#                            the new tuple id; exports T1_JOB, T1_G4_JOB, T1_TUPLE, T1_GENESIS_TUPLE.
#   t1_stop DIR              `init-root stop`, SIGTERM to ROOT_PID (SIGKILL after 30 s), SIGTERM to MOCK_PID.
#   t1_owner_check DIR       the owner's state (snapshot taken by t1_install) must be unchanged.
#
# The mock script is "$T1_MOCK_SCRIPT" ($DIR/t1.mock.jsonl). The mock reads it once, at start, and keeps one cursor per
# (session, purpose), so the turns of later steps must be in it before t1_install: set T1_MOCK_EXTRA=<file> and its
# lines are appended after the T1 turns (a session that continues gets them next; a new session starts again at the
# hello turn). Rust: no download. The owner's rustup (~/.cargo/bin, toolchain 1.97.1 with
# wasm32-wasip2 and clippy) is put on PATH for get.sh --no-rust, so install.sh records that toolchain (T0+T1+T2).
# Main mode: both steps, t1_stop, the owner check, then the verdict $DIR/t1.json (copied to $DIR.json when $DIR is
# removed). Signals go only to the PIDs this script started. $DIR is removed by exact path unless --keep. Sourcing sets
# `set -euo pipefail` and umask 077 in the caller's shell; the functions return 1 on failure (they never exit).
set -euo pipefail
umask 077

T1_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
T1_EXT=upgrade-probe
T1_BRANCH=evo/upgrade-probe
T1_RUST=1.97.1
T1_DISK_MIN="${T1_DISK_MIN:-20}"
T1_PW_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)')"
ROOT_PID="${ROOT_PID:-}"; MOCK_PID="${MOCK_PID:-}"

t1_log() { printf '[t1 %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
t1_die() { t1_log "FAIL: $*"; return 1; }
t1_ms() { /usr/bin/python3 -I -c 'import time;print(int(time.time()*1000))'; }
t1_free_gib() { df -g / | awk 'NR==2{print $4}'; }
t1_renv() { env -i HOME="$T1_PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$@"; }
t1_paths() { # DIR → the layout
  T1_DIR="$1"; H="$T1_DIR/h"; RH="$T1_DIR/r"; IH="$T1_DIR/i"; BIN="$H/.local/bin"; T1_MOCK_SCRIPT="$T1_DIR/t1.mock.jsonl"
  export T1_DIR H RH IH BIN T1_MOCK_SCRIPT
}

# The owner's state, metadata only (scripts/bundle-e2e.sh snap()): names, sizes, mtimes, modes; names and modes only
# where an idle, running owner install writes on timers; ~/.init-root/sealed is not listed.
t1_snap() {
  /usr/bin/python3 -I - "$T1_PW_HOME" <<'PY'
import hashlib, os, sys
h = sys.argv[1]
out = []
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

# One owner line through `init-agent --raw`, frames until idle (or LIMIT s). Prints {rc, idle, reply, wall_ms}.
t1_write_driver() {
  cat >"$T1_DIR/drive.py" <<'PY'
import json, queue, subprocess, sys, threading, time
init, home, out, say, want, limit = sys.argv[1:7]
limit = float(limit)
# cwd: the test home, so the session's workspace (the Root's /undo snapshot, the status bar) is never the checkout
# this harness runs from (REL-050: a recording showed the repository's path, with the account name, in the status bar)
p = subprocess.Popen([init, "--raw", "--no-replay"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                     env={"HOME": home, "PATH": "/usr/bin:/bin"}, cwd=home, text=True, bufsize=1)
q = queue.Queue()
threading.Thread(target=lambda: ([q.put(l.rstrip("\n")) for l in p.stdout], q.put(None)), daemon=True).start()
t0 = time.monotonic()
time.sleep(0.3)
p.stdin.write(say + "\n")
p.stdin.flush()
texts, busy, idle = [], False, False
with open(out, "w") as f:
    while time.monotonic() - t0 < limit:
        try:
            line = q.get(timeout=0.5)
        except queue.Empty:
            continue
        if line is None:
            break
        f.write(line + "\n")
        f.flush()
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
rc = 0 if idle and want.lower() in reply.lower() else (1 if idle else 2)
p.stdin.close()
try:
    p.wait(timeout=3)
except subprocess.TimeoutExpired:
    p.terminate()  # our own child
    p.wait()
print(json.dumps({"rc": rc, "idle": idle, "reply": reply[-200:], "wall_ms": int((time.monotonic() - t0) * 1000)}))
sys.exit(rc)
PY
}

# The mock script: the hello turn, then the T1 turns (each a bash call of the mock agent), then T1_MOCK_EXTRA's lines.
t1_write_mock_script() {
  /usr/bin/python3 -I - "$T1_MOCK_SCRIPT" "$T1_EXT" "$T1_BRANCH" <<'PY'
import json, sys
path, ext, branch = sys.argv[1:4]
crate = ext.replace("-", "_")
def turn(reply, expect=None, n=0):
    t = {"v": 1, "kind": "turn", "reply": reply}
    if expect:
        t["expect"] = expect
    reply.setdefault("usage", {"input": 12000 + 200 * n, "cache_read": 11776 if n else 0, "output": 9})
    return t
def bash(cmd, timeout=None):
    i = {"command": cmd}
    if timeout:
        i["timeout"] = timeout
    return {"calls": [{"name": "bash", "input": i}], "stop": "tool_use"}
# The agent's working directory is the task workspace; every step uses $INIT_HOME and its own CARGO_TARGET_DIR default.
TD = '${CARGO_TARGET_DIR:-$INIT_HOME/work/target}'
scaffold = f"""set -eu
cd "$INIT_HOME/body"
git switch -q -c {branch}
cp -R skills/ext-authoring/assets/template ext/{ext}
mv ext/{ext}/Cargo.toml.template ext/{ext}/Cargo.toml
sed -i.orig 's/"count-lines"/"{ext}"/' ext/{ext}/Cargo.toml ext/{ext}/src/lib.rs
sed -i.orig 's/^const DESCRIPTION: &str = "Count the lines/const DESCRIPTION: \\&str = "Upgrade probe, a kept T1 extension: count the lines/' ext/{ext}/src/lib.rs
rm -f ext/{ext}/Cargo.toml.orig ext/{ext}/src/lib.rs.orig
grep -n -e '{ext}' -e 'Upgrade probe' ext/{ext}/Cargo.toml ext/{ext}/src/lib.rs"""
build = f"""set -eu -o pipefail
export CARGO_TARGET_DIR="{TD}"
cd "$INIT_HOME/body/ext"
command -v cargo
cargo build --offline --target wasm32-wasip2 2>&1 | tail -n 5
ls -l "$CARGO_TARGET_DIR/wasm32-wasip2/debug/{crate}.wasm" "$CARGO_TARGET_DIR/wasm32-wasip2/debug/count_lines.wasm\""""
selfmap = f"""set -eu
export CARGO_TARGET_DIR="{TD}"
"$INIT_HOME/kernel/bin/init-kernel" selfmap --root "$INIT_HOME/body" --write --no-build --binary "$INIT_HOME/bin/init-body" --components "$CARGO_TARGET_DIR/wasm32-wasip2/debug"
git -C "$INIT_HOME/body" status --short"""
msg = f"""feat(ext): add the {ext} extension

A kept T1 extension that the upgrade check expects to survive an upgrade.

Why: the v0.5.0 upgrade check needs a kept T1 extension to carry across an upgrade.
Predicts: ext_calls:{ext} >= 0
Compensates-for: none (new capability)
Prefix-change: no"""
commit = f"""set -eu
cd "$INIT_HOME/body"
git add ext/{ext} ext/Cargo.lock self.json
git -c user.name='INIT Agent' -c user.email=agent@init.local -c commit.gpgsign=false commit -q -F - <<'MSG'
{msg}
MSG
git log -1 --format='%H %an <%ae> %s'
git status --short"""
lines = [
    {"v": 1, "kind": "meta", "provider": "deepseek", "model": "deepseek-flash", "name": "upgrade-e2e-t1", "chunk": 16},
    turn({"text": "hello from the mock", "stop": "end_turn"}, {"last_role": "user"}),
    turn(bash(scaffold), {"last_role": "user", "last_text_contains": f"Keep a new extension {ext}"}, 1),
    turn(bash(build, 1800), n=2),
    turn(bash(selfmap, 600), n=3),
    turn(bash(commit), n=4),
    turn(bash(f"initctl promote {branch}"), n=5),
    turn({"text": "T1 promote submitted", "stop": "end_turn"}, n=6),
]
with open(path, "w") as f:
    for l in lines:
        f.write(json.dumps(l) + "\n")
PY
  if [ -n "${T1_MOCK_EXTRA:-}" ]; then
    [ -f "$T1_MOCK_EXTRA" ] || t1_die "T1_MOCK_EXTRA=$T1_MOCK_EXTRA is not a file" || return 1
    cat "$T1_MOCK_EXTRA" >>"$T1_MOCK_SCRIPT"
    t1_log "mock script: $(wc -l <"$T1_MOCK_EXTRA" | tr -d ' ') extra line(s) from $T1_MOCK_EXTRA"
  fi
}

t1_install() { # TARBALL DIR
  local tgz="$1" name getsh t0 grc ready
  t1_paths "$2"
  [ -f "$tgz" ] && [ -f "$tgz.sha256" ] || { t1_die "no bundle $tgz (with its .sha256)"; return 1; }
  [ ${#RH} -le 64 ] && [ ${#IH} -le 64 ] || { t1_die "test homes exceed 64 bytes ($RH)"; return 1; }
  [ "$(t1_free_gib)" -ge 21 ] || { t1_die "$(t1_free_gib) GiB free on /, below 21 GiB"; return 1; }
  [ -x "$T1_PW_HOME/.cargo/bin/rustup" ] || { t1_die "no rustup at $T1_PW_HOME/.cargo/bin"; return 1; }
  name="$(basename "$tgz" .tar.gz)"
  if [ -e "$T1_DIR" ]; then chmod -R u+w "$T1_DIR"; rm -rf -- "$T1_DIR"; fi
  mkdir -p "$H" "$T1_DIR/mock" "$T1_DIR/tmp"
  t1_snap >"$T1_DIR/owner-before.txt"
  t1_write_driver

  # 1. the $0 mock upstream, from the bundle itself
  /usr/bin/tar -xzf "$tgz" -C "$T1_DIR" "$name/init-mockllm" "$name/scripts/get.sh"
  mv "$T1_DIR/$name/init-mockllm" "$T1_DIR/init-mockllm"
  getsh="$(dirname "$tgz")/get.sh"
  if [ -f "$getsh" ]; then cmp -s "$getsh" "$T1_DIR/$name/scripts/get.sh" || t1_log "note: $getsh differs from the bundle's own get.sh (using the one next to the tarball)"
  else getsh="$T1_DIR/get.sh"; mv "$T1_DIR/$name/scripts/get.sh" "$getsh"; fi
  rm -rf -- "${T1_DIR:?}/$name"
  t1_write_mock_script
  "$T1_DIR/init-mockllm" --mode upstream --listen 127.0.0.1:0 --strict deepseek --port-file "$T1_DIR/mock.port" \
    --script "$T1_MOCK_SCRIPT" --record-dir "$T1_DIR/mock" >"$T1_DIR/mock.out" 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 100); do [ -s "$T1_DIR/mock.port" ] && break; sleep 0.1; done
  PORT="$(tr -d '\n' <"$T1_DIR/mock.port" 2>/dev/null || true)"
  [ -n "$PORT" ] || { cat "$T1_DIR/mock.out" >&2; t1_die "the mock did not start"; return 1; }
  export MOCK_PID PORT
  t1_log "mock upstream on 127.0.0.1:$PORT (pid $MOCK_PID); script $T1_MOCK_SCRIPT"

  # 2. get.sh --no-rust with the owner's rustup on PATH (toolchain 1.97.1 pinned for this run): no download
  t0=$(t1_ms)
  set +e
  env -i HOME="$H" PATH="$T1_PW_HOME/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin" TMPDIR="$T1_DIR/tmp/" LANG=en_US.UTF-8 \
    INIT_ROOT_HOME="$RH" INIT_HOME="$IH" INIT_BIN_DIR="$BIN" \
    RUSTUP_HOME="$T1_PW_HOME/.rustup" CARGO_HOME="$T1_PW_HOME/.cargo" RUSTUP_TOOLCHAIN="$T1_RUST" \
    /bin/sh "$getsh" --from "$tgz" --no-rust $(grep -q -- --no-modify-path "$getsh" && echo --no-modify-path) -- --no-service --disk-floor 5 --mock-upstream "127.0.0.1:$PORT" \
    >"$T1_DIR/get.log" 2>&1 </dev/null
  grc=$?
  set -e
  T1_GET_MS=$(( $(t1_ms) - t0 ))
  t1_log "get.sh exit $grc in $T1_GET_MS ms"
  [ $grc = 0 ] || { tail -40 "$T1_DIR/get.log" >&2; t1_die "get.sh"; return 1; }
  [ -x "$BIN/init-agent" ] || { t1_die "no $BIN/init-agent"; return 1; }
  # the installed release's own wording: "hash-equal to the maintainer's build" from v0.5.3, "verified bit for bit" before
  grep -qE "installed \(prebuilt body, (hash-equal to the maintainer's build|verified bit for bit)\)" "$T1_DIR/get.log" \
    || { t1_die "the install did not use the prebuilt body"; return 1; }
  T1_TC="$(/usr/bin/python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1])).get("toolchain") or "")' "$RH/install.json")"
  export T1_TC
  case "$T1_TC" in "$T1_PW_HOME/.rustup/toolchains/$T1_RUST-"*) ;; *) t1_die "recorded toolchain '$T1_TC' is not rustup's $T1_RUST"; return 1 ;; esac

  # 3. a test key (the harness's), then the Root as our child
  (umask 077; printf 'test-upg-t1-mock-key' >"$RH/keys/providers/deepseek")
  t0=$(t1_ms)
  # `env` execs init-root, so $! is the Root's own pid (a backgrounded shell function would give a subshell's)
  env -i HOME="$T1_PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$RH/bin/init-root" start --foreground >"$T1_DIR/root.out" 2>&1 </dev/null &
  ROOT_PID=$!
  export ROOT_PID
  ready=0
  for _ in $(seq 1 240); do
    if t1_renv "$RH/bin/init-root" status --json 2>/dev/null \
         | /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("slot")=="running" else 1)' 2>/dev/null \
       && [ -f "$IH/state/run/ready" ]; then ready=1; break; fi
    sleep 0.5
  done
  [ $ready = 1 ] || { tail -20 "$RH"/logs/*.stderr 2>/dev/null >&2; t1_die "kernel+body not ready in 120 s"; return 1; }
  T1_READY_MS=$(( $(t1_ms) - t0 ))
  t1_renv "$RH/bin/init-root" status >"$T1_DIR/status0.txt" 2>&1 || true
  grep -q 'self-evolution T0+T1+T2 ' "$T1_DIR/status0.txt" || { cat "$T1_DIR/status0.txt" >&2; t1_die "init-root status does not report self-evolution T0+T1+T2"; return 1; }
  t1_log "init-root start (pid $ROOT_PID): ready after $T1_READY_MS ms; self-evolution T0+T1+T2 · toolchain $T1_TC"

  # 4. one owner line: the mock's hello
  /usr/bin/python3 -I "$T1_DIR/drive.py" "$BIN/init-agent" "$H" "$T1_DIR/attach0.jsonl" "Say hello." "hello from the mock" 120 \
    >"$T1_DIR/drive0.json" || { cat "$T1_DIR/drive0.json" >&2; t1_die "the hello turn"; return 1; }
  t1_log "hello: $(cat "$T1_DIR/drive0.json")"
  export T1_GET_MS T1_READY_MS
}

# The tool results the mock saw (the agent's bash output), newest last; for diagnostics.
t1_tool_outputs() {
  /usr/bin/python3 -I - "$T1_DIR/mock" <<'PY'
import json, os, sys
d = sys.argv[1]
seen = []
for f in sorted(os.listdir(d), key=lambda n: (len(n), n)):
    if not f.endswith(".json"):
        continue
    try:
        rec = json.load(open(os.path.join(d, f)))
        msgs = rec.get("body", rec).get("messages", [])
    except (ValueError, AttributeError, OSError):
        continue
    tools = [m.get("content") for m in msgs if m.get("role") == "tool"]
    if tools:
        seen = tools
for i, t in enumerate(seen):
    print(f"--- tool result {i + 1}:\n{str(t)[:1500]}")
PY
}

# Folds the Root's lineage (tuple.rs Ptrs::apply): genesis, current, lkg tuple ids, and the current/genesis ext sets.
t1_tuples() {
  /usr/bin/python3 -I - "$RH" <<'PY'
import json, os, sys
rh = sys.argv[1]
cur = genesis = lkg = None
for l in open(os.path.join(rh, "lineage/lineage.jsonl")):
    r = json.loads(l)
    d = r.get("data") or {}
    k = r.get("kind")
    if k == "tuple":
        cur = d.get("id"); genesis = genesis or cur
    elif k == "rollback":
        cur = d.get("to")
    elif k == "lkg":
        lkg = d.get("tuple")
def ext(i):
    if not i:
        return None
    t = json.load(open(os.path.join(rh, "store/tuples", i + ".json")))
    return {"set": t["ext"].get("set"), "components": sorted((c.get("name") or c.get("id") or json.dumps(c, sort_keys=True), c.get("sha256")) for c in t["ext"].get("components") or [])}
print(json.dumps({"genesis": genesis, "current": cur, "lkg": lkg, "genesis_ext": ext(genesis), "current_ext": ext(cur)}))
PY
}

t1_job_summary() { # JOBFILE
  /usr/bin/python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d.get("id"), d.get("cmd"), d.get("state"), [(s.get("stage"), s.get("pass"), s.get("wall_ms")) for s in d.get("stages", [])], (d.get("error") or ""))' "$1"
}
t1_job_state() { /usr/bin/python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1])).get("state") or "")' "$1" 2>/dev/null || true; }

t1_change() { # DIR
  t1_paths "$1"
  local body="$IH/body" t0 drc job="" g4="" st free n jobs before
  [ -n "$ROOT_PID" ] && kill -0 "$ROOT_PID" 2>/dev/null || { t1_die "the Root is not running (t1_install first)"; return 1; }
  [ "$(t1_free_gib)" -ge 21 ] || { t1_die "$(t1_free_gib) GiB free on /, below 21 GiB"; return 1; }
  before="$(ls "$IH"/state/kernel/jobs/j-*.json 2>/dev/null | wc -l | tr -d ' ')"
  t0=$(t1_ms)
  # The kernel socket refuses peers outside the body's zone (E_PEER): the live agent does the work and runs initctl.
  set +e
  /usr/bin/python3 -I "$T1_DIR/drive.py" "$BIN/init-agent" "$H" "$T1_DIR/attach1.jsonl" \
    "Keep a new extension $T1_EXT: scaffold it from the ext-authoring template, build it, refresh self.json, commit it on $T1_BRANCH and promote it." \
    "T1 promote submitted" 1800 >"$T1_DIR/drive1.json"
  drc=$?
  set -e
  T1_AGENT_MS=$(( $(t1_ms) - t0 ))
  t1_log "agent turn (scaffold, build, selfmap, commit, promote): $(cat "$T1_DIR/drive1.json")"
  t1_tool_outputs >"$T1_DIR/agent-tools.txt" 2>&1 || true
  [ $drc = 0 ] || { cat "$T1_DIR/agent-tools.txt" >&2; t1_die "the agent's T1 turn did not finish"; return 1; }
  # the candidate as the agent committed it
  T1_SHA="$(git -C "$body" rev-parse --verify -q "refs/heads/$T1_BRANCH" || true)"
  [ -n "$T1_SHA" ] || { cat "$T1_DIR/agent-tools.txt" >&2; t1_die "no branch $T1_BRANCH in $body"; return 1; }
  git -C "$body" log -1 --format='%an <%ae>%n%B' "$T1_SHA" >"$T1_DIR/candidate.txt"
  git -C "$body" diff --stat "$T1_SHA^" "$T1_SHA" >>"$T1_DIR/candidate.txt"
  t1_log "candidate $T1_BRANCH $T1_SHA: $(git -C "$body" diff --name-only "$T1_SHA^" "$T1_SHA" | tr '\n' ' ')"
  for _ in $(seq 1 60); do # a refused promote (validator, E_*) makes no job
    n="$(ls "$IH"/state/kernel/jobs/j-*.json 2>/dev/null | wc -l | tr -d ' ')"
    [ "$n" -gt "$before" ] && break
    sleep 1
  done
  for j in "$IH"/state/kernel/jobs/j-*.json; do
    [ -f "$j" ] || continue
    /usr/bin/python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get("cmd")=="promote" and sys.argv[2] in json.dumps(d.get("args")) else 1)' "$j" "$T1_BRANCH" 2>/dev/null && job="$j"
  done
  [ -n "$job" ] || { cat "$T1_DIR/agent-tools.txt" >&2; t1_die "no promote job for $T1_SHA (the agent's initctl output is above)"; return 1; }
  T1_JOB="$(basename "$job" .json)"
  t1_log "promote job $T1_JOB (cold body build in G0, then G1-G3; about 25-35 min)"
  T1_PROMOTE_T0=$(t1_ms)
  st=""
  for i in $(seq 1 2700); do # 90 min
    st="$(t1_job_state "$job")"
    case "$st" in succeeded|failed|cancelled) break ;; esac
    if [ $(( i % 30 )) = 0 ]; then
      free="$(t1_free_gib)"
      t1_log "  $T1_JOB $st after $(( ($(t1_ms) - T1_PROMOTE_T0) / 1000 )) s; $free GiB free; $(/usr/bin/python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print([(s.get("stage"), s.get("pass"), s.get("wall_ms")) for s in d.get("stages", [])], (d.get("progress") or {}).get("msg", ""))' "$job" 2>/dev/null || true)"
      [ "$free" -ge "$T1_DISK_MIN" ] || { t1_die "free disk $free GiB is below $T1_DISK_MIN GiB: stopping"; return 1; }
    fi
    sleep 2
  done
  T1_PROMOTE_MS=$(( $(t1_ms) - T1_PROMOTE_T0 ))
  cp "$job" "$T1_DIR/promote-job.json"
  [ -f "$IH/state/kernel/gate/$T1_JOB/report.json" ] && cp "$IH/state/kernel/gate/$T1_JOB/report.json" "$T1_DIR/gate-report.json"
  t1_log "promote job after $(( T1_PROMOTE_MS / 1000 )) s: $(t1_job_summary "$job")"
  [ "$st" = succeeded ] || { [ -f "$T1_DIR/gate-report.json" ] && head -c 3000 "$T1_DIR/gate-report.json" >&2; t1_die "promote job $T1_JOB $st"; return 1; }
  # The asynchronous G4 (no sealed S and no pool on a fresh install: accept no_instrument), then LKG and the Root's tuple.
  t0=$(t1_ms)
  for i in $(seq 1 900); do # 30 min
    g4=""
    for j in "$IH"/state/kernel/jobs/j-*.json; do
      [ -f "$j" ] || continue
      /usr/bin/python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get("cmd")=="g4" and sys.argv[2] in json.dumps(d.get("args")) else 1)' "$j" "$T1_SHA" 2>/dev/null && g4="$j"
    done
    if [ -n "$g4" ]; then st="$(t1_job_state "$g4")"; case "$st" in succeeded|failed|cancelled) break ;; esac; fi
    [ $(( i % 30 )) = 0 ] && t1_log "  waiting for G4 ($([ -n "$g4" ] && basename "$g4" .json || echo 'no job yet') ${st:-}) after $(( ($(t1_ms) - t0) / 1000 )) s"
    sleep 2
  done
  T1_G4_MS=$(( $(t1_ms) - t0 ))
  [ -n "$g4" ] || { t1_die "no G4 job for $T1_SHA after $(( T1_G4_MS / 1000 )) s"; return 1; }
  T1_G4_JOB="$(basename "$g4" .json)"
  cp "$g4" "$T1_DIR/g4-job.json"
  t1_log "G4 job after $(( T1_G4_MS / 1000 )) s: $(t1_job_summary "$g4")"
  [ "$st" = succeeded ] || { t1_die "G4 job $T1_G4_JOB $st"; return 1; }
  for _ in $(seq 1 60); do # the Root forms the tuple when the kernel's promotion{lkg:true} lands
    t1_tuples >"$T1_DIR/tuples.json"
    /usr/bin/python3 -I -c 'import json,sys; t=json.load(open(sys.argv[1])); sys.exit(0 if t["current"]!=t["genesis"] else 1)' "$T1_DIR/tuples.json" && break
    sleep 1
  done
  T1_TUPLE="$(/usr/bin/python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["current"])' "$T1_DIR/tuples.json")"
  T1_GENESIS_TUPLE="$(/usr/bin/python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["genesis"])' "$T1_DIR/tuples.json")"
  /usr/bin/python3 -I - "$T1_DIR/tuples.json" "$T1_EXT" <<'PY' || { cat "$T1_DIR/tuples.json" >&2; t1_die "the Root's current tuple does not carry $T1_EXT in a new ext set"; return 1; }
import json, sys
t = json.load(open(sys.argv[1])); ext = sys.argv[2]
ok = t["current"] != t["genesis"] and t["current_ext"] != t["genesis_ext"] \
     and any(n == ext for n, _ in t["current_ext"]["components"]) and not any(n == ext for n, _ in t["genesis_ext"]["components"])
sys.exit(0 if ok else 1)
PY
  export T1_SHA T1_JOB T1_G4_JOB T1_TUPLE T1_GENESIS_TUPLE T1_AGENT_MS T1_PROMOTE_MS T1_G4_MS
  echo "T1 job $T1_JOB succeeded; stages: $(/usr/bin/python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(" ".join("%s=%sms" % (s.get("stage"), s.get("wall_ms")) for s in d.get("stages", [])))' "$T1_DIR/promote-job.json"); G4 $T1_G4_JOB; tuple $T1_GENESIS_TUPLE -> $T1_TUPLE"
}

t1_stop() { # DIR
  t1_paths "$1"
  set +e
  if [ -n "$ROOT_PID" ] && kill -0 "$ROOT_PID" 2>/dev/null; then
    t1_renv "$RH/bin/init-root" stop >/dev/null 2>&1
    kill -TERM "$ROOT_PID" 2>/dev/null
    for _ in $(seq 1 120); do kill -0 "$ROOT_PID" 2>/dev/null || break; sleep 0.25; done
    kill -0 "$ROOT_PID" 2>/dev/null && { t1_log "our Root (pid $ROOT_PID) did not exit on SIGTERM: SIGKILL"; kill -KILL "$ROOT_PID" 2>/dev/null; }
    wait "$ROOT_PID" 2>/dev/null
  fi
  ROOT_PID=
  if [ -n "$MOCK_PID" ] && kill -0 "$MOCK_PID" 2>/dev/null; then kill -TERM "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null; fi
  MOCK_PID=
  set -e
}

t1_owner_check() { # DIR
  t1_paths "$1"
  t1_snap >"$T1_DIR/owner-after.txt"
  if ! diff -u "$T1_DIR/owner-before.txt" "$T1_DIR/owner-after.txt" >"$T1_DIR/owner.diff"; then
    head -20 "$T1_DIR/owner.diff" >&2; t1_die "the owner's state changed (see $T1_DIR/owner.diff)"; return 1
  fi
  t1_log "owner state unchanged: ~/.init, ~/.init-root (sealed/ unlisted), ~/.local/bin, ~/.local/share/init, ~/Library/LaunchAgents, shell rc files"
}

t1_main() {
  local keep=0 tgz="" dir="" a rc=0 phase=install
  for a in "$@"; do case "$a" in --keep) keep=1 ;; -*) echo "usage: $0 [--keep] [TARBALL] [DIR]" >&2; return 64 ;; *) if [ -z "$tgz" ]; then tgz="$a"; else dir="$a"; fi ;; esac; done
  tgz="${tgz:-$T1_REPO/target/dist/v0.4.1-preview/init-v0.4.1-preview-macos-arm64.tar.gz}"
  dir="${dir:-/private/tmp/i5-upg-t1}"
  t1_paths "$dir"
  T1_KEEP=$keep
  trap 't1_stop "$T1_DIR"; if [ "$T1_KEEP" = 0 ] && [ -e "$T1_DIR" ]; then [ -f "$T1_DIR/t1.json" ] && cp "$T1_DIR/t1.json" "$T1_DIR.json"; chmod -R u+w "$T1_DIR"; rm -rf -- "$T1_DIR"; fi' EXIT
  t1_install "$tgz" "$dir" || rc=$?
  if [ $rc = 0 ]; then phase=change; t1_change "$dir" || rc=$?; fi
  t1_renv "$RH/bin/init-root" status --json >"$T1_DIR/status.json" 2>/dev/null || true
  t1_stop "$dir"
  local owner=ok
  t1_owner_check "$dir" || owner=changed
  /usr/bin/python3 -I - "$T1_DIR" "$tgz" "$rc" "$phase" "$owner" "${T1_GET_MS:-0}" "${T1_READY_MS:-0}" "${T1_AGENT_MS:-0}" \
    "${T1_PROMOTE_MS:-0}" "${T1_G4_MS:-0}" "${T1_TC:-}" "${T1_SHA:-}" "$T1_EXT" <<'PY'
import json, os, sys
d, tgz, rc, phase, owner, get_ms, ready_ms, agent_ms, promote_ms, g4_ms, tc, sha, ext = sys.argv[1:14]
def load(n):
    p = os.path.join(d, n)
    try:
        return json.load(open(p))
    except (OSError, ValueError):
        return {}
job, g4, tuples = load("promote-job.json"), load("g4-job.json"), load("tuples.json")
gate = load("gate-report.json")
v = {"v": 1, "bundle": os.path.basename(tgz), "ok": rc == "0" and owner == "ok", "failed_in": None if rc == "0" else phase,
     "owner_state": owner, "toolchain": tc, "candidate": sha, "ext": ext,
     "times_ms": {"get_sh_install": int(get_ms), "start_to_ready": int(ready_ms), "agent_t1_turn": int(agent_ms),
                  "promote_wait": int(promote_ms), "g4_wait": int(g4_ms)},
     "promote": {"job": job.get("id"), "state": job.get("state"), "error": job.get("error"),
                 "stages": [{k: s.get(k) for k in ("stage", "pass", "wall_ms")} for s in job.get("stages", [])]},
     "g4": {"job": g4.get("id"), "state": g4.get("state"), "result": g4.get("result"), "error": g4.get("error")},
     "tuples": tuples}
if gate.get("failure"):
    v["promote"]["failure_head"] = gate["failure"][:1500]
json.dump(v, open(os.path.join(d, "t1.json"), "w"), indent=1, sort_keys=True)
print(json.dumps(v, sort_keys=True))
PY
  [ $rc = 0 ] && [ "$owner" = ok ] && t1_log "PASS" || t1_log "FAIL (rc $rc in $phase, owner state $owner; verdict $T1_DIR/t1.json)"
  [ $rc = 0 ] && [ "$owner" = ok ]
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then t1_main "$@"; fi
