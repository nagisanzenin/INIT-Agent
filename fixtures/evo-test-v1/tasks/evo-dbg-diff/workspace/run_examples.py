"""Runs every example in examples/ through run.py and compares what it does with what it should do.

An example is a directory examples/NAME/ that holds:
  cmd        the arguments of run.py, on one line, separated by spaces (for example `diff -U 1 old.txt new.txt`);
  stdin      optional: the standard input of the command;
  expected/  `stdout`: the exact standard output; `status`: the exit status; any other file: the exact content
             that the file of the same name must have after the command (a file the command writes);
  anything else is an input file, copied into a fresh directory in which the command runs.

Prints `ok   NAME` or `FAIL NAME` (with a short diff) per example; exits with status 1 when any example fails.
Run it with: python3 run_examples.py [NAME...]"""
import difflib
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
EXAMPLES = os.path.join(HERE, "examples")
NOT_INPUTS = ("cmd", "stdin", "expected")
MAX_DIFF_LINES = 30


def read(path):
    with open(path, "rb") as f:
        return f.read().decode("utf-8", "replace")


def short_diff(what, want, got):
    """A unified diff of the expected and the actual text, cut after MAX_DIFF_LINES lines."""
    lines = list(difflib.unified_diff(want.splitlines(True), got.splitlines(True), "expected " + what,
                                      "actual " + what))
    out = []
    for line in lines[:MAX_DIFF_LINES]:
        out.append("    " + (line if line.endswith("\n") else line + "\n    (no newline at end)\n"))
    if len(lines) > MAX_DIFF_LINES:
        out.append("    ... (%d more lines)\n" % (len(lines) - MAX_DIFF_LINES))
    return "".join(out)


def run_example(name):
    """Runs one example; returns the list of problems (empty when it passes)."""
    src = os.path.join(EXAMPLES, name)
    work = tempfile.mkdtemp(prefix="ldiff-example-")
    try:
        for entry in sorted(os.listdir(src)):
            if entry not in NOT_INPUTS:
                shutil.copyfile(os.path.join(src, entry), os.path.join(work, entry))
        args = read(os.path.join(src, "cmd")).split()
        stdin_path = os.path.join(src, "stdin")
        stdin = read(stdin_path) if os.path.exists(stdin_path) else ""
        try:
            proc = subprocess.run([sys.executable, os.path.join(HERE, "run.py")] + args, cwd=work,
                                  input=stdin.encode("utf-8"), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                  timeout=30)
        except subprocess.TimeoutExpired:
            return ["timed out after 30 s\n"]
        problems = []
        expected_dir = os.path.join(src, "expected")
        want_status = int(read(os.path.join(expected_dir, "status")).strip())
        if proc.returncode != want_status:
            err = proc.stderr.decode("utf-8", "replace").strip().splitlines()
            problems.append("exit status %d, expected %d%s\n" % (proc.returncode, want_status,
                                                                 (" (stderr: %s)" % err[-1]) if err else ""))
        stdout = proc.stdout.decode("utf-8", "replace")
        want = read(os.path.join(expected_dir, "stdout"))
        if stdout != want:
            problems.append("standard output differs:\n" + short_diff("stdout", want, stdout))
        for entry in sorted(os.listdir(expected_dir)):
            if entry in ("stdout", "status"):
                continue
            path = os.path.join(work, entry)
            if not os.path.exists(path):
                problems.append("file %s was not written\n" % entry)
                continue
            got, want = read(path), read(os.path.join(expected_dir, entry))
            if got != want:
                problems.append("file %s differs:\n%s" % (entry, short_diff(entry, want, got)))
        return problems
    finally:
        shutil.rmtree(work, ignore_errors=True)


def main(argv):
    names = argv[1:] or sorted(n for n in os.listdir(EXAMPLES) if os.path.isdir(os.path.join(EXAMPLES, n)))
    failed = 0
    for name in names:
        problems = run_example(name)
        if problems:
            failed += 1
            print("FAIL %s" % name)
            sys.stdout.write("".join("  " + p for p in problems))
        else:
            print("ok   %s" % name)
    print("%d of %d examples passed" % (len(names) - failed, len(names)))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
