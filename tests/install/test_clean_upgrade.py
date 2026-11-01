"""`init-agent upgrade` takes nothing from the environment (v0.5.4; the post-release review of v0.5.3).

Until 0.5.3 the launcher's upgrade preferred an INIT_ROOT_HOME in the environment over the home install.sh baked into it,
and handed the whole environment on to the kept get.sh. With full access the agent can write the owner's shell startup
files, so a planted INIT_ROOT_HOME ran another home's cache/get.sh, one the agent wrote (docs/release-signing.md A3).
The case: a launcher baked for home A, as install.sh writes it, run with INIT_ROOT_HOME=B (whose cache/get.sh is the
agent's), INIT_DIST_URL, INIT_BIN_DIR and a planted PATH in the environment. A's get.sh must run, with HOME, A and the
system PATH only; B's must not; and the launcher says what it ignored. Stdlib only.
"""
import os
import socket
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
        self.assertEqual(p.stderr.count("is ignored"), 1, "said once")
        # the home it was installed for, given again: no note
        p = subprocess.run(["/bin/bash", launcher, "upgrade"], env=dict(env, INIT_ROOT_HOME=a), stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual((p.returncode, p.stderr), (0, ""))


class BakedHome(unittest.TestCase):
    """v0.5.5 (the post-release review of v0.5.4): the TUI and `init-root attach` use the launcher's baked home too.
    Until 0.5.4 an INIT_ROOT_HOME in the environment won there, so a planted one ran, as the owner, the init-tui (or the
    init-root) of a home the agent wrote. The case: a launcher baked for home A, as install.sh writes it, with no init-tui
    beside it, run with INIT_ROOT_HOME=B, whose bin/init-tui and bin/init-root write a marker. A's TUI runs, with
    INIT_ROOT_HOME=A; then, with A's TUI gone, A's `init-root attach`; B's never; the launcher says once what it ignored,
    and nothing when the environment names A itself."""

    def test_the_tui_and_attach_take_the_baked_home(self):
        t = tempfile.mkdtemp(prefix="i5-bh-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", t], check=False)
        a, b, planted = os.path.join(t, "a"), os.path.join(t, "b"), os.path.join(t, "planted.txt")
        for h, body in ((a, 'printf "%s|%s|%s" "${0##*/}" "$INIT_ROOT_HOME" "$*"'), (b, "echo planted >>'%s'" % planted)):
            os.makedirs(os.path.join(h, "bin"))
            os.makedirs(os.path.join(h, "run"))
            with open(os.path.join(h, "install.json"), "w") as f:
                f.write('{"init_home":"%s/i","v":1}\n' % h)
            for n in ("init-tui", "init-root"):
                with open(os.path.join(h, "bin", n), "w") as f:
                    f.write("#!/bin/sh\n%s\n" % body)
                os.chmod(os.path.join(h, "bin", n), 0o755)
        socks = []
        for h in (a, b):  # a running Root's owner socket, for the attach path
            s = socket.socket(socket.AF_UNIX)
            s.bind(os.path.join(h, "run", "owner.sock"))
            socks.append(s)
            self.addCleanup(s.close)
        launcher = put_launcher(os.path.join(t, "bin"), a)
        env = {"PATH": SYSTEM_PATH, "HOME": t, "INIT_ROOT_HOME": b, "LANG": "C"}
        run = lambda *args, **e: subprocess.run([launcher, *args], env=dict(env, **e), stdin=subprocess.DEVNULL,
                                                capture_output=True, text=True, timeout=30)
        note = ("init-agent: note: INIT_ROOT_HOME=%s in this shell's environment is ignored; this opens the INIT in\n  %s,"
                " the home this launcher was installed for." % (b, a))
        p = run("--plain")
        self.assertEqual((p.returncode, p.stdout), (0, "init-tui|%s|--plain" % a), p.stderr)
        self.assertIn(note, p.stderr)
        self.assertEqual(p.stderr.count("is ignored"), 1, "said once")
        p = run("--which")
        self.assertEqual(p.stdout, os.path.join(a, "bin", "init-tui") + "\n", p.stderr)
        os.remove(os.path.join(a, "bin", "init-tui"))  # no TUI: init-root attach
        p = run()
        self.assertEqual((p.returncode, p.stdout), (0, "init-root|%s|attach" % a), p.stderr)
        self.assertIn(note, p.stderr)
        self.assertFalse(os.path.exists(planted), "the environment's home ran")
        p = run(INIT_ROOT_HOME=a)  # the home it was installed for: no note
        self.assertEqual((p.returncode, p.stdout, p.stderr), (0, "init-root|%s|attach" % a, ""))

    def test_an_installed_launcher_ignores_init_tui(self):
        # v0.5.5 (review B of lane J): INIT_TUI chose the program the launcher runs as the owner, and a full-access agent
        # can plant it (with a file of its own anywhere outside INIT_HOME). An installed launcher ignores it and says so
        # once; inside INIT_HOME it is ignored the same way (the installed TUI opens). The repository's own copy, with no
        # home baked in, still takes it (the stand-in TUIs of the other tests), and still refuses one inside INIT_HOME.
        t = tempfile.mkdtemp(prefix="i5-bt-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", t], check=False)
        a, ih, planted = os.path.join(t, "a"), os.path.join(t, "ih"), os.path.join(t, "planted.txt")
        os.makedirs(os.path.join(a, "bin"))
        os.makedirs(ih)
        with open(os.path.join(a, "install.json"), "w") as f:
            f.write('{"init_home":"%s","v":1}\n' % ih)
        tuis = {os.path.join(a, "bin", "init-tui"): 'printf "%s|%s" "$0" "$*"', os.path.join(t, "evil-tui"): "echo planted >>'%s'" % planted,
                os.path.join(ih, "init-tui"): "echo planted >>'%s'" % planted}
        for path, body in tuis.items():
            with open(path, "w") as f:
                f.write("#!/bin/sh\n%s\n" % body)
            os.chmod(path, 0o755)
        launcher = put_launcher(os.path.join(t, "bin"), a)
        for evil in (os.path.join(t, "evil-tui"), os.path.join(ih, "init-tui")):
            with self.subTest(init_tui=evil):
                p = subprocess.run([launcher, "--plain"], env={"PATH": SYSTEM_PATH, "HOME": t, "LANG": "C", "INIT_TUI": evil},
                                   stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
                self.assertEqual((p.returncode, p.stdout), (0, "%s|--plain" % os.path.join(a, "bin", "init-tui")), p.stderr)
                self.assertEqual(p.stderr, "init-agent: note: INIT_TUI=%s in this shell's environment is ignored; this opens the "
                                 "init-tui installed\n  with this launcher. (If you did not set it, something else did: look at "
                                 "your shell's startup files.)\n" % evil)
                self.assertFalse(os.path.exists(planted), "the environment's TUI ran")
        p = subprocess.run([launcher, "--plain"], env={"PATH": SYSTEM_PATH, "HOME": t, "LANG": "C"}, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=30)
        self.assertEqual((p.returncode, p.stderr), (0, ""), "no INIT_TUI: no note")
        repo_copy = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))), "scripts", "init-agent")
        env = {"PATH": SYSTEM_PATH, "HOME": t, "LANG": "C", "INIT_ROOT_HOME": a}
        p = subprocess.run(["/bin/bash", repo_copy, "--which"], env=dict(env, INIT_TUI=os.path.join(t, "evil-tui")),
                           stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
        self.assertEqual((p.returncode, p.stdout), (0, os.path.join(t, "evil-tui") + "\n"), "the repository's copy takes INIT_TUI")
        p = subprocess.run(["/bin/bash", repo_copy, "--which"], env=dict(env, INIT_TUI=os.path.join(ih, "init-tui")),
                           stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
        self.assertEqual(p.returncode, 77, "and refuses one inside INIT_HOME (T-4)")


if __name__ == "__main__":
    unittest.main()
