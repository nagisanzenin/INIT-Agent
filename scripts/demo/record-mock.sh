#!/bin/bash
# The recording kit (TUI-OBSERVE, v0.5.0), at $0 on the mock model: install a bundle into a short temporary home, open
# the owner's TUI (`init-agent`) in a recorded terminal, and have INIT make two self-changes through the real kernel,
# Root and body: one the gates keep and one they refuse. The recording shows the gate track live and collapsed, the
# failed promote's output under the call, and the lineage panel in Ctrl-O with `verify ✓`. Every frame carries a caption
# row that says "mock model" (brand §09 rule 4: replays and simulations say so).
#
#   scripts/demo/record-mock.sh [--keep] TARBALL [DIR]
#       TARBALL  a bundle built from the tree under test (scripts/bundle.sh; its .sha256 next to it). A bundle older
#                than v0.5.0 has no notice data and no lineage view: the TUI then shows plain notices.
#       DIR      default /private/tmp/i5-rec (ROOT_HOME and INIT_HOME must stay at most 64 bytes)
#   Run it inside ci/slot.sh, after `df -g /` (21 GiB free at least). About 35-50 min: the kept change is a T1 change (a
#   new Wasm extension, word-count), whose G0 builds the body cold (25-35 min without Rust caches); the refused change
#   breaks word-count's source on purpose, so G0 refuses it once the build reaches it (a few minutes, warm).
#   A T2 change would need a second cold build of the body and is not used.
#
# Out: DIR/demo.cast (asciinema v2, 100x32, markers "kept change", "refused change", "lineage") and DIR/demo.gif when
# `agg` is on PATH (https://github.com/asciinema/agg; not installed by this script). Without --keep, DIR is removed
# and the cast is copied to DIR.cast (and DIR.gif). The verdict is DIR/rec.json (copied to DIR.json).
#
# Install, the mock and the T1 change come from scripts/upgrade-e2e-t1.sh (sourced: t1_install, t1_change, t1_stop,
# t1_owner_check), whose mock script this kit extends with the refused change's turns (T1_MOCK_EXTRA). Signals go
# only to the PIDs these scripts started. Nothing touches ~/.init, ~/.init-root or ~/.local/bin (t1_owner_check).
set -euo pipefail
umask 077
REC_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
[ -f "$REC_REPO/scripts/upgrade-e2e-t1.sh" ] || { echo "record-mock.sh: needs scripts/upgrade-e2e-t1.sh (lane UPGRADE)" >&2; exit 69; }
# shellcheck source=../upgrade-e2e-t1.sh
. "$REC_REPO/scripts/upgrade-e2e-t1.sh"
T1_EXT=word-count
T1_BRANCH=evo/word-count
REC_BROKEN=evo/broken-word-count
REC_CAPTION='■□□□ INIT · mock model (init-mockllm, $0) · scripted run on the real kernel, Root and body'
REC_PID=""

rec_log() { printf '[rec %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
rec_ctl() { printf '%s\n' "$*" >"$T1_DIR/rec.ctl"; }

# The refused change's turns, after the T1 turns in the same session: break word-count, commit, promote and wait.
rec_mock_extra() { # FILE
  /usr/bin/python3 -I - "$1" "$T1_EXT" "$REC_BROKEN" <<'PY'
import json, sys
path, ext, branch = sys.argv[1:4]
def turn(reply, expect=None, n=0):
    t = {"v": 1, "kind": "turn", "reply": reply}
    if expect:
        t["expect"] = expect
    reply.setdefault("usage", {"input": 14000 + 200 * n, "cache_read": 13800, "output": 9})
    return t
def bash(cmd, timeout=None):
    i = {"command": cmd}
    if timeout:
        i["timeout"] = timeout
    return {"calls": [{"name": "bash", "input": i}], "stop": "tool_use"}
msg = f"""feat(ext): {ext} counts words faster

A deliberately broken change for the recording: it does not compile.

Why: the recording needs a change the gates refuse, whatever the agent says.
Predicts: ext_calls:{ext} >= 0
Compensates-for: none (new capability)
Prefix-change: no"""
broken = f"""set -eu
cd "$INIT_HOME/body"
git switch -q -c {branch}
printf '\\nfn broken( {{\\n' >>ext/{ext}/src/lib.rs
git add ext/{ext}/src/lib.rs
git -c user.name='INIT Agent' -c user.email=agent@init.local -c commit.gpgsign=false commit -q -F - <<'MSG'
{msg}
MSG
git log -1 --format='%h %s'"""
lines = [
    # last_role only: after the kept change's reload the body appends its `[init:reload]` note after the owner's line,
    # so the owner's words are no longer the last message's text (REL-050: the mock refused the request on that)
    turn(bash(broken), {"last_role": "user"}, 1),
    turn(bash(f"initctl promote {branch} --wait", 1800), n=2),
    turn({"text": "The gate refused it, so nothing changed: the running version still has the kept word-count.", "stop": "end_turn"}, n=3),
]
with open(path, "w") as f:
    for l in lines:
        f.write(json.dumps(l) + "\n")
PY
}

rec_start() { # the recorded terminal: init-agent (init-tui from the bundle) in a 100x32 pty
  rm -f "$T1_DIR/rec.ctl"
  mkfifo "$T1_DIR/rec.ctl"
  (cd "$H" && exec /usr/bin/python3 -I "$REC_REPO/scripts/demo/rec.py" --out "$T1_DIR/demo.cast" --cols 100 --rows 32 \
     --caption "$REC_CAPTION" --ctl "$T1_DIR/rec.ctl" --env "HOME=$H" --env "COLORFGBG=15;0" -- "$BIN/init-agent") \
     >"$T1_DIR/rec.out" 2>&1 &
  REC_PID=$!
  sleep 4
  kill -0 "$REC_PID" 2>/dev/null || { cat "$T1_DIR/rec.out" >&2; t1_die "the recorder did not start"; return 1; }
  rec_log "recording $T1_DIR/demo.cast (pid $REC_PID)"
}

rec_stop() {
  [ -n "$REC_PID" ] || return 0
  if kill -0 "$REC_PID" 2>/dev/null; then
    rec_ctl end || true
    for _ in $(seq 1 60); do kill -0 "$REC_PID" 2>/dev/null || break; sleep 0.25; done
    kill -0 "$REC_PID" 2>/dev/null && kill -TERM "$REC_PID" 2>/dev/null # our own recorder, by its pid
  fi
  wait "$REC_PID" 2>/dev/null || true
  REC_PID=""
}

rec_refused() { # the second owner line; waits for the agent's reply (its promote --wait returns when G0 refuses)
  local t0 drc
  t0=$(t1_ms)
  set +e
  /usr/bin/python3 -I "$T1_DIR/drive.py" "$BIN/init-agent" "$H" "$T1_DIR/attach2.jsonl" \
    "Try a broken change on purpose: break $T1_EXT so it does not compile, commit it on $REC_BROKEN and promote it." \
    "nothing changed" 1800 >"$T1_DIR/drive2.json"
  drc=$?
  set -e
  REC_REFUSED_MS=$(( $(t1_ms) - t0 ))
  rec_log "refused change: $(cat "$T1_DIR/drive2.json") after $(( REC_REFUSED_MS / 1000 )) s"
  [ $drc = 0 ] || { t1_tool_outputs >&2 || true; t1_die "the refused change's turn did not finish"; return 1; }
}

# What the cast must show (text with escapes removed): the caption, both outcomes, the failed call's output, verify ✓.
rec_check() {
  /usr/bin/python3 -I - "$T1_DIR/demo.cast" "$T1_DIR/rec-check.json" <<'PY'
import json, re, sys
evs = [json.loads(l) for l in open(sys.argv[1])][1:]
out = [e for e in evs if e[1] == "o"]
text = re.sub(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[78]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)", "", "".join(e[2] for e in out))
want = {"caption on every frame": all("mock model" in e[2] for e in out), "gate track": "◇ self-change" in text,
        "kept": "✓ kept" in text, "refused": "✕ refused at G" in text, "failed call output": "│ " in text,
        "lineage verify": "verify ✓" in text, "markers": [e[2] for e in evs if e[1] == "m"]}
json.dump(want, open(sys.argv[2], "w"), indent=1)
print(json.dumps(want))
sys.exit(0 if all(v for k, v in want.items() if k != "markers") else 1)
PY
}

rec_main() {
  local keep=0 tgz="" dir="" a rc=0 phase=install owner=ok extra
  for a in "$@"; do case "$a" in --keep) keep=1 ;; -*) echo "usage: $0 [--keep] TARBALL [DIR]" >&2; return 64 ;; *) if [ -z "$tgz" ]; then tgz="$a"; else dir="$a"; fi ;; esac; done
  [ -n "$tgz" ] || { echo "usage: $0 [--keep] TARBALL [DIR]" >&2; return 64; }
  dir="${dir:-/private/tmp/i5-rec}"
  t1_paths "$dir"
  extra="$dir.mock-extra.jsonl"
  rec_mock_extra "$extra"
  export T1_MOCK_EXTRA="$extra"
  REC_KEEP=$keep
  trap 'rec_stop; t1_stop "$T1_DIR"; rm -f -- "$T1_DIR.mock-extra.jsonl"; if [ "$REC_KEEP" = 0 ] && [ -e "$T1_DIR" ]; then for f in demo.cast demo.gif rec.json; do [ -f "$T1_DIR/$f" ] && cp "$T1_DIR/$f" "$T1_DIR.${f#demo.}"; done; chmod -R u+w "$T1_DIR"; rm -rf -- "$T1_DIR"; fi' EXIT
  t1_install "$tgz" "$dir" || rc=$?
  if [ $rc = 0 ]; then phase=record; rec_start || rc=$?; fi
  if [ $rc = 0 ]; then phase=kept; rec_ctl "mark kept change"; t1_change "$dir" || rc=$?; sleep 3; fi
  if [ $rc = 0 ]; then phase=refused; rec_ctl "mark refused change"; rec_refused || rc=$?; sleep 3; fi
  if [ $rc = 0 ]; then
    phase=lineage
    rec_ctl "mark lineage"; rec_ctl "key ctrl-o"; sleep 8; rec_ctl "key q"; sleep 2
  fi
  rec_stop
  if [ $rc = 0 ]; then phase=check; rec_check || rc=$?; fi
  if [ -f "$T1_DIR/demo.cast" ] && command -v agg >/dev/null 2>&1; then
    agg --font-size 16 "$T1_DIR/demo.cast" "$T1_DIR/demo.gif" >/dev/null 2>&1 && rec_log "GIF $T1_DIR/demo.gif" || rec_log "agg failed (the cast is kept)"
  fi
  t1_stop "$dir"
  t1_owner_check "$dir" || owner=changed
  /usr/bin/python3 -I - "$rc" "$phase" "$owner" "$(basename "$tgz")" "$T1_DIR/rec-check.json" "$T1_DIR/rec.json" <<'PY'
import json, os, sys
rc, phase, owner, bundle, check, out = sys.argv[1:7]
cast = json.load(open(check)) if os.path.exists(check) else {}
json.dump({"v": 1, "ok": rc == "0" and owner == "ok", "failed_in": None if rc == "0" else phase, "owner_state": owner,
           "bundle": bundle, "kept": "T1: a new Wasm extension, word-count", "refused": "word-count broken on purpose (G0)",
           "cast": cast}, open(out, "w"), indent=1)
PY
  [ $rc = 0 ] && [ "$owner" = ok ] && rec_log "PASS: $T1_DIR/demo.cast" || rec_log "FAIL (rc $rc in $phase, owner state $owner)"
  [ $rc = 0 ] && [ "$owner" = ok ]
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then rec_main "$@"; fi
