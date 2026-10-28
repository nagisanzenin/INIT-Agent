#!/bin/bash
# Export the proof artifacts of a finished demo install into one directory, then refuse to finish if a key or secret
# is in them (lane HYGIENE, v0.5.0; the key scan of HANDOFF.md §2 rule 1, extended). It only reads the install: it
# copies, never moves or deletes, and never starts or stops anything. It publishes nothing.
#
#   scripts/proof/export.sh --root-home RH --init-home IH --out DIR
#                           [--init-root BIN] [--secret-file FILE]... [--deny TEXT]...
#
# DIR must not exist. It gets:
#   session/*.jsonl          the session transcripts (IH/state/sessions/)
#   lineage/lineage.jsonl    the Root's signed lineage (RH/lineage/), with head.json
#   lineage/lineage.pub      the genesis public key (RH/keys/lineage.pub); publish it apart from the files too
#   diffs/<seq>-<kind>-<sha12>.diff   each self-change named by a `promotion` or `change_open` record: its commits
#                            (message and trailers) and the diff, from IH/lineage/body.git
#   spend/usage/*.jsonl      the Root's per-request spend ledger (RH/egress/usage/)
#   PUBLISH.txt              the key and head hash to put in the release notes, and the verify command
#   MANIFEST.sha256          sha256 of every file above
#
# The scan reads every exported file and fails (exit 3, SCAN-FAILED.txt, no MANIFEST) on: the value of each provider
# key in RH/keys/providers/ and RH/keys/actions/; the Root's lineage signing key (RH/keys/lineage.ed25519, raw and hex);
# the value of each --secret-file (for example the owner's own key file, passed by the owner); each --deny TEXT (for
# example the owner's email); and common token shapes (sk-…, AKIA…, ghp_…, github_pat_…, xox?-…, AIza…, a PEM private
# key). It prints the file and the kind of match, never the value. A value shorter than 8 bytes is not scanned for.
# With --init-root (an init-root that has `audit verify --file`, v0.5.0+), the exported lineage is verified offline
# against the exported key and head (lineage/verify.txt); a failure also stops the export.
# Exit: 0 done; 2 bad input; 3 a secret matched; 4 the lineage did not verify.
set -euo pipefail
umask 077
export LC_ALL=C

usage() { echo "usage: $0 --root-home RH --init-home IH --out DIR [--init-root BIN] [--secret-file FILE]... [--deny TEXT]..." >&2; exit 2; }
RH=; IH=; OUT=; IROOT=; SECRETS=(); DENY=()
while [ $# -gt 0 ]; do
  case "$1" in
    --root-home) RH="${2:-}"; shift 2 ;;
    --init-home) IH="${2:-}"; shift 2 ;;
    --out) OUT="${2:-}"; shift 2 ;;
    --init-root) IROOT="${2:-}"; shift 2 ;;
    --secret-file) SECRETS+=("${2:-}"); shift 2 ;;
    --deny) DENY+=("${2:-}"); shift 2 ;;
    *) usage ;;
  esac
done
[ -n "$RH" ] && [ -n "$IH" ] && [ -n "$OUT" ] || usage
die() { echo "proof export: $*" >&2; exit 2; }
[ -f "$RH/lineage/lineage.jsonl" ] || die "$RH/lineage/lineage.jsonl: no lineage (is --root-home the demo's ROOT_HOME?)"
[ -f "$RH/keys/lineage.pub" ] || die "$RH/keys/lineage.pub: no public key"
[ -d "$IH/state/sessions" ] || die "$IH/state/sessions: no sessions (is --init-home the demo's INIT_HOME?)"
[ -e "$OUT" ] && die "$OUT exists; give a new directory"
for f in ${SECRETS[@]+"${SECRETS[@]}"}; do [ -f "$f" ] || die "--secret-file $f: not a file"; done
if [ -n "$IROOT" ]; then [ -x "$IROOT" ] || die "--init-root $IROOT: not executable"; fi

mkdir -p "$OUT/session" "$OUT/lineage" "$OUT/diffs" "$OUT/spend/usage"
OUT="$(cd "$OUT" && pwd -P)"

# ---- copies (cp -p: read-only use of the install)
n=0
for f in "$IH"/state/sessions/*.jsonl; do [ -f "$f" ] && { cp -p "$f" "$OUT/session/"; n=$((n + 1)); }; done
[ $n -gt 0 ] || echo "proof export: note: no session transcript in $IH/state/sessions" >&2
cp -p "$RH/lineage/lineage.jsonl" "$OUT/lineage/lineage.jsonl"
[ -f "$RH/lineage/head.json" ] && cp -p "$RH/lineage/head.json" "$OUT/lineage/head.json"
cp -p "$RH/keys/lineage.pub" "$OUT/lineage/lineage.pub"
for f in "$RH"/egress/usage/*.jsonl; do [ -f "$f" ] && cp -p "$f" "$OUT/spend/usage/"; done

# ---- self-change diffs, from the body's lineage repository (git with no user or system config)
GH="$(mktemp -d "${TMPDIR:-/tmp}/proof-git.XXXXXX")"
trap 'rm -rf -- "$GH"' EXIT
G=(env -i HOME="$GH" PATH=/usr/bin:/bin GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null git --git-dir="$IH/lineage/body.git")
/usr/bin/python3 -I - "$OUT/lineage/lineage.jsonl" >"$GH/changes" <<'PY'
import json, sys
seen = set()
for line in open(sys.argv[1], encoding="utf-8"):
    try:
        r = json.loads(line)
    except ValueError:
        continue
    d = r.get("data") or {}
    sha, frm = d.get("sha"), d.get("from_sha")
    if r.get("kind") in ("promotion", "change_open") and isinstance(sha, str) and isinstance(frm, str) and sha and frm and sha not in seen:
        seen.add(sha)
        print(r["seq"], r["kind"], frm, sha)
PY
nd=0
while read -r seq kind frm sha; do
  f="$OUT/diffs/$(printf '%06d' "$seq")-$kind-${sha:0:12}.diff"
  if "${G[@]}" cat-file -e "$sha^{commit}" </dev/null 2>/dev/null && "${G[@]}" cat-file -e "$frm^{commit}" </dev/null 2>/dev/null; then
    {
      echo "# lineage record $seq ($kind): $frm -> $sha"
      "${G[@]}" log --no-color --format='commit %H%nAuthor: %an <%ae>%nDate:   %aI%n%n%B' "$frm..$sha"
      "${G[@]}" diff --no-color --stat "$frm" "$sha"
      echo
      "${G[@]}" diff --no-color "$frm" "$sha"
    } </dev/null >"$f"
    nd=$((nd + 1))
  else
    echo "lineage record $seq ($kind): $frm -> $sha: a commit is not in body.git" >>"$OUT/diffs/MISSING.txt"
  fi
done <"$GH/changes"

# ---- the key and secret scan: the file and the kind of match only, never the value
set +e
/usr/bin/python3 -I - "$OUT" "$RH" ${SECRETS[@]+"${SECRETS[@]}"} <<'PY' 3<<<"$(printf '%s\n' ${DENY[@]+"${DENY[@]}"})"
import os, re, sys
out, rh, extra = sys.argv[1], sys.argv[2], sys.argv[3:]
needles = []  # (bytes, label)
def add(value, label):
    v = value.strip()
    if len(v) >= 8:
        needles.append((v, label))
for d in ("keys/providers", "keys/actions"):
    p = os.path.join(rh, d)
    for name in sorted(os.listdir(p)) if os.path.isdir(p) else []:
        f = os.path.join(p, name)
        if os.path.isfile(f):
            add(open(f, "rb").read(), f"the provider or action key {d}/{name}")
seed = os.path.join(rh, "keys/lineage.ed25519")
if os.path.isfile(seed):
    raw = open(seed, "rb").read()
    add(raw, "the lineage signing key (raw)")
    add(raw.hex().encode(), "the lineage signing key (hex)")
for i, f in enumerate(extra):
    add(open(f, "rb").read(), f"--secret-file #{i + 1}")
with os.fdopen(3, "rb") as deny:
    for i, line in enumerate(deny.read().split(b"\n")):
        if line.strip():
            add(line, f"--deny #{i + 1}")
shapes = [(re.compile(p), label) for p, label in [
    (rb"sk-[A-Za-z0-9_-]{20,}", "an sk- API key"),
    (rb"AKIA[0-9A-Z]{16}", "an AWS access key id"),
    (rb"gh[pousr]_[A-Za-z0-9]{36,}", "a GitHub token"),
    (rb"github_pat_[A-Za-z0-9_]{40,}", "a GitHub token"),
    (rb"xox[abpr]-[A-Za-z0-9-]{10,}", "a Slack token"),
    (rb"AIza[0-9A-Za-z_-]{35}", "a Google API key"),
    (rb"-----BEGIN [A-Z ]*PRIVATE KEY-----", "a PEM private key"),
]]
hits, files = [], 0
for base, _, names in os.walk(out):
    for name in sorted(names):
        f = os.path.join(base, name)
        rel = os.path.relpath(f, out)
        data = open(f, "rb").read()
        files += 1
        for v, label in needles:
            if v in data:
                hits.append(f"{rel}: {label}")
        for rx, label in shapes:
            if rx.search(data):
                hits.append(f"{rel}: {label} (shape)")
print(f"scan: {files} files, {len(needles)} secret values, {len(shapes)} token shapes")
if hits:
    open(os.path.join(out, "SCAN-FAILED.txt"), "w").write("\n".join(hits) + "\n")
    for h in hits:
        print("SCAN MATCH " + h)
    sys.exit(3)
PY
rc=$?
set -e
if [ $rc -ne 0 ]; then
  echo "proof export: REFUSED: a secret matched (list in $OUT/SCAN-FAILED.txt; values are not printed). Do not publish $OUT." >&2
  exit 3
fi

# ---- the key and head to publish apart from the files; the offline verify
read -r PUB <"$OUT/lineage/lineage.pub"
HEAD="$(/usr/bin/python3 -I -c 'import json,sys; l=[x for x in open(sys.argv[1]) if x.strip()]; r=json.loads(l[-1]); print(r["seq"], r["hash"])' "$OUT/lineage/lineage.jsonl")"
HEAD_SEQ="${HEAD%% *}"; HEAD_HASH="${HEAD#* }"
if [ -n "$IROOT" ]; then
  if ! env -i HOME="$GH" PATH=/usr/bin:/bin "$IROOT" audit verify --file "$OUT/lineage/lineage.jsonl" --pub "$PUB" \
       --head "$HEAD_HASH" >"$OUT/lineage/verify.txt" 2>&1; then
    cat "$OUT/lineage/verify.txt" >&2
    echo "proof export: the exported lineage does not verify; do not publish $OUT" >&2
    exit 4
  fi
fi
cat >"$OUT/PUBLISH.txt" <<EOF
Publish these two values in the release notes, apart from these files:
  lineage public key (genesis): $PUB
  lineage head: seq $HEAD_SEQ $HEAD_HASH

Anyone can then check the records offline with the init-root binary (v0.5.0 or later), no install needed:
  init-root audit verify --file lineage/lineage.jsonl --pub <the key from the release notes> --head <the head hash>

A valid result shows that no record was changed, reordered or removed after the Root signed it with that key. It does
not show that the binaries match the source, and it is only as good as the key's source: take the key from the release
notes, not from lineage/lineage.pub in this directory.
EOF
if command -v sha256sum >/dev/null; then SUM=(sha256sum); else SUM=(shasum -a 256); fi
(cd "$OUT" && find . -type f ! -name MANIFEST.sha256 | sort | sed 's|^\./||' | while read -r f; do "${SUM[@]}" "$f"; done) >"$OUT/MANIFEST.sha256"
echo "proof export: $OUT: $n session file(s), $nd self-change diff(s), lineage head seq $HEAD_SEQ$([ -n "$IROOT" ] && echo ", verified offline")"
