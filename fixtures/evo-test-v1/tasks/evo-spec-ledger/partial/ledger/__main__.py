"""python3 -m ledger FILE (SPEC.md R40)."""
import sys

from .core import process

USAGE = "usage: python3 -m ledger FILE\n"


def main(argv):
    if len(argv) != 1 or (argv[0].startswith("-") and argv[0] != "-"):
        sys.stderr.write(USAGE)
        return 2
    path = argv[0]
    try:
        if path == "-":
            data = sys.stdin.buffer.read()
        else:
            with open(path, "rb") as f:
                data = f.read()
        text = data.decode("utf-8")
    except (OSError, UnicodeDecodeError):
        sys.stderr.write("ledger: cannot read %s\n" % path)
        return 2
    r = process(text)
    sys.stdout.buffer.write(r.output.encode("utf-8"))
    sys.stdout.flush()
    for e in r.errors:
        sys.stderr.write(e + "\n")
    return 1 if r.errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
