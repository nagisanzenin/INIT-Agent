"""Writes the evo-prose-* task dirs from prose.py (run once; the outputs are committed)."""
import json, os, shutil, sys
H = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, H)
import prose
GROUP = {"library": "train", "garage": "train", "pantry": "heldout", "clinic": "heldout", "studio": "heldout"}
for v, g in GROUP.items():
    d = os.path.join(os.path.dirname(H), "evo-prose-" + v)
    os.makedirs(os.path.join(d, "hidden"), exist_ok=True)
    c = prose.config(v)
    with open(os.path.join(d, "workspace.gen"), "w") as f:
        f.write('exec /usr/bin/python3 -I -S -B "$(dirname "$0")/../_prose/prose.py" ws %s "$1"\n' % v)
    with open(os.path.join(d, "solve.sh"), "w") as f:
        f.write('exec /usr/bin/python3 -I -S -B "$(dirname "$0")/../_prose/prose.py" solve %s .\n' % v)
    with open(os.path.join(d, "hidden", "check.sh"), "w") as f:
        f.write('exec python3 -S -s -B "$INIT_CHECK_DIR/hidden/check.py"\n')
    shutil.copyfile(os.path.join(H, "check.py"), os.path.join(d, "hidden", "check.py"))
    prose.hidden(v, os.path.join(d, "hidden"))
    n = len(prose.sites(c, v))
    names = ["r%02d" % k for k in range(1, n + 1)] + ["regress-data", "regress-cli"]
    t = {"id": "evo-prose-" + v, "title": "Release %s of the %s package (design note %s)" % (c["ver2"], c["pkg"], c["batch"]),
         "family": "t", "difficulty": 4, "labels": ["py", "prose"], "max_turns": 80,
         "prompt": ("The package `%s` in the current directory has a design note for its next release in CHANGES.md. "
                    "Implement every change described in CHANGES.md. Code must run on /usr/bin/python3 (Python 3.9), "
                    "the interpreter the checks use; standard library only. tests/test_visible.py covers a few of the "
                    "changes (`/usr/bin/python3 -m unittest discover -s tests`); other checks cover every change. "
                    "Do not modify CHANGES.md or tests/test_visible.py.") % c["pkg"],
         "source": {"kind": "fresh"}, "protected": ["CHANGES.md", "tests/test_visible.py"],
         "steps": [{"kind": "sh", "script": "hidden/check.sh", "names": names, "timeout_s": 30}],
         "evo": {"group": g, "kind": "prose", "spare": False,
                 "pair": "evo-prose-library" if v != "library" else "evo-prose-pantry"}}
    with open(os.path.join(d, "task.in.json"), "w") as f:
        json.dump(t, f, indent=1, ensure_ascii=False)
        f.write("\n")
print("ok")
