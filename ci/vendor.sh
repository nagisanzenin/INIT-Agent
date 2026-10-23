#!/usr/bin/env bash
# Regenerate the single repo-root vendor/ from all five workspaces at once.
#
#   ci/vendor.sh              resolve (updating a Cargo.lock only where a manifest changed) and vendor
#   ci/vendor.sh --locked     vendor exactly what the committed lockfiles say; fail if one is stale
#   ci/vendor.sh --no-delete  (combinable) add and update crates, but remove nothing from vendor/; use it
#                             while other lanes build against vendor/. Stale crates stay until a later
#                             run without it; nothing a lockfile needs is ever missing either way.
#
# Always sync all five manifests together: `cargo vendor` deletes every crate in vendor/ that
# the manifests it was given do not need, so vendoring one workspace alone breaks the others.
# Runs from the repo root, outside every workspace, so the workspaces' offline
# .cargo/config.toml files do not apply. Network: crates.io only (index + static).
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

locked=()
nodelete=()
for a in "$@"; do
  case "$a" in
    --locked) locked=(--locked) ;;
    --no-delete) nodelete=(--no-delete) ;;
    *) echo "usage: ci/vendor.sh [--locked] [--no-delete]" >&2; exit 64 ;;
  esac
done

# One vendoring at a time (lanes run in parallel). mkdir is atomic.
lock="$repo/.vendor.lock"
if ! mkdir "$lock" 2>/dev/null; then
  echo "ci/vendor.sh: another vendoring holds $lock; wait for it (or remove that exact dir if stale)" >&2
  exit 75
fi
trap 'rmdir "$lock"' EXIT

df -h / | tail -1
cargo vendor "${locked[@]}" "${nodelete[@]}" \
  --manifest-path kernel/Cargo.toml \
  --sync root/Cargo.toml \
  --sync seed/Cargo.toml \
  --sync seed/ext/Cargo.toml \
  --sync tui/Cargo.toml \
  vendor >/dev/null
echo "vendor/: $(find vendor -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ') crates, $(du -sh vendor | cut -f1)"
df -h / | tail -1
