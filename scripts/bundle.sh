#!/bin/bash
# Build the prebuilt release bundle (docs/ux-v1.md §3.2.2, §3.5.1; lane UX-BUNDLE): one tar.gz that installs INIT
# on a Mac or a Linux machine with no Rust, in about a minute, through scripts/get.sh. It builds for the machine it
# runs on: macos-arm64 (Apple silicon), linux-arm64 (aarch64) or linux-x86_64.
#
#   scripts/bundle.sh [--label preview] [--reuse-release] [--keep-release] [-- <release.sh args>]
#
#   --label L        the pre-release label: the bundle is v<version>-L (default preview; --label "" for none)
#   --reuse-release  use target/dist/v<version>/ as it is (it must name HEAD and be clean); else release.sh runs
#                    with --from-head (other lanes' uncommitted edits never reach the bundle) and --skip-tests
#                    unless --with-tests is among the release.sh args
#   --keep-release   keep target/dist/v<version>/ afterwards (default: removed by exact path; only the bundle stays)
#
# Output, in target/dist/v<version>-<label>/:
#   init-v<version>-<label>-<target>.tar.gz          the bundle (top dir init-v<version>-<label>-<target>/)
#   init-v<version>-<label>-<target>.tar.gz.sha256   `<sha256>  <file>`, what get.sh checks
#   latest.json                                         version, file, sha256, bytes, commit (a channel file)
#
# The bundle holds the release directory (release.sh: init-root, init-kernel, initctl, init-mockllm, init-tui, the
# kernel source bundle src/<version>.bundle, the seed bundle seed/seed-v<version>.bundle with its skills and
# vendor/, docs, wit, ext, assets, MANIFEST.json), plus:
#   body/init-body, body/BODY.json   the prebuilt genesis body and its record {sha256, commit, tag, profile, rustc}
#   scripts/{install.sh,init-agent,init-shim,uninstall.sh,get.sh}, VERSION, README.md
#   SHA256SUMS                       every other file (`shasum -a 256 -c SHA256SUMS`, or `sha256sum -c` on Linux)
#
# The prebuilt body is the one the install-mode kernel builds: this script installs the release into scratch
# homes under target/ub/ (mock provider, no service, nothing outside target/), so the body comes out of the
# kernel's own G0 build command (dev profile, kernel/supervise.md §3.6.2) from the seed bundle's tag. It is then
# checked: its sha256 equals lkg.json's and the genesis tuple's, its commit is the seed tag's, and G1
# (`init-kernel selfmap` in gate mode) on the seed tag's tree with this body reproduces the committed self.json.
# The scratch homes are removed by exact path. Needs Rust 1.97.1 with wasm32-wasip2 (the maintainer's machine).
set -euo pipefail
umask 022

REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
LABEL=preview; REUSE=0; KEEP=0; RARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --label) LABEL="$2"; shift 2 ;;
    --reuse-release) REUSE=1; shift ;;
    --keep-release) KEEP=1; shift ;;
    --) shift; RARGS=("$@"); break ;;
    -h|--help) sed -n '2,32p' "$0"; exit 0 ;;
    *) echo "bundle.sh: unknown argument $1 (see --help)" >&2; exit 64 ;;
  esac
done

log() { printf '[bundle %s] %s\n' "$(date +%H:%M:%S)" "$*"; }
die() { log "FAIL: $*"; exit 1; }
free_mb() { df -m / | awk 'NR==2{print $4}'; }
now() { date +%s; }
T0=$(now)

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) TARGET=macos-arm64 ;;
  Linux/aarch64|Linux/arm64) TARGET=linux-arm64 ;;
  Linux/x86_64) TARGET=linux-x86_64 ;;
  *) die "no bundle target for $(uname -s) $(uname -m) (macOS arm64, Linux aarch64 or x86_64)" ;;
esac
if [ "$(uname -s)" = Darwin ]; then
  sha256() { shasum -a 256 "$@"; }
  sha_check() { shasum -a 256 -c --quiet "$@"; }
  pack() { COPYFILE_DISABLE=1 /usr/bin/tar --no-mac-metadata -czf "$@"; }
else
  sha256() { sha256sum "$@"; }
  sha_check() { sha256sum -c --quiet "$@"; }
  pack() { tar --owner=0 --group=0 --numeric-owner --sort=name -czf "$@"; }
fi
bytes() { wc -c <"$1" | tr -d ' '; }
VERSION="$(git -C "$REPO" show HEAD:root/Cargo.toml | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)"
[ -n "$VERSION" ] || die "no version in HEAD's root/Cargo.toml"
TAG="v$VERSION${LABEL:+-$LABEL}"
NAME="init-$TAG-$TARGET"
REL="$REPO/target/dist/v$VERSION"
OUT="$REPO/target/dist/$TAG"
UB="$REPO/target/ub" # scratch homes: short, so ROOT_HOME stays under the Root's 64-byte socket-path limit
COMMIT="$(git -C "$REPO" rev-parse HEAD)"
[ "$(free_mb)" -ge 4000 ] || die "only $(free_mb) MB free; the bundle needs about 4 GB while it builds"
log "INIT $TAG ($NAME) at ${COMMIT:0:12}; free $(free_mb) MB"

cleanup() {
  if [ -e "$UB" ]; then chmod -R u+w "$UB" 2>/dev/null || true; rm -rf -- "$UB"; fi
  if [ -e "$OUT/$NAME" ]; then rm -rf -- "$OUT/$NAME"; fi
}
trap cleanup EXIT

# ---------------------------------------------------------------- 1. the release directory
if [ $REUSE = 1 ]; then
  [ -f "$REL/MANIFEST.json" ] || die "--reuse-release: $REL/MANIFEST.json is missing"
else
  want_tests=0
  for a in ${RARGS[@]+"${RARGS[@]}"}; do [ "$a" = --with-tests ] && want_tests=1; done
  args=(--from-head)
  [ $want_tests = 1 ] || args+=(--skip-tests)
  for a in ${RARGS[@]+"${RARGS[@]}"}; do [ "$a" = --with-tests ] || args+=("$a"); done
  log "release.sh ${args[*]}"
  "$REPO/scripts/release.sh" "${args[@]}" || die "release.sh failed"
fi
# the release must be clean and built from a commit whose built trees equal HEAD's (a scripts/ or docs/ commit since
# changes nothing in it)
RCOMMIT="$(/usr/bin/python3 -I -c 'import json,sys; m=json.load(open(sys.argv[1])); print("" if m.get("dirty") else m.get("commit",""))' "$REL/MANIFEST.json")"
[ -n "$RCOMMIT" ] || die "$REL was built from a dirty tree"
if [ $REUSE = 1 ]; then # (a release just built names the HEAD it started from, even if other lanes commit meanwhile)
  git -C "$REPO" diff --quiet "$RCOMMIT" HEAD -- kernel root seed wit tui \
    || die "$REL (built from ${RCOMMIT:0:12}) differs from HEAD in kernel/ root/ seed/ wit/ tui/"
fi
COMMIT="$RCOMMIT"
SEED_BUNDLE="$REL/seed/seed-v$VERSION.bundle"
[ -f "$SEED_BUNDLE" ] || die "the release has no $SEED_BUNDLE"
T_REL=$(now)

# ---------------------------------------------------------------- 2. the prebuilt body (the install-mode kernel's build)
cleanup
mkdir -p "$UB"
log "installing the release into scratch homes $UB/{r,h} so the install-mode kernel builds the body (minutes)"
set +e
env INIT_ROOT_HOME="$UB/r" INIT_HOME="$UB/h" INIT_BIN_DIR="$UB/b" "$REPO/scripts/install.sh" --reuse --yes --no-service \
  --disk-floor 1 --mock-upstream 127.0.0.1:9 >"$UB/install.log" 2>&1
rc=$?
set -e
[ $rc = 0 ] || { tail -40 "$UB/install.log" >&2; die "the scratch install failed (exit $rc)"; }
BODY="$UB/h/bin/init-body"
[ -f "$BODY" ] || die "the scratch install left no $BODY"
cat >"$UB/info.py" <<'PY'
import json, os, sys
lkg = json.load(open(sys.argv[1]))
# the genesis tuple: the Root store (root/root.md §3.7); its body blob is what the Root admitted
tdir = os.path.join(sys.argv[2], "store/tuples")
bodies = set()
for f in os.listdir(tdir):
    if not f.endswith(".json"):
        continue
    t = json.load(open(os.path.join(tdir, f)))
    bodies.add(t.get("body", {}).get("binary") or "")
print(lkg["binary_sha256"], lkg["sha"], ",".join(sorted(b for b in bodies if b)) or "-")
PY
read -r BODY_SHA BODY_COMMIT GENESIS_BODY <<<"$(/usr/bin/python3 -I "$UB/info.py" "$UB/h/state/kernel/lkg.json" "$UB/r")"
GOT="$(sha256 "$BODY" | cut -d' ' -f1)"
[ "$GOT" = "$BODY_SHA" ] || die "bin/init-body is $GOT, lkg.json says $BODY_SHA"
[ "$GENESIS_BODY" = "$BODY_SHA" ] || die "the genesis tuple's body ($GENESIS_BODY) is not bin/init-body ($BODY_SHA)"
SEED_COMMIT="$(/usr/bin/git bundle list-heads "$SEED_BUNDLE" "refs/tags/seed-v$VERSION" | cut -d' ' -f1)"
# an annotated tag would list the tag object; release.sh makes a lightweight one, so this is the commit
[ "$SEED_COMMIT" = "$BODY_COMMIT" ] || die "the body was built from $BODY_COMMIT, the seed tag names $SEED_COMMIT"
log "prebuilt body $BODY_SHA (seed commit ${BODY_COMMIT:0:12}, $(( $(bytes "$BODY") / 1048576 )) MB)"
T_BODY=$(now)

# ---------------------------------------------------------------- 3. G1: the body and the seed's committed self.json agree
G1="$UB/g1"
mkdir -p "$G1/ext" "$G1/home" "$G1/tmp"
/usr/bin/git -c advice.detachedHead=false clone -q --branch "seed-v$VERSION" "$SEED_BUNDLE" "$G1/src" 2>/dev/null \
  || die "cannot check out seed-v$VERSION from $SEED_BUNDLE"
rm -rf -- "$G1/src/.git" # G1 walks an export (no .git), as promote::stages::g1 does
for w in "$REL"/ext/*.wasm; do # the release builds each component as <crate_name>.wasm; G1 names them <dir>.wasm
  b="$(basename "$w" .wasm)"; cp "$w" "$G1/ext/${b//_/-}.wasm"
done
TCB="$(dirname "$(rustup which rustc)")"
set +e
env -i PATH="$TCB:/usr/bin:/bin" HOME="$G1/home" TMPDIR="$G1/tmp/" LANG=C "$REL/init-kernel" selfmap --root "$G1/src" \
  --binary "$BODY" --components "$G1/ext" --out "$G1/self.json" >"$G1/selfmap.log" 2>&1
rc=$?
set -e
[ $rc = 0 ] || { cat "$G1/selfmap.log" >&2; die "init-kernel selfmap (G1) exit $rc"; }
/usr/bin/python3 -I - "$G1/src/self.json" "$G1/self.json" <<'PY' || die "G1: the seed's committed self.json differs from the regenerated one (regenerate seed/self.json and commit it)"
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
if a != b:
    print("differing top-level keys:", sorted(k for k in set(a) | set(b) if a.get(k) != b.get(k)), file=sys.stderr)
    sys.exit(1)
PY
log "G1: the committed self.json matches the prebuilt body and the seed tag's tree"

# ---------------------------------------------------------------- 4. stage, checksum, pack
D="$OUT/$NAME"
mkdir -p "$OUT"
if [ -e "$D" ]; then rm -rf -- "$D"; fi
mkdir -p "$D"
cp -cR "$REL/." "$D/" 2>/dev/null || cp -R "$REL/." "$D/" # the contents ("/." : GNU cp would nest "$REL/" as a subdir)
mkdir -p "$D/body" "$D/scripts"
cp -c "$BODY" "$D/body/init-body" 2>/dev/null || cp "$BODY" "$D/body/init-body"
chmod 755 "$D/body/init-body"
RUSTC_V="$(rustc -V)"
/usr/bin/python3 -I - "$D/body/BODY.json" "$BODY_SHA" "$BODY_COMMIT" "seed-v$VERSION" "$RUSTC_V" <<'PY'
import json, sys
out, sha, commit, tag, rustc = sys.argv[1:6]
rec = {"v": 1, "sha256": sha, "commit": commit, "tag": tag, "profile": "dev", "rustc": rustc,
       "built_by": "init-kernel install mode (the G0 build command, kernel/supervise.md §3.6.2)",
       "g1": "self.json reproduced"}
open(out, "w").write(json.dumps(rec, indent=1, sort_keys=True) + "\n")
PY
for f in install.sh init-agent init-shim uninstall.sh get.sh; do cp "$REPO/scripts/$f" "$D/scripts/$f"; chmod 755 "$D/scripts/$f"; done
cp "$REPO/README.md" "$D/README.md"
printf '%s\n' "${TAG#v}" >"$D/VERSION"
(cd "$D" && find . -type f ! -name SHA256SUMS | sed 's|^\./||' | LC_ALL=C sort | while IFS= read -r f; do sha256 "$f"; done) >"$D/SHA256SUMS"
(cd "$D" && sha_check SHA256SUMS) || die "SHA256SUMS does not verify"
TGZ="$OUT/$NAME.tar.gz"
pack "$TGZ.partial" -C "$OUT" "$NAME"
mv "$TGZ.partial" "$TGZ"
TGZ_SHA="$(sha256 "$TGZ" | cut -d' ' -f1)"
printf '%s  %s\n' "$TGZ_SHA" "$NAME.tar.gz" >"$TGZ.sha256"
/usr/bin/python3 -I - "$OUT/latest.json" "${TAG#v}" "$NAME.tar.gz" "$TGZ_SHA" "$(bytes "$TGZ")" "$COMMIT" "$BODY_SHA" "$TARGET" <<'PY'
import json, sys
out, version, f, sha, size, commit, body, target = sys.argv[1:9]
# one channel file per target; scripts/get.sh derives every target's asset name from "version"
m = {"v": 1, "version": version, "target": target, "file": f, "sha256": sha, "bytes": int(size),
     "commit": commit, "body_sha256": body}
open(out, "w").write(json.dumps(m, indent=1, sort_keys=True) + "\n")
PY
UNPACKED_MB=$(du -sm "$D" | cut -f1)
cleanup
if [ $KEEP = 0 ] && [ -e "$REL" ]; then rm -rf -- "$REL"; log "removed $REL (--keep-release keeps it)"; fi
log "bundle $TGZ ($(( $(bytes "$TGZ") / 1048576 )) MB packed, $UNPACKED_MB MB unpacked; sha256 ${TGZ_SHA:0:16}…)"
log "times: release $(( T_REL - T0 )) s, prebuilt body $(( T_BODY - T_REL )) s, total $(( $(now) - T0 )) s; free $(free_mb) MB"
