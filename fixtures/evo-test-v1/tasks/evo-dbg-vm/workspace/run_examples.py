#!/usr/bin/env python3
"""Runs every examples/NAME.kite through run.py and compares the result with examples/NAME.out.

NAME.out holds the expected standard output; its last line is `exit N`, the expected exit status.
Usage: python3 run_examples.py [NAME ...]
"""
import difflib
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
EXAMPLES = os.path.join(HERE, "examples")


def run(name):
    prog = os.path.join(EXAMPLES, name + ".kite")
    with open(os.path.join(EXAMPLES, name + ".out"), encoding="utf-8") as f:
        want = f.read()
    try:
        p = subprocess.run([sys.executable, os.path.join(HERE, "run.py"), prog], stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, timeout=20)
        got = p.stdout.decode("utf-8", "replace") + "exit %d\n" % p.returncode
    except subprocess.TimeoutExpired:
        got = "(timed out after 20 s)\n"
    if got == want:
        print("ok   %s" % name)
        return True
    print("FAIL %s" % name)
    for line in difflib.unified_diff(want.splitlines(), got.splitlines(), "expected", "got", lineterm="", n=2):
        print("    " + line)
    return False


def main(argv):
    names = argv[1:] or sorted(n[:-5] for n in os.listdir(EXAMPLES) if n.endswith(".kite"))
    failed = [n for n in names if not run(n)]
    print("%d/%d examples pass" % (len(names) - len(failed), len(names)))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
