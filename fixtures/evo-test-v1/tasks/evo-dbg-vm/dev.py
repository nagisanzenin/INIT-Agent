#!/usr/bin/env python3
"""Author tool for an evo debug task (lives in the task dir; never copied into the workspace or check/).
Run it with /usr/bin/python3 -I from anywhere:

    dev.py cases             run cases_src.ITEMS through the reference (workspace + every fix in bugs.json), write
                             hidden/cases.json and the step names in task.in.json
    dev.py partial           regenerate partial/ = the files the `visible` fixes change (the naive-pass fixture)
    dev.py score [V ...]     score variants against hidden/cases.json; V = none|visible|hidden|all|ID[,ID...]
    dev.py examples [V ...]  run the workspace's run_examples.py on variants (tail of its output)
    dev.py refws DIR         materialize a variant workspace in DIR (default variant all; --variant V)
"""
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile

TASK = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, TASK)
import fix  # noqa: E402


def load_check():
    spec = importlib.util.spec_from_file_location("evo_dbg_check", os.path.join(TASK, "hidden", "check.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def build_ws(dest, variant="none"):
    src = os.path.join(TASK, "workspace")
    for dp, dn, fn in os.walk(src):
        dn[:] = sorted(d for d in dn if d != "__pycache__")
        rel = os.path.relpath(dp, src)
        out = dest if rel == "." else os.path.join(dest, rel)
        os.makedirs(out, exist_ok=True)
        for n in sorted(fn):
            if n.endswith(".pyc") or n == ".DS_Store":
                continue
            shutil.copyfile(os.path.join(dp, n), os.path.join(out, ".gitignore" if n == "dot.gitignore" else n))
            os.chmod(os.path.join(out, n if n != "dot.gitignore" else ".gitignore"),
                     0o755 if os.stat(os.path.join(dp, n)).st_mode & 0o111 else 0o644)
    gen = os.path.join(TASK, "workspace.gen")
    if os.path.isfile(gen):
        subprocess.run(["/bin/sh", gen, dest], cwd=dest, check=True, env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"})
    fix.apply(fix.select(fix.load(TASK), variant), dest)
    return dest


def files_of(root):
    out = {}
    for dp, dn, fn in os.walk(root):
        dn[:] = [d for d in dn if d != "__pycache__"]
        for n in fn:
            p = os.path.join(dp, n)
            with open(p, "rb") as f:
                out[os.path.relpath(p, root)] = f.read()
    return out


def cmd_cases(tmp):
    chk = load_check()
    sys.path.insert(0, TASK)
    import cases_src
    ws = build_ws(os.path.join(tmp, "ref"), "all")
    scratch = os.path.join(tmp, "scratch")
    os.makedirs(scratch)
    items, idx, names = [], 0, []
    for name, cases in cases_src.ITEMS:
        out = []
        for c in cases:
            r = chk.run_case(ws, scratch, idx, c)
            idx += 1
            if r is None:
                raise SystemExit("reference timed out on %s case %d" % (name, len(out)))
            stdout, stderr, code, files = r
            e = {"files": c.get("files", {}), "argv": c["argv"], "stdin": c.get("stdin", ""), "stdout": stdout,
                 "code": code}
            if c.get("stderr"):
                e["stderr"] = stderr
            if c.get("outfiles"):
                if any(v is None for v in files.values()):
                    raise SystemExit("reference wrote no %s in %s" % (c["outfiles"], name))
                e["outfiles"] = files
            out.append(e)
        if name in names:
            raise SystemExit("duplicate item %s" % name)
        names.append(name)
        items.append({"name": name, "cases": out})
    with open(os.path.join(TASK, "hidden", "cases.json"), "w", encoding="utf-8") as f:
        json.dump({"items": items}, f, indent=1, sort_keys=True, ensure_ascii=True)
        f.write("\n")
    tp = os.path.join(TASK, "task.in.json")
    with open(tp, "rb") as f:
        t = json.loads(f.read().decode("utf-8"))
    t["steps"][0]["names"] = names
    with open(tp, "w", encoding="utf-8") as f:
        json.dump(t, f, indent=1, ensure_ascii=True)
        f.write("\n")
    print("%d items, %d cases" % (len(names), idx))


def cmd_partial(tmp):
    base = files_of(build_ws(os.path.join(tmp, "base"), "none"))
    vis = files_of(build_ws(os.path.join(tmp, "vis"), "visible"))
    pdir = os.path.join(TASK, "partial")
    if os.path.isdir(pdir):
        shutil.rmtree(pdir)
    for rel, data in sorted(vis.items()):
        if base.get(rel) != data:
            p = os.path.join(pdir, rel)
            os.makedirs(os.path.dirname(p), exist_ok=True)
            with open(p, "wb") as f:
                f.write(data)
            print("partial/" + rel)


def cmd_score(tmp, variants):
    chk = load_check()
    with open(os.path.join(TASK, "hidden", "cases.json"), "rb") as f:
        items = json.loads(f.read().decode("utf-8"))["items"]
    for v in variants or ["none", "visible", "all"]:
        d = os.path.join(tmp, "v-" + v.replace(",", "_"))
        ws = build_ws(os.path.join(d, "ws"), v)
        os.makedirs(os.path.join(d, "s"))
        res = chk.run_items(ws, os.path.join(d, "s"), items)
        ok = sum(1 for _, o in res if o)
        print("%-24s %4d  (%d/%d)  failing: %s" % (v, 1000 * ok // len(res), ok, len(res),
                                                  " ".join(n for n, o in res if not o)))


def cmd_examples(tmp, variants):
    for v in variants or ["none", "visible", "all"]:
        ws = build_ws(os.path.join(tmp, "e-" + v.replace(",", "_")), v)
        p = subprocess.run([sys.executable, "-S", "-B", "run_examples.py"], cwd=ws, stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT, env={"PATH": "/usr/bin:/bin", "LC_ALL": "C",
                                                          "PYTHONDONTWRITEBYTECODE": "1"}, timeout=120)
        lines = p.stdout.decode("utf-8", "replace").splitlines()
        print("== %s (exit %d)" % (v, p.returncode))
        print("\n".join(lines[-40:]))


def main(argv):
    if len(argv) < 2:
        raise SystemExit(__doc__)
    tmp = tempfile.mkdtemp(prefix="evo-dbg-dev-")
    try:
        if argv[1] == "cases":
            cmd_cases(tmp)
        elif argv[1] == "partial":
            cmd_partial(tmp)
        elif argv[1] == "score":
            cmd_score(tmp, argv[2:])
        elif argv[1] == "examples":
            cmd_examples(tmp, argv[2:])
        elif argv[1] == "refws":
            v = "all"
            rest = argv[2:]
            if len(rest) == 3 and rest[1] == "--variant":
                v = rest[2]
            if os.path.exists(rest[0]):
                raise SystemExit("%s exists" % rest[0])
            build_ws(rest[0], v)
        else:
            raise SystemExit(__doc__)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
