"""`init-agent upgrade` takes nothing from the environment (v0.5.4; the post-release review of v0.5.3).

Until 0.5.3 the launcher's upgrade preferred an INIT_ROOT_HOME in the environment over the home install.sh baked into it,
and handed the whole environment on to the kept get.sh. With full access the agent can write the owner's shell startup
files, so a planted INIT_ROOT_HOME ran another home's cache/get.sh, one the agent wrote (docs/release-signing.md A3).
The case: a launcher baked for home A, as install.sh writes it, run with INIT_ROOT_HOME=B (whose cache/get.sh is the
agent's), INIT_DIST_URL, INIT_BIN_DIR and a planted PATH in the environment. A's get.sh must run, with HOME, A and the
system PATH only; B's must not; and the launcher says what it ignored. Stdlib only.
"""
import os
import subprocess
import tempfile
import unittest

from test_installed_elsewhere import put_launcher

SYSTEM_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"


class CleanUpgrade(unittest.TestCase):
    def test_the_baked_home_and_a_clean_environment(self):
        t = tempfile.mkdtemp(prefix="i5-cu-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", t], check=False)
        a, b, report, planted = (os.path.join(t, n) for n in ("a", "b", "get.txt", "planted.txt"))
        for h in (a, b):
            os.makedirs(os.path.join(h, "cache"))
            with open(os.path.join(h, "install.json"), "w") as f:
                f.write('{"init_home":"%s/i","v":1}\n' % h)
        with open(os.path.join(a, "cache", "get.sh"), "w") as f:
            f.write('{ for a in "$@"; do printf "%%s\\n" "$a"; done; env | sort; } >\'%s\'\n' % report)
        with open(os.path.join(b, "cache", "get.sh"), "w") as f:
            f.write("echo planted >'%s'\n" % planted)
        launcher = put_launcher(os.path.join(t, "bin"), a)
        env = {"PATH": "%s/evil:%s" % (t, SYSTEM_PATH), "HOME": t, "INIT_ROOT_HOME": b, "LANG": "C",
               "INIT_DIST_URL": "https://dist.invalid/agent", "INIT_BIN_DIR": os.path.join(t, "evil"), "XDG_RUNTIME_DIR": t}
        p = subprocess.run(["/bin/bash", launcher, "upgrade", "--version", "0.5.4"], env=env, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertFalse(os.path.exists(planted), "the get.sh of the home in the environment ran")
        with open(report) as f:
            lines = f.read().splitlines()
        self.assertEqual(lines[:3], ["--upgrade", "--version", "0.5.4"])
        self.assertEqual(sorted(l for l in lines[3:] if not l.startswith(("PWD=", "SHLVL=", "_=", "OLDPWD="))),
                         ["HOME=%s" % t, "INIT_ROOT_HOME=%s" % a, "PATH=%s" % SYSTEM_PATH])
        self.assertIn("INIT_ROOT_HOME=%s in this shell's environment is ignored; this upgrades the INIT in\n  %s," % (b, a), p.stderr)
        # the home it was installed for, given again: no note
        p = subprocess.run(["/bin/bash", launcher, "upgrade"], env=dict(env, INIT_ROOT_HOME=a), stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual((p.returncode, p.stderr), (0, ""))


if __name__ == "__main__":
    unittest.main()
