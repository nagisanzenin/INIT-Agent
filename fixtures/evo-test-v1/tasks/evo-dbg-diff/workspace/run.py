"""ldiff command line: python3 run.py diff|patch ... (see SPEC.md)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from ldiff.cli import main  # noqa: E402

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
