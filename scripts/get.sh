#!/bin/sh
# Install INIT from a prebuilt release bundle; it installs the Rust toolchain itself (docs/ux-v1.md §3.2.3; lane UX-BUNDLE).
#
#   curl -fsSL "$INIT_DIST_URL/get.sh" | sh                       download the current bundle and install it
#   sh scripts/get.sh --from init-v0.5.0-preview-macos-arm64.tar.gz [--sha256 HEX] [-- <install.sh args>]
#   (Linux: init-v<version>-linux-arm64.tar.gz on aarch64, init-v<version>-linux-x86_64.tar.gz on x86_64)
#
#   --from PATH|URL   a local bundle (.tar.gz) or a URL; default "$INIT_DIST_URL/init-v<version>-<target>.tar.gz", the
#                     version from "$INIT_DIST_URL/latest.json" and the target from this machine
#   --sha256 HEX      the bundle's expected sha256; default: the <bundle>.sha256 file next to it (or latest.json's)
#   --unpack-only     verify and unpack, do not install
#   --no-rust         skip the Rust toolchain (INIT then rewrites only its skills and prompts, tier T0)
#   --upgrade         INIT is installed: upgrade it to this bundle (docs/guide.md "Upgrade"). The download, its check and
#                     the unpacked copy go under ROOT_HOME/cache/upgrade/ (the agent cannot write ROOT_HOME in any access
#                     mode), then that copy's scripts/install.sh --upgrade runs. No Rust step. `init-agent upgrade` runs
#                     this; INIT 0.4.1 has no such command: curl -fsSL <get.sh URL> | sh -s -- --upgrade
#   --version V       that release (tag v<V>) instead of the latest
#   -- ARGS           passed to the bundle's scripts/install.sh, after --yes (for example --no-service, --disk-floor 10)
#
# Steps: macOS 14+ on Apple silicon or Linux on aarch64/x86_64, not root → download (curl) → sha256 → unpack into
# ~/.local/share/init/<version>/ (INIT_APP_DIR overrides ~/.local/share/init) → check every file against the
# bundle's SHA256SUMS → the bundle's scripts/install.sh --yes, which installs with the defaults (ROOT_HOME
# ~/.init-root, INIT_HOME ~/.init, the `init-agent` command in ~/.local/bin, the LaunchAgent or the systemd user unit unless
# --no-service). Self-rewrite is the point of INIT, so get.sh also makes sure the Rust toolchain it rebuilds itself
# with is present: rustup's toolchain 1.97.1 with the wasm32-wasip2 target and clippy, which the gate runs (installed with rustup if missing, without
# touching your shell files or your default toolchain). The provider, key and model are chosen on the first run of
# `init-agent` (the TUI's setup). Nothing here needs sudo or a key.
#
# INIT_DIST_URL defaults to the latest GitHub release of nagisanzenin/INIT-Agent. POSIX sh: it runs under sh, dash and
# zsh's sh mode.
set -eu
umask 022

DIST_URL="${INIT_DIST_URL:-https://github.com/nagisanzenin/INIT-Agent/releases/latest/download}"
FROM=""; WANT=""; UNPACK_ONLY=0; NO_RUST=0; UPGRADE=0; RELEASE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --from) FROM="$2"; shift 2 ;;
    --sha256) WANT="$2"; shift 2 ;;
    --unpack-only) UNPACK_ONLY=1; shift ;;
    --no-rust) NO_RUST=1; shift ;;
    --upgrade) UPGRADE=1; shift ;;
    --version) RELEASE="$2"; shift 2 ;;
    --) shift; break ;;
    -h|--help) sed -n '2,32p' "$0"; exit 0 ;;
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
RH="${INIT_ROOT_HOME:-$HOME/.init-root}"
case "$RELEASE" in
  "") ;;
  *[!0-9A-Za-z.-]*) die "--version takes a version such as 0.5.0 or 0.5.0-preview" ;;
  *) [ -n "${INIT_DIST_URL:-}" ] || DIST_URL="https://github.com/nagisanzenin/INIT-Agent/releases/download/v${RELEASE#v}" ;;
esac
if [ $UPGRADE = 1 ]; then
  [ -f "$RH/install.json" ] || die "INIT is not installed in $RH (no install.json): run this without --upgrade to install it"
  [ -d "$RH/cache" ] && [ ! -L "$RH/cache" ] || die "$RH/cache is not a directory"
  APP="$RH/cache/upgrade" # the download, its check and the unpacked release live where the zone cannot write
elif [ -f "$RH/install.json" ] && [ $UNPACK_ONLY = 0 ]; then
  die "INIT is already installed in $RH. Upgrade it instead, which keeps what INIT has become: init-agent upgrade (INIT 0.5.0 and later), or rerun this with --upgrade (curl -fsSL <get.sh URL> | sh -s -- --upgrade)"
fi
mkdir -p "$APP"
say "INIT installer · $PLATFORM · $APP"

# ---------------------------------------------------------------- download (or a local file)
TMP="$(mktemp -d "$APP/.get.XXXXXX")"
trap 'rm -rf -- "$TMP"' EXIT INT TERM
fetch() { curl -fsSL --proto '=https' --tlsv1.2 --retry 2 "$1" -o "$2"; }
tname() { # a bundle target as a platform name
  case "$1" in
    macos-arm64) echo "macOS 14+ on Apple silicon ($1)" ;; linux-arm64) echo "Linux aarch64 ($1)" ;;
    linux-x86_64) echo "Linux x86_64 ($1)" ;; *) echo "$1" ;;
  esac
}
no_bundle() { # <url> <http code>: the release has no bundle for this machine (until v0.5.0: every Linux x86_64 host)
  HAVE=""
  [ ! -f "$TMP/latest.json" ] || HAVE="$(sed -n 's/^ *"sha256_\([a-z0-9_-]*\)": .*/\1/p; s/^ *"target": "\([a-z0-9_-]*\)".*/\1/p' \
    "$TMP/latest.json" | sort -u)"
  case " $(echo $HAVE) " in *" $TARGET "*) # listed but missing: an upload in progress, or a broken release
    die "$1 answered HTTP $2, although latest.json lists a $TARGET bundle; the release may still be uploading. Try again in a few minutes, or use --from <bundle.tar.gz>" ;;
  esac
  {
    printf 'get.sh: there is no INIT bundle for this machine (%s, target %s) in this release:\n' "$PLATFORM" "$TARGET"
    printf '  %s answered HTTP %s.\n' "$1" "$2"
    if [ -n "$HAVE" ]; then
      printf '  This release%s has bundles for:\n' "${LVER:+ (INIT $LVER)}"
      for t in $HAVE; do printf '    %s\n' "$(tname "$t")"; done
    fi
    printf '  What you can do:\n'
    printf '    - install on %s, or\n' "$([ -n "$HAVE" ] && echo 'one of the platforms above' || echo 'a platform the release page lists')"
    printf '    - build from source here: clone the repository and run scripts/install.sh (Rust 1.97.1; on Linux also\n'
    printf '      bubblewrap >= 0.13, docs/platforms.md section 3.4.0)%s, or\n' \
      "$([ "$TARGET" = linux-x86_64 ] && echo '; on Linux x86_64 this compiles but is not tested yet')"
    printf '    - run this installer again when a %s bundle is released: %s\n' "$TARGET" "${DIST_URL%/latest/download}"
  } >&2
  exit 1
}
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
      # latest.json carries "sha256_<target>" per target (and "sha256" for its "target"); a target it does not
      # list takes the checksum from the .sha256 next to the bundle
      [ -n "$WANT" ] || WANT="$(sed -n "s/^ *\"sha256_$TARGET\": \"\([0-9a-f]\{64\}\)\".*/\1/p" "$TMP/latest.json")"
      [ -n "$WANT" ] || [ "$LTARGET" != "$TARGET" ] || WANT="$(sed -n 's/^ *"sha256": "\([0-9a-f]\{64\}\)".*/\1/p' "$TMP/latest.json")"
      FROM="$DIST_URL/$FILE"
    fi
    FILE="$(basename "$FROM")"
    say "  downloading $FILE"
    CODE=0
    HTTP="$(curl -fL --progress-bar --proto '=https' --tlsv1.2 --retry 2 -w '%{http_code}' "$FROM" -o "$TMP/$FILE")" || CODE=$?
    case "$CODE/$HTTP" in
      0/*) ;;
      22/404|22/410) no_bundle "$FROM" "$HTTP" ;;
      *) die "download failed: $FROM (curl exit $CODE, HTTP ${HTTP:-none}); check the connection, or use --from <bundle.tar.gz>" ;;
    esac
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
  elif [ $UPGRADE = 1 ]; then # our own staging copy under ROOT_HOME/cache/upgrade/: replaced (exact path)
    chmod -R u+w "$DEST" && rm -rf -- "$DEST" && mv "$TMP/x/$TOP" "$DEST" && chmod -R a-w "$DEST"
    ok "unpacked INIT $VERSION in $DEST (replacing an older copy of it)"
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
if [ $UPGRADE = 1 ]; then
  say ""
  "$DEST/scripts/install.sh" --upgrade --yes "$@" || exit $?
  exit 0
fi

# ---------------------------------------------------------------- the Rust toolchain INIT rebuilds itself with
RUST_TC=1.97.1
if [ $NO_RUST = 0 ]; then
  RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"; CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"; export RUSTUP_HOME CARGO_HOME
  CARGO_BIN="$CARGO_HOME/bin"
  RUSTUP="$(command -v rustup 2>/dev/null || true)"
  [ -n "$RUSTUP" ] || { [ -x "$CARGO_BIN/rustup" ] && RUSTUP="$CARGO_BIN/rustup"; } || true
  if [ -z "$RUSTUP" ]; then
    say "  ..  installing Rust $RUST_TC with rustup (self-rewrite needs it; about 1-2 min; --no-rust skips it)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --quiet --no-modify-path --profile minimal --component clippy --default-toolchain "$RUST_TC" --target wasm32-wasip2 \
        >"$TMP/rustup.log" 2>&1 \
      || { tail -5 "$TMP/rustup.log" >&2; die "installing Rust failed; rerun with --no-rust to install without self-rewrite (T0 only), or install rustup yourself"; }
    RUSTUP="$CARGO_BIN/rustup"
  else
    say "  ..  making sure rustup has Rust $RUST_TC with wasm32-wasip2 (your default toolchain is left alone)"
    "$RUSTUP" toolchain install "$RUST_TC" --profile minimal --component clippy --target wasm32-wasip2 --no-self-update >/dev/null 2>&1 \
      || die "rustup could not install toolchain $RUST_TC; rerun with --no-rust to skip self-rewrite"
  fi
  "$RUSTUP" target add --toolchain "$RUST_TC" wasm32-wasip2 >/dev/null 2>&1 || die "rustup could not add wasm32-wasip2"
  # the gate's G0 runs cargo clippy; a toolchain installed earlier with the minimal profile lacks it
  "$RUSTUP" component add --toolchain "$RUST_TC" clippy >/dev/null 2>&1 || die "rustup could not add clippy (the gate's G0 runs it)"
  ok "Rust $("$RUSTUP" run "$RUST_TC" rustc -V 2>/dev/null | awk '{print $2}') with wasm32-wasip2 and clippy: INIT can rewrite its tools (T1) and its core (T2)"
  # install.sh records the toolchain rustup resolves; pin it to the one above for this run only.
  PATH="$(dirname "$RUSTUP"):$PATH"; RUSTUP_TOOLCHAIN="$RUST_TC"; export PATH RUSTUP_TOOLCHAIN
else
  say "  --  no Rust (--no-rust): INIT will rewrite only its skills and prompts (T0). Later: install Rust $RUST_TC, then init-root toolchain --set"
fi

# ---------------------------------------------------------------- install (non-interactive, defaults)
say ""
"$DEST/scripts/install.sh" --yes "$@" || die "install failed; fix the cause above, then run: $DEST/scripts/install.sh --yes $*"
RH="${INIT_ROOT_HOME:-$HOME/.init-root}"
BIN="${INIT_BIN_DIR:-$HOME/.local/bin}"
cat <<EOF

INIT $VERSION installed in $(( $(date +%s) - T0 )) s (after the download).
Start it:
  $BIN/init-agent
The first run asks for the provider, your key (hidden) and the model, then opens the chat.
From a shell instead: $RH/bin/init-root keys set deepseek   (the key is never echoed or logged)
EOF
case ":$PATH:" in *":$BIN:"*) ;; *) say "($BIN is not on your PATH; for this terminal: export PATH=\"$BIN:\$PATH\")" ;; esac
