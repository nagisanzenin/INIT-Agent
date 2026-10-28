"""The one-time AppArmor step for Ubuntu 24.04 and later (docs/platforms.md 3.4.6; REL-050).

- `install.sh --print-apparmor` prints exactly the commands the owner runs once with sudo: root-owned copies of util-linux
  unshare and bubblewrap in /usr/local/lib/init-agent (mode 0750, the owner's group, so other users cannot run them), and
  a profile that grants those two paths `userns` and nothing else. It writes nothing and runs nothing.
- Nothing INIT prints asks for the global sysctl (kernel.apparmor_restrict_unprivileged_userns=0): not install.sh, not
  uninstall.sh, not the user-facing docs.
- uninstall.sh refuses an unknown argument before it touches anything.

The Root's side (it uses a copy only when nobody but root can change it) is tested in init-sandbox-profiles entry.rs;
the step itself runs on a hosted Ubuntu 24.04 runner (ci/github/linux-x86_64.yml). Stdlib only; ci/local.sh runs it
with the other tests here (`python3 -m unittest discover -s tests/install`).
"""
import os
import re
import shutil
import subprocess
import tempfile
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SCRIPTS = os.path.join(REPO, "scripts")
HELPERS = "/usr/local/lib/init-agent"


def tree(d):
    out = []
    for root, dirs, files in os.walk(d):
        for n in sorted(dirs + files):
            out.append(os.path.relpath(os.path.join(root, n), d))
    return sorted(out)


class AppArmorStep(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="iaa-", dir="/tmp")
        self.addCleanup(lambda: shutil.rmtree(self.tmp, ignore_errors=True))
        self.env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": self.tmp, "INIT_ROOT_HOME": os.path.join(self.tmp, "r"),
                    "INIT_HOME": os.path.join(self.tmp, "i"), "INIT_BIN_DIR": os.path.join(self.tmp, "bin")}

    def test_print_apparmor_prints_only_the_narrow_commands(self):
        before = tree(self.tmp)
        r = subprocess.run(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--print-apparmor"], env=self.env,
                           capture_output=True, text=True, timeout=30)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stderr, "")
        self.assertEqual(tree(self.tmp), before, "it writes nothing")
        lines = r.stdout.splitlines()
        gid = os.getgid()
        self.assertEqual(lines[0], f"sudo install -d -o root -g root -m 0755 {HELPERS}")
        self.assertEqual(lines[1], f"sudo install -o root -g {gid} -m 0750 /usr/bin/unshare {HELPERS}/unshare")
        self.assertEqual(lines[2], f"sudo install -o root -g {gid} -m 0750 /usr/bin/bwrap {HELPERS}/bwrap")
        self.assertTrue(lines[3].endswith("| sudo tee /etc/apparmor.d/init-agent >/dev/null"), lines[3])
        self.assertEqual(lines[4], "sudo apparmor_parser -r /etc/apparmor.d/init-agent")
        self.assertEqual(len(lines), 5)
        # the profile text, as the printf writes it: two unconfined-mode profiles, each attached to one root-owned copy
        # and granting `userns` alone
        quoted = re.findall(r"'([^']*)'", lines[3])
        self.assertEqual(quoted[0], "%s\\n")
        profile = quoted[1:]
        self.assertEqual(profile, ["abi <abi/4.0>,", "include <tunables/global>",
                                   f"profile init-agent-unshare {HELPERS}/unshare flags=(unconfined) {{", "  userns,", "}",
                                   f"profile init-agent-bwrap {HELPERS}/bwrap flags=(unconfined) {{", "  userns,", "}"])
        rules = [x.strip() for x in profile if x.startswith("  ")]
        self.assertEqual(rules, ["userns,", "userns,"], "no rule but userns")
        self.assertNotIn("sysctl", r.stdout)

    def test_nothing_asks_for_the_global_sysctl(self):
        # what INIT prints or tells the owner to type: the installer, the uninstaller, the launcher, get.sh, the guide,
        # the platform doc's user-facing text. A line may name the sysctl only to say INIT does not ask for it.
        files = [os.path.join(SCRIPTS, f) for f in ("install.sh", "uninstall.sh", "get.sh", "init-agent")]
        files += [os.path.join(REPO, "docs", f) for f in ("guide.md", "platforms.md")]
        files += [os.path.join(REPO, "README.md"), os.path.join(REPO, "docs", "release-notes", "v0.5.0-preview.md")]
        for f in files:
            with open(f, encoding="utf-8") as h:
                for n, line in enumerate(h, 1):
                    self.assertNotRegex(line, r"sysctl\s+-w\s+kernel\.apparmor_restrict_unprivileged_userns",
                                        f"{os.path.relpath(f, REPO)}:{n} tells the owner to lift the restriction for every program")

    def test_uninstall_refuses_an_unknown_argument_first(self):
        before = tree(self.tmp)
        r = subprocess.run(["/bin/bash", os.path.join(SCRIPTS, "uninstall.sh"), "--bogus"], env=self.env,
                           capture_output=True, text=True, timeout=30)
        self.assertEqual(r.returncode, 1)
        self.assertIn("unknown argument --bogus", r.stderr)
        self.assertEqual(tree(self.tmp), before)


if __name__ == "__main__":
    unittest.main()
