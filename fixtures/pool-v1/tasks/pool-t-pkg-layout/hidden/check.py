"""pool-t-pkg-layout check: runs `python3 -m invtool ...` from the workspace root, with the visible CSV
(relative path) and a hidden CSV (absolute path, and relative from the workspace), and compares the output with
an independent model."""
import os
import shutil
import subprocess
import sys

WS = os.getcwd()
CD = os.environ["INIT_CHECK_DIR"]
SCRATCH = os.environ["S1_SCRATCH"]
ENV = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": SCRATCH, "LC_ALL": "C", "PYTHONHASHSEED": "0",
       "PYTHONDONTWRITEBYTECODE": "1"}


def parse(path):
    import csv
    items = []
    with open(path, newline="") as f:
        for row in csv.DictReader(f):
            whole, _, frac = row["price"].partition(".")
            items.append((row["sku"], row["name"], row["category"], int(row["qty"]),
                          int(whole) * 100 + int((frac + "00")[:2])))
    return items


def money(c):
    return "%d.%02d" % divmod(c, 100)


def want_report(path):
    lines = ["%-8s %-20s %-10s %5s %10s" % ("SKU", "NAME", "CATEGORY", "QTY", "VALUE")]
    for sku, name, cat, qty, price in sorted(parse(path)):
        lines.append("%-8s %-20s %-10s %5d %10s" % (sku, name, cat, qty, money(qty * price)))
    return "\n".join(lines) + "\n"


def want_total(path, cat=None):
    return money(sum(q * p for _, _, c, q, p in parse(path) if cat is None or c == cat)) + "\n"


def run(args):
    p = subprocess.run([sys.executable, "-S", "-s", "-B", "-m", "invtool"] + args, cwd=WS, env=ENV,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
    return p.returncode, p.stdout.decode("utf-8", "replace")


def say(name, ok):
    print("%s %s" % ("PASS" if ok else "FAIL", name))


def main():
    hidden = os.path.join(SCRATCH, "inv2.csv")
    shutil.copyfile(os.path.join(CD, "hidden", "inv2.csv"), hidden)
    rel = os.path.relpath(hidden, WS)
    vis = os.path.join(WS, "data", "inventory.csv")
    say("report_visible", run(["report", "data/inventory.csv"]) == (0, want_report(vis)))
    say("total_visible", run(["total", "data/inventory.csv"]) == (0, want_total(vis)))
    say("total_category", run(["total", "data/inventory.csv", "--category", "safety"]) == (0, want_total(vis, "safety")))
    say("report_hidden_absolute", run(["report", hidden]) == (0, want_report(hidden)))
    say("total_hidden_relative", run(["total", rel, "--category", "tools"]) == (0, want_total(hidden, "tools")))
    say("total_hidden_absolute", run(["total", hidden]) == (0, want_total(hidden)))


main()
