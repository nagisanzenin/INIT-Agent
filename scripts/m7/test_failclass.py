#!/usr/bin/env python3
"""Tests for scripts/m7/failclass.py (m7 3.7 step 2). Stdlib only, Python 3.9.

    /usr/bin/python3 -I -m unittest discover -s scripts/m7 -p 'test_*.py'     (from the repo root)
    /usr/bin/python3 -I scripts/m7/test_failclass.py

Scratch space is target/test-homes/pool-fc/ (removed by each test that makes some).
"""
from __future__ import annotations

import builtins
import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import failclass  # noqa: E402

SCRIPT = os.path.join(HERE, "failclass.py")
TESTDATA = os.path.join(HERE, "testdata")
SREAL_MANIFEST = os.path.join(TESTDATA, "sreal.json")
SYN = os.path.join(TESTDATA, "synthetic")
SYN_MANIFEST = os.path.join(SYN, "manifest.json")
SYN_HOME = os.path.join(SYN, "home")
REAL = os.path.join(ROOT, "build", "real-artifacts", "real")
SCRATCH = os.path.join(ROOT, "target", "test-homes", "pool-fc")
SENTINEL = "HOLDOUT-SENTINEL-DO-NOT-READ"

CLASSES = ("stopped_tests_failing", "path_confusion", "cap_hit_exploring", "edit_tool_mismatch", "other")


def run_cli(*args):
    """Run the script in a fresh interpreter (slow to start here: use sparingly). -> (exit code, stdout, stderr)."""
    p = subprocess.run([sys.executable, "-I", SCRIPT] + list(args), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                       universal_newlines=True, cwd=ROOT)
    return p.returncode, p.stdout, p.stderr


def run_main(*args):
    """Run failclass.main in this process. -> (exit code, stdout, stderr)."""
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = failclass.main(list(args))
    return code, out.getvalue(), err.getvalue()


def report(*args, **kw):
    runner = run_cli if kw.get("cli") else run_main
    code, out, err = runner(*(list(args) + ["--json"]))
    if code != 0:
        raise AssertionError("exit %d: %s" % (code, err))
    return json.loads(out)


def by_run(rep):
    return dict((r["run"], r) for r in rep["runs"])


def sigs(run):
    return sorted(set(s["signal"] for s in run["signals"]))


def walk(o):
    yield o
    if isinstance(o, dict):
        for v in o.values():
            for x in walk(v):
                yield x
    elif isinstance(o, list):
        for v in o:
            for x in walk(v):
                yield x


class Scratch(object):
    """A temp dir under target/test-homes/pool-fc/, removed on exit."""

    def __enter__(self):
        os.makedirs(SCRATCH, exist_ok=True)
        self.path = tempfile.mkdtemp(prefix="t-", dir=SCRATCH)
        return self.path

    def __exit__(self, *exc):
        shutil.rmtree(self.path, ignore_errors=True)
        try:
            os.rmdir(SCRATCH)  # only if empty
        except OSError:
            pass


@unittest.skipUnless(os.path.isdir(REAL), "build/real-artifacts/real is not present")
class SRealTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.rep = report("--manifest", SREAL_MANIFEST, cli=True)
        cls.runs = by_run(cls.rep)

    def test_a1_expected_failure_is_not_finish_tests_failing(self):
        r = self.runs["sreal/a1/20261020-063736-939a9b"]
        s = sigs(r)
        self.assertIn("path.home_relative", s)
        self.assertIn("bash.exit:cd", s)
        self.assertIn("task.fail", s)
        # the failing `python3 -m unittest` was fixed by an edit and passed: ordinary test-driven work
        self.assertNotIn("finish.tests_failing", s)
        self.assertNotIn("bash.exit:python3", s)
        self.assertNotIn("finish.untested", s)
        hr = [x for x in r["signals"] if x["signal"] == "path.home_relative"][0]
        self.assertEqual(hr["ref"], "event:a78baa4a82213c46")
        self.assertIn("work/a1-calc", hr["detail"])
        self.assertEqual((r["class"], r["reason"]), ("other", "other:no_signal"))

    def test_a2_path_errors_and_cap(self):
        r = self.runs["sreal/a2/20261020-073230-dd1b56"]
        s = sigs(r)
        for want in ("path.home_relative", "path.doubled_prefix", "cap.hit:cost", "tool.error:read:not_found",
                     "tool.error:ext:ext_failed", "task.fail"):
            self.assertIn(want, s)
        dbl = [x for x in r["signals"] if x["signal"] == "path.doubled_prefix"][0]
        self.assertIn("/work/work/a2/versions.txt", dbl["detail"])
        self.assertEqual(dbl["ref"], "event:c497bb8961a8f579")
        self.assertEqual(r["class"], "path_confusion")  # cap hit, but not while exploring: it had written and built

    def test_e2e2_doubled_prefix_after_a_successful_write(self):
        r = self.runs["sreal/e2e2/20261020-064859-a31ee0"]
        self.assertIn("path.doubled_prefix", sigs(r))
        self.assertEqual(r["class"], "path_confusion")
        self.assertEqual(r["reason"], "path.fatal:path.doubled_prefix")

    def test_h5_a3_guessed_absolute_and_initctl_refusal(self):
        r = self.runs["sreal/h5-a3/20261020-093001-1599d5"]
        s = sigs(r)
        self.assertIn("path.guessed_absolute", s)
        self.assertIn("initctl.refused:trailer.subject", s)
        self.assertIn("tool.error:edit:not_found", s)
        ga = [x for x in r["signals"] if x["signal"] == "path.guessed_absolute"][0]
        self.assertIn("/Users/init/work/wt-doc/src/loop/mod.rs", ga["detail"])
        # the guess was repaired by a later edit on the real path, and not_found is not an edit-tool mismatch
        self.assertEqual((r["class"], r["reason"]), ("other", "other:finish.untested"))

    def test_totals_and_candidate(self):
        self.assertEqual(self.rep["totals"], {"stopped_tests_failing": 0, "path_confusion": 2,
                                              "cap_hit_exploring": 0, "edit_tool_mismatch": 0, "other": 2})
        self.assertEqual(self.rep["share_permille"]["path_confusion"], 500)
        self.assertEqual(self.rep["largest"], "path_confusion")
        self.assertEqual(self.rep["candidate"], "workspace-map")


class SyntheticTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.rep = report("--manifest", SYN_MANIFEST)
        cls.runs = by_run(cls.rep)

    def cls_of(self, name):
        return self.runs["syn/" + name]["class"]

    def test_one_per_class(self):
        self.assertEqual(self.cls_of("tests_failing"), "stopped_tests_failing")
        self.assertEqual(self.cls_of("tests_failing_piped"), "stopped_tests_failing")
        self.assertEqual(self.cls_of("path_confusion"), "path_confusion")
        self.assertEqual(self.cls_of("path_guessed_fatal"), "path_confusion")
        self.assertEqual(self.cls_of("cap_hit_exploring"), "cap_hit_exploring")
        self.assertEqual(self.cls_of("edit_tool_mismatch"), "edit_tool_mismatch")
        self.assertEqual(self.cls_of("other_untested"), "other")
        self.assertEqual(self.cls_of("loops_timeout"), "other")
        self.assertEqual(self.cls_of("cap_hit_editing"), "other")

    def test_reasons(self):
        r = self.runs
        self.assertEqual(r["syn/other_untested"]["reason"], "other:finish.untested")
        self.assertEqual(r["syn/loops_timeout"]["reason"], "other:timeout")
        self.assertEqual(r["syn/cap_hit_editing"]["reason"], "other:cap_hit_while_editing")
        self.assertEqual(r["syn/cap_hit_exploring"]["reason"], "cap.hit:turns;no_file_change")
        self.assertEqual(r["syn/edit_tool_mismatch"]["reason"], "edit.fatal:tool.error:edit:no_match")
        self.assertEqual(r["syn/path_guessed_fatal"]["reason"], "path.fatal:path.guessed_absolute")

    def test_signals(self):
        r = self.runs
        self.assertEqual(sigs(r["syn/tests_failing"]), ["bash.exit:pytest", "finish.tests_failing", "task.fail"])
        self.assertIn("finish.tests_failing", sigs(r["syn/tests_failing_piped"]))
        self.assertNotIn("bash.exit:python3", sigs(r["syn/tests_failing_piped"]))  # exit 0 through `| tail`
        for want in ("path.home_relative", "path.doubled_prefix", "tool.error:read:not_found", "bash.exit:other"):
            self.assertIn(want, sigs(r["syn/path_confusion"]))
        self.assertEqual(sigs(r["syn/cap_hit_exploring"]), ["cap.hit:turns", "task.fail"])  # from the error event
        self.assertEqual(sigs(r["syn/cap_hit_editing"]), ["cap.hit:tokens_out", "task.fail"])
        self.assertEqual(sigs(r["syn/edit_tool_mismatch"]), ["task.fail", "tool.error:edit:no_match"])
        self.assertEqual(sigs(r["syn/other_untested"]), ["finish.untested", "task.fail"])
        for want in ("bash.timeout", "loop.repeat_read", "loop.repeat_cmd", "retry.same_call"):
            self.assertIn(want, sigs(r["syn/loops_timeout"]))

    def test_expected_failure_is_not_friction(self):
        failing = self.runs["syn/expected_failure_failing"]
        self.assertEqual(sigs(failing), ["task.fail"])
        self.assertEqual(failing["class"], "other")
        self.assertEqual(failing["reason"], "other:no_signal")
        passing = self.runs["syn/expected_failure_pass"]
        self.assertEqual(passing["class"], "pass")
        self.assertEqual(passing["signals"], [])

    def test_unscored_holdout_and_missing_log(self):
        self.assertEqual(self.runs["syn/unscored"]["class"], "unscored")
        self.assertNotIn("syn/holdout_never_read", self.runs)
        self.assertEqual(self.rep["skipped"], {"non_pool": 1, "no_log": 1})
        self.assertNotIn("syn/no_log", self.runs)
        self.assertEqual(self.rep["counts"], {"deduped": 0, "failing": 10, "pass": 1, "unscored": 1})

    def test_totals_shares_and_candidate(self):
        self.assertEqual(self.rep["totals"], {"stopped_tests_failing": 2, "path_confusion": 2, "cap_hit_exploring": 1,
                                              "edit_tool_mismatch": 1, "other": 4})
        self.assertEqual(self.rep["share_permille"], {"stopped_tests_failing": 200, "path_confusion": 200,
                                                      "cap_hit_exploring": 100, "edit_tool_mismatch": 100,
                                                      "other": 400})
        self.assertEqual(self.rep["largest"], "other")
        self.assertIsNone(self.rep["candidate"])

    def test_report_shape(self):
        self.assertEqual(self.rep["v"], 1)
        self.assertEqual(sorted(self.rep), ["candidate", "counts", "largest", "runs", "share_permille", "skipped",
                                            "totals", "v"])
        names = [r["run"] for r in self.rep["runs"]]
        self.assertEqual(names, sorted(names))
        for r in self.rep["runs"]:
            self.assertEqual(sorted(r), ["class", "labels", "outcome", "reason", "run", "score_permille", "set",
                                         "signals", "task"])
            self.assertIn(r["class"], CLASSES + ("pass", "unscored"))
            keys = [(s["signal"], s["ref"], s["detail"]) for s in r["signals"]]
            self.assertEqual(keys, sorted(keys))
            for s in r["signals"]:
                self.assertEqual(sorted(s), ["detail", "ref", "signal"])
                self.assertLessEqual(len(s["detail"].encode("utf-8")), 200)
                self.assertRegex(s["ref"], r"^(event:[0-9A-Za-z]+|run:\S+|trace:\S+)$")


class HomeTests(unittest.TestCase):
    def test_pool_layout(self):
        rep = report("--home", SYN_HOME)
        runs = by_run(rep)
        self.assertEqual(rep["skipped"], {"non_pool": 4, "no_log": 1})  # holdout x2, canary, capability; c-t7 has no log
        self.assertEqual(sorted(runs), ["j-000001/c-t1-r1", "j-000001/c-t2-r1", "j-000001/c-t3-r1",
                                        "j-000001/p-t4-r1", "j-000001/p-t5-r1", "j-000001/p-t6-r1",
                                        "j-000002/p-t8-r1"])
        want = {"j-000001/c-t1-r1": "stopped_tests_failing", "j-000001/c-t2-r1": "cap_hit_exploring",
                "j-000001/c-t3-r1": "path_confusion", "j-000001/p-t4-r1": "edit_tool_mismatch",
                "j-000001/p-t5-r1": "other", "j-000001/p-t6-r1": "pass", "j-000002/p-t8-r1": "pass"}
        self.assertEqual(dict((k, v["class"]) for k, v in runs.items()), want)
        self.assertEqual(rep["totals"], dict((c, 1) for c in CLASSES))
        self.assertEqual(rep["share_permille"], dict((c, 200) for c in CLASSES))
        self.assertEqual(rep["largest"], "stopped_tests_failing")  # a tie goes to the earlier class
        self.assertEqual(rep["candidate"], "verify-on-finish")
        self.assertEqual(runs["j-000001/c-t1-r1"]["task"], "t1")
        self.assertEqual(runs["j-000001/c-t1-r1"]["labels"], ["interactive"])
        # task.fail refs the RunRecord, the 429 cap refusal refs the agent-view request record
        tf = [s for s in runs["j-000001/c-t1-r1"]["signals"] if s["signal"] == "task.fail"][0]
        self.assertEqual(tf["ref"], "run:j-000001/c-t1-r1")
        cap = [s for s in runs["j-000001/c-t2-r1"]["signals"] if s["signal"].startswith("cap.hit:")]
        # the session's own error event and the 429 request record are two refs for the same cap
        self.assertEqual(sorted(s["signal"] for s in cap), ["cap.hit:turns", "cap.hit:turns"])
        self.assertEqual(sorted(s["ref"].split(":")[0] for s in cap), ["event", "trace"])
        self.assertIn("trace:01a1580000000003", [s["ref"] for s in cap])

    def test_arm_filter(self):
        rep = report("--home", SYN_HOME, "--arm", "p")
        self.assertEqual(sorted(by_run(rep)), ["j-000001/p-t4-r1", "j-000001/p-t5-r1", "j-000001/p-t6-r1",
                                              "j-000002/p-t8-r1"])
        self.assertEqual(rep["totals"]["edit_tool_mismatch"], 1)
        self.assertEqual(rep["totals"]["other"], 1)

    def test_holdout_canary_and_s_runs_are_never_read(self):
        opened = []
        parsed = []
        real_open = builtins.open
        real_loads = json.loads

        def spy_open(file, *a, **kw):
            opened.append(str(file))
            return real_open(file, *a, **kw)

        def spy_loads(s, *a, **kw):
            parsed.append(s if isinstance(s, str) else repr(s))
            return real_loads(s, *a, **kw)

        out = io.StringIO()
        with mock.patch.object(builtins, "open", spy_open), mock.patch.object(json, "loads", spy_loads), \
                contextlib.redirect_stdout(out):
            code = failclass.main(["--home", SYN_HOME, "--json"])
        self.assertEqual(code, 0)
        joined = "\n".join(opened)
        for forbidden in ("c-h1-r1", "c-k1-r1", "c-s1-r1", "c-h2-r1"):
            self.assertNotIn(forbidden + ".jsonl", joined)
        self.assertIn("runs.jsonl", joined)
        self.assertIn("c-t1-r1.jsonl", joined)
        self.assertFalse([p for p in parsed if SENTINEL in p], "a holdout/canary/kernel record was parsed")
        self.assertNotIn(SENTINEL, out.getvalue())

    def test_manifest_holdout_entry_is_never_opened(self):
        opened = []
        real_open = builtins.open

        def spy_open(file, *a, **kw):
            opened.append(str(file))
            return real_open(file, *a, **kw)

        with mock.patch.object(builtins, "open", spy_open):
            code, out, _err = run_main("--manifest", SYN_MANIFEST, "--json")
        self.assertEqual(code, 0)
        self.assertFalse([p for p in opened if "never_read" in p])
        self.assertIn("tests_failing.jsonl", "\n".join(opened))

    def test_session_symlink_out_of_the_home_is_not_followed(self):
        with Scratch() as tmp:
            home = os.path.join(tmp, "home")
            shutil.copytree(SYN_HOME, home)
            sess = os.path.join(home, "vault", "gate", "jobs", "j-000001", "sessions", "c-t1-r1.jsonl")
            os.remove(sess)
            os.symlink(os.path.join(SYN, "tests_failing.jsonl"), sess)
            rep = report("--home", home)
            self.assertEqual(rep["skipped"]["no_log"], 2)
            self.assertNotIn("j-000001/c-t1-r1", by_run(rep))


class ArtifactsTests(unittest.TestCase):
    def test_s_real_style_dir_with_summaries(self):
        with Scratch() as tmp:
            sess = os.path.join(tmp, "chk", "init", "sessions")
            trace = os.path.join(tmp, "chk", "run", "traces")
            os.makedirs(sess)
            os.makedirs(trace)
            name = "20261020-120003-a00003"
            shutil.copy(os.path.join(SYN, "cap_hit_exploring.jsonl"), os.path.join(sess, name + ".jsonl"))
            shutil.copy(os.path.join(SYN, "cap_hit_exploring.jsonl"), os.path.join(sess, "20261020-120099-ffffff.jsonl"))
            with open(os.path.join(trace, "summaries.jsonl"), "w") as f:
                f.write(json.dumps({"req_id": "rq0001", "session": name, "task": "tt1", "status": 200,
                                    "outcome": "ok", "tool_calls": []}) + "\n")
                f.write(json.dumps({"req_id": "rq0002", "session": name, "task": "tt1", "status": 429,
                                    "outcome": "cap", "cap": "turns", "tool_calls": []}) + "\n")
            rep = report("--artifacts", tmp)
            self.assertEqual(rep["counts"]["deduped"], 1)  # byte-identical copy counted once
            self.assertEqual(len(rep["runs"]), 1)
            r = rep["runs"][0]
            self.assertEqual((r["run"], r["task"], r["class"], r["set"]), ("chk/" + name, "tt1", "unscored", "artifact"))
            self.assertEqual(sorted((s["signal"], s["ref"]) for s in r["signals"]),
                             sorted([("cap.hit:turns", "trace:rq0002"), ("cap.hit:turns", r["signals"][0]["ref"])]))

    @unittest.skipUnless(os.path.isdir(REAL), "build/real-artifacts/real is not present")
    def test_real_artifacts_dir(self):
        rep = report("--artifacts", REAL)
        runs = by_run(rep)
        self.assertEqual(rep["totals"], dict((c, 0) for c in CLASSES))  # nothing is scored without a manifest
        self.assertEqual(rep["counts"]["failing"], 0)
        self.assertGreater(rep["counts"]["deduped"], 0)
        self.assertGreater(rep["counts"]["unscored"], 10)
        self.assertIsNone(rep["largest"])
        self.assertIsNone(rep["candidate"])
        a2 = [v for k, v in runs.items() if k.endswith("20261020-073230-dd1b56")]
        self.assertEqual(len(a2), 1)
        self.assertIn("path.doubled_prefix", sigs(a2[0]))
        self.assertEqual(a2[0]["task"], "a669e23e93fdaa25")  # from the check's summaries.jsonl


class DeterminismAndInputTests(unittest.TestCase):
    def test_byte_identical_and_integer_only(self):
        args = ["--manifest", SYN_MANIFEST, "--home", SYN_HOME, "--json"]
        c1, o1, _ = run_cli(*args)
        c2, o2, _ = run_main(*args)[0:3]
        self.assertEqual((c1, c2), (0, 0))
        self.assertEqual(o1, o2)
        self.assertTrue(o1.endswith("\n"))
        self.assertEqual(o1, json.dumps(json.loads(o1), sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n")

        def no_float(s):
            raise AssertionError("float in output: %s" % s)

        data = json.loads(o1, parse_float=no_float, parse_constant=no_float)
        for x in walk(data):
            self.assertNotIsInstance(x, (float, bool))

    def test_order_of_inputs_does_not_matter(self):
        a = run_main("--manifest", SYN_MANIFEST, "--home", SYN_HOME, "--json")[1]
        b = run_main("--home", SYN_HOME, "--manifest", SYN_MANIFEST, "--json")[1]
        self.assertEqual(a, b)

    def test_default_output_is_a_short_table(self):
        code, out, _ = run_main("--manifest", SYN_MANIFEST)
        self.assertEqual(code, 0)
        self.assertLess(len(out.splitlines()), 30)
        self.assertIn("largest: other | candidate: None", out)
        for c in CLASSES:
            self.assertIn(c, out)

    def test_bad_input_exits_2(self):
        self.assertEqual(run_main()[0], 2)
        self.assertEqual(run_main("--home", os.path.join(SCRATCH, "does-not-exist"))[0], 2)
        self.assertEqual(run_main("--artifacts", os.path.join(SCRATCH, "does-not-exist"))[0], 2)
        self.assertEqual(run_main("--manifest", os.path.join(SCRATCH, "nope.json"))[0], 2)
        with Scratch() as tmp:
            bad = os.path.join(tmp, "bad.json")
            with open(bad, "w") as f:
                f.write("{not json")
            self.assertEqual(run_main("--manifest", bad)[0], 2)
            nolist = os.path.join(tmp, "nolist.json")
            with open(nolist, "w") as f:
                f.write('{"runs": 3}')
            self.assertEqual(run_main("--manifest", nolist)[0], 2)
            frac = os.path.join(tmp, "frac.json")
            with open(frac, "w") as f:
                f.write('[{"run": "x", "score_permille": 0.5}]')
            self.assertEqual(run_main("--manifest", frac)[0], 2)
            dup = os.path.join(tmp, "dup.json")
            with open(dup, "w") as f:
                f.write('[{"run": "x", "score_permille": 1000}, {"run": "x", "score_permille": 1000}]')
            code, _out, err = run_main("--manifest", dup)
            self.assertEqual(code, 2)
            self.assertIn("duplicate run id x", err)

    def test_cwd_and_home_from_manifest_when_the_log_has_no_boot(self):
        with Scratch() as tmp:
            log = os.path.join(tmp, "s.jsonl")
            with open(os.path.join(SYN, "path_confusion.jsonl")) as src, open(log, "w") as dst:
                dst.write("".join(l + "\n" for l in src.read().split("\n") if l and '"type":"boot"' not in l))
            home = "/Users/quanduong/Documents/INIT Agent/target/test-homes/synthetic/ih"
            man = os.path.join(tmp, "m.json")
            ent = {"run": "with-fields", "session": "s.jsonl", "outcome": "done", "score_permille": 0,
                   "cwd": home + "/work/task", "init_home": home}
            bare = {"run": "bare", "session": "s.jsonl", "outcome": "done", "score_permille": 0}
            with open(man, "w") as f:
                json.dump([ent, bare], f)
            runs = by_run(report("--manifest", man))
            self.assertEqual(runs["with-fields"]["class"], "path_confusion")
            self.assertIn("path.home_relative", sigs(runs["with-fields"]))
            self.assertEqual(sigs(runs["bare"]), ["bash.exit:other", "finish.untested", "task.fail",
                                                  "tool.error:read:not_found"])  # no cwd: no path signal
            runs2 = by_run(report("--manifest", man, "--cwd", ent["cwd"], "--init-home", home))
            self.assertEqual(sigs(runs2["bare"]), sigs(runs["with-fields"]))

    def test_sealed_material_is_refused(self):
        with Scratch() as tmp:
            sealed = os.path.join(tmp, "sealed", "packs")
            os.makedirs(sealed)
            self.assertEqual(run_main("--home", os.path.join(tmp, "sealed"))[0], 2)
            self.assertEqual(run_main("--artifacts", sealed)[0], 2)
            man = os.path.join(tmp, "m.json")
            with open(man, "w") as f:
                json.dump([{"run": "r", "session": "sealed/packs/x.jsonl", "score_permille": 0}], f)
            code, _out, err = run_main("--manifest", man)
            self.assertEqual(code, 2)
            self.assertIn("sealed", err)

    def test_corrupt_log_lines_are_skipped(self):
        with Scratch() as tmp:
            log = os.path.join(tmp, "s.jsonl")
            with open(os.path.join(SYN, "tests_failing.jsonl")) as src, open(log, "w") as dst:
                lines = src.read().split("\n")
                dst.write("\n".join(lines[:3] + ["{broken", "[1,2]", ""] + lines[3:]))
            man = os.path.join(tmp, "m.json")
            with open(man, "w") as f:
                json.dump([{"run": "r1", "session": "s.jsonl", "outcome": "done", "score_permille": 0}], f)
            rep = report("--manifest", man)
            self.assertEqual(rep["runs"][0]["class"], "stopped_tests_failing")


class UnitTests(unittest.TestCase):
    def test_clip_is_bytes_and_single_line(self):
        self.assertEqual(failclass.clip("a\n b\t c"), "a b c")
        s = failclass.clip("é" * 300)
        self.assertLessEqual(len(s.encode("utf-8")), 200)
        s.encode("utf-8")

    def test_permille_rounds_half_up(self):
        self.assertEqual(failclass.permille(1, 3), 333)
        self.assertEqual(failclass.permille(2, 3), 667)
        self.assertEqual(failclass.permille(0, 0), 0)

    def test_test_outcome(self):
        f = failclass.test_outcome
        self.assertEqual(f("Ran 3 tests in 0.0s\n\nOK\n", 0), "pass")
        self.assertEqual(f("Ran 3 tests in 0.0s\n\nFAILED (failures=1)\n", 0), "fail")
        self.assertEqual(f("1 failed, 2 passed in 0.03s", 1), "fail")
        self.assertEqual(f("3 passed in 0.02s", 0), "pass")
        self.assertEqual(f("test result: ok. 4 passed; 0 failed", 0), "pass")
        self.assertEqual(f("", None), "unknown")

    def test_bash_info(self):
        i = failclass.bash_info
        self.assertTrue(i("ls -la && cat -n a.py | head -5")["ro"])
        self.assertTrue(i("git log --oneline -3 2>&1")["ro"])
        self.assertFalse(i("echo hi > out.txt")["ro"])
        self.assertIsNotNone(i("echo hi > out.txt")["mut_pos"])
        self.assertIsNone(i("ls 2>/dev/null; echo ok 2>&1")["mut_pos"])
        self.assertIsNotNone(i("sed -i s/a/b/ f.txt")["mut_pos"])
        self.assertIsNone(i("sed -n 1,5p f.txt")["mut_pos"])
        self.assertIsNotNone(i("git commit -m x")["mut_pos"])
        self.assertIsNotNone(i("python3 - <<'PY'\nfrom pathlib import Path\nPath('a').write_text('x')\nPY")["mut_pos"])
        self.assertIsNone(i("python3 - <<'PY'\nprint(open('a').read())\nPY")["mut_pos"])
        self.assertEqual(i("cd a && python3 -m unittest -q 2>&1 | tail -3")["test"]["key"], "python3 unittest")
        self.assertEqual(i("pytest -x tests/a.py")["test"]["key"], "pytest tests/a.py")
        self.assertIsNone(i("grep pytest requirements.txt")["test"])
        self.assertIsNone(i("cat test_calc.py")["test"])
        for cmd, key in (("timeout 60 pytest -q tests/", "pytest tests/"), ("(cd x && pytest -q)", "pytest"),
                         ("PYTHONPATH=. python3 -m pytest -x", "python3 pytest"), ("make test", "make test"),
                         ("cargo test --quiet 2>&1 | tail -30", "cargo test"), ("bash run_tests.sh", "bash run_tests.sh"),
                         ("go test ./...", "go test ./..."), ("npm test", "npm test")):
            self.assertEqual(i(cmd)["test"]["key"], key, cmd)

    def test_path_helpers(self):
        names = failclass.doubled_names("/h/ih/work")
        self.assertTrue(failclass.has_doubled("/work/work/a2/v.txt", names))
        self.assertTrue(failclass.has_doubled("work/work/x", names))
        self.assertFalse(failclass.has_doubled("work/a/work", names))
        self.assertEqual(failclass.collapse_doubled("/h/ih/work/work/e2e/x", names), "/h/ih/work/e2e/x")
        self.assertEqual(failclass.derive_home("/h/ih/work"), "/h/ih")
        self.assertEqual(failclass.derive_home("/h/ih/work/task"), "/h/ih")
        self.assertIsNone(failclass.derive_home("/h/other"))
        self.assertEqual(failclass.user_home("/Users/a/Documents/x"), "/Users/a")
        self.assertEqual(failclass.resolve("a/../b", "/c"), "/c/b")
        self.assertIsNone(failclass.resolve("a", None))


if __name__ == "__main__":
    unittest.main()
