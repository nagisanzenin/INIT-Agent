# WP9: `kernel-selfmap`, the self.json generator (lane K2)

Status: **done** (2026-10-20). Crate: `kernel/crates/kernel-selfmap/` (`kernel_selfmap`), built per kernel/selfmap.md v0.3.

## Done
- **Committed body file** (`body::generate`), `self_v` 2, generator version 2, committed fields only. It has no
  `live` key (D12). It covers:
  - the file inventory and `source_digest`, with `walk` in gate mode and `git ls-files --cached --others
    --exclude-standard` in write/check mode;
  - the budget set, `tests_tokens` and `uncounted_tokens`;
  - modules and items from the syn 3.0.9 scan, plus `forbid_unsafe`;
  - components with the ids of evolution.md §3.1 (`mod`, `tool`, `ext`, `skill`, `profile`, `prompt`; `hooks`
    is always empty);
  - `@init compensates-for` markers in all four forms, with `marker_grammar`, `marker_conflict` and the
    `marker_not_doc` warning;
  - registry dependencies from `Cargo.lock`;
  - pretty-ICJ output.
- **Tokens:** `o200k_base` from tiktoken-rs 0.12.1, using `encode_ordinary`.
- **Tools** from `env -i <binary> --manifest`. The run uses an empty cwd, closed stdin, a 10 s timeout and a
  1 MiB cap, and requires exactly one line. Tools are normalized as the proxy does it, then:
  - `tools.sha256` is computed with `init-tools-v1`, plus each tool's `schema_sha256` and `tokens`;
  - the manifest's own `tools_sha256` is checked against it (`manifest_tools_hash`);
  - the rest of the manifest is copied verbatim.
  `manifest::normalize_request_tools` also covers the openai-chat, anthropic and responses renderings, as
  shared vectors for P-13. The manifest input is **stubbed** until WP13:
  - CLI `--manifest-json <file>`;
  - fixture `tests/fixtures/manifest-seed.json` (boot.md §4.3 with tools.md §4.1 verbatim, 3,405 bytes of
    ICJ tools);
  - fixture `tools_sha256` = `sha256:996fe26661cb88a81ef73e0bccc0bcd5d96c9ba57c0b198e71ce39226ca732e4`,
    computed independently with Python.
- **WIT inventory** (`wit::wit_inventory`, wit-component 0.262.0): exports, imports, structural signatures and
  `wit_sha256`.
- **`describe()`** runs only in a child process, `kernel-selfmap --describe <wasm>` (`describe::child_main`). The
  child uses:
  - Wasmtime =48.0.6 and `bindgen!` for `init:ext/tool`;
  - all of WASI p2 with an empty `WasiCtx`, and `process` returning `denied("describe has no capabilities")`;
  - fuel of 2×10⁸, `OptLevel::None` and a 64 MiB memory cap;
  - two calls, which must be equal.

  The parent then validates the result per ext/wit.md §3.7: name, core names, description, schema keywords,
  integers only, version and canonical form. Failures are reported as `ext_contract`.
- **Drift** (`drift::compare`): the same semantics as `kernel-gate` `g1::compare_self_json`. A test runs both on
  11 cases and requires equal codes, warnings and report bytes.
- **Also built, from the §8 tests:**
  - `kernel-self.json` (`kernel_tree`, `--kernel`);
  - `capability-v1` (`capability`, `--capability`), using the gate's §3.8.3 primitives, cross-checked bit for
    bit;
  - the served-view builder (`serve::build_served`, `kernel_live`, `write_atomic`);
  - both v2 JSON Schemas (`schema/`, verbatim from §4.2 and §4.2a).
- **CLI:** `cli::run(args, self_cmd)` gives the exit codes 0/1/2/3/4 and JSON-line diagnostics. The binary is
  `kernel-selfmap`.

## Tests
- `cd kernel && CARGO_TARGET_DIR=target/k2 cargo test --offline -p kernel-selfmap`: **49 passed** (37 unit,
  12 integration), about 34 s.
- `cargo clippy --offline -p kernel-selfmap --all-targets -- -D warnings`: clean. `rustfmt --edition 2024`
  has been applied.
- §8 coverage:
  - **S-01:** CLI output is byte-identical over 3 runs, one of them with a different HOME, TZ and locale.
  - **S-02:** reported through `--check` on a git tree. `src/loop.rs` is named first, together with
    `/files/N/sha256` and `/modules/N/tokens`. An untracked file is reported as "added".
  - **S-03, S-04, S-05, S-06 and S-07** pass. S-07 goes through `kernel_gate::g1::check_tag_markers`.
  - **S-08:** `count-lines` is built with `CARGO_TARGET_DIR=seed/target cargo build --offline --target
    wasm32-wasip2` in `seed/ext`.
  - **S-09:** a describe that loops reports "all fuel consumed"; one that reads a file traps; a
    nondeterministic one is caught. The fixtures are in `tests/fixtures/ext/`.
  - **S-10** and **S-11** pass.
  - **S-12:** measured, see below.
  - **S-13** through **S-19** pass.
  - Every generated `self.json`, served view and `kernel-self.json` validates against the v2 schemas. This
    includes the real `seed/` tree and the real `kernel/` tree.
- **S-12:** the seed body plus 3 components takes **3.6 s with the debug build**. Most of that is the
  Cranelift compile of the 248 KB `count-lines` at opt-level 0 in the kernel's dev profile. The release
  build was **not measured**, because the coordinator limited disk use (6.6 GB free). The debug test asserts
  < 20 s; a release run of the same test asserts < 2 s.
- Test homes live only under `kernel/target/test-homes/WP9/` and are removed by exact path. None remain.
  The ext fixtures build into `kernel/target/k2/tmp/selfmap-ext`.

## Deviations
1. **`files` and `source_digest` omit `vendor/`, `target/` and `.git/`** at the root. Listing and tokenizing
   about 10⁴ vendored files would break the < 2 s target. Vendored crates are pinned by `dependencies`, the
   `Cargo.lock` checksums that cargo verifies in `--frozen` builds.
2. **syn adds the `printing` feature** (in this crate's Cargo.toml only). It gives `Spanned` for item lines;
   `quote` was already in the lock and in vendor.
3. **`bindgen!` reads the repo-root `wit/init-ext/v1`.** This repo has no `kernel/wit`.
4. **`src/tools/mod.rs` and `src/profiles/mod.rs` are `mod/` components.** A file becomes `tool/<name>` only
   if `<name>` is a manifest tool. A marker in `build.rs` gets the warning `marker_unattached`.
5. **Marker regions:**
   - a file marker's region is lines 1 to the last line, with the file's tokens;
   - a skill marker's region runs from its front-matter line to the end of the file.
6. **`uncounted_tokens`** is the tokens of files that are neither in the budget nor tests (Cargo.lock,
   `wit/`, README and so on).
7. **`describe` details:**
   - it is called twice **on one instance**, so static state is caught as well;
   - `timeout_ms` is recorded as declared (null means the default).
8. **What the decoded WIT shows (W-3 finding):** a decoded import interface lists **only the functions the
   component imports**. For example, `uses-process` shows `init:ext/process@1.0.0` with `["run"]`.
   `count-lines` does not import `init:ext/process` at all. Imports also list the world's types
   (`tool-info`, `tool-error`) as `kind: type`.
9. **Kernel components** extend the §3.13 table to this repo's lane crates:
   - `kernel-<area>/src/<first module>` first, then the area itself;
   - `init-proto` and `initctl` go to `kernel/initctl`.

   Contracts not given in the doc were chosen here:

   | Component | Contract |
   |---|---|
   | `kernel/supervise` | supervise.md §4.1-§4.5 |
   | `kernel/evolution` | evolution.md §3.8 |
   | `kernel/sandbox` | sandbox.md §4 |
   | `kernel/config` | config.md §3.1 |

   `--kernel --write/--check` does not build `init-kernel`, because nothing in `kernel-self.json` comes from
   the binary.
10. **Capability inputs beyond RunRecord v2.** RunRecord v2 has no timestamp and no default model. The
    following fields are read when present:
    - optional `ts`, an RFC 3339 timestamp;
    - optional `model`, else the `models` key with the most requests;
    - optional `sealed`. A `capability`-set run counts only if it carries `"sealed": false`, because D2
      makes set A sealed.

    Without `ts`, the window is the last 20 jobs only, and `window.from`, `window.to` and `last_run` are
    null. The schema requires `last_run` to be a string.
11. **CLI additions:**
    - `--manifest-json <file>`, the WP13 stub;
    - `--check` prints `<code>\n<report>` on stdout;
    - the codes `usage` and `build_failed`;
    - a generator error (exit 4) writes no file; a check failure (exit 1) still writes it;
    - write mode adds the vendored-source `--config` flags only when `<root>/vendor/` exists, with an
      absolute path.
12. **The drift report also names `kernel.files`** for `kernel-self.json`. Body behaviour is identical to the
    gate's.
13. **S-11 is a regression pin** of 2,638 tokens. No independent `o200k_base` implementation is available
    offline. The ids for `hello world` (`[24912, 2375]`) are asserted as a sanity check.

## Needs from others
- **init-kernel owner:** dispatch `init-kernel selfmap …` to
  `kernel_selfmap::cli::run(&args_after_selfmap, &[current_exe, "selfmap".into()])`.
- **WP13:** the real `init-body --manifest` (shape and hash in `tests/fixtures/manifest-seed.json`). Then drop
  `--manifest-json`.
- **WP8 / gate:**
  - add `ts` (and `sealed`, plus a default `model`) to RunRecord v2;
  - wire G1 to run `init-kernel selfmap --root … --binary … --components … --out …` in the build sandbox;
  - teach `g1::file_hashes` to read `kernel.files` for K0, and use `gate.k0.kernel_self_drift`.
- **WP4 / supervise:**
  - serve mode: `serve::build_served` plus `write_atomic` on the §3.11 triggers;
  - the embedded kernel component table.
- **WP5 / proxy:** compute `init-tools-v1` the same way; `manifest::normalize_request_tools` and S-10 are the
  shared vectors.
- **WP0 / orchestrator:**
  - `kernel/Cargo.toml` `[profile.dev.package]` opt-level for wasmtime and cranelift, to speed up `describe`
    in debug builds;
  - consider a `kernel/wit` for L3 self-containment.
- **kernel/Cargo.lock:** this commit adds only `kernel-gate` and `regex` (dev-dependencies) to the
  `kernel-selfmap` entry. Other lanes' uncommitted lock edits are left untouched.
- **selfmap.md owner:**
  - decide what `last_run`/`window` hold when there is no timestamp;
  - decide that `wit_imports[].functions` means the functions actually imported.

## Left
- A release-build S-12 number. It needs about 0.5 GB more of `k2` and waits for disk.
- The `--write`/`--check` build step (cargo build of `init-body` and the ext crates) has not been run end to
  end. Tests use `--no-build`, because a seed build would need a new target dir (disk) and the seed's
  `--manifest` is still a stub.
