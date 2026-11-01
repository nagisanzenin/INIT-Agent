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
        self.assertEqual(lines[0], f"/usr/bin/sudo install -d -o root -g root -m 0755 {HELPERS}")
        self.assertEqual(lines[1], f"/usr/bin/sudo install -o root -g {gid} -m 0750 /usr/bin/unshare {HELPERS}/unshare")
        self.assertEqual(lines[2], f"/usr/bin/sudo install -o root -g {gid} -m 0750 /usr/bin/bwrap {HELPERS}/bwrap")
        self.assertTrue(lines[3].endswith("| /usr/bin/sudo tee /etc/apparmor.d/init-agent >/dev/null"), lines[3])
        self.assertEqual(lines[4], "/usr/bin/sudo apparmor_parser -r /etc/apparmor.d/init-agent")
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

    def test_every_sudo_line_names_it_by_its_path(self):
        # v0.5.5 (review B of lane J): a bare `sudo` in a line the owner types is found on PATH, and a full-access agent
        # can write a PATH directory (~/.local/bin is on it): a planted sudo would get the owner's password. The
        # uninstaller's removal commands, printed for a profile and copies that exist (here: in a scratch folder), and
        # every line of install.sh and uninstall.sh that prints a command run with sudo
        import test_printed_commands as pc
        t = tempfile.mkdtemp(prefix="iaa-su-", dir="/tmp")
        self.addCleanup(lambda: shutil.rmtree(t, ignore_errors=True))
        prof, helpers = os.path.join(t, "init-agent"), os.path.join(t, "lib")
        open(prof, "w").close()
        os.makedirs(helpers)
        src = pc.functions(os.path.join(SCRIPTS, "uninstall.sh"), "aa_remove") + "\naa_remove"
        r = subprocess.run(["/bin/bash", "-p", "-c", src], env={"PATH": "/usr/bin:/bin", "AA_PROFILE": prof, "HELPERS": helpers},
                           capture_output=True, text=True, timeout=30)
        self.assertEqual(r.stdout.splitlines(), ["/usr/bin/sudo apparmor_parser -R " + prof, "/usr/bin/sudo rm -f " + prof,
                                                 "/usr/bin/sudo rm -f %s/unshare %s/bwrap" % (helpers, helpers),
                                                 "/usr/bin/sudo rmdir " + helpers], r.stderr)
        bare = []
        for name in ("install.sh", "uninstall.sh"):
            with open(os.path.join(SCRIPTS, name)) as f:
                for n, line in enumerate(f, 1):
                    if not line.lstrip().startswith("#") and re.search(r"(?<![/\w.-])sudo [a-z_]", line):
                        bare.append("%s:%d: %s" % (name, n, line.strip()))
        self.assertEqual(bare, [], "a bare sudo in a printed command")


class ChildBash(unittest.TestCase):
    """v0.5.5 (review B of lane J): the commands the AppArmor step runs with sudo go to a child bash, which is started
    as `/bin/bash -p -e`. Without -p that bash ran the file BASH_ENV names and imported exported functions, so the
    environment, which a full-access agent can set through the owner's startup files, could run code first or in place
    of a command. The case: each script's own pipeline, as written, fed `true` and `echo ok` in place of the commands
    (no sudo runs), with a planted BASH_ENV and an exported function `true` that write a marker."""

    def test_the_child_bash_takes_nothing_from_the_environment(self):
        t = tempfile.mkdtemp(prefix="iaa-cb-", dir="/tmp")
        self.addCleanup(lambda: shutil.rmtree(t, ignore_errors=True))
        marker, evil = os.path.join(t, "ran"), os.path.join(t, "evil.sh")
        with open(evil, "w") as f:
            f.write("echo BASH_ENV >>'%s'\n" % marker)
        fn = "() { echo function >>'%s'; }" % marker
        env = {"PATH": "/usr/bin:/bin", "HOME": t, "BASH_ENV": evil, "BASH_FUNC_true%%": fn, "BASH_FUNC_true()": fn}
        for name, producer in (("uninstall.sh", "aa_remove"), ("install.sh", "apparmor_commands")):
            with self.subTest(script=name):
                if os.path.exists(marker):
                    os.remove(marker)
                with open(os.path.join(SCRIPTS, name)) as f:
                    m = re.search(r"%s \| (/bin/bash[^|&;]*?) (?:&&|\|\|)" % producer, f.read())
                self.assertTrue(m, "no `%s | /bin/bash …` in %s" % (producer, name))
                p = subprocess.run(["/bin/bash", "-p", "-c", "printf 'true\\necho ok\\n' | " + m.group(1)], env=env,
                                   stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
                self.assertEqual((p.returncode, p.stdout), (0, "ok\n"), p.stderr)
                self.assertFalse(os.path.exists(marker), "the environment ran code in %s's child bash" % name)


if __name__ == "__main__":
    unittest.main()
