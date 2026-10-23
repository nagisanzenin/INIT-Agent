"""Runs the examples in examples/ and compares each output with the expected one.

    python3 run_examples.py [NAME ...]

An example is a directory examples/NAME/ holding book.csv, optionally edits.txt, and expected.txt: the exact
stdout of `python3 run.py book.csv [edits.txt]` run inside that directory, which must exit with status 0.
Prints `ok   NAME` or `FAIL NAME` (with a short diff) per example; exits 1 if any example failed.
"""
import difflib
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
EXAMPLES = os.path.join(HERE, "examples")


def run_example(name):
    d = os.path.join(EXAMPLES, name)
    argv = [sys.executable, "-B", os.path.join(HERE, "run.py"), "book.csv"]
    if os.path.isfile(os.path.join(d, "edits.txt")):
        argv.append("edits.txt")
    try:
        p = subprocess.run(argv, cwd=d, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)
    except subprocess.TimeoutExpired:
        return ["timed out"]
    with open(os.path.join(d, "expected.txt"), "rb") as f:
        want = f.read().decode("utf-8")
    got = p.stdout.decode("utf-8", "replace")
    problems = []
    if p.returncode != 0:
        problems.append("exit status %d" % p.returncode)
        problems.extend(p.stderr.decode("utf-8", "replace").splitlines()[-5:])
    if got != want:
        diff = difflib.unified_diff(want.splitlines(), got.splitlines(), "expected", "got", lineterm="", n=1)
        problems.extend(list(diff)[:24])
    return problems


def main(argv):
    names = argv[1:] or sorted(n for n in os.listdir(EXAMPLES) if os.path.isdir(os.path.join(EXAMPLES, n)))
    failed = 0
    for name in names:
        problems = run_example(name)
        if problems:
            failed += 1
            print("FAIL %s" % name)
            for line in problems:
                print("    " + line)
        else:
            print("ok   %s" % name)
    print("%d/%d examples ok" % (len(names) - failed, len(names)))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
