"""The upgrade's entry points refuse the wrong situation before they write anything (lane UPGRADE, v0.5.0).

- get.sh --upgrade with no install, get.sh without --upgrade over an install, install.sh --upgrade with no install and
  install.sh without --upgrade over an install: each exits 1 with a message that names the right command, and leaves
  the scratch directory as it was (get.sh checks this before it downloads or unpacks anything).
- `init-agent upgrade` runs the get.sh kept in ROOT_HOME/cache/ with --upgrade and its own flags mapped (--bundle is
  get.sh's --from); with no kept copy (an install of 0.4.1) it exits 69 and prints the one-line upgrade that runs the
  current get.sh with the system's curl and sh.
- install.sh over 0.4.1 or 0.5.0 names its own `install.sh --upgrade` and that one-line upgrade, never
  `init-agent upgrade` (0.5.0's runs 0.5.0's get.sh); over 0.5.1 or later, `init-agent upgrade` (v0.5.3: with its
  path). Each line carries this
  ROOT_HOME and quotes the paths (the repository's own path has a space).
- install.sh --upgrade on Linux under `env -i HOME=… init-agent upgrade` (v0.5.3, the release review): systemctl
  --user reaches the user's systemd without XDG_RUNTIME_DIR, and a stop or start the service refuses is reported with
  what it said and the state INIT is in (a stand-in systemctl and Root).

Each run gets a scratch HOME, ROOT_HOME and INIT_HOME. Stdlib only; ci/local.sh runs it with the other tests here
(`python3 -m unittest discover -s tests/install`). The upgrade itself runs end to end in scripts/upgrade-e2e.sh.
"""
import os
import platform
import re
import shutil
import subprocess
import tempfile
import time
import unittest

from test_printed_commands import functions, shq

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SCRIPTS = os.path.join(REPO, "scripts")
INSTALL = os.path.join(SCRIPTS, "install.sh")
# get.sh's platform checks come first: macOS on arm64, or Linux with bubblewrap
GET_RUNS_HERE = (platform.system() == "Darwin" and platform.machine() == "arm64") or os.path.exists("/usr/bin/bwrap")


def tree(d):
    """Names and sizes under d, without Library/ (macOS's python3 shim caches there when get.sh checks for python3)."""
    out = []
    for root, dirs, files in os.walk(d):
        if root == d:
            dirs[:] = [x for x in dirs if x != "Library"]
        for n in sorted(dirs + files):
            p = os.path.join(root, n)
            if os.path.relpath(p, d) != "Library":
                out.append((os.path.relpath(p, d), os.path.getsize(p) if os.path.isfile(p) else -1))
    return sorted(out)


class UpgradeGuards(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="iu-", dir="/tmp")  # short: homes are at most 64 bytes
        self.addCleanup(lambda: shutil.rmtree(self.tmp, ignore_errors=True))
        self.rh, self.ih = os.path.join(self.tmp, "r"), os.path.join(self.tmp, "i")
        self.env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": self.tmp, "INIT_ROOT_HOME": self.rh, "INIT_HOME": self.ih,
                    "INIT_APP_DIR": os.path.join(self.tmp, "app"), "INIT_BIN_DIR": os.path.join(self.tmp, "bin"),
                    "INIT_DIST_URL": "https://releases.invalid/init"}

    def installed(self):
        os.makedirs(os.path.join(self.rh, "cache"))
        with open(os.path.join(self.rh, "install.json"), "w") as f:
            f.write('{"init_home":"%s","v":1}\n' % self.ih)

    def run_script(self, argv):
        before = tree(self.tmp)
        p = subprocess.run(argv, env=self.env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)
        self.assertEqual(tree(self.tmp), before, f"{argv}: wrote something\n{p.stdout}\n{p.stderr}")
        return p

    @unittest.skipUnless(GET_RUNS_HERE, "get.sh refuses this platform before its upgrade checks")
    def test_get_upgrade_needs_an_install(self):
        p = self.run_script(["/bin/sh", os.path.join(SCRIPTS, "get.sh"), "--upgrade"])
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("is not installed", p.stderr)
        self.assertIn("without --upgrade", p.stderr)

    @unittest.skipUnless(GET_RUNS_HERE, "get.sh refuses this platform before its upgrade checks")
    def test_get_over_an_install_points_to_the_upgrade(self):
        self.installed()
        p = self.run_script(["/bin/sh", os.path.join(SCRIPTS, "get.sh")])
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("already installed", p.stderr)
        self.assertIn("init-agent upgrade", p.stderr)
        self.assertIn("--upgrade", p.stderr)

    def test_get_version_is_a_plain_version(self):
        if not GET_RUNS_HERE:
            self.skipTest("get.sh refuses this platform first")
        self.installed()
        p = self.run_script(["/bin/sh", os.path.join(SCRIPTS, "get.sh"), "--upgrade", "--version", "0.5.0;rm"])
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("--version takes a version", p.stderr)

    def test_install_upgrade_needs_an_install(self):
        p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--upgrade", "--yes"])
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("INIT is not installed", p.stderr)

    def test_install_over_an_install_points_to_the_upgrade(self):
        self.installed()
        p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--yes", "--no-service"])
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("already installed", p.stderr)
        self.assertIn("init-agent upgrade", p.stderr)

    def fake_root(self, version):
        os.makedirs(os.path.join(self.rh, "bin"), exist_ok=True)
        p = os.path.join(self.rh, "bin", "init-root")
        with open(p, "w") as f:
            f.write('#!/bin/sh\n[ "$1" = --version ] && echo "init-root %s"\n' % version)
        os.chmod(p, 0o755)

    def test_install_over_a_pre_041_install_prints_the_uninstall(self):
        # the sibling of get.sh's message (build/status/REL-050.md, "After the release"): 0.3.1 has no init-agent
        self.installed()
        self.fake_root("0.3.1")
        for extra in ([], ["--upgrade"]):
            p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--yes", "--no-service", *extra])
            self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
            self.assertIn("INIT 0.3.1 is installed", p.stderr)
            self.assertIn("before 0.4.1 is not supported", p.stderr)
            self.assertIn("\n  INIT_ROOT_HOME=%s bash %s\n" % (shq(self.rh), shq(os.path.join(SCRIPTS, "uninstall.sh"))), p.stderr)
            self.assertNotIn("init-agent upgrade", p.stderr)

    def test_install_over_041_or_050_points_to_install_sh_upgrade_and_the_safe_line(self):
        self.installed()
        rh = "INIT_ROOT_HOME=%s " % shq(self.rh)
        for version in ("0.4.1", "0.5.0"):
            self.fake_root(version)
            p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--yes", "--no-service"])
            self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
            self.assertIn("INIT %s is already installed" % version, p.stderr)
            self.assertIn("\n  %s%s --upgrade\n" % (rh, shq(os.path.join(SCRIPTS, "install.sh"))), p.stderr)
            self.assertIn("once from your own shell: /usr/bin/curl -q -fsSL https://releases.invalid/init/get.sh | %s/bin/sh -s --"
                          " --upgrade\n" % rh, p.stderr)
            self.assertNotIn("\n  init-agent upgrade", p.stderr)
            self.assertNotIn("\n  %sinit-agent upgrade" % rh, p.stderr)
        self.fake_root("0.5.1")
        p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "install.sh"), "--yes", "--no-service"])
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        # v0.5.3: with its path (INIT_BIN_DIR is HOME/bin here; ~/.local/bin is not on a fresh Mac's PATH)
        self.assertIn("\n  %s~/bin/init-agent upgrade        (or: %s%s --upgrade)\n" % (rh, rh, shq(os.path.join(SCRIPTS, "install.sh"))), p.stderr)
        self.assertNotIn("/usr/bin/curl", p.stderr)

    def test_launcher_without_a_kept_get_sh(self):
        self.installed()
        p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "init-agent"), "upgrade"])
        self.assertEqual(p.returncode, 69, p.stderr)
        self.assertIn("/usr/bin/curl -q -fsSL https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh |"
                      " INIT_ROOT_HOME=%s /bin/sh -s -- --upgrade\n" % shq(self.rh), p.stderr)

    def test_launcher_runs_the_kept_get_sh_with_its_flags(self):
        cache = os.path.join(self.rh, "cache")
        os.makedirs(cache)
        out = os.path.join(self.tmp, "argv.txt")
        with open(os.path.join(cache, "get.sh"), "w") as f:
            f.write('for a in "$@"; do printf "%%s\\n" "$a"; done > "%s"\n' % out)
        p = subprocess.run(["/bin/bash", os.path.join(SCRIPTS, "init-agent"), "upgrade", "--bundle", "b.tar.gz", "--sha256", "ab",
                            "--version", "0.5.0", "--", "--probe-timeout", "30"], env=self.env, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        with open(out) as f:
            self.assertEqual(f.read().split("\n")[:-1], ["--upgrade", "--from", "b.tar.gz", "--sha256", "ab", "--version", "0.5.0",
                                                         "--", "--probe-timeout", "30"])
        p = self.run_script(["/bin/bash", os.path.join(SCRIPTS, "init-agent"), "upgrade", "--nope"])
        self.assertEqual(p.returncode, 64, p.stderr)


# A stand-in systemctl (Linux): like systemd's, `--user` reaches no manager without XDG_RUNTIME_DIR or
# DBUS_SESSION_BUS_ADDRESS; with them, `stop` ends the stand-in Root (the lock holder in $PIDF) and `start` starts one
# (the lock and owner.sock). $FAIL makes it refuse the way systemd does.
FAKE_SYSTEMCTL = r"""#!/bin/sh
echo "$* XDG=${XDG_RUNTIME_DIR-unset}" >>"$SVC_LOG"
if [ -z "${XDG_RUNTIME_DIR:-}" ] && [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
  echo 'Failed to connect to bus: $DBUS_SESSION_BUS_ADDRESS and $XDG_RUNTIME_DIR not defined (consider using --machine=<user>@.host --user to connect to bus of other user)' >&2
  exit 1
fi
[ -z "${FAIL:-}" ] || { echo "$FAIL" >&2; exit 1; }
case "$2" in
  stop) kill "$(cat "$PIDF")" ;;
  start) /usr/bin/python3 -I -c "$HOLD" "$RH/run/root.lock" "$PIDF" "$RH/run/owner.sock" </dev/null >/dev/null 2>&1 &
         while [ ! -S "$RH/run/owner.sock" ]; do sleep 0.1; done ;;
esac
"""
# a stand-in Root: holds run/root.lock (and, when given, listens on owner.sock) until it is killed
HOLD = """import fcntl, os, socket, sys, time
fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT, 0o600)
fcntl.flock(fd, fcntl.LOCK_EX)
if len(sys.argv) > 3:
    if os.path.exists(sys.argv[3]):
        os.remove(sys.argv[3])  # a stale one, as a Root finds it after a stop
    s = socket.socket(socket.AF_UNIX)
    s.bind(sys.argv[3])
open(sys.argv[2] + ".new", "w").write(str(os.getpid()))
os.rename(sys.argv[2] + ".new", sys.argv[2])
time.sleep(300)
"""


class UpgradeServiceUnderEnvI(unittest.TestCase):
    """install.sh --upgrade's stop and start of the systemd user unit (v0.5.3, the release review). The release notes
    give `env -i HOME="$HOME" ~/.local/bin/init-agent upgrade`, which clears XDG_RUNTIME_DIR and
    DBUS_SESSION_BUS_ADDRESS: every `systemctl --user` failed, silently, and the upgrade stopped the agent, then said
    "the Root did not stop within 60 s". install.sh now defaults XDG_RUNTIME_DIR on Linux, and a service command that
    fails is reported with what it said and the state INIT is left in. install.sh's functions and that line run alone
    (OS=Linux, on any host), with a stand-in systemctl, Root and lock."""

    FNS = ("say", "die", "shq", "rh_env", "rcmd", "lock_free", "svc", "svc_cmd", "svc_err", "svc_start", "stop_root")

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="iuv-", dir="/tmp")  # short: owner.sock must fit sun_path
        self.addCleanup(lambda: shutil.rmtree(self.tmp, ignore_errors=True))
        self.rh = os.path.join(self.tmp, "r")
        for d in ("run", "bin"):
            os.makedirs(os.path.join(self.rh, d))
        os.makedirs(os.path.join(self.tmp, "fake"))
        os.makedirs(os.path.join(self.tmp, "w"))
        self.log, self.pidf = os.path.join(self.tmp, "svc.log"), os.path.join(self.rh, "run", "root.pid")
        for name, text in (("fake/systemctl", FAKE_SYSTEMCTL), ("r/bin/init-root", "#!/bin/sh\nexit 0\n")):
            with open(os.path.join(self.tmp, name), "w") as f:
                f.write(text)
            os.chmod(os.path.join(self.tmp, name), 0o755)
        self.addCleanup(self.kill_root)
        p = subprocess.Popen(["/usr/bin/python3", "-I", "-c", HOLD, os.path.join(self.rh, "run", "root.lock"), self.pidf],
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(p.wait)
        while not os.path.exists(self.pidf):
            time.sleep(0.05)
        with open(INSTALL) as f:
            self.xdg = re.search(r'^if \[ "\$OS" = Linux \]; then XDG_RUNTIME_DIR=.*\n', f.read(), re.M).group(0)

    def kill_root(self):
        try:
            with open(self.pidf) as f:
                os.kill(int(f.read()), 9)
        except (OSError, ValueError):
            pass

    def run_fns(self, call, **env):
        src = "set -euo pipefail\n" + self.xdg + functions(INSTALL, *self.FNS) + '\nIENV=(env -i HOME="$HOME" PATH=/usr/bin:/bin)\n' + call
        # as under `env -i HOME=… init-agent upgrade`: no XDG_RUNTIME_DIR, no DBUS_SESSION_BUS_ADDRESS
        base = {"PATH": "%s:/usr/bin:/bin" % os.path.join(self.tmp, "fake"), "HOME": self.tmp, "LANG": "C", "OS": "Linux", "SVC": "1",
                "RH": self.rh, "PW_HOME": self.tmp, "WORK": os.path.join(self.tmp, "w"), "ROOT_BIN": os.path.join(self.rh, "bin", "init-root"),
                "SVC_LOG": self.log, "PIDF": self.pidf, "HOLD": HOLD}
        base.update(env)
        t = time.monotonic()
        p = subprocess.run(["/bin/bash", "-c", src], env=base, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=90)
        return p, time.monotonic() - t

    def calls(self):
        with open(self.log) as f:
            return f.read().splitlines()

    def test_the_stop_and_the_start_reach_systemd_without_xdg_runtime_dir(self):
        p, _ = self.run_fns("stop_root; echo stopped=$(lock_free && echo yes); svc_start")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("  stopped (the service)\nstopped=yes\n  the Root runs again under the service\n", p.stdout)
        xdg = "/run/user/%d" % os.getuid()
        self.assertEqual(self.calls(), ["--user stop init-root.service XDG=%s" % xdg, "--user start init-root.service XDG=%s" % xdg])
        # an XDG_RUNTIME_DIR that is set is kept
        os.remove(self.log)
        self.kill_root()
        p, _ = self.run_fns("svc_start", XDG_RUNTIME_DIR="/run/user/elsewhere")
        self.assertEqual((p.returncode, self.calls()), (0, ["--user start init-root.service XDG=/run/user/elsewhere"]), p.stderr)
        self.assertEqual(p.stdout, "  the Root runs again under the service\n")

    def test_a_stop_the_service_refuses_is_reported_with_the_state_inits_in(self):
        why = "Failed to stop init-root.service: Access denied"
        p, took = self.run_fns("stop_root", FAIL=why)
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertLess(took, 30, "a refused stop is not waited out for 60 s")
        with open(self.pidf) as f:
            pid = f.read()
        q = "INIT_ROOT_HOME=%s %s/bin/init-root" % (shq(self.rh), self.rh)
        self.assertIn("install.sh: the Root did not stop within 10 s (systemctl --user stop init-root.service failed: %s). Nothing was "
                      "changed: the Root (pid %s) still runs the version you had, with its agent stopped by the upgrade (start it again:"
                      " %s start). To upgrade, stop the Root yourself (systemctl --user stop init-root.service), then run the upgrade"
                      " again.\n" % (why, pid, q), p.stderr)
        self.kill_root()
        p, _ = self.run_fns("svc_start", FAIL="Failed to start init-root.service: Unit init-root.service not found.")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("  warning: systemctl --user start init-root.service failed: Failed to start init-root.service: Unit "
                      "init-root.service not found.\n  The Root is not running. Start it: systemctl --user start init-root.service"
                      "   (or, in a terminal of its own: %s start --foreground)\n" % q, p.stdout)


if __name__ == "__main__":
    unittest.main()
