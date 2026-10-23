# WP0 build times: first cut of M-1 / M-2 (2026-10-20, about 01:55-02:00 +07)

These numbers come from the WP0 **skeleton**: stub crates with the full planned dependency set
declared and vendored. Clean builds are therefore realistic, because they are dominated by
dependencies. Incremental numbers measure only the cargo, rustc and link floor on tiny crates.
The definitive runs are WP0m (`measurements/m1.json`, `m2.json`, testing.md §3.11), whose M-1
subject is a ~200-line seed that constructs an `Engine`, `Linker` and `Component`, and whose
M-2 subject is the 150-line `fixtures/ext/semver-sort/`.

## Environment

| Item | Value |
|---|---|
| Machine | Apple M2, 8 cores, 16 GiB, macOS 14.6 (23G80) |
| Toolchain | rustc 1.97.1 (8bab26f4f 2026-07-14), cargo 1.97.1; `wasm32-wasip2` installed by WP0 (101 MB) |
| Load | load averages 15.5 / 19.0 / 12.3 at 02:00: other sessions were running, so the times are pessimistic |
| Method | `/usr/bin/time -p` wall time; `cargo build --offline` run inside each workspace (vendored, offline); n = 1 for clean builds, n = 3-5 for incremental builds |

## Results

| Workspace | Clean build (empty target dir) | Incremental: `touch` one file, rebuild | No-op | `cargo test` after build | Target dir |
|---|---|---|---|---|---|
| `seed/` (`init-body`; wasmtime 48.0.6, deps at opt-level 2) | **116.6 s** (user 727 s) | `src/main.rs`: 0.47, 0.25, 0.25 s (median 0.25) | 0.15 s | 1.0 s, 2 tests pass | 786 MB (938 MB with the ext builds below) |
| `seed/ext/` (`count-lines` template, `wasm32-wasip2`) | **32.9 s** in its own empty target dir; 28.8 s into `seed/target` | `touch lib.rs`: 0.21 s ×5 (median 0.21, max 0.21); comment edit: 0.33, 0.21 ×4; string-literal edit: 0.33, 0.23, 0.23 | 0.10 s | host `cargo test`: 2.95 s, 3 tests pass | 138 MB own dir; `.wasm` 247,697 bytes |
| `root/` (`init-root`) | **8.5 s** | `src/main.rs`: 0.19, 0.19, 0.20 s | — | 0.49 s, 8 tests pass | 208 MB |
| `kernel/` (10 crates, wasmtime in `kernel-selfmap`) | **54.6 s** (user 265 s) | `init-kernel/src/main.rs`: 0.48, 0.27, 0.27 s; `init-proto/src/lib.rs` (rebuilds every dependent): 0.42, 0.39, 0.39 s; `kernel-selfmap/src/lib.rs`: 0.29, 0.30, 0.30 s | 0.18 s | 5.0 s, 15 tests pass | 1.9 GB |

`ci/local.sh` on warm target dirs (build and test of all four workspaces): about 2 s.

### Against the thresholds

- **M-1:** a clean seed build takes 117 s, against supervise.md's 900 s cold build timeout and the 10-minute risk trigger. The target dir is 0.8-0.9 GB, under the 4 GB trigger. No risk is raised.
- **M-2:** the warm rebuild median is 0.21 s, far under the 5.0 s fallback-lane threshold (ext/authoring.md §3.10 C1). The cold build is 33 s, inside authoring.md's 25-60 s estimate. The subject is the ~120-line template, not the 150-line `semver-sort` fixture.

## Disk

| Item | Size |
|---|---|
| `vendor/` (repo root; 290 crates for all four workspaces) | 379 MB |
| `kernel/target`, `root/target`, `seed/target` | 1.9 GB, 208 MB, 938 MB |
| `~/.cargo/registry` (download cache) | 295 MB → 665 MB |
| `wasm32-wasip2` target in the toolchain | 101 MB |
| Free on `/` | 17 GiB before → 13 GiB after (other sessions were also writing) |

## Versions not given by the docs (resolved on 2026-10-20)

| Crate | Where | Requirement | Resolved |
|---|---|---|---|
| `anyhow` | `kernel-selfmap` (selfmap.md §3.6 code) | `1` | 1.0.104 |
| `indexmap` | `kernel-selfmap` (selfmap.md §3.6 code) | `2` | 2.14.2 (the same as wit-parser's) |
| `serde`, `serde_json` | `seed/ext` (`"1"` per authoring.md) | `1` | 1.0.229, 1.0.154, matching the body pins |
| `tokio` | `seed` (through wasmtime-wasi) | `^1` | pinned in `seed/Cargo.lock` to 1.53.3 (`cargo update --precise`) so that vendor/ holds a single tokio, the kernel's `=1.53.3`; 1.54.0 would otherwise be chosen |

Every other direct dependency uses the exact version from CONVENTIONS.md §5 or the owning doc. No crate in any lockfile pulls `aws-lc-rs`; rustls uses `ring` everywhere.
