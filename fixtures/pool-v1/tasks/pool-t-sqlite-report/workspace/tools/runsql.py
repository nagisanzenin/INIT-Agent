#!/usr/bin/env python3
"""Runs a query file against an in-memory database built from a data file.

    python3 tools/runsql.py shop.sql report.sql

Prints one line per result row, columns joined with "|" (NULL prints as an empty string).
"""
import sqlite3
import sys


def main(argv):
    if len(argv) != 3:
        sys.stderr.write("usage: runsql.py <data.sql> <query.sql>\n")
        return 64
    db = sqlite3.connect(":memory:")
    with open(argv[1]) as f:
        db.executescript(f.read())
    with open(argv[2]) as f:
        query = f.read()
    for row in db.execute(query):
        print("|".join("" if v is None else str(v) for v in row))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
