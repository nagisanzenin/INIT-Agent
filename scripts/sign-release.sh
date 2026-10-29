#!/bin/bash
# Sign a release (v0.5.3, PLAN-052 L4; docs/release-signing.md; release-protocol.md §6). The release's SHA256SUMS lists
# the sha256 of every asset under the release's version and an expiry date; SHA256SUMS.sig is an OpenSSH signature of
# it (`ssh-keygen -Y sign -n init-release`) by the maintainer's offline ed25519 release key. get.sh checks it with
# `ssh-keygen -Y verify`, and the installed Root with the key compiled into it (`init-root release verify`).
#
#   scripts/sign-release.sh sums --version V [--valid-days N] DIR   SHA256SUMS over every other file in DIR (no key)
#   scripts/sign-release.sh sign --key KEY [--key KEY2] [--yes] DIR  SHA256SUMS.sig: the offline step
#   scripts/sign-release.sh check DIR                               verify it, as a user's get.sh and Root will
#
# DIR holds the release's assets exactly as they are uploaded: get.sh, every bundle with its .sha256, latest.json.
#   sums   checks that latest.json and every bundle name carry V, that each .sha256 matches its bundle, and that DIR's
#          get.sh pins a release key; then writes DIR/SHA256SUMS, valid for N days from today, UTC (default 180,
#          30 to 366). Run it where the assets are; it needs no key.
#   sign   shows what it signs and the key's fingerprint, asks (unless --yes), and writes DIR/SHA256SUMS.sig with
#          ssh-keygen (it asks for the key's passphrase itself). Several --key: one signature each, in that order, the
#          newest key first (a rotation, docs/release-signing.md §6; ssh-keygen -Y verify reads the first only). It
#          refuses a key that is not a regular file of yours with mode 0600 or 0400, that is not ed25519, or that lies
#          inside a git work tree, and warns about a cloud-synced folder. It never prints or copies key material: the
#          key file is read by ssh-keygen only. Then it runs `check`.
#   check  verifies SHA256SUMS.sig under the keys DIR's get.sh pins (ssh-keygen), every listed file's sha256, that
#          every file in DIR is listed, the version against latest.json, and the days left; and, where this machine
#          can run it, with the init-root inside this machine's bundle (its compiled-in key; a scratch ROOT_HOME).
#
# Nothing here touches the network, an INIT install or a GitHub setting.
set -euo pipefail
umask 077

die() { printf 'sign-release.sh: %s\n' "$*" >&2; exit 1; }
say() { printf '%s\n' "$*"; }
shq() { case "$1" in ''|*[!A-Za-z0-9/._+,:@%=-]*) printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")" ;; *) printf '%s' "$1" ;; esac; } # install.sh's
case "${1:-}" in
  "") sed -n '2,25p' "$0"; exit 64 ;;
  -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
esac
CMD="$1"; shift
VERSION=""; DAYS=180; KEYS=(); YES=0; DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --valid-days) DAYS="$2"; shift 2 ;;
    --key) KEYS+=("$2"); shift 2 ;;
    --yes) YES=1; shift ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    -*) die "unknown argument $1 (see --help)" ;;
    *) [ -z "$DIR" ] || die "one DIR only"; DIR="$1"; shift ;;
  esac
done
[ -n "$DIR" ] && [ -d "$DIR" ] || die "DIR (the release's assets) is missing or not a directory: ${DIR:-none}"
DIR="$(cd "$DIR" && pwd -P)"
SUMS="$DIR/SHA256SUMS"
PY=/usr/bin/python3
[ -x "$PY" ] || die "/usr/bin/python3 is needed"
KEYGEN="$(command -v ssh-keygen || true)"
[ -n "$KEYGEN" ] || die "ssh-keygen is needed (OpenSSH 8.1 or newer)"
if command -v shasum >/dev/null; then sha() { shasum -a 256 "$1" | cut -d' ' -f1; }; else sha() { sha256sum "$1" | cut -d' ' -f1; }; fi
T="$(mktemp -d "${TMPDIR:-/tmp}/sign-release.XXXXXX")"
trap 'rm -rf -- "$T"' EXIT

pinned() { # the RELEASE_KEYS lines of DIR's get.sh, the release's own (the file users run)
  [ -f "$DIR/get.sh" ] || die "$DIR/get.sh is missing: the release's get.sh is one of its assets"
  "$PY" -I - "$DIR/get.sh" <<'PY'
import re, sys
m = re.search(r"^RELEASE_KEYS='([^']*)'", open(sys.argv[1]).read(), re.M)
print("\n".join(l for l in (m.group(1) if m else "").splitlines() if l.strip()))
PY
}

assets() { # every file in DIR but SHA256SUMS and its signature, sorted (C locale); refuses anything else in DIR
  local n
  for n in "$DIR"/* "$DIR"/.[!.]*; do
    [ -e "$n" ] || continue
    n="${n##*/}"
    case "$n" in SHA256SUMS|SHA256SUMS.sig) continue ;; esac
    [ -f "$DIR/$n" ] && [ ! -L "$DIR/$n" ] || die "$DIR/$n is not a regular file: DIR holds the assets only"
    [[ "$n" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$ ]] || die "$n is not a plain asset name"
    printf '%s\n' "$n"
  done | LC_ALL=C sort
}

check() {
  [ -f "$SUMS" ] && [ -f "$SUMS.sig" ] || die "$SUMS or $SUMS.sig is missing"
  local keys n want got v exp left fail=0
  keys="$(pinned)"
  [ -n "$keys" ] || die "$DIR/get.sh pins no release key (RELEASE_KEYS is empty): pin it first (docs/release-signing.md §8)"
  printf '%s\n' "$keys" | sed -n 's|^\(ssh-ed25519 [A-Za-z0-9+/=]*\).*|init-release namespaces="init-release" \1|p' >"$T/allowed"
  "$KEYGEN" -Y verify -f "$T/allowed" -I init-release -n init-release -s "$SUMS.sig" <"$SUMS" >"$T/v.out" 2>&1 \
    || die "SHA256SUMS.sig does not verify under the key(s) get.sh pins: $(tr '\n' ' ' <"$T/v.out")"
  say "  ok  $(head -n 1 "$T/v.out")"
  [ "$(sed -n 1p "$SUMS")" = "# INIT release SHA256SUMS v1" ] || die "SHA256SUMS: not format v1"
  v="$(sed -n '2s/^# version: //p' "$SUMS")"; exp="$(sed -n '3s/^# expires: //p' "$SUMS")"
  while read -r want n; do
    [ -f "$DIR/$n" ] || { say "  FAIL $n is listed and missing"; fail=1; continue; }
    got="$(sha "$DIR/$n")"
    [ "$got" = "$want" ] || { say "  FAIL $n: sha256 $got, listed $want"; fail=1; }
  done < <(sed -n '4,$p' "$SUMS")
  diff <(assets) <(sed -n '4,$p' "$SUMS" | awk '{print $2}') >/dev/null || { say "  FAIL DIR and SHA256SUMS name different files"; fail=1; }
  [ $fail = 0 ] || die "the assets do not match SHA256SUMS"
  say "  ok  every asset matches SHA256SUMS (INIT $v)"
  [ "$("$PY" -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$DIR/latest.json")" = "$v" ] \
    || die "latest.json names another release than SHA256SUMS ($v)"
  left="$("$PY" -I -c 'import datetime,sys; print((datetime.date.fromisoformat(sys.argv[1]) - datetime.datetime.now(datetime.timezone.utc).date()).days)' "$exp")"
  [ "$left" -ge 0 ] || die "SHA256SUMS expired on $exp"
  [ "$left" -ge 30 ] || say "  WARN SHA256SUMS expires in $left days ($exp): get.sh refuses it from the day after"
  say "  ok  valid until $exp ($left days)"
  # the Root's own check, with the key compiled into the release's init-root, where this machine can run it
  local target tgz member
  case "$(uname -s)/$(uname -m)" in Darwin/arm64) target=macos-arm64 ;; Linux/x86_64) target=linux-x86_64 ;;
    Linux/aarch64) target=linux-arm64 ;; *) target="" ;; esac
  tgz="$DIR/init-v$v-$target.tar.gz"
  if [ -n "$target" ] && [ -f "$tgz" ]; then
    member="$(tar -tzf "$tgz" 2>/dev/null | grep -E '^[^/]+/init-root$' | head -n 1 || true)"
    if [ -z "$member" ]; then say "  WARN not checked by the release's own init-root: $tgz holds none"; return 0; fi
    mkdir -p "$T/x" "$T/rh"
    tar -xzf "$tgz" -C "$T/x" "$member"
    env -i HOME="$T" PATH=/usr/bin:/bin "$T/x/$member" --root-home "$T/rh" release verify --sums "$SUMS" --sig "$SUMS.sig" \
      --file "$tgz" >"$T/r.out" 2>&1 || die "the release's own init-root refuses it: $(cat "$T/r.out")"
    say "  ok  the release's own init-root ($target) verifies it with its compiled-in key: $(cat "$T/r.out")"
  else
    say "  --  not checked by the release's own init-root: this machine runs none of its bundles"
  fi
}

case "$CMD" in
  sums)
    [ -n "$VERSION" ] || die "sums needs --version V (the bundles' version, for example 0.5.3-preview)"
    [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || die "--version $VERSION is not a version"
    [[ "$DAYS" =~ ^[0-9]+$ ]] && [ "$DAYS" -ge 30 ] && [ "$DAYS" -le 366 ] || die "--valid-days takes 30 to 366"
    [ -n "$(pinned)" ] || die "$DIR/get.sh pins no release key (RELEASE_KEYS is empty): a release must pin it (docs/release-signing.md §8)"
    [ -f "$DIR/latest.json" ] || die "$DIR/latest.json is missing"
    lv="$("$PY" -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$DIR/latest.json")"
    [ "$lv" = "$VERSION" ] || die "latest.json names $lv, not $VERSION"
    n=0
    while IFS= read -r f; do
      case "$f" in
        init-v*.tar.gz)
          [[ "$f" == "init-v$VERSION-"*.tar.gz ]] || die "$f is not a bundle of $VERSION"
          [ -f "$DIR/$f.sha256" ] || die "$f.sha256 is missing"
          [ "$(cut -d' ' -f1 "$DIR/$f.sha256")" = "$(sha "$DIR/$f")" ] || die "$f.sha256 does not match $f"
          n=$((n + 1)) ;;
        init-v*) [[ "$f" == "init-v$VERSION-"*.tar.gz.sha256 ]] || die "$f is not a file of $VERSION" ;;
      esac
    done < <(assets)
    [ $n -ge 1 ] || die "$DIR holds no bundle init-v$VERSION-<target>.tar.gz"
    exp="$("$PY" -I -c 'import datetime,sys; print((datetime.datetime.now(datetime.timezone.utc).date() + datetime.timedelta(days=int(sys.argv[1]))).isoformat())' "$DAYS")"
    { printf '# INIT release SHA256SUMS v1\n# version: %s\n# expires: %s\n' "$VERSION" "$exp"
      while IFS= read -r f; do printf '%s  %s\n' "$(sha "$DIR/$f")" "$f"; done < <(assets); } >"$T/SHA256SUMS"
    [ ! -e "$SUMS.sig" ] || die "$SUMS.sig exists: it signs an older SHA256SUMS; move it away first"
    mv "$T/SHA256SUMS" "$SUMS" && chmod 644 "$SUMS"
    say "wrote $SUMS: INIT $VERSION, $n bundle(s), $(($(wc -l <"$SUMS") - 3)) assets, valid until $exp. Next, the offline step:"
    say "  scripts/sign-release.sh sign --key <the release key> $(shq "$DIR")" ;; # quoted: the checkout's path may hold a space
  sign)
    [ ${#KEYS[@]} -ge 1 ] || die "sign needs --key KEY (the offline release key; docs/release-signing.md §8)"
    [ -f "$SUMS" ] || die "$SUMS is missing: run sums first"
    [ ! -e "$SUMS.sig" ] || die "$SUMS.sig exists already; move it away to sign again"
    for k in "${KEYS[@]}"; do
      # the key file: yours, regular, readable by you alone, ed25519, outside any git work tree
      st="$("$PY" -I -c 'import os,stat,sys
s = os.lstat(sys.argv[1])
print("link" if stat.S_ISLNK(s.st_mode) else "file" if stat.S_ISREG(s.st_mode) else "other", "0%o" % (s.st_mode & 0o777), int(s.st_uid == os.getuid()))' "$k" 2>/dev/null)" \
        || die "$k: no such file"
      read -r kind mode mine <<<"$st"
      [ "$kind" = file ] || die "$k is a $kind, not a regular file: give the key file itself"
      [ "$mine" = 1 ] || die "$k is not owned by you"
      [ $((mode & 077)) = 0 ] || die "$k is readable or writable by others (mode ${mode#0}): run chmod 600 on it first. A release key that others can read is a release key others can use"
      d="$(cd "$(dirname "$k")" && pwd -P)"
      while :; do
        [ ! -e "$d/.git" ] || die "$k lies inside the git work tree $d: a key there can be committed by accident. Keep it on its offline volume (docs/release-signing.md §8)"
        [ "$d" != / ] || break
        d="$(dirname "$d")"
      done
      case "$(cd "$(dirname "$k")" && pwd -P)/" in
        "$HOME"/Documents/*|"$HOME"/Desktop/*|"$HOME"/Library/Mobile\ Documents/*|"$HOME"/Library/CloudStorage/*|"$HOME"/Dropbox/*)
          printf 'sign-release.sh: warning: %s is in a folder that is often synced to a cloud (iCloud Desktop & Documents, Dropbox)\n' "$k" >&2 ;;
      esac
      fp="$("$KEYGEN" -l -f "$k" 2>/dev/null)" || die "$k is not an OpenSSH key file ssh-keygen can read"
      case "$fp" in *"(ED25519)") ;; *) die "$k is not an ed25519 key: $fp" ;; esac
      say "key: $fp"
    done
    say "signing $SUMS:"
    sed 's/^/  | /' "$SUMS"
    if [ $YES = 0 ]; then
      printf 'Sign this with the key(s) above? [y/N] '
      read -r a </dev/tty || a=""
      case "$a" in y|Y|yes) ;; *) die "not signed" ;; esac
    fi
    : >"$T/all.sig"
    i=0
    for k in "${KEYS[@]}"; do
      i=$((i + 1)); mkdir -p "$T/s$i"; cp "$SUMS" "$T/s$i/SHA256SUMS"
      "$KEYGEN" -q -Y sign -f "$k" -n init-release "$T/s$i/SHA256SUMS" || die "ssh-keygen could not sign with $k" # a passphrase: it asks
      cat "$T/s$i/SHA256SUMS.sig" >>"$T/all.sig"
    done
    mv "$T/all.sig" "$SUMS.sig" && chmod 644 "$SUMS.sig"
    say "wrote $SUMS.sig"
    check ;;
  check) check ;;
  *) die "unknown command $CMD (sums, sign or check; see --help)" ;;
esac
