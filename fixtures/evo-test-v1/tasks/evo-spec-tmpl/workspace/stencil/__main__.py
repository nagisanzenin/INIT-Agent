"""Command line: python3 -m stencil TEMPLATE [CONTEXT] (SPEC.md, rule R64)."""

import sys


def main(argv):
    """Render the TEMPLATE file with the JSON object in the CONTEXT file; return the exit status (SPEC.md R64)."""
    raise NotImplementedError("see SPEC.md R64")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
