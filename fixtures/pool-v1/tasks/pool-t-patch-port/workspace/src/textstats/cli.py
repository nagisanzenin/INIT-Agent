"""textstats command line: python3 -m textstats.cli FILE."""
import argparse

from .core import line_count, word_count


def main(argv=None):
    ap = argparse.ArgumentParser(prog="textstats")
    ap.add_argument("file")
    ap.add_argument("--lines", action="store_true", help="also print the number of lines")
    args = ap.parse_args(argv)
    with open(args.file) as f:
        text = f.read()
    print("words: %d" % word_count(text))
    if args.lines:
        print("lines: %d" % line_count(text))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
