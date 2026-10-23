#!/bin/sh
# usage: calc.sh add|sub A B
case "$1" in
  add) echo $(($2 + $3)) ;;
  sub) echo $(($2 - $3)) ;;
  *) echo "usage: calc.sh add|sub A B" >&2; exit 2 ;;
esac
