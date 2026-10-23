#!/bin/bash
# Build an INIT release directory (root/root.md §4.3 "The release directory") from this repository.
#
#   scripts/release.sh [--out DIR] [--skip-tests] [--allow-dirty | --from-head] [--jobs N] [--serial] [--lean] [--min-free MB]
#
# --from-head: build HEAD, not the working tree, so other lanes' uncommitted edits in the one shared checkout
# never reach a release. HEAD's kernel/ root/ seed/ wit/ tui/ are exported to target/release-src/ (vendor/ is a
# symlink to the repo's) and built there into each workspace's default target dir (CARGO_TARGET_DIR); the export
# goes again by exact path on exit. MANIFEST.json records dirty=false: the binaries and the bundles are HEAD's.
#
# What it builds (release profile, offline, --locked, each workspace's default target dir):
#   kernel/  init-kernel, initctl, init-mockllm (the $0 mock the gate's G0 runs the seed's provider tests against;
#            no network calls)   → kernel/target/release/
#   root/    init-root                   → root/target/release/
#   seed/    init-body                   → seed/target/release/   (a check and an artifact: the installed body
#                                          is built by the install-mode kernel itself, dev profile, supervise.md §3.6.2)
#   seed/ext the template extension count-lines, wasm32-wasip2, dev profile (ext/authoring.md §3.4: the only
#            profile used for extensions), CARGO_TARGET_DIR=seed/target
#   tui/     init-tui (docs/tui.md), when the tui/ workspace exists → tui/target/release/
# The workspaces build in parallel (--serial: one at a time, for a shared machine; ci/env.sh caps CARGO_BUILD_JOBS);
# the ext template builds after the seed (same target dir).
#
# What it assembles in DIR (default target/dist/v<version>/), as APFS clones where possible:
#   init-root init-kernel initctl init-mockllm init-body   the binaries (and init-tui when tui/ exists)
#   src/<version>.bundle                      the kernel source at HEAD (kernel/ and wit/), one ref refs/heads/release (§3.8.1)
#   seed/seed-v<version>.bundle               the seed at HEAD: branch `seed`, tag seed-v<version>, its own vendor/
#                                             cut to its lock files (scripts/slice1/seedpack.py, FIX4)
#   docs/                                     the kernel docs the agent reads (docs/kernel/*.md, incl. the selfmap recipes)
#   wit/                                      the extension WIT (wit/init-ext)
#   ext/count_lines.wasm                      the template extension, built
#   assets/                                   init-root's charter template and constitution (for install.sh's wizard)
#   MANIFEST.json                             commit, dirty flag, toolchain, sha256 and size of every file, build times
#
# Quality guard (on by default; --skip-tests skips it): `cargo test --release --offline` for the root and the seed
# workspaces (as ci/local.sh runs them, no features) must both be green before the directory is assembled.
# --lean (a disk-constrained machine): each release target dir is removed (exact path) once its outputs are staged and
# its tests ran; with --serial the kernel's goes right after its build. The next release build is then a cold one.
# Disk: refuses to start below 8 GB free for a cold build, 6 GB for a warm one (the three release target dirs measured
# 2.35 GB; the release-mode tests and the seed packaging add about 0.7 GB); --min-free MB overrides that when the
# caller knows better (a partly built tree). The work dir for the bundles is removed by exact path when done.
set -euo pipefail
umask 022

REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
OUT=
SKIP_TESTS=0
ALLOW_DIRTY=0
FROM_HEAD=0
SERIAL=0
LEAN=0
MIN_FREE=
JOBS=
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --skip-tests) SKIP_TESTS=1; shift ;;
    --allow-dirty) ALLOW_DIRTY=1; shift ;;
    --jobs) JOBS="$2"; shift 2 ;;
    --serial) SERIAL=1; shift ;;
    --lean) LEAN=1; shift ;;
    --min-free) MIN_FREE="$2"; shift 2 ;;
    --from-head) FROM_HEAD=1; shift ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "release.sh: unknown argument $1 (see --help)" >&2; exit 64 ;;
  esac
done

log() { printf '[release %s] %s\n' "$(date +%H:%M:%S)" "$*"; }
die() { log "FAIL: $*"; exit 1; }
free_mb() { df -m / | awk 'NR==2{print $4}'; }
now() { date +%s; }

# shellcheck source=../ci/env.sh
source "$REPO/ci/env.sh"
export CARGO_TERM_COLOR=never
[ -n "$JOBS" ] && export CARGO_BUILD_JOBS="$JOBS"

VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/root/Cargo.toml" | head -1)"
[ -n "$VERSION" ] || die "no workspace version in root/Cargo.toml"
for ws in kernel seed; do
  v="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/$ws/Cargo.toml" | head -1)"
  [ "$v" = "$VERSION" ] || die "$ws/Cargo.toml version $v differs from root's $VERSION"
done
OUT="${OUT:-$REPO/target/dist/v$VERSION}"

# ---------------------------------------------------------------- preflight
command -v cargo >/dev/null || die "cargo not found: install the Rust toolchain (rustup, 1.97)"
RUSTC_V="$(rustc -V)"
case "$RUSTC_V" in rustc\ 1.97.*) ;; *) die "need rustc 1.97.x, found: $RUSTC_V" ;; esac
rustup target list --installed 2>/dev/null | grep -qx wasm32-wasip2 || die "missing target: rustup target add wasm32-wasip2"
[ -d "$REPO/vendor" ] || die "vendor/ is missing: run ci/vendor.sh --locked once (crates.io only)"
FREE0="$(free_mb)"
NEED_MB=8000 # cold: the three release target dirs grow by 2.35 GB (measured), the release-mode tests and the seed packaging by about 0.7 GB
[ -x "$REPO/kernel/target/release/init-kernel" ] && [ -x "$REPO/seed/target/release/init-body" ] && NEED_MB=6000 # warm: the test builds and the seed packaging add about 1 GB
[ -n "$MIN_FREE" ] && NEED_MB="$MIN_FREE"
[ "$FREE0" -ge "$NEED_MB" ] || die "only $FREE0 MB free; this build needs $NEED_MB MB (the floor is 5 GB)"
COMMIT="$(git -C "$REPO" rev-parse HEAD)"
TUI=0; [ -f "$REPO/tui/Cargo.toml" ] && TUI=1
DIRTY="$(git -C "$REPO" status --porcelain --untracked-files=no -- kernel root seed wit $([ $TUI = 1 ] && echo tui) | head -20)" # untracked files build nothing unless a tracked one names them (then it is dirty)
if [ -n "$DIRTY" ] && [ $FROM_HEAD = 1 ]; then
  log "the working tree has uncommitted changes (other lanes'?); --from-head builds HEAD without them"
  DIRTY=""
fi
if [ -n "$DIRTY" ]; then
  [ $ALLOW_DIRTY = 1 ] || { echo "$DIRTY" >&2; die "kernel/ root/ seed/ or wit/ has uncommitted changes; commit them, or pass --allow-dirty (recorded in MANIFEST.json)"; }
  log "warning: building a dirty tree (--allow-dirty); the binaries are the working tree's, the bundles are HEAD's"
fi
log "INIT v$VERSION at ${COMMIT:0:12}${DIRTY:+ (dirty)}; $RUSTC_V; free $FREE0 MB"

WORK="$OUT.work"
LOGS="$OUT.logs"
if [ -e "$WORK" ]; then rm -rf -- "$WORK"; fi
if [ -e "$LOGS" ]; then rm -rf -- "$LOGS"; fi
mkdir -p "$WORK" "$LOGS"
SRC="$REPO" # where the workspaces are built from: the checkout, or HEAD's export (--from-head)
if [ $FROM_HEAD = 1 ]; then
  SRC="$REPO/target/release-src"
  if [ -e "$SRC" ]; then rm -rf -- "$SRC"; fi
  mkdir -p "$SRC"
  git -C "$REPO" archive HEAD kernel root seed wit $([ $TUI = 1 ] && echo tui) | /usr/bin/tar -x -C "$SRC"
  ln -s "$REPO/vendor" "$SRC/vendor"
  log "building HEAD ${COMMIT:0:12} from its export $SRC"
fi
trap 'if [ -e "$WORK" ]; then rm -rf -- "$WORK"; fi; if [ $FROM_HEAD = 1 ] && [ -e "$SRC" ]; then rm -rf -- "$SRC"; fi' EXIT
# The charter template and the constitution the init-root binary embeds (include_str!), snapshotted before the
# build so install.sh's wizard writes a charter in the dialect of exactly this binary.
mkdir -p "$WORK/new/assets"
cp "$SRC/root/crates/init-root/assets/charter-template.toml" "$SRC/root/crates/init-root/assets/CONSTITUTION.md" "$WORK/new/assets/"

# ---------------------------------------------------------------- builds (parallel)
build() { # <name> <dir> <cargo args…>: times one cargo command; writes $LOGS/<name>.{log,rc,s}
  local n="$1" d="$2"; shift 2
  local s; s=$(now)
  set +e
  if [ $FROM_HEAD = 1 ]; then # the export builds into the checkout's own target dir (seed/ext sets its own)
    (cd "$SRC/$d" && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO/${d%%/*}/target}" "$@") >"$LOGS/$n.log" 2>&1
  else
    (cd "$REPO/$d" && "$@") >"$LOGS/$n.log" 2>&1
  fi
  echo $? >"$LOGS/$n.rc"
  set -e
  echo $(( $(now) - s )) >"$LOGS/$n.s"
}
N="$WORK/new"
BIN_K="$REPO/kernel/target/release"
BIN_R="$REPO/root/target/release"
BIN_S="$REPO/seed/target/release"
BIN_T="$REPO/tui/target/release"
EXT_WASM="$REPO/seed/target/wasm32-wasip2/debug/count_lines.wasm"
stage() { # <file>…: APFS clones into the new release dir (a file already staged is kept)
  local f
  for f in "$@"; do
    [ -f "$N/$(basename "$f")" ] && continue
    [ -f "$f" ] || die "missing build output $f"
    cp -c "$f" "$N/" 2>/dev/null || cp "$f" "$N/"
  done
}
lean_rm() { # --lean: remove release target dirs whose outputs are staged (exact paths)
  [ $LEAN = 1 ] || return 0
  local d
  for d in "$@"; do if [ -e "$d" ]; then rm -rf -- "$d"; log "lean: removed $d; free $(free_mb) MB"; fi; done
}
T0=$(now)
# --serial: one cargo at a time (a shared, loaded machine); otherwise the workspaces build in parallel
bg() { if [ $SERIAL = 1 ]; then log "building ${2:-$1} (serial)"; "$@"; else "$@" & PIDS+=($!); fi; }
PIDS=()
kernel_job() { # no kernel test runs here, so with --serial --lean its target dir goes as soon as its binaries are staged
  build kernel kernel cargo build --release --offline --locked -p init-kernel -p initctl -p init-testkit \
    --bin init-kernel --bin initctl --bin init-mockllm
  if [ $SERIAL = 1 ] && [ "$(cat "$LOGS/kernel.rc")" = 0 ]; then stage "$BIN_K/init-kernel" "$BIN_K/initctl" "$BIN_K/init-mockllm"; lean_rm "$BIN_K"; fi
}
ok() { [ "$(cat "$LOGS/$1.rc" 2>/dev/null)" = 0 ]; }
guard_tests() { # <ws>: the quality guard, `cargo test --release` of root or seed (skipped with --skip-tests)
  [ $SKIP_TESTS = 0 ] || return 0
  build "test-$1" "$1" cargo test --release --offline --locked
}
# With --serial each workspace runs build → tests → stage → (--lean) remove before the next starts, so the disk peak
# is one workspace's target dir, not three.
root_job() {
  build root root cargo build --release --offline --locked -p init-root --bin init-root
  if [ $SERIAL = 1 ] && ok root; then
    guard_tests root
    if [ $SKIP_TESTS = 1 ] || ok test-root; then stage "$BIN_R/init-root"; lean_rm "$BIN_R"; fi
  fi
}
seed_and_ext() {
  build seed seed cargo build --release --offline --locked --bin init-body
  build ext seed/ext env CARGO_TARGET_DIR="$REPO/seed/target" cargo build --offline --locked --target wasm32-wasip2 -p count-lines
  if [ $SERIAL = 1 ] && ok seed; then
    guard_tests seed
    if [ $SKIP_TESTS = 1 ] || ok test-seed; then stage "$BIN_S/init-body"; lean_rm "$BIN_S"; fi
  fi
}
bg root_job      # smallest first: with --serial --lean each one's target dir is gone before the next grows
bg seed_and_ext
bg kernel_job
NAMES="kernel root seed ext"
if [ $TUI = 1 ]; then
  bg build tui tui cargo build --release --offline --locked
  NAMES="$NAMES tui"
fi
[ $SERIAL = 1 ] || log "building $NAMES in parallel; logs in $LOGS"
for p in ${PIDS[@]+"${PIDS[@]}"}; do wait "$p" || true; done
for n in $NAMES; do
  rc="$(cat "$LOGS/$n.rc" 2>/dev/null || echo missing)"
  [ "$rc" = 0 ] || { tail -30 "$LOGS/$n.log" >&2; die "$n build failed (exit $rc); see $LOGS/$n.log"; }
  log "built $n in $(cat "$LOGS/$n.s") s"
done
BUILD_S=$(( $(now) - T0 ))
log "all builds done in $BUILD_S s$([ $SERIAL = 1 ] && [ $SKIP_TESTS = 0 ] && echo " (serial mode: tests included)"); free $(free_mb) MB"

stage "$BIN_K/init-kernel" "$BIN_K/initctl" "$BIN_K/init-mockllm" "$BIN_R/init-root" "$BIN_S/init-body"
[ $TUI = 1 ] && stage "$BIN_T/init-tui"
mkdir -p "$N/ext"
[ -f "$EXT_WASM" ] || die "missing build output $EXT_WASM"
cp -c "$EXT_WASM" "$N/ext/" 2>/dev/null || cp "$EXT_WASM" "$N/ext/"
lean_rm "$BIN_K" # tui/target/release (about 50 MB) stays: lane TUI shares that workspace

# ---------------------------------------------------------------- quality guard: release-mode tests
TEST_S=0
if [ $SKIP_TESTS = 0 ]; then
  if [ $SERIAL = 0 ]; then # (serial mode ran them right after each build)
    PIDS=()
    bg guard_tests root
    bg guard_tests seed
    for p in ${PIDS[@]+"${PIDS[@]}"}; do wait "$p" || true; done
  fi
  for n in test-root test-seed; do
    rc="$(cat "$LOGS/$n.rc")"
    sum="$(grep -E '^test result:' "$LOGS/$n.log" | awk '{p+=$4; f+=$6; i+=$8} END{printf "%d passed, %d failed, %d ignored", p, f, i}')"
    [ "$rc" = 0 ] || { grep -E '^(test .* FAILED|failures:|---- )' "$LOGS/$n.log" | head -30 >&2; die "$n red (exit $rc; $sum); see $LOGS/$n.log"; }
    log "$n green: $sum ($(cat "$LOGS/$n.s") s)"
    TEST_S=$(( TEST_S + $(cat "$LOGS/$n.s") ))
  done
  lean_rm "$BIN_R" "$BIN_S"
else
  log "release-mode tests skipped (--skip-tests)"
  lean_rm "$BIN_R" "$BIN_S"
fi

# ---------------------------------------------------------------- bundles (from HEAD, fixed identity and date)
G() { # git in a work repo: no owner config, no hooks, no detached gc racing the rm below
  local d="$1"; shift
  env -i PATH=/usr/bin:/bin HOME="$WORK/git-home" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null TZ=UTC \
    GIT_AUTHOR_DATE=2026-10-20T00:00:00Z GIT_COMMITTER_DATE=2026-10-20T00:00:00Z \
    /usr/bin/git -C "$d" -c core.hooksPath=/dev/null -c user.name=init-maintainer -c user.email=maintainer@init.invalid \
    -c commit.gpgsign=false -c tag.gpgsign=false -c gc.auto=0 -c maintenance.auto=false "$@"
}
mkdir -p "$WORK/git-home" "$WORK/new/src" "$WORK/new/seed"

# the kernel source: kernel/ and wit/ at HEAD (kernel-selfmap reads ../../../wit), one ref refs/heads/release
mkdir -p "$WORK/ksrc"
git -C "$REPO" archive HEAD kernel wit | /usr/bin/tar -x -C "$WORK/ksrc"
G "$WORK/ksrc" init -q -b release
G "$WORK/ksrc" add -A
G "$WORK/ksrc" commit -q -m "init kernel v$VERSION (source ${COMMIT:0:12})"
KSRC_SHA="$(G "$WORK/ksrc" rev-parse HEAD)"
G "$WORK/ksrc" bundle create -q "$WORK/new/src/$VERSION.bundle" refs/heads/release
rm -rf -- "$WORK/ksrc"
log "kernel source bundle src/$VERSION.bundle = ${KSRC_SHA:0:12}"

# the seed: seed/ at HEAD plus its own vendor/ (the maintainer's packaging step; FIX4 seedpack.py, read-only reuse)
SEED_TAG="seed-v$VERSION"
/usr/bin/python3 -I "$REPO/scripts/slice1/seedpack.py" --repo "$REPO" --dst "$WORK/seedsrc" --kind repo --rev HEAD \
  >"$LOGS/seedpack.json" || die "seed packaging (scripts/slice1/seedpack.py)"
G "$WORK/seedsrc" init -q -b seed
G "$WORK/seedsrc" add -A
G "$WORK/seedsrc" commit -q -m "seed $SEED_TAG (INIT v$VERSION, source ${COMMIT:0:12})"
G "$WORK/seedsrc" tag "$SEED_TAG"
SEED_SHA="$(G "$WORK/seedsrc" rev-parse HEAD)"
G "$WORK/seedsrc" bundle create -q "$WORK/new/seed/$SEED_TAG.bundle" refs/heads/seed "refs/tags/$SEED_TAG"
rm -rf -- "$WORK/seedsrc"
log "seed bundle seed/$SEED_TAG.bundle = ${SEED_SHA:0:12} ($(cat "$LOGS/seedpack.json"))"

# ---------------------------------------------------------------- assemble
mkdir -p "$N/docs" "$N/wit"
cp "$REPO"/docs/kernel/*.md "$N/docs/"
cp -R "$SRC/wit/init-ext" "$N/wit/"
/usr/bin/python3 -I - "$N" "$VERSION" "$COMMIT" "${DIRTY:+1}" "$RUSTC_V" "$(rustc -vV | sed -n 's/^host: //p')" \
  "$KSRC_SHA" "$SEED_TAG" "$SEED_SHA" "$BUILD_S" "$TEST_S" "$SKIP_TESTS" "$LOGS" <<'PY'
import hashlib, json, os, sys
(n, version, commit, dirty, rustc, host, ksrc, seed_tag, seed_sha, build_s, test_s, skip, logs) = sys.argv[1:14]
files = {}
for d, _, fs in os.walk(n):
    for f in sorted(fs):
        p = os.path.join(d, f)
        h = hashlib.sha256()
        with open(p, "rb") as fh:
            for b in iter(lambda: fh.read(1 << 20), b""):
                h.update(b)
        files[os.path.relpath(p, n)] = {"sha256": h.hexdigest(), "bytes": os.path.getsize(p)}
def num(p):
    try: return int(open(os.path.join(logs, p)).read().strip())
    except Exception: return None
m = {"v": 1, "name": "init", "version": version, "commit": commit, "dirty": bool(dirty), "rustc": rustc, "host": host,
     "kernel_source": {"bundle": f"src/{version}.bundle", "ref": "refs/heads/release", "sha": ksrc},
     "seed": {"bundle": f"seed/{seed_tag}.bundle", "tag": seed_tag, "sha": seed_sha},
     "build_s": {k: num(f"{k}.s") for k in ("kernel", "root", "seed", "ext", "tui") if num(f"{k}.s") is not None} | {"total": int(build_s)},
     "tests": None if skip == "1" else {"root": "green", "seed": "green", "s": int(test_s)},
     "files": dict(sorted(files.items()))}
with open(os.path.join(n, "MANIFEST.json"), "w") as f:
    json.dump(m, f, indent=1, sort_keys=True)
    f.write("\n")
PY
if [ -e "$OUT" ]; then rm -rf -- "$OUT"; fi
mkdir -p "$(dirname "$OUT")"
mv "$N" "$OUT"
log "release v$VERSION in $OUT ($(du -sm "$OUT" | cut -f1) MB apparent; binaries are APFS clones); free $(free_mb) MB"
ls -l "$OUT" | sed 's/^/  /'
