"""Command line of invtool: `python3 -m invtool report|total ...`."""
import argparse
import sys

from report import render
from loader import load


def main(argv=None):
    ap = argparse.ArgumentParser(prog="invtool")
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("report", help="print the inventory table")
    r.add_argument("csv")
    t = sub.add_parser("total", help="print the stock value")
    t.add_argument("csv")
    t.add_argument("--category")
    args = ap.parse_args(argv)
    items = load(args.csv)
    if args.cmd == "report":
        sys.stdout.write(render(items))
    else:
        cents = sum(i["qty"] * i["price_cents"] for i in items
                    if args.category is None or i["category"] == args.category)
        print("%d.%02d" % divmod(cents, 100))
    return 0
