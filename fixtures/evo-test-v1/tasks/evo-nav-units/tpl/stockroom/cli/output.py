"""How commands print: results to stdout, diagnostics to stderr."""
import json
import sys


def out(text="") -> None:
    sys.stdout.write(text if text.endswith("\n") else text + "\n")


def out_json(obj) -> None:
    sys.stdout.write(json.dumps(obj, indent=2) + "\n")


def err(text) -> None:
    sys.stderr.write(text.rstrip("\n") + "\n")
