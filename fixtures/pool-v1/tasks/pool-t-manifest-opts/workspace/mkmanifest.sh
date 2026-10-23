#!/bin/sh
# mkmanifest.sh: list the files of one or more directories (see README.md).
out=-
name=
exclude=
verbose=0
while getopts "o:nx:v" opt; do
  case $opt in
    o) out=$OPTARG ;;
    n) name=$OPTARG ;;
    x) exclude=$OPTARG ;;
    v) verbose=1 ;;
  esac
done
shift $OPTIND
[ -n "$name" ] || name=$(basename $1)
tmp=$(mktemp)
for d in $@; do
  find $d | while read f; do
    case $(basename "$f") in
      $exclude) ;;
      *) echo "$f" ;;
    esac
  done
done | sort > "$tmp"
{
  echo "# manifest: $name"
  cat "$tmp"
} > "$out"
[ $verbose = 1 ] && echo "$(wc -l < "$tmp") files" >&2
rm -f "$tmp"
