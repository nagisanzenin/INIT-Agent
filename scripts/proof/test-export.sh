#!/bin/bash
# $0 test of scripts/proof/export.sh (lane HYGIENE) on a synthetic mock-model install: a ROOT_HOME and an INIT_HOME
# with the paths the export reads (sessions, lineage, keys, the spend ledger, body.git with one self-change by the
# agent identity). Checks the export, then that each planted secret stops it without printing the value.
# Everything lives in one mktemp directory, removed by its exact path. No network, no INIT process.
#   scripts/proof/test-export.sh
set -euo pipefail
umask 077
REPO="$(cd "$(dirname "$0")/../.." && pwd -P)"
EXPORT="$REPO/scripts/proof/export.sh"
T="$(mktemp -d "${TMPDIR:-/tmp}/i5-hyg-exp.XXXXXX")"
trap 'rm -rf -- "$T"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }

RH="$T/rh"; IH="$T/ih"
mkdir -p "$RH/lineage" "$RH/keys/providers" "$RH/egress/usage" "$IH/state/sessions" "$IH/lineage" "$T/gh"
KEY="test-0123456789abcdef0123"
printf '%s\n' "$KEY" >"$RH/keys/providers/deepseek"
head -c 32 /dev/urandom >"$RH/keys/lineage.ed25519"
PUB="$(printf '%064d' 7)"
printf '%s\n' "$PUB" >"$RH/keys/lineage.pub"

# body.git: the seed, then one change by the agent identity
G=(env -i HOME="$T/gh" PATH=/usr/bin:/bin GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null git)
"${G[@]}" init -q --bare "$IH/lineage/body.git"
"${G[@]}" init -q -b main "$T/w"
printf 'fn main() {}\n' >"$T/w/main.rs"
"${G[@]}" -C "$T/w" add -A
"${G[@]}" -C "$T/w" -c user.name='INIT seed' -c user.email=seed@init.local commit -q -m "seed"
SEED="$("${G[@]}" -C "$T/w" rev-parse HEAD)"
printf 'fn main() {\n    println!("{}", 1);\n}\n' >"$T/w/main.rs"
"${G[@]}" -C "$T/w" -c user.name='INIT Agent' -c user.email=agent@init.local commit -q -a -m "feat(main): print one

Why: the demo's self-change.
Compensates-for: none
Prefix-change: no"
CHANGE="$("${G[@]}" -C "$T/w" rev-parse HEAD)"
"${G[@]}" -C "$T/w" push -q "$IH/lineage/body.git" main

# the lineage (the export copies it; the shape is the Root's, the signatures are not checked here)
z="sha256:$(printf '%064d' 0)"
cat >"$RH/lineage/lineage.jsonl" <<EOF
{"at":"2026-10-27T12:00:00.000Z","by":"root","data":{"install_id":"00","pubkey":"$PUB","root_version":"0.5.0"},"hash":"sha256:$(printf '%064d' 1)","kind":"genesis","prev":"$z","seq":0,"sig":"ed25519:00","v":1}
{"at":"2026-10-27T12:01:00.000Z","by":"kernel","data":{"from_sha":"$SEED","group":1,"scope":"t2","sha":"$CHANGE"},"hash":"sha256:$(printf '%064d' 2)","kind":"change_open","prev":"sha256:$(printf '%064d' 1)","seq":1,"sig":"ed25519:00","v":1}
{"at":"2026-10-27T12:02:00.000Z","by":"kernel","data":{"from_sha":"$SEED","lkg":false,"scope":"t2","sha":"$CHANGE"},"hash":"sha256:$(printf '%064d' 3)","kind":"promotion","prev":"sha256:$(printf '%064d' 2)","seq":2,"sig":"ed25519:00","v":1}
EOF
printf '{"hash":"sha256:%064d","seq":2}\n' 3 >"$RH/lineage/head.json"
printf '{"id":"e1","kind":"owner","text":"Show how long each command took."}\n{"id":"e2","kind":"assistant","text":"Done: exit 0, 1 lines, 0.41s"}\n' >"$IH/state/sessions/20261027-120000-a1b2c3.jsonl"
printf '{"model":"mock","musd":0,"provider":"mock","tokens_in":120,"tokens_out":12}\n' >"$RH/egress/usage/2026-10-27.jsonl"

# 1. a clean export
"$EXPORT" --root-home "$RH" --init-home "$IH" --out "$T/out" >"$T/log" 2>&1 || { cat "$T/log"; fail "a clean export failed"; }
for f in session/20261027-120000-a1b2c3.jsonl lineage/lineage.jsonl lineage/head.json lineage/lineage.pub \
         spend/usage/2026-10-27.jsonl PUBLISH.txt MANIFEST.sha256; do
  [ -f "$T/out/$f" ] || fail "missing $f"
done
D="$T/out/diffs/000001-change_open-${CHANGE:0:12}.diff"
[ -f "$D" ] || fail "no diff for the change_open record ($(ls "$T/out/diffs"))"
[ "$(ls "$T/out/diffs" | wc -l | tr -d ' ')" = 1 ] || fail "the promotion of the same sha is one diff, not two: $(ls "$T/out/diffs")"
grep -q "Author: INIT Agent <agent@init.local>" "$D" || fail "the diff names the agent's commit"
grep -q '^+    println!("{}", 1);' "$D" || fail "the diff carries the change"
grep -q "Compensates-for: none" "$D" || fail "the diff carries the trailers"
grep -q "lineage public key (genesis): $PUB" "$T/out/PUBLISH.txt" || fail "PUBLISH.txt names the key"
grep -q "lineage head: seq 2 sha256:$(printf '%064d' 3)" "$T/out/PUBLISH.txt" || fail "PUBLISH.txt names the head"
if command -v sha256sum >/dev/null; then (cd "$T/out" && sha256sum -c --quiet MANIFEST.sha256); else (cd "$T/out" && shasum -a 256 -c --quiet MANIFEST.sha256); fi \
  || fail "MANIFEST.sha256 does not check"
[ ! -e "$T/out/SCAN-FAILED.txt" ] || fail "a clean export has no scan failure"
cmp -s "$RH/lineage/lineage.jsonl" "$T/out/lineage/lineage.jsonl" || fail "the lineage is a byte copy"
[ -f "$RH/keys/providers/deepseek" ] && [ -f "$IH/state/sessions/20261027-120000-a1b2c3.jsonl" ] || fail "the install is untouched"
ok "clean export: $(tail -1 "$T/log")"

# 2. an existing --out is refused
set +e; "$EXPORT" --root-home "$RH" --init-home "$IH" --out "$T/out" >/dev/null 2>&1; rc=$?; set -e
[ $rc = 2 ] || fail "an existing --out: exit $rc, want 2"
ok "an existing --out is refused"

# 3. each planted secret stops the export (exit 3) and its value is never printed
plant() { # <name> <file in the install> <text> [export args...]
  local name="$1" file="$2" text="$3"; shift 3
  cp -p "$file" "$T/saved"
  printf '%s\n' "$text" >>"$file"
  set +e; "$EXPORT" --root-home "$RH" --init-home "$IH" --out "$T/o-$name" "$@" >"$T/log-$name" 2>&1; rc=$?; set -e
  cp -p "$T/saved" "$file"
  [ $rc = 3 ] || { cat "$T/log-$name"; fail "$name: exit $rc, want 3"; }
  [ -f "$T/o-$name/SCAN-FAILED.txt" ] || fail "$name: no SCAN-FAILED.txt"
  [ ! -f "$T/o-$name/MANIFEST.sha256" ] || fail "$name: a refused export has no MANIFEST"
  if grep -rqF -- "$text" "$T/log-$name" "$T/o-$name/SCAN-FAILED.txt"; then fail "$name: the value was printed"; fi
  ok "$name: refused ($(head -1 "$T/o-$name/SCAN-FAILED.txt"))"
}
S="$IH/state/sessions/20261027-120000-a1b2c3.jsonl"
plant provider-key "$S" "{\"text\":\"my key is $KEY\"}"
grep -q "session/20261027-120000-a1b2c3.jsonl: the provider or action key keys/providers/deepseek" "$T/o-provider-key/SCAN-FAILED.txt" \
  || fail "provider-key: the match names the file and the key's source"
SEEDHEX="$(od -An -tx1 -v "$RH/keys/lineage.ed25519" | tr -d ' \n')"
plant signing-key "$RH/egress/usage/2026-10-27.jsonl" "{\"note\":\"$SEEDHEX\"}"
TOKSHAPE="sk-$(printf '%s' ABCDEFGHIJKLMNOPQRSTUVWXYZ012345)" # built at run time, so no key-shaped literal is committed
plant token-shape "$RH/egress/usage/2026-10-27.jsonl" "{\"note\":\"$TOKSHAPE\"}"
printf 'owner-key-%s\n' 0123456789 >"$T/ownerkey"
plant secret-file "$S" '{"text":"owner-key-0123456789"}' --secret-file "$T/ownerkey"
plant deny "$S" '{"text":"write to owner@example.com"}' --deny owner@example.com
echo "all export checks passed"
