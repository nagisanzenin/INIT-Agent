"""tgen: shared generator helpers for family T (macOS terminal mini-tasks) of pack s1.

Written fresh for INIT (INIT licence). Python >= 3.9, stdlib only. Used by every T/<template>/gen/run:

    import os, sys
    sys.dont_write_bytecode = True
    HERE = os.path.dirname(os.path.abspath(__file__))
    sys.path.insert(0, os.path.join(HERE, "..", "..", "_shared"))
    import tgen                      # also puts <pack>/lib on sys.path
    s1lib = tgen.s1lib

    def build(ctx):
        inst = s1lib.Instance(ctx, prompt=...)
        ...
        tgen.finish(inst, ctx, spec={...}, names=[...])
        return inst

    if __name__ == "__main__":
        s1lib.main(build)

The check of every T instance is one `sh` step of the shared runner: hidden/check.sh finds python3 >= 3.9 and
runs hidden/tcheck.py, which is _shared/tcheck_common.py followed by the template's check.py. The checker
reads hidden/spec.json (expected values) and prints `PASS <name>` / `FAIL <name>` lines.
"""
from __future__ import annotations

import hashlib
import json
import os
import struct
import sys
import zlib

SHARED = os.path.dirname(os.path.abspath(__file__))
PACK_ROOT = os.path.dirname(os.path.dirname(SHARED))
sys.path.insert(0, os.path.join(PACK_ROOT, "lib"))
import s1lib  # noqa: E402

CHECK_SH = """#!/bin/sh
# INIT pack s1, family T: hidden check entry (run by the shared runner's sh step; cwd = the workspace).
d=$(cd "$(dirname "$0")" && pwd -P) || { echo "FAIL check_dir"; exit 0; }
for py in python3 /usr/bin/python3 /usr/local/bin/python3 /opt/homebrew/bin/python3; do
  if command -v "$py" >/dev/null 2>&1 &&
     "$py" -I -S -c 'import sys; sys.exit(0 if sys.version_info >= (3, 9) else 1)' >/dev/null 2>&1; then
    exec "$py" -I -S -B "$d/tcheck.py" "$d"
  fi
done
echo "FAIL toolchain_python3"
"""


def read_shared(name: str) -> str:
    with open(os.path.join(SHARED, name), "rb") as f:
        return f.read().decode("utf-8")


def finish(inst, ctx, spec: dict, names: list, check_file: str = "check.py", timeout_s: int = 30,
           weight: int = 1) -> None:
    """Adds the hidden checker (check.sh, tcheck.py, spec.json) and its sh step to the instance."""
    if len(set(names)) != len(names) or not names:
        raise s1lib.GenError("check names must be unique and non-empty")
    body = ctx.read(check_file)
    tcheck = read_shared("tcheck_common.py") + "\n\n# ---- template check: %s/%s\n\n" % (
        ctx.template_id, check_file) + body + "\n\nif __name__ == \"__main__\":\n    main()\n"
    spec = dict(spec)
    spec["_names"] = list(names)
    inst.hidden["hidden/check.sh"] = CHECK_SH
    inst.hidden["hidden/tcheck.py"] = tcheck
    inst.hidden["hidden/spec.json"] = json.dumps(spec, sort_keys=True, indent=1) + "\n"
    inst.step({"kind": "sh", "name": ctx.template_id, "script": "hidden/check.sh", "names": list(names),
               "timeout_s": timeout_s, "weight": weight})


# ------------------------------------------------------------------------------------------ text helpers


def lines(*ls: str) -> str:
    return "\n".join(ls) + "\n"


def b64(data: bytes) -> str:
    import base64
    return base64.b64encode(data).decode("ascii")


# ------------------------------------------------------------------------------------------ git writer


class Git:
    """A tiny deterministic git repository writer (loose objects, refs, index v2) for workspaces.

    Everything is a function of the inputs: fixed author dates, zlib level 9, sorted trees. The result is a
    {path: bytes} dict rooted at `prefix` (e.g. "repo/.git/..."), to merge into Instance.workspace."""

    def __init__(self, prefix: str = "repo", user: tuple = ("Dev", "dev@example.invalid")):
        self.prefix = prefix.rstrip("/")
        self.objects = {}           # sha -> compressed bytes
        self.refs = {}              # "refs/heads/main" -> sha
        self.head = "refs/heads/main"
        self.user = user

    def _git(self, rel: str) -> str:
        return "%s/.git/%s" % (self.prefix, rel) if self.prefix else ".git/" + rel

    def obj(self, kind: str, data: bytes) -> str:
        raw = kind.encode("ascii") + b" " + str(len(data)).encode("ascii") + b"\0" + data
        sha = hashlib.sha1(raw).hexdigest()
        if sha not in self.objects:
            self.objects[sha] = zlib.compress(raw, 9)
        return sha

    def blob(self, data) -> str:
        return self.obj("blob", data if isinstance(data, bytes) else data.encode("utf-8"))

    def tree_of(self, files: dict, executable: frozenset = frozenset()) -> str:
        """files: {relative path: str|bytes} (a full snapshot) -> root tree sha."""
        root = {}
        for path, data in files.items():
            parts = path.split("/")
            node = root
            for p in parts[:-1]:
                node = node.setdefault(p, {})
                if not isinstance(node, dict):
                    raise s1lib.GenError("git: %s is both a file and a directory" % path)
            node[parts[-1]] = (self.blob(data), "100755" if path in executable else "100644")
        return self._tree(root)

    def _tree(self, node: dict) -> str:
        entries = []
        for name, v in node.items():
            if isinstance(v, dict):
                entries.append((name + "/", "40000", name, self._tree(v)))
            else:
                entries.append((name, v[1], name, v[0]))
        entries.sort(key=lambda e: e[0].encode("utf-8"))
        data = b"".join(mode.encode() + b" " + name.encode("utf-8") + b"\0" + bytes.fromhex(sha)
                        for _, mode, name, sha in entries)
        return self.obj("tree", data)

    def commit(self, tree: str, parents: list, message: str, when: int, author: tuple | None = None,
               committer: tuple | None = None, tz: str = "+0000") -> str:
        a = author or self.user
        c = committer or a
        body = "tree %s\n" % tree
        for p in parents:
            body += "parent %s\n" % p
        body += "author %s <%s> %d %s\n" % (a[0], a[1], when, tz)
        body += "committer %s <%s> %d %s\n" % (c[0], c[1], when, tz)
        body += "\n" + message.rstrip("\n") + "\n"
        return self.obj("commit", body.encode("utf-8"))

    def index(self, files: dict, executable: frozenset = frozenset()) -> bytes:
        """Index v2 for a snapshot, with zeroed stat data (git refreshes it on the first status)."""
        ents = []
        for path in sorted(files, key=lambda p: p.encode("utf-8")):
            data = files[path] if isinstance(files[path], bytes) else files[path].encode("utf-8")
            sha = self.blob(data)
            mode = 0o100755 if path in executable else 0o100644
            name = path.encode("utf-8")
            e = struct.pack(">10I", 0, 0, 0, 0, 0, 0, mode, 0, 0, len(data)) + bytes.fromhex(sha)
            e += struct.pack(">H", min(len(name), 0xFFF)) + name
            pad = 8 - (len(e) % 8)
            ents.append(e + b"\0" * pad)
        body = b"DIRC" + struct.pack(">II", 2, len(ents)) + b"".join(ents)
        return body + hashlib.sha1(body).digest()

    def files(self, worktree: dict | None = None, executable: frozenset = frozenset(),
              config_extra: str = "") -> dict:
        """The .git directory (and optionally the checked-out worktree plus a matching index)."""
        out = {}
        for sha, comp in self.objects.items():
            out[self._git("objects/%s/%s" % (sha[:2], sha[2:]))] = comp
        for ref, sha in self.refs.items():
            out[self._git(ref)] = sha + "\n"
        out[self._git("HEAD")] = "ref: %s\n" % self.head
        out[self._git("config")] = (
            "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n"
            "\tlogallrefupdates = true\n[user]\n\tname = %s\n\temail = %s\n%s" % (
                self.user[0], self.user[1], config_extra))
        out[self._git("description")] = "Unnamed repository.\n"
        if worktree is not None:
            for p, data in worktree.items():
                out[("%s/%s" % (self.prefix, p)) if self.prefix else p] = data
            out[self._git("index")] = self.index(worktree, executable)
        return out
