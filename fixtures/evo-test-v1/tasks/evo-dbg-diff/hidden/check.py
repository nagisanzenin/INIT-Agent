"""evo debug-task check: runs the workspace's run.py on hidden inputs and compares its stdout (and, where a case
asks for it, stderr and the files it writes) and its exit status with the behaviour SPEC.md describes.

cases.json: {"items": [{"name": N, "cases": [{"files": {rel: text}, "argv": [...], "stdin": str, "stdout": str,
"code": int, "stderr"?: str, "outfiles"?: {rel: text}}]}]}. An item passes when every one of its cases matches.
Each case runs in its own scratch dir (cwd), never in the workspace; workspace output is never echoed."""
import json
import os
import resource
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

TIMEOUT_S = 5
WORKERS = 4
FSIZE_CAP = 8 << 20  # a runaway writer is stopped by SIGXFSZ (inherited by every child)


def child_env(ws, scratch):
    return {"PATH": "/usr/bin:/bin", "HOME": scratch, "TMPDIR": scratch, "LC_ALL": "C", "LANG": "C",
            "PYTHONHASHSEED": "0", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONIOENCODING": "utf-8", "PYTHONPATH": ws}


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)


def read(path):
    try:
        with open(path, "rb") as f:
            return f.read().decode("utf-8", "replace")
    except OSError:
        return None


def run_case(ws, scratch, idx, case, py=None):
    """Runs one case; returns (stdout, stderr, code, {outfile: text|None}) or None on timeout / spawn failure."""
    d = os.path.join(scratch, "case%04d" % idx)
    os.makedirs(d)
    for rel, text in sorted(case.get("files", {}).items()):
        write(os.path.join(d, rel), text)
    out_p, err_p = os.path.join(d, ".stdout"), os.path.join(d, ".stderr")
    argv = [py or sys.executable, "-S", "-s", "-B", os.path.join(ws, "run.py")] + list(case["argv"])
    try:
        with open(out_p, "wb") as fo, open(err_p, "wb") as fe:
            p = subprocess.run(argv, cwd=d, env=child_env(ws, scratch), input=case.get("stdin", "").encode("utf-8"),
                               stdout=fo, stderr=fe, timeout=TIMEOUT_S)
    except (OSError, subprocess.TimeoutExpired):
        return None
    files = {rel: read(os.path.join(d, rel)) for rel in case.get("outfiles", [])}
    return read(out_p) or "", read(err_p) or "", p.returncode, files


def case_ok(ws, scratch, idx, case):
    r = run_case(ws, scratch, idx, case)
    if r is None:
        return False
    out, err, code, files = r
    if out != case["stdout"] or code != case["code"]:
        return False
    if "stderr" in case and err != case["stderr"]:
        return False
    for rel, text in case.get("outfiles", {}).items():
        if files.get(rel) != text:
            return False
    return True


def run_items(ws, scratch, items, workers=WORKERS):
    jobs, idx = [], 0
    for it in items:
        for c in it["cases"]:
            jobs.append((it["name"], idx, c))
            idx += 1
    with ThreadPoolExecutor(max_workers=workers) as ex:
        oks = list(ex.map(lambda j: case_ok(ws, scratch, j[1], j[2]), jobs))
    res = {it["name"]: True for it in items}
    for (name, _, _), ok in zip(jobs, oks):
        res[name] = res[name] and ok
    return [(it["name"], res[it["name"]]) for it in items]


def main():
    cd = os.environ["INIT_CHECK_DIR"]
    with open(os.path.join(cd, "hidden", "cases.json"), "rb") as f:
        items = json.loads(f.read().decode("utf-8"))["items"]
    resource.setrlimit(resource.RLIMIT_FSIZE, (FSIZE_CAP, FSIZE_CAP))
    for name, ok in run_items(os.getcwd(), os.environ["S1_SCRATCH"], items):
        print("%s %s" % ("PASS" if ok else "FAIL", name))
    sys.stdout.flush()


if __name__ == "__main__":
    main()
