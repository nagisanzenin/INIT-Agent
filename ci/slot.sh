#!/usr/bin/env bash
# Run one heavy command (cargo build/test, an install, a scenario) inside a shared build slot, so that
# parallel lanes don't overload the machine (2026-10-20: load 274, 6 GB swap).
#
#   ci/slot.sh cargo test --offline -p init-root
#   INIT_SLOT_POOL=real ci/slot.sh scripts/slice1/s1.sh ...   # the real-model lane's reserved slot
#
# Pools: "build" has INIT_SLOTS slots (default 3); "real" has 1; "light" has INIT_LIGHT_SLOTS (default 2) for
# template/pool validators (many short processes; run them niced so the build pool keeps the CPU). A slot is a directory under
# .build-slots/ that holds the owner's PID. A slot whose PID no longer exists is reclaimed
# (checked with `kill -0`, which sends no signal). The slot is released on exit.
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pool="${INIT_SLOT_POOL:-build}"
case "$pool" in
  build) n="${INIT_SLOTS:-3}" ;;
  real) n=1 ;;
  light) n="${INIT_LIGHT_SLOTS:-2}" ;;
  *) echo "ci/slot.sh: unknown pool $pool" >&2; exit 64 ;;
esac
[ $# -gt 0 ] || { echo "usage: ci/slot.sh <command...>" >&2; exit 64; }
mkdir -p "$repo/.build-slots"

held=""
waited=0
while [ -z "$held" ]; do
  for i in $(seq 1 "$n"); do
    d="$repo/.build-slots/$pool-$i"
    if mkdir "$d" 2>/dev/null; then
      echo $$ > "$d/pid"; held="$d"; break
    fi
    owner="$(cat "$d/pid" 2>/dev/null || true)"
    if [ -n "$owner" ] && ! kill -0 "$owner" 2>/dev/null; then
      # stale: the holder exited without releasing
      rm -f "$d/pid" && rmdir "$d" 2>/dev/null || true
    fi
  done
  if [ -z "$held" ]; then
    [ $((waited % 60)) -eq 0 ] && echo "ci/slot.sh: waiting for a $pool slot (${waited}s)" >&2
    sleep 5; waited=$((waited + 5))
  fi
done
trap 'rm -f "$held/pid"; rmdir "$held" 2>/dev/null' EXIT
"$@"
