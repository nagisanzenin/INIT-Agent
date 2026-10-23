#!/usr/bin/env python3
"""Runs a Kite program: python3 run.py PROGRAM.kite (see SPEC.md for the output and the exit status)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from kite.compiler import compile_program  # noqa: E402
from kite.errors import KiteRuntimeError, KiteSyntaxError  # noqa: E402
from kite.lexer import tokenize  # noqa: E402
from kite.parser import parse  # noqa: E402
from kite.scope import resolve  # noqa: E402
from kite.vm import VM  # noqa: E402


def main(argv):
    if len(argv) != 2:
        sys.stderr.write("usage: run.py PROGRAM.kite\n")
        return 64
    try:
        with open(argv[1], encoding="utf-8") as f:
            source = f.read()
    except (OSError, UnicodeDecodeError) as e:
        sys.stderr.write("run.py: cannot read %s: %s\n" % (argv[1], e))
        return 66
    out = sys.stdout
    try:
        tree = parse(tokenize(source))
        info = resolve(tree)
        main_proto = compile_program(tree, info)
    except KiteSyntaxError as e:
        out.write(e.report() + "\n")
        sys.stderr.write("%s\n" % e.message)
        return 2
    try:
        VM(out).run(main_proto)
    except KiteRuntimeError as e:
        out.write(e.report() + "\n")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
