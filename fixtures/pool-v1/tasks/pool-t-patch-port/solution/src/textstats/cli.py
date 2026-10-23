"""textstats command line: python3 -m textstats.cli FILE."""
import argparse

from .core import line_count, unique_words, word_count


def main(argv=None):
    ap = argparse.ArgumentParser(prog="textstats")
    ap.add_argument("file")
    ap.add_argument("--lines", action="store_true", help="also print the number of lines")
    ap.add_argument("--unique", action="store_true", help="also print the number of different words")
    args = ap.parse_args(argv)
    with open(args.file) as f:
        text = f.read()
    print("words: %d" % word_count(text))
    if args.lines:
        print("lines: %d" % line_count(text))
    if args.unique:
        print("unique: %d" % unique_words(text))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
