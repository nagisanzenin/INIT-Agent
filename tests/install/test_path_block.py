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
                         functions(INSTALL, "shq", "tilde", "path_add", "path_file", "path_line", "path_old", "path_old_note", "path_into",
                                   "path_step"), "path_step"])
        env = {"PATH": path, "HOME": self.home, "SHELL": "/bin/" + shell, "OS": os_name, "BIN_DIR": bin_dir,
               "RH": os.path.join(self.home, ".init-root"), "IH": os.path.join(self.home, ".init"), "PW_HOME": self.home,
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
            self.assertEqual(read(rc), "alias ll='ls -l'\n%s\nexport PATH=\"$PATH:$HOME/.local/bin\"\n%s\n" % (BEGIN, END))
            sh = "/bin/zsh" if shell == "zsh" else "/bin/bash" if shell == "bash" else "/bin/sh"
            if os.path.exists(sh):
                # v0.5.5: last on PATH, after the system's folders
                self.assertEqual(self.path_in(sh, rc), "/usr/bin:/bin:" + b, (shell, os_name, read(rc)))
            self.assertIn("already puts", self.step(shell, os_name=os_name))  # a re-install or an upgrade: no second block
            self.assertEqual(read(rc).count(BEGIN), 1)
            os.remove(rc)

    def test_fish_gets_a_file_of_its_own(self):
        out = self.step("fish")
        f = os.path.join(self.home, ".config", "fish", "conf.d", "init-agent.fish")
        self.assertIn("in ~/.config/fish/conf.d/init-agent.fish;", out)
        q = "'%s'" % os.path.join(self.home, ".local", "bin")
        self.assertEqual(read(f), "%s\ncontains -- %s $PATH; or set -gx PATH $PATH %s\n%s\n" % (BEGIN, q, q, END))

    def test_a_custom_bin_dir_is_quoted_for_each_shell(self):
        b = os.path.join(self.t, "my $bin's \"dir\"")  # outside HOME: the absolute path
        self.step("zsh", bin_dir=b)
        self.assertEqual(self.path_in("/bin/zsh" if os.path.exists("/bin/zsh") else "/bin/sh", os.path.join(self.home, ".zshrc")),
                         "/usr/bin:/bin:" + b)
        self.step("fish", bin_dir=b)
        self.assertIn("set -gx PATH $PATH '%s'" % b.replace("'", "\\'"),
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

    def test_the_block_puts_the_system_first(self):
        # v0.5.5 (the coordinator's review of lane J): the block appends BIN_DIR. With it first, a bare command the owner
        # types (sudo, git, bash) could be a file a full-access agent dropped into ~/.local/bin; last, the system's wins,
        # and init-agent, which is nowhere else, still resolves. The case: the block step 6 writes, read by a new
        # terminal of that shell, with a planted sudo and the launcher in ~/.local/bin
        b = os.path.join(self.home, ".local", "bin")
        os.makedirs(b)
        for n in ("sudo", "init-agent"):
            with open(os.path.join(b, n), "w") as f:
                f.write("#!/bin/sh\necho planted\n")
            os.chmod(os.path.join(b, n), 0o755)
        for shell, rel, sh in (("zsh", ".zshrc", "/bin/zsh"), ("bash", ".bashrc", "/bin/bash"), ("dash", ".profile", "/bin/sh")):
            with self.subTest(shell=shell):
                self.step(shell)
                if not os.path.exists(sh):
                    continue
                p = subprocess.run([sh, "-c", '. "$1"; command -v sudo; command -v init-agent', "sh", os.path.join(self.home, rel)],
                                   stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30,
                                   env={"PATH": "/usr/bin:/bin", "HOME": self.home})
                self.assertEqual(p.stdout.split(), ["/usr/bin/sudo", os.path.join(b, "init-agent")], p.stderr)

    def test_a_v054_block_is_noted_never_doubled_and_the_uninstaller_takes_it(self):
        # v0.5.5: v0.5.4's block put BIN_DIR first. Neither a re-install nor the upgrade edits a startup file, so each
        # says which file has it and what its middle line is now (per shell), and adds no second block; the uninstaller
        # removes the old block as it removes the new one
        old_sh = 'export PATH="$HOME/.local/bin:$PATH"'
        q = "'%s'" % os.path.join(self.home, ".local", "bin")
        old_fish = "contains -- %s $PATH; or set -gx PATH %s $PATH" % (q, q)
        zshrc = os.path.join(self.home, ".zshrc")
        fishf = os.path.join(self.home, ".config", "fish", "conf.d", "init-agent.fish")
        os.makedirs(os.path.dirname(fishf))
        for f, line in ((zshrc, old_sh), (fishf, old_fish)):
            with open(f, "w") as h:
                h.write("%s\n%s\n%s\n" % (BEGIN, line, END))
        before = (read(zshrc), read(fishf))
        out = self.step("zsh")
        self.assertIn("  note: ~/.zshrc puts ~/.local/bin first on PATH (INIT's block from v0.5.4); INIT now adds it last, so a "
                      "command you type is the system's, not a file an agent dropped there. To change it, make the block's "
                      "middle line: export PATH=\"$PATH:$HOME/.local/bin\"\n", out)
        self.assertIn("  note: ~/.config/fish/conf.d/init-agent.fish puts ~/.local/bin first on PATH (INIT's block from v0.5.4); "
                      "INIT now adds it last, so a command you type is the system's, not a file an agent dropped there. To "
                      "change it, make the block's middle line: contains -- %s $PATH; or set -gx PATH $PATH %s\n" % (q, q), out)
        self.assertNotIn("added", out)
        self.assertEqual((read(zshrc), read(fishf)), before, "a startup file was changed")
        # the upgrade says the same, after its commit (it calls the same function there)
        text = read(INSTALL)
        commit = text[text.index('step "upgrade 5/5 commit"'):text.index("INIT is upgraded to v$VERSION")]
        self.assertIn("path_old_note || true", commit)
        self.install_json(os.path.join(self.home, ".local", "bin"))
        p = self.uninstall()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(read(zshrc), "")
        self.assertFalse(os.path.exists(fishf))

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
        self.assertEqual(read(zshrc), "%s\nexport PATH=\"$PATH:$HOME/.local/bin2\"\n%s\n" % (BEGIN, END))

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

    def test_a_link_into_inits_folders_or_to_nothing_is_not_written(self):
        # v0.5.5 (the post-release review of v0.5.4): a startup file that is a link into INIT's own folders (one an
        # earlier agent left) had the block appended to its target, say the new charter.toml or lineage.jsonl, and a
        # dangling link created its target. Now neither is written, the note says why, and it prints the line itself
        # (`echo … >> ~/.zshrc` would write through the same link).
        rh, ih = os.path.join(self.home, ".init-root"), os.path.join(self.home, ".init")
        demo = os.path.join(self.home, ".init-root-demo")  # another install's ROOT_HOME, and the INIT_HOME it names
        demo_ih = os.path.join(self.home, "demo world")
        for d in (rh, ih, os.path.join(demo, "keys"), demo_ih, os.path.join(self.home, ".config", "init-agent")):
            os.makedirs(d)
        with open(os.path.join(demo, "install.json"), "w") as f:
            json.dump({"init_home": demo_ih, "root_home": demo, "v": 1}, f)
        line = 'export PATH="$PATH:$HOME/.local/bin"'
        cases = [  # (shell, OS, startup file, its link's target, the folder named)
            ("zsh", "Darwin", ".zshrc", os.path.join(rh, "charter.toml"), rh),
            ("bash", "Linux", ".bashrc", os.path.join(ih, "lineage.jsonl"), ih),
            ("dash", "Linux", ".profile", os.path.join(demo, "notes"), demo),
            ("bash", "Darwin", ".bash_profile", os.path.join(demo_ih, "x"), demo_ih),
            ("zsh", "Linux", ".zshrc", os.path.join(self.home, ".config", "init-agent", "deepseek"), os.path.join(self.home, ".config", "init-agent")),
        ]
        for shell, os_name, rel, target, folder in cases:
            with self.subTest(rel=rel, target=target):
                with open(target, "w") as f:
                    f.write("# INIT's file\n")
                rc = os.path.join(self.home, rel)
                os.symlink(target, rc)
                out = self.step(shell, os_name=os_name)
                self.assertIn("and ~/%s leads into %s, one of INIT's own folders, so INIT does not write to it. Check it, then"
                              " add this line to your shell's startup file yourself: %s\n" % (rel, folder, line), out)
                self.assertEqual(read(target), "# INIT's file\n")
                os.remove(rc)
        # fish's file through a folder that is a link into INIT_HOME
        os.makedirs(os.path.join(ih, "fish", "conf.d"))
        os.symlink(os.path.join(ih, "fish"), os.path.join(self.home, ".config", "fish"))
        out = self.step("fish")
        self.assertIn("~/.config/fish/conf.d/init-agent.fish leads into %s, one of INIT's own folders" % ih, out)
        self.assertEqual(os.listdir(os.path.join(ih, "fish", "conf.d")), [])
        os.remove(os.path.join(self.home, ".config", "fish"))
        # a dangling link: into ROOT_HOME, and to a folder of the home that does not exist
        for target in (os.path.join(rh, "new.toml"), os.path.join(self.home, "dotfiles", "zshrc")):
            with self.subTest(dangling=target):
                os.symlink(target, os.path.join(self.home, ".zshrc"))
                out = self.step("zsh")
                self.assertIn("and ~/.zshrc is a link to %s, which does not exist, so INIT does not write to it. Check it,"
                              " then add this line to your shell's startup file yourself: %s\n" % (target, line), out)
                self.assertFalse(os.path.lexists(target), "the link's target was created")
                os.remove(os.path.join(self.home, ".zshrc"))
        # v0.5.5 (review B of lane J): compared by file identity, not as text. macOS's file systems ignore case and
        # Unicode normalization, so ~/.INIT-ROOT/charter.toml is the Root's charter; and the service files count too
        with open(os.path.join(rh, "charter.toml"), "w") as f:
            f.write("# the charter\n")
        world = os.path.join(self.home, "w\u00f6rld")  # an INIT_HOME whose name is NFC; the link spells it NFD
        os.makedirs(world)
        with open(os.path.join(demo, "install.json"), "w") as f:
            json.dump({"init_home": world, "root_home": demo, "v": 1}, f)
        with open(os.path.join(world, "state.json"), "w") as f:
            f.write("{}\n")
        plist = os.path.join(self.home, "Library", "LaunchAgents", "dev.init.root.plist")
        unit = os.path.join(self.home, ".config", "systemd", "user", "init-root.service")
        for p_ in (plist, unit):
            os.makedirs(os.path.dirname(p_))
            with open(p_, "w") as f:
                f.write("<service/>\n")
        variants = [(os.path.join(self.home, ".INIT-ROOT", "charter.toml"), os.path.join(rh, "charter.toml"), "leads into %s, one of INIT's own folders" % rh),
                    (os.path.join(self.home, "wo\u0308rld", "state.json"), os.path.join(world, "state.json"), "leads into %s, one of INIT's own folders" % world),
                    (plist, plist, "leads to %s, INIT's service file" % plist), (unit, unit, "leads to %s, INIT's service file" % unit)]
        for spelled, target, why in variants:
            with self.subTest(spelled=spelled):
                if not os.path.exists(spelled):
                    continue  # a file system that tells these names apart (Linux): that link leads nowhere, refused above
                with open(target) as f:
                    before = f.read()
                os.symlink(spelled, os.path.join(self.home, ".zshrc"))
                try:
                    out = self.step("zsh")
                    with open(target) as f:
                        after = f.read()
                    with open(target, "w") as f:
                        f.write(before)
                finally:
                    os.remove(os.path.join(self.home, ".zshrc"))
                self.assertIn("and ~/.zshrc %s, so INIT does not write to it." % why, out)
                self.assertEqual(after, before)
        os.link(os.path.join(rh, "charter.toml"), os.path.join(self.home, ".zshrc"))  # a hard link: no folder on its path
        self.assertIn("and ~/.zshrc has another hard link", self.step("zsh"))
        self.assertEqual(read(os.path.join(rh, "charter.toml")), "# the charter\n")
        os.remove(os.path.join(self.home, ".zshrc"))
        # still written: a link to a file of the owner's in the home (a dotfiles folder)
        os.makedirs(os.path.join(self.home, "dotfiles"))
        mine = os.path.join(self.home, "dotfiles", "zshrc")
        with open(mine, "w") as f:
            f.write("# mine\n")
        os.symlink(mine, os.path.join(self.home, ".zshrc"))
        self.assertIn("added ~/.local/bin to PATH in ~/.zshrc", self.step("zsh"))
        self.assertEqual(read(mine), "# mine\n%s\n%s\n%s\n" % (BEGIN, line, END))


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
