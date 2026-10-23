# Kernel self-map generator (`self.json`)

```
Status: v0.3 + build sync 2026-10-20 · Area: kernel
Depends on: CONVENTIONS.md, INIT Agent spec.md (v0.2 §5, §6.3 G1, §6.5, §6.6; v0.3 §13, §15, §16),
            adr/007-root-and-autonomy-ladder.md, root/root.md (R7 level and ladder publication),
            root/sealed-eval.md (sealed-audit summary), kernel/gate.md, kernel/proxy.md,
            kernel/supervise.md, kernel/sandbox.md, body/boot.md (§4.3 --manifest), body/tools.md (§4.1),
            body/log-context.md (§3.8.1 ICJ), ext/wit.md, ext/host.md, evolution.md (§3.1 component ids,
            §3.8 trailers, §3.11–3.12), awareness.md (§3.7)
Defines:    the self.json schema (self_v 2, JSON Schema below): the committed, tree-derived part and the
            kernel-served view with its `live` section (level, ladder state, kernel build, capability
            self-model); the generator algorithm and its inputs; the token measure (o200k_base); the
            `@init compensates-for` marker grammar and where markers live; component extraction (mod, tool,
            ext, skill, profile, prompt, and kernel components from L3); the WIT export and describe()
            extraction from components; the capability self-model (per-category pass rates from gate
            data); the drift-check semantics used by G1; `init-kernel selfmap` CLI
```

---

## 1. Purpose

`self.json` answers "what exactly am I made of?" (spec §5) from evidence the agent cannot fake:
- the committed source, parsed by the kernel;
- the body binary's own `--manifest` output;
- each compiled Wasm component's exports and `describe()` result.

The kernel regenerates it at every gate (G1). Any difference from the committed copy fails the gate, so the self-model can never go stale. It is also the component registry that retirement and thinning work from (evolution.md §3.11–3.12).

v0.3 widens "what am I" to the agent's new position (spec v0.3 §13–§15):
- **Where am I on the ladder?** The autonomy level, its ceiling in the charter, and the state of the next unlock criteria, as the Root publishes them (R7).
- **What is my kernel?** From L3 the kernel is the agent's to evolve, so kernel source files become part of the verified inventory, under the same drift check as the body. Below L3 the running kernel build is listed for reference only.
- **What am I good at?** A capability self-model: per-category pass rates derived from the runs the gate already pays for. The agent then knows its strengths and weaknesses by measurement, not by recollection.

These facts change without a commit, so they cannot live in the committed file (§3.10). The kernel serves them in a `live` section of a kernel-maintained copy, `state/kernel/self.json` (§3.11).

## 2. Responsibilities / non-goals

**Responsibilities**
- Define the `self.json` schema and keep it byte-deterministic: same tree and same binaries give identical bytes.
- Scan Rust sources with syn; inventory modules, items, components and markers.
- Measure size in tokens with one fixed, model-independent tokenizer.
- Read tool schemas from the body binary (`init-body --manifest`) and normalize them exactly as the proxy does, so G2 can compare them with what the model was actually sent.
- Extract each component's WIT exports, imports and `describe()` result from the binary.
- Define `Compensates-for` markers and attach them to components.
- Define drift semantics and the G1 checks that use them.
- Inventory the kernel tree (`lineage/kernel.git`, evolution.md §3.15.6) with the same rigour as the body, as a committed `kernel-self.json` that K0 drift-checks; from L3 it is part of the agent's verified inventory (§3.13).
- Maintain the served view `state/kernel/self.json`: the running version's committed `self.json` plus the `live` section (§3.11), including the capability self-model (§3.12).

**Non-goals**
- Running G1 itself or deciding gate verdicts (kernel/gate.md).
- Trailer grammar (evolution.md §3.8). This doc only defines how markers are found and cross-checked.
- Volatile facts **in the committed file**: the running sha, LKG, provisional state, level, spend. The committed file stays tree-derived (§3.10). The served view carries the level, the ladder, the running and LKG shas, and the capability data. Spend and provisional state stay in `initctl status` and `state/kernel/boot.json` (kernel/supervise.md).
- Computing the level or the ladder criteria. That is the Root's (R7, root/root.md). This doc copies them verbatim.

## 3. Design

### 3.1 Modes and inputs

| Mode | Invocation | Who | Inputs | Output |
|---|---|---|---|---|
| gate | `init-kernel selfmap --root <export> --binary <path> --components <dir> --out <file>` | kernel (G1), as a subprocess in the `build` sandbox profile (`SRC` = export, `OUT` = `cache/gate/<job>/`) | export tree, kernel-built binary and components | `<file>` |
| write | `init-kernel selfmap --root <worktree> --write` | agent (inside its sandbox) | worktree; builds binary and components itself | `<worktree>/self.json` |
| check | `init-kernel selfmap --root <worktree> --check` | agent | as write | exit 0 if the committed/working `self.json` matches, 1 with a diff otherwise |
| kernel write / check | `init-kernel selfmap --kernel --root <kernel worktree> --write\|--check` | agent at L3 (in `work/kwt-<id>`), the maintainer for releases | the kernel tree; no build (nothing in `kernel-self.json` comes from the binary) | `<root>/kernel-self.json` (§3.13) |
| kernel gate | `init-kernel selfmap --kernel --root <export> --out <file>` | the running kernel's gate, stage K0 (kernel/gate.md §3.1), in the `build` profile | the candidate kernel export | `<file>` |
| serve | in-process in `init-kernel supervise` (no subprocess: it executes no candidate code) | kernel | the running sha's committed `self.json` (`$G show <running>:self.json`), the Root's published level and ladder (R7), the running kernel build's embedded component table, `vault/gate/results/` (kernel/gate.md §3.10), the Root's sealed-audit summary if published | `state/kernel/self.json` (§3.11) |

In write and check modes the generator builds first:
- `cargo build --frozen -p init-body --bin init-body`, then `cargo build --frozen -p <ext> --target wasm32-wasip2` for each ext crate;
- with the caller's environment (the agent's `CARGO_TARGET_DIR`, normally `$INIT_HOME/work/target`) and the same two `--config` flags the gate uses (kernel/gate.md §3.3). The flags are added only when `<root>/vendor/` exists, and the vendored-source path is absolute.

The agent runs the kernel binary from `$INIT_HOME/kernel/bin/` (read + exec), so the same code that judges `self.json` also produces it. It runs with the agent's privileges, which grants nothing.

### 3.2 File inventory and `source_digest`

The file set is:
- in gate mode, every regular file in the export (exactly the commit's tree, supervise.md §3.6.2);
- in write/check mode, `git -C <root> ls-files -z --cached --others --exclude-standard`, i.e. tracked plus untracked-but-not-ignored. The agent runs this in its own repo.

`self.json` itself is excluded. Symlinks are recorded by target string, not followed. `vendor/`, `target/` and `.git/` at the root are not inventoried in either mode, so `files` and `source_digest` omit them: listing and tokenizing about 10⁴ vendored files would break the < 2 s target (§6). Vendored crates stay pinned by `dependencies`, the `Cargo.lock` checksums that cargo verifies in every `--frozen` build.

For each file the generator records `{path, sha256, bytes, tokens, counted}`:
- `tokens` is computed for UTF-8 text ≤ 1 MiB; otherwise it is `null` and a warning names the file;
- `counted` says whether the file is part of the **body budget** (spec §4: about 25k tokens). `budget.uncounted_tokens` is the token total of the files that are neither in the budget nor tests (`Cargo.lock`, `wit/`, the README and so on).

**Budget set (generator v1).**
- Included: `src/**/*.rs`, `build.rs`, `Cargo.toml`, `.cargo/config.toml`, `GENESIS.md`, `SELF.md`, `skills/**`, `ext/Cargo.toml`, `ext/*/src/**/*.rs`, `ext/*/Cargo.toml`, `ext/*/build.rs`.
- Excluded: `vendor/**`, `Cargo.lock`, `ext/Cargo.lock`, `target/**`, `tests/**` (reported separately as `budget.tests_tokens`), `ext/*/tests/**`, `wit/**` (a kernel-owned copy, ext/wit.md; G1 checks it byte for byte), binary files.

`body_tokens` (top level) equals `budget.tokens`. It is the single number that evolution.md, awareness.md and supervise.md's `boot.json.selfmap` read (awareness.md X4, supervise.md Q10). supervise reads it for any sha with `$G show <sha>:self.json`. Kernel tokens never count toward `body_tokens`: the 25k budget is the body's (spec §4), and the kernel has its own budget in `kernel-self.json` (§3.13).

`source_digest = "sha256:" + hex(SHA-256(concat over files sorted by path bytes of: path ‖ 0x00 ‖ hex(sha256(content)) ‖ 0x0A)))`. It pins the file to an exact tree. In write mode, an untracked file the agent then forgets to commit changes the digest, and G1 names it.

### 3.3 Rust scan (syn)

Crates: syn 3.0.9 (`default-features = false`, features `full`, `parsing`, `printing`, `visit`; `printing` gives `Spanned` for the item start and end lines and pulls in only `quote`, which the lock and vendor already hold) and proc-macro2 1.0.107 (feature `span-locations` for line numbers). syn 3 is chosen over 2.0.119 for the newest-syntax coverage an agent writing current Rust needs. The cost is compiling syn twice while serde_derive still uses syn 2, which only affects kernel build time.

```rust
use syn::{visit::Visit, Attribute, File, Item, Meta};
use proc_macro2::Span;

pub struct FileScan {
    pub path: String,
    pub items: Vec<ItemInfo>,              // top-level items only
    pub inner_attrs: Vec<String>,          // e.g. "forbid(unsafe_code)"
    pub markers: Vec<Marker>,              // §3.7
    pub parse_error: Option<String>,
}
pub struct ItemInfo { pub kind: &'static str, pub name: String, pub vis: &'static str,
                      pub line: usize, pub end_line: usize, pub tokens: u32 }

pub fn scan_rust(path: &str, text: &str, tok: &Tokenizer) -> FileScan {
    let file: File = match syn::parse_file(text) { Ok(f) => f, Err(e) => return FileScan::error(path, e) };
    // inner attributes: #![forbid(unsafe_code)] and //! doc markers
    for a in &file.attrs { /* a.style == AttrStyle::Inner; a.path().is_ident("forbid") / is_ident("doc") */ }
    for it in &file.items {
        let (kind, name, vis, attrs) = describe_item(it);   // Fn, Struct, Enum, Trait, Impl ("impl <Type>"),
                                                           // Mod, Const, Static, Type, Macro (macro_rules name), Use (skipped)
        let (l0, l1) = (span_start(it).line, span_end(it).line);  // proc-macro2 LineColumn, 1-based lines
        let tokens = tok.count(&slice_lines(text, l0, l1));
        // outer doc markers on the item: /// @init compensates-for: …
    }
    /* … */
}
```

- **`#![forbid(unsafe_code)]` check (used by gate G0 step 3).** The file's `attrs` contain an `Attribute` with `style: AttrStyle::Inner(_)`, `path().is_ident("forbid")`, and a parsed meta list whose idents include `unsafe_code`.
- **Parse errors** (for example syntax newer than syn 3.0.9 supports) make G1 fail with `selfmap_error` naming the file and line. The agent can work around it, and the maintainer can bump syn.
- **Modules** are files, not inline `mod {}` blocks. An inline module is listed as an item of its file.

### 3.4 Token measure: `o200k_base` (decision)

`tokens` everywhere in `self.json` is the count of `o200k_base` BPE tokens, via `tiktoken-rs` 0.12.1. The count is `CoreBPE::encode_ordinary(text).len()` on the singleton from `tiktoken_rs::o200k_base_singleton()`. "Ordinary" means special-token strings in a file count as plain text.

| Option | Verdict |
|---|---|
| Bytes / 4 (or / 3.5) heuristic | Rejected. It is systematically off for symbol-dense code, and blind to edits that change token count without changing length. |
| The active model's tokenizer (e.g. DeepSeek V4.1's `tokenizer.json`, 6.4 MB, via HF `tokenizers` 0.23.2) | Rejected. The size would depend on `config.toml`, so changing the model would make every body "drift" in G1, break the size trend across thinning (spec §6.6), and make the "smaller body" tie-break model-dependent. The `tokenizers` crate also pulls C/C++ builds (`onig`, `esaxx_fast`) by default. |
| **`o200k_base` (chosen)** | Model-independent, deterministic, pure Rust, a real BPE used by current frontier models, and offline (the ranks are embedded in the crate). |

- The count is a **measure**, not the bill. Different frontier tokenizers differ on code by roughly ±15% (UNVERIFIED for DeepSeek V4.1). To relate the two, the proxy observes the real `prompt_tokens` of the head for the active model. A calibration ratio `model_tokens / o200k_tokens` for the head can be shown by awareness.md if wanted, as a measured number.
- Changing the tokenizer is a generator version bump (§3.8).

### 3.5 Tools: from `--manifest`, normalized, cross-checked by the proxy

The generator runs the body binary exactly as body/boot.md §4.3 requires: `env -i <binary> --manifest`.
- No env, cwd = an empty tmp dir, timeout 10 s, stdout ≤ 1 MiB.
- Gate mode runs the whole generator inside the `build` profile (no network, no sockets, no `vault/`), and `--manifest` gets no `INIT_*` variables.
- Exit 0 and exactly one line of JSON are required (`manifest_error` otherwise).

From the manifest:
- `tools` (body/tools.md §4.1 registry objects `{description, input_schema, name}`) are normalized with the proxy's function (kernel/proxy.md §3.12.2: missing description → `""`, missing schema → `{}`), sorted by name and serialized as ICJ;
- `tools.sha256` = `init-tools-v1` = `"sha256:" + hex(SHA-256(ICJ(sorted list)))`;
- each tool's `schema_sha256` = `"sha256:" + hex(SHA-256(ICJ(tool)))`;
- `tokens` of each tool = tokens of its ICJ text (what it costs in the prefix).
- The manifest's own `tools_sha256` is compared with ours. body/boot.md §4.3 computes it with `init-tools-v1`, so a difference is a **G1 failure** (`gate.g1.manifest_tools_hash`): an honesty check on the binary's self-report).
- The rest of the manifest object (everything except `tools`, so `body_version`, `control_channel`, `env`, `modes`, `models`, its own `tools_sha256`, and any field body/boot.md adds later, such as supported WIT majors) is copied verbatim into `manifest`. New manifest fields therefore need no schema change here (body/boot.md Q-B7).

**Cross-check against what the model saw.** G1 proves the binary *declares* these tools. G2 proves the binary *sends* them: the proxy computes `tools_sha256` on the probe's first request with the same function and the gate compares it with `self.json` `tools.sha256` (kernel/gate.md §3.5, criterion 4). A body that declares one tool set and sends another fails G2 with a per-tool `schema_sha256` diff.

### 3.6 Extensions: WIT exports and `describe()` from the binary

For each member `ext/<name>/` of the extension workspace `ext/Cargo.toml` (ext/authoring.md §3.3), the input is the kernel-built component `<components>/<name>.wasm`. Its SHA-256 and size are **not** recorded in `self.json`: rustc embeds absolute source paths, so the agent's build and the gate's build of the same source differ (ext/host.md §3.15). The gate record keeps the hash of the binary it actually tested.

**1. Exports and imports from the component's type information** (wit-component 0.262.0, wit-parser 0.262.0):

```rust
use wit_component::{decode, DecodedWasm, WitPrinter};
use wit_parser::{Resolve, WorldItem, WorldKey};

pub fn wit_inventory(bytes: &[u8]) -> anyhow::Result<WitInventory> {
    let decoded = decode(bytes)?;
    let (resolve, world_id) = match &decoded {
        DecodedWasm::Component(r, w) => (r, *w),
        DecodedWasm::WitPackage(..) => anyhow::bail!("not a component"),
    };
    let world = &resolve.worlds[world_id];
    let render = |items: &indexmap::IndexMap<WorldKey, WorldItem>| -> Vec<WitItem> {
        let mut v: Vec<WitItem> = items.iter().map(|(k, it)| match it {
            WorldItem::Function(f) => WitItem::func(resolve.name_world_key(k), render_func(resolve, f)),
            WorldItem::Interface { id, .. } => WitItem::interface(
                resolve.id_of(*id).unwrap_or_else(|| resolve.name_world_key(k)),          // e.g. "init:ext/process@1.0.0"
                resolve.interfaces[*id].functions.keys().cloned().collect()),
            WorldItem::Type { id, .. } => WitItem::ty(resolve.name_world_key(k), *id),
        }).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name)); v
    };
    let mut printer = WitPrinter::default();
    printer.print(resolve, decoded.package(), &[])?;
    let wit_text: String = printer.output.into();
    Ok(WitInventory { exports: render(&world.exports), imports: render(&world.imports),
                      wit_sha256: sha256_hex(wit_text.as_bytes()) })
}
```

- `render_func` prints `func(<name>: <type>, …) -> <type>` with a small printer over `wit_parser::Type`: primitives by WIT name; `Type::Id` by the type definition's name, or structurally (`list<…>`, `option<…>`, `result<…, …>`, `tuple<…>`) when anonymous.
- For an `init:ext@1.0.0` `tool` component the expected exports are the world-level functions `describe` and `call` (ext/wit.md §3.2).
- A decoded import interface lists only the functions the component actually imports, so `wit_imports[].functions` means the imported functions (measured, ext/wit.md W-3). A component that calls only `process::run` shows `init:ext/process@1.0.0` with `["run"]`, and one that never calls `process` does not import it at all (`count-lines` imports only `wasi:*`). The imports also list the world's types (`tool-info`, `tool-error`) as `kind: type`.
- `api` = `"init:ext@<major>"`: the newest major of the kernel's WIT copies whose `tool` world the component type-checks against (ext/wit.md D8).
- The names of the synthetic root package and world that `decode` produces are UNVERIFIED. They are not stored; only exports, imports and `wit_sha256` are.

**2. `describe()` called by the kernel** (ext/wit.md D10: pure, no capabilities). The engine is the one the body hosts components with: wasmtime **=48.0.6** (the LTS line, ext/host.md §10.2) with `component-model` and Cranelift, plus wasmtime-wasi =48.0.6. What the kernel validates should be what the body runs.

It runs in a **child process**, `init-kernel selfmap --describe <wasm>`, which prints canonical JSON (ext/host.md §3.15). A Wasm sandbox escape then reaches only that child, and in gate mode the whole generator is already a subprocess in the `build` profile.

```rust
use wasmtime::{Config, Engine, Store, component::{Component, Linker, ResourceTable}};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

wasmtime::component::bindgen!({ path: "../../wit/init-ext/v1", world: "init:ext/tool" });
// the kernel tree's wit/init-ext/v1, i.e. kernel/wit/init-ext/v1 (installed as $INIT_HOME/kernel/wit/init-ext/v1;
// ext/wit.md §3.4 and §4.1, CONVENTIONS §4; R1 follow-up E12), relative to crates/init-kernel/. One directory and one bindgen! per major,
// so supporting N and N−1 means two modules (ext/wit.md §3.6; ext.md E12)

struct Host { wasi: WasiCtx, table: ResourceTable }
// impl WasiView for Host { fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> { … } }   (exact 48.x shape UNVERIFIED)
// impl init::ext::process::Host for Host: every function returns Err(ProcessError::Denied)  (stub; never called)

pub fn describe(engine: &Engine, bytes: &[u8]) -> anyhow::Result<ToolInfo> {
    let component = Component::from_binary(engine, bytes)?;
    let mut linker: Linker<Host> = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;          // name per 48.x: UNVERIFIED
    Tool::add_to_linker(&mut linker, |h: &mut Host| h)?;          // bindgen-generated
    let mut store = Store::new(engine, Host { wasi: WasiCtxBuilder::new().build(), table: ResourceTable::new() });
    store.set_fuel(200_000_000)?;                                  // Config::consume_fuel(true)
    let tool = Tool::instantiate(&mut store, &component, &linker)?;
    Ok(tool.call_describe(&mut store)?)
}
```

- **As built,** `bindgen!` reads the repo-root `wit/init-ext/v1`, because there is no `kernel/wit` yet. A `kernel/wit` copy is for L3 self-containment later.
- Engine config: `Config::new().consume_fuel(true).cranelift_opt_level(OptLevel::None)`, because describe runs once and a fast compile matters more than fast code. Memory is limited to 64 MiB via `StoreLimits`.
- The `WasiCtx` is empty: no preopens, no env, no args, no inherited stdio, no network.
- `describe` is called twice on one instance, so static state is caught as well; the two results must be byte-identical (ext/wit.md determinism rule). `timeout_ms` is recorded as declared (`null` means the default).
- The input schema is validated and canonicalized exactly as ext/wit.md §3.7 specifies (≤ 16 KiB, root object, allowed keywords, ICJ form).
- `name` must equal the directory name.

Failures give `ext_contract` with the component and reason. Examples:
- the component does not decode;
- `describe`/`call` are missing;
- instantiation fails;
- fuel runs out;
- the two calls differ;
- the name mismatches;
- the schema is invalid.

### 3.7 `Compensates-for` markers (decision: in-code markers)

**Decision.** A scaffold is declared **where it lives**, with an `@init` marker in its own source:

| File type | Marker | Attaches to |
|---|---|---|
| Rust module or crate | inner doc comment `//! @init compensates-for: <value>` at the top of the file | the file's component (`mod/<path>`, or `tool/<name>` for `src/tools/<name>.rs`, or `ext/<name>` for `ext/<name>/src/lib.rs`) |
| Rust item | outer doc comment `/// @init compensates-for: <value>` on any top-level item | the containing file's component; the item is recorded in `markers[]` with its lines and tokens |
| Skill | front-matter line `compensates-for: <value>` between the leading `---` lines of `SKILL.md` | `skill/<name>` |
| Prompt text (`GENESIS.md`, `SELF.md`) | `<!-- @init compensates-for: <value> -->` … `<!-- @init end -->` around a block | `prompt/<file>/<slug>` (evolution.md §3.1) |

Grammar (same value grammar as the trailer, evolution.md §3.8):

```abnf
value  = "none" [ SP "(" 1*200text ")" ] / slug [ ":" SP 1*200text ]
slug   = 3*40( %x61-7A / DIGIT / "-" )
```

Rules:
- **One weakness per component.** All markers attributed to one component must carry the same slug, otherwise `marker_conflict`. If a file holds two scaffolds for different weaknesses, split it. Thinning removes whole components (evolution.md §3.12), so the unit of tagging must be the unit of deletion.
- A component without a marker has `compensates_for: "none"`.
- **Regions.** A file marker's region is lines 1 to the last line, with the file's tokens. A skill marker's region runs from its front-matter line to the end of the file. A marker in a file that belongs to no component (for example `build.rs`) gets the warning `marker_unattached`.
- Rust doc comments reach syn as `#[doc = "…"]` / `#![doc = "…"]` attributes, so markers survive formatting and need no comment parser. A marker inside a normal `//` comment is not seen, and is reported as a warning if a line matches `@init compensates-for` outside doc comments (`marker_not_doc`).

**Why not the alternatives.**
- A central `scaffolds.toml` drifts: entries outlive the code they describe, and paths break on rename. It is also one more file the agent must remember to update. An in-code marker disappears with the code and lets the generator measure the scaffold's size from the AST (spec §6.6: "deleting this saves N tokens").
- Commit trailers alone describe *changes*, not the *current* inventory. A later commit can move or delete a scaffold without a trailer.

The trailer stays and is cross-checked against the markers (§3.8, check 3).

**What markers cannot do.** They cannot find untagged scaffolding. "Every scaffold carries a tag" (spec §6.6) remains a norm. The `guard` change type (evolution.md §3.8), which requires a slug, and G2's prefix hash, which catches prompt additions, are the mechanical backstops.

### 3.8 Drift-check semantics (G1)

G1 (kernel/gate.md §3.4) compares the regenerated file **R** with the committed file **C**:

1. **Presence.** C missing → `self_json_missing`.
2. **Generator version.** If `C.generator.version ≠ R.generator.version`, or `C.self_v ≠ R.self_v`, the result is `generator_changed`. The kernel was upgraded; the fix is to run `init-kernel selfmap --write` and commit. This is a one-time cost after kernel upgrades that change output; the maintainer bumps `generator.version` whenever output changes for the same input.
3. **Only committed fields are compared (v03-decisions D12).** Pass rates, the capability self-model, the level and the ladder live only in the kernel-written served copy `state/kernel/self.json` (§3.11). The committed file leaves them out: it has no `live` key, and `--write` never writes one. Before comparing, G1 drops `live` from both R and C, so a committed `live` (for example a copy of the served file) changes nothing. It is reported as the warning `live_ignored` and never read.
4. **Equality.** Parse both as JSON. Pass iff they are equal as `serde_json::Value` (objects compared as maps, arrays in order). Equal values with different bytes give a warning `non_canonical_format`: the agent hand-edited whitespace. No field is excluded from comparison, because nothing in the committed file is volatile. The kernel tree's `kernel-self.json` gets the same check at K0 (§3.13).
5. **Drift report** (`self_json_drift`), ≤ 4 KiB, for the agent:
   - the RFC 6901 JSON Pointers of every differing leaf, sorted, each with the committed and regenerated values cut at 120 bytes;
   - first, the list of files whose `sha256` differs (`files`, or `kernel.files` for `kernel-self.json`). That is usually the whole story: "src/loop.rs changed but self.json was not regenerated".

Additional G1 checks, run on R alone or on R against the parent's self.json **P**:

| # | Check | Failure |
|---|---|---|
| 1 | every Rust file parsed | `selfmap_error` |
| 2 | every ext component passes §3.6 | `ext_contract` |
| 3 | trailer/marker (evolution.md §3.9 item 10): for change types `feat`, `fix`, `guard`, every component id in R but not in P has `compensates_for` equal to the slug of the commit's `Compensates-for` trailer (`none` if the trailer is `none`) | `tag_marker_mismatch` |
| 4 | markers well-formed; one slug per component | `marker_grammar`, `marker_conflict` |
| 5 | manifest ran and parsed | `manifest_error` |
| 6 | the manifest's self-reported `tools_sha256` equals the `init-tools-v1` hash of its own `tools` | `manifest_tools_hash` |
| 7 | — (D12: `live` is not compared; a committed `live` only raises the warning `live_ignored`) | — |

The `self.json` of the running version (P) is read from lineage: `$G show <running>:self.json`.

### 3.9 Components

The generator emits the component list that evolution.md §3.1 relies on, with exactly these ids:

| Kind | Id | Source of truth | `core` |
|---|---|---|---|
| `mod` | `mod/<path>` | every `src/**/*.rs` file except `src/tools/*.rs` and `src/profiles/*.rs`; `src/tools/mod.rs` and `src/profiles/mod.rs` are `mod/` components | false |
| `tool` | `tool/<name>` | each tool in the manifest; source file `src/tools/<name>.rs` if present (a file becomes `tool/<name>` only if `<name>` is a manifest tool; any other is a `mod/` component) | true for `read`, `write`, `edit`, `bash`, `ext` (body/tools.md §4.1) |
| `ext` | `ext/<name>` | each ext crate with a built component | false |
| `hook` | `hook/<ext>/<hook>` | none in `init:ext@1.x` (ext/wit.md §3.5); always empty | — |
| `skill` | `skill/<name>` | each `skills/<dir>/SKILL.md` (name from front matter, else the dir name) | false |
| `profile` | `profile/<family>` | each `src/profiles/<family>.rs` file (body/tools.md §2; the seed ships none) | false |
| `prompt` | `prompt/<file>/<slug>` | each marked block in `GENESIS.md` / `SELF.md` | false |

`tokens` of a component:
- `mod`: the file;
- `tool`: its source file plus its schema's ICJ text;
- `ext`: all budget-counted files under `ext/<name>/`;
- `skill`: all files under the skill dir;
- `profile`: its files;
- `prompt`: the marked block.

`paths` lists the files. Components are sorted by `id`.

Kernel components live in the kernel tree's own `kernel-self.json` (§3.13) and in the served view's `live.kernel`, never in `components`. Retirement and thinning (evolution.md §3.11–3.12) work only on body components, and kernel changes go through K0–K3 and the gate-of-gates (kernel/gate.md §3.1).

### 3.10 What the committed file leaves out, and where it goes

Spec §5 lists "SHA, LKG" among `self.json` contents, and v0.3 adds the level and the ladder. None of these can sit in the committed file:
- a file cannot contain the hash of the commit that contains it;
- LKG changes without any commit (a G4 verdict), and so do the level (the Root's R7) and the capability data (every G4). Including them would make every committed `self.json` stale the moment it is accepted, and G1 could never pass.

The design therefore has two layers with one schema (`self_v` 2):

| Layer | Path | Written by | Contents | Checked by |
|---|---|---|---|---|
| **Committed (body)** | `body/self.json` (in the agent's repo) | the agent, with `init-kernel selfmap --write` | everything derived from the body tree and the built binaries: inventory, tools, extensions, markers; no `live` key (D12) | G1: regenerated and compared on committed fields only (§3.8) |
| **Committed (kernel)** | `kernel-self.json` at the root of the kernel tree (`lineage/kernel.git`) | the maintainer's release, or from L3 the agent, with `init-kernel selfmap --kernel --write` | the kernel inventory (§3.13) | K0: regenerated and compared with the same semantics (kernel/gate.md §3.1) |
| **Served view** | `state/kernel/self.json` (kernel-written, agent-readable) | the kernel (serve mode, §3.11) | the running version's committed file, byte for byte in every field except `live`, which is filled in | built by the kernel from kernel-held data; never committed (§3.8 check 7) |

*As built (SELFJSON, 2026-10-20):* the seed ships its committed file as `seed/self.json` in the monorepo, which is `self.json` at the root of the seed repository and `body/self.json` after install. Without it, `initctl gate --quick` on an unchanged, freshly installed body failed `gate.g1.self_json_missing` (S-REAL). The maintainer generates it with the body's own write mode on a body checkout cut by `scripts/slice1/seedpack.py`, because that cut rewrites `.cargo/config.toml` (`directory = "vendor"`) and adds `vendor/`, so it matches what G1 sees in an install. The ci/local.sh step "seed G0 + G1" (`kernel-gate/tests/seed_g0.rs`) runs G1 on that cut with the shipped `init-kernel` in the `build` profile, and fails on drift with the regenerate command: `INIT_SEED_G1_BLESS=1 cargo test --offline -p kernel-gate --test seed_g0 -- --ignored` (in `kernel/`), which runs `init-kernel selfmap --root <cut> --write` and copies the result to `seed/self.json`. Any change under `seed/` therefore needs a regenerated `seed/self.json` in the same commit.

The generator still records `source_digest` (§3.2), which pins the committed file to its tree. The served view adds `live.running_sha` and `live.lkg_sha`, so the agent finds spec §5's "SHA, LKG" in one place. `state/kernel/boot.json`, `lkg.json` and `initctl status` stay authoritative for those values (kernel/supervise.md).

### 3.11 The served view and the `live` section

**When it is rebuilt.** The served view is rebuilt in-process by supervise (serve mode, §3.1), and written atomically (temp file in `state/kernel/`, `fsync`, `rename`) on each of:
- every body boot (the running sha may have changed);
- every LKG change;
- every level change the Root announces (R7);
- every G4 verdict (new capability data);
- every new sealed-audit summary the Root publishes;
- at most once a minute otherwise.

Serve mode executes no candidate code, so it needs no sandbox subprocess.

**`live` fields:**

| Field | Source | Notes |
|---|---|---|
| `as_of` | kernel clock | RFC 3339 UTC |
| `running_sha`, `lkg_sha` | supervise (`boot.json`, `lkg.json`) | the same values `initctl status` shows |
| `level` | the Root's published status (`INIT_HOME/root/status.json`, updated by the Root's `level` event; root/root.md §3.10.6, §4.1.4): `{current, max, since}` | `max` is the charter's `autonomy.max_level` |
| `ladder` | the Root's published ledger `INIT_HOME/root/ledger.json` (root/root.md §3.13), **copied verbatim** | root/root.md owns its fields (the effective unlock criteria, counts since `entered_seq`, the last level change and demotions with their triggers). Copying verbatim means a new Root field needs no schema change here, the same rule as `manifest` (§3.5). `null` if the file is absent |
| `kernel` | the running `init-kernel` binary and, when `lineage/kernel.git` exists, the running kernel sha's `kernel-self.json` | `{version, build_sha256, kernel_sha, source_digest, verified, evolvable, budget_tokens, components:[{id, tokens, contracts}]}`. `kernel_sha` and the inventory fields are `null` if no kernel repo exists; the component list then comes from a table the maintainer's build embeds in the binary. `verified: true` iff `build_sha256` equals the build digest recorded when that `kernel_sha` was admitted (the K-stage report and the Root's `kernel_admitted` record, evolution.md §3.15.6; a maintainer release takes the same path). `evolvable` is true from L3. The full inventory (files, modules, dependencies) stays in `kernel-self.json`, so the served view stays small |
| `capability` | §3.12 | the capability self-model |

### 3.12 The capability self-model (`live.capability`, method `capability-v1`)

**Purpose.** The agent should know what it is good at from measurement, not recollection, in the same spirit as the rest of this doc (spec §5: "verify, don't trust"). The gate already runs both arms on labelled tasks for every G4, so per-category pass rates of the current LKG cost nothing extra.

**Data.** The run records the gate compacts into `vault/gate/results/<sha12>.jsonl` (kernel/gate.md §3.10, §4.4 RunRecord v2):
- runs whose arm `sha` is the current LKG. These are the LKG arm of every later gate, plus the candidate arm of the gate that made it LKG;
- within a window of the last 30 days and the last 20 G4 jobs, whichever is smaller;
- sets `pool` and `capability` only, and from agent families only the **visible** instances. Holdout, canary and sealed results (the gate's own holdout, the sealed halves of agent families, and S) are excluded (awareness.md §3.7.1).

**Computation** (deterministic; the primitives are kernel/gate.md §3.8.3's):

```text
for each key (category, source, model):
    category ∈ the task's labels ∪ {"_all"}                 // a task with k labels counts in each of them
    source   = "agent"   if task source.kind = "agent"       // visible family instances only
               "pool"    otherwise                           // owner, maintainer, session tasks
    model    = the run's default model (its scope label, kernel/proxy.md §3.5a), "<provider>/<model>"
    T        = the distinct tasks with runs for this key
    S_i      = mean over task i's runs of score_permille / 1000
    pass_permille        = round(1000 × mean_i S_i)
    [lo, hi]_permille    = 2.5% and 97.5% percentiles of B = 2,000 cluster-bootstrap means over tasks,
                           rng = SplitMix64(seed_u64(lkg_sha, "capability/" + category + "/" + source + "/" + model));
                           lo = hi = pass if |T| = 1
    cost_ref_musd_p50    = median over tasks of mean cost_ref_musd;   wall_ms_p50 likewise
    low_n                = |T| < 3
    prev_pass_permille   = the same key's pass_permille for the previous LKG, else null
```

- **No redaction needed.** Holdout, canary and sealed results never enter the model (above), so nothing in it needs redacting at any level.
- **The sealed audit, separately.** `live.sealed_audit` copies the Root's latest sealed-audit summary verbatim, if the Root publishes one (root/sealed-eval.md). It is aggregate only, never task contents, and `null` otherwise. It is kept outside `capability` so that awareness.md's capability view contains only measurements on tasks the agent can see. It is the one number the agent's own tasks cannot shape.
- **Sorting.** `categories` is sorted by (category, source, model). The agent sorts by `pass_permille` itself (§4.4 recipes).
- **What it is not.** It is not a gate input and not a promotion criterion. It describes the current LKG on the gate's task mix, which the agent partly wrote (from L1). `low_n` and the interval say how much to trust each row.

### 3.13 Kernel inventory (`kernel-self.json`)

From L3 the kernel is agent-evolvable (spec v0.3 §15), so its files join the verified inventory. The kernel's source is its own repo, `lineage/kernel.git`, seeded from the maintainer's release and read-only to the agent. The agent clones it to `work/kernel/` and works on `kevo/<id>` branches (evolution.md §3.15.6, which owns the flow). The kernel tree therefore carries its own committed inventory, `kernel-self.json`, rather than a section of the body's `self.json`.

- **Generated by** `init-kernel selfmap --kernel --root <kernel worktree> --write` (or `--check`). In K0 the gate runs the same generator on the candidate's export and compares the result with the committed file, using §3.8's semantics: presence, generator version, equality, drift report. The code is `gate.k0.kernel_self_drift`.
- **Schema** (§4.2a): `{self_v: 2, generator, kernel: <kernel_tree>}`. `kernel_tree` holds `source_digest`, `budget` (`tokens`, `bytes`, `files`, `tests_tokens`), `files`, `modules`, `dependencies` (from the kernel's `Cargo.lock`) and `components`, all built as for the body (§3.2–§3.4).
- **Budget set:** `**/*.rs`, `**/Cargo.toml`, `**/build.rs`, `**/*.toml` config defaults (kernel/config.md §3.1). Excluded: `vendor/**`, `target/**`, `**/tests/**` (reported as `budget.tests_tokens`), `wit/**` (byte-checked as today). Kernel tokens never count toward the body's `body_tokens`.
- **Components** (generator table v2). The first top-level module directory or file under `crates/init-kernel/src/` maps as follows:
  - `supervise` → `kernel/supervise`
  - `proxy` → `kernel/proxy`
  - `gate` → `kernel/gate`
  - `selfmap` → `kernel/selfmap`
  - `serve`, and the whole `crates/initctl/` crate → `kernel/initctl`
  - `evo` → `kernel/evolution`
  - `sandbox` → `kernel/sandbox`
  - `config` → `kernel/config`
  - anything else counted → `kernel/other`

  This repository splits the kernel into lane crates (`crates/kernel-<area>/`), so the same table is applied to them: first the crate's own first module (`kernel-gate/src/evo/…` → `kernel/evolution`), then the area name (`kernel-gate/src/g1.rs` → `kernel/gate`). `crates/initctl/` and `crates/init-proto/` (the initctl wire types) both map to `kernel/initctl`.

  The module names are fixed in the build phase, and a rename is a generator version bump. Each component has `{id, tokens, paths, evolvable_from: "L3", contracts}`. `contracts` lists, from the same table, the cross-doc contracts that component must keep, so that a rewrite knows what other parts rely on. For example `kernel/gate` → `kernel/gate.md §4.1a`; `kernel/proxy` → `kernel/proxy.md §7`; `kernel/selfmap` → `kernel/selfmap.md §4.2`; `kernel/initctl` → `kernel/initctl.md wire protocol`. Contracts the build chose where this doc gave none: `kernel/supervise` → `kernel/supervise.md §4.1-§4.5`; `kernel/evolution` → `evolution.md §3.8`; `kernel/sandbox` → `kernel/sandbox.md §4`; `kernel/config` → `kernel/config.md §3.1`.
- **Not markers.** `@init compensates-for` markers in the kernel tree are ignored, and their lines are reported in `warnings`. Thinning applies to body scaffolds only (evolution.md §3.12).
- **Below L3** the same file exists for the maintainer's kernel, if `lineage/kernel.git` is installed at that level (root/root.md and supervise.md decide, §9). The agent can read it but cannot change it: the served view shows `evolvable: false`.
- **Never the Root.** Nothing of `init-root` is inventoried, at any level. The kernel tree contains no Root code (evolution.md §3.15.6). The agent can read its charter and Root lineage, but the Root's code is not part of "what am I made of" (spec v0.3 §13).

## 4. Interfaces

### 4.1 CLI

```
init-kernel selfmap --root <dir> --binary <path> --components <dir> --out <file>   gate mode (G1)
init-kernel selfmap --root <dir> --write                                          agent: build, generate, write <dir>/self.json
init-kernel selfmap --root <dir> --check                                          agent: build, generate, compare; diff on stdout
init-kernel selfmap --root <dir> --print [--no-build]                             print to stdout; with --no-build, needs --binary/--components
init-kernel selfmap --describe <component.wasm>                                   child mode: call describe() twice, print canonical JSON
init-kernel selfmap --capability --results <dir> --lkg <sha> [--prev <sha>]       offline: print the `capability` object of §3.12 (tests, owner)
init-kernel selfmap --kernel --root <dir> --write|--check|--out <file>          kernel tree: kernel-self.json (§3.13)
any body mode: --manifest-json <file>                                           read the manifest from a file instead of running --manifest
```

Serve mode (§3.11) has no CLI: it runs in-process in supervise and writes `state/kernel/self.json`.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | ok / no drift |
| 1 | drift (`--check`) or a G1 check failed |
| 2 | usage error |
| 3 | build failed (`--write`/`--check`) |
| 4 | generator error (parse, manifest, component) |

Diagnostics go to stderr as JSON lines `{"code","message","path"?,"line"?}`. Two more codes exist: `usage` (exit 2) and `build_failed` (exit 3).

- `--manifest-json <file>` takes the manifest's stdout bytes from a file instead of running `env -i <binary> --manifest` (a stub from before the body's `--manifest` existed; the tests use it). The same parse and checks apply.
- `--check` prints `<code>` and then the drift report on stdout (`self_json_drift`, or `kernel_self_drift` with `--kernel`); warnings go to stderr.
- A generator error (exit 4) writes no file. A check failure (exit 1) still writes the file.

**Output form.** `self.json` is written as pretty-printed ICJ: keys sorted byte-wise, 2-space indent, `\n` line ends and a final newline, integers only. It is pretty rather than single-line so `git diff` of a regeneration is readable. Equality in G1 is semantic (§3.8), so the form matters only for readability.

### 4.2 `self.json` JSON Schema (`self_v` 2)

v2 = v1 plus the optional `live` (§3.11–§3.12), plus a refactor that changes no meaning: `file` and `dependency` become `$defs`, so that `kernel-self.json` (§4.2a) can reuse them. The committed file has no `live` key (D12). The served view adds `live` and is otherwise identical.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://init.local/schemas/self-v2.json",
  "type": "object",
  "additionalProperties": false,
  "required": ["self_v", "generator", "source_digest", "body", "budget", "components", "tools", "extensions",
               "skills", "modules", "hooks", "profiles", "markers", "dependencies", "files", "manifest", "warnings",
               "body_tokens"],
  "properties": {
    "self_v": {"const": 2},
    "generator": {
      "type": "object", "additionalProperties": false, "required": ["name", "version", "tokenizer"],
      "properties": {"name": {"const": "init-kernel selfmap"},
                     "version": {"type": "integer", "minimum": 1},
                     "tokenizer": {"const": "o200k_base"}}},
    "source_digest": {"$ref": "#/$defs/sha"},
    "body_tokens": {"type": "integer", "minimum": 0},
    "body": {
      "type": "object", "additionalProperties": false, "required": ["crate", "version", "edition"],
      "properties": {"crate": {"type": "string"}, "version": {"type": "string"},
                     "edition": {"type": "string"}, "rust_version": {"type": ["string", "null"]}}},
    "budget": {
      "type": "object", "additionalProperties": false,
      "required": ["tokens", "bytes", "files", "tests_tokens", "uncounted_tokens"],
      "properties": {"tokens": {"type": "integer"}, "bytes": {"type": "integer"}, "files": {"type": "integer"},
                     "tests_tokens": {"type": "integer"}, "uncounted_tokens": {"type": "integer"}}},
    "components": {"type": "array", "items": {"$ref": "#/$defs/component"}},
    "tools": {
      "type": "object", "additionalProperties": false, "required": ["sha256", "list"],
      "properties": {"sha256": {"$ref": "#/$defs/sha"},
                     "list": {"type": "array", "items": {"$ref": "#/$defs/tool"}}}},
    "extensions": {"type": "array", "items": {"$ref": "#/$defs/extension"}},
    "skills": {"type": "array", "items": {"$ref": "#/$defs/skill"}},
    "modules": {"type": "array", "items": {"$ref": "#/$defs/module"}},
    "hooks": {"type": "array", "maxItems": 0},
    "profiles": {"type": "array", "items": {
      "type": "object", "additionalProperties": false, "required": ["family", "paths", "tokens"],
      "properties": {"family": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}},
                     "tokens": {"type": "integer"}}}},
    "markers": {"type": "array", "items": {"$ref": "#/$defs/marker"}},
    "dependencies": {"type": "array", "items": {"$ref": "#/$defs/dependency"}},
    "files": {"type": "array", "items": {"$ref": "#/$defs/file"}},
    "live": {"$ref": "#/$defs/live", "description": "served view only (state/kernel/self.json); never in the committed file (D12)"},
    "manifest": {
      "type": "object",
      "description": "the --manifest object verbatim (body/boot.md §4.3) without 'tools'; its 'tools_sha256' kept as reported",
      "required": ["manifest"],
      "properties": {"manifest": {"type": "integer"}, "tools_sha256": {"type": ["string", "null"]}}},
    "warnings": {"type": "array", "items": {"type": "string"}}
  },
  "$defs": {
    "sha": {"type": "string", "pattern": "^sha256:[0-9a-f]{64}$"},
    "slug": {"type": "string", "pattern": "^[a-z0-9-]{3,40}$"},
    "level": {"enum": ["L0", "L1", "L2", "L3", "L4"]},
    "file": {
      "type": "object", "additionalProperties": false, "required": ["path", "sha256", "bytes", "tokens", "counted"],
      "properties": {"path": {"type": "string"}, "sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
                     "bytes": {"type": "integer"}, "tokens": {"type": ["integer", "null"]},
                     "counted": {"type": "boolean"}}},
    "dependency": {
      "type": "object", "additionalProperties": false, "required": ["name", "version", "checksum"],
      "properties": {"name": {"type": "string"}, "version": {"type": "string"},
                     "checksum": {"type": ["string", "null"], "pattern": "^[0-9a-f]{64}$"}}},
    "kernel_tree": {
      "type": "object", "additionalProperties": false,
      "required": ["source_digest", "budget", "components", "modules", "files", "dependencies"],
      "properties": {
        "source_digest": {"$ref": "#/$defs/sha"},
        "budget": {"type": "object", "additionalProperties": false, "required": ["tokens", "bytes", "files", "tests_tokens"],
                   "properties": {"tokens": {"type": "integer"}, "bytes": {"type": "integer"},
                                  "files": {"type": "integer"}, "tests_tokens": {"type": "integer"}}},
        "components": {"type": "array", "items": {"$ref": "#/$defs/kernel_component"}},
        "modules": {"type": "array", "items": {"$ref": "#/$defs/module"}},
        "files": {"type": "array", "items": {"$ref": "#/$defs/file"}},
        "dependencies": {"type": "array", "items": {"$ref": "#/$defs/dependency"}}}},
    "kernel_component": {
      "type": "object", "additionalProperties": false, "required": ["id", "tokens", "paths", "evolvable_from", "contracts"],
      "properties": {"id": {"type": "string", "pattern": "^kernel/[a-z0-9_-]{1,40}$"},
                     "tokens": {"type": "integer", "minimum": 0},
                     "paths": {"type": "array", "items": {"type": "string"}},
                     "evolvable_from": {"const": "L3"},
                     "contracts": {"type": "array", "items": {"type": "string"}}}},
    "live": {
      "type": "object", "additionalProperties": false,
      "required": ["as_of", "running_sha", "lkg_sha", "level", "ladder", "kernel", "capability", "sealed_audit"],
      "properties": {
        "as_of": {"type": "string", "format": "date-time"},
        "running_sha": {"type": "string", "pattern": "^[0-9a-f]{40}$"},
        "lkg_sha": {"type": "string", "pattern": "^[0-9a-f]{40}$"},
        "level": {"type": "object", "additionalProperties": false, "required": ["current", "max", "since"],
                  "properties": {"current": {"$ref": "#/$defs/level"}, "max": {"$ref": "#/$defs/level"},
                                 "since": {"type": "string", "format": "date-time"}}},
        "ladder": {"type": ["object", "null"], "description": "verbatim from the Root's published ladder state (root/root.md R7)"},
        "kernel": {"type": "object", "additionalProperties": false,
                   "required": ["version", "build_sha256", "kernel_sha", "source_digest", "verified", "evolvable",
                                "budget_tokens", "components"],
                   "properties": {"version": {"type": "string"}, "build_sha256": {"type": "string"},
                                  "kernel_sha": {"type": ["string", "null"], "pattern": "^[0-9a-f]{40}$"},
                                  "source_digest": {"oneOf": [{"type": "null"}, {"$ref": "#/$defs/sha"}]},
                                  "verified": {"type": "boolean"}, "evolvable": {"type": "boolean"},
                                  "budget_tokens": {"type": ["integer", "null"]},
                                  "components": {"type": "array", "items": {
                                    "type": "object", "additionalProperties": false, "required": ["id", "tokens", "contracts"],
                                    "properties": {"id": {"type": "string"}, "tokens": {"type": "integer"},
                                                   "contracts": {"type": "array", "items": {"type": "string"}}}}}}},
        "capability": {"$ref": "#/$defs/capability"},
        "sealed_audit": {"type": ["object", "null"], "description": "verbatim from the Root's latest sealed-audit summary; aggregate only"}}},
    "capability": {
      "type": "object", "additionalProperties": false, "required": ["method", "lkg_sha", "window", "categories"],
      "properties": {
        "method": {"const": "capability-v1"},
        "lkg_sha": {"type": "string", "pattern": "^[0-9a-f]{40}$"},
        "window": {"type": "object", "additionalProperties": false, "required": ["from", "to", "gates", "runs"],
                   "properties": {"from": {"type": ["string", "null"]}, "to": {"type": ["string", "null"]},
                                  "gates": {"type": "integer"}, "runs": {"type": "integer"}}},
        "categories": {"type": "array", "items": {"$ref": "#/$defs/capability_entry"}}}},
    "capability_entry": {
      "type": "object", "additionalProperties": false,
      "required": ["category", "source", "model", "tasks", "runs", "pass_permille", "lo_permille", "hi_permille",
                   "cost_ref_musd_p50", "wall_ms_p50", "last_run", "low_n", "prev_pass_permille"],
      "properties": {"category": {"type": "string", "pattern": "^([a-z0-9-]{1,40}|_all)$"},
                     "source": {"enum": ["pool", "agent"]},
                     "model": {"type": "string"},
                     "tasks": {"type": "integer", "minimum": 1}, "runs": {"type": "integer", "minimum": 1},
                     "pass_permille": {"type": "integer", "minimum": 0, "maximum": 1000},
                     "lo_permille": {"type": "integer", "minimum": 0, "maximum": 1000},
                     "hi_permille": {"type": "integer", "minimum": 0, "maximum": 1000},
                     "cost_ref_musd_p50": {"type": "integer"}, "wall_ms_p50": {"type": "integer"},
                     "last_run": {"type": "string", "format": "date-time"},
                     "low_n": {"type": "boolean"},
                     "prev_pass_permille": {"type": ["integer", "null"]}}},
    "compensates": {"type": "string", "pattern": "^(none|[a-z0-9-]{3,40})$"},
    "component": {
      "type": "object", "additionalProperties": false,
      "required": ["id", "kind", "core", "tokens", "paths", "compensates_for", "compensates_note"],
      "properties": {
        "id": {"type": "string", "pattern": "^(mod|tool|ext|hook|skill|profile|prompt)/[A-Za-z0-9_./-]{1,200}$"},
        "kind": {"enum": ["mod", "tool", "ext", "hook", "skill", "profile", "prompt"]},
        "core": {"type": "boolean"},
        "tokens": {"type": "integer", "minimum": 0},
        "paths": {"type": "array", "items": {"type": "string"}},
        "compensates_for": {"$ref": "#/$defs/compensates"},
        "compensates_note": {"type": ["string", "null"], "maxLength": 200}}},
    "tool": {
      "type": "object", "additionalProperties": false,
      "required": ["name", "description", "input_schema", "schema_sha256", "tokens"],
      "properties": {"name": {"type": "string"}, "description": {"type": "string"},
                     "input_schema": {"type": "object"}, "schema_sha256": {"$ref": "#/$defs/sha"},
                     "tokens": {"type": "integer"}}},
    "extension": {
      "type": "object", "additionalProperties": false,
      "required": ["name", "crate", "api", "wit_sha256", "wit_exports", "wit_imports", "describe"],
      "properties": {
        "name": {"type": "string", "pattern": "^[a-z][a-z0-9-]{0,31}$"},
        "crate": {"type": "string"},
        "api": {"type": "string", "pattern": "^init:ext@[0-9]+$"},
        "wit_sha256": {"$ref": "#/$defs/sha"},
        "wit_exports": {"type": "array", "items": {"$ref": "#/$defs/wit_item"}},
        "wit_imports": {"type": "array", "items": {"$ref": "#/$defs/wit_item"}},
        "describe": {
          "type": "object", "additionalProperties": false,
          "required": ["name", "description", "input_schema", "version", "timeout_ms"],
          "properties": {"name": {"type": "string"}, "description": {"type": "string"},
                         "input_schema": {"type": "object"}, "version": {"type": "string"},
                         "timeout_ms": {"type": ["integer", "null"]}}}}},
    "wit_item": {
      "type": "object", "additionalProperties": false, "required": ["name", "kind"],
      "properties": {"name": {"type": "string"}, "kind": {"enum": ["func", "interface", "type"]},
                     "signature": {"type": "string"}, "functions": {"type": "array", "items": {"type": "string"}}}},
    "skill": {
      "type": "object", "additionalProperties": false, "required": ["name", "description", "path", "tokens"],
      "properties": {"name": {"type": "string"}, "description": {"type": "string"},
                     "path": {"type": "string"}, "tokens": {"type": "integer"}}},
    "module": {
      "type": "object", "additionalProperties": false, "required": ["path", "tokens", "lines", "items", "forbid_unsafe"],
      "properties": {
        "path": {"type": "string"}, "tokens": {"type": "integer"}, "lines": {"type": "integer"},
        "forbid_unsafe": {"type": "boolean"},
        "items": {"type": "array", "items": {
          "type": "object", "additionalProperties": false,
          "required": ["kind", "name", "vis", "line", "end_line", "tokens"],
          "properties": {"kind": {"enum": ["fn", "struct", "enum", "trait", "impl", "mod", "const", "static", "type", "macro", "union", "other"]},
                         "name": {"type": "string"}, "vis": {"enum": ["pub", "crate", "restricted", "private"]},
                         "line": {"type": "integer"}, "end_line": {"type": "integer"}, "tokens": {"type": "integer"}}}}}},
    "marker": {
      "type": "object", "additionalProperties": false,
      "required": ["component", "compensates_for", "note", "path", "line", "end_line", "item", "tokens"],
      "properties": {"component": {"type": "string"}, "compensates_for": {"$ref": "#/$defs/compensates"},
                     "note": {"type": ["string", "null"]}, "path": {"type": "string"},
                     "line": {"type": "integer"}, "end_line": {"type": "integer"},
                     "item": {"type": ["string", "null"]}, "tokens": {"type": "integer"}}}
  }
}
```

Array orders (part of the determinism contract):

| Array | Order |
|---|---|
| `components`, `files`, `modules`, `skills`, `profiles` (and the same arrays in `kernel-self.json`) | by id, path or name |
| `live.capability.categories` | by (category, source, model) |
| `live.kernel.components` | by id |
| `tools.list` | by name |
| `extensions` | by name |
| `wit_exports`, `wit_imports` | by name |
| `markers` | by (path, line) |
| `dependencies` (and `kernel.dependencies` in `kernel-self.json`) | by (name, version) |
| `module.items` | by line |
| `warnings` | sorted, deduplicated |

`dependencies` comes from `Cargo.lock`, parsed with the `toml` crate (1.1.8). Only `[[package]]` entries with a `source` (registry packages; workspace members have none) are listed.

### 4.2a `kernel-self.json` (§3.13)

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://init.local/schemas/kernel-self-v2.json",
  "type": "object", "additionalProperties": false,
  "required": ["self_v", "generator", "kernel", "warnings"],
  "properties": {
    "self_v": {"const": 2},
    "generator": {"$ref": "self-v2.json#/properties/generator"},
    "kernel": {"$ref": "self-v2.json#/$defs/kernel_tree"},
    "warnings": {"type": "array", "items": {"type": "string"}}
  }
}
```

Output form, array orders and the drift semantics are those of the body file (§3.8, §4.1).

### 4.3 Example (abridged, seed body, committed file)

```json
{
  "body": {"crate": "init-body", "edition": "2024", "rust_version": null, "version": "0.1.0"},
  "budget": {"bytes": 84120, "files": 31, "tests_tokens": 6120, "tokens": 21873, "uncounted_tokens": 0},
  "components": [
    {"compensates_for": "none", "compensates_note": null, "core": false, "id": "mod/src/context.rs",
     "kind": "mod", "paths": ["src/context.rs"], "tokens": 3410},
    {"compensates_for": "none", "compensates_note": "new capability", "core": false, "id": "ext/pty",
     "kind": "ext", "paths": ["ext/pty/Cargo.toml", "ext/pty/src/lib.rs"], "tokens": 1840},
    {"compensates_for": "none", "compensates_note": null, "core": true, "id": "tool/bash",
     "kind": "tool", "paths": ["src/tools/bash.rs"], "tokens": 1288}
  ],
  "dependencies": [{"checksum": "…64 hex…", "name": "serde_json", "version": "1.0.154"}],
  "body_tokens": 21873,
  "extensions": [{"api": "init:ext@1", "crate": "ext-pty",
                  "describe": {"description": "Drive an interactive program through a pseudo-terminal.",
                               "input_schema": {"properties": {"…": {}}, "type": "object"},
                               "name": "pty", "timeout_ms": 60000, "version": "0.1.0"},
                  "name": "pty", "wit_exports": [{"kind": "func", "name": "call", "signature": "func(input: string) -> result<string, tool-error>"},
                                                  {"kind": "func", "name": "describe", "signature": "func() -> tool-info"}],
                  "wit_imports": [{"functions": ["close", "run", "sessions", "spawn", "write"], "kind": "interface", "name": "init:ext/process@1.0.0"}],
                  "wit_sha256": "sha256:…"}],
  "generator": {"name": "init-kernel selfmap", "tokenizer": "o200k_base", "version": 2},
  "hooks": [],
  "self_v": 2,
  "source_digest": "sha256:…",
  "tools": {"list": [{"description": "Run a command with bash -c …", "input_schema": {"…": {}}, "name": "bash",
                      "schema_sha256": "sha256:…", "tokens": 212}], "sha256": "sha256:51ab…"},
  "warnings": []
}
```

The same version's **served view** (`state/kernel/self.json`) differs only in `live` (abridged; the `ladder` content is the Root's, shown here as an illustration only):

```json
"live": {
  "as_of": "2026-11-20T09:00:00.000Z",
  "capability": {
    "categories": [
      {"category": "_all", "cost_ref_musd_p50": 21000, "hi_permille": 790, "last_run": "2026-11-19T22:10:00.000Z",
       "lo_permille": 620, "low_n": false, "model": "deepseek/deepseek-flash", "pass_permille": 710,
       "prev_pass_permille": 690, "runs": 96, "source": "pool", "tasks": 24, "wall_ms_p50": 140000},
      {"category": "interactive", "cost_ref_musd_p50": 26000, "hi_permille": 1000, "last_run": "2026-11-19T22:10:00.000Z",
       "lo_permille": 750, "low_n": false, "model": "deepseek/deepseek-flash", "pass_permille": 917,
       "prev_pass_permille": 250, "runs": 24, "source": "agent", "tasks": 6, "wall_ms_p50": 151000},
      {"category": "refactor", "cost_ref_musd_p50": 30000, "hi_permille": 600, "last_run": "2026-11-18T03:00:00.000Z",
       "lo_permille": 100, "low_n": true, "model": "deepseek/deepseek-flash", "pass_permille": 350,
       "prev_pass_permille": null, "runs": 4, "source": "pool", "tasks": 2, "wall_ms_p50": 233000}],
    "lkg_sha": "0e786c733a9648164a2b4c4aea7ece24d0beb7c7",
    "method": "capability-v1",
    "window": {"from": "2026-10-21T00:00:00.000Z", "gates": 9, "runs": 412, "to": "2026-11-19T22:10:00.000Z"}},
  "kernel": {"budget_tokens": 196400, "build_sha256": "…",
             "components": [{"contracts": ["kernel/gate.md §4.1a"], "id": "kernel/gate", "tokens": 41200},
                            {"contracts": ["kernel/proxy.md §7"], "id": "kernel/proxy", "tokens": 38800}],
             "evolvable": false, "kernel_sha": "…40 hex…", "source_digest": "sha256:…", "verified": true, "version": "0.3.0"},
  "ladder": {"…": "verbatim from INIT_HOME/root/ledger.json (root/root.md §3.13)"},
  "level": {"current": "L1", "max": "L3", "since": "2026-11-02T04:00:00.000Z"},
  "lkg_sha": "0e786c733a9648164a2b4c4aea7ece24d0beb7c7",
  "running_sha": "0e786c733a9648164a2b4c4aea7ece24d0beb7c7",
  "sealed_audit": {"…": "verbatim from the Root"}}
```

### 4.4 Query recipes (installed as `kernel/docs/selfmap.md`; awareness.md §3.7)

```sh
jq '.body_tokens, .budget' body/self.json                                # how big am I
jq -r '.components[] | "\(.tokens)\t\(.id)"' body/self.json | sort -rn | head   # biggest parts
jq '.components[] | select(.compensates_for != "none") | {id, compensates_for, tokens}' body/self.json
jq -r '.tools.list[].name' body/self.json                                # tools the model is sent
jq '.extensions[] | {name, wit_exports}' body/self.json                  # T1 exports, from the binary
jq -r '.modules[] | .path as $p | .items[] | "\($p):\(.line) \(.kind) \(.name)"' body/self.json  # where things live

S=$INIT_HOME/state/kernel/self.json                                      # the served view (v0.3)
jq '.live.level, .live.ladder' $S                                        # where am I on the ladder
jq -r '.live.capability.categories[] | select(.category != "_all" and (.low_n | not))
       | "\(.pass_permille)\t[\(.lo_permille)-\(.hi_permille)]\t\(.category)\t\(.source)\t\(.model)"' $S | sort -rn
                                                                         # what am I good at, best first (measured)
jq '.live.sealed_audit' $S                                               # the one number my own tasks cannot shape
jq -r '.live.kernel.components[] | "\(.tokens)\t\(.id)\t\(.contracts | join(", "))"' $S
                                                                         # my kernel and the contracts each part keeps
git -C $INIT_HOME/lineage/kernel.git show "$(jq -r .live.kernel.kernel_sha $S):kernel-self.json" | jq '.kernel.budget'
                                                                         # the full kernel inventory (when the repo exists)
```

These confirm the key names awareness.md assumed (`compensates_for`, `input_schema`, `wit_exports`, `id`, `body_tokens`), answering its X4. The v0.3 recipes use the served view for `live` and the committed file for everything else. `init-kernel install` copies this section into `$INIT_HOME/kernel/docs/selfmap.md` (cross-doc: kernel/supervise.md owns install).

## 5. Failure modes and recovery

| Failure | Effect | Recovery |
|---|---|---|
| Agent forgot to regenerate | G1 `self_json_drift` listing changed files | `init-kernel selfmap --root <wt> --write`, amend, promote again |
| Kernel upgraded the generator | `generator_changed` on the next gate | same command once |
| syn cannot parse new syntax | `selfmap_error` at file:line | agent avoids the construct, or maintainer bumps syn |
| Manifest prints logs or reads env | `manifest_error` | body/boot.md §4.3 contract violated; fix the body |
| Component traps or loops in `describe` | fuel exhausted → `ext_contract` | fix the component (describe must be pure) |
| Non-UTF-8 or > 1 MiB file | `tokens: null`, warning | none needed unless it should count |
| Untracked file present at `--write` but not committed | digest differs → drift names the file | commit or delete it |
| Agent commits a served view (`live` filled) | G1 ignores `live` and warns `live_ignored` (D12) | none needed; `init-kernel selfmap --write` never writes `live` |
| Kernel upgraded to generator v2 | `generator_changed` once | same command once (`self_v` 2) |
| Root ladder publication unreachable | `live.ladder` stale | keep the last copy, with `as_of` unchanged for that field; `live.level` falls back to the level supervise last received |
| No G4 yet for the current LKG (fresh install, or right after a baseline) | `live.capability.categories` empty | expected: `window.gates = 0`; filled after the next G4 |
| Results file of an old LKG deleted | `prev_pass_permille` null | expected; only the last 5 LKGs are kept (kernel/gate.md §4.2) |
| Kernel tree fails to parse, or `kernel-self.json` is stale | K0 fails `gate.k0.selfmap_error` or `gate.k0.kernel_self_drift` | `init-kernel selfmap --kernel --root <kwt> --write`, amend, promote again |
| No `lineage/kernel.git` (below L3, if not installed) | `live.kernel.kernel_sha` null, `verified: false` | expected; the embedded component table is shown |

## 6. Performance notes (spec §9)

- **Rank 4, self-edit latency.** Generation targets < 2 s for a 25k-token body plus a few components:
  - syn parsing: milliseconds;
  - `o200k_base` initialization: about 100–300 ms the first time per process (UNVERIFIED; measured in Phase 0);
  - counting: under 10 ms;
  - `--manifest`: a process spawn;
  - Cranelift compile at `OptLevel::None` per component: tens to hundreds of ms.
- **Rank 1, cache.** `self.json` is never in the prompt (awareness.md §3.7: on demand), so its size and churn cost no cache. It *protects* the cache: the tools hash links the declared tools to the proxy-observed ones.
- **Rank 5, footprint.** The `files` and `modules.items` lists grow linearly with the body. At 25k tokens, `self.json` is about 30–60 KB. The kernel inventory lives in its own file (`kernel-self.json`), roughly the same size per token of source; a kernel of 150–250k tokens makes it a few hundred KB (estimate, measured in Phase 3). The served view carries only the kernel's component summary. Neither is ever in the prompt.
- **Serve mode.** It rebuilds from in-memory data and at most 20 jobs' compacted results (a few thousand run records). The capability bootstrap is 2,000 resamples per key over tens of tasks: milliseconds. It is triggered by events and rate-limited to once a minute.

## 7. Invariants and security (spec §2.1)

| Invariant | How |
|---|---|
| #2 Honest measuring stick | tools come from the binary (`env -i --manifest` in a no-capability sandbox) and are re-verified against what the proxy saw; T1 exports and `describe()` come from the kernel-built component, run with no capabilities; markers are cross-checked against trailers; drift fails the gate |
| #3 Blast radius | gate-mode generation runs in the `build` sandbox profile; agent code it executes (`--manifest`, `describe`) gets no network, no sockets, no env, and (for `describe`) no WASI capabilities at all |
| #4 Lineage | `source_digest` binds the inventory to an exact tree; the regenerated file is kept in `vault/gate/jobs/<job>/self.json`; the level and ladder in `live` are copies of the Root's lineage-backed state (R4, R7), never computed by the kernel |
| Ladder honesty (spec v0.3 §15) | the agent cannot claim a level or a capability by committing it: `live` must be null in a commit (G1 check 7), and the served view is written only by the kernel from Root and gate data |

Known limits:
- `self.json` describes what is *declared and built*. Untagged scaffolds and behaviour not visible in names (for example a prompt rule hidden in a tool description) are covered by the prefix hash and G4, not by the self-map.
- **From L3 the generator itself is agent-evolvable** (`kernel/selfmap`). A weakened generator could hide drift. That is a case of "disable or weaken a check", which the gate-of-gates targets (spec v0.3 §16; kernel/gate.md §4.1a), and the sealed audit still measures behaviour. The self-map is evidence, not an anchor.
- The capability self-model reflects the gate's task mix, which from L1 the agent partly writes. `source` separates agent tasks from pool tasks, and `live.sealed_audit` is the Root's independent number.

## 8. Test plan

| # | Case | Expectation |
|---|---|---|
| S-01 | determinism: generate twice on the same fixture tree and binaries; once with a different `HOME`, `TZ` and locale | byte-identical output |
| S-02 | drift: edit one function in `src/loop.rs`, keep the old `self.json` | `self_json_drift`; report lists `src/loop.rs` first and the JSON pointers `/files/…/sha256`, `/modules/…/tokens` |
| S-03 | formatting-only edit of `self.json` | pass + `non_canonical_format` |
| S-04 | generator version bump | `generator_changed` |
| S-05 | `#![forbid(unsafe_code)]` detection: present, absent, `#![forbid(unsafe_code, missing_docs)]`, inside `cfg_attr` (not accepted) | true / false / true / false |
| S-06 | markers: `//!` file marker, `///` item marker, two different slugs in one file, marker in a `//` comment, SKILL.md front matter, GENESIS.md block | attach as §3.7; `marker_conflict`; `marker_not_doc` warning |
| S-07 | trailer cross-check: `feat` adding `ext/pty` with marker `none` but trailer `Compensates-for: loops-on-repl` | `tag_marker_mismatch` |
| S-08 | WIT extraction on fixture components checked into `kernel/tests/fixtures/` (built once from ext/authoring.md's template) | exports `call`, `describe`; imports `init:ext/process@1.0.0` when used (ext/wit.md W-2/W-3) |
| S-09 | `describe` that loops, that reads a file, that returns different bytes on the second call | `ext_contract` (fuel, no preopen, nondeterminism) |
| S-10 | manifest tools vs proxy: normalize the openai-chat, anthropic and responses renderings of tools.md §4.1 | all three give the same `tools.sha256` as the manifest (shared test vectors with proxy P-13) |
| S-11 | token count fixture: a 10 KB Rust file with a checked-in expected `o200k_base` count | exact; a regression pin of 2,638 tokens, because no independent `o200k_base` implementation is available offline. The ids for `hello world` (`[24912, 2375]`) are asserted as a sanity check |
| S-12 | performance: seed body + 3 components | < 2 s wall on the M2 |
| S-13 | D12: commit a copy of `state/kernel/self.json`; then change only a pass rate in the served copy | G1 passes with the warning `live_ignored`; the pass-rate change never causes drift |
| S-14 | served view: rebuild after a boot, an LKG change, a level change (mock Root L0 → L1) and a G4 verdict | `live.level.current` follows the Root; `running_sha`/`lkg_sha` match `boot.json`/`lkg.json`; every field outside `live` is byte-identical to `$G show <running>:self.json` |
| S-15 | capability determinism: `--capability` on a fixture results directory, twice and with a different `TZ` | byte-identical; `pass_permille`, `lo`, `hi` match hand-computed values for a 3-task fixture |
| S-16 | capability exclusions: fixture with holdout runs, canary runs, sealed family instances and visible instances | only pool and visible-instance runs appear; no holdout, canary or sealed rows at any level |
| S-17 | capability sources and models: agent-authored and owner tasks with the same label, and runs under two default models | separate rows per `source` and per `model`; `low_n` set below 3 tasks |
| S-18 | kernel inventory: `--kernel` on a fixture kernel tree with modules `gate`, `proxy`, `serve`, `misc` and an `@init` marker | `kernel.components` = `kernel/gate`, `kernel/initctl`, `kernel/other`, `kernel/proxy` with their contracts; the marker is ignored with a warning; `--check` after editing `gate/mod.rs` reports drift naming that file |
| S-19 | served `live.kernel`: with a kernel repo whose admitted build digest matches the running binary, with one that does not, and with no kernel repo | `verified` true / false / false; `kernel_sha` null and the embedded table in the third case; `evolvable` follows the level (L3 → true) |

## 9. Open questions

**Within this doc**
1. Should `files` list every file, or only counted ones? Every file today, so drift reports can name any changed file. Revisit if `self.json` passes 100 KB. From L3 the kernel inventory makes that likely; the first candidate to drop is `kernel.files` entries for uncounted files.
2. Should the capability self-model weight recent runs more, or use only the current LKG's runs as now? Only the current LKG, because it is simplest to read, and `prev_pass_permille` shows the trend. Revisit when LKGs change faster than gates accumulate per-label data.
3. Should categories come only from task labels, or also from the `Predicts:` labels the agent claims? Only from labels: claims are hypotheses, and labels are attached to tasks that were measured.

**Cross-doc** (v0.3 round 2; items addressed to this doc are settled in reconciliation/v03-kernel-gate.md; the R2 items stay settled: the manifest hash, the `prompt/` component kind, the profile location, the describe import set, G1 in the `build` profile, and the recipes):
- **Q-RT2 → root/root.md: `lineage/kernel.git` (D7: the Root writes it and supplies the L3 source bundle).** Is it also installed, read-only, below L3, so that the maintainer's kernel is inventoried at every level? (Recommended: yes.) And which R4 record carries the admitted build digest for `live.kernel.verified`?
- **Q-SE1 → root/sealed-eval.md:** publish an aggregate sealed-audit summary for `live.sealed_audit` (overall, and per category if S is labelled), with the date and δ_S, for example in `INIT_HOME/root/`. Never contents.
- **Q-SV1 → kernel/supervise.md:** run serve mode on the triggers of §3.11; add `state/kernel/self.json` to the runtime layout; embed the kernel component table in `init-kernel` at build time (for `live.kernel` when no kernel repo exists); install the new recipes into `kernel/docs/selfmap.md`.
- **Q-AW1 → awareness.md:** point the "what exactly am I made of" row and the genesis prompt's "self.json (your verified inventory)" at both files: `body/self.json` for the inventory, `state/kernel/self.json` for level, ladder and capability. Consider a one-line capability summary in the boot event (spec App. B), if the cache cost is acceptable.
- **Q-GT1 → kernel/gate.md** (same author; recorded for traceability): the compacted results files (`vault/gate/results/<sha12>.jsonl`) and RunRecord v2 fields `source`, `set` and `models` are inputs here; `gate.g1.live_in_commit` was dropped by D12 (a committed `live` is ignored).

## 10. Spec deltas

| Spec v0.2 | This doc | Why |
|---|---|---|
| §5 `self.json` includes "SHA, LKG" | replaced by `source_digest`; sha and LKG live in `boot.json`/`lkg.json` | a file cannot hold its own commit hash; LKG changes without commits |
| §5 "hooks" | always empty in 1.x | ext/wit.md defers hooks |
| §5 "token size" (no measure named) | `o200k_base` via tiktoken-rs, model-independent | trend and tie-break must not change with the model |
| §6.6 "Every scaffold carries a `Compensates-for:` tag" (location unspecified) | in-code `@init` markers (doc comments, front matter, HTML-comment blocks), one slug per component, cross-checked with trailers | the tag lives and dies with the code |
| (new) | `generator.version`; G1 fails `generator_changed` after kernel upgrades that change output | the generator is part of the measuring stick |

**v0.3 changes** (spec v0.3 Part II, ADR-007):

| Spec / v0.2 doc | This doc (v0.3) | Why |
|---|---|---|
| v0.2 `self_v` 1, one committed file | `self_v` 2: the committed file (tree-derived, drift-checked, no `live` key) plus a kernel-served view `state/kernel/self.json` with a `live` section; G1 compares committed fields only and ignores a committed `live` (`live_ignored`); generator version 2 | level, ladder and capability change without commits; v03-decisions D12 |
| spec §5 "SHA, LKG" (v0.2: moved out of `self.json`) | back in the served view as `live.running_sha` and `live.lkg_sha` (`boot.json`/`lkg.json` stay authoritative) | one place to answer "what am I now" |
| (new) | `live.level` and `live.ladder`, copied verbatim from the Root (R7) | spec v0.3 §15: the agent should know its level and what unlocks the next one; the Root computes, the kernel copies |
| (new) | kernel inventory: a committed `kernel-self.json` in `lineage/kernel.git` (files, modules, dependencies, components `kernel/<name>` with `evolvable_from` and `contracts`), drift-checked at K0; summarized in the served view as `live.kernel` with `verified` (build digest matches the admitted one) and `evolvable` (L3+) | spec v0.3 §15 L3: the kernel becomes agent-evolvable; evolution.md §3.15.6 puts the kernel source in its own repo |
| (new) | capability self-model `live.capability` (`capability-v1`): per (category, source, model) pass rates of the LKG with 95% cluster-bootstrap intervals, cost and wall medians, trend, `low_n`, and the Root's sealed summary verbatim | the agent knows what it is good at by measurement, from gate data at no extra cost |
| v0.2 holdout redaction (gate-owned) | the capability model excludes holdout, canary and sealed results entirely; the Root's sealed-audit summary is a separate `live.sealed_audit` | awareness.md §3.7.1; v03-decisions D2, D12 |
| (R1 follow-up E12) | the describe host's `bindgen!` reads the kernel tree's `wit/init-ext/v1` (`kernel/wit/init-ext/v1`) with world `init:ext/tool` | ext/wit.md §3.4, §4.1 |

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.11):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP9 D1 | `files` and `source_digest` omit `vendor/`, `target/` and `.git/` at the root, to keep the < 2 s target; `Cargo.lock` pins the vendored crates | §3.2 |
| build: WP9 D3 | as built, `bindgen!` reads the repo-root `wit/init-ext/v1`, because there is no `kernel/wit` yet; a `kernel/wit` is for L3 self-containment later | §3.6 |
| build: WP9 D2 | syn gets the `printing` feature (`Spanned` for item lines; only `quote` is added, already in the lock) | §3.3 |
| build: WP9 D4 | `src/tools/mod.rs` and `src/profiles/mod.rs` are `mod/` components; a file is `tool/<name>` only if `<name>` is a manifest tool; a marker in a file with no component (`build.rs`) gets `marker_unattached` | §3.7, §3.9 |
| build: WP9 D5 | marker regions: a file marker covers lines 1 to the last line; a skill marker runs from its front-matter line to the end of the file | §3.7 |
| build: WP9 D6 | `budget.uncounted_tokens` is the tokens of files that are neither in the budget nor tests | §3.2 |
| build: WP9 D7 | `describe` is called twice on one instance (static state is caught); `timeout_ms` is recorded as declared, `null` meaning the default | §3.6 |
| build: WP9 D9 | kernel components also map the lane crates `kernel-<area>` (first module, then the area) and `init-proto`; contracts chosen for `kernel/supervise`, `kernel/evolution`, `kernel/sandbox` and `kernel/config`; `--kernel --write\|--check` does not build `init-kernel` | §3.1, §3.13 |
| build: WP9 D11 | CLI: `--manifest-json`, `--check` prints `<code>` and the report on stdout, codes `usage` and `build_failed`, a generator error writes no file while a check failure still does, vendored-source flags only when `<root>/vendor/` exists (absolute path) | §3.1, §4.1 |
| build: WP9 D12 | the drift report names `kernel.files` for `kernel-self.json` | §3.8 |
| build: WP9 D13 | S-11 is a regression pin of 2,638 tokens, with the ids for `hello world` as a sanity check | §8 |
| build: WP9 D8, WP14 F (W-3) | `wit_imports[].functions` means the functions the component actually imports; the world's types are listed as `kind: type` | §3.6 |

## 11. References

All checked 2026-10-19 unless marked. Added in v0.3 (2026-10-20, this repo): spec v0.3 Part II §13, §15, §16; adr/007-root-and-autonomy-ladder.md; kernel/gate.md §3.7.1a, §3.8.3 (bootstrap primitives reused by §3.12), §3.10, §4.4.
- syn 3.0.9 — https://docs.rs/syn/3.0.9/syn/fn.parse_file.html , https://docs.rs/syn/3.0.9/syn/visit/trait.Visit.html , https://docs.rs/syn/3.0.9/syn/struct.Attribute.html
- proc-macro2 1.0.107 `span-locations` — https://docs.rs/proc-macro2/1.0.107/proc_macro2/struct.LineColumn.html
- wit-component 0.262.0 `decode`, `DecodedWasm`, `WitPrinter` — https://docs.rs/wit-component/0.262.0/wit_component/fn.decode.html , https://docs.rs/wit-component/0.262.0/wit_component/struct.WitPrinter.html
- wit-parser 0.262.0 `WorldItem` — https://docs.rs/wit-parser/0.262.0/wit_parser/enum.WorldItem.html
- wasm-tools `component wit` CLI (`-j/--json`) — https://raw.githubusercontent.com/bytecodealliance/wasm-tools/main/src/bin/wasm-tools/component.rs
- wasmtime / wasmtime-wasi =48.0.6 (LTS, ext/host.md §10.2; current 49.0.3 released 2026-10-14) — https://docs.rs/wasmtime-wasi/49.0.3/wasmtime_wasi/
- tiktoken-rs 0.12.1 (`o200k_base`, `CoreBPE::encode_ordinary`) — https://docs.rs/tiktoken-rs/0.12.1/tiktoken_rs/struct.CoreBPE.html
- HF tokenizers 0.23.2 default features (rejected) — https://crates.io/api/v1/crates/tokenizers/0.23.2
- DeepSeek V4.1-Flash tokenizer size (rejected option) — https://huggingface.co/api/models/deepseek-ai/DeepSeek-V4-Pro/tree/main
- toml 1.1.8 — https://crates.io/crates/toml
- RFC 6901 JSON Pointer — https://www.rfc-editor.org/rfc/rfc6901
- JSON Schema 2020-12 — https://json-schema.org/draft/2020-12/schema
- body/boot.md §4.3, body/tools.md §4.1, ext/wit.md §3.2–§3.7, evolution.md §3.1, §3.8–§3.12, awareness.md §3.7 (this repo)
