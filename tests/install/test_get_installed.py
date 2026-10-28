"""scripts/get.sh on a machine where INIT is already installed: it names the installed release and the way forward.

Before this, get.sh told an owner of INIT 0.3.1 to run `init-agent upgrade` or `get.sh --upgrade`; 0.3.1 has no
`init-agent` and the upgrade starts at 0.4.1 (build/status/REL-050.md, "After the release"). Each case makes a fake
ROOT_HOME with install.json and a stand-in bin/init-root that prints "init-root <version>" (or none at all), runs
get.sh with and without --upgrade, and checks the message: before 0.4.1 the uninstall command (and --upgrade refuses
before any download), 0.4.1 the --upgrade command, 0.5.0 or later `init-agent upgrade`. get.sh must stop before it
fetches anything: `curl` on PATH is a stand-in that fails the test if it runs. Needs a host get.sh accepts (macOS 14+
on arm64, or Linux with /usr/bin/bwrap and /usr/bin/python3); anything else skips. Stdlib only.
"""
import os
import platform
import subprocess
import tempfile
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GET = os.path.join(REPO, "scripts", "get.sh")
UNINSTALL = "https://raw.githubusercontent.com/nagisanzenin/INIT-Agent/main/scripts/uninstall.sh"


def host_ok():
    if platform.system() == "Darwin" and platform.machine() == "arm64":
        return int(platform.mac_ver()[0].split(".")[0] or 0) >= 14 and os.access("/usr/bin/python3", os.X_OK)
    if platform.system() == "Linux":
        return os.access("/usr/bin/bwrap", os.X_OK) and os.access("/usr/bin/python3", os.X_OK) \
            and platform.machine() in ("x86_64", "amd64", "aarch64", "arm64")
    return False


@unittest.skipUnless(host_ok(), "get.sh does not run on this host")
class InstalledRelease(unittest.TestCase):
    def run_get(self, version, *args):
        with tempfile.TemporaryDirectory() as home:
            rh = os.path.join(home, ".init-root")
            os.makedirs(os.path.join(rh, "bin"))
            os.makedirs(os.path.join(rh, "cache"))
            with open(os.path.join(rh, "install.json"), "w") as f:
                f.write("{}\n")
            if version is not None:
                with open(os.path.join(rh, "bin", "init-root"), "w") as f:
                    f.write('#!/bin/sh\n[ "$1" = --version ] && echo "init-root %s"\n' % version)
                os.chmod(os.path.join(rh, "bin", "init-root"), 0o755)
            fake = os.path.join(home, "fakebin")
            os.makedirs(fake)
            with open(os.path.join(fake, "curl"), "w") as f:
                f.write('#!/bin/sh\necho "curl ran: $*" >&2\nexit 99\n')
            os.chmod(os.path.join(fake, "curl"), 0o755)
            env = {"HOME": home, "PATH": fake + ":/usr/bin:/bin:/usr/sbin:/sbin", "LANG": "C"}
            p = subprocess.run(["/bin/sh", GET, *args], env=env, capture_output=True, text=True, timeout=60)
            self.assertFalse(os.path.exists(os.path.join(rh, "cache", "upgrade")) and version in ("0.3.1", "0.4.0"),
                             "get.sh made cache/upgrade for a release it refuses")
            return p

    def test_before_041_prints_the_uninstall(self):
        for version in ("0.3.1", "0.4.0"):
            for args in ((), ("--upgrade",)):
                p = self.run_get(version, *args)
                self.assertEqual(p.returncode, 1, (version, args, p.stderr))
                self.assertIn("INIT %s is installed" % version, p.stderr)
                self.assertIn("before 0.4.1 is not supported", p.stderr)
                self.assertIn(UNINSTALL, p.stderr)
                self.assertNotIn("init-agent upgrade", p.stderr)
                self.assertNotIn("curl ran", p.stderr)

    def test_041_gets_the_upgrade_command(self):
        p = self.run_get("0.4.1")
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("INIT 0.4.1 is already installed", p.stderr)
        self.assertIn("get.sh | sh -s -- --upgrade", p.stderr)
        self.assertNotIn("curl ran", p.stderr)

    def test_050_and_later_get_init_agent_upgrade(self):
        for version in ("0.5.0", "0.6.2", "1.0.0"):
            p = self.run_get(version)
            self.assertEqual(p.returncode, 1, (version, p.stderr))
            self.assertIn("INIT %s is already installed" % version, p.stderr)
            self.assertIn("\n  init-agent upgrade\n", p.stderr)
            self.assertNotIn("curl ran", p.stderr)

    def test_unreadable_version_lists_every_path(self):
        p = self.run_get(None)
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("its version could not be read", p.stderr)
        for s in ("init-agent upgrade", "--upgrade", UNINSTALL):
            self.assertIn(s, p.stderr)

    def test_upgrade_from_041_goes_on_to_the_download(self):
        # --upgrade on 0.4.1 is the supported path: get.sh passes the guard and reaches its download (the stand-in curl)
        p = self.run_get("0.4.1", "--upgrade")
        self.assertNotEqual(p.returncode, 0)
        self.assertNotIn("not supported", p.stderr)
        self.assertIn("curl ran", p.stderr)


if __name__ == "__main__":
    unittest.main()
