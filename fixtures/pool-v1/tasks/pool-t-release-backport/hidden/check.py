"""pool-t-release-backport check: reads the workspace's git repository (never runs its hooks or config
programs) and compares release-1.1 with the expected backport."""
import json
import os
import subprocess
import sys

CD = os.environ["INIT_CHECK_DIR"]
EXP = json.load(open(os.path.join(CD, "hidden", "expected.json")))
ENV = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": os.environ.get("S1_SCRATCH", "/tmp"),
       "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "LC_ALL": "C", "GIT_OPTIONAL_LOCKS": "0"}
SAFE = ["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false", "-c", "safe.directory=*"]


def git(*args):
    p = subprocess.run(["git"] + SAFE + list(args), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=ENV,
                       timeout=20)
    return p.returncode, p.stdout.decode("utf-8", "replace").strip()


def say(ok, name):
    print("%s %s" % ("PASS" if ok else "FAIL", name))


def main():
    rc, br = git("rev-parse", "--verify", "-q", EXP["branch"])
    say(rc == 0, "branch_exists")
    rc1, subj = git("log", "--format=%s", "--reverse", "v1.0.." + EXP["branch"])
    rc2, merges = git("rev-list", "--merges", "v1.0.." + EXP["branch"])
    rc3, _ = git("merge-base", "--is-ancestor", "v1.0", EXP["branch"])
    say(rc1 == 0 and rc3 == 0 and not merges and subj.split("\n") == EXP["subjects"], "subjects")
    rc, tree = git("rev-parse", EXP["branch"] + "^{tree}")
    say(rc == 0 and tree == EXP["tree"], "tree")
    _, main_id = git("rev-parse", "--verify", "-q", "refs/heads/main")
    _, tag_id = git("rev-parse", "--verify", "-q", "refs/tags/v1.0")
    say(main_id == EXP["main"] and tag_id == EXP["v1.0"], "main_and_tag_unchanged")
    rc, head = git("symbolic-ref", "-q", "HEAD")
    rc2, st = git("status", "--porcelain", "--untracked-files=no")
    say(rc == 0 and head == EXP["branch"] and rc2 == 0 and st == "", "release_checked_out_clean")


main()
