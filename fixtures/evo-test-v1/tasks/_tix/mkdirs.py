"""Writes the evo-tix-* task dirs from tix.py (run once; the outputs are committed)."""
import json, os, shutil, subprocess, sys, tempfile
H = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, H)
import tix
GROUP = {"library": "train", "garage": "train", "pantry": "heldout", "clinic": "heldout", "studio": "heldout"}
for v, g in GROUP.items():
    d = os.path.join(os.path.dirname(H), "evo-tix-" + v)
    os.makedirs(os.path.join(d, "hidden"), exist_ok=True)
    c = tix.V[v]
    n = len(tix.SETS[v])
    with open(os.path.join(d, "workspace.gen"), "w") as f:
        f.write('exec /usr/bin/python3 -I -S -B "$(dirname "$0")/../_tix/tix.py" ws %s "$1"\n' % v)
    with open(os.path.join(d, "solve.sh"), "w") as f:
        f.write('exec /usr/bin/python3 -I -S -B "$(dirname "$0")/../_tix/tix.py" solve %s .\n' % v)
    with open(os.path.join(d, "hidden", "check.sh"), "w") as f:
        f.write('exec python3 -S -s -B "$INIT_CHECK_DIR/hidden/check.py"\n')
    shutil.copyfile(os.path.join(H, "check.py"), os.path.join(d, "hidden", "check.py"))
    tix.hidden(v, os.path.join(d, "hidden"))
    names = ["t%02d" % k for k in range(1, n + 1)] + ["regress"]
    t = {"id": "evo-tix-" + v, "title": "Ticket batch %s for the %s package" % (c["batch"], c["pkg"]), "family": "t",
         "difficulty": 3, "labels": ["py", "tickets"], "max_turns": 60,
         "prompt": ("The package `%s` in the current directory has a batch of %d change requests in TICKETS.md. "
                    "Implement every ticket. Code must run on /usr/bin/python3 (Python 3.9), the interpreter the checks "
                    "use; standard library only. tests/test_visible.py covers a couple of the tickets "
                    "(`/usr/bin/python3 -m unittest discover -s tests`); other checks cover every ticket. "
                    "Do not modify TICKETS.md or tests/test_visible.py.") % (c["pkg"], n),
         "source": {"kind": "fresh"}, "protected": ["TICKETS.md", "tests/test_visible.py"],
         "steps": [{"kind": "sh", "script": "hidden/check.sh", "names": names, "timeout_s": 30}],
         "evo": {"group": g, "kind": "tickets", "spare": False, "pair": "evo-tix-library" if v != "library" else "evo-tix-pantry"}}
    with open(os.path.join(d, "task.in.json"), "w") as f:
        json.dump(t, f, indent=1, ensure_ascii=False)
        f.write("\n")
print("ok")
