"""scripts/get.sh on a machine where INIT is already installed: it names the installed release and the way forward.

Before this, get.sh told an owner of INIT 0.3.1 to run `init-agent upgrade` or `get.sh --upgrade`; 0.3.1 has no
`init-agent` and the upgrade starts at 0.4.1 (build/status/REL-050.md, "After the release"). Each case makes a fake
ROOT_HOME with install.json and a stand-in bin/init-root that prints "init-root <version>" (or none at all), runs
get.sh with and without --upgrade, and checks the message: before 0.4.1 the uninstall command (and --upgrade refuses
before any download); 0.4.1 and 0.5.0 the upgrade that runs the current get.sh with the system's curl and sh (0.5.0's
own `init-agent upgrade` runs 0.5.0's get.sh, which 0.5.1 fixed; v0.5.1, lane SCRIPTS-FIX2); 0.5.1 or later
`init-agent upgrade`. Each line carries INIT_ROOT_HOME= (on the sh side of a pipe) when ROOT_HOME is not the passwd
home's ~/.init-root, which a scratch HOME never is, and runs as printed (run_pipe). get.sh must stop before it
fetches anything: `curl` on PATH is a stand-in that fails the test if it runs. (An upgrade runs /usr/bin/curl, never
one on PATH: the case that passes the guard fetches from https://127.0.0.1:1/, which refuses at once.) Needs a host get.sh accepts (macOS 14+
on arm64, or Linux with /usr/bin/bwrap and /usr/bin/python3); anything else skips. Stdlib only.
"""
import os
import platform
import subprocess
import tempfile
import unittest

from test_printed_commands import run_pipe, shq

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GET = os.path.join(REPO, "scripts", "get.sh")
UNINSTALL = "https://raw.githubusercontent.com/nagisanzenin/INIT-Agent/main/scripts/uninstall.sh"
DIST = "https://127.0.0.1:1/rel/latest/download"


def pipe(rh):
    """The upgrade get.sh prints for 0.4.1 and 0.5.0 (up_pipe)."""
    return "/usr/bin/curl -q -fsSL %s/get.sh | INIT_ROOT_HOME=%s /bin/sh -s -- --upgrade" % (DIST, shq(rh))


def host_ok():
    if platform.system() == "Darwin" and platform.machine() == "arm64":
        return int(platform.mac_ver()[0].split(".")[0] or 0) >= 14 and os.access("/usr/bin/python3", os.X_OK)
    if platform.system() == "Linux":
        return os.access("/usr/bin/bwrap", os.X_OK) and os.access("/usr/bin/python3", os.X_OK) \
            and platform.machine() in ("x86_64", "amd64", "aarch64", "arm64")
    return False


@unittest.skipUnless(host_ok(), "get.sh does not run on this host")
class InstalledRelease(unittest.TestCase):
    def run_get(self, version, *args, rh_name=".init-root"):
        """get.sh over a fake install of `version` in HOME/<rh_name> (INIT_ROOT_HOME names any other); self.rh, self.home."""
        with tempfile.TemporaryDirectory(prefix="i5-sc2-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None) as home:
            rh = os.path.join(home, rh_name)
            self.rh, self.home = rh, home
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
            env = {"HOME": home, "PATH": fake + ":/usr/bin:/bin:/usr/sbin:/sbin", "LANG": "C", "INIT_DIST_URL": DIST}
            if rh_name != ".init-root":
                env["INIT_ROOT_HOME"] = rh
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

    def test_041_and_050_get_the_current_get_sh_with_the_system_curl_and_sh(self):
        for version, why in (("0.4.1", "0.4.1 has no init-agent upgrade"),
                             ("0.5.0", "0.5.0's own init-agent upgrade runs 0.5.0's get.sh, which 0.5.1 fixed")):
            p = self.run_get(version)
            self.assertEqual(p.returncode, 1, p.stderr)
            self.assertIn("INIT %s is already installed" % version, p.stderr)
            self.assertIn("(%s):\n  %s\n" % (why, pipe(self.rh)), p.stderr)
            self.assertNotIn("\n  init-agent upgrade", p.stderr)
            self.assertNotIn("curl ran", p.stderr)

    def test_051_and_later_get_init_agent_upgrade(self):
        for version in ("0.5.1", "0.6.2", "1.0.0"):
            p = self.run_get(version)
            self.assertEqual(p.returncode, 1, (version, p.stderr))
            self.assertIn("INIT %s is already installed" % version, p.stderr)
            self.assertIn("\n  INIT_ROOT_HOME=%s init-agent upgrade\n" % shq(self.rh), p.stderr)
            self.assertNotIn("/usr/bin/curl", p.stderr)
            self.assertNotIn("curl ran", p.stderr)

    def test_a_root_home_elsewhere_is_in_every_line_and_each_runs_as_printed(self):
        name = "my rh's"  # INIT_ROOT_HOME=HOME/my rh's: a space and a quote
        for version in ("0.4.1", "0.5.0"):
            p = self.run_get(version, rh_name=name)
            line = p.stderr.splitlines()[-1].strip()
            self.assertEqual(line, pipe(self.rh), p.stderr)
            self.assertEqual(run_pipe(self, line, DIST + "/get.sh", self.home), "home=%s|args=--upgrade" % self.rh)
        p = self.run_get("0.5.1", rh_name=name)
        line = p.stderr.splitlines()[-1].strip()
        self.assertEqual(line, "INIT_ROOT_HOME=%s init-agent upgrade" % shq(self.rh))
        with tempfile.TemporaryDirectory() as fake:  # a stand-in init-agent on PATH, as the owner's ~/.local/bin has it
            with open(os.path.join(fake, "init-agent"), "w") as f:
                f.write('#!/bin/sh\necho "home=${INIT_ROOT_HOME-unset}|args=$*"\n')
            os.chmod(os.path.join(fake, "init-agent"), 0o755)
            q = subprocess.run(["/bin/sh", "-c", line], env={"PATH": fake + ":/usr/bin:/bin", "HOME": "/nonexistent"},
                               capture_output=True, text=True, timeout=30)
            self.assertEqual(q.stdout.strip(), "home=%s|args=upgrade" % self.rh, line + q.stderr)
        p = self.run_get("0.3.1", rh_name=name)
        self.assertIn(' && INIT_ROOT_HOME=%s bash "$f"' % shq(self.rh), p.stderr, "the uninstaller finds this ROOT_HOME")

    def test_unreadable_version_lists_every_path(self):
        p = self.run_get(None)
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("its version could not be read", p.stderr)
        for s in ("From 0.5.1: INIT_ROOT_HOME=%s init-agent upgrade." % shq(self.rh),
                  "From 0.4.1 or 0.5.0: %s." % pipe(self.rh), UNINSTALL):
            self.assertIn(s, p.stderr)

    def test_upgrade_from_041_goes_on_to_the_download(self):
        # --upgrade on 0.4.1 is the supported path: get.sh passes the guard and reaches its download, with the system
        # curl (an upgrade runs no tool from PATH), which the loopback address refuses
        p = self.run_get("0.4.1", "--upgrade")
        self.assertNotEqual(p.returncode, 0)
        self.assertNotIn("not supported", p.stderr)
        self.assertIn("cannot fetch https://127.0.0.1:1/rel/latest/download/latest.json", p.stderr)
        self.assertNotIn("curl ran", p.stderr)


if __name__ == "__main__":
    unittest.main()
