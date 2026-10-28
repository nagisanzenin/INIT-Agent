"""The installers on a machine where INIT is already installed, but not where this run would install it (v0.5.2).

The post-release review of v0.5.1 found two ways the one-line installer led an existing user astray:
- **INIT in another ROOT_HOME.** An owner who installed with INIT_ROOT_HOME=X (and the service), then pasted the README
  one-liner without it, got a second install in ~/.init-root: install.sh rewrote the one service file (the LaunchAgent
  dev.init.root, or the systemd user unit init-root.service) for the new home and overwrote the init-agent launcher,
  which left the evolved INIT in X behind. Now get.sh and install.sh look for an install elsewhere before a fresh one:
  the ROOT_HOME the service file names (when this run would write the service: not with --no-service, and only when
  HOME is the passwd home) and the one INIT's launchers in INIT_BIN_DIR have baked in. When that home holds an
  install.json they refuse, before anything is downloaded or written, with that home's upgrade line (INIT_ROOT_HOME=X
  filled in, by release as for an install in this ROOT_HOME) and how to make a separate second INIT instead.
- **No bundle for this machine.** A Linux aarch64 user on 0.4.1 (no 0.5.x aarch64 bundle) was told to upgrade with a
  command that then stopped on a missing bundle. Now get.sh reads the latest release's latest.json before it advises
  an upgrade; when it lists no bundle for this target, the message says so, that the installed INIT keeps working, and
  to upgrade when a bundle for this target is released. An upgrade that finds no bundle says nothing was changed.

The cases run get.sh against the stand-in curl of test_get_download.py (https://dist.invalid/…), install.sh from this
checkout up to its refusal, and the scripts' own functions alone with a fake passwd home for the service files (a test
cannot be the passwd home: the owner's real LaunchAgent lives there and is never read for this). Each case has its own
scratch directory under /private/tmp (or the system temp dir) and leaves nothing behind. Stdlib only.
"""
import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest

from test_get_download import FAKE_CURL, FAKE_UNAME, host_target
from test_printed_commands import functions, run_pipe, shq

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GET = os.path.join(REPO, "scripts", "get.sh")
INSTALL = os.path.join(REPO, "scripts", "install.sh")
LAUNCHER = os.path.join(REPO, "scripts", "init-agent")
DIST = "https://dist.invalid/rel"
TARGET, FAKE_X86 = host_target()
TNAME = {"macos-arm64": "macOS 14+ on Apple silicon (macos-arm64)", "linux-arm64": "Linux aarch64 (linux-arm64)",
         "linux-x86_64": "Linux x86_64 (linux-x86_64)"}
TMP_BASE = "/private/tmp" if os.path.isdir("/private/tmp") else None


def scratch(test):
    d = tempfile.mkdtemp(prefix="i5-rp52-", dir=TMP_BASE)
    test.addCleanup(lambda: subprocess.run(["/bin/sh", "-c", 'chmod -R u+w "$0" 2>/dev/null; rm -rf -- "$0"', d], check=False))
    return d


def tree(d):
    """The names under d, without a Library/ directory (macOS's python3 shim may cache there under a scratch HOME)."""
    out = []
    for root, dirs, files in os.walk(d):
        dirs[:] = [x for x in dirs if x != "Library"]
        for n in sorted(dirs + files):
            if n != "Library":
                out.append(os.path.relpath(os.path.join(root, n), d))
    return sorted(out)


def fake_install(rh, version):
    """An install.json and a stand-in bin/init-root that prints `init-root <version>`."""
    os.makedirs(os.path.join(rh, "bin"))
    os.makedirs(os.path.join(rh, "cache"))
    with open(os.path.join(rh, "install.json"), "w") as f:
        f.write('{"init_home":"%s","v":1}\n' % os.path.join(os.path.dirname(rh), "ih"))
    exe = os.path.join(rh, "bin", "init-root")
    with open(exe, "w") as f:
        f.write('#!/bin/sh\n[ "$1" = --version ] && echo "init-root %s"\n' % version)
    os.chmod(exe, 0o755)


def put_launcher(bin_dir, rh):
    """INIT's launcher with ROOT_HOME baked in, as install.sh's `launchers` writes it."""
    os.makedirs(bin_dir, exist_ok=True)
    with open(LAUNCHER) as f:
        text = f.read()
    text = "\n".join('INIT_ROOT_HOME_DEFAULT="%s"' % rh if l.startswith("INIT_ROOT_HOME_DEFAULT=") else l
                     for l in text.split("\n"))
    p = os.path.join(bin_dir, "init-agent")
    with open(p, "w") as f:
        f.write(text)
    os.chmod(p, 0o755)
    return p


def plist(rh):
    """The LaunchAgent as init-root install writes it (root/crates/init-root/src/install.rs service_file)."""
    x = lambda s: s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
    exe, h = x(os.path.join(rh, "bin", "init-root")), x(rh)
    return ('<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
            '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict>\n'
            '<key>Label</key><string>dev.init.root</string>\n'
            '<key>ProgramArguments</key><array><string>%s</string><string>start</string></array>\n'
            '<key>EnvironmentVariables</key><dict><key>INIT_ROOT_HOME</key><string>%s</string></dict>\n'
            '<key>KeepAlive</key><true/>\n<key>ThrottleInterval</key><integer>10</integer>\n'
            '<key>ExitTimeOut</key><integer>60</integer>\n'
            '<key>StandardOutPath</key><string>%s/logs/launchd.log</string>\n'
            '<key>StandardErrorPath</key><string>%s/logs/launchd.log</string>\n</dict></plist>\n') % (exe, h, h, h)


def unit(rh):
    """The systemd user unit as init-root install writes it (systemd quoting)."""
    q = lambda s: s.replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%")
    return ('[Unit]\nDescription=INIT Root\n\n[Service]\nExecStart="%s" start\nEnvironment="INIT_ROOT_HOME=%s"\n'
            'Restart=always\nRestartSec=2\nKillMode=mixed\nTimeoutStopSec=60\nDelegate=memory\n\n[Install]\n'
            'WantedBy=default.target\n') % (q(os.path.join(rh, "bin", "init-root")), q(rh))


@unittest.skipUnless(TARGET, "needs macOS 14+ on arm64, or Linux with /usr/bin/bwrap and /usr/bin/python3")
class GetSh(unittest.TestCase):
    """get.sh, end to end against a fake release."""

    def setUp(self):
        self.t = scratch(self)
        self.home = os.path.join(self.t, "home")
        self.dist = os.path.join(self.t, "dist", "rel")
        self.fake = os.path.join(self.t, "fakebin")
        self.tmpdir = os.path.join(self.t, "tmpdir")
        for d in (self.home, self.dist, self.fake, self.tmpdir):
            os.makedirs(d)
        for name, text in [("curl", FAKE_CURL)] + ([("uname", FAKE_UNAME)] if FAKE_X86 else []):
            with open(os.path.join(self.fake, name), "w") as f:
                f.write(text)
            os.chmod(os.path.join(self.fake, name), 0o755)
        self.rh = os.path.join(self.home, ".init-root")  # this run's ROOT_HOME: the default, never created
        self.bin = os.path.join(self.home, ".local", "bin")
        self.other = os.path.join(self.t, "my rh's")  # the earlier install's ROOT_HOME (a space and a quote)

    def latest(self, targets, single=False):
        rec = {"v": 1, "version": "9.9.9-test", "target": targets[0] if targets else "none", "file": "x.tar.gz"}
        if single:  # one release file per target, as bundle.sh writes it: "target" and "sha256"
            rec["sha256"] = "0" * 64
        else:
            rec.update({"sha256_%s" % t: "0" * 64 for t in targets})
        with open(os.path.join(self.dist, "latest.json"), "w") as f:
            json.dump(rec, f, indent=1, sort_keys=True)

    def get(self, *args, **env_extra):
        env = {"PATH": self.fake + ":/usr/bin:/bin:/usr/sbin:/sbin", "HOME": self.home, "LANG": "C", "INIT_DIST_URL": DIST,
               "FAKE_DIST": os.path.join(self.t, "dist"), "INIT_APP_DIR": os.path.join(self.t, "app"), "TMPDIR": self.tmpdir}
        env.update(env_extra)
        p = subprocess.run(["/bin/sh", GET, *args], env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                           timeout=120)
        self.assertEqual(os.listdir(self.tmpdir), [], "get.sh left a temporary file behind")
        return p

    def fetched(self):
        log = os.path.join(self.t, "curl.log")
        if not os.path.exists(log):
            return []
        with open(log) as f:
            return f.read().split()

    def assert_refused_before_anything(self, p):
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertFalse(os.path.exists(self.rh), "get.sh created this run's ROOT_HOME")
        self.assertFalse(os.path.exists(os.path.join(self.t, "app")), "get.sh unpacked something")
        self.assertEqual(self.fetched(), [DIST + "/latest.json"], "get.sh fetched more than latest.json")

    # ---------------------------------------------------------------- 1. INIT in another ROOT_HOME
    def test_a_launcher_naming_another_home_gets_that_homes_upgrade_line(self):
        self.latest([TARGET])
        fake_install(self.other, "0.5.1")
        put_launcher(self.bin, self.other)
        p = self.get()
        self.assert_refused_before_anything(p)
        e = p.stderr
        self.assertIn("INIT 0.5.1 is already installed in %s: the init-agent command in %s opens it. A second install, in %s,"
                      " would take that over and leave this one behind. Upgrade it instead" % (self.other, self.bin, self.rh), e)
        line = "INIT_ROOT_HOME=%s init-agent upgrade" % shq(self.other)
        self.assertIn("\n  %s\n" % line, e)
        self.assertIn("own INIT_ROOT_HOME, INIT_HOME and INIT_BIN_DIR, without the service: sh -s -- -- --no-service.", e)
        self.assertNotIn("\n  init-agent upgrade", e)
        # the line runs as printed: a stand-in init-agent on PATH, in a new terminal without INIT_ROOT_HOME
        stand_in = os.path.join(self.t, "path")
        os.makedirs(stand_in)
        with open(os.path.join(stand_in, "init-agent"), "w") as f:
            f.write('#!/bin/sh\necho "home=${INIT_ROOT_HOME-unset}|args=$*"\n')
        os.chmod(os.path.join(stand_in, "init-agent"), 0o755)
        q = subprocess.run(["/bin/sh", "-c", line], env={"PATH": stand_in + ":/usr/bin:/bin", "HOME": self.home},
                           capture_output=True, text=True, timeout=30)
        self.assertEqual(q.stdout.strip(), "home=%s|args=upgrade" % self.other, q.stderr)

    def test_every_release_gets_its_own_line_for_that_home(self):
        self.latest([TARGET])
        put_launcher(self.bin, self.other)
        fake_install(self.other, "0.5.0")
        p = self.get()
        self.assert_refused_before_anything(p)
        pipe = "/usr/bin/curl -q -fsSL %s/get.sh | INIT_ROOT_HOME=%s /bin/sh -s -- --upgrade" % (DIST, shq(self.other))
        self.assertIn("INIT 0.5.0 is already installed in %s: the init-agent command" % self.other, p.stderr)
        self.assertIn("\n  %s\n" % pipe, p.stderr)
        self.assertEqual(run_pipe(self, pipe, DIST + "/get.sh", self.home), "home=%s|args=--upgrade" % self.other)
        shutil.rmtree(self.other)
        os.remove(os.path.join(self.t, "curl.log"))
        fake_install(self.other, "0.3.1")
        p = self.get()
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("INIT 0.3.1 is installed in %s: the init-agent command" % self.other, p.stderr)
        self.assertIn('&& INIT_ROOT_HOME=%s bash "$f"' % shq(self.other), p.stderr)
        self.assertEqual(self.fetched(), [], "a release before 0.4.1 needs no download to say so")

    def test_no_refusal_without_an_install_there_or_with_a_separate_setup(self):
        # the launcher names a home with no install.json (removed by hand): nothing to strand, the install goes on
        self.latest([TARGET])
        put_launcher(self.bin, self.other)
        os.makedirs(self.other)
        p = self.get()
        self.assertNotIn("already installed", p.stderr)
        self.assertIn("%s/init-v9.9.9-test-%s.tar.gz" % (DIST, TARGET), self.fetched(), p.stderr)
        # an install there, and this run in its own INIT_BIN_DIR without the service: a separate INIT, not refused
        fake_install(os.path.join(self.t, "x"), "0.5.1")
        put_launcher(self.bin, os.path.join(self.t, "x"))
        os.remove(os.path.join(self.t, "curl.log"))
        p = self.get("--", "--no-service", INIT_BIN_DIR=os.path.join(self.t, "bin2"))
        self.assertNotIn("already installed", p.stderr)
        self.assertIn("%s/init-v9.9.9-test-%s.tar.gz" % (DIST, TARGET), self.fetched(), p.stderr)

    def test_a_link_or_a_foreign_file_is_not_a_launcher(self):
        self.latest([TARGET])
        fake_install(self.other, "0.5.1")
        os.makedirs(self.bin)
        real = put_launcher(os.path.join(self.t, "elsewhere"), self.other)
        os.symlink(real, os.path.join(self.bin, "init-agent"))  # a link: not INIT's file in BIN_DIR
        with open(os.path.join(self.bin, "init"), "w") as f:
            f.write("#!/bin/sh\nINIT_ROOT_HOME_DEFAULT_X=1\n")  # someone else's init
        p = self.get()
        self.assertNotIn("already installed", p.stderr)

    # ---------------------------------------------------------------- 2. no bundle for this machine
    def test_no_bundle_for_this_target_is_said_before_the_upgrade_line(self):
        others = [t for t in ("macos-arm64", "linux-arm64", "linux-x86_64") if t != TARGET]
        for version, tail in (("0.4.1", "Run this once, from your own shell (0.4.1 has no init-agent upgrade):\n  /usr/bin/curl"),
                              ("0.5.1", ":\n  INIT_ROOT_HOME=%s init-agent upgrade\n" % shq(self.rh))):
            with self.subTest(version=version):
                self.latest(others)
                fake_install(self.rh, version)
                p = self.get()
                self.assertEqual(p.returncode, 1, p.stderr)
                self.assertIn("INIT %s is already installed in %s. The latest release (INIT 9.9.9-test) has no bundle yet "
                              "for this machine, %s, so an upgrade has nothing to install here: the INIT you have keeps "
                              "working as it is. When a %s bundle is released (%s), upgrade it, which keeps what INIT has "
                              "become" % (version, self.rh, TNAME[TARGET], TARGET, DIST), p.stderr)
                self.assertIn(tail, p.stderr)
                self.assertNotIn("Upgrade it instead", p.stderr)
                self.assertEqual(self.fetched(), [DIST + "/latest.json"])
                shutil.rmtree(self.rh)
                os.remove(os.path.join(self.t, "curl.log"))

    def test_a_listed_bundle_keeps_the_plain_upgrade_advice(self):
        fake_install(self.rh, "0.5.1")
        for single in (False, True):
            with self.subTest(single=single):
                self.latest([TARGET], single=single)
                p = self.get()
                self.assertEqual(p.returncode, 1, p.stderr)
                self.assertIn("INIT 0.5.1 is already installed in %s. Upgrade it instead, which keeps what INIT has become:\n"
                              "  INIT_ROOT_HOME=%s init-agent upgrade\n" % (self.rh, shq(self.rh)), p.stderr)
                self.assertNotIn("no bundle", p.stderr)

    def test_an_unreachable_release_keeps_the_plain_upgrade_advice(self):
        fake_install(self.rh, "0.5.1")
        p = self.get(INIT_DIST_URL="https://down.invalid/rel")
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("Upgrade it instead, which keeps what INIT has become:", p.stderr)
        self.assertNotIn("curl:", p.stderr)


class Functions(unittest.TestCase):
    """The scripts' functions alone: the service files in a fake passwd home, and an upgrade with no bundle."""

    def run_fn(self, script, shell, names, call, env):
        src = functions(script, *names) + "\n" + call
        base = {"PATH": "/usr/bin:/bin", "LANG": "C"}
        base.update(env)
        return subprocess.run([shell, "-c", src], env=base, capture_output=True, text=True, timeout=60)

    def test_the_service_file_names_the_other_home(self):
        t = scratch(self)
        pw = os.path.join(t, "pw")
        bin_dir = os.path.join(pw, ".local", "bin")
        os.makedirs(bin_dir)
        for script, shell in ((GET, "/bin/sh"), (INSTALL, "/bin/bash")):
            for osname, rel, text, by in (
                    ("Darwin", "Library/LaunchAgents/dev.init.root.plist", plist, "the LaunchAgent dev.init.root runs it"),
                    ("Linux", ".config/systemd/user/init-root.service", unit, "the systemd user unit init-root.service runs it")):
                other = os.path.join(t, "a&b <c> 50% \"rh\"")  # XML and systemd escapes both
                fake_install(other, "0.5.1")
                svc = os.path.join(pw, rel)
                os.makedirs(os.path.dirname(svc), exist_ok=True)
                with open(svc, "w") as f:
                    f.write(text(other))
                env = {"PW_HOME": pw, "HOME": pw, "NO_SERVICE": "0", "OS": osname, "BIN_DIR": bin_dir,
                       "RH": os.path.join(pw, ".init-root")}
                call = 'other_home "$(svc_file)"'
                with self.subTest(script=os.path.basename(script), os=osname):
                    p = self.run_fn(script, shell, ("svc_file", "other_home"), call, env)
                    self.assertEqual((p.returncode, p.stdout), (0, "%s\n%s\n" % (other, by)), p.stderr)
                    # the install writes no service: with --no-service, or with HOME not the passwd home
                    for extra in ({"NO_SERVICE": "1"}, {"HOME": os.path.join(t, "elsewhere")}):
                        p = self.run_fn(script, shell, ("svc_file", "other_home"), call, dict(env, **extra))
                        self.assertEqual((p.returncode, p.stdout), (0, ""), (extra, p.stderr))
                    # the home it names holds no install: nothing to strand
                    os.remove(os.path.join(other, "install.json"))
                    p = self.run_fn(script, shell, ("svc_file", "other_home"), call, env)
                    self.assertEqual((p.returncode, p.stdout), (0, ""), p.stderr)
                shutil.rmtree(other)
                os.remove(svc)

    def test_get_sh_reads_no_service_from_the_install_sh_arguments(self):
        # the block after get.sh's argument loop, alone: "$@" is what goes to install.sh after --
        with open(GET) as f:
            m = re.search(r"^NO_SERVICE=0\b.*\n.*\n", f.read(), re.M)
        self.assertIsNotNone(m, "get.sh sets no NO_SERVICE")
        for args, want in (([], "0"), (["--disk-floor", "5"], "0"), (["--no-service"], "1"), (["--yes", "--no-service", "-x"], "1")):
            p = subprocess.run(["/bin/sh", "-c", m.group(0) + 'echo "$NO_SERVICE"', "get.sh", *args],
                               env={"PATH": "/usr/bin:/bin"}, capture_output=True, text=True, timeout=30)
            self.assertEqual(p.stdout.strip(), want, (args, p.stderr))

    def test_install_sh_refuses_with_that_homes_lines(self):
        # install.sh from this checkout, with the launcher of an install elsewhere in the default INIT_BIN_DIR
        t = tempfile.mkdtemp(prefix="i5-rp52-", dir="/tmp")  # short: install.sh checks home lengths first
        self.addCleanup(shutil.rmtree, t, True)
        home = os.path.join(t, "h")
        other = os.path.join(t, "o r")
        os.makedirs(home)
        fake_install(other, "0.5.1")
        bin_dir = os.path.join(home, ".local", "bin")
        put_launcher(bin_dir, other)
        before = tree(t)
        p = subprocess.run(["/bin/bash", INSTALL, "--yes", "--no-service"], stdin=subprocess.DEVNULL, capture_output=True,
                           text=True, timeout=120, env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": home, "LANG": "C"})
        self.assertEqual(tree(t), before, "install.sh wrote something\n" + p.stdout + p.stderr)
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        e = p.stderr
        rh = "INIT_ROOT_HOME=%s " % shq(other)
        self.assertIn("INIT 0.5.1 is already installed in %s: the init-agent command in %s opens it. A second install, in %s,"
                      " would take that over and leave this one behind. Upgrade it to v" % (other, bin_dir, os.path.join(home, ".init-root")), e)
        self.assertIn("\n  %sinit-agent upgrade        (or: %s%s --upgrade)\n" % (rh, rh, shq(INSTALL)), e)
        self.assertIn("own INIT_ROOT_HOME, INIT_HOME and INIT_BIN_DIR, and pass --no-service.", e)
        self.assertNotIn("/install.json exists", e)

    def test_an_upgrade_that_finds_no_bundle_says_nothing_changed(self):
        t = scratch(self)
        with open(os.path.join(t, "latest.json"), "w") as f:
            json.dump({"version": "9.9.9", "target": "macos-arm64", "sha256_macos-arm64": "0" * 64}, f, indent=1)
        env = {"TMP": t, "TARGET": "linux-arm64", "PLATFORM": "Linux 6.1 aarch64", "LVER": "9.9.9",
               "DIST_URL": "https://github.com/nagisanzenin/INIT-Agent/releases/latest/download"}
        call = 'no_bundle "$DIST_URL/init-v9.9.9-linux-arm64.tar.gz" 404'
        names = ("die", "tname", "page", "no_bundle")
        p = self.run_fn(GET, "/bin/sh", names, call, dict(env, UPGRADE="1"))
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("there is no INIT bundle for this machine (Linux 6.1 aarch64, target linux-arm64)", p.stderr)
        self.assertIn("This release (INIT 9.9.9) has bundles for:\n    macOS 14+ on Apple silicon (macos-arm64)\n", p.stderr)
        self.assertIn("  Nothing was changed: the INIT you have keeps working as it is. Run this upgrade again when a linux-arm64\n"
                      "  bundle is released: https://github.com/nagisanzenin/INIT-Agent/releases\n", p.stderr)
        self.assertNotIn("scripts/install.sh", p.stderr)  # a source install over an install refuses
        p = self.run_fn(GET, "/bin/sh", names, call, dict(env, UPGRADE="0"))
        self.assertIn("build from source here", p.stderr)
        self.assertIn("run this installer again when a linux-arm64 bundle is released: https://github.com/nagisanzenin/"
                      "INIT-Agent/releases\n", p.stderr)
        self.assertNotIn("Nothing was changed", p.stderr)


if __name__ == "__main__":
    unittest.main()
