"""evo-prose check (protocol v3 §11.3): one PASS/FAIL line per requirement-site from hidden/spec.json. Each check runs
Python with the workspace on sys.path and cwd = the workspace, in a child with a timeout, and compares its stdout
exactly (stripped). A copy of _tix/check.py."""
import json
import os
import subprocess
import sys

CD = os.environ["INIT_CHECK_DIR"]
WS = os.getcwd()
SCRATCH = os.environ.get("S1_SCRATCH") or WS
ENV = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": SCRATCH, "TMPDIR": SCRATCH, "LC_ALL": "C",
       "PYTHONHASHSEED": "0", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": WS, "PYTHONIOENCODING": "utf-8"}


def run(code):
    try:
        p = subprocess.run([sys.executable, "-S", "-s", "-B", "-c", code], cwd=WS, env=ENV, stdin=subprocess.DEVNULL,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        return p.returncode, p.stdout.decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        return None, ""


spec = json.load(open(os.path.join(CD, "hidden", "spec.json"), encoding="utf-8"))
for item in spec["items"]:
    ok = True
    for code, want in item["checks"]:
        rc, out = run(code)
        ok = ok and rc == 0 and out.strip() == want.strip()
    print("%s %s" % ("PASS" if ok else "FAIL", item["name"]))
