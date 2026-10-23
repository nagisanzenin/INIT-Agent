"""s1lib: the shared generator library of pack s1 (docs/m7-design.md §3.1.2). Python >= 3.9, stdlib only.

Every template's `gen/run <seed> <outdir>` is a small Python script that builds an `Instance` and hands it
to this library, which writes the instance deterministically (see FORMAT.md for the contract):

    #!/usr/bin/env python3
    import os, sys
    sys.dont_write_bytecode = True
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "lib"))
    import s1lib

    def build(ctx):
        rng = ctx.rng()
        inst = s1lib.Instance(ctx, prompt="...")
        ...
        return inst

    if __name__ == "__main__":
        s1lib.main(build)

Determinism: all randomness comes from `ctx.rng(...)`, a SHA-256 counter stream keyed by the seed and the
template id; nothing reads the clock, the environment (beyond the documented INIT_S1_* inputs) or
/dev/urandom. Tars are written with sorted entries, fixed mtime, uid/gid 0 and fixed modes.
"""
from __future__ import annotations

import calendar
import hashlib
import io
import json
import os
import re
import subprocess
import sys
import tarfile

PACK_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUNNER_DIR = os.path.join(PACK_ROOT, "runner")
RUNNER_FILES = (("check", "run", 0o755), ("s1check.py", "s1check.py", 0o644))  # (pack file, check/ name, mode)
CREATED = "2026-10-20T00:00:00Z"
MTIME = calendar.timegm((2026, 10, 20, 0, 0, 0))
FAMILIES = ("E", "B", "C", "T", "canary")
TASK_FAMILY = {"E": "e", "B": "b", "C": "c", "T": "t", "canary": "canary"}  # task.json family values
MAX_TURNS = {"E": 8, "C": 8, "T": 10, "B": 12, "canary": 8}  # m7-design §3.1.3
LIMITS = {"wall_s": 600, "max_cost_musd": 100_000, "max_output_tokens": 24_000, "max_input_tokens": 400_000}
CHECK_TIMEOUT_S = 60
DEV_GUID = "0" * 32
N_AUTO_TOKENS = 3
HEX64 = re.compile(r"^[0-9a-f]{64}$")
HEX32 = re.compile(r"^[0-9a-f]{32}$")
ID_RE = re.compile(r"^[a-z0-9._-]{1,64}$")
LABEL_RE = re.compile(r"^[a-z0-9-]{1,40}$")

# comment syntax for the marker lines (GUID and seeded tokens), by suffix or basename
_HASH = (".py", ".sh", ".bash", ".toml", ".cfg", ".ini", ".yml", ".yaml", ".mk", ".pl", ".rb", ".r")
_SLASH = (".rs", ".c", ".h", ".go", ".js", ".ts", ".java", ".cpp", ".hpp", ".swift", ".kt")
_HASH_NAMES = ("Makefile", "makefile", "GNUmakefile", "Dockerfile", ".gitignore", ".editorconfig")


class GenError(Exception):
    """The template produced an invalid instance."""


# --------------------------------------------------------------------------------------------- RNG


class Rng:
    """A SHA-256 counter stream: stable across Python versions and platforms (unlike `random`)."""

    def __init__(self, key: bytes, stream: str):
        self._key = hashlib.sha256(b"s1/rng/" + stream.encode("utf-8") + b"/" + key).digest()
        self._ctr = 0
        self._buf = b""

    def bytes(self, n: int) -> bytes:
        while len(self._buf) < n:
            self._buf += hashlib.sha256(self._key + self._ctr.to_bytes(8, "big")).digest()
            self._ctr += 1
        out, self._buf = self._buf[:n], self._buf[n:]
        return out

    def u64(self) -> int:
        return int.from_bytes(self.bytes(8), "big")

    def below(self, n: int) -> int:
        """Uniform in [0, n) by rejection sampling."""
        if n <= 0:
            raise ValueError("below(n) needs n > 0")
        limit = (1 << 64) - ((1 << 64) % n)
        while True:
            x = self.u64()
            if x < limit:
                return x % n

    def randint(self, a: int, b: int) -> int:
        """Uniform in [a, b], both ends included."""
        return a + self.below(b - a + 1)

    def choice(self, seq):
        return seq[self.below(len(seq))]

    def shuffled(self, seq) -> list:
        out = list(seq)
        for i in range(len(out) - 1, 0, -1):
            j = self.below(i + 1)
            out[i], out[j] = out[j], out[i]
        return out

    def sample(self, seq, k: int) -> list:
        return self.shuffled(seq)[:k]

    def chance(self, num: int, den: int) -> bool:
        return self.below(den) < num

    def hex(self, nbytes: int) -> str:
        return self.bytes(nbytes).hex()

    def word(self, n: int = 6, alphabet: str = "abcdefghjkmnpqrstuvwxyz") -> str:
        return "".join(alphabet[self.below(len(alphabet))] for _ in range(n))

    def ident(self, prefix: str, n: int = 5) -> str:
        """A seeded identifier such as `parse_kqzmw`; register it with Instance.token() when it should
        count as an instance token (it must then be >= 8 characters)."""
        return "%s_%s" % (prefix, self.word(n))

    def fork(self, stream: str) -> "Rng":
        return Rng(self.bytes(32), stream)


def auto_tokens(seed: bytes, template_id: str) -> list:
    """The instance tokens every instance carries (m7-design §3.5, M7-P09): 3 × 12 hex (48 bits each),
    a function of (seed, template) only."""
    r = Rng(seed, template_id + "/tokens")
    return ["%s" % r.hex(6) for _ in range(N_AUTO_TOKENS)]


# --------------------------------------------------------------------------------------------- ctx


def load_template(template_dir: str) -> dict:
    with open(os.path.join(template_dir, "template.json"), "rb") as f:
        return json.loads(f.read().decode("utf-8"))


class Ctx:
    """What a template's build() gets: the seed, the knobs, and a seeded RNG."""

    def __init__(self, template_dir: str, seed_hex: str, guid: str = DEV_GUID, difficulty: int | None = None,
                 dev: bool = False):
        if not HEX64.match(seed_hex):
            raise GenError("seed must be 64 lowercase hex characters")
        if not HEX32.match(guid):
            raise GenError("INIT_S1_GUID must be 32 lowercase hex characters")
        self.template_dir = os.path.abspath(template_dir)
        self.template = load_template(self.template_dir)
        self.template_id = self.template["id"]
        self.family = self.template["family"]
        if self.family not in FAMILIES:
            raise GenError("template.json: unknown family %r" % self.family)
        self.seed_hex = seed_hex
        self.seed = bytes.fromhex(seed_hex)
        self.guid = guid
        d = self.template["difficulty"]
        self.difficulty = d["default"] if difficulty is None else difficulty
        if not d["min"] <= self.difficulty <= d["max"]:
            raise GenError("difficulty %d outside %d..%d" % (self.difficulty, d["min"], d["max"]))
        self.dev = dev  # dev only (validator): also write dev/ fixtures

    def rng(self, stream: str = "main") -> Rng:
        return Rng(self.seed, self.template_id + "/" + stream)

    def path(self, *parts: str) -> str:
        """A file inside the template directory (data files, reference sources)."""
        return os.path.join(self.template_dir, *parts)

    def read(self, *parts: str) -> str:
        with open(self.path(*parts), "rb") as f:
            return f.read().decode("utf-8")


# ---------------------------------------------------------------------------------------- instance


def _b(v) -> bytes:
    return v if isinstance(v, bytes) else v.encode("utf-8")


def _comment(path: str, text: str) -> str | None:
    base = os.path.basename(path)
    if base in _HASH_NAMES or path.endswith(_HASH):
        return "# %s\n" % text
    if path.endswith(_SLASH):
        return "// %s\n" % text
    if path.endswith((".md", ".html", ".xml")):
        return "<!-- %s -->\n" % text
    if path.endswith((".sql", ".lua", ".hs")):
        return "-- %s\n" % text
    return None


def _insert_marker(path: str, data: bytes, line: str) -> bytes:
    text = data.decode("utf-8")
    if text.startswith("#!"):
        nl = text.find("\n")
        if nl < 0:
            return (text + "\n" + line).encode("utf-8")
        return (text[:nl + 1] + line + text[nl + 1:]).encode("utf-8")
    return (line + text).encode("utf-8")


def rel_ok(p: str) -> bool:
    if not isinstance(p, str) or not p or len(p) > 255 or "\0" in p or "\\" in p or p.startswith("/"):
        return False
    return all(part not in ("", ".", "..") for part in p.split("/"))


def make_tar(files: dict, executable: set = frozenset()) -> bytes:
    """A deterministic POSIX tar (pax-compatible, uncompressed): sorted entries, explicit parent
    directories, mtime fixed, uid/gid 0, modes 0644/0755. Regular files and directories only."""
    dirs = set()
    for p in files:
        if not rel_ok(p):
            raise GenError("bad workspace path %r" % p)
        parts = p.split("/")
        for i in range(1, len(parts)):
            dirs.add("/".join(parts[:i]))
    clash = dirs & set(files)
    if clash:
        raise GenError("path is both a file and a directory: %s" % sorted(clash))
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.PAX_FORMAT) as tf:
        for name in sorted(dirs | set(files)):
            ti = tarfile.TarInfo(name)
            ti.mtime, ti.uid, ti.gid, ti.uname, ti.gname = MTIME, 0, 0, "", ""
            if name in dirs:
                ti.type, ti.mode = tarfile.DIRTYPE, 0o755
                tf.addfile(ti)
            else:
                data = _b(files[name])
                ti.size, ti.mode = len(data), (0o755 if name in executable else 0o644)
                tf.addfile(ti, io.BytesIO(data))
    return buf.getvalue()


def dumps(obj) -> bytes:
    return (json.dumps(obj, sort_keys=True, indent=1, ensure_ascii=True) + "\n").encode("ascii")


class Instance:
    """One generated task instance. Fill it in build(); the library writes it.

    workspace   {path: str|bytes}  the initial workspace (workspace.tar)
    solution    {path: str|bytes|None}  overlay on the workspace giving a passing workspace (solution.tar);
                None deletes a file. For canaries, see FORMAT.md (honest solution).
    executable  workspace paths written with mode 0755
    hidden      {check-relative path under hidden/: str|bytes}  hidden tests and data (check dir)
    steps       check.json steps (see FORMAT.md)
    protected   workspace paths whose sha256 goes into check/protected.json (visible tests, fixtures)
    """

    def __init__(self, ctx: Ctx, prompt: str, title: str | None = None):
        self.ctx = ctx
        self.prompt = prompt
        self.title = title or ctx.template["title"]
        self.workspace = {}
        self.solution = {}
        self.executable = set()
        self.hidden = {}
        self.steps = []
        self.scoring = "fraction"
        self.on_tamper = "zero"
        self.budget_s = None
        self.protected = []
        self.extra_tokens = []
        self.marker_files = None
        self.secret = None      # canary leak: written to check/secret (Root-held only)
        self.canary = None      # canaries: dict for canary.json, e.g. {"kind": "leak"}
        self.limits = {}        # overrides of the family defaults (max_turns, ...)
        self.labels = []
        self.dev = {}           # dev fixtures: name -> overlay (written to dev/<name>.tar only with INIT_S1_DEV=1)

    # -- helpers used by build()
    def protect(self, *paths: str) -> None:
        for p in paths:
            if p not in self.protected:
                self.protected.append(p)

    def token(self, s: str) -> str:
        """Registers an extra seeded token (>= 8 chars) that appears in the workspace."""
        if len(s) < 8:
            raise GenError("token %r shorter than 8 characters" % s)
        self.extra_tokens.append(s)
        return s

    def step(self, spec: dict) -> None:
        self.steps.append(spec)

    def notice(self, path: str = "NOTICE") -> None:
        """E and C: put the template's NOTICE plus its LICENSE text into the workspace (MIT attribution)."""
        text = self.ctx.read("NOTICE").rstrip("\n") + "\n\n" + self.ctx.read("LICENSE")
        self.workspace[path] = text

    # -- writing
    def _markers(self) -> dict:
        """path -> marker line: the install GUID with token 1 in the first marker file, then one token per
        further marker file (round-robin), so that a partial copy still carries tokens."""
        toks = auto_tokens(self.ctx.seed, self.ctx.template_id)
        files = self.marker_files
        if files is None:
            files = [p for p in sorted(self.workspace) if _comment(p, "x") is not None][:N_AUTO_TOKENS]
        if not files:
            raise GenError("no workspace file takes a comment line; set marker_files or add e.g. a README.md")
        lines = {}
        for i, tok in enumerate(toks):
            p = files[i % len(files)]
            if _comment(p, "x") is None:
                raise GenError("marker file %s has no known comment syntax" % p)
            text = "ref: %s %s" % (self.ctx.guid, tok) if i == 0 else "ref: %s" % tok
            lines[p] = lines.get(p, "") + _comment(p, text)
        return lines

    def materialize(self) -> dict:
        """Everything write() puts on disk, as {relative path: (bytes, mode)}."""
        ctx = self.ctx
        if not self.prompt or len(self.prompt) > 32768:
            raise GenError("prompt must be 1..32768 characters")
        if len(self.title) > 120:
            raise GenError("title longer than 120 characters")
        markers = self._markers()
        ws = {p: _b(v) for p, v in self.workspace.items()}
        for p, line in markers.items():
            ws[p] = _insert_marker(p, ws[p], line)
        sol = dict(ws)
        for p, v in self.solution.items():
            if v is None:
                sol.pop(p, None)
            else:
                sol[p] = _insert_marker(p, _b(v), markers[p]) if p in markers else _b(v)
        for p in self.protected:
            if p not in ws:
                raise GenError("protected path %s is not in the workspace" % p)
            if sol.get(p) != ws[p]:
                raise GenError("the solution must not change protected file %s" % p)
        tokens = auto_tokens(ctx.seed, ctx.template_id) + list(self.extra_tokens)
        blob = b"\n".join(ws.values())
        for t in tokens:
            if t.encode("utf-8") not in blob:
                raise GenError("token %s does not appear in the workspace" % t)
        for p in self.hidden:
            if not rel_ok(p) or not p.startswith("hidden/"):
                raise GenError("hidden files live under hidden/: %r" % p)
        exe = {p for p in self.executable if p in ws}
        ws_tar = make_tar(ws, exe)
        sol_tar = make_tar(sol, {p for p in self.executable if p in sol})
        out = {}
        for src, name, mode in RUNNER_FILES:
            with open(os.path.join(RUNNER_DIR, src), "rb") as f:
                out["check/" + name] = (f.read(), mode)
        check = {"v": 1, "runner": "s1check/1", "scoring": self.scoring, "steps": self.steps}
        if self.on_tamper != "zero":
            check["on_tamper"] = self.on_tamper
        if self.budget_s is not None:
            check["budget_s"] = self.budget_s
        out["check/check.json"] = (dumps(check), 0o644)
        prot = [{"path": p, "sha256": hashlib.sha256(ws[p]).hexdigest()} for p in sorted(self.protected)]
        out["check/protected.json"] = (dumps(prot), 0o644)
        for p, v in self.hidden.items():
            out["check/" + p] = (_b(v), 0o644)
        if self.secret is not None:
            out["check/secret"] = (_b(self.secret) + b"\n", 0o644)
        fam = ctx.family
        limits = dict(LIMITS, max_turns=MAX_TURNS[fam])
        limits.update(self.limits)
        lang = ctx.template.get("lang", "any")
        labels = ["s1", TASK_FAMILY[fam]] + ([lang] if lang != "any" else []) + list(self.labels)
        for lb in labels:
            if not LABEL_RE.match(lb):
                raise GenError("bad label %r" % lb)
        task = {
            "v": 3,
            "id": "s1." + hashlib.sha256(b"s1/id/" + ctx.seed).hexdigest()[:12],  # the Root assigns its own id at draw
            "title": self.title,
            "prompt": self.prompt,
            "labels": labels,
            "workspace": {"file": "workspace.tar", "sha256": hashlib.sha256(ws_tar).hexdigest(), "bytes": len(ws_tar)},
            "check": {"argv": ["check/run"], "timeout_s": CHECK_TIMEOUT_S},
            "limits": limits,
            "source": {"kind": "maintainer"},
            "created": CREATED,
            "family": TASK_FAMILY[fam],
            "template": ctx.template_id,
            "difficulty": ctx.difficulty,
        }
        out["task.json"] = (dumps(task), 0o644)
        out["workspace.tar"] = (ws_tar, 0o644)
        out["solution.tar"] = (sol_tar, 0o644)
        out["tokens.json"] = (dumps({"v": 1, "tokens": tokens}), 0o644)
        if self.canary is not None:
            c = dict(self.canary, v=1)
            if self.secret is not None:
                c["secret_sha256"] = hashlib.sha256(_b(self.secret)).hexdigest()
            out["canary.json"] = (dumps(c), 0o644)
        if ctx.dev:
            for name, overlay in sorted(self.dev.items()):
                files = dict(ws)
                for p, v in overlay.items():
                    if v is None:
                        files.pop(p, None)
                    else:
                        files[p] = _insert_marker(p, _b(v), markers[p]) if p in markers else _b(v)
                out["dev/%s.tar" % name] = (make_tar(files, {p for p in self.executable if p in files}), 0o644)
        return out

    def write(self, outdir: str) -> None:
        files = self.materialize()
        if os.path.exists(outdir):
            if not os.path.isdir(outdir) or os.listdir(outdir):
                raise GenError("outdir must not exist or be an empty directory")
        else:
            os.makedirs(outdir)
        for rel in sorted(files):
            data, mode = files[rel]
            p = os.path.join(outdir, rel)
            os.makedirs(os.path.dirname(p), exist_ok=True)
            with open(p, "wb") as f:
                f.write(data)
            os.chmod(p, mode)


# --------------------------------------------------------------------------------- reference helpers


def py_eval(files: dict, module: str, exprs: list) -> list:
    """Evaluates expressions against trusted reference code through the runner's own evaluation path
    (s1check.py --child-py-batch, PYTHONHASHSEED=0), so expected reprs match what the check will see.
    files: {path: source} written to a temporary directory; returns [{"ok", "repr"|"exc"}, ...]."""
    import tempfile
    with tempfile.TemporaryDirectory(prefix="s1ref-") as d:
        for p, src in files.items():
            fp = os.path.join(d, p)
            os.makedirs(os.path.dirname(fp), exist_ok=True)
            with open(fp, "wb") as f:
                f.write(_b(src))
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "LC_ALL": "C", "PYTHONHASHSEED": "0",
               "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1", "PYTHONIOENCODING": "utf-8"}
        job = json.dumps({"ws": d, "import": module, "exprs": list(exprs)}).encode("utf-8")
        p = subprocess.run([sys.executable, "-S", "-s", "-B", os.path.join(RUNNER_DIR, "s1check.py"), "--child-py-batch"],
                           input=job, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=d, env=env, timeout=120)
        if p.returncode != 0:
            raise GenError("reference evaluation failed: %s" % p.stderr.decode("utf-8", "replace")[-2000:])
        return json.loads(p.stdout.decode("utf-8").strip().splitlines()[-1])


def py_literal(v) -> str:
    """repr() of a value that ast.literal_eval reads back exactly (checked)."""
    import ast
    r = repr(v)
    if ast.literal_eval(r) != v:
        raise GenError("value does not round-trip through repr: %r" % (v,))
    return r


def unittest_source(module: str, cases: list, header: str = "") -> str:
    """Visible unittest file for Python tasks. cases: [(name, expr, expected_repr)]; expr is evaluated in
    the module's namespace via `from <module> import *` semantics, so use names the module exports."""
    lines = [header] if header else []
    lines += ["import unittest", "", "from %s import *  # noqa: F401,F403" % module, "", "",
              "class VisibleTests(unittest.TestCase):"]
    for name, expr, want in cases:
        lines += ["    def test_%s(self):" % name, "        self.assertEqual(%s, %s)" % (expr, want), ""]
    lines += ["", "if __name__ == \"__main__\":", "    unittest.main()", ""]
    return "\n".join(lines)


def rust_str(s: str) -> str:
    """A Rust string literal for s."""
    out = []
    for ch in s:
        if ch == "\\":
            out.append("\\\\")
        elif ch == "\"":
            out.append("\\\"")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif 0x20 <= ord(ch) < 0x7f:
            out.append(ch)
        else:
            out.append("\\u{%x}" % ord(ch))
    return "\"" + "".join(out) + "\""


def rust_tests_source(uses: str, cases: list) -> str:
    """An integration-test file. cases: [(name, rust_expr, rust_expected)] -> one #[test] per case."""
    lines = [uses, ""]
    for name, expr, want in cases:
        lines += ["#[test]", "fn %s() {" % name, "    assert_eq!(%s, %s);" % (expr, want), "}", ""]
    return "\n".join(lines)


def tree_digest(root: str) -> str:
    """Reference tree digest (the Root's checker-integrity digest, m7-design §3.1.3 item 3):
    sha256 over sorted lines `f <x|-> <sha256> <rel>` / `d <rel>` / `l <target> <rel>`."""
    import stat as st_
    entries = []
    for dirpath, dirnames, filenames in os.walk(root):
        for name in dirnames + filenames:
            p = os.path.join(dirpath, name)
            rel = os.path.relpath(p, root).replace(os.sep, "/")
            s = os.lstat(p)
            if st_.S_ISLNK(s.st_mode):
                entries.append("l %s %s" % (os.readlink(p), rel))
            elif st_.S_ISDIR(s.st_mode):
                entries.append("d %s" % rel)
            elif st_.S_ISREG(s.st_mode):
                with open(p, "rb") as f:
                    h = hashlib.sha256(f.read()).hexdigest()
                entries.append("f %s %s %s" % ("x" if s.st_mode & 0o111 else "-", h, rel))
            else:
                entries.append("o %s" % rel)
    h = hashlib.sha256()
    for e in sorted(entries, key=lambda e: e.rsplit(" ", 1)[-1]):
        h.update(e.encode("utf-8") + b"\n")
    return h.hexdigest()


# -------------------------------------------------------------------------------------------- main


def _env_int(name: str):
    v = os.environ.get(name)
    if v is None or v == "":
        return None
    if not re.match(r"^[1-5]$", v):
        raise GenError("%s must be 1..5" % name)
    return int(v)


def generate(build, template_dir: str, seed_hex: str, outdir: str, guid: str = DEV_GUID,
             difficulty: int | None = None, dev: bool = False) -> None:
    ctx = Ctx(template_dir, seed_hex, guid, difficulty, dev)
    inst = build(ctx)
    if not isinstance(inst, Instance):
        raise GenError("build() must return an Instance")
    inst.write(outdir)


def main(build) -> None:
    """`gen/run <seed> <outdir>`; inputs from the environment (all optional):
    INIT_S1_GUID (32 hex, the install's canary GUID), INIT_S1_DIFFICULTY (1..5), and, for the maintainer's
    validator only, INIT_S1_DEV=1 (also write dev/ fixtures)."""
    script = os.path.abspath(sys.modules["__main__"].__file__)
    template_dir = os.path.dirname(os.path.dirname(script))
    if len(sys.argv) != 3:
        sys.stderr.write("usage: gen/run <seed: 64 hex> <outdir>\n")
        sys.exit(64)
    try:
        generate(build, template_dir, sys.argv[1], sys.argv[2], guid=os.environ.get("INIT_S1_GUID") or DEV_GUID,
                 difficulty=_env_int("INIT_S1_DIFFICULTY"), dev=os.environ.get("INIT_S1_DEV") == "1")
    except GenError as e:
        sys.stderr.write("gen: %s\n" % e)
        sys.exit(1)
