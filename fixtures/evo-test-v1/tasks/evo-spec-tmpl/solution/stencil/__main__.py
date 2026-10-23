"""Command line: python3 -m stencil TEMPLATE [CONTEXT] (SPEC.md, rule R64)."""

import json
import sys

from .engine import TemplateError, render


def main(argv):
    if len(argv) not in (1, 2):
        sys.stderr.write("usage: python3 -m stencil TEMPLATE [CONTEXT]\n")
        return 2
    try:
        with open(argv[0], "rb") as f:
            src = f.read().decode("utf-8")
    except (OSError, UnicodeDecodeError):
        sys.stderr.write("stencil: cannot read %s\n" % argv[0])
        return 2
    ctx = {}
    if len(argv) == 2:
        try:
            with open(argv[1], "rb") as f:
                ctx = json.loads(f.read().decode("utf-8"))
        except (OSError, ValueError):
            ctx = None
        if not isinstance(ctx, dict):
            sys.stderr.write("stencil: bad context\n")
            return 2
    try:
        out = render(src, ctx)
    except TemplateError as e:
        sys.stderr.write("stencil: %s\n" % e)
        return 1
    sys.stdout.buffer.write(out.encode("utf-8"))
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
