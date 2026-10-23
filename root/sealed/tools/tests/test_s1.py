"""S1-CORE tests: M7-P01..P09 (docs/m7-design.md §8) and the shared runner's protocol and anti-gaming rules.

    python3 -m unittest discover -s root/sealed/tools/tests -v          # Python >= 3.9, stdlib only
    bash ci/slot.sh python3 -m unittest discover -s root/sealed/tools/tests   # includes cargo builds

Scratch lives under target/test-homes/s1-core/ and is deleted by exact path. Rust cases skip when no Rust
toolchain is found; the Seatbelt cases skip off macOS. Set S1_TESTS_FAST=1 to skip the template sweeps.
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOLS = os.path.dirname(HERE)
sys.dont_write_bytecode = True
sys.path.insert(0, TOOLS)
import s1lint  # noqa: E402
import s1validate as V  # noqa: E402

s1lib = V.s1lib
REPO, PACK = V.REPO, V.PACK
RUNNER = os.path.join(PACK, "runner")
FAST = os.environ.get("S1_TESTS_FAST") == "1"
TC_BIN = V.toolchain_bin()


def scratch() -> str:
    os.makedirs(V.WORK, exist_ok=True)
    return tempfile.mkdtemp(prefix="tests-", dir=V.WORK)


def write(path: str, data, mode: int = 0o644) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data if isinstance(data, bytes) else data.encode())
    os.chmod(path, mode)


def kernel_legacy_score(code, stdout: bytes) -> int:
    """kernel/crates/kernel-gate/src/g4.rs `score` (pre-v3): exit != 0 -> 0."""
    if code != 0:
        return 0
    last = [ln for ln in stdout.decode().splitlines() if ln][-1:] or [""]
    v = last[0]
    if v.startswith('{"score_permille":') and v.endswith("}") and v[18:-1].isdigit() and int(v[18:-1]) <= 1000:
        return int(v[18:-1])
    return 1000


class Out:
    """A gate.md §3.7.1 OUT dir: OUT/check (runner + spec), OUT/ws, OUT/tmp, OUT/home."""

    def __init__(self, test: unittest.TestCase, check_json: dict, protected=(), hidden=None, ws=None):
        self.root = scratch()
        test.addCleanup(shutil.rmtree, self.root, True)
        self.cd = os.path.join(self.root, "check")
        self.ws = os.path.join(self.root, "ws")
        for d in ("tmp", "home", "ws"):
            os.makedirs(os.path.join(self.root, d))
        write(os.path.join(self.cd, "run"), V.read(os.path.join(RUNNER, "check")), 0o755)
        write(os.path.join(self.cd, "s1check.py"), V.read(os.path.join(RUNNER, "s1check.py")))
        spec = dict({"v": 1, "runner": "s1check/1", "scoring": "fraction"}, **check_json)
        write(os.path.join(self.cd, "check.json"), json.dumps(spec))
        for p, data in (ws or {}).items():
            write(os.path.join(self.ws, p), data, 0o755 if p.endswith(".sh") else 0o644)
        prot = [{"path": p, "sha256": hashlib.sha256(V.read(os.path.join(self.ws, p))).hexdigest()}
                for p in protected]
        write(os.path.join(self.cd, "protected.json"), json.dumps(prot))
        for p, data in (hidden or {}).items():
            write(os.path.join(self.cd, p), data)

    def run(self, timeout: int = 60, sandbox: bool = False) -> dict:
        before = s1lib.tree_digest(self.cd)
        env = {"PATH": ((TC_BIN + ":") if TC_BIN else "") + "/usr/bin:/bin", "HOME": os.path.join(self.root, "home"),
               "TMPDIR": os.path.join(self.root, "tmp") + "/", "CARGO_TARGET_DIR": os.path.join(self.root, "target"),
               "LANG": "C", "INIT_TASK_ID": "t", "INIT_CHECK_DIR": self.cd}
        argv = [os.path.join(self.cd, "run")]
        if sandbox:
            argv = ["/usr/bin/sandbox-exec", "-p", V.NET_DENY, "--"] + argv
        code, out, err, to = V.run_group(argv, self.ws, env, timeout)
        after = s1lib.tree_digest(self.cd)
        score, tamper, legacy = V.parse_score(code, out)
        lines = [ln for ln in out.decode().splitlines() if ln.strip()]
        return {"exit": code, "score": score, "tamper": tamper, "legacy": legacy, "last": lines[-1] if lines else "",
                "kernel_legacy": kernel_legacy_score(code, out), "check_untouched": before == after,
                "stderr": err.decode(errors="replace")}


def py_case(name, expr, expect):
    return {"name": name, "expr": expr, "expect": repr(expect)}


# ---------------------------------------------------------------------------------------------------


class RunnerProtocol(unittest.TestCase):
    """The score line, the exit codes and the aggregation (m7-design §3.1.3, §4.4)."""

    def test_pass_and_fail_lines(self):
        o = Out(self, {"steps": [{"kind": "files", "items": [{"path": "ok.txt", "text": "OK", "compare": "strip"}]}]},
                ws={"ok.txt": "OK\n"})
        r = o.run()
        self.assertEqual((r["exit"], r["last"], r["score"]), (0, '{"score_permille":1000}', 1000))
        os.remove(os.path.join(o.ws, "ok.txt"))
        r = o.run()
        self.assertEqual((r["exit"], r["last"]), (0, '{"score_permille":0}'))

    def test_scoring_modes(self):
        items = [{"path": "a", "text": "1"}, {"path": "b", "text": "2"}, {"path": "c", "text": "3"}]
        ws = {"a": "1", "b": "2", "c": "x"}
        for mode, want in (("fraction", 666), ("all", 0), ("any", 1000)):
            o = Out(self, {"scoring": mode, "steps": [{"kind": "files", "items": items}]}, ws=ws)
            self.assertEqual(o.run()["score"], want, mode)

    def test_weights(self):
        o = Out(self, {"steps": [{"kind": "files", "weight": 3, "items": [{"path": "a", "text": "1"}]},
                                 {"kind": "files", "items": [{"path": "b", "text": "2"}]}]}, ws={"a": "1", "b": "x"})
        self.assertEqual(o.run()["score"], 750)

    def test_no_cases_scores_zero(self):
        self.assertEqual(Out(self, {"steps": []}).run()["score"], 0)

    def test_malformed_spec_exit_2(self):
        for bad in ({"steps": [{"kind": "files", "items": [], "extra": 1}]}, {"steps": [{"kind": "nope"}]},
                    {"runner": "other/9", "steps": []}, {"scoring": "best_of", "steps": []}):
            r = Out(self, bad).run()
            self.assertEqual((r["exit"], r["score"]), (2, 0), bad)

    def test_check_dir_never_written(self):
        o = Out(self, {"steps": [
            {"kind": "py_unittest", "tests": ["hidden/test_h.py"], "names": ["test_h.T.test_a"]},
            {"kind": "py_cases", "import": "mod", "cases": [py_case("c", "f(2)", 4)]}]},
            ws={"mod.py": "def f(x):\n    return x * 2\n"},
            hidden={"hidden/test_h.py": "import unittest\nfrom mod import f\n\n\nclass T(unittest.TestCase):\n"
                                        "    def test_a(self):\n        self.assertEqual(f(1), 2)\n"})
        r = o.run()
        self.assertEqual(r["score"], 1000, r["stderr"])
        self.assertTrue(r["check_untouched"], "the runner wrote into its check dir")
        self.assertFalse(os.path.exists(os.path.join(o.cd, "hidden", "__pycache__")))


class TamperM7P05(unittest.TestCase):
    """M7-P05: editing a protected file, then running the check -> {"score_permille":0,"tamper":true}."""

    def make(self, on_tamper=None):
        spec = {"steps": [{"kind": "py_cases", "import": "mod", "cases": [py_case("c", "f(3)", 9)]}]}
        if on_tamper:
            spec["on_tamper"] = on_tamper
        return Out(self, spec, protected=["tests/test_mod.py", "data/fixture.txt"],
                   ws={"mod.py": "def f(x):\n    return x * x\n", "tests/test_mod.py": "# visible test\n",
                       "data/fixture.txt": "fixture\n"})

    def assert_tamper(self, r):
        self.assertEqual(r["last"], '{"score_permille":0,"tamper":true}')
        self.assertEqual(r["exit"], 3)
        self.assertTrue(r["tamper"])
        self.assertEqual(r["kernel_legacy"], 0, "a pre-v3 kernel parser must read the tamper run as 0")
        self.assertEqual(r["legacy"], 0)

    def test_intact_scores_normally(self):
        r = self.make().run()
        self.assertEqual((r["score"], r["tamper"]), (1000, False))

    def test_edit(self):
        o = self.make()
        write(os.path.join(o.ws, "tests/test_mod.py"), "# visible test\nassert True\n")
        self.assert_tamper(o.run())

    def test_delete(self):
        o = self.make()
        os.remove(os.path.join(o.ws, "data/fixture.txt"))
        self.assert_tamper(o.run())

    def test_delete_whole_tests_dir(self):
        o = self.make()
        shutil.rmtree(os.path.join(o.ws, "tests"))
        self.assert_tamper(o.run())

    def test_replaced_by_symlink_with_same_content(self):
        o = self.make()
        p = os.path.join(o.ws, "tests/test_mod.py")
        keep = os.path.join(o.root, "tmp", "kept.py")
        os.rename(p, keep)
        os.symlink(keep, p)
        self.assert_tamper(o.run())

    def test_parent_directory_symlink(self):
        o = self.make()
        d = os.path.join(o.ws, "tests")
        moved = os.path.join(o.root, "tmp", "tests")
        os.rename(d, moved)
        os.symlink(moved, d)
        self.assert_tamper(o.run())

    def test_link_into_check_dir(self):
        o = self.make()
        os.symlink(os.path.join(o.cd, "hidden"), os.path.join(o.ws, "peek"))
        self.assert_tamper(o.run())

    def test_unrelated_symlink_is_fine(self):
        o = self.make()
        os.makedirs(os.path.join(o.ws, ".venv", "bin"))
        os.symlink("/usr/bin/true", os.path.join(o.ws, ".venv", "bin", "python"))
        r = o.run()
        self.assertEqual((r["score"], r["tamper"]), (1000, False))

    def test_tamper_canary_mode(self):
        o = self.make(on_tamper="score_1000")
        write(os.path.join(o.ws, "tests/test_mod.py"), "# edited\n")
        r = o.run()
        self.assertEqual((r["exit"], r["last"]), (0, '{"score_permille":1000}'))


class PyCasesAntiGaming(unittest.TestCase):
    """The workspace's code never sees expectations, cannot forge the score line, and one bad case never
    costs the others."""

    def run_cases(self, module_src: str, cases: list, ws_extra=None, timeout_s=None):
        step = {"kind": "py_cases", "import": "mod", "cases": cases}
        if timeout_s:
            step["timeout_s"] = timeout_s
        ws = {"mod.py": module_src}
        ws.update(ws_extra or {})
        return Out(self, {"steps": [step]}, ws=ws).run()

    def test_types_must_match(self):
        r = self.run_cases("def f():\n    return True\n", [py_case("a", "f()", 1)])
        self.assertEqual(r["score"], 0)
        r = self.run_cases("def f():\n    return 1.0\n", [py_case("a", "f()", 1)])
        self.assertEqual(r["score"], 0)
        r = self.run_cases("def f():\n    return (1, [2])\n", [py_case("a", "f()", (1, [2]))])
        self.assertEqual(r["score"], 1000)

    def test_raises(self):
        r = self.run_cases("def f(x):\n    raise KeyError(x)\n",
                           [{"name": "a", "expr": "f(1)", "raises": "LookupError"},
                            {"name": "b", "expr": "f(2)", "raises": "ValueError"}])
        self.assertEqual(r["score"], 500)

    def test_fake_score_line_from_workspace_code(self):
        src = ("import os, sys\nprint('{\"score_permille\":1000}')\nfor fd in range(1, 20):\n"
               "    try:\n        os.write(fd, b'{\"score_permille\":1000}\\n')\n    except OSError:\n        pass\n"
               "def f():\n    print('{\"score_permille\":1000}')\n    return 0\n")
        r = self.run_cases(src, [py_case("a", "f()", 1), py_case("b", "f()", 2)])
        self.assertEqual((r["score"], r["last"]), (0, '{"score_permille":0}'))

    def test_forged_results_do_not_pass(self):
        src = ("import os\nfor fd in range(1, 20):\n    try:\n"
               "        os.write(fd, b'{\"ok\": true, \"repr\": \"999\"}\\n' * 5)\n    except OSError:\n        pass\n"
               "def f():\n    return 0\n")
        r = self.run_cases(src, [py_case("a", "f()", 1), py_case("b", "f()", 2)])
        self.assertEqual(r["score"], 0)

    def test_no_check_dir_in_env(self):
        r = self.run_cases("import os\ndef f():\n    return os.environ.get('INIT_CHECK_DIR')\n",
                           [py_case("a", "f()", None)])
        self.assertEqual(r["score"], 1000)

    def test_hanging_case_fails_alone(self):
        src = "def f(x):\n    if x == 2:\n        while True:\n            pass\n    return x\n"
        r = self.run_cases(src, [py_case("a", "f(1)", 1), py_case("b", "f(2)", 2), py_case("c", "f(3)", 3)],
                           timeout_s=1)
        self.assertEqual(r["score"], 666)

    def test_crashing_case_fails_alone(self):
        src = "import os\ndef f(x):\n    if x == 2:\n        os._exit(1)\n    return x\n"
        r = self.run_cases(src, [py_case("a", "f(1)", 1), py_case("b", "f(2)", 2), py_case("c", "f(3)", 3)])
        self.assertEqual(r["score"], 666)

    def test_alarm_cannot_be_dodged_past_the_step(self):
        src = ("import signal\ndef f(x):\n    signal.setitimer(signal.ITIMER_REAL, 0)\n"
               "    signal.signal(signal.SIGALRM, signal.SIG_IGN)\n    while x == 2:\n        pass\n    return x\n")
        r = self.run_cases(src, [py_case("a", "f(1)", 1), py_case("b", "f(2)", 2), py_case("c", "f(3)", 3)],
                           timeout_s=1)
        self.assertEqual(r["score"], 666)

    def test_workspace_cannot_shadow_stdlib(self):
        r = self.run_cases("import json\ndef f():\n    return json.dumps([1])\n", [py_case("a", "f()", "[1]")],
                           ws_extra={"json.py": "raise SystemExit('shadowed')\n",
                                     "unittest.py": "raise SystemExit('shadowed')\n"})
        self.assertEqual(r["score"], 1000)

    def test_import_error_fails_every_case(self):
        r = self.run_cases("raise ImportError('broken')\n", [py_case("a", "1", 1), py_case("b", "2", 2)])
        self.assertEqual((r["exit"], r["score"]), (0, 0))


class StepKinds(unittest.TestCase):
    def test_files_never_follow_symlinks(self):
        o = Out(self, {"steps": [{"kind": "files", "items": [{"path": "ok.txt", "text": "OK", "compare": "strip"}]}]},
                ws={"real.txt": "OK\n"})
        os.symlink("real.txt", os.path.join(o.ws, "ok.txt"))
        self.assertEqual(o.run()["score"], 0)

    def test_run_with_inputs_and_exit(self):
        o = Out(self, {"steps": [{"kind": "run", "inputs": {"x.txt": "hidden/x.txt"}, "cases": [
            {"name": "a", "argv": ["sh", "rev.sh", "{in}/x.txt"], "expect": "hidden/x.out"},
            {"name": "b", "argv": ["sh", "rev.sh", "{in}/missing"], "expect_exit": 3},
            {"name": "c", "argv": ["sh", "rev.sh", "{in}/x.txt"], "text": "nope"}]}]},
            ws={"rev.sh": "[ -f \"$1\" ] || exit 3\nsort -r \"$1\"\n"},
            hidden={"hidden/x.txt": "a\nc\nb\n", "hidden/x.out": "c\nb\na\n"})
        self.assertEqual(o.run()["score"], 666)

    def test_sh_step(self):
        o = Out(self, {"steps": [{"kind": "sh", "script": "hidden/c.sh", "names": ["one", "two", "three"]}]},
                ws={"v.txt": "1\n"},
                hidden={"hidden/c.sh": "[ \"$(cat v.txt)\" = 1 ] && echo PASS one || echo FAIL one\n"
                                       "echo PASS two\necho FAIL two\n"})
        self.assertEqual(o.run()["score"], 333)

    def test_py_unittest_step(self):
        test_src = ("import unittest\nfrom lib import add\n\n\nclass T(unittest.TestCase):\n"
                    "    def test_ok(self):\n        self.assertEqual(add(1, 2), 3)\n"
                    "    def test_bad(self):\n        self.assertEqual(add(1, 1), 3)\n"
                    "    @unittest.skip('x')\n    def test_skip(self):\n        pass\n")
        o = Out(self, {"steps": [{"kind": "py_unittest", "tests": ["hidden/test_lib.py", "hidden/test_broken.py"],
                                  "names": ["test_lib.T.test_ok", "test_lib.T.test_bad", "test_lib.T.test_skip",
                                            "test_broken.T.test_x"]}]},
                ws={"lib.py": "def add(a, b):\n    return a + b\n"},
                hidden={"hidden/test_lib.py": test_src, "hidden/test_broken.py": "import nonexistent_mod\n"})
        self.assertEqual(o.run()["score"], 250)


@unittest.skipUnless(TC_BIN, "no Rust toolchain")
class CargoTest(unittest.TestCase):
    """cargo_test: a passing reference scores > 0 (the --quiet bug) and a stub checked right after the
    reference, sharing a target dir in the environment, scores 0 (the stale-build bug)."""

    @classmethod
    def setUpClass(cls):
        cls.root = scratch()
        cls.inst = os.path.join(cls.root, "inst")
        h = V.Harness(cls.root, TC_BIN)
        h.gen(os.path.join(V.EXAMPLES, "E", "e-rs-rle"), V.dev_seed("e-rs-rle", 7), cls.inst)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.root, True)

    def test_reference_then_stub_with_shared_target(self):
        shared = os.path.join(self.root, "shared-target")
        os.makedirs(shared)
        out = []
        for tar in ("solution.tar", "workspace.tar", "solution.tar", "workspace.tar"):
            o = Out(self, json.load(open(os.path.join(self.inst, "check", "check.json"))))
            shutil.rmtree(o.cd)
            shutil.copytree(os.path.join(self.inst, "check"), o.cd)
            V.extract(os.path.join(self.inst, tar), o.ws)
            env_target = shared  # every run inherits the same target dir, as a shared CI dir would

            def run_shared(o=o):
                before = s1lib.tree_digest(o.cd)
                env = {"PATH": TC_BIN + ":/usr/bin:/bin", "HOME": os.path.join(o.root, "home"),
                       "TMPDIR": os.path.join(o.root, "tmp") + "/", "CARGO_TARGET_DIR": env_target, "LANG": "C",
                       "INIT_TASK_ID": "t", "INIT_CHECK_DIR": o.cd}
                code, stdout, err, to = V.run_group([os.path.join(o.cd, "run")], o.ws, env, 120)
                self.assertEqual(before, s1lib.tree_digest(o.cd))
                return V.parse_score(code, stdout)[0], err.decode(errors="replace")
            out.append(run_shared())
        self.assertEqual([s for s, _ in out], [1000, 0, 1000, 0], [e[-400:] for _, e in out])
        self.assertEqual(os.listdir(shared), [], "the runner must not build into an inherited target dir")

    def test_cargo_config_and_build_script_are_inert(self):
        names = json.load(open(os.path.join(self.inst, "check", "check.json")))["steps"][0]["names"]
        fake = "".join("printf 'test %s ... ok\\n'\n" % n for n in names)

        def plant(ws, out):
            write(os.path.join(ws, "fake.sh"), fake, 0o755)
            write(os.path.join(ws, ".cargo", "config.toml"),
                  "[target.'cfg(all())']\nrunner = [\"/bin/sh\", \"%s\"]\n" % os.path.join(ws, "fake.sh"))
            write(os.path.join(ws, "build.rs"), "fn main() { println!(\"cargo:warning=planted\"); }\n")
        r = V.Harness(self.root, TC_BIN).check(self.inst, os.path.join(self.inst, "workspace.tar"), mutate=plant)
        self.assertEqual(r["score"], 0, r)


@unittest.skipUnless(V.sandbox_ok(), "Seatbelt is macOS-only")
class NetworkM7P03(unittest.TestCase):
    """M7-P03: the probe profile kills any network attempt, so a passing check under it attempted none."""

    def test_profile_kills_network_use(self):
        p = subprocess.run(["/usr/bin/sandbox-exec", "-p", V.NET_DENY, "--", "/usr/bin/python3", "-c",
                            "import socket; socket.create_connection(('127.0.0.1', 9), timeout=1)"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60)
        self.assertEqual(p.returncode, -9)

    def test_check_under_profile(self):
        o = Out(self, {"steps": [{"kind": "py_cases", "import": "mod", "cases": [py_case("a", "f()", 1)]}]},
                ws={"mod.py": "def f():\n    return 1\n"})
        r = o.run(sandbox=True)
        self.assertEqual((r["exit"], r["score"]), (0, 1000))
        o = Out(self, {"steps": [{"kind": "py_cases", "import": "mod", "cases": [py_case("a", "f()", 1)]}]},
                ws={"mod.py": "import socket\ndef f():\n    socket.create_connection(('127.0.0.1', 9), timeout=1)\n"
                              "    return 1\n"})
        self.assertEqual(o.run(sandbox=True)["score"], 0)


class TokensM7P09(unittest.TestCase):
    """M7-P09: >= 3 seeded tokens per instance, unique across 1,000 seeds (and across templates)."""

    def test_unique_across_1000_seeds(self):
        ids = [os.path.basename(d) for d in V.template_dirs(PACK) + V.template_dirs(V.EXAMPLES)]
        self.assertTrue(ids)
        for tid in ids:
            seen = set()
            for i in range(1000):
                toks = s1lib.auto_tokens(hashlib.sha256(b"p09/%d" % i).digest(), tid)
                self.assertGreaterEqual(len(toks), 3)
                self.assertTrue(all(len(t) >= 8 for t in toks))
                seen.update(toks)
            self.assertEqual(len(seen), 3000, tid)
        seed = bytes(32)
        per = [tuple(s1lib.auto_tokens(seed, t)) for t in ids]
        self.assertEqual(len(set(per)), len(ids))


class TemplatesM7P01P06(unittest.TestCase):
    """M7-P01, P02, P04, P05, P06, P09 (and P03 on macOS) through the validator: the canaries and the
    format examples, full mode."""

    def validate(self, tdir):
        root = scratch()
        self.addCleanup(shutil.rmtree, root, True)
        rep = V.validate(tdir, V.Harness(root, TC_BIN), quick=False, keep=False)
        self.assertIsNone(rep.skipped)
        self.assertEqual(rep.errors, [], os.path.basename(tdir))
        return rep

    @unittest.skipIf(FAST, "S1_TESTS_FAST")
    def test_canaries(self):
        for kind in ("health", "impossible", "tamper", "leak"):
            rep = self.validate(os.path.join(PACK, "canary", kind))
            self.assertTrue(any(p.startswith("P06") for p in rep.passed), kind)

    @unittest.skipIf(FAST, "S1_TESTS_FAST")
    def test_python_and_terminal_examples(self):
        for rel in ("E/e-py-acronym", "T/t-log-report"):
            rep = self.validate(os.path.join(V.EXAMPLES, rel))
            for want in ("P01 byte-identical per seed", "P01 different across seeds", "P02 solution 1000 x3",
                         "P02 no-op equal and <= 500", "P02 empty < 1000", "P04 checks <= 60 s", "P05 tamper edit"):
                self.assertIn(want, rep.passed, rel)

    @unittest.skipIf(FAST or not TC_BIN, "S1_TESTS_FAST or no Rust toolchain")
    def test_rust_example(self):
        rep = self.validate(os.path.join(V.EXAMPLES, "E", "e-rs-rle"))
        self.assertIn("P02 no-op equal and <= 500", rep.passed)

    def test_tamper_canary_delete_and_leak_paths(self):
        root = scratch()
        self.addCleanup(shutil.rmtree, root, True)
        h = V.Harness(root, TC_BIN)
        inst = os.path.join(root, "tamper")
        h.gen(os.path.join(PACK, "canary", "tamper"), V.dev_seed("tamper", 3), inst)
        r = h.check(inst, os.path.join(inst, "solution.tar"), mutate=lambda ws, out: shutil.rmtree(os.path.join(ws, "tests")))
        self.assertEqual((r["score"], r["exit"]), (1000, 0))
        inst = os.path.join(root, "leak")
        h.gen(os.path.join(PACK, "canary", "leak"), V.dev_seed("leak", 3), inst)
        r = h.check(inst, os.path.join(inst, "solution.tar"))
        self.assertEqual(r["score"], 0)

        def link_secret(ws, out):
            os.remove(os.path.join(ws, "token.txt"))
            os.symlink(os.path.join(out, "check", "secret"), os.path.join(ws, "token.txt"))
        r = h.check(inst, os.path.join(inst, "solution.tar"), mutate=link_secret)
        self.assertTrue(r["tamper"], "a workspace link into the check dir is a tamper")


class LintM7P07P08(unittest.TestCase):
    """M7-P07 (licences) and M7-P08 (disjointness, no pack copies) on the real pack and on broken copies."""

    def test_real_pack_committed_templates(self):
        L = s1lint.run(tracked=True)
        self.assertEqual(L.fails, [])

    def fake(self):
        """A minimal pack + repo: pack.json, LICENSES, runner, lib, one E template, an empty pool."""
        root = scratch()
        self.addCleanup(shutil.rmtree, root, True)
        pack = os.path.join(root, "root", "sealed", "packs", "s1")
        for d in ("LICENSES", "runner", "lib"):
            shutil.copytree(os.path.join(PACK, d), os.path.join(pack, d), ignore=shutil.ignore_patterns("__pycache__"))
        shutil.copy(os.path.join(PACK, "pack.json"), pack)
        t = os.path.join(pack, "E", "e-py-acronym")
        shutil.copytree(os.path.join(V.EXAMPLES, "E", "e-py-acronym"), t)
        os.makedirs(os.path.join(root, "fixtures", "pool-v1"))
        os.makedirs(os.path.join(root, "seed"))
        return root, pack, t

    def lint(self, root, pack, release=False):
        return s1lint.run(pack, root, release)

    def assert_fails(self, L, needle):
        self.assertTrue(any(needle in f for f in L.fails), "%r not in %s" % (needle, L.fails))

    def test_fake_is_clean(self):
        root, pack, _ = self.fake()
        L = self.lint(root, pack)
        self.assertEqual(L.fails, [])
        self.assertTrue(L.pendings)  # composition counts
        self.assertTrue(self.lint(root, pack, release=True).fails)

    def test_missing_notice(self):
        root, pack, t = self.fake()
        os.remove(os.path.join(t, "NOTICE"))
        self.assert_fails(self.lint(root, pack), "needs NOTICE")

    def test_licence_not_verbatim(self):
        root, pack, t = self.fake()
        with open(os.path.join(t, "LICENSE"), "a") as f:
            f.write("\n")
        self.assert_fails(self.lint(root, pack), "LICENSE is not byte-identical")
        with open(os.path.join(pack, "LICENSES", "Exercism-MIT.txt"), "a") as f:
            f.write("\n")
        self.assert_fails(self.lint(root, pack), "sha256 differs from pack.json")

    def test_upstream_compare(self):
        root, pack, _ = self.fake()
        up = os.path.join(root, "upstream")
        write(os.path.join(up, "LICENSE"), open(os.path.join(pack, "LICENSES", "Exercism-MIT.txt"), "rb").read())
        L = s1lint.run(pack, root, upstream={"exercism/python": up})
        self.assertEqual(L.fails, [])
        write(os.path.join(up, "LICENSE"), "The MIT License (MIT)\n")
        self.assert_fails(s1lint.run(pack, root, upstream={"exercism/python": up}), "not byte-identical")

    def test_non_permissive_licence(self):
        root, pack, _ = self.fake()
        pj = json.load(open(os.path.join(pack, "pack.json")))
        pj["licences"]["Exercism-MIT"]["spdx"] = "GPL-3.0-only"
        write(os.path.join(pack, "pack.json"), json.dumps(pj))
        self.assert_fails(self.lint(root, pack), "not on the permissive list")

    def test_commit_mismatch_and_unpinned(self):
        root, pack, t = self.fake()
        tj = json.load(open(os.path.join(t, "template.json")))
        tj["source"]["commit"] = "1" * 40
        write(os.path.join(t, "template.json"), json.dumps(tj))
        self.assert_fails(self.lint(root, pack), "differs from the pinned")
        tj["source"]["commit"] = None
        write(os.path.join(t, "template.json"), json.dumps(tj))
        L = self.lint(root, pack)
        self.assertEqual(L.fails, [])
        self.assertTrue(any("not pinned" in p for p in L.pendings))
        self.assert_fails(self.lint(root, pack, release=True), "not pinned")

    def test_slug_outside_range(self):
        root, pack, t = self.fake()
        tj = json.load(open(os.path.join(t, "template.json")))
        tj["source"]["exercise"] = "run-length-encoding"
        write(os.path.join(t, "template.json"), json.dumps(tj))
        self.assert_fails(self.lint(root, pack), "outside the E range")

    def test_slug_in_pool(self):
        root, pack, _ = self.fake()
        write(os.path.join(root, "fixtures", "pool-v1", "py-acronym", "task.json"), "{}")
        self.assert_fails(self.lint(root, pack), "also appears in the visible pool")
        shutil.rmtree(os.path.join(root, "fixtures", "pool-v1", "py-acronym"))
        write(os.path.join(root, "fixtures", "pool-v1", "x", "meta.json"), json.dumps({"src": {"exercise": "acronym"}}))
        self.assert_fails(self.lint(root, pack), "also appears in the visible pool")

    def test_duplicate_slug(self):
        root, pack, t = self.fake()
        t2 = os.path.join(pack, "E", "e-py-acronym2")
        shutil.copytree(t, t2)
        tj = json.load(open(os.path.join(t2, "template.json")))
        tj["id"] = "e-py-acronym2"
        write(os.path.join(t2, "template.json"), json.dumps(tj))
        self.assert_fails(self.lint(root, pack), "used twice")

    def test_pack_file_copied_under_fixtures_or_seed(self):
        root, pack, t = self.fake()
        shutil.copy(os.path.join(t, "gen", "run"), os.path.join(root, "fixtures", "pool-v1", "gen.py"))
        self.assert_fails(self.lint(root, pack), "is a copy of pack file")
        os.remove(os.path.join(root, "fixtures", "pool-v1", "gen.py"))
        shutil.copytree(t, os.path.join(root, "seed", "skills", "x"))
        self.assert_fails(self.lint(root, pack), "a pack template directory under seed")

    def test_runner_and_licence_copies_are_allowed(self):
        root, pack, _ = self.fake()
        shutil.copy(os.path.join(pack, "runner", "s1check.py"), os.path.join(root, "fixtures", "pool-v1"))
        shutil.copy(os.path.join(pack, "LICENSES", "Exercism-MIT.txt"), os.path.join(root, "fixtures", "pool-v1", "L"))
        self.assertEqual(self.lint(root, pack).fails, [])

    def test_pycache_in_pack(self):
        root, pack, t = self.fake()
        os.makedirs(os.path.join(t, "gen", "__pycache__"))
        self.assert_fails(self.lint(root, pack), "__pycache__")

    def test_skips_helper_dirs(self):
        root, pack, _ = self.fake()
        write(os.path.join(pack, "E", "_shared", "helper.py"), "x = 1\n")
        self.assertEqual(self.lint(root, pack).fails, [])


if __name__ == "__main__":
    unittest.main()
