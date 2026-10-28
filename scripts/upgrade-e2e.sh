#!/bin/bash
# The upgrade end-to-end check (lane UPGRADE, v0.5.0; docs/guide.md "Upgrade"), at $0 on the mock model. An install that
# INIT has changed itself (a kept T1 extension, through the gate) is upgraded to NEW, and then:
#   1. the upgrade keeps the body, its extensions, the keys, the charter, the sessions and the lineage (whose old bytes
#      stay a prefix of the new file); `init-root audit verify --full` passes before and after; the new Root, kernel and
#      launchers are installed, INIT's old `init` is gone, and the upgraded install answers a turn;
#   2. inside the agent's sandbox (the mock agent's bash tool), in full access and in workspace mode, nothing can write
#      the Root's binary, the kernel's binaries, INIT's launchers, the upgrade's staging or journal, or run the upgrade
#      (the launcher's `upgrade`, or a release's `init-root upgrade` with this ROOT_HOME): every attempt fails and those
#      files keep their sha256;
#   3. a deliberately broken bundle (NEW with an init-kernel that exits 1) fails the health probe and is rolled back:
#      the Root binary, the kernel, the launchers and the current tuple are the ones from before, a `rollback` record
#      names the upgrade, the lineage verifies, and the old install still answers a turn;
#   4. a bundle whose kernel expects another body protocol (NEW whose reference body says stdio-jsonl/2) is refused, and
#      nothing changes (the lineage file is byte for byte the same).
#
#   scripts/upgrade-e2e.sh [--keep] [--home DIR] [--base BASE] [--old OLD_TARBALL] --bundle NEW_TARBALL
#       DIR    the old install (default /private/tmp/i5-upg-t1; ROOT_HOME and INIT_HOME must stay at most 64 bytes), as
#              scripts/upgrade-e2e-t1.sh makes it: DIR/{h,r,i}, its mock port in DIR/mock.port. When DIR is missing it
#              is made first: OLD_TARBALL (default the published v0.4.1-preview macOS bundle) installed and changed by T1
#              (about 26 min). BASE (default DIR.base when it exists): a copy of DIR (APFS clone) that DIR is reset from
#              before the run, so a run can be repeated. DIR is removed afterwards unless --keep; BASE is never touched.
#       NEW    the release to upgrade to (its .sha256 next to it, which this harness passes as --sha256: an upgrade
#              trusts no .sha256 beside a local bundle): at integration the 0.5.0 bundle from scripts/bundle.sh.
#   scripts/upgrade-e2e.sh --make-test-bundle OUT [--old OLD_TARBALL] [--root INIT_ROOT] [--label L]
#       a TEST bundle without a release build: OLD_TARBALL with INIT_ROOT (default root/target/debug/init-root, this
#       tree's Root) and this tree's scripts, a marker appended to init-kernel (a different binary that runs the same),
#       VERSION L (default <old version>-upgtest), MANIFEST.json marked `test_bundle`, SHA256SUMS regenerated. Prints the
#       path. Not a release: it proves the upgrade mechanics, not the new kernel.
#
# The broken and incompatible bundles are derived from NEW the same way (each with its own marker on init-kernel, so it
# is never "the release INIT runs"). Get.sh for the first upgrade is NEW's own (an install of 0.4.1 has no `init-agent
# upgrade`); the later ones go through the upgraded launcher (`init-agent upgrade --bundle`). Verdict:
# build/status/upgrade-e2e.json. The owner's ~/.init, ~/.init-root, ~/.local/bin, ~/.local/share/init and
# ~/Library/LaunchAgents are snapshotted before and after and must not change. Signals go only to the PIDs this script
# started. Needs 21 GiB free on / (the harness stops below it). Run it inside ci/slot.sh.
set -euo pipefail
umask 077

UPG_REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
# shellcheck source=scripts/upgrade-e2e-t1.sh
. "$UPG_REPO/scripts/upgrade-e2e-t1.sh"
KEEP=0; DIR=/private/tmp/i5-upg-t1; BASE=""; OLD="$UPG_REPO/target/dist/v0.4.1-preview/init-v0.4.1-preview-macos-arm64.tar.gz"
NEW=""; MAKE=""; ROOTBIN="$UPG_REPO/root/target/debug/init-root"; LABEL=""
while [ $# -gt 0 ]; do
  case "$1" in
    --keep) KEEP=1; shift ;;
    --home) DIR="$2"; shift 2 ;;
    --base) BASE="$2"; shift 2 ;;
    --old) OLD="$2"; shift 2 ;;
    --bundle) NEW="$2"; shift 2 ;;
    --make-test-bundle) MAKE="$2"; shift 2 ;;
    --root) ROOTBIN="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    -h|--help) sed -n '2,38p' "$0"; exit 0 ;;
    *) echo "upgrade-e2e.sh: unknown argument $1 (see --help)" >&2; exit 64 ;;
  esac
done
log() { printf '[upg %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { log "FAIL: $*"; exit 1; }
sha() { /usr/bin/shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1; }
case "$(uname -s)" in Darwin) ;; *) sha() { sha256sum "$1" 2>/dev/null | cut -d' ' -f1; } ;; esac

# ---------------------------------------------------------------- a bundle derived from another one
# mk_bundle SRC_TGZ OUTDIR LABEL [REL=FILE | REL=+MARKER | REL=-]...: SRC's tree, renamed to LABEL, with each REL
# replaced by FILE, given MARKER appended (a shell comment line), or removed; scripts/{install.sh,init-agent,
# uninstall.sh,get.sh} from this tree when SCRIPTS=1; VERSION, MANIFEST.json (version, commit, test_bundle, the files'
# hashes) and SHA256SUMS rewritten; packed like scripts/bundle.sh. Prints the tarball's path.
mk_bundle() {
  local src="$1" out="$2" label="$3" w top ver target name d rel what
  shift 3
  w="$(mktemp -d "$out/.mk.XXXXXX")"
  /usr/bin/tar -xzf "$src" -C "$w"
  top="$(ls "$w")"; ver="$(cat "$w/$top/VERSION")"; target="${top#init-v$ver-}"
  name="init-v$label-$target"; d="$w/$name"
  [ "$top" != "$name" ] && mv "$w/$top" "$d"
  chmod -R u+w "$d"
  for x in "$@"; do
    rel="${x%%=*}"; what="${x#*=}"
    case "$what" in
      -) rm -f -- "${d:?}/$rel" ;;
      +*) printf '\n# %s\n' "${what#+}" >>"$d/$rel" ;;
      *) cp "$what" "$d/$rel"; chmod 755 "$d/$rel" ;;
    esac
  done
  if [ "${SCRIPTS:-0}" = 1 ]; then
    for f in install.sh init-agent uninstall.sh get.sh; do cp "$UPG_REPO/scripts/$f" "$d/scripts/$f"; chmod 755 "$d/scripts/$f"; done
    rm -f -- "$d/scripts/init-shim" # bundle.sh no longer ships it
  fi
  printf '%s\n' "$label" >"$d/VERSION"
  /usr/bin/python3 -I - "$d" "$label" "$(git -C "$UPG_REPO" rev-parse HEAD)" "$src" <<'PY'
import hashlib, json, os, sys
d, label, head, src = sys.argv[1:5]
p = os.path.join(d, "MANIFEST.json")
m = json.load(open(p))
m["version"] = label
m["commit"] = head
m["test_bundle"] = f"scripts/upgrade-e2e.sh: derived from {os.path.basename(src)}, not a release build"
for f, e in m.get("files", {}).items():
    q = os.path.join(d, f)
    if os.path.isfile(q):
        e["sha256"] = hashlib.sha256(open(q, "rb").read()).hexdigest()
        e["size"] = os.path.getsize(q)
open(p, "w").write(json.dumps(m, indent=1, sort_keys=True) + "\n")
PY
  (cd "$d" && find . -type f ! -name SHA256SUMS | sed 's|^\./||' | LC_ALL=C sort | while IFS= read -r f; do /usr/bin/shasum -a 256 "$f"; done) >"$d/SHA256SUMS"
  (cd "$w" && COPYFILE_DISABLE=1 /usr/bin/tar --no-mac-metadata -czf "$out/$name.tar.gz" "$name")
  printf '%s  %s\n' "$(sha "$out/$name.tar.gz")" "$name.tar.gz" >"$out/$name.tar.gz.sha256"
  chmod -R u+w "$w"; rm -rf -- "$w"
  printf '%s\n' "$out/$name.tar.gz"
}

if [ -n "$MAKE" ]; then
  mkdir -p "$MAKE"; MAKE="$(cd "$MAKE" && pwd -P)"
  [ -x "$ROOTBIN" ] || die "no init-root at $ROOTBIN (build it: cd root && cargo build --offline -p init-root)"
  OV="$(/usr/bin/tar -xzOf "$OLD" "$(basename "$OLD" .tar.gz)/VERSION")"
  SCRIPTS=1 mk_bundle "$OLD" "$MAKE" "${LABEL:-$OV-upgtest}" "init-root=$ROOTBIN" "init-kernel=+upgrade-e2e test bundle: a different kernel binary"
  exit 0
fi
[ -n "$NEW" ] && [ -f "$NEW" ] && [ -f "$NEW.sha256" ] || die "--bundle NEW_TARBALL (with its .sha256) is required"
NEW="$(cd "$(dirname "$NEW")" && pwd -P)/$(basename "$NEW")"
# an upgrade from a local bundle takes its sha256 from the command line only (v0.5.1): the harness reads it here
NEW_SHA="$(cut -d' ' -f1 "$NEW.sha256")"
[ "$(t1_free_gib)" -ge 21 ] || die "$(t1_free_gib) GiB free on /, below 21 GiB"
[ -z "$BASE" ] && [ -d "$DIR.base" ] && BASE="$DIR.base"
t1_paths "$DIR"
VERDICT="$UPG_REPO/build/status/upgrade-e2e.json"
PIDS=(); ROOT_PID=""; MOCK_PID=""
cleanup() {
  set +e
  [ -z "$ROOT_PID" ] || { t1_renv "$RH/bin/init-root" stop >/dev/null 2>&1; kill -TERM "$ROOT_PID" 2>/dev/null; wait "$ROOT_PID" 2>/dev/null; }
  [ -z "$MOCK_PID" ] || { kill -TERM "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null; }
  if [ $KEEP = 0 ] && [ -e "$DIR" ] && [ -n "$BASE" ]; then chmod -R u+w "$DIR" 2>/dev/null; rm -rf -- "$DIR"; fi
}
trap cleanup EXIT

# ---------------------------------------------------------------- 0. the old install (made, or reset from BASE)
T_ALL=$(t1_ms)
if [ -n "$BASE" ]; then
  [ -d "$BASE" ] || die "--base $BASE is not a directory"
  if [ -e "$DIR" ]; then chmod -R u+w "$DIR"; rm -rf -- "$DIR"; fi
  cp -cRp "$BASE" "$DIR" 2>/dev/null || cp -Rp "$BASE" "$DIR" 2>/dev/null || true # sockets are not copied
  log "old install reset from $BASE"
elif [ ! -d "$DIR" ]; then
  log "no old install at $DIR: installing $(basename "$OLD") and making a T1 change (about 26 min)"
  t1_install "$OLD" "$DIR" || die "t1_install"
  t1_change "$DIR" || die "t1_change"
  t1_stop "$DIR" || die "t1_stop"
  ROOT_PID=""; MOCK_PID=""
fi
[ -f "$RH/install.json" ] || die "$DIR holds no install ($RH/install.json)"
# An old install reused from an earlier run must carry the kept T1 extension, or the "an evolved body survives the
# upgrade" check would pass on an unevolved one (REL-051: a run reused the DIR of a run whose T1 promote had failed).
/usr/bin/python3 -I - "$DIR/tuples.json" "$T1_EXT" <<'PY' || die "$DIR carries no kept T1 extension ($T1_EXT in the current tuple's ext set): its T1 change did not finish; remove $DIR and run again"
import json, sys
try:
    t = json.load(open(sys.argv[1]))
except (OSError, ValueError):
    sys.exit(1)  # t1_change writes tuples.json only after its promote; no file means no kept change
sys.exit(0 if any(n == sys.argv[2] for n, _ in (t.get("current_ext") or {}).get("components", [])) else 1)
PY
PORT="$(tr -d '\n' <"$DIR/mock.port")"
W="$DIR/upg"; mkdir -p "$W"
t1_snap >"$W/owner-before.txt"
lock_free() {
  /usr/bin/python3 -I -c 'import fcntl,os,sys
try: fd = os.open(sys.argv[1], os.O_RDONLY)
except FileNotFoundError: sys.exit(0)
try: fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError: sys.exit(1)
sys.exit(0)' "$RH/run/root.lock"
}
lock_free || die "a Root runs on $RH; stop it first"

# The state an upgrade must keep (sha256 of files; the lineage and the session logs as byte prefixes)
cat >"$W/kept.py" <<'PY'
import hashlib, json, os, subprocess, sys
rh, ih, out = sys.argv[1:4]
def h(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()
def tree(root, skip=()):
    r = {}
    for d, dirs, files in os.walk(root):
        dirs[:] = sorted(x for x in dirs if os.path.relpath(os.path.join(d, x), root) not in skip)
        for f in sorted(files):
            p = os.path.join(d, f)
            if os.path.isfile(p) and not os.path.islink(p):
                r[os.path.relpath(p, root)] = h(p)
    return r
git = lambda *a: subprocess.run(["/usr/bin/git", "-c", "core.fsmonitor=", "-c", "core.hooksPath=/dev/null", "-C", os.path.join(ih, "body"), *a],
                                capture_output=True, text=True).stdout.strip()
k = {
    "keys": tree(os.path.join(rh, "keys")),
    "charter": {f: h(os.path.join(rh, f)) for f in ("charter.toml", "state/charter.active.toml")},
    "body_bin": h(os.path.join(ih, "bin/init-body")),
    "ext_link": os.readlink(os.path.join(ih, "bin/ext")) if os.path.islink(os.path.join(ih, "bin/ext")) else None,
    "ext_sets": tree(os.path.join(ih, "bin/ext.d")),
    "body_head": git("rev-parse", "HEAD"), "body_tree": git("rev-parse", "HEAD^{tree}"),
    "body_branches": git("for-each-ref", "--format=%(refname) %(objectname)", "refs/heads"),
    "body_git": subprocess.run(["/usr/bin/git", "--git-dir", os.path.join(ih, "lineage/body.git"), "for-each-ref", "--format=%(refname) %(objectname)"],
                               capture_output=True, text=True).stdout.strip(),
    "kernel_config": h(os.path.join(ih, "kernel/config.toml")),
    "store_blobs": sorted(os.listdir(os.path.join(rh, "store/blobs"))),
}
sess = os.path.join(ih, "state/sessions")
prefix = {"lineage": open(os.path.join(rh, "lineage/lineage.jsonl"), "rb").read().decode()}
for f in sorted(os.listdir(sess)) if os.path.isdir(sess) else []:
    prefix["session:" + f] = open(os.path.join(sess, f), "rb").read().decode(errors="replace")
json.dump({"exact": k, "prefix": prefix}, open(out, "w"))
PY
kept() { /usr/bin/python3 -I "$W/kept.py" "$RH" "$IH" "$1"; }
kept_same() { # BEFORE AFTER: every exact item equal, every prefix item still a prefix; prints the differences
  /usr/bin/python3 -I - "$1" "$2" <<'PY'
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
bad = [k for k in a["exact"] if k not in ("store_blobs", "body_branches") and a["exact"][k] != b["exact"][k]]
# body/ is the agent's working repo: at boot the body syncs its branches to the authoritative lineage/body.git (body/boot.md
# gitsync), so a branch may move, but only to the lineage's commit of the same name
refs = lambda s: dict(l.split(" ", 1) for l in s.splitlines() if " " in l)
lin = {r.replace("refs/heads/", ""): v for r, v in refs(b["exact"]["body_git"]).items()}
ba, bb = refs(a["exact"]["body_branches"]), refs(b["exact"]["body_branches"])
bad += [f"body branch {r}" for r, v in ba.items() if bb.get(r) != v and bb.get(r) != lin.get(r.replace("refs/heads/", ""))]
bad += [f"store_blobs lost {sorted(set(a['exact']['store_blobs']) - set(b['exact']['store_blobs']))}"] if not set(a["exact"]["store_blobs"]) <= set(b["exact"]["store_blobs"]) else []
bad += [f"{k} is not a prefix" for k in a["prefix"] if not b["prefix"].get(k, "").startswith(a["prefix"][k])]
print(json.dumps(bad))
sys.exit(1 if bad else 0)
PY
}
tuples() { /usr/bin/python3 -I - "$RH" <<'PY'
import json, os, sys
rh = sys.argv[1]
ptr = {}
recs = [json.loads(l) for l in open(os.path.join(rh, "lineage/lineage.jsonl"))]
for r in recs:
    k, d = r["kind"], r["data"]
    if k == "tuple": ptr["current"] = d["id"]
    elif k == "rollback": ptr["current"] = d["to"]
    elif k == "lkg": ptr["lkg"] = d["tuple"]
t = json.load(open(os.path.join(rh, "store/tuples", ptr["current"] + ".json")))
print(json.dumps({"current": ptr["current"], "lkg": ptr.get("lkg"), "body": t["body"]["binary"], "ext": t["ext"], "kernel": t["kernel"]["init_kernel"],
                  "seq": recs[-1]["seq"], "kinds_tail": [r["kind"] for r in recs[-6:]]}))
PY
}
jq1() { /usr/bin/python3 -I -c 'import json,sys
v=json.load(open(sys.argv[1]))
for k in sys.argv[2].split("."): v = v.get(k) if isinstance(v, dict) else None
print("" if v is None else (json.dumps(v) if isinstance(v, (dict, list)) else v))' "$1" "$2"; }
verify() { t1_renv "$RH/bin/init-root" audit verify --full >"$W/verify-$1.txt" 2>&1; }
member() { /usr/bin/tar -xzOf "$1" "$(basename "$1" .tar.gz)/$2"; }
mk_get() { local g="$W/get-$(basename "$1" .tar.gz).sh"; member "$1" scripts/get.sh >"$g"; chmod 700 "$g"; printf '%s\n' "$g"; }
GET_ENV=(env -i HOME="$H" PATH=/usr/bin:/bin:/usr/sbin:/sbin TMPDIR="$DIR/tmp/" LANG=en_US.UTF-8 INIT_ROOT_HOME="$RH")

# the mock for the turns after the upgrade (a fresh mock: its cursor starts at the first turn of this script)
ATT="$IH/work/upg-attack.tgz"
cat >"$W/attack.sh" <<EOF
out="\$INIT_HOME/work/upg-attack-\$INIT_ACCESS.txt"; : >"\$out"
t() { n="\$1"; shift; ( "\$@" ) >/dev/null 2>"\$out.err"; rc=\$?; echo "\$n \$rc \$(tail -1 "\$out.err" | tr -s ' ' | cut -c1-120)" >>"\$out"; }
t write_root_bin sh -c 'printf x >>"$RH/bin/init-root"'
t write_kernel_bin sh -c 'printf x >>"$IH/kernel/bin/init-kernel"'
t new_kernel_file sh -c 'printf x >"$IH/kernel/bin/init-evil"'
t write_launcher sh -c 'printf x >>"$BIN/init-agent"'
t write_tui sh -c 'printf x >>"$BIN/init-tui"'
t create_init sh -c 'printf x >"$BIN/init"'
t rename_launcher mv "$BIN/init-agent" "$BIN/init-agent.moved"
t unlink_tui rm "$BIN/init-tui"
t stage_dir mkdir "$RH/cache/upgrade/evil"
t get_sh sh -c 'printf x >>"$RH/cache/get.sh"'
t journal sh -c 'printf x >"$RH/state/upgrade.json"'
t snapshot_dir mkdir "$RH/state/upgrade/evil"
t launcher_upgrade "$BIN/init-agent" upgrade --bundle "$ATT" --sha256 "$NEW_SHA"
t replace_step sh -c 'd=\$(mktemp -d "\$INIT_HOME/work/rel.XXXXXX") && tar -xzf "$ATT" -C "\$d" && r=\$(ls -d "\$d"/init-v*) && INIT_ROOT_HOME="$RH" "\$r/init-root" upgrade --release "\$r" --bin-dir "$BIN"'
echo end >>"\$out"
cat "\$out"
EOF
/usr/bin/python3 -I - "$W/upg.mock.jsonl" "$W/attack.sh" <<'PY'
import json, sys
out, attack = sys.argv[1:3]
cmd = open(attack).read()
u = lambda n: {"input": 12000 + 100 * n, "cache_read": 11776, "output": 9}
# hello; the sandbox check (a bash call, then its summary) in full access and in workspace mode; hello after the rollback.
# No text matching: the kernel may append pending notices (the T1 gate's) after the owner's line in the same message.
lines = [{"v": 1, "kind": "meta", "provider": "deepseek", "model": "deepseek-flash", "name": "upgrade-e2e", "chunk": 16},
    {"v": 1, "kind": "turn", "expect": {"last_role": "user"}, "reply": {"text": "hello from the mock", "stop": "end_turn", "usage": u(0)}},
    {"v": 1, "kind": "turn", "expect": {"last_role": "user"}, "reply": {"calls": [{"name": "bash", "input": {"command": cmd, "timeout": 300}}], "stop": "tool_use", "usage": u(1)}},
    {"v": 1, "kind": "turn", "reply": {"text": "sandbox check done", "stop": "end_turn", "usage": u(2)}},
    {"v": 1, "kind": "turn", "expect": {"last_role": "user"}, "reply": {"calls": [{"name": "bash", "input": {"command": cmd, "timeout": 300}}], "stop": "tool_use", "usage": u(3)}},
    {"v": 1, "kind": "turn", "reply": {"text": "sandbox check done", "stop": "end_turn", "usage": u(4)}},
    {"v": 1, "kind": "turn", "expect": {"last_role": "user"}, "reply": {"text": "hello from the mock", "stop": "end_turn", "usage": u(5)}},
]
open(out, "w").write("".join(json.dumps(l) + "\n" for l in lines))
PY
# drive.py with extra launcher arguments (--workspace) and a cwd
cat >"$W/drive.py" <<'PY'
import json, queue, subprocess, sys, threading, time
init, home, out, say, want, limit, cwd, *extra = sys.argv[1:]
p = subprocess.Popen([init, "--raw", "--no-replay", *extra], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                     env={"HOME": home, "PATH": "/usr/bin:/bin"}, text=True, bufsize=1, cwd=cwd)
q = queue.Queue()
threading.Thread(target=lambda: ([q.put(l.rstrip("\n")) for l in p.stdout], q.put(None)), daemon=True).start()
t0 = time.monotonic(); time.sleep(0.3)
p.stdin.write(say + "\n"); p.stdin.flush()
texts, busy, idle = [], False, False
with open(out, "w") as f:
    while time.monotonic() - t0 < float(limit):
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
        if v.get("t") == "display" and d.get("kind") in ("text", None) and isinstance(d.get("text"), str):
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
start_mock() {
  "$DIR/init-mockllm" --mode upstream --listen "127.0.0.1:$PORT" --strict deepseek --port-file "$W/mock.port" \
    --script "$W/upg.mock.jsonl" --record-dir "$W/mock" >>"$W/mock.out" 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 100); do [ -s "$W/mock.port" ] && break; sleep 0.1; done
  [ -s "$W/mock.port" ] || die "the mock did not start on port $PORT"
}
start_root() { # as our child; waits for the zone (slot running, the body's ready written after the start)
  local t0 m0; t0=$(t1_ms); m0="$(date +%s)"
  # `env` execs init-root, so $! is the Root's own pid (a backgrounded shell function would give a subshell's)
  env -i HOME="$T1_PW_HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin "$RH/bin/init-root" start --foreground >>"$W/root.out" 2>&1 </dev/null &
  ROOT_PID=$!
  for _ in $(seq 1 240); do
    if t1_renv "$RH/bin/init-root" status --json 2>/dev/null | /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("slot")=="running" else 1)' 2>/dev/null \
       && [ -f "$IH/state/run/ready" ] && [ "$(stat -f %m "$IH/state/run/ready" 2>/dev/null || stat -c %Y "$IH/state/run/ready")" -ge "$m0" ]; then
      log "Root (pid $ROOT_PID) ready after $(( $(t1_ms) - t0 )) ms"; return 0
    fi
    sleep 0.5
  done
  die "the Root did not bring up the zone in 120 s (see $W/root.out)"
}
stop_root() {
  t1_renv "$RH/bin/init-root" stop >/dev/null 2>&1 || true
  kill -TERM "$ROOT_PID" 2>/dev/null || true
  for _ in $(seq 1 120); do kill -0 "$ROOT_PID" 2>/dev/null || break; sleep 0.25; done
  wait "$ROOT_PID" 2>/dev/null || true
  ROOT_PID=""
}
drive() { # NAME SAY WANT [launcher args]
  local n="$1" s="$2" want="$3"; shift 3
  mkdir -p "$DIR/ws"
  /usr/bin/python3 -I "$W/drive.py" "$BIN/init-agent" "$H" "$W/attach-$n.jsonl" "$s" "$want" 300 "$DIR/ws" "$@" >"$W/drive-$n.json" \
    || { cat "$W/drive-$n.json" >&2; die "drive $n"; }
  log "drive $n: $(cat "$W/drive-$n.json")"
}
protected() { # the Root's binary, the kernel's binaries, the launchers, the staging and the journal: sha256 (or ABSENT)
  for f in "$RH/bin/init-root" "$IH/kernel/bin/init-kernel" "$IH/kernel/bin/initctl" "$IH/kernel/bin/init-evil" "$BIN/init-agent" \
           "$BIN/init-tui" "$BIN/init" "$BIN/init-agent.moved" "$RH/cache/get.sh" "$RH/state/upgrade.json"; do
    printf '%s %s\n' "$f" "$( [ -e "$f" ] && sha "$f" || echo ABSENT)"
  done
  for d in "$RH/cache/upgrade/evil" "$RH/state/upgrade/evil"; do printf '%s %s\n' "$d" "$( [ -e "$d" ] && echo PRESENT || echo ABSENT)"; done
}

# ---------------------------------------------------------------- 1. before
NV="$(member "$NEW" VERSION)"
log "old install $DIR ($(t1_renv "$RH/bin/init-root" --version)); new bundle $(basename "$NEW") ($NV)"
verify before || die "audit verify --full fails before the upgrade"
kept "$W/kept-0.json"; tuples >"$W/tuples-0.json"
log "before: $(cat "$W/tuples-0.json")"
OLD_INIT="$( [ -e "$BIN/init" ] && echo present || echo absent)"

# ---------------------------------------------------------------- 2. the upgrade (NEW's get.sh: 0.4.1 has no init-agent upgrade)
T=$(t1_ms)
set +e
"${GET_ENV[@]}" /bin/sh "$(mk_get "$NEW")" --upgrade --from "$NEW" --sha256 "$NEW_SHA" -- --probe-timeout 120 >"$W/up1.log" 2>&1 </dev/null
RC1=$?
set -e
UP1_MS=$(( $(t1_ms) - T ))
log "upgrade 1 exit $RC1 in $UP1_MS ms"
[ $RC1 = 0 ] || { tail -40 "$W/up1.log" >&2; die "the upgrade to $NV"; }
kept "$W/kept-1.json"; tuples >"$W/tuples-1.json"
K1="$(kept_same "$W/kept-0.json" "$W/kept-1.json")" || die "the upgrade changed kept state: $K1"
verify after1 || die "audit verify --full fails after the upgrade"
NEWROOT="$(member "$NEW" init-root | /usr/bin/shasum -a 256 | cut -d' ' -f1)"
NEWK="$(member "$NEW" init-kernel | /usr/bin/shasum -a 256 | cut -d' ' -f1)"
[ "$(sha "$RH/bin/init-root")" = "$NEWROOT" ] || die "ROOT_HOME/bin/init-root is not the new bundle's"
[ "$(sha "$IH/kernel/bin/init-kernel")" = "$NEWK" ] || die "kernel/bin/init-kernel is not the new bundle's"
[ "$(jq1 "$RH/state/upgrade.json" phase)" = committed ] || die "the journal is not committed"
[ "$(jq1 "$RH/install.json" release.version)" = "$NV" ] || die "install.json release.version is not $NV"
[ ! -e "$RH/state/upgrade/$(jq1 "$RH/state/upgrade.json" id)" ] || die "the snapshot was not removed after the commit"
[ "$(jq1 "$W/tuples-1.json" body)" = "$(jq1 "$W/tuples-0.json" body)" ] || die "the current tuple's body changed"
[ "$(jq1 "$W/tuples-1.json" ext)" = "$(jq1 "$W/tuples-0.json" ext)" ] || die "the current tuple's ext changed"
[ "$(jq1 "$W/tuples-1.json" kernel)" = "$NEWK" ] || die "the current tuple's kernel is not the new one"
[ "$(jq1 "$W/tuples-1.json" lkg)" = "$(jq1 "$W/tuples-1.json" current)" ] || die "lkg did not move to the upgraded tuple"
grep -q '= upgrade ]; then' "$BIN/init-agent" || die "the launcher is not the new one (no upgrade subcommand)"
[ ! -e "$BIN/init" ] || die "INIT's old init ($OLD_INIT before) is still in $BIN"
cmp -s "$RH/cache/get.sh" <(member "$NEW" scripts/get.sh) || die "ROOT_HOME/cache/get.sh is not the new bundle's get.sh"
log "upgrade 1 ok: $(cat "$W/tuples-1.json")"

# ---------------------------------------------------------------- 3. the upgraded install answers; the sandbox cannot upgrade or write the Root
cp "$NEW" "$ATT"
mkdir -p "$W/mock"; start_mock; start_root
drive hello1 "Say hello." "hello from the mock"
protected >"$W/protected-0.txt"
drive full "Check the Root from inside." "sandbox check done"
drive ws "Check the Root from inside." "sandbox check done" --workspace
stop_root
protected >"$W/protected-1.txt"
diff "$W/protected-0.txt" "$W/protected-1.txt" >&2 || die "a protected file changed from inside the sandbox"
# one record per access mode the body ran in ($INIT_ACCESS): full, and the confined one (workspace, or none when the
# launch directory is no valid workspace)
MODES=""
for f in "$IH"/work/upg-attack-*.txt; do
  [ -f "$f" ] || continue
  m="$(basename "$f" .txt)"; m="${m#upg-attack-}"; MODES="$MODES $m"
  tail -1 "$f" | grep -qx end || die "the attack record for access $m is incomplete ($f)"
  [ "$(grep -cv '^end$' "$f")" = 14 ] || die "the attack record for access $m has not 14 attempts"
  if grep -v '^end$' "$f" | awk '$2 == 0' | grep -q .; then die "an attempt succeeded in access $m: $(awk '$2 == 0' "$f" | tr '\n' ' ')"; fi
  log "sandbox ($m): $(grep -v '^end$' "$f" | awk '{print $1, $2}' | tr '\n' ' ')"
done
case " $MODES " in *" full "*) ;; *) die "no attack ran in full access (modes:$MODES)" ;; esac
[ "$(echo $MODES | wc -w | tr -d ' ')" -ge 2 ] || die "the attack ran in one access mode only ($MODES)"
verify after-sandbox || die "audit verify --full fails after the sandbox check"
rm -f -- "$ATT"; rm -rf -- "$IH"/work/rel.*

# ---------------------------------------------------------------- 4. a broken bundle: the probe fails, the old binaries come back
mkdir -p "$W/bundles"
printf '#!/bin/sh\necho "upgrade-e2e: a deliberately broken kernel" >&2\nexit 1\n' >"$W/broken-kernel"
BROKEN="$(mk_bundle "$NEW" "$W/bundles" "$NV-broken" "init-kernel=$W/broken-kernel")"
kept "$W/kept-2.json"; tuples >"$W/tuples-2.json"; protected >"$W/protected-2.txt"
T=$(t1_ms)
set +e
"${GET_ENV[@]}" "$BIN/init-agent" upgrade --bundle "$BROKEN" --sha256 "$(cut -d' ' -f1 "$BROKEN.sha256")" -- --probe-timeout 60 >"$W/up2.log" 2>&1 </dev/null
RC2=$?
set -e
UP2_MS=$(( $(t1_ms) - T ))
log "upgrade 2 (broken) exit $RC2 in $UP2_MS ms"
[ $RC2 != 0 ] || die "the broken bundle was accepted"
grep -q 'rolled back' "$W/up2.log" || { tail -30 "$W/up2.log" >&2; die "the broken upgrade was not rolled back"; }
tuples >"$W/tuples-3.json"; protected >"$W/protected-3.txt"; kept "$W/kept-3.json"
diff "$W/protected-2.txt" "$W/protected-3.txt" | grep -v upgrade.json | grep '^[<>]' >&2 && die "the rollback did not restore the Root, kernel and launchers"
[ "$(jq1 "$W/tuples-3.json" current)" = "$(jq1 "$W/tuples-2.json" current)" ] || die "the current tuple is not the one from before"
[ "$(jq1 "$RH/state/upgrade.json" phase)" = rolled_back ] || die "the journal is not rolled_back"
# the rollback record: the upgrade's own (reason upgrade_failed), or the Root's crash-loop rollback to LKG during the probe
# (LKG stays on the old tuple until the commit), both back to the tuple from before
RB="$(grep '"kind":"rollback"' "$RH/lineage/lineage.jsonl" | tail -1)"
/usr/bin/python3 -I -c 'import json,sys; d=json.loads(sys.argv[1])["data"]; sys.exit(0 if d["to"]==sys.argv[2] and d["reason"] in ("upgrade_failed","rollback_system") else 1)' \
  "$RB" "$(jq1 "$W/tuples-2.json" current)" || die "no rollback record back to the tuple from before: $RB"
log "rollback record: $(/usr/bin/python3 -I -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1])["data"]))' "$RB")"
K3="$(kept_same "$W/kept-2.json" "$W/kept-3.json")" || die "the failed upgrade changed kept state: $K3"
verify after-broken || die "audit verify --full fails after the rollback"
start_root
drive hello2 "Say hello." "hello from the mock"
stop_root
log "broken bundle rolled back; the old install answers"

# ---------------------------------------------------------------- 5. a body this release cannot run: refused, nothing changes
printf '#!/bin/sh\n[ "$1" = --manifest ] || exit 64\necho %s\n' \
  "'{\"body_version\":\"9.9.9\",\"control_channel\":\"stdio-jsonl/2\",\"ext_api_majors\":[1],\"manifest\":1}'" >"$W/fake-body"
INCOMPAT="$(mk_bundle "$NEW" "$W/bundles" "$NV-incompat" "body/init-body=$W/fake-body" "init-kernel=+upgrade-e2e: incompatible variant")"
cp "$RH/lineage/lineage.jsonl" "$W/lineage-4.jsonl"; protected >"$W/protected-4.txt"; cp "$RH/state/upgrade.json" "$W/journal-4.json"
set +e
"${GET_ENV[@]}" "$BIN/init-agent" upgrade --bundle "$INCOMPAT" --sha256 "$(cut -d' ' -f1 "$INCOMPAT.sha256")" >"$W/up3.log" 2>&1 </dev/null
RC3=$?
set -e
log "upgrade 3 (incompatible body) exit $RC3"
[ $RC3 != 0 ] || die "the incompatible bundle was accepted"
grep -q 'speaks stdio-jsonl/1, and this release.s kernel speaks stdio-jsonl/2' "$W/up3.log" || { tail -20 "$W/up3.log" >&2; die "no body-protocol refusal"; }
cmp -s "$RH/lineage/lineage.jsonl" "$W/lineage-4.jsonl" || die "the refused upgrade changed the lineage"
cmp -s "$RH/state/upgrade.json" "$W/journal-4.json" || die "the refused upgrade changed the journal"
protected >"$W/protected-5.txt"; diff "$W/protected-4.txt" "$W/protected-5.txt" >&2 || die "the refused upgrade changed a binary or launcher"

# ---------------------------------------------------------------- 6. the owner's state and the verdict
t1_snap >"$W/owner-after.txt"
diff "$W/owner-before.txt" "$W/owner-after.txt" >"$W/owner.diff" || die "the owner's state changed: $(head -5 "$W/owner.diff")"
/usr/bin/python3 -I - "$W" "$VERDICT" "$NEW" "$NV" "$UP1_MS" "$UP2_MS" "$(( $(t1_ms) - T_ALL ))" <<'PY'
import json, os, sys
w, out, new, nv, up1, up2, total = sys.argv[1:8]
j = lambda f: json.load(open(os.path.join(w, f)))
import tarfile
with tarfile.open(new) as t:
    top = os.path.basename(new)[:-len(".tar.gz")]
    man = json.load(t.extractfile(f"{top}/MANIFEST.json"))
r = {"ok": True, "bundle": os.path.basename(new), "version": nv, "test_bundle": man.get("test_bundle"), "commit": man.get("commit"),
     "tuples": {"before": j("tuples-0.json"), "upgraded": j("tuples-1.json"), "after_broken": j("tuples-3.json")},
     "kept_unchanged": True, "audit_verify_full": "0 before, after the upgrade, after the sandbox check, after the rollback",
     "sandbox": {f[len("upg-attack-"):-4]: open(os.path.join(w, "..", "i", "work", f)).read().split("\n")[:-2]
                 for f in sorted(os.listdir(os.path.join(w, "..", "i", "work"))) if f.startswith("upg-attack-")},
     "broken_bundle": "probe failed, rolled back to the old binaries and tuple; the old install answered",
     "incompatible_body": "refused (stdio-jsonl/1 vs stdio-jsonl/2); lineage, journal, binaries unchanged",
     "times_ms": {"upgrade": int(up1), "broken_upgrade_and_rollback": int(up2), "total": int(total)},
     "drives": {n: j(f"drive-{n}.json") for n in ("hello1", "full", "ws", "hello2")}, "owner_state": "unchanged"}
open(out, "w").write(json.dumps(r, indent=1, sort_keys=True) + "\n")
print(json.dumps({"ok": True, "times_ms": r["times_ms"]}))
PY
log "PASS; verdict $VERDICT"
