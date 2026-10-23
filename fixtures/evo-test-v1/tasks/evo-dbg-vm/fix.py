#!/usr/bin/env python3
"""Author tool (task dir only, never in the workspace): applies the fixes of the planted bugs listed in bugs.json
(next to this file) to a workspace copy. solve.sh runs `fix.py all .`; dev.py uses `visible` for the partial/
overlay (the naive pass: only the bugs the visible examples reveal).

    fix.py all|visible|hidden|ID[,ID...] WSDIR

bugs.json: {"bugs": [{"id", "visible": bool, "what": one line, "edits": [[path, old, new], ...]}]}; every old text
must occur exactly once in the file at the time it is applied."""
import json
import os
import sys


def load(task_dir):
    with open(os.path.join(task_dir, "bugs.json"), "rb") as f:
        return json.loads(f.read().decode("utf-8"))["bugs"]


def select(bugs, which):
    if which == "all":
        return bugs
    if which == "visible":
        return [b for b in bugs if b["visible"]]
    if which == "hidden":
        return [b for b in bugs if not b["visible"]]
    if which == "none":
        return []
    ids = set(which.split(","))
    unknown = ids - {b["id"] for b in bugs}
    if unknown:
        raise SystemExit("fix.py: unknown bug ids %s" % sorted(unknown))
    return [b for b in bugs if b["id"] in ids]


def apply(bugs, ws):
    for b in bugs:
        for path, old, new in b["edits"]:
            p = os.path.join(ws, path)
            with open(p, "rb") as f:
                text = f.read().decode("utf-8")
            n = text.count(old)
            if n != 1:
                raise SystemExit("fix.py: %s: old text occurs %d times in %s" % (b["id"], n, path))
            with open(p, "wb") as f:
                f.write(text.replace(old, new).encode("utf-8"))


def main(argv):
    if len(argv) != 3:
        raise SystemExit(__doc__)
    apply(select(load(os.path.dirname(os.path.abspath(__file__))), argv[1]), argv[2])
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
