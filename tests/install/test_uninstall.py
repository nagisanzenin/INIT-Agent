"""scripts/uninstall.sh finds INIT's launchers where the install put them (v0.5.2; the post-release review of v0.5.1).

install.sh records the directory of the `init-agent` command in ROOT_HOME/install.json ("bin_dir"; INIT_BIN_DIR at
install time, else ~/.local/bin). uninstall.sh read only INIT_BIN_DIR and ~/.local/bin, so after an install with a
custom INIT_BIN_DIR, uninstalling without it left init-agent and init-tui behind, pointing at a ROOT_HOME in the Trash.
The case: a scratch HOME with an install whose record names its own bin directory, and a launcher of another install in
~/.local/bin that must stay. uninstall.sh runs with only INIT_ROOT_HOME set and `uninstall` on stdin; it must move
ROOT_HOME, INIT_HOME, init-agent and init-tui from the recorded directory to the scratch Trash and nothing else. No
Root runs (no run/root.lock), and the scratch ROOT_HOME is named by no service file, so nothing is stopped or unloaded.
Stdlib only.
"""
import fcntl
import json
import os
import platform
import socket
import subprocess
import tempfile
import unittest

from test_installed_elsewhere import put_launcher

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
UNINSTALL = os.path.join(REPO, "scripts", "uninstall.sh")


class RecordedBinDir(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.mkdtemp(prefix="i5-rp52-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", self.t], check=False)

    def test_the_launchers_in_the_recorded_bin_dir_go_to_the_trash(self):
        home = os.path.join(self.t, "home")
        rh, ih, bin_dir = os.path.join(home, "rh"), os.path.join(home, "ih"), os.path.join(home, "my bin")
        os.makedirs(os.path.join(rh, "keys"))
        os.makedirs(ih)
        with open(os.path.join(rh, "install.json"), "w") as f:
            json.dump({"init_home": ih, "bin_dir": bin_dir, "root_home": rh, "v": 1}, f)
        put_launcher(bin_dir, rh)
        with open(os.path.join(bin_dir, "init-tui"), "w") as f:
            f.write("tui\n")
        decoy = put_launcher(os.path.join(home, ".local", "bin"), os.path.join(home, "another-rh"))  # another install's
        p = subprocess.run(["/bin/bash", UNINSTALL], input="uninstall\n", capture_output=True, text=True, timeout=120,
                           env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": home, "INIT_ROOT_HOME": rh, "LANG": "C"})
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        trash = os.path.join(home, ".Trash") if platform.system() == "Darwin" else os.path.join(home, ".local", "share", "Trash", "files")
        moved = sorted(n.rsplit(" ", 2)[0] for n in os.listdir(trash))
        self.assertEqual(moved, ["ih", "init-agent", "init-tui", "rh"], p.stdout)
        for gone in (rh, ih, os.path.join(bin_dir, "init-agent"), os.path.join(bin_dir, "init-tui")):
            self.assertFalse(os.path.exists(gone), gone)
        self.assertTrue(os.path.isfile(decoy), "the launcher of another install in ~/.local/bin was moved")
        self.assertIn("  %s/init-agent\n" % bin_dir, p.stdout)


def trash_of(home):
    return os.path.join(home, ".Trash") if platform.system() == "Darwin" else os.path.join(home, ".local", "share", "Trash", "files")


class UnpackedRelease(unittest.TestCase):
    """v0.5.4 (the v0.5.3 user sessions): the release get.sh unpacked, ~/.local/share/init/<version>/, goes to the Trash
    with the install (as "init-<version>"), read-only as get.sh leaves it; anything else in that folder stays."""

    def test_the_unpacked_release_goes_to_the_trash_and_nothing_else_there(self):
        t = tempfile.mkdtemp(prefix="i5-u54-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/sh", "-c", 'chmod -R u+w "$1"; rm -rf -- "$1"', "sh", t], check=False)
        home = os.path.join(t, "home")
        rh, ih = os.path.join(home, ".init-root"), os.path.join(home, ".init")
        os.makedirs(ih)
        os.makedirs(os.path.join(rh, "keys"))
        with open(os.path.join(rh, "install.json"), "w") as f:
            json.dump({"init_home": ih, "bin_dir": os.path.join(home, ".local", "bin"), "v": 1}, f)
        app = os.path.join(home, ".local", "share", "init")
        rel = os.path.join(app, "0.5.3-preview")
        os.makedirs(os.path.join(rel, "scripts"))
        for n, text in (("VERSION", "0.5.3-preview"), ("SHA256SUMS", ""), ("MANIFEST.json", "{}\n")):
            with open(os.path.join(rel, n), "w") as f:
                f.write(text)
        other = os.path.join(app, "notes")  # not a release: stays
        os.makedirs(other)
        subprocess.run(["/bin/chmod", "-R", "a-w", rel], check=True)  # as get.sh leaves it
        p = subprocess.run(["/bin/bash", UNINSTALL], input="uninstall\n", capture_output=True, text=True, timeout=120,
                           env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": home, "LANG": "C"})
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("  %s\n" % rel, p.stdout, "the release is in the list of what moves")
        self.assertFalse(os.path.exists(rel), p.stdout)
        self.assertTrue(os.path.isdir(other), "a folder that is not a release was moved")
        moved = sorted(n.rsplit(" ", 2)[0] for n in os.listdir(trash_of(home)))
        self.assertEqual(moved, [".init", ".init-root", "init-0.5.3-preview"], p.stdout)


class PanicLine(unittest.TestCase):
    """v0.5.4 (the post-release review of v0.5.3): when the zone does not stop, the `panic` line names this install's
    init-root and its home. init-root finds its home in INIT_ROOT_HOME, so a bare `<home>/bin/init-root panic` on a custom
    home panicked the main install. (About 60 s: the uninstaller waits that long for the zone.)"""

    def test_the_panic_line_names_this_home(self):
        t = tempfile.mkdtemp(prefix="i5p", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", t], check=False)
        home, rh = os.path.join(t, "h"), os.path.join(t, "my rh")
        os.makedirs(os.path.join(rh, "bin"))
        os.makedirs(os.path.join(rh, "run"))
        os.makedirs(home)
        root = os.path.join(rh, "bin", "init-root")
        with open(root, "w") as f:  # a zone that never stops
            f.write('#!/bin/sh\ncase "$*" in *status*) echo \'{"slot":"running","instances":[{}]}\' ;; esac\nexit 0\n')
        os.chmod(root, 0o755)
        sock = socket.socket(socket.AF_UNIX)
        sock.bind(os.path.join(rh, "run", "owner.sock"))
        self.addCleanup(sock.close)
        lock = open(os.path.join(rh, "run", "root.lock"), "w")
        fcntl.flock(lock, fcntl.LOCK_EX)  # held by this process, as a running Root holds it
        self.addCleanup(lock.close)
        p = subprocess.run(["/bin/bash", UNINSTALL], input="", capture_output=True, text=True, timeout=180,
                           env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": home, "INIT_ROOT_HOME": rh, "LANG": "C"})
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        line = "INIT_ROOT_HOME='%s' '%s' panic" % (rh, root)
        self.assertIn("try: %s\n" % line, p.stderr)
        self.assertTrue(os.path.isdir(rh), "nothing moves")


if __name__ == "__main__":
    unittest.main()
