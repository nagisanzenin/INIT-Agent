# Native platform release gate

Run before publishing a release:

```sh
python3 scripts/qualify-release.py \
  --receipt /ci-evidence/macos/receipt.json \
  --receipt /ci-evidence/linux/receipt.json \
  --receipt /ci-evidence/windows-native/receipt.json \
  --out /release-artifacts/platform-qualification.json
```

Exit status is **0 only when all required native platforms are qualified**, 1 for missing/failed/unsupported qualification, and 2 for a gate execution error. Running without receipts is a useful read-only preflight and **returns 1**, even on a machine with every prerequisite installed. It does not build, launch INIT, change confinement, access credentials or perform network I/O.

`release_allowed` is the platform gate verdict only. Provider compatibility, self-evolution benchmark, packaging provenance and final release approval remain separate requirements. `scripts/release.sh` still creates development build artifacts; successful packaging is not release qualification.

Statuses:

- `unsupported`: a known implementation blocker remains in the current source.
- `unverified`: current native test evidence is missing, including any missing required check.
- `failed`: a native check failed, skipped an assertion, or returned nonzero.
- `tested`: every required check passed with source-matching, native evidence.

Required checks are `native-build`, `confinement`, `egress-isolation`, `held-process-stop`, `snapshot-restore`, `memory-limit`, `evolution-lifecycle`, and `install-update-uninstall`. Each check must exercise its runtime behavior on the native OS and preserve named assertions. A compilation alone does not certify any runtime check. A mock Python gate unit test certifies only the gate, not an OS capability. WSL and cross-compilation evidence cannot substitute for native Windows or native Linux qualification.

The current source has explicit Linux launcher/watchdog stubs and no native Windows backend. Passing receipts cannot override these source blockers. macOS is implemented but remains unverified until the current source has all required evidence. Source checks are conservative known-blocker checks, not a Rust cfg parser or a complete security audit; replacing a backend requires reviewing these checks and adding native integration coverage.

## Evidence contract

Receipts are **trusted CI output**, not cryptographic attestations. The verifier checks consistency, source freshness, artifact integrity and coverage. It cannot determine whether a malicious producer forged assertion results. Keep evidence from the execution runner, not handwritten success claims.

A receipt has this shape (the sample is intentionally incomplete and cannot pass):

```json
{
  "schema": "init-platform-evidence-v1",
  "platform": "macos",
  "source_sha256": "the source_sha256 from this gate on the tested checkout",
  "host": {
    "platform": "macos",
    "system": "Darwin",
    "release": "actual native OS release",
    "architecture": "arm64",
    "environment": "native"
  },
  "checks": [
    {
      "id": "confinement",
      "artifact": "confinement.json",
      "artifact_sha256": "sha256 of the exact assertion artifact bytes"
    }
  ]
}
```

The referenced assertion artifact records the executed command and results:

```json
{
  "id": "confinement",
  "command": ["the actual runner", "its arguments"],
  "exit_code": 0,
  "assertions": [
    {"name": "Root home read denied", "status": "passed"},
    {"name": "Root home write denied", "status": "passed"},
    {"name": "Workspace grant enforced", "status": "passed"}
  ]
}
```

Artifacts must be under the receipt directory; absolute paths, traversal and escaping symlinks are refused. Empty assertion lists, skipped assertions, duplicate checks/platforms, malformed data, altered artifact bytes and stale source digests block release. Gate/source files are fingerprinted; receipts and artifacts should live outside source scopes. The source digest excludes vendors, build outputs and sealed task sources, and does not attest dependency or sealed-task contents.

Existing macOS suites (`root` sandbox selftests, `rsafe`, `access`, `memwatch`, `x8`, kernel recovery/extend tests and `scripts/release-e2e.sh`) are ingredients for a native runner. They are not automatically counted as satisfying checks: some require fixture features, some have platform guards, and successful zero-test runs are insufficient. Native end-to-end upgrade and evolution evidence must also be included. This initial gate does not fabricate missing runner integrations.

Run the free gate unit tests:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests/platform_release -p 'test_*.py' -v
```
