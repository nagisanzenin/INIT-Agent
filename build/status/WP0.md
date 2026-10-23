# WP0: repo skeleton, vendoring, wasm32-wasip2

Status: **done** (2026-10-20 about 02:05 +07).

## Done
- `kernel/` workspace (Rust 2024, resolver 3): lib crates `init-proto`, `kernel-supervise`, `kernel-proxy`, `kernel-gate`, `kernel-selfmap`, `kernel-serve`, `kernel-sandbox`; bins `initctl` (std + init-proto only) and `init-kernel` (std argv, no clap); `init-testkit` (`publish = false`, a lib plus the stub bins `init-mockllm`, `init-fakebody`, `init-fakekernel`, `init-fakeroot` and `init-measure`, which exit 69).
- `root/` workspace: bin `init-root` with `#![deny(unsafe_code)]` and the modules `charter` (R5), `lineage` (R4), `zone` (R1 spawn broker), `sup` (R1 supervision), `egress` (R2/R3), `ledger` (R7) and `sealed` (R6). The names follow root.md §4.8.
- `seed/` workspace: `init-body` per boot.md §3.16, with `#![forbid(unsafe_code)]` as the first line, `exclude = ["ext"]`, and dependencies at opt-level 2 in dev. `seed/ext/` follows ext/authoring.md §3.3 verbatim, with the template crate `count-lines` (cdylib, wit-bindgen =0.63.0; `src/lib.rs` is byte-identical to authoring.md §3.4). `seed/GENESIS.md` (1,333 bytes) and `seed/SELF.md` (79 lines) are copied from awareness.md §3.2 and §3.4.
- `wit/init-ext/v1/init-ext.wit` is the ext/wit.md §3.2 text. `seed/wit/init-ext/v1/init-ext.wit` is a byte-identical copy.
- The **full planned dependency set** is declared in the owning crates, so the lockfiles and `vendor/` are complete (the pins are in each workspace's `[workspace.dependencies]`).
  - **kernel:** serde, serde_json, toml, sha2, libc; tokio, hyper, hyper-util, http-body-util, tokio-tungstenite and the rustls stack (proxy); syn 3, proc-macro2, wit-component, wit-parser, tiktoken-rs, anyhow, indexmap and wasmtime/wasmtime-wasi (selfmap); landlock and seccompiler (sandbox, Linux only).
  - **root:** serde, serde_json, toml, sha2, ed25519-dalek, libc, ureq (rustls), httparse; seccompiler (Linux only).
- `vendor/` is at the repo root, with 290 crates (379 MB). It is not committed. `ci/vendor.sh [--locked]` regenerates it from **all four** manifests at once, under a mkdir lock.
- `ci/local.sh` builds and tests all four workspaces, checks the WIT copy for drift, and exits 1 on any failure.
- `rustup target add wasm32-wasip2` is done (101 MB).
- `measurements/wp0-build-times.md` records the build times.
- `build/status/.gitkeep` is added.

## Tests
- `ci/local.sh`: all steps pass, exit 0. Each step was also run on its own:
  - `cargo build --offline` and `cargo test --offline` in `kernel/`: 15 tests pass.
  - The same in `root/`: 8 tests pass.
  - The same in `seed/`: 2 tests pass, one of which starts a Wasmtime `Engine` and `Linker`.
  - In `seed/ext/` with `CARGO_TARGET_DIR=seed/target`: `cargo build --offline --target wasm32-wasip2` produces `count_lines.wasm` (247,697 bytes), and the host `cargo test` passes 3 tests.
- There were no compiler warnings.

## Deviations
1. **`seed/.cargo/config.toml`** reads `directory = "../vendor"` (the single repo-root vendor/ from BUILD_PLAN), not boot.md §3.16's `"vendor"`. CONVENTIONS §2 and the roadmap's WP0 row say `seed/vendor/`. The installed body still needs `directory = "vendor"` and its own `body/vendor/`, so install (WP4/WP13) must write that form.
2. **The ext workspace glob catches its own target dir.** With `members = ["*"]`, a `seed/ext/target/` directory becomes a "member" and every cargo command fails. This settles authoring.md's UNVERIFIED note: a non-crate subdirectory is a hard error. Ext builds must therefore set `CARGO_TARGET_DIR` outside `seed/ext/`; ci/local.sh uses `seed/target`, as roadmap §4.1 says. The installed body already does this (`work/target`).
3. **`seed/Cargo.toml` wasmtime lines** carry ext/host.md §3.2's features (`default-features = false`, …), as boot.md's comment points to.
4. **kernel-proxy declares rustls 0.23.46 (ring), hyper-rustls 0.27.10 and rustls-platform-verifier 0.7.1.** CONVENTIONS §5 and the orchestrator ask for them, but proxy.md v0.3 moves TLS to the Root (its Q-CV1). WP5 may drop them. tokio-tungstenite has no TLS features.
5. **Root egress uses ureq 3.4.2 and httparse 1.10.1**, as root/egress.md §6 specifies; that section explicitly rejects tokio + hyper. The brief had said tokio/hyper/rustls.
6. **Template crate placement.** The template is a real crate at `seed/ext/count-lines/`, as the brief asked. The docs ship it as `skills/ext-authoring/assets/template/Cargo.toml.template` (WP14), and a crate in `seed/ext/` is a seed extension. WP14 decides whether it stays.
7. **Dependency choices the docs leave open.** `nix` is not declared, because initctl.md makes it optional and libc is used. syn gets `full`, `parsing` and `visit`, while `span-locations` goes on proc-macro2 (selfmap.md §3.3). In `seed/Cargo.lock`, tokio is pinned to 1.53.3 so that vendor/ holds a single tokio.
8. **`.gitignore`:** `/vendor/` became `/vendor`, so that a `vendor` symlink in a worktree is ignored too, and `/.vendor.lock/` was added.

## Needs from others
- **Lanes in worktrees.** `../vendor` resolves inside the worktree, so each worktree needs `ln -s <main checkout>/vendor vendor` at its root. Ext builds need `CARGO_TARGET_DIR` set (deviation 2).
- **Vendoring.** A lane that really adds a crate must use `ci/vendor.sh`. Running `cargo vendor` on one manifest deletes the other workspaces' crates.
- **WP9.** wasmtime in `kernel-selfmap` builds at opt-level 0, because the kernel has no profile override. `selfmap --describe` compiles will be slower; WP9 may add a `[profile.dev.package]` override.

## Left
- WP0m (M-1/M-2 JSON on the real measurement subjects).
- `.github/workflows/ci.yml`, fmt and clippy tiers (WP17).
