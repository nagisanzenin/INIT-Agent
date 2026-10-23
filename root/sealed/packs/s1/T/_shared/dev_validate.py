#!/usr/bin/env python3
"""Family T dev validator (maintainer side, dev seeds only; never Root seeds). Python >= 3.9, stdlib only.

    python3 T/_shared/dev_validate.py [--seeds N] [--keep DIR] [template ...]

Per template (default: every T/<dir> with a template.json), for N dev seeds (default 3):
  M7-P01  gen/run twice per seed: byte-identical; different across seeds
  M7-P02  solution x3 = 1000; no-op (workspace.tar) x3 equal and <= 500; empty workspace < 1000
  M7-P04  every check run <= 10 s
  M7-P05  a changed protected file scores {"score_permille":0,"tamper":true}
  digest  the check dir is unchanged by a check run (checker integrity, m7 §3.1.3 item 3)
Also every difficulty 1..5 generates and its solution scores 1000 (one seed).
The check runs like the Root's: env cleared plus PATH, HOME, TMPDIR, LANG=C, INIT_CHECK_DIR; cwd = ws.
"""
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

sys.dont_write_bytecode = True  # the pack lint rejects __pycache__ (the installed pack is read-only)
T_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PACK = os.path.dirname(T_DIR)
sys.path.insert(0, os.path.join(PACK, "lib"))
import s1lib  # noqa: E402

PY = sys.executable


def dev_seed(i, salt="t-dev"):
    return hashlib.sha256(("%s-%d" % (salt, i)).encode()).hexdigest()


def gen(tdir, seed, out, difficulty=None):
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "LANG": "C", "LC_ALL": "C"}
    if difficulty is not None:
        env["INIT_S1_DIFFICULTY"] = str(difficulty)
    t0 = time.monotonic()
    p = subprocess.run([os.path.join(tdir, "gen", "run"), seed, out], env=env, stdout=subprocess.PIPE,
                       stderr=subprocess.PIPE, timeout=120)
    if p.returncode != 0:
        raise RuntimeError("gen failed (%d): %s" % (p.returncode, p.stderr.decode()[-3000:]))
    return time.monotonic() - t0


def extract(tar_path, dst):
    os.makedirs(dst)
    with tarfile.open(tar_path) as tf:
        try:
            tf.extractall(dst, filter="data")
        except TypeError:
            tf.extractall(dst)


def run_check(inst, ws_tar, work, mutate=None, empty=False):
    """Returns (score line dict, seconds, digest_ok)."""
    out = tempfile.mkdtemp(prefix="out-", dir=work)
    ws = os.path.join(out, "ws")
    if empty:
        os.makedirs(ws)
    else:
        extract(ws_tar, ws)
    if mutate:
        mutate(ws)
    chk = os.path.join(out, "check")
    shutil.copytree(os.path.join(inst, "check"), chk)
    for d in ("tmp", "home"):
        os.makedirs(os.path.join(out, d))
    before = s1lib.tree_digest(chk)
    env = {"PATH": "/usr/bin:/bin", "HOME": os.path.join(out, "home"), "TMPDIR": os.path.join(out, "tmp"),
           "LANG": "C", "INIT_TASK_ID": "dev", "INIT_CHECK_DIR": chk}
    t0 = time.monotonic()
    p = subprocess.run([os.path.join(chk, "run")], cwd=ws, env=env, stdout=subprocess.PIPE,
                       stderr=subprocess.PIPE, timeout=120)
    dt = time.monotonic() - t0
    after = s1lib.tree_digest(chk)
    lines = p.stdout.decode("utf-8", "replace").strip().splitlines()
    try:
        score = json.loads(lines[-1]) if lines else {}
    except ValueError:
        score = {}
    if p.returncode not in (0, 3):
        score = {"score_permille": 0, "exit": p.returncode}
    score["_stderr"] = p.stderr.decode("utf-8", "replace")[-1500:]
    shutil.rmtree(out)
    return score, dt, before == after


def digest_dir(d):
    return s1lib.tree_digest(d)


def validate(tdir, nseeds, work, verbose):
    name = os.path.basename(tdir)
    errs = []
    notes = []
    digests = {}
    for i in range(nseeds):
        seed = dev_seed(i)
        a = os.path.join(work, "%s-%d-a" % (name, i))
        b = os.path.join(work, "%s-%d-b" % (name, i))
        ga = gen(tdir, seed, a)
        gen(tdir, seed, b)
        da, db = digest_dir(a), digest_dir(b)
        if da != db:
            errs.append("seed %d: gen not deterministic" % i)
        if da in digests.values():
            errs.append("seed %d: same instance as another seed" % i)
        digests[i] = da
        shutil.rmtree(b)
        sol = os.path.join(a, "solution.tar")
        wst = os.path.join(a, "workspace.tar")
        sols = [run_check(a, sol, work) for _ in range(3)]
        noops = [run_check(a, wst, work) for _ in range(3)]
        empty = run_check(a, wst, work, empty=True)
        times = [s[1] for s in sols + noops + [empty]]
        for s, _, dig in sols + noops + [empty]:
            if not dig:
                errs.append("seed %d: check dir changed by a check run" % i)
        sv = [s[0].get("score_permille") for s in sols]
        nv = [s[0].get("score_permille") for s in noops]
        if sv != [1000] * 3:
            errs.append("seed %d: solution scores %s\n%s" % (i, sv, sols[0][0].get("_stderr")))
        if len(set(nv)) != 1 or nv[0] is None or nv[0] > 500:
            errs.append("seed %d: no-op scores %s\n%s" % (i, nv, noops[0][0].get("_stderr")))
        if empty[0].get("score_permille", 0) >= 1000:
            errs.append("seed %d: empty workspace scores 1000" % i)
        if max(times) > 10:
            errs.append("seed %d: a check took %.1f s" % (i, max(times)))
        # tamper: change the first protected file in the solution workspace
        prot = json.load(open(os.path.join(a, "check", "protected.json")))
        if prot:
            def mut(ws, p=prot[0]["path"]):
                with open(os.path.join(ws, p), "ab") as f:
                    f.write(b"\n# edited\n")
            tam = run_check(a, sol, work, mutate=mut)[0]
            if not (tam.get("tamper") is True and tam.get("score_permille") == 0):
                errs.append("seed %d: tamper not detected: %s" % (i, tam))
        else:
            notes.append("no protected files")
        notes.append("seed %d: gen %.2fs, check max %.2fs, no-op %s, empty %s" % (
            i, ga, max(times), nv[0], empty[0].get("score_permille")))
        shutil.rmtree(a)
    # difficulty knob range
    tj = json.load(open(os.path.join(tdir, "template.json")))
    d = tj["difficulty"]
    for lvl in range(d["min"], d["max"] + 1):
        o = os.path.join(work, "%s-d%d" % (name, lvl))
        gen(tdir, dev_seed(100 + lvl), o, difficulty=lvl)
        s = run_check(o, os.path.join(o, "solution.tar"), work)[0]
        n = run_check(o, os.path.join(o, "workspace.tar"), work)[0]
        if s.get("score_permille") != 1000:
            errs.append("difficulty %d: solution scores %s\n%s" % (lvl, s.get("score_permille"), s.get("_stderr")))
        if (n.get("score_permille") or 0) > 500:
            errs.append("difficulty %d: no-op scores %s" % (lvl, n.get("score_permille")))
        shutil.rmtree(o)
    return errs, notes


def main(argv):
    nseeds = 3
    names = []
    keep = None
    it = iter(argv)
    for a in it:
        if a == "--seeds":
            nseeds = int(next(it))
        elif a == "--keep":
            keep = next(it)
        else:
            names.append(a.rstrip("/").split("/")[-1])
    if not names:
        names = sorted(d for d in os.listdir(T_DIR) if os.path.isfile(os.path.join(T_DIR, d, "template.json")))
    work = keep or tempfile.mkdtemp(prefix="tval-")
    os.makedirs(work, exist_ok=True)
    bad = 0
    try:
        for n in names:
            t0 = time.monotonic()
            try:
                errs, notes = validate(os.path.join(T_DIR, n), nseeds, work, True)
            except Exception as e:  # noqa: BLE001
                errs, notes = ["exception: %s: %s" % (type(e).__name__, e)], []
            status = "OK" if not errs else "FAIL"
            bad += bool(errs)
            print("%-22s %s (%.1fs)" % (n, status, time.monotonic() - t0))
            for x in notes:
                print("    " + x)
            for e in errs:
                print("  ! " + e)
            sys.stdout.flush()
    finally:
        if keep is None:
            shutil.rmtree(work, ignore_errors=True)
    print("%d/%d templates OK" % (len(names) - bad, len(names)))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
