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
import json
import os
import platform
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


if __name__ == "__main__":
    unittest.main()
