# FIX8: M-5 body RSS growth (lane B2, seed)

Status: **fix done; M-5 still fails as written, decision needed** (2026-10-20, 07:26–08:10 +07). Paths: `seed/src/context/project.rs`, `seed/src/context/engine.rs`, `seed/src/context/tests.rs`, `measurements/m5-fix8-{before,after}.json`. Mock only, $0. WP16's tool (`init-measure m5`) is unchanged.

## Cause
There is no leak. The body's live heap is bounded, and every compaction frees it: the troughs after compactions stay flat at 1.4–2.5 MiB. The measured growth has three parts.

1. **A real retention defect, now fixed.** log-context.md §3.7 bounds the view's live content by `T_trigger` tokens (about 0.8 MB). The view broke that bound. `View::window` held every event in full from `first_kept` to the leaf, and that included:
   - every **masked** tool output (the projection uses the placeholder stored in the `mask` event, so these are never read);
   - per-response metadata that nothing in the window reads: `usage.native`, `request`, `stop_details`, `response_id`, and the signal's `extra`.

   Masking runs first and postpones compaction by design (§3.11). In M-5 the masks come after steps 155, 252, 329, 400, 595, 691, 768 and 830, and the compactions after steps 465 and 899. So the window grew with the step count, not the token count. On top of that, each epoch briefly held two copies: `rebuild_view` built the new view while the old one was still alive, and `compaction_epoch` cloned the whole span (`window[..cut].to_vec()`) at the window's peak.

   Live heap, measured with `heap <pid>`: it peaked at **6.9 MiB** before each compaction (≈ 650 retained 2.5 KB output blocks), about 9 times §3.7's bound.
2. **The macOS allocator, a measurement artifact.** `ps -o rss=` also counts pages that libmalloc has freed and marked reusable but left resident, and its medium-zone region cache. That RSS figure never falls.
   - vmmap at step 860, after the fix: the malloc zones held 4.5 MB allocated and 9.7 MB of free space ("frag"). Physical footprint was 12.0 MB; ps RSS was 29.7 MB.
   - Diagnostic: with `MallocMediumZone=0`, through a wrapper script around the fixed body, the ps-RSS slope falls from about 12 to 4.0–4.4 KiB/step, and the second half to −0.6–1.0 KiB/step.
3. **The scenario's baseline is set during context fill.** At step 100 the context holds about 55k of the 114.7k-token `T_trigger` (every epoch logs `tokens_before` ≈ 114.7k). The first epoch is at step 155. Memory that scales with the context (window, byte cache, request body and its copy, the projection) grows legitimately from step 100 until the context first reaches `T_trigger`. So "max ≤ 1.25 × RSS at step 100" compares a half-full context with a full one. WP16 noted this too.

## Done
- **Bounded view** (`project.rs`):
  - `push_loaded` takes the event's token estimate first, then calls `slim_unread`. This drops the unread metadata listed above, and moves each `mask` event's entries into `View::masked` instead of cloning them.
  - `build_view` ends with `slim_masked` over the window, using the final mask set:
    - a masked output becomes empty;
    - a masked `write`/`edit` input becomes its masked form, which is what the projection replays.
  - Every mask epoch rebuilds the view, so this is the one place where masked parts are dropped. Projection, estimates and rendered bytes are unchanged; the new tests check this.
- **No doubled peaks** (`engine.rs`):
  - `rebuild_view` frees the old view and the byte cache before it builds the new view. A failed rebuild is a fatal log error (exit 1), as before.
  - `compaction_epoch` borrows the summarized span instead of cloning it.
- **Regression tests** (`context/tests.rs`):
  - `bounded_view_slims_without_changing_the_request`: on `mask_session`, which covers every mask kind (output, input, image, spill, errors), it checks after the epoch that:
    - masked outputs are empty, a masked write input is in its masked form, and the metadata and mask entries are gone;
    - the projection equals the projection of the full events reloaded from disk, under both thinking profiles;
    - the estimate is equal, `render_uncached` is byte-identical to the request that was sent, and a restarted `Context` renders the same bytes.
  - `bounded_view_memory_does_not_grow_with_masked_steps`: 150 steps of 8 KB bash outputs on a 64k window. The window must keep ≥ 140 results and pass through ≥ 5 mask epochs, while its output bytes stay ≤ `4 × T_trigger + 8,000` at every step.
  - Both tests **fail on the old code** (checked by restoring the HEAD versions of `project.rs` and `engine.rs`) and pass with the fix.

## Tests
- `cd seed && CARGO_INCREMENTAL=0 cargo test --offline`: **pass**. Unit 271 passed, 5 ignored; `tests/boot.rs` 16; `tests/self_md.rs` 8.
- `cargo clippy --offline -p init-body --all-targets -- -D warnings`: clean. `cargo fmt --check` is clean for my files; other seed files, such as `boot/*`, `model.rs` and `tests/boot.rs`, already had fmt diffs, which I left alone.
- **M-5, with WP16's tool, unchanged rule (ps RSS sampled every 1 s, skip 100):**

| Run | RSS at 0 / 100 / 500 / 1000 (KiB) | Slope (KiB/step, ≤ 2) | Max/base (≤ 1.25) | 2nd-half slope |
|---|---|---|---|---|
| WP16 (m5.json, m5-100ms.json) | 9872 / 18496 / 30128 / 32944 | 15.2 / 14.9 | 1.78 / 1.91 | 6.4 / 8.4 |
| before (`measurements/m5-fix8-before.json`) | 9696 / 18176 / 29664 / 33744 | 14.18 | 1.857 | 9.42 |
| after (`measurements/m5-fix8-after.json`) | 10128 / 18304 / 26624 / 27904 | **8.11** | **1.528** | 3.22 |
| 3 more alternating trials | before: 15.1–18.6 slope, 1.75–1.97 ratio | after: **11.6–12.3** slope, **1.52–1.65** ratio | | |

- **The same 3 trials, sampling physical footprint (`footprint -p <pid> -f bytes`, every 0.5 s):**
  - Skip 100:
    - before: slope 1.4–3.4, ratio 1.50–1.57, peak 15.8–17.7 MiB;
    - after: slope −2.2–2.2, ratio 1.32–1.33, peak 13.6–14.7 MiB.
  - Baseline at the first epoch (step 155):
    - before: slope 0.7–3.0, ratio 1.23–1.45, so 2 of 3 runs fail;
    - after: slope −3.0–1.98, ratio 1.02–1.13, so **all 3 runs pass**.
- **Live heap peak** (`heap`): 6.9 → **5.0 MiB**. Troughs: 1.5–2.5 → 1.4–1.6 MiB.
- Every run: 1,001 turn requests, 2 compactions, 8 masks, 0 errors, 0 protocol errors, body exit 0.

## Deviations (decide)
1. **decide: the M-5 metric on macOS** (testing.md §3.11 M-5 step 3). Recommended: sample `phys_footprint` instead of `ps -o rss=` and keep ps RSS as a reported diagnostic.
   - `phys_footprint` is `footprint -p <pid> -f bytes`, about 12 ms per sample, or `task_info(TASK_VM_INFO).phys_footprint`. Activity Monitor and jetsam use the same number.
   - On macOS, ps RSS mostly measures libmalloc's retention: see vmmap and the `MallocMediumZone=0` diagnostic above. On Linux, RSS can stay.
   - This needs a `--metric footprint` option in `init-measure m5` (WP16's file, not mine).
2. **decide: the M-5 baseline** (M-5 "Pass", loop.md item 21). Recommended: replace "excluding the first 100 steps / RSS at step 100" with "from the first epoch event in the session log". That is the first `mask` or `compaction`, meaning the context has reached `T_trigger`; here it is step 155. The thresholds stay at 2 KiB/step and 1.25×.
   - Alternative: keep step 100 and make the context fill first, for example `--output-bytes 8192`. That reaches `T_trigger` at about step 55, but it changes the epoch cadence.
   - The two changes are needed together. Footprint with the step-100 baseline still fails the ratio (1.32), and ps RSS with the step-155 baseline still fails the slope (about 11).
   - With both, the fixed body passes 3 of 3 runs and the old body fails 2 of 3, so the rule still discriminates.
3. **decide: log-context.md §3.7 wording.** "Everything except the index must be flat" does not hold even with the fix. Masking-first epochs keep each masked step's event structs in the window until the next compaction. That is about 3.5 KB per step: `Loaded` is 568 B inline × 4 events, plus the tool-input map.
   - Steps between compactions: about 430 here; the §3.12 thresholds allow up to about 1,000.
   - The bound is really "≤ `T_trigger` tokens of content + about 3.5 KB × steps since the last compaction + 25 B/event of index". It stays bounded and falls back at every compaction.
   - Optional follow-up, which would cut it by about 40% and add margin to the footprint slope (the after trials reached 1.98):
     - box `MessageEv` in `EventBody`, which shrinks `Loaded` from 568 B to about 256 B;
     - keep `tool_call` markers out of the window.

## Needs from others
- **testing.md owner:** decisions 1 and 2 above, then the M-5 text.
- **WP16 (`init-measure`):** a footprint sampler and the new baseline rule in `m5::analyze`, once decided. Re-run with `init-measure m5 --body seed/target/debug/init-body --mock kernel/target/k1/debug/init-mockllm`.
- **log-context.md owner:** the wording in decision 3.

## Left
- The M-5 full-stack variant and the T1-reload variant (WP16 "Left").
- The optional per-event shrink in decision 3.
- Disk: 6.4 GiB free. I removed my frozen binaries under `target/test-homes/fix8/` and the M-5 homes by exact path. Every process I signalled was my own child.
