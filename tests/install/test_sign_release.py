"""scripts/sign-release.sh (v0.5.3, PLAN-052 L4; docs/release-signing.md §8): SHA256SUMS, its signature, the check.

With a throwaway key in the test's scratch directory (tests/install/signing.py) and a stand-in release directory: the
three steps in order; what it refuses about the key file (others can read it, a link, inside a git work tree, not
ed25519); a signature by a key the release's get.sh does not pin; an asset changed after the signature; a stale
.sha256; and that no key material reaches its output. Needs ssh-keygen with -Y (OpenSSH 8.1+). Stdlib only.
"""
import datetime
import json
import os
import shlex
import subprocess
import tempfile
import unittest

import signing
from test_printed_commands import shq
from test_upgrade_trust import rm_tree, sha

SIGN = os.path.join(signing.REPO, "scripts", "sign-release.sh")


@unittest.skipUnless(signing.ssh_keygen_signs(), "needs ssh-keygen with -Y (OpenSSH 8.1+)")
class SignRelease(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="isr-", dir="/tmp")
        self.addCleanup(rm_tree, self.tmp)
        self.keys = os.path.join(self.tmp, "keys")
        self.k1, self.pub1 = signing.keypair(self.keys, "k1")
        self.k2, _ = signing.keypair(self.keys, "k2")
        self.dir = os.path.join(self.tmp, "assets")
        os.makedirs(self.dir)
        signing.get_sh(self.dir, [self.pub1])
        self.bundle = "init-v9.9.9-test-macos-arm64.tar.gz"
        self.put(self.bundle, b"a stand-in bundle\n")
        self.put(self.bundle + ".sha256", f"{sha(b'a stand-in bundle' + bytes([10]))}  {self.bundle}\n".encode())
        self.put("latest.json", json.dumps({"v": 1, "version": "9.9.9-test", "file": self.bundle}).encode())

    def put(self, name, data):
        with open(os.path.join(self.dir, name), "wb") as f:
            f.write(data)

    def run_sr(self, *args):
        env = {"PATH": "/usr/bin:/bin", "HOME": self.tmp, "LANG": "C", "TMPDIR": self.tmp}
        return subprocess.run(["/bin/bash", SIGN, *args], env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                              timeout=60)

    def test_sums_sign_check_in_order(self):
        p = self.run_sr("sums", "--version", "9.9.9-test", self.dir)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        with open(os.path.join(self.dir, "SHA256SUMS")) as f:
            text = f.read()
        exp = (datetime.datetime.now(datetime.timezone.utc).date() + datetime.timedelta(days=180)).isoformat()
        self.assertTrue(text.startswith(f"# INIT release SHA256SUMS v1\n# version: 9.9.9-test\n# expires: {exp}\n"), text)
        self.assertEqual([l.split("  ")[1] for l in text.splitlines()[3:]], ["get.sh", self.bundle, self.bundle + ".sha256", "latest.json"])
        p = self.run_sr("sign", "--key", self.k1, "--yes", self.dir)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("wrote %s/SHA256SUMS.sig" % os.path.realpath(self.dir), p.stdout)
        self.assertIn('Good "init-release" signature for init-release with ED25519 key', p.stdout)
        self.assertIn("every asset matches SHA256SUMS (INIT 9.9.9-test)", p.stdout)
        with open(self.k1) as f:
            secret = [l for l in f.read().splitlines() if not l.startswith("-----")]
        for line in secret:
            self.assertNotIn(line, p.stdout + p.stderr, "key material in the output")
        # an asset changed after the signature: check finds it
        self.put("get.sh", b"#!/bin/sh\necho changed\n")
        p = self.run_sr("check", self.dir)
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("pins no release key", p.stderr)  # this get.sh pins nothing: the check stops there first
        signing.get_sh(self.dir, [self.pub1])
        self.put(self.bundle, b"another bundle\n")
        p = self.run_sr("check", self.dir)
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("FAIL %s: sha256" % self.bundle, p.stdout)

    def test_a_key_others_can_read_is_refused(self):
        self.assertEqual(self.run_sr("sums", "--version", "9.9.9-test", self.dir).returncode, 0)
        for mode in (0o640, 0o604, 0o660):
            os.chmod(self.k1, mode)
            p = self.run_sr("sign", "--key", self.k1, "--yes", self.dir)
            self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
            self.assertIn("is readable or writable by others (mode %o): run chmod 600 on it first" % mode, p.stderr)
            self.assertFalse(os.path.exists(os.path.join(self.dir, "SHA256SUMS.sig")))
        os.chmod(self.k1, 0o400)
        self.assertEqual(self.run_sr("sign", "--key", self.k1, "--yes", self.dir).returncode, 0)

    def test_a_link_a_key_in_a_git_work_tree_and_another_key_type_are_refused(self):
        self.assertEqual(self.run_sr("sums", "--version", "9.9.9-test", self.dir).returncode, 0)
        link = os.path.join(self.tmp, "link")
        os.symlink(self.k1, link)
        self.assertIn("is a link, not a regular file", self.run_sr("sign", "--key", link, "--yes", self.dir).stderr)
        repo = os.path.join(self.tmp, "repo")
        os.makedirs(os.path.join(repo, ".git"))
        os.makedirs(os.path.join(repo, "sub"))
        inside = os.path.join(repo, "sub", "key")
        os.rename(self.k1, inside)
        p = self.run_sr("sign", "--key", inside, "--yes", self.dir)
        self.assertIn("lies inside the git work tree %s" % os.path.realpath(repo), p.stderr)
        rsa = os.path.join(self.keys, "rsa")
        subprocess.run([signing.SSH_KEYGEN, "-q", "-t", "rsa", "-b", "2048", "-N", "", "-f", rsa], check=True)
        self.assertIn("is not an ed25519 key", self.run_sr("sign", "--key", rsa, "--yes", self.dir).stderr)
        self.assertFalse(os.path.exists(os.path.join(self.dir, "SHA256SUMS.sig")))

    def test_a_key_get_sh_does_not_pin_fails_the_check(self):
        self.assertEqual(self.run_sr("sums", "--version", "9.9.9-test", self.dir).returncode, 0)
        p = self.run_sr("sign", "--key", self.k2, "--yes", self.dir)
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("SHA256SUMS.sig does not verify under the key(s) get.sh pins", p.stderr)

    def test_the_next_step_runs_as_printed_from_a_path_with_a_space(self):
        # the release review of v0.5.3: sums printed DIR unquoted, and the main checkout's path holds a space, so the
        # pasted line failed ("one DIR only")
        spaced = os.path.join(self.tmp, "INIT Agent's build", "assets")
        os.makedirs(os.path.dirname(spaced))
        os.rename(self.dir, spaced)
        p = self.run_sr("sums", "--version", "9.9.9-test", spaced)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        line = p.stdout.splitlines()[-1].strip()
        self.assertEqual(line, "scripts/sign-release.sh sign --key <the release key> %s" % shq(os.path.realpath(spaced)))
        line = line.replace("<the release key>", shlex.quote(self.k1)) + " --yes"
        env = {"PATH": "/usr/bin:/bin", "HOME": self.tmp, "LANG": "C", "TMPDIR": self.tmp}
        r = subprocess.run(["/bin/sh", "-c", line], cwd=signing.REPO, env=env, stdin=subprocess.DEVNULL, capture_output=True,
                           text=True, timeout=60)
        self.assertEqual(r.returncode, 0, line + "\n" + r.stdout + r.stderr)
        self.assertTrue(os.path.isfile(os.path.join(spaced, "SHA256SUMS.sig")))

    def test_sums_refuses_what_a_release_must_not_be(self):
        self.put(self.bundle + ".sha256", f"{'0' * 64}  {self.bundle}\n".encode())
        self.assertIn(".sha256 does not match", self.run_sr("sums", "--version", "9.9.9-test", self.dir).stderr)
        self.assertIn("latest.json names 9.9.9-test, not 9.9.8", self.run_sr("sums", "--version", "9.9.8", self.dir).stderr)
        self.assertIn("--valid-days takes 30 to 366", self.run_sr("sums", "--version", "9.9.9-test", "--valid-days", "999", self.dir).stderr)
        signing.get_sh(self.dir, [])
        self.assertIn("pins no release key", self.run_sr("sums", "--version", "9.9.9-test", self.dir).stderr)
        self.assertFalse(os.path.exists(os.path.join(self.dir, "SHA256SUMS")))


if __name__ == "__main__":
    unittest.main()
