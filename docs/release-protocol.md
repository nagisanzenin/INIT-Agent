# Release protocol

The order and the bar for every INIT release. [docs/release.md](release.md) holds the mechanics, meaning how each
script builds, checks and publishes. This file says which gates a release must pass, in what order, and what "ready
for a user" means.

**Every gate here traces to a real failure that the gates before it could not see.** The table at the end names each
one. Two sources shaped this file:
- Engram's `RELEASE_PROTOCOL.md` contributed the bug classes, the mutation test, the adversarial and post-release
  reviews, and the agent-run user session.
- temm1e's `tem-release-protocol` contributed the version sweep, distribution parity, and "tag only after green".

The INIT-specific gates came out of REL-050 and from what the owner found on the day v0.5.0 shipped. These are the
install matrix, the paste-the-whole-block rule, and the real-machine check.

A release ships when two things hold: every gate is green on the tagged commit, and the user session's verdict is
**ship**. A gate is never skipped silently. A gate that cannot run (for example a live provider without the owner's
go) is listed under "Not verified" in the release notes, with the reason.

**Version numbers (pre-1.0, `-preview`):**
- A user-visible feature is a **minor** release.
- A fix, doc or polish change is a **patch** release.
- A change to the body protocol, the manifest format, a WIT major, the install layout or the charter dialect is a
  minor release, and it needs an upgrade note in the release notes.
- A change to this file alone ships no version.

---

## The failures INIT cannot ship

Read this first. Every gate below exists to catch one of these. The list is ordered by harm.

| # | Class | Why it is fatal here | The one that taught us |
|---|---|---|---|
| 1 | **A path that weakens the Root** | INIT's whole claim is that the agent cannot touch its keys, its record, its undo or its stop. A hole here makes every other promise false. | REL-050: the R-SAFE test caught a new signal caller (`upgrade::Daemon::drop`). It was reviewed and added to the list, not waved through. |
| 2 | **Lost or silently changed owner state** | The evolved body, the lineage and the keys are the only things that cannot be rebuilt. | Before v0.5.0, the only route to a new release was uninstalling, which moved all three to the Trash. The upgrade path, and its rollback test, exist because of this. |
| 3 | **A stranded user** | A command we print that fails on a real user's machine is worse than a missing feature, because it is the first thing they see. | 2026-10-28, found by the owner, not by any gate. On a 0.3.1 install, get.sh told them to run `init-agent upgrade`, which 0.3.1 does not have. The README block chained the same command, so pasting it gave "command not found" twice. |
| 4 | **A number that flatters** | The `$` meter, the gate times and the site's numbers are what an owner believes. An under-count gets trusted. | Anthropic and OpenAI cache writes were billed at the input price, about 20 % under the provider's bill, until v0.5.0. The site also said "4.3 s · v0.3.2" while the receipt it linked already held v0.5.0's 4.1 s. |
| 5 | **A secret or a private thing published** | A public commit cannot be taken back. | REL-050: the export scan stopped on a key-shaped literal in a test and on the private repository's name. 2026-10-28: a site commit was pushed with the owner's personal e-mail and had to be re-authored. |
| 6 | **A flaky gate** | G0 runs the body's own tests on the user's machine. A race in a test refuses a good self-change there, so for INIT a flake is a product bug. | REL-050: races in t20, t22 and t26 refused G0 on the x86_64 runner. |
| 7 | **A stale label** | The number is right, but the text next to it is from an older release. | After v0.5.0 shipped: the CHANGELOG still said "candidate … not released"; docs/release.md §5b still described get.sh as macOS-only with a placeholder URL; the site said v0.3.3 two releases later. |

> **Keep this sentence:** every command, number and label a user sees must be true in the state that user is
> actually in, not only in a fresh test home.

---

## 0 · Open the release

- Open `build/status/REL-<version>.md`. It is the release record: every gate's verdict goes there, with the commit it
  ran on.
- Decide which targets ship: macOS arm64, Linux x86_64, Linux aarch64. A target ships only if its gates in §5 pass.
  The release notes, README and site name exactly the targets that shipped.
- Decide which providers the release claims as "tested live" (§5.6). Every other provider is "experimental".

## 1 · Land the work

Every behaviour change gets a test, and every new test passes the mutation test in §4.1. Docs and the guide change
in the same commit as the behaviour they describe.

## 2 · The version, everywhere it is written

There is no single version constant. Sweep all of these:

| Where | What |
|---|---|
| `kernel/`, `root/`, `tui/` `Cargo.toml` (`[workspace.package]`), `seed/Cargo.toml`, the lock files | the version; `release.sh` refuses a mismatch |
| `seed/self.json` | regenerated **on the packaged seed tree** (docs/release.md §1), never in-repo |
| `tui/` golden `brand_identity` | `UPDATE_GOLDEN=1 cargo test --offline --bin init-tui brand_identity_line` |
| `CHANGELOG.md` | `## v<version>-preview (<date>)`, with no "candidate" left after the release |
| `docs/release-notes/v<version>-preview.md` | header date, targets, the "Verified" and "Not verified" sections |
| `README.md` | platform line, install block, upgrade line, "New in …", the autonomy ladder's version marker |
| `docs/guide.md`, `docs/release.md`, `docs/platforms.md` | every place that names a version, a target or a command |
| INIT Labs site (`~/Documents/INIT Labs/site`) | version fallbacks, the body map (`scripts/body-map.py` from the release's `seed/self.json`), every measured number and its receipt, platform and models lines, the autonomy-cap text |

Sweep for stale versions and dead commands, then read every hit:

```sh
P=0.4.1   # the previous release
git grep -nE "v?${P//./\\.}" -- README.md docs/*.md | grep -v release-notes   # each hit must be historical, on purpose
grep -n "^## v" CHANGELOG.md | head -3                                         # dated, no "candidate"
grep -rnE "0\.[0-9]+\.[0-9]+" ~/Documents/"INIT Labs"/site/*.html ~/Documents/"INIT Labs"/site/*.js | grep -v deck
```

Then find every command a user is told to type in README, the guide, the release notes, the site and the messages
of `get.sh`, `install.sh`, `uninstall.sh` and `init-agent`. Each one must exist in the state where it is printed;
§5.5 runs them.

## 3 · CHANGELOG and release notes

Write them for a reader, not as a git log. Include the embarrassing parts: the race that refused a good change, and
the bug the owner found before the gates did.

The release notes must have:
- **Verified for this release:** what ran, on which target, on the mock or live, with links to the verdicts.
- **Not verified:** what did not run, and why.
- **Upgrading:** one line for each prior release a user may have, all three branches: 0.5.0 and later, 0.4.1, and
  before 0.4.1.
- **Limits, plainly.**

---

# The gates

## 4 · CI

- `ci/local.sh` once, with `clippy -D warnings` clean in all four workspaces. Then the body's test suite (`cargo test`
  in `seed/`, the suite G0 runs on the user's machine) five more times while the machine is loaded, for example
  alongside the e2e chain. A flake in the body suite refuses a good self-change on a user's machine (class 6). A flake in the
  Root, kernel or TUI suites costs only a developer a rerun. So the repetition goes where the harm is. (v1.1: this
  replaced "three full runs in a row", which took about 3 hours and stressed every suite equally.)
- The GitHub `fast-linux` job.
- The x86_64 workflow run on the private repository's CI branch, which has a $0 Actions budget with stop-usage.
- Record the test count. README and the site use that exact count, or none at all.

A red result stops the release. A flake is a red result, and so is a test that passes only when run alone (class 6).
Fix the test, or record it as a residual risk with a reproduction, in the release notes and REL-<version>.md. Never
just rerun it until it is green.

## 4.1 · Mutation-test every new check

A check that still passes when you revert its fix proves nothing. For each new test:
1. Revert the fix the test guards.
2. Run the test, and confirm that **this** test fails.
3. Restore the fix.

For a race fix, revert the wait and loop the test under load (`ci/slot.sh`), or record why the race cannot be
reproduced locally.

When you fix a bug class, grep for its siblings. The get.sh refusal had a twin in `install.sh`'s own "already
installed" message, so check both.

## 4.2 · Root safety

- `root/` R-SAFE tests are green. Any new caller of signal, kill, exec or a file write outside the zone is reviewed
  and added to the reviewed list, never allowed by widening a pattern.
- The sandbox tests are green in both full and workspace mode.
- The upgrade's "the zone cannot run or write the upgrade" checks (`tests/upgrade.rs`) are green.
- No change gives the agent a path to remove or weaken the Root, its keys, its record or its stop. This is a design
  rule, not a test, so the reviewer in §4.3 is told to look for it.

## 4.3 · Adversarial review

Run `/code-review high`, or independent reviewer agents, on the release diff.
- Give reviewers an **extracted tree**: `git archive <tag-candidate> | tar -x -C /tmp/rev/new`, with the previous
  release in `old/` and the diff beside them. Never point them at the live checkout, where a reviewer's `git checkout`
  can switch the files under the others.
- Give them the contract as well as the diff: the charter template, the guide, the messages users see, and
  root/root.md. A field that a file outside the diff never passes is invisible in the diff.
- Name the risk areas:
  - the Root boundary (class 1);
  - state kept across upgrade, uninstall and rollback (class 2);
  - every printed command and every number (classes 3 and 4);
  - concurrency in tests and in the zone (class 6).
- **Count the reports you received against the reviewers you launched.** A reviewer that errored or never reported is
  an unrun gate: run it yourself instead of inheriting its silence.
- Every confirmed finding gets a fix and a test that fails without the fix (§4.1).

## 4.4 · The numbers audit

List every number a user sees from this release, and answer four questions for each in REL-<version>.md:
1. **Which way does it fail?** Spend must over-count, never under-count: a model without write prices is billed at
   twice the input price.
2. **Does it agree with every other surface?** Compare the TUI `$`, `/usage`, `init-root egress stats`, a goal's
   `usd=` meter and the ledger on the same session.
3. **What is its receipt?** For README, site and release-notes numbers, check three things:
   - the linked file holds **this** release's data;
   - the label names the release, the model (live or mock) and the machine;
   - the receipt is still true at the URL. A link to `main` changes under it, so re-label the number or relink it on
     every release.
4. **Does the label survive a reader?** "Tested live" requires a live run (§5.6). "Mock model" or "simulated model"
   is said wherever it applies.

The Root's size is one of these numbers. Run root/root.md §4.8's command (`rootloc.py`) at the tagged commit, for the
whole Root and for the TCB split that docs/foundations.md §10 defines. README, foundations.md §10 and REL-<version>.md
give both totals with that commit.

## 4.5 · Read paths degrade, never brick

When a release changes a state format (charter, `install.json`, `status.json`, ledger, lineage, sessions), feed
hand-edited and truncated copies to:
- `init-root status`;
- `init-root audit verify`;
- the TUI's startup;
- `init-agent upgrade`'s preflight.

Each must print what is wrong and exit, never panic or hang.

---

## 5 · Every target, the owner's path

The mechanics are in docs/release.md §2b and §3. Every command here is $0, on the mock model.

| Check | Command | What it proves |
|---|---|---|
| fresh install | `scripts/bundle-e2e.sh <tarball>` | get.sh with Rust in a test home, one turn, owner state unchanged |
| a self-change | `scripts/bundle-e2e.sh --t2 <tarball>` | the shipped gate G0–G3 and the hand-off; 14–32 min measured (docs/release.md §2b) |
| pause | `scripts/bundle-e2e.sh --pause <tarball>` | Esc/Ctrl-C before the first token pauses, and the resume is one request |
| source install | `scripts/release-e2e.sh` | install.sh, a tool turn, `audit verify`, the uninstaller's refusals and the Trash |
| upgrade | `scripts/upgrade-e2e.sh --bundle <tarball>` | an evolved previous release keeps its state; a broken bundle rolls back; an incompatible one is refused |

- **macOS arm64:** all five checks.
- **Linux x86_64:** bundle, hello, pause and T2 on the CI runner, including INIT's AppArmor step on Ubuntu 24.04.
- **Linux aarch64:** the same checks in Docker.

A target that fails is not shipped. The release notes say so.

## 5.5 · The install matrix: every state a real user is in

```sh
scripts/install-matrix.sh --bundle <tarball>              # before the tag
scripts/install-matrix.sh --published --bundle <tarball>  # after the publish (§7)
```

The script runs these rows in test homes, on the mock, while the owner's install stays untouched:
- **fresh:** a clean home.
- **one row per public release since 0.3.1:** that release installed with its own get.sh, with its Root running.

For each row:
1. Run the new get.sh with no flags, as a user pastes it. Its message must name the installed release and the right
   way forward, and every command it prints must exist in that state.
2. Then do what the message says:
   - where the upgrade is supported, `get.sh --upgrade`, and the upgraded install answers;
   - otherwise, the printed uninstall command, which refuses while the Root runs and then moves everything to the
     Trash, followed by a fresh install that answers.

`--published` also checks that the public `get.sh` and the uninstaller at the URL the message prints are the bytes
that were tested.

**Paste the whole block.** Paste every multi-line command block in README, the site and the release notes as one
paste, in each state it addresses (fresh, and an existing install). A block that fails halfway must fail with a
message that says what to do next. A command that fails with "command not found" is the failure in class 3.

## 5.6 · Live (owner's go and a $ cap)

- `scripts/qualify-provider.sh --cap-usd <=10` for each provider the release calls tested.
- `UBE_LIVE_KEYFILE=… scripts/bundle-e2e.sh --live` for DeepSeek.

A provider is "tested live" only with a verdict from this release, or from a named earlier release when the
provider code did not change since then. Without the owner's go, these runs are listed under "Not verified", and the
labels say "experimental".

## 5.7 · The user session: the "ready for a user" gate

Everything above proves INIT is correct. Only this gate asks whether a stranger would get through it.

**Separation.** The release agent runs the harness. A **persona agent in its own fresh context** plays the user. It is
given only:
- the public README and the site's text;
- what its terminal shows: a real `init-agent` TUI in a test home, driven through `tmux send-keys` and
  `capture-pane`, on the mock upstream (`install.sh --mock-upstream`), or on a capped live key with the owner's go.

The agent that wrote the release must not write the persona's lines.

**At least two personas per release:**
1. **New:** a developer who has never used INIT, on a clean home, who pastes the README block.
2. **Returning:** an early adopter with an older release installed (from §5.5's oldest row and from the previous
   release), who follows whatever the screen says.

**The tasks:**
1. install;
2. the first-run setup (provider, key and model pickers);
3. one small coding task;
4. watch one self-change go through the gate track;
5. `/undo` or a rollback;
6. `init-agent upgrade` where it applies;
7. uninstall.

**The rules:**
- The persona answers terse and honest, never gets a hint, and never performs competence it does not have.
- Nothing is fixed during the session. Write it down and keep going.
- Afterwards, check every number the persona was shown (`$`, tokens, gate times, levels) against the Root's records,
  on the same state.

**The report** goes in `docs/user-sessions/v<version>-<persona>.md`:

```
## User session — v<version> — <persona>
PROVENANCE: <who played the user; what a simulation certifies (flow, text, numbers, the gate) and what it cannot
            (a real person's patience, a real provider's latency and bill)>
target: <macOS arm64 | Linux x86_64 | …>   model: <mock | live, provider, cap>   real minutes: <n>

WHAT WORKED
WHAT CONFUSED ME
WHAT ANNOYED ME, and what I would have quit over
WHAT IT TOLD ME vs WHAT WAS TRUE     (every number shown; does any surface disagree; did any flatter?)
EVERY COMMAND I WAS TOLD TO TYPE      (did each one work, in my state?)
WOULD A STRANGER GET THROUGH THIS?   yes / no, and why
VERDICT: ship / do not ship
```

The verdict is binding. Real-world signal outranks a simulated verdict: an owner's report, a GitHub issue or an HN
comment. When the two disagree, the next release says so on the record.

**Harness rules.** Every one of these was learned in v0.5.0's sessions, where harness artifacts drove both verdicts
alongside real bugs:
- Run the persona's shells and the Root with the **passwd HOME**. Isolate with INIT_ROOT_HOME, INIT_HOME and
  INIT_BIN_DIR. The broker refuses a Root whose HOME is not the passwd home. A persona that restarts the Root from a
  test-HOME shell gets a body that never spawns, and blames the product.
- Put the persona's project folder **inside the passwd home**, for example `~/init-us-<n>/project`, removed afterwards
  by exact path. The Root refuses a workspace outside the home, so a project under /private/tmp silently becomes "no
  workspace".
- Keep the Root's tmux window alive after the Root exits (`remain-on-exit on`), so a Ctrl-C does not tear down the
  window chain.
- Diff the persona's own keystroke log against its report. Use the log where the two disagree.
- The persona returns its report as text, and the operator files it verbatim.

---

## 6 · Tag and publish

The mechanics are in docs/release.md §4 and §4b. The owner's go is needed to publish, because it is a public action.

- Tag only after every gate is green on the tagged commit.
- **Never move a published public tag, and never replace a published asset.** From v0.5.3 the public repository has
  immutable releases (docs/release-signing.md §7.1): once published, a release's assets and its tag cannot change. A
  fix after publishing is a patch release. (Until v0.5.2 a fix could ship as a replaced `get.sh` asset with a record.)
  A private tag moved before the publish is recorded in REL-<version>.md.
- **Sign the release** (docs/release-signing.md §9). Put every asset, exactly as it will be uploaded, in one
  directory: both bundles with their `.sha256` (the Linux one from the CI artifact, checked against its
  `MANIFEST.json`), the merged `latest.json`, and the tagged commit's `get.sh`. Then:
  - `scripts/sign-release.sh sums --version <V> DIR`;
  - the offline step, `scripts/sign-release.sh sign --key <the release key> DIR`, done by the owner;
  - `scripts/sign-release.sh check DIR`.
  A release whose get.sh pins no release key, or whose `SHA256SUMS.sig` does not verify under that key, is not
  published.
- `scripts/publish-public.sh`: every scan must report 0. Commit with the noreply identity in **every** repository that
  is shared or published, including the INIT Labs site.
- Publish the GitHub release as a draft first, with **every** asset, `SHA256SUMS` and `SHA256SUMS.sig` included:
  nothing can be added once it is published. Check its asset names and sizes against the local files, then publish it
  as latest.
- **The expiry calendar.** A release's `SHA256SUMS` is valid for 180 days from signing (its `# expires:` line). The
  next release, a patch if nothing else, ships before the newest one has 60 days left. Otherwise fresh installs refuse
  it from the day after it expires (docs/release-signing.md §5). Put the date in REL-<version>.md.

## 7 · Verify the release is real

- **Public download check:**
  - download every asset from `releases/latest/download/`, `SHA256SUMS` and `SHA256SUMS.sig` included, and confirm
    each is byte-identical to what was uploaded;
  - `scripts/sign-release.sh check <the download dir>`: the signature under the key the published get.sh pins, every
    asset's sha256, and the macOS bundle's own `init-root release verify`;
  - `gh release verify v<version>`, and `gh release verify-asset v<version> <file>` for each asset (immutable, per
    GitHub's attestation);
  - `bundle-e2e` on the downloaded tarball.
- `scripts/install-matrix.sh --published`.
- **The real machine:** with the owner's consent, run the published one-liner, or `init-agent upgrade`, on the
  owner's Mac. Test homes never touch the real LaunchAgent, systemd unit or login items, so only this run exercises
  `launchctl bootstrap` and `bootout`. On 2026-10-28 the uninstaller's `bootout` was verified for the first time this
  way.
- The site and README show the new release, and the site's receipts hold this release's data (§2, §4.4).

## 7.5 · Post-release review

The gates above are run by the people who wrote the release, so they confirm what those people already believe.
Within 24 hours, spawn an independent reviewer against the **shipped public tree**, not the diff. Give it the list of
what the pre-release review already found, and this standing instruction:

> Find a command we print that fails in some user's state, a number that flatters, and a path that weakens the Root.

If it finds something, fix it immediately and say so plainly in the CHANGELOG. A patch release an hour later is not a
failure (from v0.5.3 nothing published is replaced: immutable releases, §6). A stranded user left standing is.

## 8 · Tell users

- The release notes' "Upgrading" lines, the same day on README and the site.
- Close the issues this release fixes, with a real reply: what shipped, what the catch was, and how to get it.
- Announcements (Show HN, Reddit) are written by the owner, by hand. Agents supply fact tables only.

---

## One-glance checklist

- [ ] REL-<version>.md opened; targets and live-tested providers decided
- [ ] version swept everywhere (§2); stale-version and dead-command grep read hit by hit
- [ ] CHANGELOG dated (no "candidate"); release notes with Verified, Not verified and Upgrading for every prior release
- [ ] §4 `ci/local.sh` once, clippy clean, the body suite ×5 under load, fast-linux green, x86_64 run green; test count recorded
- [ ] §4.1 every new test mutation-tested; sibling bugs grepped
- [ ] §4.2 R-SAFE, sandbox and upgrade-boundary tests green; new signal/exec callers reviewed
- [ ] §4.3 adversarial review on an extracted tree; reports received = reviewers launched; findings fixed and tested
- [ ] §4.4 numbers audit written; receipts hold this release's data; spend errs toward over-counting
- [ ] §4.5 read paths fed broken state when a format changed
- [ ] §5 bundle-e2e (fresh, t2, pause), release-e2e, upgrade-e2e on macOS; x86_64 on CI; aarch64 in Docker, or not shipped
- [ ] §5.5 `install-matrix.sh` green for fresh and every public release since 0.3.1; every block pasted whole
- [ ] §5.6 live qualification with the owner's go and a cap, or listed as not verified and labelled experimental
- [ ] §5.7 two user sessions run by a separate persona agent; reports filed; verdict **ship**
- [ ] §6 tag on the green commit; signed (`sign-release.sh check` green); export scans 0; noreply identity; draft with
      every asset checked; published as latest; the expiry date and the next release's latest date in REL-<version>.md
- [ ] §7 public download byte-identical and `sign-release.sh check` green on it; `gh release verify`;
      `install-matrix.sh --published`; the real-Mac run with the owner's consent
- [ ] §7 site and README on the new release; receipts relabelled
- [ ] §7.5 post-release independent review within 24 h
- [ ] §8 upgrade lines published; fixed issues closed with a real reply

## Why each gate exists

| Gate | The failure it caught, or would have caught |
|---|---|
| §2 version sweep | the CHANGELOG said "not released" after the release; the site said v0.3.3 at v0.5.0; docs/release.md §5b described a get.sh from v0.2 |
| §4 CI once, the body suite ×5 under load, no flake | t20, t22 and t26 races refused G0 on the x86_64 runner (REL-050 runs 7 and 8) |
| §4.2 Root safety | the R-SAFE test caught the upgrade's new signal caller (REL-050) |
| §4.4 numbers audit | cache writes under-billed by about 20 %; the site's swap time labelled v0.3.2 while its receipt held v0.5.0 |
| §5 targets | the x86_64 disk-floor bug (E_DISK, runs 6–7) and the Ubuntu 24.04 AppArmor refusal (runs 3–4) appeared only on Linux |
| §5.5 install matrix | the owner's 0.3.1 Mac: get.sh printed `init-agent upgrade` to a user who had no `init-agent` (2026-10-28) |
| §5.5 paste the whole block | the README block ran `init-agent` twice after the install refused: "command not found" ×2 |
| §6 identity and scans | a key-shaped literal and the private repository's name stopped the export (REL-050); a site commit carried the owner's personal e-mail (2026-10-28) |
| §6 signing, immutable releases | until v0.5.2, whoever controlled the release page, or a server named by `INIT_DIST_URL`, chose both the bundle and its checksum; with full access the agent could plant that variable in the owner's shell (docs/release-signing.md A1, A3) |
| §7 real machine | the uninstaller's `launchctl bootout` had never run before the owner's Mac (2026-10-28) |
| §7.5 post-release review | the get.sh bug shipped behind every green gate; the owner found it within hours |

**The pattern:** the gates a release's authors write confirm what they already believe. The bugs that mattered were
found by things that do not share the authors' assumptions: a runner on another platform, a machine with an old
install, and the owner pasting the README. Budget for those every release. They are the measurements that are not
looking in a mirror.

## Version history of this protocol

- **v1.3 (2026-10-29, during v0.5.3, PLAN-052 L4):** §6: every release is signed (`scripts/sign-release.sh`), and
  with immutable releases no published asset is ever replaced; the expiry calendar. §7: the download check verifies the
  signature and GitHub's attestation. §7.5: a fix is a patch release.
- **v1.2 (2026-10-29, during v0.5.3):** §4.4: the Root's size and its TCB's, counted at the tag.
- **v1.1 (2026-10-28, during v0.5.1):**
  - §4: one full CI run plus five loaded runs of the body suite, instead of three full runs.
  - §5.7: the harness rules.
  - bundle.sh: `--bless`, and it refuses a dirty scripts/.
- **v1 (2026-10-28, after v0.5.0-preview):** first version. Combined Engram's RELEASE_PROTOCOL (bug classes,
  mutation test, adversarial and post-release reviews, the agent-run user session) and temm1e's tem-release-protocol
  (version sweep, distribution parity, tag only after green) with REL-050's lessons and the get.sh incident
  (§5.5, §7).
