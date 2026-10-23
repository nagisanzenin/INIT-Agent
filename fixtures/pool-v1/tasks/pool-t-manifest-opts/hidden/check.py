"""pool-t-manifest-opts check: runs `sh <ws>/mkmanifest.sh ...` in fresh fixture trees and compares with an
independent Python model of README.md's contract."""
import fnmatch
import os
import shutil
import subprocess
import sys

WS = os.getcwd()
SCRATCH = os.environ["S1_SCRATCH"]
SCRIPT = os.path.join(WS, "mkmanifest.sh")
ENV = {"PATH": "/usr/bin:/bin", "HOME": SCRATCH, "LC_ALL": "C", "TMPDIR": SCRATCH}

TREE = {
    "proj/a.txt": "a", "proj/b.tmp": "b", "proj/sub/c.txt": "c", "proj/sub/d.bak": "d",
    "proj/sub/deep/E.txt": "e", "proj/notes file.md": "n", "proj/z-last.txt": "z",
    "my docs/one.txt": "1", "my docs/two words.txt": "2", "my docs/skip.bak": "s",
    "-dash/x.txt": "x", "empty/.keep": "k",
}


def fixture(tag):
    root = os.path.join(SCRATCH, tag)
    for rel, data in TREE.items():
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(data)
    os.makedirs(os.path.join(root, "proj", "emptydir"), exist_ok=True)
    os.symlink("a.txt", os.path.join(root, "proj", "link.txt"))
    return root


def model(root, dirs, name=None, excludes=()):
    lines = []
    for d in dirs:
        for dp, dn, fn in os.walk(os.path.join(root, d)):
            for f in fn:
                full = os.path.join(dp, f)
                if os.path.islink(full) or not os.path.isfile(full):
                    continue
                if any(fnmatch.fnmatchcase(f, p) for p in excludes):
                    continue
                lines.append(d + full[len(os.path.join(root, d)):])
    lines.sort(key=lambda s: s.encode())
    return "# manifest: %s\n" % (name or os.path.basename(dirs[0])) + "".join(l + "\n" for l in lines)


def run(root, args):
    p = subprocess.run(["/bin/sh", SCRIPT] + args, cwd=root, env=ENV, stdout=subprocess.PIPE,
                       stderr=subprocess.PIPE, timeout=10)
    return p.returncode, p.stdout.decode("utf-8", "replace"), p.stderr.decode("utf-8", "replace")


def case(name, fn):
    try:
        ok = fn(fixture(name))
    except Exception as e:  # noqa: BLE001
        sys.stderr.write("%s: %s\n" % (name, e))
        ok = False
    print("%s %s" % ("PASS" if ok else "FAIL", name))


def main():
    if not os.path.isfile(SCRIPT):
        for n in NAMES:
            print("FAIL %s" % n)
        return
    case("basic", lambda r: run(r, ["proj"]) == (0, model(r, ["proj"]), ""))
    case("name_option", lambda r: run(r, ["-n", "release 2", "proj"])[:2] == (0, model(r, ["proj"], "release 2")))
    def out_file(r):
        code, out, err = run(r, ["-o", "out/m.txt", "-n", "x", "proj"]) if os.makedirs(os.path.join(r, "out")) is None else (1, "", "")
        return code == 0 and out == "" and open(os.path.join(r, "out", "m.txt")).read() == model(r, ["proj"], "x")
    case("out_file", out_file)
    case("out_dash", lambda r: run(r, ["-o", "-", "proj"])[:2] == (0, model(r, ["proj"])))
    case("one_exclude", lambda r: run(r, ["-x", "*.tmp", "proj"])[:2] == (0, model(r, ["proj"], None, ["*.tmp"])))
    case("two_excludes", lambda r: run(r, ["-x", "*.tmp", "-x", "*.bak", "proj"])[:2] == (0, model(r, ["proj"], None, ["*.tmp", "*.bak"])))
    case("exclude_with_space", lambda r: run(r, ["-x", "notes file.md", "-x", "?.txt", "proj"])[:2] == (0, model(r, ["proj"], None, ["notes file.md", "?.txt"])))
    case("dirs_with_spaces", lambda r: run(r, ["-x", "*.bak", "my docs", "proj/sub"])[:2] == (0, model(r, ["my docs", "proj/sub"], None, ["*.bak"])))
    def verbose(r):
        code, out, err = run(r, ["-v", "-x", "*.bak", "my docs"])
        n = len(model(r, ["my docs"], None, ["*.bak"]).splitlines()) - 1
        return code == 0 and out == model(r, ["my docs"], None, ["*.bak"]) and err.strip() == "%d files" % n
    case("verbose", verbose)
    case("double_dash", lambda r: run(r, ["--", "-dash"])[:2] == (0, model(r, ["-dash"])))
    def unknown_option(r):
        code, out, err = run(r, ["-q", "proj"])
        return code == 2 and out == "" and any(l.startswith("usage:") for l in err.splitlines())
    case("unknown_option", unknown_option)
    def missing_dir(r):
        code, out, err = run(r, ["-n", "x"])
        return code == 2 and out == "" and any(l.startswith("usage:") for l in err.splitlines())
    case("missing_dir", missing_dir)
    def not_a_dir(r):
        code, out, err = run(r, ["proj/a.txt"])
        return code == 2 and err.strip() != ""
    case("not_a_dir", not_a_dir)


NAMES = ["basic", "name_option", "out_file", "out_dash", "one_exclude", "two_excludes", "exclude_with_space",
         "dirs_with_spaces", "verbose", "double_dash", "unknown_option", "missing_dir", "not_a_dir"]
main()
