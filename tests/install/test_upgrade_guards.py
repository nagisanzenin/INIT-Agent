"""The upgrade's entry points refuse the wrong situation before they write anything (lane UPGRADE, v0.5.0).

- get.sh --upgrade with no install, get.sh without --upgrade over an install, install.sh --upgrade with no install and
  install.sh without --upgrade over an install: each exits 1 with a message that names the right command, and leaves
  the scratch directory as it was (get.sh checks this before it downloads or unpacks anything).
- `init-agent upgrade` runs the get.sh kept in ROOT_HOME/cache/ with --upgrade and its own flags mapped (--bundle is
  get.sh's --from); with no kept copy (an install of 0.4.1) it exits 69 and prints the one-line upgrade that runs the
  current get.sh with the system's curl and sh.
- install.sh over 0.4.1 or 0.5.0 names its own `install.sh --upgrade` and that one-line upgrade, never
  `init-agent upgrade` (0.5.0's runs 0.5.0's get.sh); over 0.5.1 or later, `init-agent upgrade`. Each line carries this
  ROOT_HOME and quotes the paths (the repository's own path has a space).

Each run gets a scratch HOME, ROOT_HOME and INIT_HOME. Stdlib only; ci/local.sh runs it with the other tests here
(`python3 -m unittest discover -s tests/install`). The upgrade itself runs end to end in scripts/upgrade-e2e.sh.
"""
import os
import platform
import shutil
import subprocess
import tempfile
import unittest

from test_printed_commands import shq

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SCRIPTS = os.path.join(REPO, "scripts")
# get.sh's platform checks come first: macOS on arm64, or Linux with bubblewrap
GET_RUNS_HERE = (platform.system() == "Darwin" and platform.machine() == "arm64") or os.path.exists("/usr/bin/bwrap")


def tree(d):
    """Names and sizes under d, without Library/ (macOS's python3 shim caches there when get.sh checks for python3)."""
    out = []
    for root, dirs, files in os.walk(d):
        if root == d:
            dirs[:] = [x for x in dirs if x != "Library"]
        for n in sorted(dirs + files):
            p = os.path.join(root, n)
            if os.path.relpath(p, d) != "Library":
                out.append((os.path.relpath(p, d), os.path.getsize(p) if os.path.isfile(p) else -1))
    return sorted(out)


class UpgradeGuards(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="iu-", dir="/tmp")  # short: homes are at most 64 bytes
        self.addCleanup(lambda: shutil.rmtree(self.tmp, ignore_errors=True))
        self.rh, self.ih = os.path.join(self.tmp, "r"), os.path.join(self.tmp, "i")
        self.env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": self.tmp, "INIT_ROOT_HOME": self.rh, "INIT_HOME": self.ih,
                    "INIT_APP_DIR": os.path.join(self.tmp, "app"), "INIT_BIN_DIR": os.path.join(self.tmp, "bin"),
                    "INIT_DIST_URL": "https://releases.invalid/init"}

    def installed(self):
        os.makedirs(os.path.join(self.rh, "cache"))
        with open(os.path.join(self.rh, "install.json"), "w") as f:
            f.write('{"init_home":"%s","v":1}\n' % self.ih)

    def run_script(self, argv):
        before = tree(self.tmp)
        p = subprocess.run(argv, env=self.env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)
        self.assertEqual(tree(self.tmp), before, f"{argv}: wrote something\n{p.stdout}\n{p.stderr}")
        return p

    @unittest.skipUnless(GET_RUNS_HERE, "get.sh refuses this platform before its upgrade checks")
    def test_get_upgrade_needs_an_install(self):
        p = self.run_script(["/bin/sh", os.path.join(SCRIPTS, "get.sh"), "--upgrade"])
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("is not installed", p.stderr)
        self.assertIn("without --upgrade", p.stderr)

    @unittest.skipUnless(GET_RUNS_HERE, "get.sh refuses this platform before its upgrade checks")
    def test_get_over_an_install_points_to_the_upgrade(self):
        self.installed()
        p = self.run_script(["/bin/sh", os.path.join(SCRIPTS, "get.sh")])
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("already installed", p.stderr)
        self.assertIn("init-agent upgrade", p.stderr)
        self.assertIn("--upgrade", p.stderr)

    def test_get_version_is_a_plain_version(self):
        if not GET_RUNS_HERE:
            self.skipTest("get.sh refuses this platform first")
        self.installed()
        p = self.run_script(["/bin/sh", os.path.join(SCRIPTS, "get.sh"), "--upgrade", "--version", "0.5.0;rm"])
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("--version takes a version", p.stderr)

    def test_install_upgrade_needs_an_install(self):
        p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--upgrade", "--yes"])
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("INIT is not installed", p.stderr)

    def test_install_over_an_install_points_to_the_upgrade(self):
        self.installed()
        p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--yes", "--no-service"])
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("already installed", p.stderr)
        self.assertIn("init-agent upgrade", p.stderr)

    def fake_root(self, version):
        os.makedirs(os.path.join(self.rh, "bin"), exist_ok=True)
        p = os.path.join(self.rh, "bin", "init-root")
        with open(p, "w") as f:
            f.write('#!/bin/sh\n[ "$1" = --version ] && echo "init-root %s"\n' % version)
        os.chmod(p, 0o755)

    def test_install_over_a_pre_041_install_prints_the_uninstall(self):
        # the sibling of get.sh's message (build/status/REL-050.md, "After the release"): 0.3.1 has no init-agent
        self.installed()
        self.fake_root("0.3.1")
        for extra in ([], ["--upgrade"]):
            p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--yes", "--no-service", *extra])
            self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
            self.assertIn("INIT 0.3.1 is installed", p.stderr)
            self.assertIn("before 0.4.1 is not supported", p.stderr)
            self.assertIn("\n  INIT_ROOT_HOME=%s bash %s\n" % (shq(self.rh), shq(os.path.join(SCRIPTS, "uninstall.sh"))), p.stderr)
            self.assertNotIn("init-agent upgrade", p.stderr)

    def test_install_over_041_or_050_points_to_install_sh_upgrade_and_the_safe_line(self):
        self.installed()
        rh = "INIT_ROOT_HOME=%s " % shq(self.rh)
        for version in ("0.4.1", "0.5.0"):
            self.fake_root(version)
            p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--yes", "--no-service"])
            self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
            self.assertIn("INIT %s is already installed" % version, p.stderr)
            self.assertIn("\n  %s%s --upgrade\n" % (rh, shq(os.path.join(SCRIPTS, "install.sh"))), p.stderr)
            self.assertIn("once from your own shell: /usr/bin/curl -q -fsSL https://releases.invalid/init/get.sh | %s/bin/sh -s --"
                          " --upgrade\n" % rh, p.stderr)
            self.assertNotIn("\n  init-agent upgrade", p.stderr)
            self.assertNotIn("\n  %sinit-agent upgrade" % rh, p.stderr)
        self.fake_root("0.5.1")
        p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--yes", "--no-service"])
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("\n  %sinit-agent upgrade        (or: %s%s --upgrade)\n" % (rh, rh, shq(os.path.join(SCRIPTS, "install.sh"))), p.stderr)
        self.assertNotIn("/usr/bin/curl", p.stderr)

    def test_launcher_without_a_kept_get_sh(self):
        self.installed()
        p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "init-agent"), "upgrade"])
        self.assertEqual(p.returncode, 69, p.stderr)
        self.assertIn("/usr/bin/curl -q -fsSL https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh |"
                      " INIT_ROOT_HOME=%s /bin/sh -s -- --upgrade\n" % shq(self.rh), p.stderr)

    def test_launcher_runs_the_kept_get_sh_with_its_flags(self):
        cache = os.path.join(self.rh, "cache")
        os.makedirs(cache)
        out = os.path.join(self.tmp, "argv.txt")
        with open(os.path.join(cache, "get.sh"), "w") as f:
            f.write('for a in "$@"; do printf "%%s\\n" "$a"; done > "%s"\n' % out)
        p = subprocess.run(["/bin/bash", os.path.join(SCRIPTS, "init-agent"), "upgrade", "--bundle", "b.tar.gz", "--sha256", "ab",
                            "--version", "0.5.0", "--", "--probe-timeout", "30"], env=self.env, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        with open(out) as f:
            self.assertEqual(f.read().split("\n")[:-1], ["--upgrade", "--from", "b.tar.gz", "--sha256", "ab", "--version", "0.5.0",
                                                         "--", "--probe-timeout", "30"])
        p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "init-agent"), "upgrade", "--nope"])
        self.assertEqual(p.returncode, 64, p.stderr)


if __name__ == "__main__":
    unittest.main()
