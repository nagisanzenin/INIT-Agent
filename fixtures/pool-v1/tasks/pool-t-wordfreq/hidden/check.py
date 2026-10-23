"""pool-t-wordfreq check: runs the workspace's bin/wordfreq on hidden inputs (through its own shebang, cwd = a
scratch dir) and compares stdout, stderr and the exit status with README.md's contract."""
import os
import subprocess
import sys

sys.path.insert(0, os.path.join(os.environ["INIT_CHECK_DIR"], "hidden"))
import model  # noqa: E402

WS = os.getcwd()
PROG = os.path.join(WS, "bin", "wordfreq")
SCRATCH = os.environ["S1_SCRATCH"]
ENV = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": SCRATCH, "LC_ALL": "C", "TMPDIR": SCRATCH}
TEXTS = {
    "t1.txt": "Apple banana apple. BANANA? apple; cherry\n",
    "t2.txt": "It's its own thing; it's 'its' -- isn't it? rock'n'roll x'y 'z' a'",
    "t3.txt": "zeta alpha beta gamma delta alpha beta gamma alpha beta zeta\n",
    "t4.txt": "caf\xe9 na\xefve r\xe9sum\xe9 caf\xe9 stra\xdfe\n",
    "t5.txt": "under_score dash-ed tab\tsep 3rd 4x4 MiXeD mixed MIXED\n",
}


def run(args, stdin=b""):
    try:
        p = subprocess.run([PROG] + args, cwd=SCRATCH, env=ENV, input=stdin, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, timeout=10)
    except OSError as e:
        return None, "", str(e)
    return p.returncode, p.stdout.decode("latin-1"), p.stderr.decode("latin-1")


def say(name, ok):
    print("%s %s" % ("PASS" if ok else "FAIL", name))


def main():
    for name, text in TEXTS.items():
        with open(os.path.join(SCRATCH, name), "wb") as f:
            f.write(text.encode("utf-8"))
    T = TEXTS
    say("default_n", run(["t3.txt"])[:2] == (0, model.top([T["t3.txt"]], 10)))
    say("n_two", run(["-n", "2", "t1.txt"])[:2] == (0, model.top([T["t1.txt"]], 2)))
    say("ties_by_word", run(["-n", "4", "t3.txt"])[:2] == (0, model.top([T["t3.txt"]], 4)))
    say("apostrophes", run(["t2.txt"])[:2] == (0, model.top([T["t2.txt"]], 10)))
    say("non_ascii_splits", run(["t4.txt"])[:2] == (0, model.top([T["t4.txt"]], 10)))
    say("separators", run(["-n", "20", "t5.txt"])[:2] == (0, model.top([T["t5.txt"]], 20)))
    say("several_files", run(["-n", "5", "t1.txt", "t3.txt"])[:2] == (0, model.top([T["t1.txt"], T["t3.txt"]], 5)))
    say("stdin", run(["-n", "3"], T["t1.txt"].encode("utf-8"))[:2] == (0, model.top([T["t1.txt"]], 3)))
    say("n_zero", run(["-n", "0", "t1.txt"])[:2] == (0, ""))
    code, out, err = run(["t1.txt", "missing.txt"])
    say("missing_file", code == 1 and out == model.top([T["t1.txt"]], 10) and err.strip() == "wordfreq: cannot read missing.txt")
    code, out, err = run(["-n", "x", "t1.txt"])
    say("bad_n", code == 2 and out == "" and err.startswith("usage:"))
    code, out, err = run(["-n", "-3", "t1.txt"])
    say("negative_n", code == 2 and out == "" and err.startswith("usage:"))


main()
