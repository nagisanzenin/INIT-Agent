"""The launcher runs no code from the environment before its first line (v0.5.4; the release review of v0.5.4).

`#!/bin/bash` honours BASH_ENV (a file it runs first), SHELLOPTS with xtrace and a PS4 that runs a command, and
exported functions (one named `exec` replaced the launcher's env -i hand-over to the kept get.sh). With full access the
agent can set all three through the owner's shell startup files. `#!/bin/bash -p` (privileged mode) ignores them, in
macOS's bash 3.2 and in bash 5. The case: a launcher baked for a home whose cache/get.sh records that it ran, started
as the owner starts it (by its path, so the #! line counts), with each of the three set to write a marker. No marker
may appear, and the kept get.sh must still run. Also run with bash 5 (Homebrew's, or Linux's /bin/bash) when present.
Stdlib only.
"""
import os
import shutil
import subprocess
import tempfile
import unittest

from test_installed_elsewhere import put_launcher


class NothingFromTheEnvironment(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.mkdtemp(prefix="i5-le-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", self.t], check=False)
        self.rh, self.marker, self.ran = (os.path.join(self.t, n) for n in ("rh", "planted.txt", "get-ran.txt"))
        os.makedirs(os.path.join(self.rh, "cache"))
        with open(os.path.join(self.rh, "install.json"), "w") as f:
            f.write('{"init_home":"%s/i","v":1}\n' % self.t)
        with open(os.path.join(self.rh, "cache", "get.sh"), "w") as f:
            f.write("echo ran >'%s'\n" % self.ran)
        self.launcher = put_launcher(os.path.join(self.t, "bin"), self.rh)
        self.evil = os.path.join(self.t, "evil.sh")
        with open(self.evil, "w") as f:
            f.write("echo BASH_ENV >>'%s'\n" % self.marker)

    def plants(self):
        m = self.marker
        return {"BASH_ENV": {"BASH_ENV": self.evil},
                "SHELLOPTS+PS4": {"SHELLOPTS": "braceexpand:hashall:interactive-comments:xtrace", "PS4": "$(echo PS4 >>%s)" % m},
                "exported exec": {"BASH_FUNC_exec%%": "() { echo exec >>%s; }" % m, "BASH_FUNC_exec()": "() { echo exec >>%s; }" % m}}

    def run_launcher(self, argv0, extra):
        env = dict({"PATH": "/usr/bin:/bin", "HOME": self.t, "LANG": "C"}, **extra)
        return subprocess.run(argv0 + [self.launcher, "upgrade"] if argv0 else [self.launcher, "upgrade"], env=env,
                              stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)

    def check(self, argv0):
        for name, extra in self.plants().items():
            with self.subTest(plant=name, bash=argv0 or "the #! line"):
                for p in (self.marker, self.ran):
                    if os.path.exists(p):
                        os.remove(p)
                p = self.run_launcher(argv0, extra)
                self.assertFalse(os.path.exists(self.marker), "%s ran: %s" % (name, p.stderr))
                self.assertTrue(os.path.exists(self.ran), "the kept get.sh did not run: " + p.stderr)

    def test_the_hash_bang_line_ignores_them(self):
        self.check(None)  # as the owner runs it: ~/.local/bin/init-agent upgrade

    def test_bash_5_with_p_ignores_them(self):
        for b in ("/opt/homebrew/bin/bash", "/usr/local/bin/bash", "/bin/bash"):
            v = subprocess.run([b, "-c", "echo ${BASH_VERSINFO[0]}"], capture_output=True, text=True) if os.path.exists(b) else None
            if v and v.stdout.strip() >= "5":
                self.check([b, "-p"])
                return
        self.skipTest("no bash 5 here")


if __name__ == "__main__":
    unittest.main()
