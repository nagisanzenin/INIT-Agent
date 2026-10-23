#!/usr/bin/env python3
"""Free, filesystem-only tests of the platform release gate (no INIT processes)."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import gate


class GateTests(unittest.TestCase):
    def setUp(self):
        # Each test owns this disposable tree; no real source or live homes are modified.
        self.temp = tempfile.TemporaryDirectory(prefix="init-platform-gate-")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        src = self.repo / "root/crates/init-root/src"
        src.mkdir(parents=True)
        (src / "zone.rs").write_text("// fixture implementation\n")
        (src / "sys.rs").write_text("// fixture implementation\n")
        (src / "plat").mkdir()
        (src / "plat/windows.rs").write_text("// fixture backend\n")

    def receipt(self, target="macos", omit=(), assertion_status="passed"):
        base = self.repo / "receipts" / target
        base.mkdir(parents=True, exist_ok=True)
        rows = []
        for name in gate.CHECKS:
            if name in omit:
                continue
            artifact = {
                "id": name, "command": ["fixture-test", name], "exit_code": 0,
                "assertions": [{"name": "fixture assertion", "status": assertion_status}],
            }
            raw = json.dumps(artifact).encode()
            (base / f"{name}.json").write_bytes(raw)
            rows.append({"id": name, "artifact": f"{name}.json", "artifact_sha256": hashlib.sha256(raw).hexdigest()})
        host = {"macos": ("Darwin", "arm64"), "linux": ("Linux", "x86_64"), "windows-native": ("Windows", "AMD64")}[target]
        data = {
            "schema": "init-platform-evidence-v1", "platform": target,
            "source_sha256": gate.source_digest(self.repo),
            "host": gate.host_info(host[0], "test-release", host[1]), "checks": rows,
        }
        path = base / "receipt.json"
        path.write_text(json.dumps(data))
        return path

    def change(self, path, mutate):
        data = json.loads(path.read_text())
        mutate(data)
        path.write_text(json.dumps(data))

    def test_missing_evidence_never_claims_tested(self):
        report = gate.evaluate(self.repo)
        self.assertFalse(report["release_allowed"])
        self.assertTrue(all(v["status"] == "unverified" for v in report["platforms"].values()))

    def test_all_native_platforms_and_checks_required(self):
        paths = [self.receipt(p) for p in gate.PLATFORMS]
        self.assertTrue(gate.evaluate(self.repo, paths)["release_allowed"])
        self.assertFalse(gate.evaluate(self.repo, paths[:2])["release_allowed"])

    def test_unsupported_source_overrides_passing_receipt(self):
        zone = self.repo / "root/crates/init-root/src/zone.rs"
        zone.write_text('the Linux zone is not built yet (WP6b)\nuse std::os::unix::net::UnixStream;')
        paths = [self.receipt(p) for p in gate.PLATFORMS]
        report = gate.evaluate(self.repo, paths)
        self.assertEqual(report["platforms"]["linux"]["status"], "unsupported")
        self.assertEqual(report["platforms"]["windows-native"]["status"], "unsupported")
        self.assertFalse(report["release_allowed"])

    def test_stale_source_receipt_rejected(self):
        path = self.receipt()
        (self.repo / "root/crates/init-root/src/zone.rs").write_text("changed")
        report = gate.evaluate(self.repo, [path])
        self.assertTrue(report["evidence_errors"])
        self.assertFalse(report["release_allowed"])

    def test_wsl_does_not_qualify_windows_or_linux_native(self):
        self.assertEqual(gate.host_info("Linux", "6.6-microsoft-standard-WSL2", "x86_64")["environment"], "wsl")
        for target in ["windows-native", "linux"]:
            path = self.receipt(target)
            self.change(path, lambda x: x.update(host=gate.host_info("Linux", "6.6-microsoft-standard-WSL2", "x86_64")))
            self.assertTrue(gate.evaluate(self.repo, [path])["evidence_errors"])

    def test_failed_skipped_and_missing_checks_block_release(self):
        for status in ["failed", "skipped"]:
            path = self.receipt(assertion_status=status)
            self.assertEqual(gate.evaluate(self.repo, [path])["platforms"]["macos"]["status"], "failed")
        path = self.receipt(omit=("confinement",))
        self.assertEqual(gate.evaluate(self.repo, [path])["platforms"]["macos"]["status"], "unverified")

    def test_zero_assertion_and_tampered_artifact_rejected(self):
        path = self.receipt()
        artifact = path.parent / "confinement.json"
        artifact.write_text("{}")
        self.assertTrue(gate.evaluate(self.repo, [path])["evidence_errors"])
        path = self.receipt()
        artifact = path.parent / "confinement.json"
        data = json.loads(artifact.read_text())
        data["assertions"] = []
        raw = json.dumps(data).encode()
        artifact.write_bytes(raw)
        self.change(path, lambda x: x["checks"][1].update(artifact_sha256=hashlib.sha256(raw).hexdigest()))
        self.assertTrue(gate.evaluate(self.repo, [path])["evidence_errors"])

    def test_duplicate_and_path_escape_rejected(self):
        path = self.receipt()
        self.assertTrue(gate.evaluate(self.repo, [path, path])["evidence_errors"])
        self.change(path, lambda x: x["checks"][0].update(artifact="../outside.json"))
        self.assertTrue(gate.evaluate(self.repo, [path])["evidence_errors"])

    def test_bad_json_shape_is_failure_not_crash(self):
        path = self.receipt()
        path.write_text("[]")
        self.assertTrue(gate.evaluate(self.repo, [path])["evidence_errors"])

    def test_build_artifacts_do_not_change_source_fingerprint(self):
        before = gate.source_digest(self.repo)
        build = self.repo / "root/crates/init-root/target"
        build.mkdir()
        (build / "binary").write_text("irrelevant")
        self.assertEqual(before, gate.source_digest(self.repo))


if __name__ == "__main__":
    unittest.main()
