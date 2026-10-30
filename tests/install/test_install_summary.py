"""What an install says at the end: one total with its parts, and where to find the uninstall line again (v0.5.4;
user session A of the v0.5.4 candidate).

One install printed three times that read as the same thing: install.sh's "installed in 24 s" (the init-root install
step), its "INIT v0.5.4 is installed (0 min 30 s)" (install.sh's whole run), and get.sh's "installed in 36 s (after the
download)" (all of get.sh, the download included). Each is now named for what it measures, and get.sh's last line is
the total with its parts. The uninstall line was only in the installer's scrollback: README's first screen has it now
(the same line get.sh prints), and install.sh's closing line points there. Stdlib only.
"""
import hashlib
import os
import re
import subprocess
import tempfile
import unittest

from test_printed_commands import functions
from test_upgrade_trust import bundle, TARGET

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GET = os.path.join(REPO, "scripts", "get.sh")
INSTALL = os.path.join(REPO, "scripts", "install.sh")
README = os.path.join(REPO, "README.md")


def read(p):
    with open(p) as f:
        return f.read()


class Summary(unittest.TestCase):
    def test_get_sh_ends_with_one_total_and_its_parts(self):
        t = tempfile.mkdtemp(prefix="i5-sum-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/sh", "-c", 'chmod -R u+w "$1"; rm -rf -- "$1"', "sh", t], check=False)
        tgz = os.path.join(t, "init-v9.9.9-test-%s.tar.gz" % TARGET)
        with open(tgz, "wb") as f:
            f.write(bundle("9.9.9-test", os.path.join(t, "report.txt")))
        with open(tgz, "rb") as f:
            sha = hashlib.sha256(f.read()).hexdigest()
        h = os.path.join(t, "h")
        os.makedirs(h)
        p = subprocess.run(["/bin/sh", GET, "--from", tgz, "--sha256", sha, "--no-rust", "--no-modify-path", "--", "--no-service"],
                           env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": h, "LANG": "C", "INIT_ROOT_HOME": os.path.join(t, "r"),
                                "INIT_APP_DIR": os.path.join(t, "app")}, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                           timeout=120)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertRegex(p.stdout, r"\n  download, checks and unpack: \d+ s\n")
        m = re.search(r"\nINIT 9\.9\.9-test installed in (\d+) s in all: download, checks and unpack (\d+) s · Rust none \(--no-rust\) "
                      r"· install\.sh (\d+) s\.\n", p.stdout)
        self.assertIsNotNone(m, p.stdout)
        total, got, inst = (int(x) for x in m.groups())
        self.assertLessEqual(got + inst, total + 1)  # whole seconds, each rounded down
        for old in ("so far", "(after the download)"):
            self.assertNotIn(old, p.stdout)

    def test_get_sh_upgrade_ends_with_its_total(self):
        # v0.5.4 (review 6): an upgrade printed install.sh's time only; get.sh now ends with the whole run's, in parts
        t = tempfile.mkdtemp(prefix="i5-sum-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/sh", "-c", 'chmod -R u+w "$1"; rm -rf -- "$1"', "sh", t], check=False)
        rh = os.path.join(t, "r")
        os.makedirs(os.path.join(rh, "cache"))
        os.makedirs(os.path.join(rh, "bin"))
        with open(os.path.join(rh, "install.json"), "w") as f:
            f.write("{}\n")
        with open(os.path.join(rh, "bin", "init-root"), "w") as f:
            f.write('#!/bin/sh\n[ "$1" = --version ] && echo "init-root 0.5.4"\n')
        os.chmod(os.path.join(rh, "bin", "init-root"), 0o755)
        tgz = os.path.join(t, "init-v9.9.9-test-%s.tar.gz" % TARGET)
        with open(tgz, "wb") as f:
            f.write(bundle("9.9.9-test", os.path.join(t, "report.txt")))
        with open(tgz, "rb") as f:
            sha = hashlib.sha256(f.read()).hexdigest()
        p = subprocess.run(["/bin/bash", "-p", GET, "--upgrade", "--from", tgz, "--sha256", sha],
                           env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": t, "LANG": "C", "INIT_ROOT_HOME": rh},
                           stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        m = re.search(r"\nThis upgrade took (\d+) s in all: download, checks and unpack (\d+) s · install\.sh (\d+) s\.\n$", p.stdout)
        self.assertIsNotNone(m, p.stdout)
        total, got, inst = (int(x) for x in m.groups())
        self.assertLessEqual(got + inst, total + 1)

    def test_install_sh_names_each_interval(self):
        text = read(INSTALL)
        self.assertIn('say "  the Root\'s install step took $(( $(date +%s) - T0 )) s; lowest free space $LOWEST MB"', text)
        self.assertNotIn('say "  installed in ', text)
        m = re.search(r"^ELAPSED=.*?^fi$", text, re.S | re.M)
        helpers = functions(INSTALL, "shq", "rh_env", "rcmd", "tilde", "agent", "path_add")
        p = subprocess.run(["/bin/bash", "-c", helpers + "\n" + m.group(0)], stdin=subprocess.DEVNULL, capture_output=True, text=True,
                           timeout=30, env={"PATH": "/usr/bin:/bin", "LANG": "C", "VERSION": "9.9.9", "T_START": "0", "REPO": "/r",
                                            "IH": "/ih", "ACCESS_EFF": "full", "SHELL": "/bin/zsh", "OS": "Darwin", "HOME": "/h",
                                            "PW_HOME": "/h", "RH": "/h/.init-root", "BIN_DIR": "/h/.local/bin"})
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertRegex(p.stdout, r"\nINIT v9\.9\.9 is installed \(install\.sh took \d+ min \d+ s\)\.\n")
        # v0.5.4 (round 4 triage): spending is pointed at /usage, not at a command that prints raw µUSD JSON
        self.assertIn("Spending is tracked, not capped: /usage in the chat shows it", p.stdout)
        self.assertNotIn("egress stats;", p.stdout)
        self.assertIn("Uninstall:   /bin/bash -p /r/scripts/uninstall.sh   (moves both homes to the Trash; the README's first\n"
                      "             screen has a line that downloads it, for when this copy is gone: github.com/nagisanzenin/INIT-Agent)\n",
                      p.stdout)
        # v0.5.4 (review 6): the README's line finds a custom ROOT_HOME only with INIT_ROOT_HOME= in front; it said
        # "works from anywhere"
        self.assertNotIn("works from anywhere", p.stdout)
        p = subprocess.run(["/bin/bash", "-c", helpers + "\n" + m.group(0)], stdin=subprocess.DEVNULL, capture_output=True, text=True,
                           timeout=30, env={"PATH": "/usr/bin:/bin", "LANG": "C", "VERSION": "9.9.9", "T_START": "0", "REPO": "/r",
                                            "IH": "/ih", "ACCESS_EFF": "full", "SHELL": "/bin/zsh", "OS": "Darwin", "HOME": "/h",
                                            "PW_HOME": "/h", "RH": "/h/my init", "BIN_DIR": "/h/.local/bin"})
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn("Uninstall:   INIT_ROOT_HOME='/h/my init' /bin/bash -p /r/scripts/uninstall.sh   (moves both homes to the Trash; "
                      "the README's first\n             screen has a line that downloads it, for when this copy is gone: "
                      "github.com/nagisanzenin/INIT-Agent; put INIT_ROOT_HOME='/h/my init' in front of it)\n", p.stdout)

    def test_the_uninstall_line_runs_under_bash_p(self):
        # v0.5.4 (review 6): macOS /bin/sh is bash 3.2 as sh, which honours SHELLOPTS=xtrace with a PS4 that runs commands;
        # one swapped "$f" between the download and its run. bash -p ignores SHELLOPTS (and BASH_ENV, exported functions)
        line = re.search(r"^To uninstall \(everything goes to the Trash\): `([^`]*)`", read(README), re.M).group(1)
        self.assertTrue(line.startswith("/bin/bash -p -c 'f=\"$(/usr/bin/mktemp)\" && /usr/bin/curl -q -fsSL "), line)
        self.assertRegex(line, r"^/bin/bash -p -c '[^'\\]*'$")  # one single-quoted word: fish reads it the same
        self.assertTrue(line.endswith(' && /bin/bash -p "$f"\''), line)

    def test_the_readme_uninstall_line_is_the_one_get_sh_prints(self):
        line = re.search(r"^To uninstall \(everything goes to the Trash\): `([^`]*)`", read(README), re.M)
        self.assertIsNotNone(line, "README has no uninstall line")
        src = functions(GET, "shq", "rh_env") + "\nRH=\"$HOME/.init-root\"\n" + next(
            l for l in read(GET).splitlines() if l.startswith("UNINSTALL=")) + '\nprintf "%s" "$UNINSTALL"'
        pw = subprocess.run(["/usr/bin/python3", "-I", "-c", "import os,pwd;print(pwd.getpwuid(os.getuid()).pw_dir)"],
                            capture_output=True, text=True).stdout.strip()
        p = subprocess.run(["/bin/sh", "-c", src], env={"PATH": "/usr/bin:/bin", "HOME": pw}, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=30)  # reads nothing in that home: the default home's line
        self.assertEqual(line.group(1), p.stdout)
        # v0.5.4 (review 6): with a custom ROOT_HOME the line fails closed ("nothing to uninstall"); README says what to add
        self.assertTrue(line.string[line.end():].startswith(" (installed with a custom `INIT_ROOT_HOME`: put "
                                                             "`INIT_ROOT_HOME=<that directory>` in front)"), line.string[line.end():][:120])


if __name__ == "__main__":
    unittest.main()
