#!/bin/bash
# Stage a commit of this repository's HEAD in a clone of the public repository (nagisanzenin/INIT-Agent), minus what
# scripts/public-exclude.txt keeps private. It never commits or pushes: you review `git -C MIRROR status`, then commit
# with the noreply identity and push (docs/release.md §4).
#
#   scripts/publish-public.sh MIRROR [--rev REV]
#
#   MIRROR   a clean clone of the public repository (`git clone https://github.com/nagisanzenin/INIT-Agent.git DIR`)
#   --rev    the commit to publish (default HEAD); only committed files are published (`git archive`)
#
# Steps: export REV with `git archive` into a temporary directory → rsync it over MIRROR's work tree with --delete
# (MIRROR's .git is never touched) → remove from MIRROR any path the exclude list keeps private → `git add -A` in
# MIRROR → scans of the staged tree: the DeepSeek key's first bytes (from ~/.config/init-agent/deepseek.key, compared
# without printing it), GitHub tokens, private-key headers, sk-/sk-ant-/AKIA keys, this repository's git e-mail and
# its origin's repository name (both read at run time, so neither is written in this public file) → a summary.
# Exit 1 on any scan hit, with nothing committed. A sanity scan for `init-agent` must hit, or the scans are broken.
set -euo pipefail
umask 022

REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
MIRROR=""; REV=HEAD
while [ $# -gt 0 ]; do
  case "$1" in
    --rev) REV="$2"; shift 2 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    -*) echo "publish-public.sh: unknown argument $1" >&2; exit 64 ;;
    *) MIRROR="$1"; shift ;;
  esac
done
die() { printf 'publish-public.sh: %s\n' "$*" >&2; exit 1; }
[ -n "$MIRROR" ] && [ -d "$MIRROR/.git" ] || die "MIRROR must be a clone of the public repository (see --help)"
MIRROR="$(cd "$MIRROR" && pwd -P)"
case "$(git -C "$MIRROR" remote get-url origin)" in
  *github.com/nagisanzenin/INIT-Agent.git|*github.com/nagisanzenin/INIT-Agent) ;;
  *) die "$MIRROR's origin is not the public repository" ;;
esac
[ -z "$(git -C "$MIRROR" status --porcelain)" ] || die "$MIRROR has uncommitted changes"
EXCL="$REPO/scripts/public-exclude.txt"
[ -f "$EXCL" ] || die "missing $EXCL"
COMMIT="$(git -C "$REPO" rev-parse --verify "$REV^{commit}")"

TMP="$(mktemp -d "${TMPDIR:-/tmp}/init-publish.XXXXXX")"
trap 'rm -rf -- "$TMP"' EXIT
git -C "$REPO" archive "$COMMIT" | tar -x -C "$TMP"
# The work tree follows the export; rules of the exclude list are applied on both sides, and .git is protected.
rsync -a --delete --checksum --filter='P /.git' --exclude=/.git --filter="merge $EXCL" "$TMP/" "$MIRROR/"
# Paths the exclude list keeps private that the public tree still has (published before the list existed): every
# tracked file that the filter does not pass is removed from the index and the work tree.
mkdir "$TMP/empty"
rsync -a --dry-run --exclude=/.git --filter="merge $EXCL" --out-format='%n' "$MIRROR/" "$TMP/empty/" | grep -v '/$' | LC_ALL=C sort >"$TMP/pass.txt"
git -C "$MIRROR" -c core.quotepath=off ls-files | LC_ALL=C sort >"$TMP/all.txt"
LC_ALL=C comm -23 "$TMP/all.txt" "$TMP/pass.txt" | while IFS= read -r f; do
  git -C "$MIRROR" rm -q --cached -- "$f" && rm -f -- "$MIRROR/$f"
done
find "$MIRROR" -mindepth 1 -depth -type d -empty ! -path "$MIRROR/.git" ! -path "$MIRROR/.git/*" -delete # the emptied dirs
git -C "$MIRROR" add -A

# ---------------------------------------------------------------- scans (counts only; nothing secret is printed)
HITS=0
count() { { git -C "$MIRROR" grep --cached -l "$@" -- . 2>/dev/null || true; } | wc -l | tr -d ' '; } # <git grep options, patterns>
scan() { # <label> <git grep options, patterns>: a hit fails the publish
  local n; n="$(count "${@:2}")"
  printf '  %-34s %s\n' "$1" "$n"
  [ "$n" = 0 ] || HITS=1
}
echo "scans of the staged tree (files with a hit):"
KEYFILE="${INIT_PUBLISH_KEYFILE:-$HOME/.config/init-agent/deepseek.key}"
if [ -s "$KEYFILE" ]; then scan "DeepSeek key prefix" -F -f <(head -c 8 "$KEYFILE")
else echo "  DeepSeek key prefix                (no key file; skipped)"; fi
scan "GitHub tokens" -E -e 'gh[pousr]_[A-Za-z0-9]{30,}' -e 'github_pat_[A-Za-z0-9_]{30,}'
scan "private-key headers" -E -e '-----BEGIN [A-Z ]*PRIVATE KEY-----'
scan "sk-/sk-ant-/AKIA keys" -E -e 'sk-ant-[A-Za-z0-9_-]{20,}' -e 'sk-[A-Za-z0-9]{32,}' -e 'AKIA[0-9A-Z]{16}'
EMAIL="$(git -C "$REPO" config user.email || true)"
[ -z "$EMAIL" ] || scan "this repository's git e-mail" -i -F -e "$EMAIL"
DEV="$(basename "$(git -C "$REPO" remote get-url origin 2>/dev/null || true)" .git)"
[ -z "$DEV" ] || [ "$DEV" = INIT-Agent ] || scan "the development repository's name" -F -e "$DEV"
[ "$(count -F -e init-agent)" != 0 ] || die "the sanity scan (init-agent) found nothing: the scans are not working"
echo "staged against $(git -C "$MIRROR" rev-parse --short HEAD): $(git -C "$MIRROR" diff --cached --name-status | awk '{c[$1]++} END {for (k in c) printf "%s %d  ", k, c[k]}')"
[ $HITS = 0 ] || die "a scan matched: inspect with git -C $MIRROR grep --cached, fix the source, publish again"
echo "ready: review git -C $MIRROR diff --cached --stat, then commit with the noreply identity and push"
