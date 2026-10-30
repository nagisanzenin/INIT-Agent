"""The install puts the init-agent command's directory on PATH itself, and the uninstaller takes it off (v0.5.4).

install.sh's path_step (step 6) adds a marked block to the startup file of the owner's shell ($SHELL: zsh
${ZDOTDIR:-~}/.zshrc, bash ~/.bashrc on Linux or ~/.bash_profile on macOS, fish ~/.config/fish/conf.d/init-agent.fish,
another ~/.profile), only when BIN_DIR is not on PATH, never with --no-modify-path, and never twice. uninstall.sh removes
exactly that block and leaves every other line, and every file without the block, as it was. Every case runs in a scratch
HOME: no owner file is read or written. get.sh passes --no-modify-path on to install.sh. Stdlib only.
"""
import hashlib
import json
import os
import platform
import subprocess
import tempfile
import unittest

from test_printed_commands import functions
from test_upgrade_trust import bundle, TARGET

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
INSTALL = os.path.join(REPO, "scripts", "install.sh")
UNINSTALL = os.path.join(REPO, "scripts", "uninstall.sh")
GET = os.path.join(REPO, "scripts", "get.sh")
BEGIN = "# >>> INIT: the init-agent command on PATH (scripts/uninstall.sh removes this block) >>>"
END = "# <<< INIT: the init-agent command on PATH <<<"


def read(p):
    with open(p) as f:
        return f.read()


class PathBlock(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.mkdtemp(prefix="i5-pb-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", self.t], check=False)
        self.home = os.path.join(self.t, "home")
        os.makedirs(self.home)

    def step(self, shell, bin_dir=None, os_name="Linux", path="/usr/bin:/bin", no_modify=0):
        bin_dir = bin_dir or os.path.join(self.home, ".local", "bin")
        grab = lambda n: next(l for l in read(INSTALL).splitlines() if l.startswith(n + "="))
        src = "\n".join([grab("PATH_BEGIN"), grab("PATH_END"), 'PATH_ADDED=""', 'say() { printf "%s\\n" "$*"; }',
                         functions(INSTALL, "shq", "tilde", "path_add", "path_file", "path_line", "path_step"), "path_step"])
        env = {"PATH": path, "HOME": self.home, "SHELL": "/bin/" + shell, "OS": os_name, "BIN_DIR": bin_dir,
               "NO_MODIFY_PATH": str(no_modify), "LANG": "C"}
        # stdin from /dev/null: bash reads ~/.bashrc even under -c when its stdin is a socket (its remote-shell check),
        # which the test runner's stdin can be; a block sourced that way put BIN_DIR on PATH, so path_step wrote nothing
        p = subprocess.run(["/bin/bash", "-c", 'PATH="$PATH"; ' + src], env=env, stdin=subprocess.DEVNULL, capture_output=True,
                           text=True, timeout=30)
        self.assertEqual(p.returncode, 0, p.stderr)
        return p.stdout

    def path_in(self, shell, rc):
        """PATH after a new terminal of `shell` reads `rc`."""
        p = subprocess.run([shell, "-c", '. "$1"; printf %s "$PATH"', "sh", rc], stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30,
                           env={"PATH": "/usr/bin:/bin", "HOME": self.home})
        self.assertEqual(p.returncode, 0, p.stderr)
        return p.stdout

    def test_zsh_bash_and_others_get_a_block_in_their_file_once(self):
        b = os.path.join(self.home, ".local", "bin")
        for shell, os_name, rel in (("zsh", "Darwin", ".zshrc"), ("bash", "Linux", ".bashrc"), ("bash", "Darwin", ".bash_profile"),
                                    ("dash", "Linux", ".profile")):
            rc = os.path.join(self.home, rel)
            with open(rc, "w") as f:
                f.write("alias ll='ls -l'")  # the owner's line, without a final newline
            out = self.step(shell, os_name=os_name)
            self.assertIn("added ~/.local/bin to PATH in ~/%s; open a new terminal, or run ~/.local/bin/init-agent now" % rel, out)
            self.assertEqual(read(rc), "alias ll='ls -l'\n%s\nexport PATH=\"$HOME/.local/bin:$PATH\"\n%s\n" % (BEGIN, END))
            sh = "/bin/zsh" if shell == "zsh" else "/bin/bash" if shell == "bash" else "/bin/sh"
            if os.path.exists(sh):
                self.assertEqual(self.path_in(sh, rc), b + ":/usr/bin:/bin", (shell, os_name, read(rc)))
            self.assertIn("already puts", self.step(shell, os_name=os_name))  # a re-install or an upgrade: no second block
            self.assertEqual(read(rc).count(BEGIN), 1)
            os.remove(rc)

    def test_fish_gets_a_file_of_its_own(self):
        out = self.step("fish")
        f = os.path.join(self.home, ".config", "fish", "conf.d", "init-agent.fish")
        self.assertIn("in ~/.config/fish/conf.d/init-agent.fish;", out)
        q = "'%s'" % os.path.join(self.home, ".local", "bin")
        self.assertEqual(read(f), "%s\ncontains -- %s $PATH; or set -gx PATH %s $PATH\n%s\n" % (BEGIN, q, q, END))

    def test_a_custom_bin_dir_is_quoted_for_each_shell(self):
        b = os.path.join(self.t, "my $bin's \"dir\"")  # outside HOME: the absolute path
        self.step("zsh", bin_dir=b)
        self.assertEqual(self.path_in("/bin/zsh" if os.path.exists("/bin/zsh") else "/bin/sh", os.path.join(self.home, ".zshrc")),
                         b + ":/usr/bin:/bin")
        self.step("fish", bin_dir=b)
        self.assertIn("set -gx PATH '%s' $PATH" % b.replace("'", "\\'"),
                      read(os.path.join(self.home, ".config", "fish", "conf.d", "init-agent.fish")))

    def test_on_path_already_or_no_modify_path_writes_nothing(self):
        b = os.path.join(self.home, ".local", "bin")
        self.assertEqual(self.step("zsh", path=b + ":/usr/bin:/bin"), "")
        out = self.step("zsh", no_modify=1)
        self.assertIn("--no-modify-path: left as it is", out)
        self.assertEqual(os.listdir(self.home), [], "a file was written")

    def test_uninstall_removes_only_the_block(self):
        rh, ih, b = (os.path.join(self.home, n) for n in (".init-root", ".init", ".local/bin"))
        os.makedirs(os.path.join(rh, "keys"))
        os.makedirs(ih)
        with open(os.path.join(rh, "install.json"), "w") as f:
            json.dump({"init_home": ih, "bin_dir": b, "v": 1}, f)
        zshrc, bashrc = os.path.join(self.home, ".zshrc"), os.path.join(self.home, ".bashrc")
        mine = "export EDITOR=vi\n"
        with open(zshrc, "w") as f:
            f.write(mine)
        self.step("zsh")
        with open(zshrc, "a") as f:
            f.write("alias g=git\n")  # the owner's line after the block
        self.step("fish")
        with open(bashrc, "w") as f:  # a file without the block
            f.write("# nothing of INIT's\n")
        before = os.stat(bashrc).st_mtime_ns
        p = subprocess.run(["/bin/bash", UNINSTALL], input="uninstall\n", capture_output=True, text=True, timeout=120,
                           env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": self.home, "LANG": "C"})
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(read(zshrc), mine + "alias g=git\n")
        self.assertFalse(os.path.exists(os.path.join(self.home, ".config", "fish", "conf.d", "init-agent.fish")))
        self.assertEqual((read(bashrc), os.stat(bashrc).st_mtime_ns), ("# nothing of INIT's\n", before))
        self.assertIn("removed INIT's PATH block from %s\n" % zshrc, p.stdout)
        self.assertIn("It also takes INIT's PATH block", p.stdout)

    def install_json(self, b):
        rh, ih = os.path.join(self.home, ".init-root"), os.path.join(self.home, ".init")
        os.makedirs(os.path.join(rh, "keys"), exist_ok=True)
        os.makedirs(ih, exist_ok=True)
        with open(os.path.join(rh, "install.json"), "w") as f:
            json.dump({"init_home": ih, "bin_dir": b, "v": 1}, f)

    def uninstall(self):
        return subprocess.run(["/bin/bash", UNINSTALL], input="uninstall\n", capture_output=True, text=True, timeout=120,
                              env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": self.home, "LANG": "C"})

    def test_another_installs_block_is_neither_this_ones_nor_removed_with_it(self):
        # v0.5.4 (the release review): "already puts … on PATH" was said for any INIT block, and the uninstaller matched
        # blocks by substring, so uninstalling ~/.local/bin took ~/.local/bin2's block too
        b, b2 = os.path.join(self.home, ".local", "bin"), os.path.join(self.home, ".local", "bin2")
        zshrc = os.path.join(self.home, ".zshrc")
        self.step("zsh", bin_dir=b2)
        self.assertIn("added ~/.local/bin to PATH", self.step("zsh", bin_dir=b))  # its own block, not "already"
        self.assertEqual(read(zshrc).count(BEGIN), 2)
        self.install_json(b)
        p = self.uninstall()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(read(zshrc), "%s\nexport PATH=\"$HOME/.local/bin2:$PATH\"\n%s\n" % (BEGIN, END))

    def test_a_startup_file_that_is_not_utf8_or_not_writable_never_stops_the_uninstall(self):
        # v0.5.4 (the release review): a non-UTF-8 file crashed the uninstaller after the service was unloaded, and a
        # read-only one after the moves
        b = os.path.join(self.home, ".local", "bin")
        self.install_json(b)
        bashrc, zshrc = os.path.join(self.home, ".bashrc"), os.path.join(self.home, ".zshrc")
        with open(bashrc, "wb") as f:
            f.write(b"# caf\xe9 latin-1\n")
        self.step("bash")
        self.step("zsh")
        os.chmod(zshrc, 0o444)
        p = self.uninstall()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        with open(bashrc, "rb") as f:
            self.assertEqual(f.read(), b"# caf\xe9 latin-1\n")
        self.assertIn("left INIT's PATH block in %s (cannot write it: PermissionError)" % zshrc, p.stdout)
        self.assertFalse(os.path.exists(os.path.join(self.home, ".init-root")), "the homes were not moved")

    def test_a_link_out_of_home_or_another_shell_is_not_written(self):
        outside = os.path.join(self.t, "dotfiles-zshrc")
        with open(outside, "w") as f:
            f.write("# mine\n")
        os.symlink(outside, os.path.join(self.home, ".zshrc"))
        self.assertIn("is a link out of your home or not a regular file", self.step("zsh"))
        self.assertEqual(read(outside), "# mine\n")
        self.assertIn("INIT does not edit the startup files of tcsh", self.step("tcsh"))


class GetPassesIt(unittest.TestCase):
    def test_get_sh_passes_no_modify_path_to_install_sh(self):
        t = tempfile.mkdtemp(prefix="i5-pg-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/sh", "-c", 'chmod -R u+w "$1"; rm -rf -- "$1"', "sh", t], check=False)
        report = os.path.join(t, "report.txt")
        tgz = os.path.join(t, "init-v9.9.9-test-%s.tar.gz" % TARGET)
        with open(tgz, "wb") as f:
            f.write(bundle("9.9.9-test", report))
        h = os.path.join(t, "h")
        os.makedirs(h)
        env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": h, "LANG": "C", "INIT_ROOT_HOME": os.path.join(t, "r")}
        with open(tgz, "rb") as f:
            sha = hashlib.sha256(f.read()).hexdigest()
        p = subprocess.run(["/bin/sh", GET, "--from", tgz, "--sha256", sha, "--no-rust", "--no-modify-path", "--", "--no-service"],
                           env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120)
        self.assertIn("args --yes --no-modify-path --no-service", read(report), p.stdout + p.stderr)


if __name__ == "__main__":
    unittest.main()
