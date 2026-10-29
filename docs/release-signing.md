# Signed releases
Status: v1 · 2026-10-29 · Area: release (PLAN-052 lane L4, v0.5.3)
Depends on: release-protocol.md, release.md, foundations.md (FG-1, FG-2, §10), adr/012 (full access)
Defines: the `SHA256SUMS` / `SHA256SUMS.sig` release assets, the release key and where it is pinned, `init-root release`,
get.sh's signature check and its `--allow-unsigned` / `--allow-expired`, the key ceremony.

## 1. Purpose, and what each claim's status means

Until v0.5.2 a release was checked by sha256 only, and the sha256 came from the same place as the bundle (latest.json,
or the `.sha256` beside it). That proves the download is intact. It does not prove the maintainer made it: whoever
controls the release page, or the path to it, chooses both the bundle and its checksum. From v0.5.3 every release
also ships a signed checksum file, and an upgrade installs a bundle only when the **installed** Root verifies that
signature against a key compiled into it.

The change is additive: two new release assets and one new Root command. Lineage record v1, `MANIFEST.json` and the
install layout do not change, so it ships as a patch (owner decision, 2026-10-29).

Every claim below carries one of these statuses (the Math First discipline):
- **proved**: follows from the structure of the code or a stated mathematical fact, with the argument given here;
- **measured**: shown by a named test or run (§11), and the test fails when its guard is reverted (§11 mutation runs);
- **assumed**: rests on something this release does not check (ed25519's unforgeability, the key's secrecy, the clock,
  GitHub's enforcement of its own settings);
- **not claimed**: outside what signing gives, said so that nobody reads it in.

## 2. Threat model

| # | Attacker | Without signing (≤ 0.5.2) | With signing (0.5.3) | Status |
|---|---|---|---|---|
| A1 | controls the release page: a stolen GitHub session or token, a malicious collaborator, a fault at GitHub; swaps or adds assets | installs and upgrades take whatever the page lists | **an upgrade from 0.5.3 on refuses it**: the installed Root checks the signature with its own key before get.sh unpacks or runs anything from the download. **A fresh install is not protected**: get.sh itself comes from that page, so its controller can serve a get.sh that pins another key or checks nothing (trust on first use). A user who checks the files against the key fingerprint from a second channel is protected (§4.4). Immutable releases (§7.1) stop changes to a release already published, so the attacker must publish a new release, which is public | refusal: measured; key secrecy and ed25519: assumed; fresh install: not claimed |
| A2 | sits on the network path (a proxy with its own root CA, a mis-issued certificate) or runs a mirror that `INIT_DIST_URL` names | as A1 | as A1: upgrades refuse; a fresh install is as safe as the TLS that fetched get.sh | measured / not claimed |
| A3 | **the agent**, through the owner's shell: in full access (ADR-012) it can write `~/.zshrc`, so the owner's next terminal can carry `INIT_DIST_URL`, a proxy or a CA file of its choosing, and `init-agent upgrade` passes the environment to get.sh | guide.md lists "the shell you type it in" as a limit. Concretely, as this lane found: get.sh read `INIT_DIST_URL` and took the sha256 from that server's latest.json, so the agent could choose the Root that the owner's next upgrade installs (class 1 in release-protocol.md). Measured: the 0.5.1 and 0.5.2 launchers pass a planted `INIT_DIST_URL` on to the kept get.sh | an upgrade from 0.5.3 on refuses anything the key did not sign, wherever it came from. The upgrade *into* 0.5.3 from 0.5.1 or 0.5.2 runs that install's own get.sh, so it is still exposed: §10 gives the command that avoids it | measured (from 0.5.3); 0.5.1/0.5.2 → 0.5.3: not claimed |
| A4 | offers an older, validly signed release as the newest, to bring back a known flaw (a downgrade) | refused by `init-root upgrade` (v0.5.0), but only after the old bundle's own install.sh started | refused by the installed Root before anything from the download runs (the signed version is older than install.json's); `init-root upgrade` refuses it again. No flag allows it: INIT never downgrades, and going back is an uninstall and a fresh install with `--version` | measured |
| A5 | offers the installed release, or an old one to a fresh install, as the newest for ever, so a fix never arrives (freeze, replay) | not detected | bounded by `expires` (§5): a newer-than-installed or fresh release past its date is refused unless `--allow-expired`; the installed release offered past its date gets a note. A replay inside the validity window is not detected | refusal: measured; inside the window: not claimed |
| A6 | steals the release key | — | the thief can sign releases every install that pins the key accepts, given A1 or A2 to deliver them. §8 recommends an offline key behind a passphrase; key 1 is a file on the maintainer's Mac without one (§8, "As done"), so anything that runs as the maintainer there can copy it, including a full-access INIT agent through the shell startup files (A3). The response is in §8.5 | not claimed |
| A7 | (no attacker) the key is lost | — | no release can be signed for installs that pin only that key; §8.5 | not claimed |
| A8 | compromises the build: the maintainer's machine, a dependency, the CI runner that builds the Linux bundle | — | signing proves who published the bytes, not that they are good or built from the tag. Reproducible builds are v0.6 | not claimed |
| A9 | passes off a signature the same key made for something else (a git commit, an SSH login) | — | the SSHSIG namespace `init-release` is part of the signed data, and both verifiers require it | proved (§4.2) and measured |
| A10 | mixes the page's files: latest.json of one release with SHA256SUMS of another, or a bundle whose `VERSION` is not the signed release | — | refused: latest.json's version, its sha256 and the bundle's `VERSION` must all be the signed ones | measured |
| A11 | **the agent** changes the downloaded files during an upgrade | the download, its check and the unpacked copy live in `ROOT_HOME/cache/upgrade/`, which the zone cannot write (v0.5.1) | unchanged: the signature files sit there too | measured (tests/install/test_upgrade_trust.py) |
| A12 | crafts a SHA256SUMS that two parsers read differently | — | the Root verifies the signature over the raw bytes before it parses them, then reads a strict grammar (§3); get.sh reads the same fixed lines | proved by construction; measured |

## 3. The signed object

Each release gets two assets beside the bundles:

```
# INIT release SHA256SUMS v1
# version: 0.5.3-preview
# expires: 2027-04-27
<sha256>  get.sh
<sha256>  init-v0.5.3-preview-linux-x86_64.tar.gz
<sha256>  init-v0.5.3-preview-linux-x86_64.tar.gz.sha256
<sha256>  init-v0.5.3-preview-macos-arm64.tar.gz
<sha256>  init-v0.5.3-preview-macos-arm64.tar.gz.sha256
<sha256>  latest.json
```

- `SHA256SUMS` lists every other asset of the release, as uploaded. The grammar is exact (release.rs `parse_sums`): the
  first line is that header; then `# version: V`, a plain version with a `MAJOR.MINOR.PATCH` core; then
  `# expires: YYYY-MM-DD`, the last valid day in UTC; then one `<64 lowercase hex>  <name>` line per asset, names
  sorted (C locale) and unique, from `[A-Za-z0-9._+-]`; LF line ends, a final LF, nothing else.
- It is not the `SHA256SUMS` inside a bundle, which lists the bundle's own files and stays as it was.
- `shasum -a 256 -c` (macOS, measured) and GNU `sha256sum -c`, also with `--strict` (coreutils 9.12, measured), skip the
  `#` lines, so a user can check the files with the usual tools.
- `SHA256SUMS.sig` is the signature (§4). With more than one key during a rotation it holds one block per key, the newest
  first (§6).

## 4. One key, two verifiers

### 4.1 The key
One ed25519 key, made with `ssh-keygen -t ed25519` (§8). Its public half is pinned as the OpenSSH line
`ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI… <comment>` in two places, the newest key first:
- `scripts/get.sh`, `RELEASE_KEYS='…'`;
- `root/crates/init-root/src/release.rs`, `RELEASE_KEYS: &[&str]`.

release.rs `the_root_and_get_sh_pin_the_same_keys` fails when the two differ (measured). Both are empty until the key
ceremony. A get.sh or a Root built that way verifies nothing and says so; `sign-release.sh` refuses to sign for such a
get.sh, so a release cannot ship without the key.

The same key in other forms: the base64 decodes to `string "ssh-ed25519"` followed by `string <32 bytes>` (SSH wire
strings: a big-endian u32 length, then the bytes), so the raw ed25519 public key is bytes 19 to 50 of the decoded blob.
Its fingerprint is `SHA256:` and the unpadded base64 of SHA-256 over the blob. `ssh-keygen -lf key.pub` and
`init-root release keys` print the same fingerprint (measured: tests/release_sig.rs `what_ssh_keygen_signs_verifies`).

### 4.2 The signature
`ssh-keygen -Y sign -n init-release -f KEY SHA256SUMS` writes `SHA256SUMS.sig` in OpenSSH's SSHSIG format
(PROTOCOL.sshsig): the armored blob `"SSHSIG"`, version 1, the signer's public key, the namespace, an empty reserved
field, the hash name `sha512`, and the signature. The signature is plain Ed25519 (RFC 8032) over
`"SSHSIG" ‖ string("init-release") ‖ string("") ‖ string("sha512") ‖ string(SHA-512(SHA256SUMS))`.
Because the namespace is inside the signed data, a signature this key made under any other namespace (`git` for a
commit, `file` by default) does not verify as a release signature (A9: proved by that construction, measured by
`a_signed_release_verifies_and_every_change_to_it_is_refused` and test_release_signature.py). One file serves both
verifiers.

### 4.3 The verifiers
| Path | Who verifies | Key | Code |
|---|---|---|---|
| fresh install, `curl … get.sh \| sh` | get.sh, with `ssh-keygen -Y verify -f <allowed_signers> -I init-release -n init-release -s SHA256SUMS.sig < SHA256SUMS` | get.sh's `RELEASE_KEYS` | get.sh `sig_by_ssh` |
| `init-agent upgrade` (0.5.3 and later) | the installed Root: `ROOT_HOME/bin/init-root release verify --sums --sig --file`, run by the kept `ROOT_HOME/cache/get.sh` | compiled into the installed Root | release.rs; get.sh `sig_by_root` |
| the pipe `curl … get.sh \| sh -s -- --upgrade` on an install before 0.5.3 (its Root has no `release` command) | get.sh, as a fresh install, plus its own downgrade check | that get.sh's `RELEASE_KEYS` (trust on first use, as a fresh install) | get.sh `sig_by_ssh` |
| `--from FILE`, a local bundle | nobody: its sha256 is checked, from `--sha256` (an upgrade) or the `.sha256` beside it (a fresh install), as before | — | unchanged |

**The upgrade's key never comes from the download (proved).** `release verify` takes no key argument. Its keys are
`RELEASE_KEYS`, a constant in the binary, except that a `test-fixtures` build, which no release is (root/root.md
§3.16), reads `INIT_TEST_RELEASE_KEYS` instead. The binary that runs is `ROOT_HOME/bin/init-root`, which the zone
cannot write in any access mode (ADR-012), and get.sh runs it before it unpacks the bundle. The new release's own
`init-root upgrade --release` runs later and cannot vouch for itself, so it checks nothing about the signature.

**The Root's side is small on purpose** (foundations.md §10: it decides which Root replaces this one). The SSH wire
format is parsed by hand in about 40 lines; Ed25519 is `ed25519-dalek` `verify_strict`, which the Root already links
for the lineage (INV-6); base64 is the `base64` crate that `ureq` already builds into the Root, with the same features
(`std`, no `simd-unsafe`), so no crate and no new code reaches the binary. It writes nothing and uses no network. Exit
0: verified; 5: only the expiry failed (the bundle is checked before the expiry, v0.5.3); 3: refused, with the reason;
1: a file could not be read; 64: usage.

### 4.4 Checking a fresh install by hand
The one-liner trusts the get.sh it downloads. A careful user can check the release first, with the key's fingerprint
taken from a second channel (the README of an earlier tag, the INIT Labs site, the release notes, the Show HN post):

```sh
d="$(mktemp -d)" && cd "$d"
for f in get.sh SHA256SUMS SHA256SUMS.sig; do curl -fsSLO "https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/$f"; done
sed -n "s/^RELEASE_KEYS='\(ssh-ed25519 [^ ']*\).*/\1/p" get.sh > release-key.pub
ssh-keygen -lf release-key.pub                    # this fingerprint must be the one published elsewhere
sed 's/^/init-release namespaces="init-release" /' release-key.pub > allowed_signers
ssh-keygen -Y verify -f allowed_signers -I init-release -n init-release -s SHA256SUMS.sig < SHA256SUMS
shasum -a 256 -c --ignore-missing SHA256SUMS      # Linux: sha256sum -c --ignore-missing SHA256SUMS
sh get.sh
```

Measured (2026-10-29, macOS 14.6, against a release made with a throwaway key): the lines after the download print
the fingerprint, `Good "init-release" signature …` and `get.sh: OK`.

## 5. Downgrade and expiry

- **Older than installed: refused, always** (measured). The installed Root compares the signed version with
  install.json `release.version` by SemVer precedence: the core, then the pre-release label (`0.5.3-alpha` is older
  than `0.5.3-preview`, and both are older than `0.5.3`), with build metadata ignored (v0.5.3, `upgrade::precedence`;
  get.sh's `ver_older` for an install whose Root cannot check). There is no flag: `init-root upgrade` refuses a
  downgrade too, by the same rule, so a flag would only move the refusal later. Going back to an older release is an
  uninstall and a fresh install with `--version`.
- **The installed release again** (`state: current`): only the same version string as install.json
  `release.version` (v0.5.3: until then any version of the same core counted, so a signed `0.5.3` or another label of
  it skipped the expiry, although `init-root upgrade` installs it). Verified as usual; the upgrade then finds nothing to
  do (exit 4). Its expiry is not enforced, since nothing new installs, but the Root prints a note that the server
  offers nothing newer (A5), and get.sh prints that note and no `--allow-expired` warning.
- **Expired**: any other release, newer than the installed one or on a fresh install, whose `expires` is before today
  (UTC) is refused: exit 5 from the Root, and get.sh's message names `--allow-expired`. The bundle is checked first, so
  an unlisted or wrong bundle is refused as such (exit 3) even when the list has also expired. The expiry date is valid
  through its whole day (measured).
- **Validity**: `sign-release.sh sums` writes 180 days by default and accepts 30 to 366. Immutable releases (§7.1)
  mean a published SHA256SUMS can never be re-signed, so the only way to renew is a new release. release-protocol.md
  §6 therefore asks for a release, a patch if nothing else, before the newest one has 60 days left. If that lapses,
  fresh installs refuse until the next release, with a message that says why and how to go on.
- **The clock** is assumed right to the day. A clock far ahead refuses valid releases (the message prints today's
  date); one far behind accepts an expired release.

## 6. Key rotation

Built in v0.5.3 (measured: `a_rotation_signs_with_both_keys_and_either_verifies`):
- `RELEASE_KEYS` is a list in both places, and `SHA256SUMS.sig` may hold several SSHSIG blocks;
- the Root accepts a release when any block verifies under any key it pins;
- `ssh-keygen -Y verify` reads the first block only (measured, OpenSSH 9.7p1, 2026-10-29), so the newest key's block
  comes first, and get.sh, always fetched fresh, pins the newest key;
- `sign-release.sh sign --key NEW --key OLD` writes the blocks in that order.

A planned rotation, for example to a hardware key: release R pins `[NEW, OLD]` in both places and is signed by both.
Every later release is signed by both while a supported install pins only OLD. Once the oldest supported release pins
NEW, OLD is dropped. OLD is never destroyed while it still signs.

Designed, not built: a **key-rotation statement**, an asset `KEY-ROTATION` (`# INIT release key rotation v1`,
`old: <line>`, `new: <line>`, `from: <version>`) signed once by OLD under the namespace `init-release-rotation`. A Root
that knows the format would then trust NEW for releases from `from` on, which lets OLD be destroyed after one signature.
Installs older than the first Root that reads statements would still need the dual-signed bridge, so v0.5.3 builds the
part that needs no format: dual signing.

A rotation cannot answer a leaked key, because the thief can sign rotations too (§8.5).

## 7. GitHub settings (the owner sets them; no agent changes a GitHub setting)

### 7.1 Immutable releases on nagisanzenin/INIT-Agent
On the public repository: **Settings → General → Releases → "Enable release immutability"** (GitHub's docs, checked
2026-10-29). GitHub's REST API also has, as third-party descriptions of it say (UNVERIFIED here):
`gh api -X PUT repos/nagisanzenin/INIT-Agent/immutable-releases` to enable it and
`gh api repos/nagisanzenin/INIT-Agent/immutable-releases` to read it.

It applies only to releases published after it is enabled, so enable it before v0.5.3 is published. Then:
- a published release's assets cannot be changed or deleted, and its tag cannot be moved or deleted (the tag name
  cannot be reused even after the release is deleted); title, notes and the latest/pre-release markers stay editable;
- GitHub makes a release attestation: `gh release verify v0.5.3-preview` checks that the release exists and is
  immutable, and `gh release verify-asset v0.5.3-preview <file>` that a local file is exactly the asset. It is
  GitHub's own statement, so it does not help against A1, but it proves no asset changed after publication;
- consequences for the protocol (release-protocol.md §6, §7.5): every asset, the Linux bundle from CI included, is
  uploaded to the draft before publishing; a fix after publishing is a new release, never a replaced asset.

### 7.2 A ruleset on the lineage mirror branch
The off-machine mirror (root/root.md §3.11) is `nagisanzenin/INIT-Lineage`, branch `lineage`, pushed only by the Root
with its deploy key, which never force-pushes. A ruleset makes that a property of the mirror too: nobody, the
account's owner included, can delete the branch or rewind it, so `audit verify --file` of a mirrored copy with
`--head` keeps its meaning.

```sh
gh api -X POST repos/nagisanzenin/INIT-Lineage/rulesets --input - <<'EOF'
{
  "name": "lineage is append-only",
  "target": "branch",
  "enforcement": "active",
  "conditions": {"ref_name": {"include": ["refs/heads/lineage"], "exclude": []}},
  "rules": [{"type": "deletion"}, {"type": "non_fast_forward"}],
  "bypass_actors": []
}
EOF
gh api repos/nagisanzenin/INIT-Lineage/rulesets            # check it is listed and active
```

Or in the UI: the repository's **Settings → Rules → Rulesets → New branch ruleset**, target `lineage`, enforcement
Active, no bypass list, "Restrict deletions" and "Block force pushes" on. Notes:
- Deploy keys cannot be added to a bypass list (GitHub's docs, checked 2026-10-29), so the Root's key is bound too.
- GitHub's pages name the Team and Enterprise plans for rulesets and leave private repositories on GitHub Free
  unclear. If the private INIT-Lineage cannot take a ruleset on the owner's plan, the choices are GitHub Pro or a
  public mirror (the records are the agent's history, so that is the owner's call), or to rely on the Root's own rule
  and on `--head` (the warning in §11).
- The repository is created at M0; until then the mirror is off and there is nothing to protect.

### 7.3 The account
A security key as the second factor on the GitHub account; no long-lived token with write access to INIT-Agent outside
the owner's Mac; the deploy keys reviewed. These make A1 harder; signing is what makes it not enough.

## 8. The key ceremony (the owner, about 1 hour, $0)

> **As done for key 1 (2026-10-29).** The owner chose the simplest custody instead of §8.1–8.2: an ed25519 file key
> **without a passphrase**, made on the Mac, at `~/.init-release/init-release-1` (folder 0700, key 0400), outside any
> git work tree and outside iCloud's Desktop & Documents. INIT's own agent cannot read that folder in either access
> mode (the Root's deny list, lane H), but in full access it can write the owner's shell startup files, which run as
> the owner and could copy the key (A3, A6): sign on a session where INIT runs in workspace mode or not at all. Fingerprint `SHA256:IS7wIWkppZkHSUwdnsJR+K4fBPkmPa/IJwfNi9sIoMs`.
> What this gives up, plainly: anything else that can read the owner's files (malware, another tool running as the
> owner, a backup) can sign a release. There is no second copy; if the file is lost, §8.5 applies (make key 2).
> The USB procedure below stays the recommended one for a later key.

### 8.1 What you need
The Mac; two USB sticks; the password manager; paper. The key is made and used only with the network off, and it lives
only on the two encrypted sticks.

### 8.2 Make the key
1. Turn Wi-Fi off and unplug any cable.
2. Format each stick in Disk Utility as **APFS (Encrypted)**, named `INITRK` and `INITRK2`, each with a passphrase
   (passphrase A). Save it in the password manager.
3. Make the key on the first stick, with its own passphrase (passphrase B: six or more random words; in the password
   manager, and on paper in a place apart from the sticks):
   ```sh
   ssh-keygen -t ed25519 -a 100 -C "INIT release key 1 (2026-11)" -f /Volumes/INITRK/init-release-1
   chmod 400 /Volumes/INITRK/init-release-1
   ssh-keygen -lf /Volumes/INITRK/init-release-1.pub      # the fingerprint: write it on the paper and in the manager
   ```
4. The backup: `cp /Volumes/INITRK/init-release-1 /Volumes/INITRK/init-release-1.pub /Volumes/INITRK2/`. Keep INITRK2 in
   another place (not with the Mac).
5. Only the public half leaves the sticks: `cp /Volumes/INITRK/init-release-1.pub ~/init-release-1.pub`.
6. `diskutil eject /Volumes/INITRK; diskutil eject /Volumes/INITRK2`, then the network back on.

### 8.3 Pin it
In the main checkout, as a normal commit:
1. `scripts/get.sh`: `RELEASE_KEYS='ssh-ed25519 AAAA… INIT release key 1 (2026-11)'` (the whole `.pub` line).
2. `root/crates/init-root/src/release.rs`: `pub const RELEASE_KEYS: &[&str] = &["ssh-ed25519 AAAA… INIT release key 1 (2026-11)"];`
3. Run `cargo test --offline -p init-root --bin init-root release::` (the two must agree) and
   `python3 -m unittest discover -s tests/install`.
4. Commit: "Release key 1: pin the release signing key (SHA256:…)".
5. Publish the fingerprint in a second channel: README (a "Verify a download" line), the INIT Labs site, the release
   notes, the Show HN post. That second channel is what gives the check in §4.4 its meaning.
6. Remove `~/init-release-1.pub` (it is public; this is only tidying).

### 8.4 Where the key never is
Never on the Mac's own disk, in `~/.ssh` or an ssh-agent, in a git work tree, in iCloud Drive (`~/Documents` and
`~/Desktop` are synced when "Desktop & Documents" is on), Dropbox, an unencrypted Time Machine backup, a CI secret, or
a chat. **Never mounted while INIT runs in full access**: its agent can read anything the owner can (ADR-012), so the
sticks are mounted for the signing step only, with the network off. `sign-release.sh` refuses a key file others can
read, a link, a non-ed25519 key and one inside a git work tree, and warns about cloud-synced folders.

### 8.5 If the key is lost, or leaked
- **Lost** (both sticks, or passphrase B): make key 2 (§8.2) and pin it. The next release can carry only key 2's
  signature, which installs that pin key 1 refuse. Its release notes tell owners to check it by hand (§4.4, with key 2's
  fingerprint from the second channel, and `gh release verify`) and then to run `init-agent upgrade --allow-unsigned`
  once. Fresh installs are unaffected.
- **Leaked**, or a stick lost with a weak passphrase: every install that pins key 1 is at risk until it pins another
  key. Make key 2 at once. Release R pins key 2 only and is signed by key 2 and then key 1 (so installs can reach it).
  Publish a GitHub security advisory, a README and site notice and an issue: upgrade now, then check that
  `~/.init-root/bin/init-root release keys` shows only key 2's fingerprint. Rotate the GitHub account's credentials,
  since a thief needs A1 or A2 to deliver a release. Every later release is signed by key 2 only. Say plainly that an
  install which never upgrades stays exposed.

### 8.6 A hardware key later
OpenSSH's FIDO keys (`ed25519-sk`) sign in another format (`sk-ssh-ed25519@openssh.com`, with flags and a counter),
which release.rs does not accept. A hardware key therefore needs either a token that holds a plain ed25519 key and
signs SSHSIG with it, or `sk` support in the verifier (a small, reviewed addition to release.rs) shipped one release
before the rotation of §6.

## 9. Signing a release

Each release, in the order of release-protocol.md §6:
1. Put the assets, exactly as they will be uploaded, in one directory, for example `target/dist/upload-v0.5.3/`:
   get.sh (the tagged commit's), each bundle with its `.sha256` (the Linux one from the CI artifact, checked against
   its `MANIFEST.json`), and the merged latest.json.
2. `scripts/sign-release.sh sums --version 0.5.3-preview target/dist/upload-v0.5.3` (online; no key).
3. The offline step: network off, mount INITRK,
   `scripts/sign-release.sh sign --key /Volumes/INITRK/init-release-1 target/dist/upload-v0.5.3`. It shows the file and
   the key's fingerprint and asks; ssh-keygen asks for passphrase B. Then eject, network on.
4. `sign` runs `check` itself: the signature under the key the release's get.sh pins, every asset's sha256, every file
   listed, the version against latest.json, the days left, and, on the Mac, the macOS bundle's own init-root verifying
   it with its compiled-in key. Run `scripts/sign-release.sh check target/dist/upload-v0.5.3` again after any copy.
5. Upload all of them, `SHA256SUMS` and `SHA256SUMS.sig` included, to the draft; check names and sizes; publish.

## 10. Who is protected, from when

| Installed | How it reaches 0.5.3 | Is that upgrade's download signature-checked? | Later upgrades |
|---|---|---|---|
| 0.5.3 and later | `init-agent upgrade` | yes, by the installed Root's key | yes |
| 0.5.1, 0.5.2 | `init-agent upgrade`, which runs that install's own get.sh | **no**: sha256 only, from the server that get.sh's environment names (A3) | yes |
| 0.4.1, 0.5.0 | the pipe, which runs 0.5.3's get.sh | yes, by ssh-keygen with that get.sh's key (trust on first use) | yes |
| none | the one-liner | yes, by ssh-keygen (trust on first use) | yes |

For 0.5.1 and 0.5.2 the upgrade into 0.5.3 keeps v0.5.2's trust: TLS to GitHub plus the sha256 on the page. A3 makes
it weaker when the agent has planted variables in the owner's shell. The release notes should therefore give those
owners a clean environment for that one upgrade:

```sh
env -i HOME="$HOME" ~/.local/bin/init-agent upgrade
```

The launcher finds its ROOT_HOME from the value install.sh baked into it, so a custom home needs nothing more.
Measured for the launcher (the 0.5.1, 0.5.2 and 0.5.3 launchers hand `--upgrade` to the kept get.sh with
`INIT_DIST_URL` unset and the baked ROOT_HOME). The whole upgrade under `env -i` is to be measured at release, as an
install-matrix row from 0.5.2.

## 11. Tests and mutation runs

- Root unit tests (src/release.rs): a signed release verifies and every change to it is refused (one byte of
  SHA256SUMS, the bundle's sha256, an unlisted file, another key, a trusted-but-unpinned key, another namespace, no key
  pinned); a tampered or missing signature; downgrade, the installed release, expiry and its last day; the strict
  grammar; dual signing; the agreement of the two pinned lists; and an `ssh-keygen -Y sign` fixture
  (tests/fixtures/release/: a TEST public key whose private half was deleted, a SHA256SUMS and its signature).
- Root integration tests (tests/release_sig.rs, `test-fixtures`): the CLI's exit codes and JSON as get.sh reads them,
  the install.json downgrade check, `release keys`, and a signature made at run time by OpenSSH's `ssh-keygen`.
  tests/wp23.rs `offline_verify_without_head_warns_that_a_cut_copy_verifies_too`: `audit verify --file` without
  `--head` warns (PLAN-052 L4).
- Install tests (tests/install/test_release_signature.py, test_sign_release.py): get.sh's fresh path with ssh-keygen and
  every refusal of §2, the upgrade path's handling of each Root verdict (a stand-in Root, and the real `test-fixtures`
  Root when `INIT_ROOT_FX_BIN` names one), an install before 0.5.3, `init-agent upgrade`'s flags, and sign-release.sh.
- Results on branch v053-s (2026-10-29, macOS 14.6 arm64): Root unit tests 277 passed (default features) and 300
  (`test-fixtures`); integration release_sig 3, wp23 5, upgrade 15; install tests 105 (80 before this lane); clippy
  `-D warnings` clean with and without `test-fixtures`, all targets.
- Mutation runs (release-protocol.md §4.1): each guard reverted, its test run, the guard restored. All caught.

| Guard reverted | Caught by |
|---|---|
| release.rs: the signature's result discarded | `a_signed_release_verifies…`, `a_tampered_or_missing_signature…`, `a_signature_made_by_ssh_keygen…` |
| any well-formed key trusted; the namespace not checked; the bundle's sha256 not compared; no key pinned not refused | `a_signed_release_verifies_and_every_change_to_it_is_refused` |
| an older release read as current; expiry ignored | `older_releases_are_refused_and_expiry_counts…` |
| `INIT_TEST_RELEASE_KEYS` read in every build | `only_a_test_build_reads_keys_from_the_environment` |
| names neither sorted nor unique; trailing bytes after the signature | `sha256sums_is_read_strictly`; `a_tampered_or_missing_signature_is_refused` |
| get.sh pins another key than release.rs | `the_root_and_get_sh_pin_the_same_keys` |
| the CLI ignores install.json (no downgrade check) | tests/release_sig.rs `verify_answers_get_sh_with_its_exit_codes_and_json` |
| `audit verify --file` without `--head` does not warn | tests/wp23.rs `offline_verify_without_head_warns…` |
| get.sh: no signature step; ssh-keygen's verdict ignored; a missing signature not refused; the latest.json version, the bundle's VERSION or the given sha256 not compared; expiry not checked | the matching `FreshInstall` case of test_release_signature.py |
| get.sh: no downgrade check on an install before 0.5.3; a Root refusal ignored; a Root that pins no key not refused | the matching `Upgrade` case |
| get.sh: the installed Root not asked on an upgrade | `test_an_upgrade_asks_the_installed_root_and_a_fresh_install_ssh_keygen` (added after this mutation first survived) |
| init-agent: the opt-outs not passed on | `LauncherFlags` |
| sign-release.sh: a key others can read; a key in a git work tree; `check` ignores ssh-keygen; a stale `.sha256` | the matching case of test_sign_release.py |

## 12. Open questions, and what is not built

- The key ceremony (§8) and the pin (§8.3) are the owner's. Until then both lists are empty, sign-release.sh refuses,
  and v0.5.3 cannot be published as a signed release.
- The rotation statement (§6) is designed, not built. FIDO keys are not accepted (§8.6).
- A local bundle is checked by sha256 only, even with signed files beside it.
- A fresh install trusts the get.sh it downloads (A1, A2); §4.4 is the way around it.
- The upgrade into 0.5.3 from 0.5.1 or 0.5.2 is not signature-checked (§10).
- Signing says nothing about how the bytes were built (A8; reproducible builds are v0.6).

## 13. References (checked 2026-10-29)

- OpenSSH, PROTOCOL.sshsig: https://github.com/openssh/openssh-portable/blob/master/PROTOCOL.sshsig
- OpenSSH 8.1 release notes (ssh-keygen -Y sign/verify), 2019-10-09: https://www.openssh.com/txt/release-8.1
- RFC 8032, Edwards-Curve Digital Signature Algorithm (EdDSA): https://www.rfc-editor.org/rfc/rfc8032
- GitHub, Immutable releases: https://docs.github.com/en/code-security/supply-chain-security/understanding-your-software-supply-chain/immutable-releases
- GitHub, Preventing changes to your releases: https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/establish-provenance-and-integrity/prevent-release-changes
- GitHub, Verifying the integrity of a release: https://docs.github.com/en/code-security/supply-chain-security/understanding-your-software-supply-chain/verifying-the-integrity-of-a-release
- GitHub, About rulesets, and Creating rulesets for a repository: https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets
- Cappos, Samuel, Baker, Hartman 2008, *A Look In The Mirror: Attacks on Package Managers* (freeze and replay attacks).
- Samuel, Mathewson, Cappos, Dingledine 2010, *Survivable Key Compromise in Software Update Systems* (TUF).
