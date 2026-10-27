"""scripts/install.sh refuses a system bin directory before it writes anything (nagisanzenin/INIT-Agent#1).

The guard runs first in the preflight, before any platform check, so these cases need no toolchain and no install:
each run gets a scratch HOME, ROOT_HOME and INIT_HOME, must exit 1 with the refusal on stderr, and must leave the
scratch directory exactly as it was. Stdlib only; ci/local.sh runs it (`python3 -m unittest discover -s tests/install`).
The rest of the command's install (init-agent, the macOS shim, the old Linux launcher, a foreign `init`) is checked
end to end by scripts/release-e2e.sh and scripts/bundle-e2e.sh.
"""
import os
import subprocess
import sys
import tempfile
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
INSTALL = os.path.join(REPO, "scripts", "install.sh")


def tree(d):
    out = []
    for root, dirs, files in os.walk(d):
        for n in sorted(dirs + files):
            out.append(os.path.relpath(os.path.join(root, n), d))
    return sorted(out)


class BinDirGuard(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="init-guard-")
        self.addCleanup(lambda: subprocess.run(["rm", "-rf", "--", self.tmp], check=False))

    def run_install(self, bin_dir):
        env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": self.tmp, "INIT_ROOT_HOME": os.path.join(self.tmp, "r"),
               "INIT_HOME": os.path.join(self.tmp, "i"), "INIT_BIN_DIR": bin_dir}
        before = tree(self.tmp)
        p = subprocess.run(["/bin/bash", INSTALL, "--yes", "--no-service"], env=env, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(tree(self.tmp), before, f"{bin_dir}: install.sh wrote something")
        return p

    def assert_refused(self, bin_dir, words="refusing INIT_BIN_DIR"):
        p = self.run_install(bin_dir)
        self.assertEqual(p.returncode, 1, f"{bin_dir}: exit {p.returncode}\n{p.stdout}\n{p.stderr}")
        self.assertIn(words, p.stderr, f"{bin_dir}: {p.stderr}")

    def test_system_directories(self):
        for d in ("/sbin", "/usr/sbin", "/bin", "/usr/bin"):
            with self.subTest(d=d):
                self.assert_refused(d)

    def test_spellings_of_a_system_directory(self):
        for d in ("/usr/bin/", "//usr//bin", "/usr/local/../bin", "/usr/bin/.", "/sbin/"):
            if os.path.isdir(d.rstrip("/") or "/"):
                with self.subTest(d=d):
                    self.assert_refused(d)

    def test_a_symlink_to_a_system_directory(self):
        link = os.path.join(self.tmp, "mybin")
        os.symlink("/usr/bin", link)
        self.assert_refused(link)

    @unittest.skipUnless(sys.platform == "darwin", "macOS volumes are case-insensitive by default")
    def test_case_insensitive_name(self):
        if os.path.isdir("/USR/BIN"):
            self.assert_refused("/USR/BIN")

    def test_relative_bin_dir(self):
        self.assert_refused("bin", words="is not an absolute path")


if __name__ == "__main__":
    unittest.main()
