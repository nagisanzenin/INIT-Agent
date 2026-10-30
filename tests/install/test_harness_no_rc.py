"""No harness script lets the installer edit a startup file (v0.5.4; it happened while the candidate was built).

From v0.5.4 install.sh adds a PATH block to the startup file of $SHELL when the bin dir is not on PATH. bundle.sh ran
`env INIT_ROOT_HOME=… INIT_HOME=… INIT_BIN_DIR=… install.sh --reuse --yes --no-service` with the owner's real HOME, and
the block for its scratch bin dir went into the owner's ~/.zshrc (restored by hand). So every scripts/ caller that runs
install.sh or get.sh to install passes --no-modify-path (for an older release's get.sh, which does not know the flag,
only when that get.sh has it; such releases never edit startup files). A static check over scripts/: each such command,
with its continuation lines, names --no-modify-path. Stdlib only.
"""
import os
import re
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SCRIPTS = os.path.join(REPO, "scripts")
# a command that runs the installer: install.sh or a get.sh, given options (comments and messages are not commands)
RUNS = re.compile(r'(scripts/install\.sh"|/bin/sh "\$(getsh|1|REPO/scripts/get\.sh)")\s+--')
# not an install: the installers themselves, and the matrix's runs of NEW's get.sh that must refuse (run_get)
SELF = {"install.sh", "get.sh", "uninstall.sh"}


class HarnessesLeaveStartupFilesAlone(unittest.TestCase):
    def test_every_installer_run_in_scripts_passes_no_modify_path(self):
        found, bad = 0, []
        for d, _, names in os.walk(SCRIPTS):
            for n in names:
                if n in SELF or not (n.endswith(".sh") or "." not in n):
                    continue
                p = os.path.join(d, n)
                try:
                    lines = open(p, encoding="utf-8").read().splitlines()
                except (UnicodeDecodeError, IsADirectoryError):
                    continue
                for i, l in enumerate(lines):
                    if l.lstrip().startswith("#") or not RUNS.search(l) or re.match(r'\s*(say|die|echo|log|printf)\b', l):
                        continue
                    cmd, j = l, i
                    while cmd.rstrip().endswith("\\") and j + 1 < len(lines):
                        j += 1
                        cmd += "\n" + lines[j]
                    found += 1
                    if "--no-modify-path" not in cmd:
                        bad.append("%s:%d: %s" % (os.path.relpath(p, REPO), i + 1, l.strip()))
        self.assertGreaterEqual(found, 5, "the scan found too few installer runs; is the pattern stale?")
        self.assertEqual(bad, [], "installer runs that may edit the owner's startup files:\n" + "\n".join(bad))


if __name__ == "__main__":
    unittest.main()
