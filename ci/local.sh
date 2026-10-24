#!/usr/bin/env bash
# Local CI (testing.md §3.12, first cut from WP0; WP17 extends it with fmt, clippy and tiers).
# Builds and tests the kernel, root and seed workspaces, the seed's extension workspace and the TUI
# workspace, offline against the repo-root vendor/. Exits non-zero if any step fails.
#
# Target dirs: one per workspace (roadmap.md §3.8.3). Override with KERNEL_TARGET_DIR,
# ROOT_TARGET_DIR, SEED_TARGET_DIR or TUI_TARGET_DIR (e.g. a worktree pointing at the main checkout's dirs).
# INIT_CI_TIER=full adds the TUI's latency and load run (TUI-13, release build).
# The ext workspace builds into the seed's target dir: its `members = ["*"]` glob would
# otherwise pick up seed/ext/target as a member and fail.
set -uo pipefail
# Disk budget: same flags every lane uses (ci/env.sh); callers may override.
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}"
export CARGO_PROFILE_TEST_DEBUG="${CARGO_PROFILE_TEST_DEBUG:-line-tables-only}"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
kernel_target="${KERNEL_TARGET_DIR:-$repo/kernel/target}"
root_target="${ROOT_TARGET_DIR:-$repo/root/target}"
seed_target="${SEED_TARGET_DIR:-$repo/seed/target}"

failed=()
step() { # name, dir, target dir, command...
  local name=$1 dir=$2 target=$3; shift 3
  echo "==> $name"
  # cargo reads .cargo/config.toml from the current directory, so run inside the workspace.
  if (cd "$dir" && CARGO_TARGET_DIR="$target" "$@"); then
    echo "ok: $name"
  else
    echo "FAILED: $name" >&2
    failed+=("$name")
  fi
}

df -h / | tail -1
if [ ! -d "$repo/vendor" ]; then
  echo "vendor/ is missing: run ci/vendor.sh --locked first" >&2
  exit 1
fi

echo "==> wit copy drift (gate.g1.wit_copy_drift)"
if cmp "$repo/wit/init-ext/v1/init-ext.wit" "$repo/seed/wit/init-ext/v1/init-ext.wit"; then
  echo "ok: wit copy identical"
else
  failed+=("wit copy drift")
fi

# The canonical constitution is the Root's embedded copy (CONVENTIONS §2); the seed copy and the
# context golden snapshot must stay byte-identical to it (build-deviations.md §1.2, WP13 D4).
echo "==> constitution copy drift"
constitution="$repo/root/crates/init-root/assets/CONSTITUTION.md"
for copy in "$repo/seed/src/boot/CONSTITUTION.md" "$repo/seed/src/context/testdata/head/CONSTITUTION.md"; do
  if cmp "$constitution" "$copy"; then
    echo "ok: ${copy#$repo/} identical"
  else
    failed+=("constitution drift: ${copy#$repo/}")
  fi
done

step "kernel build" "$repo/kernel" "$kernel_target" cargo build --offline
# The kernel's acceptance tests (WP15) run the seed's init-body and the Root's init-root: build both first, so a fresh
# checkout (or a fresh Linux container) passes without leftovers from an earlier run.
step "root build" "$repo/root" "$root_target" cargo build --offline
step "seed build" "$repo/seed" "$seed_target" cargo build --offline
step "kernel test" "$repo/kernel" "$kernel_target" env WP15_BODY_BIN="$seed_target/debug/init-body" \
  WP15_ROOT_BIN="$root_target/debug/init-root" cargo test --offline --no-fail-fast
step "root test" "$repo/root" "$root_target" cargo test --offline --no-fail-fast
# The Root's integration tests are gated on `test-fixtures` (fake clock, test ledgers; root/root.md §3.16).
# The egress tests use the kernel workspace's init-mockllm as their loopback upstream (WP22).
mock="$kernel_target/debug/init-mockllm"
if [ -x "$mock" ]; then
  step "root test (test-fixtures)" "$repo/root" "${ROOT_FX_TARGET_DIR:-$root_target}" env INIT_MOCKLLM_BIN="$mock" cargo test --offline --no-fail-fast -p init-root --features test-fixtures
else
  echo "note: $mock is missing; the init-mockllm egress test looks for another build or skips"
  step "root test (test-fixtures)" "$repo/root" "${ROOT_FX_TARGET_DIR:-$root_target}" cargo test --offline --no-fail-fast -p init-root --features test-fixtures
fi
# LINUX: the native zone qualification, never skipped on Linux (an incapable host fails it): bwrap >= 0.13.0 mount and
# seccomp confinement with a real compiler run (cc1), the sealed profile, pidfd STOP/KILL, and the memory watchdog
# killing one tool call behind the bwrap hops. INIT_TEST_CGROUP=1 (in a delegated cgroup v2 subtree) adds the
# memory.max backstop. They need the Root binary itself for exec-clean.
if [ "$(uname -s)" = Linux ]; then
  step "linux zone qualification (bwrap, seccomp, pidfd, memwatch)" "$repo/root" "$root_target" \
    env INIT_TEST_ROOT_BIN="$root_target/debug/init-root" cargo test --offline -p init-root --bin init-root --features test-fixtures \
    -- --ignored --test-threads=1 linux_
  if [ "${INIT_TEST_CGROUP:-0}" = 1 ]; then
    step "linux cgroup v2 memory.max" "$repo/root" "$root_target" cargo test --offline -p init-root --bin init-root \
      --features test-fixtures -- --ignored --test-threads=1 cgroup::tests::a_workload
  fi
fi
# The seed's provider and boot tests drive init-mockllm when it is built (else they skip, and say so).
step "seed test" "$repo/seed" "$seed_target" env INIT_MOCKLLM_BIN="$mock" cargo test --offline --no-fail-fast
step "ext build (wasm32-wasip2)" "$repo/seed/ext" "$seed_target" cargo build --offline --target wasm32-wasip2
step "ext test (host)" "$repo/seed/ext" "$seed_target" cargo test --offline --no-fail-fast
# The seed passes its own G0 (G0-SEED, $0): the seed cut into a body checkout as install packaging cuts it
# (seedpack.py), built by supervise's build command, then the real G0 runner (kernel-gate) with every command
# under the real Seatbelt kernel zone profile G0 runs in (INIT_SEED_G0_PROFILES=kernel,build adds gate.md's build
# profile) and the shipped init-mockllm. Warm target in $seed_target/g0 (about 1.3 GB); INIT_CI_SEED_G0=0 skips it.
# Then G1 on the same export (SELFJSON): the shipped init-kernel regenerates self.json in the build profile and it must
# equal the committed seed/self.json; on drift the step prints the regenerate command (INIT_SEED_G1_BLESS=1).
if [ "${INIT_CI_SEED_G0:-1}" = 1 ]; then
  step "seed G0 + G1 (the seed passes its own G0 and G1)" "$repo/kernel" "$kernel_target" env INIT_MOCKLLM_BIN="$kernel_target/debug/init-mockllm" \
    INIT_KERNEL_BIN="$kernel_target/debug/init-kernel" \
    INIT_SEED_G0_DIR="$seed_target/g0" cargo test --offline -p kernel-gate --test seed_g0 -- --ignored --nocapture
fi
# The promote pipeline under the real Seatbelt profiles (K-GATE, $0, macOS; about 1-2 min): A3's G0-G4 with every
# broker instance checked (V1-V5, class rules) and composed by the Root's own init-sandbox-profiles code, and G0 and $G
# under the kernel zone profile. It builds into the kernel target dir (no new target dir); its test homes under
# kernel/target/test-homes/sb are removed when it passes. The fast G1 check in the same file runs in "kernel test".
if [ "$(uname -s)" = Darwin ]; then
  step "promote under Seatbelt (A3, real profiles)" "$repo/kernel" "$kernel_target" \
    cargo test --offline -p kernel-supervise --test promote_seatbelt -- --ignored
else
  echo "==> promote under Seatbelt (A3): skipped, macOS only (Linux: the bwrap zone steps below)"
fi

# The TUI workspace (docs/tui.md §8): unit tests with the render goldens and the sanitizer fuzz (TUI-1…4),
# the fixture owner socket and line mode (TUI-5…10), a pty session (TUI-12).
tui_target="${TUI_TARGET_DIR:-$repo/tui/target}"
step "tui build" "$repo/tui" "$tui_target" cargo build --offline
step "tui test" "$repo/tui" "$tui_target" cargo test --offline --no-fail-fast --bin init-tui --test proto --test tty
# TUI-R1…R3 (owner-machine variant): the TUI against the real Root in test mode with its fake kernel and
# init-mockllm. The bins are frozen into tui/target/e2e-bins first: other steps rebuild the shared dirs.
fx_target="${ROOT_FX_TARGET_DIR:-$root_target}"
e2e="$tui_target/e2e-bins"
if (cd "$repo/root" && CARGO_TARGET_DIR="$fx_target" cargo build --offline -p init-root --features test-fixtures --bins) &&
  mkdir -p "$e2e" && cp "$fx_target/debug/init-root" "$fx_target/debug/init-root-fakekernel" "$mock" "$e2e/"; then
  step "tui e2e (real Root, test mode)" "$repo/tui" "$tui_target" env INIT_TUI_E2E=1 INIT_ROOT_BIN="$e2e/init-root" \
    INIT_ROOT_FAKEKERNEL_BIN="$e2e/init-root-fakekernel" INIT_MOCKLLM_BIN="$e2e/init-mockllm" cargo test --offline --test e2e_root
else
  echo "FAILED: tui e2e (the test-fixtures Root bins or $mock)" >&2
  failed+=("tui e2e bins")
fi
if [ "${INIT_CI_TIER:-fast}" = full ]; then
  step "tui release build" "$repo/tui" "$tui_target" cargo build --release --offline
  step "tui latency and load (TUI-13)" "$repo/tui" "$tui_target" env TUI13_BIN="$tui_target/release/init-tui" \
    cargo test --offline --test tty -- --ignored
fi

# Sealed pack s1 (M7 S1-CORE, docs/m7-design.md §3.1; Python >= 3.9, stdlib only): the pack lint (M7-P07 licences,
# M7-P08 disjointness and no pack copies under seed/ or fixtures/), the runner/validator/lint tests (M7-P01..P09
# and the runner's anti-gaming cases), then the template validator on every committed template (--quick;
# INIT_CI_TIER=full validates fully). Cargo checks build in per-check scratch target dirs that are deleted;
# scratch lives under target/test-homes/s1-core and is removed on exit. --tracked ignores lanes' uncommitted work.
step "sealed pack s1 lint" "$repo" "$root_target" python3 root/sealed/tools/s1lint.py --tracked
step "sealed pack s1 tests" "$repo" "$root_target" python3 -m unittest discover -s root/sealed/tools/tests
s1_mode=--quick
[ "${INIT_CI_TIER:-fast}" = full ] && s1_mode=--tracked
step "sealed pack s1 templates" "$repo" "$root_target" python3 root/sealed/tools/s1validate.py --tracked $s1_mode

df -h / | tail -1
if [ ${#failed[@]} -gt 0 ]; then
  echo "ci/local.sh: ${#failed[@]} step(s) failed: ${failed[*]}" >&2
  exit 1
fi
echo "ci/local.sh: all steps passed"
