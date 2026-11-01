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
import re
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


class PlantedTools(unittest.TestCase):
    """v0.5.5 (the post-release review of v0.5.4): uninstall.sh runs the system tools only. It ran env, sed, grep, mv,
    seq, uname, id, stat, launchctl and systemctl from PATH, as the owner and outside the sandbox, and v0.5.4's PATH block
    puts ~/.local/bin, which a full-access agent can write, first. Stand-ins for those tools (and the others it runs) in
    the scratch home's ~/.local/bin, first on PATH, each write a marker line, then run the system's own; the marker must
    stay empty. Two runs: a Root holding its lock, so the stop step runs `env -i … init-root` (then the uninstaller stops,
    because the lock is still held), and no Root, through the confirmation and the moves to the Trash."""

    TOOLS = ("env", "sed", "grep", "mv", "seq", "uname", "id", "stat", "launchctl", "systemctl", "date", "basename",
             "mkdir", "cat", "tr", "chmod", "sleep", "rmdir", "sudo")

    def setUp(self):
        self.t = tempfile.mkdtemp(prefix="i5-u55p", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", self.t], check=False)
        self.home = os.path.join(self.t, "h")
        self.planted = os.path.join(self.home, ".local", "bin")
        os.makedirs(self.planted)
        self.marker = os.path.join(self.t, "ran")
        for n in self.TOOLS:  # the service tools and sudo run nothing real, whatever reaches them
            real = "exit 0" if n in ("launchctl", "systemctl", "sudo") else \
                'for d in /usr/bin /bin /usr/sbin /sbin; do [ -x "$d/%s" ] && exec "$d/%s" "$@"; done; exit 127' % (n, n)
            with open(os.path.join(self.planted, n), "w") as f:
                f.write('#!/bin/sh\necho "%s $*" >>\'%s\'\n%s\n' % (n, self.marker, real))
            os.chmod(os.path.join(self.planted, n), 0o755)
        self.rh, self.ih = os.path.join(self.home, ".init-root"), os.path.join(self.home, ".init")
        os.makedirs(os.path.join(self.rh, "keys"))
        os.makedirs(self.ih)
        with open(os.path.join(self.rh, "install.json"), "w") as f:
            json.dump({"init_home": self.ih, "bin_dir": self.planted, "v": 1}, f)

    def run_uninstall(self, answer):
        return subprocess.run(["/bin/bash", "-p", UNINSTALL], input=answer, capture_output=True, text=True, timeout=120,
                              env={"PATH": self.planted + ":/usr/bin:/bin:/usr/sbin:/sbin", "HOME": self.home, "LANG": "C"})

    def ran(self):
        if not os.path.exists(self.marker):
            return ""
        with open(self.marker) as f:
            return f.read()

    def test_no_tool_from_path_runs(self):
        # a Root that holds run/root.lock and has an owner socket: the stop step asks it through `env -i`
        os.makedirs(os.path.join(self.rh, "bin"))
        os.makedirs(os.path.join(self.rh, "run"))
        root = os.path.join(self.rh, "bin", "init-root")
        with open(root, "w") as f:  # a zone that is already down
            f.write('#!/bin/sh\ncase "$*" in *status*) echo \'{"slot":"down"}\' ;; esac\nexit 0\n')
        os.chmod(root, 0o755)
        sock = socket.socket(socket.AF_UNIX)
        sock.bind(os.path.join(self.rh, "run", "owner.sock"))
        lock = open(os.path.join(self.rh, "run", "root.lock"), "w")
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            p = self.run_uninstall("")
        finally:
            lock.close()
            sock.close()
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("the zone is stopped", p.stdout)
        self.assertIn("is still running without the service", p.stderr)
        self.assertEqual(self.ran(), "", "a tool from PATH ran (the stop step)")
        for n in ("bin", "run"):
            subprocess.run(["/bin/rm", "-rf", "--", os.path.join(self.rh, n)], check=True)
        # no Root: the whole run, through the confirmation and the moves
        p = self.run_uninstall("uninstall\n")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(self.ran(), "", "a tool from PATH ran (the moves)")
        self.assertEqual(sorted(n.rsplit(" ", 2)[0] for n in os.listdir(trash_of(self.home))), [".init", ".init-root"], p.stdout)


class ReadWhole(unittest.TestCase):
    """v0.5.5 (the post-release review of v0.5.4): bash has read all of uninstall.sh before its first command runs. The
    printed line downloads it to a file in $TMPDIR and runs `/bin/bash -p "$f"`; bash reads a script file as it runs it,
    so while the script waited for `uninstall`, an agent that can write $TMPDIR could append to the file (or rewrite the
    rest of it), and bash then ran those lines as the owner. The case: a copy of uninstall.sh in a scratch file, run as
    the line runs it; once it asks, a line that writes a marker is appended, then `uninstall` is typed. The uninstall
    must finish as before and the appended line must not run."""

    def test_a_line_appended_while_it_waits_never_runs(self):
        t = tempfile.mkdtemp(prefix="i5-u55r", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", t], check=False)
        home, marker, f = os.path.join(t, "h"), os.path.join(t, "ran"), os.path.join(t, "tmp.XXXX")
        rh, ih = os.path.join(home, ".init-root"), os.path.join(home, ".init")
        os.makedirs(os.path.join(rh, "keys"))
        os.makedirs(ih)
        with open(os.path.join(rh, "install.json"), "w") as out:
            json.dump({"init_home": ih, "bin_dir": os.path.join(home, ".local", "bin"), "v": 1}, out)
        with open(UNINSTALL, "rb") as src, open(f, "wb") as out:
            out.write(src.read())
        p = subprocess.Popen(["/bin/bash", "-p", f], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": home, "LANG": "C"})
        self.addCleanup(lambda: p.poll() is not None or p.kill())
        seen = b""
        while not seen.endswith(b"Type uninstall to continue: "):
            c = p.stdout.read(1)
            self.assertTrue(c, "it never asked: " + seen.decode())
            seen += c
        with open(f, "ab") as out:  # what an agent could do while the owner reads the list
            out.write(b"\n/usr/bin/touch '%s'\n" % marker.encode())
        out, err = p.communicate(b"uninstall\n", timeout=120)
        self.assertEqual(p.returncode, 0, (seen + out + err).decode())
        self.assertIn(b"INIT is uninstalled.", out)
        self.assertFalse(os.path.exists(rh), "the uninstall did not finish")
        self.assertFalse(os.path.exists(marker), "a line appended to the script after it started ran")


class InMemory(unittest.TestCase):
    """v0.5.5 (review B of lane J): the printed uninstall line keeps the download in memory. v0.5.4's line wrote it to a
    mktemp file and ran `/bin/bash -p "$f"`, and a watcher replaced that file between curl and bash in 3 of 8 runs. The
    case: the README's line, as the owner pastes it, with a stand-in for the network: a `curl` at a scratch path that
    prints this uninstall.sh (or, given -o FILE, writes it there, as v0.5.4's line asked). While it runs, a watcher
    lists TMPDIR and the folder it runs in; neither may ever hold a file. Then: a failed download, and a cut-off one
    (with curl's exit 18, and even with exit 0), run nothing; and --help prints its own text, never a file named after
    $0 (here `uninstall.sh` in the working folder, which an agent could write)."""

    def setUp(self):
        self.t = tempfile.mkdtemp(prefix="i5-u55m", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", self.t], check=False)
        with open(os.path.join(REPO, "README.md")) as f:
            self.line = re.search(r"^To uninstall \(everything goes to the Trash\): `([^`]*)`", f.read(), re.M).group(1)
        self.curl = os.path.join(self.t, "curl")
        with open(UNINSTALL, "rb") as f:
            self.script = f.read()
        with open(os.path.join(self.t, "uninstall.sh.net"), "wb") as f:
            f.write(self.script)
        with open(self.curl, "w") as f:  # MODE: whole (default), fail, cut (exit 18), cut0 (exit 0)
            f.write("""#!/bin/sh
src='%s'; out=""
while [ $# -gt 0 ]; do [ "$1" = -o ] && { out="$2"; shift; }; shift; done
case "${MODE:-whole}" in
  fail) exit 22 ;;
  cut|cut0) n=$(($(wc -c <"$src") / 2)); if [ -n "$out" ]; then head -c $n "$src" >"$out"; else head -c $n "$src"; fi
            [ "$MODE" = cut0 ] || exit 18 ;;
  *) if [ -n "$out" ]; then cat "$src" >"$out"; else cat "$src"; fi ;;
esac
""" % os.path.join(self.t, "uninstall.sh.net"))
        os.chmod(self.curl, 0o755)
        self.home, self.tmp, self.cwd = (os.path.join(self.t, n) for n in ("h", "tmp", "cwd"))
        for d in (self.tmp, self.cwd):
            os.makedirs(d)
        self.rh, self.ih = os.path.join(self.home, ".init-root"), os.path.join(self.home, ".init")
        os.makedirs(os.path.join(self.rh, "keys"))
        os.makedirs(self.ih)
        with open(os.path.join(self.rh, "install.json"), "w") as f:
            json.dump({"init_home": self.ih, "bin_dir": os.path.join(self.home, ".local", "bin"), "v": 1}, f)

    def run_line(self, mode="whole", answer="uninstall\n", watch=None):
        run = self.line.replace("/usr/bin/curl ", self.curl + " ", 1)
        self.assertNotEqual(run, self.line, "the line runs no /usr/bin/curl")
        sh = "/bin/zsh" if os.path.exists("/bin/zsh") else "/bin/sh"
        return subprocess.run([sh, "-c", run], input=answer, capture_output=True, text=True, timeout=120, cwd=self.cwd,
                              env={"PATH": "/usr/bin:/bin", "HOME": self.home, "TMPDIR": self.tmp, "LANG": "C", "MODE": mode})

    def test_a_watcher_has_nothing_to_replace(self):
        import threading
        # macOS's mktemp without a template ignores TMPDIR and uses the per-user temp folder: watched too, for a file that
        # holds this script (anything else there belongs to other programs; it is only listed, and such a file, which
        # only a line that writes one leaves, is removed by its exact path)
        user_tmp = subprocess.run(["/usr/bin/getconf", "DARWIN_USER_TEMP_DIR"], capture_output=True, text=True).stdout.strip() \
            if platform.system() == "Darwin" else ""
        before = set(os.listdir(user_tmp)) if user_tmp else set()
        seen, new, stop = set(), set(), threading.Event()
        def watch():
            while not stop.is_set():
                for d in (self.tmp, self.cwd):
                    seen.update(os.path.join(d, n) for n in os.listdir(d))
                if user_tmp:
                    new.update(os.path.join(user_tmp, n) for n in set(os.listdir(user_tmp)) - before)
        w = threading.Thread(target=watch)
        w.start()
        try:
            p = self.run_line()
        finally:
            stop.set()
            w.join()
        for f in new:
            try:
                with open(f, "rb") as h:
                    if h.read(len(self.script)) == self.script:
                        seen.add(f)
                        os.remove(f)
            except OSError:
                pass
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("INIT is uninstalled.", p.stdout)
        self.assertFalse(os.path.exists(self.rh), "the uninstall did not run")
        self.assertEqual(seen, set(), "a file the line wrote, which an agent could replace before bash reads it")
        self.assertNotIn("mktemp", self.line)
        self.assertLess(len(self.script), 100000, "a bash -c argument must stay under Linux's 128 KiB per argument")

    def test_a_failed_or_cut_off_download_runs_nothing(self):
        for mode in ("fail", "cut", "cut0"):
            with self.subTest(mode=mode):
                p = self.run_line(mode)
                self.assertNotEqual(p.returncode, 0, p.stdout)
                self.assertNotIn("This moves", p.stdout)
                self.assertNotIn("Type uninstall", p.stdout)
                self.assertTrue(os.path.isdir(self.rh), "something moved")

    def test_help_reads_no_file(self):
        with open(os.path.join(self.cwd, "uninstall.sh"), "w") as f:
            f.write("# PLANTED\n" * 30)
        p = subprocess.run(["/bin/bash", "-p", "-c", self.script.decode(), "uninstall.sh", "--help"], capture_output=True,
                           text=True, timeout=30, cwd=self.cwd, env={"PATH": "/usr/bin:/bin", "HOME": self.home})
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertTrue(p.stdout.startswith("uninstall.sh [--apparmor]: stop INIT"), p.stdout)
        self.assertNotIn("PLANTED", p.stdout)


if __name__ == "__main__":
    unittest.main()
