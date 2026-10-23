#!/bin/sh
# mkmanifest.sh: list the files of one or more directories (see README.md).
usage() {
  echo "usage: mkmanifest.sh [-o OUTFILE] [-n NAME] [-x PATTERN]... [-v] DIR..." >&2
  exit 2
}
out=-
name=
verbose=0
nl='
'
excludes=
while getopts ":o:n:x:v" opt; do
  case $opt in
    o) out=$OPTARG ;;
    n) name=$OPTARG ;;
    x) excludes="$excludes$OPTARG$nl" ;;
    v) verbose=1 ;;
    *) usage ;;
  esac
done
shift $((OPTIND - 1))
[ $# -ge 1 ] || usage
for d in "$@"; do
  [ -d "$d" ] || { echo "mkmanifest.sh: not a directory: $d" >&2; exit 2; }
done
[ -n "$name" ] || name=$(basename -- "$1")
tmp=$(mktemp) || exit 1
for d in "$@"; do
  case $d in -*) fd="./$d" ;; *) fd=$d ;; esac  # find would read a leading "-" as an expression
  find "$fd" -type f | while IFS= read -r f; do
    case $d in -*) f=${f#./} ;; esac
    base=${f##*/}
    skip=0
    oldifs=$IFS
    IFS=$nl
    set -f
    for p in $excludes; do
      case $base in
        $p) skip=1 ;;
      esac
    done
    set +f
    IFS=$oldifs
    [ $skip = 1 ] || printf '%s\n' "$f"
  done
done | LC_ALL=C sort > "$tmp"
if [ "$out" = - ]; then
  { echo "# manifest: $name"; cat "$tmp"; }
else
  { echo "# manifest: $name"; cat "$tmp"; } > "$out"
fi
if [ $verbose = 1 ]; then
  n=$(wc -l < "$tmp" | tr -d ' ')
  echo "$n files" >&2
fi
rm -f "$tmp"
exit 0
