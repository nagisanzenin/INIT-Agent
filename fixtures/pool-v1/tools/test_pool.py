"""Tests of the visible pool v1 (POOL-LITE). Python >= 3.9, stdlib unittest. From the repo root:

    /usr/bin/python3 -I -m unittest discover -s fixtures/pool-v1/tools -p 'test_*.py'          # fast tests
    POOL_SLOW=1 INIT_SLOT_POOL=light bash ci/slot.sh nice -n 10 \\
        /usr/bin/python3 -I -m unittest discover -s fixtures/pool-v1/tools -p 'test_*.py'      # + import, parity

Fast: the manifest is current, the composition and sources follow PROOF-LEAN D-L4 and M7-P08, every task builds
deterministically, poolcheck reads tampering and malformed specs as the s1 runner does.
Slow (POOL_SLOW=1; runs cargo):
  - POOL-IMPORT: the full validation (gate.md §3.7.1 import rules x3, plus the s1 validator's no-op and
    empty-workspace rules) and the import into a test home under target/test-homes/pool-lite/: frozen copies in
    vault/gate/pool/<id>/ with `accepted`, a read-only mirror in kernel/pool/<id>/ without solution.tar, an
    idempotent re-import, and a refusal for the owner's live homes.
  - POOL-PARITY: one task per step kind (py_unittest, py_cases, cargo_test, sh) scored by poolcheck/1 and by the
    sealed pack's s1check/1 on the same check data, for the solution, the no-op, a partial solution and a test
    edit: identical scores, tamper flags and exit codes (m7-design §3.2.2: the screen's estimate transfers to S
    only if both runners score alike). The s1 runner is copied into a scratch dir only, never into fixtures/.
"""
from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pool  # noqa: E402

REPO = pool.REPO
SLOW = os.environ.get("POOL_SLOW") == "1"
TEST_HOMES = os.path.join(REPO, "target", "test-homes", "pool-lite")
S1_RUNNER = os.path.join(REPO, "root", "sealed", "packs", "s1", "runner")
PARITY_TASKS = {"py_unittest": "pool-e-py-rectangles", "py_cases": "pool-c-squeeze-ws",
                "cargo_test": "pool-e-rs-yacht", "sh": "pool-t-wordfreq"}


def scratch(tag: str) -> str:
    os.makedirs(TEST_HOMES, exist_ok=True)
    return tempfile.mkdtemp(prefix=tag + "-", dir=TEST_HOMES)


def read_bytes(p: str) -> bytes:
    with open(p, "rb") as f:
        return f.read()


def rmtree(p: str) -> None:
    pool.make_writable(p)
    shutil.rmtree(p, ignore_errors=True)


class Fast(unittest.TestCase):
    def test_manifest_current(self):
        with open(os.path.join(pool.POOL_DIR, "pool.json"), "rb") as f:
            self.assertEqual(f.read(), pool.dumps(pool.manifest()), "run: pool.py manifest")

    def test_composition_and_sources(self):
        ids = pool.task_ids()
        self.assertEqual(len(ids), 20)
        fams = {}
        for tid in ids:
            t = pool.load_src(tid)
            fams[t["family"]] = fams.get(t["family"], 0) + 1
            self.assertEqual(pool.validate_source(t), [], tid)
        self.assertEqual(fams, {"e": 8, "c": 5, "t": 7})
        e = [pool.load_src(t)["source"] for t in ids if t.startswith("pool-e-")]
        self.assertEqual(sorted(s["repo"] for s in e).count("exercism/python"), 4)
        self.assertTrue(all("n" <= s["slug"][0] <= "z" and s["slug"] not in pool.POLYGLOT for s in e))
        c = [pool.load_src(t)["source"]["sample"] for t in ids if t.startswith("pool-c-")]
        self.assertFalse(set(c) & pool.CRUX_SEALED)

    def test_licences_and_notices(self):
        for name in ("Exercism-MIT.txt", "CRUXEval-MIT.txt"):
            with open(os.path.join(pool.POOL_DIR, "LICENSES", name)) as f:
                self.assertTrue(f.read().startswith("MIT License"))
        for tid in pool.task_ids():
            derived = pool.load_src(tid)["source"]["kind"] != "fresh"
            p = os.path.join(pool.TASKS_DIR, tid, "NOTICE")
            self.assertEqual(os.path.isfile(p), derived, tid)
            if derived:
                with open(p) as f:
                    self.assertIn("Permission is hereby granted", f.read())

    def test_build_deterministic_and_v3(self):
        d = scratch("build")
        try:
            a, b = os.path.join(d, "a"), os.path.join(d, "b")
            pool.build_all(a)
            pool.build_all(b)
            for tid in pool.task_ids():
                self.assertEqual(pool.same_tree(os.path.join(a, tid), os.path.join(b, tid)), [], tid)
                with open(os.path.join(a, tid, "task.json"), "rb") as f:
                    task = json.loads(f.read().decode("utf-8"))
                self.assertEqual(pool.validate_task_json(task), [], tid)
                names = set(os.listdir(os.path.join(a, tid)))
                self.assertLessEqual({"task.json", "workspace.tar", "solution.tar", "check"}, names)
                cd = os.path.join(a, tid, "check")
                self.assertTrue(os.stat(os.path.join(cd, "run")).st_mode & stat.S_IXUSR)
                with open(os.path.join(cd, "poolcheck.py"), "rb") as x, \
                        open(os.path.join(pool.TOOLS_DIR, "poolcheck.py"), "rb") as y:
                    self.assertEqual(x.read(), y.read())
                # no sealed-pack file is ever shipped with the pool
                self.assertFalse(os.path.exists(os.path.join(cd, "s1check.py")))
        finally:
            rmtree(d)

    def _check(self, task_dir: str, ws_files: dict, check_json: dict | None = None) -> tuple:
        d = scratch("unit")
        try:
            out = os.path.join(d, "out")
            shutil.copytree(os.path.join(task_dir, "check"), os.path.join(out, "check"))
            if check_json is not None:
                with open(os.path.join(out, "check", "check.json"), "w") as f:
                    json.dump(check_json, f)
            ws = os.path.join(out, "ws")
            os.makedirs(ws)
            for rel, data in ws_files.items():
                p = os.path.join(ws, rel)
                os.makedirs(os.path.dirname(p), exist_ok=True)
                with open(p, "w") as f:
                    f.write(data)
            os.makedirs(os.path.join(out, "tmp"))
            p = subprocess.run([os.path.join(out, "check", "run")], cwd=ws, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, timeout=60,
                               env={"PATH": "/usr/bin:/bin", "HOME": out, "TMPDIR": os.path.join(out, "tmp"),
                                    "INIT_CHECK_DIR": os.path.join(out, "check")})
            return p.returncode, p.stdout.decode().strip().splitlines()[-1]
        finally:
            rmtree(d)

    def test_poolcheck_tamper_and_spec_errors(self):
        d = scratch("tamper")
        try:
            td = pool.build_task("pool-c-rotate-at", d)
            ws = os.path.join(d, "ws")
            pool.extract_tar(read_bytes(os.path.join(td, "solution.tar")), ws)
            files = {}
            for dp, _, fn in os.walk(ws):
                for n in fn:
                    files[os.path.relpath(os.path.join(dp, n), ws)] = read_bytes(os.path.join(dp, n)).decode()
            self.assertEqual(self._check(td, files), (0, '{"score_permille":1000}'))
            edited = dict(files)
            edited["tests/test_rotor.py"] += "\n# weaker\n"
            self.assertEqual(self._check(td, edited), (3, '{"score_permille":0,"tamper":true}'))
            gone = {k: v for k, v in files.items() if k != "tests/test_rotor.py"}
            self.assertEqual(self._check(td, gone), (3, '{"score_permille":0,"tamper":true}'))
            bad = {"v": 1, "runner": "s1check/1", "scoring": "fraction", "steps": []}
            self.assertEqual(self._check(td, files, bad), (2, '{"score_permille":0}'))
        finally:
            rmtree(d)


@unittest.skipUnless(SLOW, "POOL_SLOW=1")
class Slow(unittest.TestCase):
    def test_import_into_test_home(self):
        home = scratch("home")
        try:
            for sub in ("kernel", "vault", "state"):
                os.makedirs(os.path.join(home, sub))
            res = pool.do_import(home, pool.default_toolchain(), None, False, 3)
            self.assertEqual(len(res["added"]), 20)
            for r in res["reports"]:
                self.assertTrue(r["ok"], r)
                self.assertTrue(all(s <= 500 for s in r["initial"]), r)
                self.assertEqual(r["solution"], [1000, 1000, 1000], r)
                self.assertLessEqual(r["max_wall_ms"], 10_000, r)
            for tid in pool.task_ids():
                v = os.path.join(home, "vault", "gate", "pool", tid)
                with open(os.path.join(v, "task.json"), "rb") as f:
                    task = json.loads(f.read().decode("utf-8"))
                self.assertEqual(task["accepted"]["by"], "maintainer")
                self.assertTrue(os.path.isfile(os.path.join(v, "solution.tar")))
                m = os.path.join(home, "kernel", "pool", tid)
                self.assertFalse(os.path.exists(os.path.join(m, "solution.tar")))
                self.assertTrue(os.path.isfile(os.path.join(m, "workspace.tar")))
                self.assertTrue(os.path.isfile(os.path.join(m, "check", "check.json")))
                self.assertEqual(os.stat(os.path.join(m, "task.json")).st_mode & 0o222, 0)
                self.assertEqual(os.stat(m).st_mode & 0o222, 0)
            self.assertTrue(os.path.isfile(os.path.join(home, "kernel", "pool", "pool.json")))
            self.assertTrue(os.path.isfile(os.path.join(home, "kernel", "pool", "LICENSES", "Exercism-MIT.txt")))
            with open(os.path.join(home, "vault", "gate", "pool-import.jsonl")) as f:
                self.assertEqual(json.loads(f.readline())["kind"], "pool_import")
            again = pool.do_import(home, pool.default_toolchain(), None, False, 1)
            self.assertEqual((len(again["added"]), len(again["unchanged"])), (0, 20))
            for live in ("~/.init", "~/.init-root"):
                with self.assertRaises(pool.PoolError):
                    pool.guard_home(os.path.expanduser(live))
        finally:
            rmtree(home)

    def test_runner_parity_with_s1check(self):
        if not os.path.isfile(os.path.join(S1_RUNNER, "s1check.py")):
            self.skipTest("sealed pack runner not in this checkout")
        d = scratch("parity")
        try:
            tc = pool.default_toolchain()
            for kind, tid in sorted(PARITY_TASKS.items()):
                td = pool.build_task(tid, os.path.join(d, "built"))
                # the same task with the s1 runner: check.json identical except the runner id
                s1 = os.path.join(d, "s1", tid)
                shutil.copytree(td, s1)
                cj = os.path.join(s1, "check", "check.json")
                with open(cj) as f:
                    spec = json.load(f)
                self.assertEqual([s["kind"] for s in spec["steps"]], [kind])
                spec["runner"] = "s1check/1"
                with open(cj, "w") as f:
                    json.dump(spec, f, sort_keys=True, indent=1)
                os.remove(os.path.join(s1, "check", "poolcheck.py"))
                shutil.copyfile(os.path.join(S1_RUNNER, "check"), os.path.join(s1, "check", "run"))
                shutil.copyfile(os.path.join(S1_RUNNER, "s1check.py"), os.path.join(s1, "check", "s1check.py"))
                os.chmod(os.path.join(s1, "check", "run"), 0o755)
                ws_tar = read_bytes(os.path.join(td, "workspace.tar"))
                sol_tar = read_bytes(os.path.join(td, "solution.tar"))
                part = pool.partial_tar(tid, ws_tar, d)
                self.assertIsNotNone(part, tid)
                tws = os.path.join(d, "tamper-" + tid)
                pool.extract_tar(sol_tar, tws)
                prot = json.loads(read_bytes(os.path.join(td, "check", "protected.json")))
                with open(os.path.join(tws, prot[0]["path"]), "ab") as f:
                    f.write(b"\n")
                variants = {"solution": sol_tar, "noop": ws_tar, "partial": part, "test_edit": pool.make_tar(tws)}
                for vname, tar in sorted(variants.items()):
                    a = pool.run_check(td, tar, tc, None, d)
                    b = pool.run_check(s1, tar, tc, None, d)
                    # identical score, tamper flag, exit code and LAST stdout line (the kernel's check_score
                    # reads the last line; GATE-V2a)
                    self.assertEqual((a[:3], a[5]), (b[:3], b[5]), "%s %s: poolcheck %s vs s1check %s\n%s\n%s" %
                                     (tid, vname, (a[:3], a[5]), (b[:3], b[5]), a[4], b[4]))
                    if vname == "solution":
                        self.assertEqual(a[:3], (1000, False, 0))
                    if vname == "partial":
                        self.assertTrue(0 < a[0] < 1000, a[:3])
                    if vname == "test_edit":
                        self.assertEqual(a[:3], (0, True, 3))
                        self.assertEqual(a[5], '{"score_permille":0,"tamper":true}')
        finally:
            rmtree(d)


if __name__ == "__main__":
    unittest.main()
