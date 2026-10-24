#!/bin/bash
# Install INIT on this Mac or Linux machine with one command (root/root.md §4.3; RELEASE_PLAN.md "What v0.1.0 means" item 1).
# Two modes: from a source checkout (builds with release.sh), or from an unpacked release bundle (scripts/bundle.sh;
# scripts/get.sh runs it there), detected by SHA256SUMS and body/BODY.json next to scripts/. The bundle mode installs
# the prebuilt body (verified bit for bit). get.sh installs the Rust toolchain first (rustup 1.97.1 + wasm32-wasip2,
# unless --no-rust), which this script records for self-evolution T1/T2; without a toolchain self-evolution is T0 only.
#
#   scripts/install.sh [--yes] [--no-service] [--rebuild | --reuse] [--with-tests] [--allow-dirty]
#                      [--disk-floor GIB] [--charter FILE]
#
#   --yes          take every wizard default, ask nothing (non-interactive)
#   --no-service   do not write or load the service (launchd on macOS, a systemd user unit on Linux); you run
#                  `init-root start` yourself
#   --rebuild      run scripts/release.sh even when target/dist/v<version>/ is fresh
#   --reuse        use target/dist/v<version>/ as it is (its files verified against MANIFEST.json), never rebuild
#   --with-tests   let release.sh run its release-mode test guard (root + seed); slower
#   --allow-dirty  allow building from a tree with uncommitted kernel/ root/ seed/ wit/ changes
#   --disk-floor N the charter's free-space floor in GiB (default 20, the owner's rule); asked by the wizard
#   --charter FILE use this charter instead of the wizard (it is still validated)
#   (bundle mode ignores --rebuild, --reuse, --with-tests and --allow-dirty: the bundle is the release)
#
# Paths (environment): INIT_ROOT_HOME (default ~/.init-root), INIT_HOME (default ~/.init), INIT_BIN_DIR (where the
# `init` command goes; default ~/.local/bin). Homes are at most 64 bytes long (the Root's sockets must fit sun_path).
#
# Steps: preflight → release binaries (release.sh, or reuse a fresh target/dist/v<version>/) → charter wizard →
# `init-root install --release … --charter … --seed-source <release>/seed/seed-v<version>.bundle` (the Root runs the
# kernel in install mode: it imports the seed, builds the body cold, and the Root forms the genesis tuple; WP21b,
# FIX4) → kernel/config.toml (model, prices, reference budgets) → the `init` command → the service (launchd, or systemd --user; loaded) →
# the next steps. This script never reads, writes or asks for a provider key: you run `init-root keys set deepseek`.
#
# Test hook (never for a real install): --mock-upstream 127.0.0.1:PORT writes a test-mode charter whose provider
# egress goes to a loopback init-mockllm (as scripts/slice1/s1.sh mock does). It requires --no-service.
set -euo pipefail
umask 077

REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
YES=0; NO_SERVICE=0; REBUILD=0; REUSE=0; WITH_TESTS=0; ALLOW_DIRTY=0; FLOOR=""; CHARTER_IN=""; MOCK=""
while [ $# -gt 0 ]; do
  case "$1" in
    --yes|-y) YES=1; shift ;;
    --no-service) NO_SERVICE=1; shift ;;
    --rebuild) REBUILD=1; shift ;;
    --reuse) REUSE=1; shift ;;
    --with-tests) WITH_TESTS=1; shift ;;
    --allow-dirty) ALLOW_DIRTY=1; shift ;;
    --disk-floor) FLOOR="$2"; shift 2 ;;
    --charter) CHARTER_IN="$2"; shift 2 ;;
    --mock-upstream) MOCK="$2"; shift 2 ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "install.sh: unknown argument $1 (see --help)" >&2; exit 64 ;;
  esac
done

say() { printf '%s\n' "$*"; }
step() { printf '\n==> %s\n' "$*"; }
die() { printf '\ninstall.sh: %s\n' "$*" >&2; exit 1; }
free_mb() { df -Pm "${HOME:-/}" | awk 'NR==2{print $4}'; }
OS="$(uname -s)"
# sha256 tools: shasum on macOS, coreutils sha256sum on Linux (same `<hash>  <file>` format)
if [ "$OS" = Darwin ]; then SHA_CHECK=(/usr/bin/shasum -a 256 -c --quiet); else SHA_CHECK=(sha256sum -c --quiet); fi
gt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 > b + 0) }'; } # decimal a > b, without bc
ge() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 >= b + 0) }'; }

T_START=$(date +%s)
PW_HOME="$(/usr/bin/python3 -I -c 'import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)' 2>/dev/null || true)"
HOME="${HOME:-$PW_HOME}"
# rustup and cargo keep their state in the passwd home, even when HOME is elsewhere (a test home)
export RUSTUP_HOME="${RUSTUP_HOME:-$PW_HOME/.rustup}" CARGO_HOME="${CARGO_HOME:-$PW_HOME/.cargo}"
RH="${INIT_ROOT_HOME:-$HOME/.init-root}"
IH="${INIT_HOME:-$HOME/.init}"
BIN_DIR="${INIT_BIN_DIR:-$HOME/.local/bin}"
BUNDLE=0
if [ -f "$REPO/SHA256SUMS" ] && [ -f "$REPO/body/BODY.json" ] && [ -f "$REPO/MANIFEST.json" ]; then
  BUNDLE=1 # an unpacked release bundle: the release dir is this directory
  VERSION="$(sed -n 's/^ "version": "\(.*\)",*$/\1/p' "$REPO/MANIFEST.json" | head -1)"
  DIST="$REPO"
else
  VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/root/Cargo.toml" | head -1)"
  DIST="$REPO/target/dist/v$VERSION"
fi

# ---------------------------------------------------------------- 1. preflight
step "1/7 preflight"
case "$OS" in
  Darwin)
    MACOS="$(sw_vers -productVersion)"
    [ "${MACOS%%.*}" -ge 14 ] || die "macOS 14 or newer is needed (this is $MACOS)"
    [ -x /usr/bin/sandbox-exec ] || die "/usr/bin/sandbox-exec is missing"
    [ -x /usr/bin/python3 ] || die "/usr/bin/python3 is missing: install the Command Line Tools (xcode-select --install)"
    PLATFORM="macOS $MACOS" ;;
  Linux)
    # The zone (docs/platforms.md §3.4): bubblewrap >= 0.13.0 at /usr/bin/bwrap (fd-pinned binds), util-linux unshare,
    # unprivileged user namespaces. The Root checks the same at install; these say how to fix it first.
    [ -x /usr/bin/python3 ] || die "/usr/bin/python3 is missing: install python3 (apt install python3 / dnf install python3)"
    [ -x /usr/bin/unshare ] || die "/usr/bin/unshare is missing: install util-linux"
    [ -x /usr/bin/bwrap ] || die "/usr/bin/bwrap is missing: INIT needs bubblewrap 0.13.0 or newer (see docs/platforms.md, Linux install)"
    BWV="$(/usr/bin/bwrap --version 2>/dev/null | awk '{print $NF}')"
    /usr/bin/python3 -I -c 'import sys; v=tuple(int(x) for x in sys.argv[1].split(".")[:3]); sys.exit(0 if v>=(0,13,0) else 1)' "${BWV:-0}" 2>/dev/null \
      || die "bubblewrap $BWV is too old: INIT needs 0.13.0 or newer at /usr/bin/bwrap for its fd-pinned mounts (see docs/platforms.md, Linux install)"
    /usr/bin/bwrap --unshare-user --unshare-pid --die-with-parent --ro-bind / / --proc /proc --dev /dev /bin/true 2>/dev/null \
      || die "bubblewrap cannot create user and pid namespaces here (unprivileged user namespaces disabled, an AppArmor/SELinux rule, or a container without them); see docs/platforms.md, Linux install"
    /usr/bin/unshare --user --map-root-user --net /bin/true 2>/dev/null \
      || die "unshare cannot create a user and network namespace here (needed to keep the zone off the network); see docs/platforms.md, Linux install"
    if [ $NO_SERVICE = 0 ] && ! systemctl --user show-environment >/dev/null 2>&1; then
      die "no systemd user session here (systemctl --user does not answer), so the service cannot be installed: pass --no-service and start the Root yourself (init-root start), or log in through a systemd session"
    fi
    DISTRO="$( (. /etc/os-release 2>/dev/null && printf '%s' "${PRETTY_NAME:-}") || true)" # a subshell: os-release sets VERSION
    PLATFORM="Linux $DISTRO $(uname -r) $(uname -m) · bwrap $BWV" ;;
  *) die "v$VERSION installs on macOS and Linux only (this is $OS)" ;;
esac
[ -n "$PW_HOME" ] || die "cannot read your home directory from the passwd database"
GITV="$(git --version | awk '{print $3}')"
/usr/bin/python3 -I -c 'import sys; v=tuple(int(x) for x in sys.argv[1].split(".")[:3]); sys.exit(0 if v>=(2,39,4) else 1)' "$GITV" \
  || die "git 2.39.4 or newer is needed (found $GITV)"
RUSTUP_CMD="curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.97.1 --target wasm32-wasip2 --component clippy"
if [ $BUNDLE = 1 ]; then
  # get.sh installs Rust 1.97.1 with wasm32-wasip2 before this runs (unless --no-rust). A rustup on PATH with 1.97.x and
  # wasm32-wasip2 is recorded for self-evolution T1/T2; anything else installs without a toolchain (T0: skills, prompts,
  # extension source).
  TC=none; TCV="no Rust toolchain (self-evolution T0 only)"
  if RUSTUP="$(command -v rustup)" && RUSTC="$("$RUSTUP" which rustc 2>/dev/null)" && [ -x "$RUSTC" ]; then
    case "$("$RUSTC" -V 2>/dev/null)" in
      rustc\ 1.97.*)
        if ! [ -x "$(dirname "$RUSTC")/cargo-clippy" ]; then # the gate's G0 runs cargo clippy
          TCV="Rust without clippy (self-evolution T0 only until: rustup component add clippy; init-root toolchain --set)"
        elif "$RUSTUP" target list --installed 2>/dev/null | grep -qx wasm32-wasip2; then
          TC="$(cd "$(dirname "$RUSTC")/.." && pwd -P)"; TCV="$("$RUSTC" -V)"
        else TCV="Rust without wasm32-wasip2 (self-evolution T0 only until: rustup target add wasm32-wasip2; init-root toolchain --set)"; fi ;;
      *) TCV="$("$RUSTC" -V 2>/dev/null) is not 1.97.x (self-evolution T0 only)" ;;
    esac
  fi
else
  command -v rustup >/dev/null || [ -x "$PW_HOME/.cargo/bin/rustup" ] || die "rustup not found: install Rust from https://rustup.rs (toolchain 1.97), or install a release bundle with scripts/get.sh, which installs Rust 1.97.1 for you"
  RUSTUP="$(command -v rustup || echo "$PW_HOME/.cargo/bin/rustup")"
  RUSTC="$("$RUSTUP" which rustc 2>/dev/null)" || die "rustup has no default toolchain: rustup default 1.97.1"
  case "$("$RUSTC" -V)" in rustc\ 1.97.*) ;; *) die "need rustc 1.97.x, found: $("$RUSTC" -V)" ;; esac
  "$RUSTUP" target list --installed 2>/dev/null | grep -qx wasm32-wasip2 || die "missing target: rustup target add wasm32-wasip2"
  TC="$(cd "$(dirname "$RUSTC")/.." && pwd -P)"; TCV="$("$RUSTC" -V)"
fi
for h in "$RH" "$IH"; do
  case "$h" in /*) ;; *) die "$h is not an absolute path" ;; esac
  [ ${#h} -le 64 ] || die "$h is ${#h} bytes; a home may be at most 64 (the Root's sockets must fit sun_path)"
done
[ -e "$RH/install.json" ] && die "INIT is already installed ($RH/install.json exists). In-place upgrade to v$VERSION is not supported yet: run the uninstall.sh of the current install (it moves both homes to the Trash; nothing is deleted), then run this again."
if [ -n "$MOCK" ]; then
  [ $NO_SERVICE = 1 ] || die "--mock-upstream is a test hook and requires --no-service"
  case "$MOCK" in 127.0.0.1:[0-9]*) ;; *) die "--mock-upstream takes 127.0.0.1:PORT" ;; esac
fi
if [ $NO_SERVICE = 0 ] && [ "$HOME" != "$PW_HOME" ]; then
  die "HOME ($HOME) is not your passwd home ($PW_HOME); the service needs the real HOME (or pass --no-service)"
fi
say "$PLATFORM · $TCV · git $GITV · free $(( $(free_mb) / 1024 )) GiB"
[ $BUNDLE = 1 ] && say "release bundle $REPO"
say "ROOT_HOME $RH"
say "INIT_HOME $IH"

# ---------------------------------------------------------------- 2. release binaries
step "2/7 release binaries (v$VERSION)"
fresh() { # <any>: the files match their recorded sha256; unless <any>, the manifest also names HEAD, a clean build
  # and a still-clean tree
  [ -f "$DIST/MANIFEST.json" ] || return 1
  if [ "${1:-}" != any ]; then
    [ -z "$(git -C "$REPO" status --porcelain -- kernel root seed wit $([ -f "$REPO/tui/Cargo.toml" ] && echo tui))" ] || return 1
  fi
  /usr/bin/python3 -I - "$DIST" "$(git -C "$REPO" rev-parse HEAD)" "${1:-}" <<'PY'
import hashlib, json, os, sys
d, head, any_ = sys.argv[1:4]
m = json.load(open(os.path.join(d, "MANIFEST.json")))
if any_ != "any" and (m.get("commit") != head or m.get("dirty")):
    sys.exit(1)
for f, e in m["files"].items():
    p = os.path.join(d, f)
    if not os.path.isfile(p) or hashlib.sha256(open(p, "rb").read()).hexdigest() != e["sha256"]:
        sys.exit(1)
PY
}
if [ $BUNDLE = 1 ]; then
  (cd "$DIST" && "${SHA_CHECK[@]}" SHA256SUMS) || die "the bundle's files do not match its SHA256SUMS; download it again"
  say "release bundle v$VERSION: every file matches SHA256SUMS"
elif [ ! -d "$REPO/vendor" ]; then
  say "vendor/ is missing (a fresh clone): ci/vendor.sh --locked downloads the pinned crates from crates.io"
  "$REPO/ci/vendor.sh" --locked || die "ci/vendor.sh --locked failed"
fi
if [ $BUNDLE = 1 ]; then
  :
elif [ $REUSE = 1 ]; then
  fresh any || die "--reuse: $DIST has no MANIFEST.json, or its files do not match it"
  say "reusing $DIST as it is (--reuse; built from $(sed -n 's/.*"commit": "\(.\{12\}\).*/\1/p' "$DIST/MANIFEST.json"), files verified)"
elif [ $REBUILD = 0 ] && fresh; then
  say "reusing $DIST (built from HEAD, files verified)"
else
  ARGS=()
  [ $WITH_TESTS = 1 ] || ARGS+=(--skip-tests)
  [ $ALLOW_DIRTY = 1 ] && ARGS+=(--allow-dirty)
  say "building with scripts/release.sh ${ARGS[*]:-} (the first build takes about 10 minutes on an idle machine; later builds reuse cargo's cache)"
  "$REPO/scripts/release.sh" ${ARGS[@]+"${ARGS[@]}"} || die "release build failed"
fi
SEED_BUNDLE="$(ls "$DIST"/seed/seed-v*.bundle | head -1)"
for f in init-root init-kernel initctl; do [ -x "$DIST/$f" ] || die "the release has no $f"; done
[ -f "$SEED_BUNDLE" ] || die "the release has no seed bundle"

# ---------------------------------------------------------------- 3. the charter wizard
step "3/7 charter (your limits; root/root.md §3.12)"
ask() { # <var> <prompt> <default>: one line from the terminal, the default on Enter or with --yes
  local v="$1" q="$2" d="$3" a=""
  if [ $YES = 1 ]; then a="$d"; say "$q [$d]: $d"
  else
    printf '%s [%s]: ' "$q" "$d" >/dev/tty
    IFS= read -r a </dev/tty || a=""
    a="${a:-$d}"
  fi
  printf -v "$v" '%s' "$a"
}
money_ok() { [[ "$1" =~ ^[0-9]{1,7}(\.[0-9]{1,6})?$ ]]; }
if [ $YES = 0 ] && ! { : </dev/tty; } 2>/dev/null; then die "no terminal for the wizard: pass --yes to take the defaults"; fi
WORK="$(mktemp -d "${TMPDIR:-/tmp}/init-install.XXXXXX")"
trap 'rm -rf -- "$WORK"' EXIT
CHARTER="$WORK/charter.toml"
if [ -n "$CHARTER_IN" ]; then
  cp "$CHARTER_IN" "$CHARTER"
  MODEL="$(sed -n 's/^\[actions\.providers\.models\."\(.*\)"\]$/\1/p' "$CHARTER" | head -1)"
  MODEL="${MODEL:-deepseek-flash}"
  DAY="$(sed -n 's/^day_usd = "\(.*\)".*/\1/p' "$CHARTER" | head -1)"; DAY="${DAY:-5.00}"
  FLOOR="$(sed -n 's/^disk_min_free_gib = \([0-9]*\).*/\1/p' "$CHARTER" | head -1)"; FLOOR="${FLOOR:-20}"
  say "using the charter $CHARTER_IN"
else
  say "Each question has a default in [brackets]; press Enter to take it."
  ask PURPOSE "What is INIT for (one line)" "My personal agent on this computer: it helps me with my work and improves itself, within my limits."
  while :; do ask DAY "Daily spending reference, USD (shown next to the estimated spend; not enforced: you stop INIT with /stop or /panic)" "5"; money_ok "$DAY" && gt "$DAY" 0 && break; say "  a positive amount like 5 or 2.50"; [ $YES = 1 ] && die "bad default"; done
  while :; do ask TOTAL "Total (lifetime) spending reference, USD (also not enforced)" "25"; money_ok "$TOTAL" && ge "$TOTAL" "$DAY" && break; say "  an amount at least the daily reference ($DAY)"; [ $YES = 1 ] && die "bad default"; done
  while :; do ask LEVEL "Highest autonomy level it may ever reach, 0-4 (it starts at 0 and must earn each level)" "4"; [[ "$LEVEL" =~ ^[0-4]$ ]] && break; say "  0, 1, 2, 3 or 4"; done
  while :; do ask PROVIDER "Model provider" "deepseek"; [ "$PROVIDER" = deepseek ] && break; say "  v$VERSION supports deepseek only"; done
  while :; do ask MODEL "Model (deepseek-flash or deepseek-v4-pro)" "deepseek-flash"; case "$MODEL" in deepseek-flash|deepseek-v4-pro) break ;; esac; say "  deepseek-flash or deepseek-v4-pro"; done
  while :; do ask FLOOR "Keep at least this much disk free, GiB" "${FLOOR:-20}"; [[ "$FLOOR" =~ ^[0-9]+$ ]] && [ "$FLOOR" -ge 1 ] && break; say "  a whole number of GiB, at least 1"; done
  /usr/bin/python3 -I - "$DIST/assets" "$CHARTER" "$PURPOSE" "$DAY" "$TOTAL" "$LEVEL" "$MODEL" "$FLOOR" "$MOCK" <<'PY'
import json, os, re, sys
assets, out, purpose, day, total, level, model, floor, mock = sys.argv[1:10]
t = open(os.path.join(assets, "charter-template.toml")).read()
const = open(os.path.join(assets, "CONSTITUTION.md")).read()
q = lambda s: json.dumps(s, ensure_ascii=False)  # a TOML basic string
fix = lambda x: f"{float(x):.2f}" if re.fullmatch(r"[0-9]+(\.[0-9]{1,2})?", x) else x
def sub(pat, rep):
    global t
    t, n = re.subn(pat, lambda _m: rep, t, count=1, flags=re.M)
    if n != 1:
        sys.exit(f"charter template: no match for {pat}")
sub(r'^text = "REPLACE ME[^"]*"$', "text = " + q(purpose))
sub(r"^max_level = 4\b", f"max_level = {int(level)}")
sub(r'^day_usd = "5\.00"$', f'day_usd = "{fix(day)}"')
sub(r'^total_usd = "25\.00"$', f'total_usd = "{fix(total)}"')
sub(r"^disk_min_free_gib = 20$", f"disk_min_free_gib = {int(floor)}")
# the model table: the header names the model; each price line present in the template gets that model's price
prices = {"deepseek-flash": {"input": 300000, "cache_read": 6000, "output": 1200000},
          "deepseek-v4-pro": {"input": 1320000, "cache_read": 44000, "output": 3960000}}[model]
m = re.search(r'^\[actions\.providers\.models\."deepseek-flash"\]\n(?:[^\[\n].*\n)*', t, re.M)
if not m:
    sys.exit("charter template: no [actions.providers.models.\"deepseek-flash\"] table")
block = m.group(0).replace('"deepseek-flash"', f'"{model}"', 1)
for k, v in prices.items():
    block = re.sub(rf"^{k}_musd_per_mtok = [0-9]+$", f"{k}_musd_per_mtok = {v}", block, flags=re.M)
t = t[:m.start()] + block + t[m.end():]
if mock:  # test hook: the egress forwards to a loopback init-mockllm (root.test_mode; scripts/slice1/s1.sh mock)
    sub(r'^destinations = \["api\.deepseek\.com:443"\]$', f'destinations = ["{mock}"]')
    sub(r'^paths = \[.*\]$', 'paths = ["chat/completions", "models", "user/balance"]')
    sub(r"^\[sandbox\]$", f'[actions.providers.providers.deepseek]\nhost = "{mock}"\n\n[sandbox]')
    t += "\n[root]\ntest_mode = true\n"
t += "[constitution]\ntext = '''\n" + const + "'''\n"
with open(out, "w") as f:
    f.write(t)
PY
  [ -n "$MOCK" ] && say "TEST MODE: the provider egress goes to the mock at $MOCK"
fi
"$DIST/init-root" --version >/dev/null || die "the release's init-root does not run"
set +e
CHECK="$(env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" INIT_HOME="$IH" PATH=/usr/bin:/bin "$DIST/init-root" charter check "$CHARTER" 2>&1)"
CRC=$?
set -e
[ $CRC = 0 ] || { say "$CHECK"; die "the charter is invalid (exit $CRC)"; }
say ""
printf '%s\n' "$CHECK" | grep -E '^(purpose\.text|autonomy\.max_level|money\.(day_usd|total_usd)|quotas\.disk_min_free_gib|actions\.providers\.(destinations|models\..*input.*)|root\.test_mode|charter_sha256) = ' | sed 's/^/  /'
if [ $YES = 0 ]; then
  printf '\nWrite this charter and install? [Y/n] ' >/dev/tty
  IFS= read -r ok </dev/tty || ok=n
  case "${ok:-y}" in y|Y|yes) ;; *) die "stopped; nothing was installed" ;; esac
fi
NEED_MB=$(( FLOOR * 1024 + 2000 )); [ $BUNDLE = 1 ] && NEED_MB=$(( FLOOR * 1024 + 500 )) # the bundle builds nothing
[ "$(free_mb)" -ge "$NEED_MB" ] || die "only $(free_mb) MB free; the floor is $FLOOR GiB and the install needs about $(( NEED_MB - FLOOR * 1024 )) MB more ($NEED_MB MB). Free some disk, or choose a lower floor (--disk-floor N)."

# ---------------------------------------------------------------- 4. init-root install (minutes: the cold body build)
PRE=()
if [ $BUNDLE = 1 ]; then
  step "4/7 init-root install (the prebuilt body, verified bit for bit: seconds)"
  BODY_SHA="$(sed -n 's/.*"sha256": "\([0-9a-f]\{64\}\)".*/\1/p' "$DIST/body/BODY.json" | head -1)"
  BODY_COMMIT="$(sed -n 's/.*"commit": "\([0-9a-f]\{40\}\)".*/\1/p' "$DIST/body/BODY.json" | head -1)"
  [ -n "$BODY_SHA" ] && [ -n "$BODY_COMMIT" ] || die "body/BODY.json has no sha256 or commit"
  PRE=(--prebuilt-body "$DIST/body/init-body" --prebuilt-sha256 "$BODY_SHA" --prebuilt-commit "$BODY_COMMIT")
else
  step "4/7 init-root install (the kernel builds the body from the seed: about 3-4 minutes)"
fi
SVC=(); [ $NO_SERVICE = 1 ] && SVC=(--no-service)
# HOME is your passwd home for every init-root call: the broker's class rules compare the kernel's HOME_REAL (the
# passwd home) with the Root's HOME. With --no-service nothing is written under it.
IENV=(env -i HOME="$PW_HOME" INIT_ROOT_HOME="$RH" INIT_HOME="$IH" PATH=/usr/bin:/bin)
LOG="$WORK/install.out"
T0=$(date +%s)
# --bin-dir: the Root write-denies the dir that holds `init`/`init-tui` to the zone in every mode (ADR-009 Z3a).
"${IENV[@]}" "$DIST/init-root" install --release "$DIST" --charter "$CHARTER" --toolchain "$TC" \
  --bin-dir "$BIN_DIR" --seed-source "$SEED_BUNDLE" ${PRE[@]+"${PRE[@]}"} ${SVC[@]+"${SVC[@]}"} >"$LOG" 2>&1 &
IPID=$!
LOWEST=$(free_mb)
while kill -0 $IPID 2>/dev/null; do
  sleep 1 # a prebuilt install takes seconds; a build prints its progress every second
  f=$(free_mb); [ "$f" -lt "$LOWEST" ] && LOWEST=$f
  kill -0 $IPID 2>/dev/null && printf '  … %3d s, free %d MB\r' $(( $(date +%s) - T0 )) "$f"
done
IRC=0; wait $IPID || IRC=$?
printf '\n'
if [ $IRC != 0 ]; then
  cat "$LOG" >&2
  tail -30 "$IH/state/kernel/logs/install-build.log" 2>/dev/null >&2 || true
  die "init-root install failed (exit $IRC) after $(( $(date +%s) - T0 )) s. Fix the cause above and run install.sh again."
fi
sed 's/^/  /' "$LOG"
say "  installed in $(( $(date +%s) - T0 )) s; lowest free space $LOWEST MB"
# The build slot is a cache that the next build resets (kernel/supervise.md §3.6.2); free it now (exact path).
if [ -e "$IH/cache/build/slot" ]; then rm -rf -- "$IH/cache/build/slot"; say "  build slot freed; free $(free_mb) MB"; fi

# ---------------------------------------------------------------- 5. kernel/config.toml (yours to edit)
step "5/7 kernel/config.toml (model, prices, reference budgets)"
/usr/bin/python3 -I - "$IH/kernel/config.toml" "$MODEL" "$DAY" <<'PY'
import os, re, sys
from decimal import Decimal, ROUND_DOWN
path, model, day = sys.argv[1:4]
old = open(path).read()
sb = re.search(r"^\[sandbox\]\n(?:[^\[\n].*\n?)*", old, re.M)  # the Root wrote [sandbox] = install.json; keep it verbatim
if not sb:
    sys.exit("kernel/config.toml has no [sandbox] table")
d = Decimal(day)
cents = lambda x: str(x.quantize(Decimal("0.01"), rounding=ROUND_DOWN))
req = min(Decimal("1.00"), d / 5)          # D6: the soft day is clamped to ceiling - request_max, so keep it small
task = min(Decimal("0.50"), d / 2)
prices = {"deepseek-flash": ('{ input = "0.30", cache_read = "0.006", output = "1.20" }', '{ input = "0.15", cache_read = "0.003", output = "0.60" }'),
          "deepseek-v4-pro": ('{ input = "1.32", cache_read = "0.044", output = "3.96" }', '{ input = "0.66", cache_read = "0.022", output = "1.98" }')}[model]
text = f"""# INIT kernel config (docs/kernel/config.md), written by scripts/install.sh. Edit freely, then restart the Root.
# The key, the day boundary and the spending references are in ROOT_HOME/charter.toml. [sandbox] must stay equal to
# ROOT_HOME/install.json, because the Root's broker compares them.
schema_version = 2

[model]
provider = "deepseek"
id = "{model}"

[budget]                 # soft budgets (config.md §4.3); the Root's money policy is observe-only (root/egress.md)
day_usd = "{cents(d)}"
task_usd = "{cents(task)}"
request_max_usd = "{cents(req)}"

{sb.group(0).rstrip()}

[providers.deepseek.schedule]
tz      = "+00:00"
default = "offpeak"
windows = [
  {{ days = "mon-fri", from = "01:00", to = "04:00", use = "peak" }},
  {{ days = "mon-fri", from = "06:00", to = "10:00", use = "peak" }},
]
default_dates_tz = "+08:00"
default_dates    = []

[providers.deepseek.models."{model}"]
max_output            = 393216
reserve_output_tokens = 65536
reference_window      = "peak"
prices.peak    = {prices[0]}
prices.offpeak = {prices[1]}
"""
tmp = path + ".new"
with open(tmp, "w") as f:
    f.write(text)
    f.flush()
    os.fsync(f.fileno())
os.chmod(tmp, 0o644)
os.replace(tmp, path)
PY
say "  $IH/kernel/config.toml: model $MODEL"

# ---------------------------------------------------------------- 6. the `init` command
step "6/7 the init command"
mkdir -p "$BIN_DIR"
sed "s|^INIT_ROOT_HOME_DEFAULT=.*|INIT_ROOT_HOME_DEFAULT=\"$RH\"|" "$REPO/scripts/init" >"$BIN_DIR/.init.new"
chmod 755 "$BIN_DIR/.init.new"
mv "$BIN_DIR/.init.new" "$BIN_DIR/init"
if [ -x "$DIST/init-tui" ]; then # docs/tui.md §9 item 3: next to the launcher, a new file then a rename
  cp "$DIST/init-tui" "$BIN_DIR/.init-tui.new"
  chmod 755 "$BIN_DIR/.init-tui.new"
  mv "$BIN_DIR/.init-tui.new" "$BIN_DIR/init-tui"
  say "  $BIN_DIR/init-tui (the TUI)"
fi
say "  $BIN_DIR/init (opens the TUI when it is installed, else init-root attach)"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) say "  note: $BIN_DIR is not on your PATH; add it: echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.$(basename "${SHELL:-bash}")rc" ;; esac

# ---------------------------------------------------------------- 7. the service
step "7/7 the Root service"
PLIST="$PW_HOME/Library/LaunchAgents/dev.init.root.plist"
UNIT="$PW_HOME/.config/systemd/user/init-root.service"
if [ $NO_SERVICE = 1 ]; then
  say "  skipped (--no-service). Start the Root yourself, in its own terminal:"
  say "    INIT_ROOT_HOME=\"$RH\" \"$RH/bin/init-root\" start --foreground"
elif [ "$OS" = Linux ]; then
  # LINUX: the systemd user unit init-root install wrote (Restart=always, Delegate=memory for the per-workload
  # memory.max). It runs while you are logged in; `loginctl enable-linger` keeps it running after logout.
  [ -f "$UNIT" ] || die "init-root install wrote no $UNIT"
  systemctl --user daemon-reload 2>/dev/null || die "systemctl --user is not available (no systemd user session); reinstall with --no-service and run init-root start yourself"
  systemctl --user enable --now init-root.service || die "systemctl --user enable --now init-root.service failed; see: journalctl --user -u init-root"
  for _ in $(seq 1 120); do [ -S "$RH/run/owner.sock" ] && break; sleep 0.25; done
  [ -S "$RH/run/owner.sock" ] || die "the Root did not open $RH/run/owner.sock within 30 s; see: journalctl --user -u init-root"
  say "  enabled $UNIT (systemd --user keeps the Root running and starts it at login; for after logout: loginctl enable-linger)"
  "${IENV[@]}" "$RH/bin/init-root" status 2>/dev/null | sed 's/^/  /' | head -12 || true
else
  [ -f "$PLIST" ] || die "init-root install wrote no $PLIST"
  launchctl bootout "gui/$(id -u)/dev.init.root" 2>/dev/null || true
  launchctl bootstrap "gui/$(id -u)" "$PLIST" || die "launchctl bootstrap $PLIST failed"
  for _ in $(seq 1 120); do [ -S "$RH/run/owner.sock" ] && break; sleep 0.25; done
  [ -S "$RH/run/owner.sock" ] || die "the Root did not open $RH/run/owner.sock within 30 s; see $RH/logs/launchd.log"
  say "  loaded $PLIST (launchd keeps the Root running and starts it at login)"
  "${IENV[@]}" "$RH/bin/init-root" status 2>/dev/null | sed 's/^/  /' | head -12 || true
fi

ELAPSED=$(( $(date +%s) - T_START ))
cat <<EOF

INIT v$VERSION is installed ($(( ELAPSED / 60 )) min $(( ELAPSED % 60 )) s).

Next steps (you do these; this script never touches your key):
  1. $RH/bin/init-root keys set deepseek
       paste your DeepSeek API key when asked (input is hidden); it is stored 0600 in $RH/keys/
  2. init
       opens the chat. Type a task. /stop stops the agent's current body (no demotion);
       /panic stops everything at once and demotes one level (then /resume).

Your limits: $RH/charter.toml (edit, then: init-root charter check --apply). Spending is shown, not capped:
             /usage in the chat or init-root egress stats; set a spend limit at your provider.
Logs:        $RH/logs/ (the Root), $IH/state/kernel/logs/ (the kernel and the body)
Uninstall:   $REPO/scripts/uninstall.sh (moves both homes to the Trash)

Access: FULL (default): the agent can read and write your files and use the network;
        INIT's Root ($RH) stays off-limits. To confine it: init --workspace, /full off,
        or sandbox.mode = "workspace" in $RH/charter.toml (docs/adr/012-full-access-by-default.md).
EOF
if [ "$TC" = none ]; then
  cat <<EOF

Self-evolution: T0 only (skills, prompts, extension source). Rebuilding itself (T1/T2) needs Rust 1.97.1 with
wasm32-wasip2. get.sh installs it unless you passed --no-rust (or rustup failed). To add it now:
  $RUSTUP_CMD
  $RH/bin/init-root stop; $RH/bin/init-root toolchain --set; $RH/bin/init-root start
EOF
fi
