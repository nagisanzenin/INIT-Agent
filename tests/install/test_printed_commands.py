"""Every `init-root …` command the installers print runs as printed (v0.5.1; docs/user-sessions/v0.5.0-returning.md).

`init-root` lives in ROOT_HOME/bin and only `init-agent` is on the owner's PATH, so a bare `init-root <command>` fails
with "zsh: command not found: init-root". root/crates/init-root/tests/msg.rs audits the Root's own strings; this is
the same audit for scripts/install.sh, scripts/get.sh, scripts/init-agent and scripts/uninstall.sh (a bare
`init-root <command>`, or a typed-out `…/bin/init-root <command>` with no INIT_ROOT_HOME), plus:
- the scripts' `rcmd` (the Root's util::root_cmd form) prints a command that a shell runs as printed, with this
  install's ROOT_HOME (INIT_ROOT_HOME= in front when it is not ~/.init-root), quoted when the path needs it;
- install.sh's closing "Self-evolution: T0 only" steps stop the Root itself before `toolchain --set` (`init-root stop`
  stops only the agent, and `toolchain --set` needs the Root's lock), then start it again;
- the `init-agent` launcher (v0.5.1, lane SCRIPTS-FIX2): with the Root not running it names this platform's service
  (launchctl on macOS, the systemd user unit on Linux) and a `start --foreground` that runs as printed; `init-agent
  upgrade` with no kept get.sh prints the upgrade that runs the current get.sh with the system's curl and sh, with
  INIT_ROOT_HOME= on the sh side of the pipe when ROOT_HOME is elsewhere;
- install.sh's `left_stopped` (an upgrade whose commit or rollback failed): the Root stays stopped, the message says so
  and how to start it again after the step it names;
- install.sh's closing `Uninstall:` line (v0.5.2) carries INIT_ROOT_HOME= for a ROOT_HOME elsewhere and runs as printed;
- the launcher reads the passwd home itself (v0.5.2): under every bash on this machine (bash 5.3 expands `~` to $HOME
  even with HOME unset), a ROOT_HOME under a HOME that is not the passwd home gets INIT_ROOT_HOME= in what it prints, and
  the LaunchAgent path is the passwd home's;
- `init-agent` too is printed with its path (v0.5.3; the post-release review of v0.5.2): a bare `init-agent upgrade` or
  `init-agent --workspace` is "command not found" on a fresh Mac, whose PATH lacks ~/.local/bin. No script prints the
  bare word as a command; the path is the install's bin dir (install.json `bin_dir`, else INIT_BIN_DIR or
  ~/.local/bin), `~/…` under HOME; each printed line runs as printed in a new terminal whose PATH lacks that directory;
  the launcher hands its own path to the TUI (INIT_AGENT_LAUNCHER), for the TUI's quit line;
- install.sh's PATH hint names the startup file of the owner's shell (v0.5.3): it always said ~/.zshrc, which a Linux
  bash user never reads. Each hint, run as printed, puts the directory on PATH in that shell.
Nothing is installed and no real ROOT_HOME is touched: the functions and the block run alone in a temporary home,
against a stand-in bin/init-root. A printed `/usr/bin/curl … | … /bin/sh …` runs with a script in place of the download
(run_pipe). Stdlib only.
"""
import os
import pwd
import re
import shlex
import shutil
import subprocess
import tempfile
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SCRIPTS = os.path.join(REPO, "scripts")
INSTALL = os.path.join(SCRIPTS, "install.sh")
GET = os.path.join(SCRIPTS, "get.sh")
LAUNCHER = os.path.join(SCRIPTS, "init-agent")
GET_URL = "https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh"
PW_HOME = pwd.getpwuid(os.getuid()).pw_dir  # compared as text only; nothing under it is run or written

# The subcommands of `init-root` (root/crates/init-root/src/cli.rs USAGE; the list tests/msg.rs uses).
SUBCOMMANDS = {"install", "toolchain", "upgrade", "start", "attach", "panic", "resume", "stop", "providers", "models",
               "model", "keys", "egress", "status", "ledger", "charter", "audit", "sealed", "test", "doctor", "exec-clean"}

# Lines that name the program without telling anyone to type it: (file, the exact text around the name).
NAMES_THE_PROGRAM = [
    ("install.sh", "step \"4/7 init-root install ("),             # a step label (the step runs the release's binary)
    ("install.sh", "die \"init-root install failed (exit"),        # what failed
    ("install.sh", "die \"init-root install wrote no "),           # what did not write the service file
    ("install.sh", "(opens the TUI when it is installed, else init-root attach)"),  # what the launcher runs
    ("uninstall.sh", "say \"stopping INIT (init-root stop) …\""),  # what it runs now
    ("uninstall.sh", "say \"  (init-root stop did not answer)\""),
]

# `init-agent` words the scripts print or run that are not a command to type (v0.5.3): (file, the exact text around the
# word). Every other bare `init-agent` is one, and must carry its path. The program's own name before a colon, in its
# messages (`init-agent: …`, `init-agent upgrade: …`), is not a command either.
NAMES_INIT_AGENT = [
    ("install.sh", 'WHY="0.4.1 has no init-agent upgrade"'),            # why the safe line: no such subcommand
    ("install.sh", "0.5.0's own init-agent upgrade runs"),
    ("install.sh", "(from 0.4.1 or 0.5.0, init-agent upgrade does not apply:"),
    ("install.sh", 'put_bin init-agent "$WORK/init-agent"'),             # the file's name
    ("install.sh", "(now init-agent)"),
    ("install.sh", "(the command is init-agent)"),
    ("install.sh", 'step "upgrade 3/5 the init-agent command"'),         # step labels
    ("install.sh", 'step "6/7 the init-agent command"'),
    ("install.sh", "writes the folder init-agent starts in"),             # prose: where it starts, its first run
    ("install.sh", "the first run of init-agent."),
    ("install.sh", "to type just init-agent, add:"),                     # the PATH hint's reason
    ("install.sh", "the folder you start init-agent in."),
    ("get.sh", 'WHY="0.4.1 has no init-agent upgrade"'),
    ("get.sh", "0.5.0's own init-agent upgrade runs"),
    ("uninstall.sh", "for c in init-agent init; do"),                     # the files it moves
]
AGENT_WORD = re.compile(r"(?<![A-Za-z0-9/._$-])init-agent(?![A-Za-z0-9_-])")


def bare_agent(line):
    """The offsets of a bare `init-agent` word in a code line: not part of a path or another word, not a message prefix."""
    text = line.replace("%s", "  ")  # printf's `%sinit-agent`: the word as printed, at the same offset
    return [m.start() for m in AGENT_WORD.finditer(text) if not re.match(r"init-agent( upgrade)?:", text[m.start():])]


def bare_commands(line):
    """The offsets of `init-root <subcommand>` in `line` that read as a command to type (msg.rs bare_commands)."""
    hits = []
    for m in re.finditer(r"init-root ", line):
        i = m.start()
        if i > 0 and (line[i - 1].isalnum() or line[i - 1] in "/-_."):
            continue  # part of a path ("$RH/bin/init-root" start) or another word
        rest = line[m.end():]
        word = re.match(r"[A-Za-z0-9-]*", rest).group(0)
        if word not in SUBCOMMANDS or rest[len(word):].startswith(":") or line[:i].endswith("usage: "):
            continue
        hits.append(i)
    return hits


def code(line):
    """`line` without its trailing shell comment (an unquoted `#` after a blank); quotes are tracked per line."""
    q = None
    for i, c in enumerate(line):
        if q == "'":
            q = None if c == "'" else q
        elif q == '"':
            q = None if c == '"' and line[i - 1] != "\\" else q
        elif c in "'\"":
            q = c
        elif c == "#" and (i == 0 or line[i - 1] in " \t"):
            return line[:i]
    return line


def read(path):
    with open(path) as f:
        return f.read()


def allowed(name, line, at):
    return any(f == name and any(p <= at < p + len(t) for p in (m.start() for m in re.finditer(re.escape(t), line)))
               for f, t in NAMES_THE_PROGRAM)


def function(path, name):
    """The shell function `name` as the script defines it (one line, or up to the `}` at its own indent that closes it)."""
    lines = read(path).splitlines()
    start = next(i for i, l in enumerate(lines) if l.lstrip().startswith(name + "() {"))
    indent = lines[start][:len(lines[start]) - len(lines[start].lstrip())]
    if lines[start].rstrip().endswith("}"):
        return lines[start]
    end = next(i for i in range(start + 1, len(lines)) if lines[i] == indent + "}")
    return "\n".join(lines[start:end + 1])


def functions(path, *names):
    """The functions of `names` that the script defines, in that order."""
    text = read(path)
    return "\n".join(function(path, n) for n in names if re.search(r"^\s*%s\(\) \{" % re.escape(n), text, re.M))


def shq(w):
    """A word as the scripts' shq prints it: as it is when plain, else in single quotes."""
    return w if w and re.fullmatch(r"[A-Za-z0-9._/+=:,@%-]+", w) else "'" + w.replace("'", "'\\''") + "'"


def run_pipe(test, line, url, home):
    """Run a printed `/usr/bin/curl -q -fsSL URL | …`, with a script in place of the download that prints the
    INIT_ROOT_HOME and the arguments it was started with, as the owner would in a new terminal (no INIT_ROOT_HOME)."""
    head = "/usr/bin/curl -q -fsSL %s | " % shq(url)
    test.assertTrue(line.startswith(head), line)
    script = 'echo "home=${INIT_ROOT_HOME-unset}|args=$*"'
    p = subprocess.run(["/bin/sh", "-c", "printf '%s\\n' " + shlex.quote(script) + " | " + line[len(head):]],
                       env={"PATH": "/usr/bin:/bin", "HOME": home}, capture_output=True, text=True, timeout=30)
    test.assertEqual(p.returncode, 0, line + "\n" + p.stderr)
    return p.stdout.strip()


def stub(rh):
    """A stand-in ROOT_HOME/bin/init-root that prints what it was given."""
    os.makedirs(os.path.join(rh, "bin"), exist_ok=True)
    exe = os.path.join(rh, "bin", "init-root")
    with open(exe, "w") as f:
        f.write('#!/bin/sh\nprintf "home=%s|exe=%s|args=%s\\n" "${INIT_ROOT_HOME-unset}" "$0" "$*"\n')
    os.chmod(exe, 0o755)
    return exe


class PrintedCommands(unittest.TestCase):
    def test_no_script_prints_a_bare_init_root_command(self):
        bad, used = [], set()
        for name in ("install.sh", "get.sh", "init-agent", "uninstall.sh"):
            for n, line in enumerate(read(os.path.join(SCRIPTS, name)).splitlines(), 1):
                line = code(line)
                # a path typed out (`$RH/bin/init-root keys set …`) names no INIT_ROOT_HOME: wrong for another ROOT_HOME.
                # A command the script runs quotes its path ("$RH/bin/init-root" status) and passes the home itself.
                if any(m.group(1) in SUBCOMMANDS for m in re.finditer(r"/init-root ([A-Za-z0-9-]+)", line)):
                    bad.append("%s:%d: %s" % (name, n, line.strip()))
                for at in bare_commands(line):
                    if allowed(name, line, at):
                        used.update(t for f, t in NAMES_THE_PROGRAM if f == name and t in line)
                    else:
                        bad.append("%s:%d: %s" % (name, n, line.strip()))
        self.assertEqual(bad, [], "bare `init-root` commands (not on the owner's PATH; print them with rcmd):\n" + "\n".join(bad))
        self.assertEqual({t for _, t in NAMES_THE_PROGRAM} - used, set(), "an exemption that no longer matches")

    def test_no_script_prints_a_bare_init_agent_command(self):
        bad, used = [], set()
        for name in ("install.sh", "get.sh", "init-agent", "uninstall.sh", "qualify-provider.sh"):
            for n, line in enumerate(read(os.path.join(SCRIPTS, name)).splitlines(), 1):
                line = code(line)
                for at in bare_agent(line):
                    hit = {(f, t) for f, t in NAMES_INIT_AGENT if f == name
                           and any(p <= at < p + len(t) for p in (m.start() for m in re.finditer(re.escape(t), line)))}
                    if hit:
                        used |= hit
                    else:
                        bad.append("%s:%d: %s" % (name, n, line.strip()))
        self.assertEqual(bad, [], "bare `init-agent` commands (~/.local/bin is not on a fresh Mac's PATH; print the path):\n"
                         + "\n".join(bad))
        self.assertEqual(set(NAMES_INIT_AGENT) - used, set(), "an exemption that no longer matches")

    def test_the_init_agent_scanner(self):
        for hit in ('say "Open INIT: init-agent"', "printf '%sinit-agent upgrade' \"$(rh_env)\"", 'X="$(rh_env)init-agent upgrade"',
                    'echo "(see init-agent upgrade --help)"', 'say "To confine it: init-agent --workspace"'):
            self.assertEqual(len(bare_agent(hit)), 1, hit)
        for ok in ('say "$BIN/init-agent"', 'echo "init-agent: refusing it" >&2', 'echo "init-agent upgrade: --bundle needs a file"',
                   "HELPERS=/usr/local/lib/init-agent", "'profile init-agent-unshare $HELPERS/unshare'", 'say "$(tilde "$B/init-agent") upgrade"',
                   'KEY="$H/.config/init-agent/deepseek.key"'):
            self.assertEqual(bare_agent(ok), [], ok)

    def test_the_scanner(self):
        self.assertEqual(len(bare_commands('say "then init-root charter check --apply."')), 1)
        self.assertEqual(bare_commands('say "  $RH/bin/init-root stop; init-root-x start"'), [])
        self.assertEqual(bare_commands('die "init-root upgrade: the Root is running"'), [])
        self.assertEqual(bare_commands('for f in init-root init-kernel initctl; do'), [])
        self.assertEqual(code('X="a # b" # run init-root stop'), 'X="a # b" ')
        self.assertEqual(code("  # init-root stop"), "  ")
        self.assertEqual(code('echo "${#h} \\" # x"'), 'echo "${#h} \\" # x"')

    def rcmd(self, script, shell, pw_home, rh, args):
        """`rcmd args` as `script` defines it, with its ROOT_HOME `rh` (and, for install.sh, its PW_HOME)."""
        src = "\n".join([functions(script, "shq", "rh_env", "rcmd"), 'rcmd "$@"'])
        env = {"PATH": "/usr/bin:/bin", "RH": rh, "PW_HOME": pw_home, "LANG": "C"}
        p = subprocess.run([shell, "-c", 'RH="$RH"; PW_HOME="$PW_HOME"\n' + src, "rcmd", *args], env=env,
                           capture_output=True, text=True, timeout=30)
        self.assertEqual(p.returncode, 0, p.stderr)
        return p.stdout

    def run_printed(self, cmd, home):
        """Run a printed command as the owner would in a new terminal: HOME is theirs, no INIT_ROOT_HOME set."""
        p = subprocess.run(["/bin/sh", "-c", cmd], env={"PATH": "/usr/bin:/bin", "HOME": home}, capture_output=True,
                           text=True, timeout=30)
        self.assertEqual(p.returncode, 0, cmd + "\n" + p.stderr)
        return p.stdout.strip()

    def test_rcmd_prints_a_command_that_runs_with_this_root_home(self):
        with tempfile.TemporaryDirectory(prefix="i5-int-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None) as t:
            pw = os.path.join(t, "home")
            default = os.path.join(pw, ".init-root")
            other = os.path.join(t, "i5 int's", "rh")
            for rh in (default, other):
                stub(rh)
            # install.sh: PW_HOME is the passwd home it read; the default ROOT_HOME prints its binary alone
            for rh, want in ((default, "home=unset|exe=%s/bin/init-root|args=keys set deepseek" % default),
                             (other, "home=%s|exe=%s/bin/init-root|args=keys set deepseek" % (other, other))):
                cmd = self.rcmd(INSTALL, "/bin/bash", pw, rh, ["keys", "set", "deepseek"])
                self.assertEqual(self.run_printed(cmd, pw), want, cmd)
            self.assertEqual(self.rcmd(INSTALL, "/bin/bash", pw, default, ["stop"]), default + "/bin/init-root stop")
            self.assertEqual(self.rcmd(INSTALL, "/bin/bash", "", default, ["stop"]),
                             "INIT_ROOT_HOME=%s %s/bin/init-root stop" % (default, default), "no passwd home: the long form")
            # get.sh reads the passwd home itself; a ROOT_HOME elsewhere names itself
            cmd = self.rcmd(GET, "/bin/sh", "", other, ["keys", "set", "deepseek"])
            self.assertTrue(cmd.startswith("INIT_ROOT_HOME="), cmd)
            self.assertEqual(self.run_printed(cmd, pw), "home=%s|exe=%s/bin/init-root|args=keys set deepseek" % (other, other))
            import pwd
            real = os.path.join(pwd.getpwuid(os.getuid()).pw_dir, ".init-root")  # compared as text only, never run
            self.assertEqual(self.rcmd(GET, "/bin/sh", "", real, ["toolchain", "--set"]), real + "/bin/init-root toolchain --set")
            # rh_env alone: nothing for the passwd home's ~/.init-root, INIT_ROOT_HOME= for any other
            for script, shell in ((GET, "/bin/sh"), (INSTALL, "/bin/bash")):
                for rh, want in ((os.path.join(PW_HOME, ".init-root"), ""), (other, "INIT_ROOT_HOME=%s " % shq(other))):
                    src = "\n".join([functions(script, "shq", "rh_env"), "rh_env"])
                    p = subprocess.run([shell, "-c", src], env={"PATH": "/usr/bin:/bin", "RH": rh, "PW_HOME": PW_HOME},
                                       capture_output=True, text=True, timeout=30)
                    self.assertEqual((p.returncode, p.stdout), (0, want), (script, rh, p.stderr))

    def test_the_toolchain_steps_stop_the_root_itself(self):
        text = read(INSTALL)
        m = re.search(r'^if \[ "\$TC" = none \]; then\n.*?^fi$', text, re.S | re.M)
        self.assertIsNotNone(m, "install.sh has no closing T0-only block")
        helpers = functions(INSTALL, "shq", "rh_env", "rcmd")
        cases = {("1", "Darwin"): ("Ctrl-C in the Root's terminal", "start --foreground"),
                 ("0", "Darwin"): ("launchctl bootout gui/%d/dev.init.root" % os.getuid(),
                                   "launchctl bootstrap gui/%d /h/Library/LaunchAgents/dev.init.root.plist" % os.getuid()),
                 ("0", "Linux"): ("systemctl --user stop init-root.service", "systemctl --user start init-root.service")}
        for (nosvc, osname), (stop, start) in cases.items():
            env = {"PATH": "/usr/bin:/bin", "LANG": "C", "TC": "none", "NO_SERVICE": nosvc, "OS": osname, "PW_HOME": "/h",
                   "RH": "/h/.init-root", "RUSTUP_CMD": "curl … | sh", "PLIST": "/h/Library/LaunchAgents/dev.init.root.plist"}
            p = subprocess.run(["/bin/bash", "-c", helpers + "\n" + m.group(0)], env=env, capture_output=True, text=True,
                               timeout=30)
            self.assertEqual(p.returncode, 0, p.stderr)
            out = p.stdout
            lines = out.splitlines()
            at = lambda s: next((i for i, l in enumerate(lines) if s in l), -1)
            tc = at("/h/.init-root/bin/init-root toolchain --set")
            self.assertTrue(0 <= at(stop) < tc < at(start), "%s %s: stop the Root, then --set, then start it:\n%s" % (nosvc, osname, out))
            self.assertNotIn("init-root stop", out, out)  # stops only the agent: --set would be refused

    def test_an_upgrade_left_half_way_says_the_root_stays_stopped(self):
        # install.sh --upgrade whose commit (or rollback) failed: not started again (both steps take run/root.lock),
        # so the message says so, names the step to run, then how to start the Root for this install
        helpers = functions(INSTALL, "shq", "rh_env", "rcmd", "left_stopped")
        uid = os.getuid()
        cases = {("0", "Darwin"): ("The Root is not running", "/h/.init-root/bin/init-root start --foreground"),
                 ("1", "Darwin"): ("The Root's LaunchAgent stays stopped",
                                   "launchctl bootstrap gui/%d '/h/Library/Launch Agents/dev.init.root.plist'" % uid),
                 ("1", "Linux"): ("The Root's systemd user unit stays stopped", "systemctl --user start init-root.service")}
        for (svc, osname), (state, start) in cases.items():
            env = {"PATH": "/usr/bin:/bin", "LANG": "C", "SVC": svc, "OS": osname, "PW_HOME": "/h", "RH": "/h/.init-root",
                   "SVCF": "/h/Library/Launch Agents/dev.init.root.plist"}
            p = subprocess.run(["/bin/bash", "-c", helpers + '\nleft_stopped "$(rcmd upgrade --commit)" "back: $(rcmd upgrade --rollback)"'],
                               env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(p.returncode, 0, p.stderr)
            lines = p.stdout.splitlines()
            self.assertTrue(lines[0].startswith(state) and lines[0].endswith(", and this step needs it stopped. Run:"), p.stdout)
            self.assertEqual(lines[1:], ["  /h/.init-root/bin/init-root upgrade --commit",
                                         "  (or back: /h/.init-root/bin/init-root upgrade --rollback)",
                                         "then start the Root: " + start + ("   (in a terminal of its own)" if svc == "0" else "")],
                             p.stdout)

    def test_the_closing_uninstall_line_runs_with_this_root_home(self):
        text = read(INSTALL)
        m = re.search(r"^ELAPSED=.*?^EOF$", text, re.S | re.M)
        self.assertIsNotNone(m, "install.sh has no closing summary")
        helpers = functions(INSTALL, "shq", "rh_env", "rcmd", "tilde", "path_add")
        with tempfile.TemporaryDirectory(prefix="i5-rp52-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None) as t:
            repo = os.path.join(t, "re po's")  # the bundle's directory, quoted when printed
            os.makedirs(os.path.join(repo, "scripts"))
            with open(os.path.join(repo, "scripts", "uninstall.sh"), "w") as f:
                f.write('echo "home=${INIT_ROOT_HOME-unset}|args=$*"\n')
            pw = os.path.join(t, "pw")
            for rh, want in ((os.path.join(t, "o r"), "home=%s|args=" % os.path.join(t, "o r")),
                             (os.path.join(pw, ".init-root"), "home=unset|args=")):
                env = {"PATH": "/usr/bin:/bin", "LANG": "C", "RH": rh, "IH": os.path.join(t, "ih"), "PW_HOME": pw,
                       "REPO": repo, "VERSION": "9.9.9", "T_START": "0"}
                p = subprocess.run(["/bin/bash", "-c", helpers + "\n" + m.group(0)], env=env, capture_output=True,
                                   text=True, timeout=30)
                self.assertEqual(p.returncode, 0, p.stderr)
                line = next(l for l in p.stdout.splitlines() if l.startswith("Uninstall:"))
                cmd = line[len("Uninstall:"):].split("   (moves")[0].strip()
                self.assertEqual(cmd.startswith("INIT_ROOT_HOME="), rh != os.path.join(pw, ".init-root"), line)
                self.assertEqual(self.run_printed(cmd, pw), want, cmd)

    def stand_in(self, bin_dir):
        """A stand-in init-agent in bin_dir that prints the INIT_ROOT_HOME and the arguments it was started with."""
        os.makedirs(bin_dir, exist_ok=True)
        exe = os.path.join(bin_dir, "init-agent")
        with open(exe, "w") as f:
            f.write('#!/bin/sh\necho "home=${INIT_ROOT_HOME-unset}|args=$*"\n')
        os.chmod(exe, 0o755)
        return exe

    def test_the_init_agent_command_runs_as_printed(self):
        # get.sh's and install.sh's `agent`: the launcher in the bin_dir install.json records, else BIN_DIR, written
        # `~/…` under HOME; it runs as printed in a new terminal whose PATH lacks that directory
        with tempfile.TemporaryDirectory(prefix="i5-l53b-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None) as t:
            home = os.path.join(t, "home")
            rh = os.path.join(t, "rh")
            os.makedirs(rh)
            default = os.path.join(home, ".local", "bin")
            self.stand_in(default)
            cases = [(None, "~/.local/bin/init-agent"),                                       # an older install.json
                     (os.path.join(t, "my bin"), shq(os.path.join(t, "my bin", "init-agent"))),  # INIT_BIN_DIR elsewhere
                     (os.path.join(home, "apps bin"), "~/'apps bin/init-agent'"),               # under HOME, quoted
                     (os.path.join(t, "gone"), "~/.local/bin/init-agent")]                      # recorded, but no launcher there
            for bin_dir, want in cases:
                if bin_dir and not bin_dir.endswith("gone"):
                    self.stand_in(bin_dir)
                with open(os.path.join(rh, "install.json"), "w") as f:
                    f.write('{"init_home":"/x","v":1%s}\n' % (',"bin_dir":"%s"' % bin_dir if bin_dir else ""))
                for script, shell in ((GET, "/bin/sh"), (INSTALL, "/bin/bash")):
                    with self.subTest(script=os.path.basename(script), bin_dir=bin_dir):
                        p = subprocess.run([shell, "-c", functions(script, "shq", "tilde", "agent") + "\nagent"],
                                           env={"PATH": "/usr/bin:/bin", "HOME": home, "RH": rh, "BIN_DIR": default, "LANG": "C"},
                                           capture_output=True, text=True, timeout=30)
                        self.assertEqual((p.returncode, p.stdout), (0, want), p.stderr)
                        self.assertEqual(self.run_printed(want + " upgrade --version 0.5.3", home), "home=unset|args=upgrade --version 0.5.3")
            # tilde alone: a HOME of / or none writes the absolute path; the launcher's own tilde is the same
            for script, shell in ((GET, "/bin/sh"), (INSTALL, "/bin/bash"), (LAUNCHER, "/bin/bash")):
                for h, path, want in (("/", "/opt/b/init-agent", "/opt/b/init-agent"), ("", "/opt/b/init-agent", "/opt/b/init-agent"),
                                      (home + "/", home + "/b/init-agent", "~/b/init-agent"), (home, home, shq(home)),
                                      (home, home + "x/init-agent", shq(home + "x/init-agent"))):
                    p = subprocess.run([shell, "-c", functions(script, "shq", "tilde") + '\ntilde "$1"', "tilde", path],
                                       env={"PATH": "/usr/bin:/bin", "HOME": h}, capture_output=True, text=True, timeout=30)
                    self.assertEqual((p.returncode, p.stdout), (0, want), (script, h, path, p.stderr))

    def closing(self, env):
        """install.sh's closing summary and its access lines, run alone with `env`."""
        m = re.search(r"^ELAPSED=.*?^fi$", read(INSTALL), re.S | re.M)
        self.assertIsNotNone(m, "install.sh has no closing summary")
        helpers = functions(INSTALL, "shq", "rh_env", "rcmd", "tilde", "agent", "path_add")
        base = {"PATH": "/usr/bin:/bin", "LANG": "C", "VERSION": "9.9.9", "T_START": "0", "REPO": "/r", "IH": "/ih",
                "ACCESS_EFF": "full", "SHELL": "/bin/zsh", "OS": "Darwin"}
        base.update(env)
        p = subprocess.run(["/bin/bash", "-c", helpers + "\n" + m.group(0)], env=base, capture_output=True, text=True, timeout=30)
        self.assertEqual((p.returncode, p.stderr), (0, ""))
        return p.stdout

    def test_the_closing_lines_name_init_agent_by_its_path(self):
        with tempfile.TemporaryDirectory(prefix="i5-l53b-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None) as t:
            home = os.path.join(t, "home")
            bin_dir = os.path.join(home, ".local", "bin")
            self.stand_in(bin_dir)
            for rh, env_part in ((os.path.join(home, ".init-root"), ""), (os.path.join(t, "o r"), "INIT_ROOT_HOME=%s " % shq(os.path.join(t, "o r")))):
                out = self.closing({"HOME": home, "PW_HOME": home, "RH": rh, "BIN_DIR": bin_dir})
                lines = out.splitlines()
                at = lambda head: next(l for l in lines if l.startswith(head))
                self.assertTrue(at("Next: ").startswith("Next: ~/.local/bin/init-agent   ("), out)
                up = at("Upgrade:     ")[len("Upgrade:     "):].split(" (keeps")[0]
                self.assertEqual(up, env_part + "~/.local/bin/init-agent upgrade", out)
                self.assertEqual(self.run_printed(up, home), "home=%s|args=upgrade" % (rh if env_part else "unset"))
                confine = re.search(r"To confine it: (.*?), /full off", out.replace("\n", " "))
                self.assertIsNotNone(confine, out)
                self.assertEqual(confine.group(1), "~/.local/bin/init-agent --workspace")
                self.assertEqual(self.run_printed(confine.group(1), home), "home=unset|args=--workspace")
                self.assertIsNone(re.search(r"(^|[\s(:])init-agent (upgrade|--)", out, re.M), out)

    def test_the_path_hint_names_this_shells_startup_file(self):
        rc = lambda f: "echo 'export PATH=\"$HOME/.local/bin:$PATH\"' >> " + f
        with tempfile.TemporaryDirectory(prefix="i5-l53b-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None) as t:
            home = os.path.join(t, "home")
            bin_dir = os.path.join(home, ".local", "bin")
            os.makedirs(bin_dir)
            cases = [("/bin/zsh", "Darwin", {}, [], rc("~/.zshrc")),
                     ("/usr/bin/zsh", "Linux", {"ZDOTDIR": os.path.join(home, ".config", "zsh")}, [], rc("~/.config/zsh/.zshrc")),
                     ("/bin/bash", "Linux", {}, [], rc("~/.bashrc")),
                     ("/usr/bin/bash", "Linux", {}, [".bash_profile"], rc("~/.bashrc")),
                     ("/bin/bash", "Darwin", {}, [], rc("~/.bash_profile")),
                     ("/opt/homebrew/bin/bash", "Darwin", {}, [".profile"], rc("~/.profile")),      # never hide ~/.profile
                     ("/bin/bash", "Darwin", {}, [".bash_login", ".profile"], rc("~/.bash_login")),
                     ("/bin/bash", "Darwin", {}, [".bash_profile", ".profile"], rc("~/.bash_profile")),
                     ("/usr/local/bin/fish", "Darwin", {}, [], "fish_add_path ~/.local/bin"),
                     ("/usr/bin/fish", "Linux", {}, [], "fish_add_path ~/.local/bin"),
                     ("/bin/sh", "Linux", {}, [], rc("~/.profile")),
                     ("/bin/dash", "Linux", {}, [], rc("~/.profile")),
                     ("/bin/ksh", "Darwin", {}, [], rc("~/.profile"))]
            for shell, osname, extra, files, want in cases:
                with self.subTest(shell=shell, os=osname, files=files):
                    for f in files:
                        open(os.path.join(home, f), "w").close()
                    env = dict({"HOME": home, "PW_HOME": home, "RH": os.path.join(home, ".init-root"), "BIN_DIR": bin_dir,
                                "SHELL": shell, "OS": osname}, **extra)
                    out = self.closing(env)
                    self.assertIn("(%s is not on your PATH; to type just init-agent, add: %s)" % (bin_dir, want), out)
                    # the step 6 note says the same
                    note = re.search(r'^case ":\$PATH:" in \*":\$BIN_DIR:"\*\) ;; \*\) say "  note: .*$', read(INSTALL), re.M)
                    self.assertIsNotNone(note, "install.sh has no step 6 PATH note")
                    p = subprocess.run(["/bin/bash", "-c", functions(INSTALL, "shq", "tilde", "path_add") + "\nsay() { printf '%s\\n' \"$*\"; }\n"
                                        + note.group(0)], env=dict(env, PATH="/usr/bin:/bin"), capture_output=True, text=True, timeout=30)
                    self.assertEqual(p.stdout, "  note: %s is not on your PATH; add it: %s\n" % (bin_dir, want), p.stderr)
                    for f in files:
                        os.remove(os.path.join(home, f))
            # the hint, run as printed, puts the directory on PATH in that shell's new terminals
            for shell, osname, start in (("/bin/zsh", "Darwin", "source ~/.zshrc"), ("/bin/bash", "Linux", ". ~/.bashrc"),
                                         ("/bin/bash", "Darwin", ". ~/.bash_profile"), ("/bin/dash", "Linux", ". ~/.profile")):
                if not os.access(shell, os.X_OK):
                    continue
                with self.subTest(run=shell, os=osname):
                    out = self.closing({"HOME": home, "PW_HOME": home, "RH": os.path.join(home, ".init-root"), "BIN_DIR": bin_dir,
                                        "SHELL": shell, "OS": osname})
                    hint = re.search(r"to type just init-agent, add: (.*)\)$", out, re.M).group(1)
                    self.run_printed(hint, home)
                    p = subprocess.run([shell, "-c", start + ' && printf %s "$PATH"'], env={"PATH": "/usr/bin:/bin", "HOME": home},
                                       capture_output=True, text=True, timeout=30)
                    self.assertEqual(p.stdout.split(":")[0], bin_dir, p.stderr)
                    for f in (".zshrc", ".bashrc", ".bash_profile", ".profile"):
                        if os.path.exists(os.path.join(home, f)):
                            os.remove(os.path.join(home, f))


class LauncherPrints(unittest.TestCase):
    """What `init-agent` prints when it cannot hand over: this platform's commands, each runnable as printed."""

    def setUp(self):
        self.t = tempfile.mkdtemp(prefix="i5-sc2-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", self.t], check=False)
        self.home = os.path.join(self.t, "home")
        self.rh = os.path.join(self.t, "i5 sc2's", "rh")  # elsewhere than ~/.init-root, and a path that needs quotes
        os.makedirs(self.home)
        stub(self.rh)

    def launch(self, *args, ostype=None):
        env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": self.home, "INIT_ROOT_HOME": self.rh, "LANG": "C"}
        if ostype:
            env["OSTYPE"] = ostype  # bash keeps an OSTYPE from the environment: the other platform's branch, here
        p = subprocess.run(["/bin/bash", LAUNCHER, *args], env=env, stdin=subprocess.DEVNULL, capture_output=True,
                           text=True, timeout=30)
        self.assertEqual(p.returncode, 69, p.stdout + p.stderr)
        return p.stderr

    def runs_as_printed(self, cmd):
        p = subprocess.run(["/bin/sh", "-c", cmd], env={"PATH": "/usr/bin:/bin", "HOME": self.home}, capture_output=True,
                           text=True, timeout=30)
        self.assertEqual(p.returncode, 0, cmd + "\n" + p.stderr)
        return p.stdout.strip()

    def test_root_not_running_names_this_platforms_service(self):
        uid = os.getuid()
        plist = shq(os.path.join(PW_HOME, "Library", "LaunchAgents", "dev.init.root.plist"))
        for ostype, want, never in (("linux-gnu", ["  with the service: systemctl --user start init-root.service",
                                                   "    (if systemd no longer knows the unit: systemctl --user daemon-reload"
                                                   " && systemctl --user start init-root.service)"], "launchctl"),
                                    ("darwin23", ["  with the service: launchctl kickstart gui/%d/dev.init.root" % uid,
                                                  "    (if launchd no longer knows the job: launchctl bootstrap gui/%d %s)" % (uid, plist)],
                                     "systemctl")):
            e = self.launch(ostype=ostype)
            lines = e.splitlines()
            self.assertEqual(lines[0], "init-agent: the Root is not running (%s/run/owner.sock is missing)." % self.rh)
            self.assertEqual(lines[1:3], want, e)
            self.assertNotIn(never, e)
            self.assertNotIn("~/", e, "a path the shell would not expand where it is quoted")
            fg = lines[3]
            self.assertTrue(fg.startswith("  without it:       ") and fg.endswith("   (in another terminal)"), fg)
            cmd = fg[len("  without it:       "):-len("   (in another terminal)")]
            self.assertEqual(self.runs_as_printed(cmd), "home=%s|exe=%s/bin/init-root|args=start --foreground" % (self.rh, self.rh))

    def test_upgrade_without_a_kept_get_sh_prints_the_safe_upgrade(self):
        os.makedirs(os.path.join(self.rh, "cache"))
        with open(os.path.join(self.rh, "install.json"), "w") as f:
            f.write("{}\n")
        e = self.launch("upgrade")
        self.assertIn("get.sh is missing", e)
        self.assertIn("is 0.4.1, or its copy was removed", e)
        self.assertNotIn("and earlier", e)
        line = e.splitlines()[-1]
        self.assertEqual(line, "  /usr/bin/curl -q -fsSL %s | INIT_ROOT_HOME=%s /bin/sh -s -- --upgrade" % (GET_URL, shq(self.rh)))
        self.assertEqual(run_pipe(self, line.strip(), GET_URL, self.home), "home=%s|args=--upgrade" % self.rh)

    def test_the_passwd_home_under_every_bash(self):
        # ROOT_HOME is the scratch HOME's ~/.init-root: not the passwd home's, so every printed line needs INIT_ROOT_HOME=
        rh = os.path.join(self.home, ".init-root")
        stub(rh)
        bashes = {}
        for b in ("/bin/bash", "/usr/bin/bash", "/opt/homebrew/bin/bash", "/usr/local/bin/bash", shutil.which("bash")):
            if b and os.access(b, os.X_OK):
                bashes.setdefault(os.path.realpath(b), b)
        plist = shq(os.path.join(PW_HOME, "Library", "LaunchAgents", "dev.init.root.plist"))
        for bash in bashes.values():
            version = subprocess.run([bash, "-c", "echo $BASH_VERSION"], capture_output=True, text=True).stdout.strip()
            with self.subTest(bash=bash, version=version):
                env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": self.home, "LANG": "C", "OSTYPE": "darwin23"}
                p = subprocess.run([bash, LAUNCHER], env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                                   timeout=30)
                self.assertEqual(p.returncode, 69, p.stderr)
                lines = p.stderr.splitlines()
                self.assertIn("launchctl bootstrap gui/%d %s)" % (os.getuid(), plist), lines[2])
                fg = lines[3][len("  without it:       "):-len("   (in another terminal)")]
                self.assertTrue(fg.startswith("INIT_ROOT_HOME="), fg)
                self.assertEqual(self.runs_as_printed(fg), "home=%s|exe=%s/bin/init-root|args=start --foreground" % (rh, rh))

    def test_upgrade_with_nothing_installed_says_so(self):
        e = self.launch("upgrade")
        self.assertIn("INIT is not installed in %s (no install.json there), so there is nothing to upgrade" % self.rh, e)
        line = e.splitlines()[-1]
        self.assertEqual(line, "  /usr/bin/curl -q -fsSL %s | INIT_ROOT_HOME=%s /bin/sh" % (GET_URL, shq(self.rh)))
        self.assertEqual(run_pipe(self, line.strip(), GET_URL, self.home), "home=%s|args=" % self.rh)

    def installed_launcher(self):
        """The launcher in the scratch HOME's ~/.local/bin, as install.sh puts it."""
        b = os.path.join(self.home, ".local", "bin")
        os.makedirs(b, exist_ok=True)
        shutil.copy(LAUNCHER, os.path.join(b, "init-agent"))
        return os.path.join(b, "init-agent")

    def test_the_upgrade_usage_names_the_launcher_by_its_path(self):
        exe = self.installed_launcher()
        os.makedirs(os.path.join(self.rh, "cache"))
        with open(os.path.join(self.rh, "cache", "get.sh"), "w") as f:
            f.write("exit 0\n")
        env = {"PATH": "/usr/bin:/bin", "HOME": self.home, "INIT_ROOT_HOME": self.rh, "LANG": "C"}
        p = subprocess.run(["/bin/bash", exe, "upgrade", "--bogus"], env=env, stdin=subprocess.DEVNULL, capture_output=True,
                           text=True, timeout=30)
        self.assertEqual(p.returncode, 64, p.stderr)
        self.assertEqual(p.stderr, "init-agent upgrade: unknown argument --bogus (see ~/.local/bin/init-agent upgrade --help)\n")
        p = subprocess.run(["/bin/sh", "-c", "~/.local/bin/init-agent upgrade --help"], env=dict(env, PATH="/usr/bin:/bin"),
                           capture_output=True, text=True, timeout=30)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn("init-agent upgrade [--version V | --bundle FILE.tar.gz --sha256 HEX]", p.stdout)

    def test_the_tui_gets_the_launchers_path(self):
        # the TUI prints `init-agent` in its quit line; the launcher tells it where it is
        exe = self.installed_launcher()
        tui = os.path.join(self.t, "tui")
        with open(tui, "w") as f:
            f.write('#!/bin/sh\nprintf "%s|%s" "${INIT_AGENT_LAUNCHER-unset}" "$*"\n')
        os.chmod(tui, 0o755)
        for argv0 in (exe, os.path.join(self.home, ".local", "..", ".local", "bin", "init-agent")):
            p = subprocess.run(["/bin/bash", argv0, "--plain"], env={"PATH": "/usr/bin:/bin", "HOME": self.home, "INIT_ROOT_HOME": self.rh,
                               "INIT_TUI": tui, "LANG": "C"}, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
            self.assertEqual((p.returncode, p.stdout), (0, os.path.realpath(exe) + "|--plain"), p.stderr)


if __name__ == "__main__":
    unittest.main()
