#!/bin/sh
# Install INIT from a prebuilt release bundle, no Rust needed (docs/ux-v1.md §3.2.3; lane UX-BUNDLE).
#
#   curl -fsSL "$INIT_DIST_URL/get.sh" | sh                       download the current bundle and install it
#   sh scripts/get.sh --from init-v0.2.0-preview-macos-arm64.tar.gz [--sha256 HEX] [-- <install.sh args>]
#   (Linux: init-v<version>-linux-arm64.tar.gz on aarch64, init-v<version>-linux-x86_64.tar.gz on x86_64)
#
#   --from PATH|URL   a local bundle (.tar.gz) or a URL; default "$INIT_DIST_URL/init-v<version>-<target>.tar.gz", the
#                     version from "$INIT_DIST_URL/latest.json" and the target from this machine
#   --sha256 HEX      the bundle's expected sha256; default: the <bundle>.sha256 file next to it (or latest.json's)
#   --unpack-only     verify and unpack, do not install
#   -- ARGS           passed to the bundle's scripts/install.sh, after --yes (for example --no-service, --disk-floor 10)
#
# Steps: macOS 14+ on Apple silicon or Linux on aarch64/x86_64, not root → download (curl) → sha256 → unpack into
# ~/.local/share/init/<version>/ (INIT_APP_DIR overrides ~/.local/share/init) → check every file against the
# bundle's SHA256SUMS → the bundle's scripts/install.sh --yes, which installs with the defaults (ROOT_HOME
# ~/.init-root, INIT_HOME ~/.init, the `init` command in ~/.local/bin, the LaunchAgent or the systemd user unit unless
# --no-service). The provider, key and model are chosen on the first run of `init` (the TUI's setup). Nothing here
# needs Rust, sudo or a key.
#
# INIT_DIST_URL defaults to the latest GitHub release of nagisanzenin/INIT-Agent. POSIX sh: it runs under sh, dash and
# zsh's sh mode.
set -eu
umask 022

DIST_URL="${INIT_DIST_URL:-https://github.com/nagisanzenin/INIT-Agent/releases/latest/download}"
FROM=""; WANT=""; UNPACK_ONLY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --from) FROM="$2"; shift 2 ;;
    --sha256) WANT="$2"; shift 2 ;;
    --unpack-only) UNPACK_ONLY=1; shift ;;
    --) shift; break ;;
    -h|--help) sed -n '2,23p' "$0"; exit 0 ;;
    *) echo "get.sh: unknown argument $1 (see --help)" >&2; exit 64 ;;
  esac
done

say() { printf '%s\n' "$*"; }
ok() { printf '  ok  %s\n' "$*"; }
die() { printf 'get.sh: %s\n' "$*" >&2; exit 1; }
T0=$(date +%s)

# ---------------------------------------------------------------- who and where
[ "$(id -u)" != 0 ] || die "run this as yourself, not as root (INIT never needs sudo)"
case "$(uname -s)" in
  Darwin)
    [ "$(uname -m)" = arm64 ] || die "this bundle is for Apple silicon (arm64); this Mac is $(uname -m)"
    MACOS="$(sw_vers -productVersion)"
    [ "${MACOS%%.*}" -ge 14 ] || die "macOS 14 or newer is needed (this is $MACOS)"
    [ -x /usr/bin/python3 ] && /usr/bin/python3 -c '' 2>/dev/null \
      || die "the Command Line Tools are needed (git, python3): run xcode-select --install, then this again"
    TARGET=macos-arm64; PLATFORM="macOS $MACOS arm64"
    sha256() { shasum -a 256 "$@"; }
    sha_check() { shasum -a 256 -c --quiet "$@"; } ;;
  Linux)
    case "$(uname -m)" in
      aarch64|arm64) TARGET=linux-arm64 ;;
      x86_64|amd64) TARGET=linux-x86_64 ;;
      *) die "no Linux bundle for $(uname -m) (aarch64 and x86_64 only)" ;;
    esac
    for t in git tar sha256sum; do command -v "$t" >/dev/null || die "$t is missing: install it with your package manager"; done
    [ -x /usr/bin/python3 ] || die "/usr/bin/python3 is missing: install python3 with your package manager"
    [ -x /usr/bin/bwrap ] || die "/usr/bin/bwrap is missing: INIT needs bubblewrap 0.13.0 or newer (docs/platforms.md, Linux install)"
    PLATFORM="Linux $(uname -r) $(uname -m)"
    sha256() { sha256sum "$@"; }
    sha_check() { sha256sum -c --quiet "$@"; } ;;
  *) die "INIT bundles exist for macOS and Linux only (this is $(uname -s); on Windows use WSL2, docs/platforms.md)" ;;
esac
APP="${INIT_APP_DIR:-$HOME/.local/share/init}"
mkdir -p "$APP"
say "INIT installer · $PLATFORM · $APP"

# ---------------------------------------------------------------- download (or a local file)
TMP="$(mktemp -d "$APP/.get.XXXXXX")"
trap 'rm -rf -- "$TMP"' EXIT INT TERM
fetch() { curl -fsSL --proto '=https' --tlsv1.2 --retry 2 "$1" -o "$2"; }
case "$FROM" in
  http://*) die "refusing a plain-http URL: $FROM" ;;
  https://*|"")
    command -v curl >/dev/null || die "curl is missing"
    if [ -z "$FROM" ]; then
      fetch "$DIST_URL/latest.json" "$TMP/latest.json" || die "cannot fetch $DIST_URL/latest.json (or use --from <bundle.tar.gz>)"
      LVER="$(sed -n 's/^ *"version": "\([0-9A-Za-z.-]*\)".*/\1/p' "$TMP/latest.json")"
      LTARGET="$(sed -n 's/^ *"target": "\([^"]*\)".*/\1/p' "$TMP/latest.json")"
      [ -n "$LVER" ] || die "$DIST_URL/latest.json names no version"
      FILE="init-v$LVER-$TARGET.tar.gz"
      # latest.json carries one target's checksum; any other target's comes from its .sha256 next to it
      [ -n "$WANT" ] || [ "$LTARGET" != "$TARGET" ] || WANT="$(sed -n 's/^ *"sha256": "\([0-9a-f]\{64\}\)".*/\1/p' "$TMP/latest.json")"
      FROM="$DIST_URL/$FILE"
    fi
    FILE="$(basename "$FROM")"
    say "  downloading $FILE"
    curl -fL --progress-bar --proto '=https' --tlsv1.2 --retry 2 "$FROM" -o "$TMP/$FILE" || die "download failed: $FROM"
    [ -n "$WANT" ] || { fetch "$FROM.sha256" "$TMP/$FILE.sha256" && WANT="$(cut -d' ' -f1 "$TMP/$FILE.sha256")"; } || true
    TGZ="$TMP/$FILE" ;;
  *)
    [ -f "$FROM" ] || die "no such bundle: $FROM"
    TGZ="$(cd "$(dirname "$FROM")" && pwd -P)/$(basename "$FROM")"
    FILE="$(basename "$FROM")"
    [ -n "$WANT" ] || { [ -f "$TGZ.sha256" ] && WANT="$(cut -d' ' -f1 "$TGZ.sha256")"; } || true ;;
esac

# ---------------------------------------------------------------- verify
case "$WANT" in
  [0-9a-f]*) [ ${#WANT} = 64 ] || die "the expected sha256 is not 64 hex digits" ;;
  *) die "no checksum to verify $FILE against: pass --sha256 HEX, or keep $FILE.sha256 next to it" ;;
esac
GOT="$(sha256 "$TGZ" | cut -d' ' -f1)"
[ "$GOT" = "$WANT" ] || die "checksum mismatch for $FILE: got $GOT, expected $WANT. Download it again."
ok "checksum matches ($(printf '%.16s' "$GOT")…)"

# ---------------------------------------------------------------- unpack
mkdir -p "$TMP/x"
tar -xzf "$TGZ" -C "$TMP/x" || die "cannot unpack $FILE"
TOP="$(ls "$TMP/x")"
[ "$(printf '%s\n' "$TOP" | wc -l | tr -d ' ')" = 1 ] && [ -f "$TMP/x/$TOP/VERSION" ] || die "$FILE is not an INIT bundle (one top directory with VERSION)"
case "$TOP" in *-"$TARGET") ;; *) die "$FILE is not a $TARGET bundle (its top directory is $TOP); download the one for this machine" ;; esac
VERSION="$(cat "$TMP/x/$TOP/VERSION")"
case "$VERSION" in *[!0-9A-Za-z.-]*|"") die "bad VERSION in the bundle: $VERSION" ;; esac
(cd "$TMP/x/$TOP" && sha_check SHA256SUMS) || die "the unpacked files do not match the bundle's SHA256SUMS"
DEST="$APP/$VERSION"
if [ -e "$DEST" ]; then
  if cmp -s "$DEST/SHA256SUMS" "$TMP/x/$TOP/SHA256SUMS" && (cd "$DEST" && sha_check SHA256SUMS); then
    ok "INIT $VERSION is already unpacked in $DEST (verified)"
  else
    die "$DEST exists and differs from this bundle; move it away (for example to the Trash), then run this again"
  fi
else
  mv "$TMP/x/$TOP" "$DEST"
  chmod -R a-w "$DEST" # read-only: the code INIT runs as you (docs/ux-v1.md §3.2.3 step 6)
  ok "unpacked INIT $VERSION in $DEST"
fi
say "  $(( $(date +%s) - T0 )) s so far"
[ $UNPACK_ONLY = 0 ] || { say "Unpacked only. Install: $DEST/scripts/install.sh --yes"; exit 0; }

# ---------------------------------------------------------------- install (non-interactive, defaults)
say ""
"$DEST/scripts/install.sh" --yes "$@" || die "install failed; fix the cause above, then run: $DEST/scripts/install.sh --yes $*"
RH="${INIT_ROOT_HOME:-$HOME/.init-root}"
BIN="${INIT_BIN_DIR:-$HOME/.local/bin}"
cat <<EOF

INIT $VERSION installed in $(( $(date +%s) - T0 )) s (after the download).
Start it:
  $BIN/init
The first run asks for the provider, your key (hidden) and the model, then opens the chat.
From a shell instead: $RH/bin/init-root keys set deepseek   (the key is never echoed or logged)
EOF
case ":$PATH:" in *":$BIN:"*) ;; *) say "($BIN is not on your PATH; for this terminal: export PATH=\"$BIN:\$PATH\")" ;; esac
