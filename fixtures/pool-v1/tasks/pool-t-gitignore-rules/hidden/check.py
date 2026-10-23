"""pool-t-gitignore-rules check: copies every .gitignore of the workspace into a fresh repo, creates the
probe paths there and asks `git check-ignore --no-index` about each (global, system and info/exclude
rules are off, so only the workspace's .gitignore files count)."""
import os
import shutil
import subprocess
import sys

CD = os.environ["INIT_CHECK_DIR"]
SCRATCH = os.environ["S1_SCRATCH"]


def main():
    probes = []
    for line in open(os.path.join(CD, "hidden", "probes.txt")):
        if line.strip() and not line.startswith("#"):
            want, name, path = line.split()
            probes.append((want, name, path))
    repo = os.path.join(SCRATCH, "repo")
    os.makedirs(repo)
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": SCRATCH, "GIT_CONFIG_NOSYSTEM": "1",
           "GIT_CONFIG_GLOBAL": "/dev/null", "LC_ALL": "C"}
    if subprocess.run(["git", "init", "-q", "--template=", repo], env=env).returncode != 0:
        for _, name, _ in probes:
            print("FAIL %s" % name)
        return
    ws = os.getcwd()
    for dirpath, dirnames, filenames in os.walk(ws):
        dirnames[:] = [d for d in dirnames if d != ".git"]
        if ".gitignore" in filenames:
            src = os.path.join(dirpath, ".gitignore")
            if os.path.islink(src):
                continue
            rel = os.path.relpath(src, ws)
            dst = os.path.join(repo, rel)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copyfile(src, dst)
    for _, _, path in probes:
        p = os.path.join(repo, path)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        if not os.path.exists(p):
            with open(p, "w") as f:
                f.write("probe\n")
    for want, name, path in probes:
        r = subprocess.run(["git", "-c", "core.excludesFile=/dev/null", "check-ignore", "-q", "--no-index", path],
                           cwd=repo, env=env)
        got = "I" if r.returncode == 0 else ("T" if r.returncode == 1 else "?")
        print("%s %s" % ("PASS" if got == want else "FAIL", name))
        if got != want:
            sys.stderr.write("%s: %s is %s, want %s\n" % (name, path, got, want))


main()
