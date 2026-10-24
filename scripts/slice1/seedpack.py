#!/usr/bin/python3 -I
"""Slice-1 seed packaging (the maintainer's step before `init-root install --seed-source`, root/root.md §4.3
step 7): writes a self-contained seed tree into <dst>, which the harness then commits on branch `seed`,
tags and bundles. Prints one JSON summary line.

  --kind repo      the repository's seed/: `git archive <rev> seed` (default rev HEAD), or the working tree
                   (tracked plus untracked, not ignored files) with --rev worktree
  --kind fakebody  a seed whose `init-body` binary is the kernel test kit's scripted fake body
                   (init_testkit::fakebody, testing.md §3.5): kernel/crates/init-proto and init-testkit at the
                   same rev, a root package `init-body`, and a Cargo.lock cut from kernel/Cargo.lock by cargo
                   itself (offline, the repo vendor/ as the only source)

Either way the seed vendors its own crates (CONVENTIONS §2): the repo's vendor/ cut to the crates its lock files
name, as APFS clones. Crates that only build for Windows or Linux (windows*, winapi*, linux-raw-sys) become stubs
(manifest, empty lib, empty checksum list), as cargo-vendor-filterer does; a `cargo build --frozen` that needed a
stubbed crate would fail loudly."""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys

GIT = "/usr/bin/git"
# crates never built on this host get a stub; LINUX: rustix needs the real linux-raw-sys on Linux
STUB = re.compile(r"^(windows|winapi|linux-raw-sys)" if sys.platform == "darwin" else r"^(windows|winapi)")
# copies: APFS clones on macOS (cp -c), reflinks where the filesystem has them on Linux (GNU cp)
CP = ["/bin/cp", "-c"] if sys.platform == "darwin" else ["/bin/cp", "--reflink=auto", "-p"]
VENDOR_CFG = '[source.crates-io]\nreplace-with = "vendored-sources"\n[source.vendored-sources]\ndirectory = "vendor"\n[net]\noffline = true\n'


def clone(src, to):  # APFS clone (cp -c), keeps the mode
    os.makedirs(os.path.dirname(to), exist_ok=True)
    subprocess.run(CP + [src, to], check=True)


def export(repo, rev, path, dst):
    """<repo>/<path> at <rev> (or the working tree) into <dst>, without the <path> prefix."""
    os.makedirs(dst, exist_ok=True)
    if rev != "worktree":
        a = subprocess.run([GIT, "-C", repo, "archive", rev, path], check=True, capture_output=True).stdout
        n = path.rstrip("/").count("/") + 1
        subprocess.run(["/usr/bin/tar", "-x", "-C", dst, "--strip-components", str(n)], input=a, check=True)
        return sum(len(f) for _, _, f in os.walk(dst))
    out = subprocess.run([GIT, "-C", repo, "ls-files", "-z", "-co", "--exclude-standard", "--", path],
                         check=True, capture_output=True).stdout.split(b"\0")
    files = sorted({f.decode() for f in out if f and os.path.isfile(os.path.join(repo, f.decode()))})
    for f in files:
        clone(os.path.join(repo, f), os.path.join(dst, f[len(path.rstrip("/")) + 1:]))
    return len(files)


def fakebody(repo, rev, dst, cargo, cargo_home):
    n = 0
    for c in ("init-proto", "init-testkit"):  # the manifests and sources only (no tests/, fixtures/)
        tmp = os.path.join(dst, ".x", c)
        export(repo, rev, f"kernel/crates/{c}", tmp)
        for part in ("Cargo.toml", "src"):
            os.renames(os.path.join(tmp, part), os.path.join(dst, "crates", c, part))
        n += sum(len(f) for _, _, f in os.walk(os.path.join(dst, "crates", c)))
    shutil.rmtree(os.path.join(dst, ".x"))
    # the kernel workspace's pins for exactly the dependencies these two crates inherit
    ws = open(os.path.join(repo, "kernel/Cargo.toml")).read()
    pins = []
    for dep in ("serde", "serde_json", "toml", "sha2", "libc"):
        m = re.search(rf"^{re.escape(dep)} = .*$", ws, re.M)
        if not m:
            sys.exit(f"kernel/Cargo.toml pins no {dep}")
        pins.append(m.group(0).split("#")[0].rstrip())
    with open(os.path.join(dst, "Cargo.toml"), "w") as f:
        f.write("# The Slice-1 fake-body seed (scripts/slice1/seedpack.py): `init-body` is init_testkit::fakebody.\n"
                "[package]\nname = \"init-body\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n"
                "[[bin]]\nname = \"init-body\"\npath = \"src/main.rs\"\n\n"
                "[dependencies]\ninit-testkit = { path = \"crates/init-testkit\" }\n\n"
                "[workspace]\nresolver = \"3\"\nmembers = [\".\", \"crates/init-proto\", \"crates/init-testkit\"]\n\n"
                "[workspace.package]\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n"
                "[workspace.dependencies]\ninit-proto = { path = \"crates/init-proto\" }\n" + "\n".join(pins) + "\n")
    os.makedirs(os.path.join(dst, "src"), exist_ok=True)
    with open(os.path.join(dst, "src/main.rs"), "w") as f:
        f.write("//! The Slice-1 fake body: the kernel test kit's scripted body (testing.md §3.5) as `init-body`.\n"
                "fn main() {\n    std::process::exit(init_testkit::fakebody::main());\n}\n")
    with open(os.path.join(dst, "GENESIS.md"), "w") as f:
        f.write("# GENESIS\n\nThe Slice-1 fake-body seed. `init-body` is the kernel test kit's scripted fake body; it has no\n"
                "model loop. It exists so the harness can check the Root, the kernel and the install path without the\n"
                "real body.\n")
    os.makedirs(os.path.join(dst, ".cargo"), exist_ok=True)
    with open(os.path.join(dst, ".cargo/config.toml"), "w") as f:
        f.write(VENDOR_CFG)
    # Cargo.lock: the kernel's lock, pruned to this graph by cargo (offline; the repo vendor/ is the only source),
    # so every crate keeps the version the kernel lanes build and test with.
    shutil.copyfile(os.path.join(repo, "kernel/Cargo.lock"), os.path.join(dst, "Cargo.lock"))
    env = {"PATH": f"{os.path.dirname(cargo)}:/usr/bin:/bin", "HOME": cargo_home, "CARGO_HOME": cargo_home}
    os.makedirs(cargo_home, exist_ok=True)
    vend = os.path.join(repo, "vendor").replace("\\", "\\\\").replace('"', '\\"')
    subprocess.run([cargo, "metadata", "--offline", "--format-version", "1",
                    "--config", f'source.vendored-sources.directory="{vend}"'],
                   cwd=dst, env=env, check=True, stdout=subprocess.DEVNULL)
    return n


def cut_vendor(repo, dst):
    want = set()
    for lock in ("Cargo.lock", "ext/Cargo.lock"):
        p = os.path.join(dst, lock)
        if os.path.isfile(p):
            for n, v in re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"\nsource = "registry', open(p).read()):
                want.add((n, v))
    vend = os.path.join(repo, "vendor")
    names = set(os.listdir(vend))

    def pkg_version(d):
        m = re.search(r'^\[package\][^\[]*?^version = "([^"]+)"', open(os.path.join(vend, d, "Cargo.toml")).read(), re.M | re.S)
        return m and m.group(1)

    copied = stubbed = 0
    for n, v in sorted(want):
        d = next((c for c in (f"{n}-{v}", n) if c in names and pkg_version(c) == v), None)
        if d is None:
            sys.exit(f"vendor/ has no {n} {v}")
        to = os.path.join(dst, "vendor", d)
        if STUB.match(n):  # never built on this host: manifest, empty lib, empty checksum list
            clone(os.path.join(vend, d, "Cargo.toml"), os.path.join(to, "Cargo.toml"))
            os.makedirs(os.path.join(to, "src"), exist_ok=True)
            open(os.path.join(to, "src/lib.rs"), "w").close()
            ck = json.load(open(os.path.join(vend, d, ".cargo-checksum.json")))
            json.dump({"files": {}, "package": ck.get("package")}, open(os.path.join(to, ".cargo-checksum.json"), "w"))
            stubbed += 1
        else:
            os.makedirs(os.path.dirname(to), exist_ok=True)
            subprocess.run(CP + ["-R", os.path.join(vend, d), to], check=True)
            copied += 1
    return copied, stubbed


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True)
    ap.add_argument("--dst", required=True, help="must not exist")
    ap.add_argument("--kind", choices=("repo", "fakebody"), required=True)
    ap.add_argument("--rev", default="HEAD", help="a git rev, or `worktree`")
    ap.add_argument("--cargo", help="the toolchain's cargo (fakebody: prunes the lock, offline)")
    ap.add_argument("--cargo-home", help="an empty CARGO_HOME for that call (fakebody)")
    a = ap.parse_args()
    os.makedirs(a.dst)
    if a.kind == "repo":
        n = export(a.repo, a.rev, "seed", a.dst)
        cfg = os.path.join(a.dst, ".cargo/config.toml")  # the seed repo vendors its own crates
        with open(cfg) as f:
            t = f.read()
        with open(cfg, "w") as f:
            f.write(t.replace('directory = "../vendor"', 'directory = "vendor"'))
    else:
        if not (a.cargo and a.cargo_home):
            sys.exit("--kind fakebody needs --cargo and --cargo-home")
        n = fakebody(a.repo, a.rev, a.dst, a.cargo, a.cargo_home)
    copied, stubbed = cut_vendor(a.repo, a.dst)
    print(json.dumps({"kind": a.kind, "source": a.rev, "seed_files": n, "crates": copied, "stubs": stubbed}))


if __name__ == "__main__":
    main()
