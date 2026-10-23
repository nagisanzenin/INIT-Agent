"""epy: shared helpers for the S1-E-PY templates (pack s1, family E, Python). Python >= 3.9, stdlib only.

A template's gen/run puts ../../lib (s1lib) and ../_shared_py on sys.path, then:

    from epy import assemble, load_ref, ref_with_params

- ref_with_params(ctx, params, name="solution.py"): ref/<name> with its `P = None  # @@PARAMS@@` line
  replaced by the instance's parameters (the parameterized reference solution).
- load_ref(src): the reference as an in-process namespace, for building case inputs.
- assemble(ctx, module, task, rules, ref_src, exprs, n_visible, dev, stub, raises=("ValueError",)): one E
  instance: the stub, visible unittest examples (the first n_visible cases, protected) and NOTICE in the
  workspace; the reference as the solution; 20-40 hidden py_cases evaluated through the runner's own path
  (s1lib.py_eval); dev fixtures {name: source} as dev/<name>.tar overlays of <module>.py.
"""
import ast

import s1lib

PARAM_MARK = "P = None  # @@PARAMS@@"


def ref_with_params(ctx, params, name="solution.py"):
    """ref/<name> with its parameter line filled in (the parameterized reference solution)."""
    src = ctx.read("ref", name)
    if src.count(PARAM_MARK) != 1:
        raise s1lib.GenError("ref/%s must contain %r exactly once" % (name, PARAM_MARK))
    lit = repr(params)
    if ast.literal_eval(lit) != params:
        raise s1lib.GenError("parameters do not round-trip through repr")
    return src.replace(PARAM_MARK, "P = " + lit)


def load_ref(src):
    """The reference as an in-process namespace (trusted code), for building case inputs."""
    ns = {"__name__": "ref"}
    exec(compile(src, "<ref>", "exec"), ns)  # noqa: S102 - our own reference solution
    return ns


def make_cases(module, ref_src, exprs, raises=("ValueError",)):
    """Hidden cases: the reference evaluated through the runner's own path (s1lib.py_eval)."""
    if len(set(exprs)) != len(exprs):
        raise s1lib.GenError("duplicate case expressions")
    results = s1lib.py_eval({module + ".py": ref_src}, module, exprs)
    cases = []
    for i, (expr, r) in enumerate(zip(exprs, results)):
        name = "c%02d" % (i + 1)
        if r.get("ok"):
            if not r.get("repr"):
                raise s1lib.GenError("reference result too large: %s" % expr)
            ast.literal_eval(r["repr"])
            cases.append({"name": name, "expr": expr, "expect": r["repr"]})
            continue
        hit = [x for x in raises if x in r.get("exc", [])]
        if not hit:
            raise s1lib.GenError("reference raised %s on %s" % (r.get("exc"), expr))
        cases.append({"name": name, "expr": expr, "raises": hit[0]})
    return cases


def visible_tests(module, cases):
    lines = ["import unittest", "", "from %s import *  # noqa: F401,F403" % module, "", "",
             "class VisibleExamples(unittest.TestCase):"]
    for i, c in enumerate(cases, 1):
        lines.append("    def test_example_%d(self):" % i)
        if "raises" in c:
            lines += ["        with self.assertRaises(%s):" % c["raises"], "            %s" % c["expr"]]
        else:
            lines.append("        self.assertEqual(%s, %s)" % (c["expr"], c["expect"]))
        lines.append("")
    lines += ["", 'if __name__ == "__main__":', "    unittest.main()", ""]
    return "\n".join(lines)


def examples(cases):
    out = []
    for c in cases:
        if "raises" in c:
            out.append(">>> %s\nTraceback (most recent call last):\n  ...\n%s" % (c["expr"], c["raises"]))
        else:
            out.append(">>> %s\n%s" % (c["expr"], c["expect"]))
    return "```python\n" + "\n".join(out) + "\n```"


def assemble(ctx, module, task, rules, ref_src, exprs, n_visible, dev, stub, raises=("ValueError",)):
    """One E instance: stub + visible tests + NOTICE in the workspace, the reference as the solution and
    20-40 hidden py_cases (the first n_visible are also the visible examples)."""
    if not 20 <= len(exprs) <= 40 or not 3 <= n_visible <= 5:
        raise s1lib.GenError("need 20-40 cases and 3-5 visible examples (%d, %d)" % (len(exprs), n_visible))
    cases = make_cases(module, ref_src, exprs, raises)
    visible = cases[:n_visible]
    test_path = "tests/test_%s.py" % module
    prompt = (task + "\n\nThe rules below differ from the well-known exercise of a similar name, so follow them "
              "exactly.\n\n" + rules.strip() + "\n\n## Examples\n\n" + examples(visible) + "\n\n"
              "`%s` checks these examples (run `python3 -m unittest %s`). Do not edit the tests. Your code "
              "will also be checked on many other inputs under the same rules. Use only the Python standard "
              "library (python3 >= 3.9)." % (test_path, test_path))
    inst = s1lib.Instance(ctx, prompt=prompt)
    inst.workspace[module + ".py"] = stub
    inst.workspace[test_path] = visible_tests(module, visible)
    inst.notice()
    inst.protect(test_path)
    inst.solution[module + ".py"] = ref_src
    inst.hidden["hidden/cases.json"] = s1lib.dumps(cases)
    inst.step({"kind": "py_cases", "name": module, "import": module, "cases": "hidden/cases.json"})
    for name, src in sorted(dev.items()):
        inst.dev[name] = {module + ".py": src}
    return inst
