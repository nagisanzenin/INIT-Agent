"""s1c: the family-C helper of pack s1 (docs/m7-design.md §3.1.1 C). Python >= 3.9, stdlib only.

Family C = CRUXEval-seeded *change* tasks. Each template takes one CRUXEval function (facebookresearch/cruxeval
at CRUX_COMMIT, MIT), gives it a seeded name, splits it into a small package (`<pkg>/__init__.py`,
`<pkg>/<core>.py`, `<pkg>/<helper>.py`) and asks for a seeded behaviour change drawn from the eight operators
below. The task always asks for NEW behaviour, never "restore the original", so looking the public function up
solves nothing. Hidden I/O comes from the modified reference, evaluated through the shared runner's own
evaluation path (s1lib.py_eval) on seeded, type-preserving perturbations of the CRUXEval input plus fresh
seeded inputs of the same types.

Operators (one primary operator per template; its parameters are seeded):
  out_map    post-process the return value (a type-directed transform of the result)
  in_map     pre-process an argument before the existing logic runs
  const      change literal constants of the logic (thresholds, characters, fill values)
  param      lift hard-coded values into new parameters (hidden calls always pass them)
  cond       change a predicate of a condition or filter
  order      change a traversal, selection or sort order
  ret_extra  also return a seeded extra value computed by the same logic (the result becomes a pair)
  case_add   add new cases to existing dispatch logic, at stated positions

Difficulty knob (template.json difficulty 1..5; the Root moves it by one step from per-template pass rates):
  1  primary change, 5 visible examples, plus the template's hint line
  2  primary change, 4 visible examples
  3  primary change, 3 visible examples (the default)
  4  primary change + a seeded secondary out_map twist on the final result, 4 visible examples
  5  primary change + twist, 3 visible examples

Cases: the hidden set is the visible examples plus N_HIDDEN seeded cases. At most MAX_SAME_FRAC of the hidden
cases may have the same expected value under the original function (regression guards), so the unchanged
workspace scores <= 1000 * (MAX_SAME_FRAC * N_HIDDEN) / (N_VIS + N_HIDDEN) < 500. Inputs on which the
modified reference raises are dropped. Values are ASCII only and never depend on set or hash order, so the
expected reprs are identical on every python3 >= 3.9.
"""
from __future__ import annotations

import os
import sys

sys.dont_write_bytecode = True
_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(_HERE, "..", "..", "lib"))
import s1lib  # noqa: E402

CRUX_REPO = "https://github.com/facebookresearch/cruxeval"
CRUX_COMMIT = "190faf16d175b5847b0af05d937872b1fb395942"
OPERATORS = ("out_map", "in_map", "const", "param", "cond", "order", "ret_extra", "case_add")

N_HIDDEN = 27
MAX_SAME_FRAC_NUM, MAX_SAME_FRAC_DEN = 1, 5   # <= 20% of the hidden cases may be regression guards
N_VIS = {1: 5, 2: 4, 3: 3, 4: 4, 5: 3}
VIS_MAX_CHARS = 150                           # expr + expected repr of a visible example
CASE_MAX_CHARS = 600

PKG_STEMS = ["tally", "wordkit", "strops", "listops", "ledger", "glyph", "parcel", "relay", "sieve", "quill",
             "beacon", "loom", "ferry", "spool", "lattice", "harbor", "cobble", "tinder", "marrow", "pylon"]
CORE_NAMES = ["core", "ops", "logic", "impl", "engine"]
HELPER_NAMES = ["util", "helpers", "support", "consts", "shared"]

# Secondary twists (difficulty >= 4): (description, expression over the result `r`), by result kind.
TWISTS = {
    "str": [
        ("Finally, the function must return the resulting string reversed.", "r[::-1]"),
        ("Finally, the function must swap the case of every letter of the resulting string (as `str.swapcase` "
         "does) before returning it.", "r.swapcase()"),
        ("Finally, the function must wrap the resulting string in square brackets, returning "
         "`'[' + result + ']'`.", "'[' + r + ']'"),
    ],
    "list": [
        ("Finally, the function must return the resulting list in reverse order.", "r[::-1]"),
        ("Finally, the function must rotate the resulting list left by one position before returning it: "
         "the first element moves to the end (an empty list stays empty).", "r[1:] + r[:1]"),
        ("Finally, the function must append the length of the resulting list (before appending) as one extra "
         "last element.", "r + [len(r)]"),
    ],
    "int": [
        ("Finally, the function must return `2 * result + 1` instead of `result`.", "2 * r + 1"),
        ("Finally, the function must return `result - 7` instead of `result`.", "r - 7"),
    ],
}


class Names:
    """The seeded identifiers of one instance."""

    def __init__(self, rng: s1lib.Rng, fn_choices: list):
        self.pkg = "%s_%s" % (rng.choice(PKG_STEMS), rng.word(5))
        self.core = rng.choice(CORE_NAMES)
        self.helper = rng.choice([h for h in HELPER_NAMES if h != self.core])
        self.fn = rng.choice(fn_choices)

    def sub(self, src: str) -> str:
        return (src.replace("__FN__", self.fn).replace("__HELPER__", self.helper)
                .replace("__CORE__", self.core).replace("__PKG__", self.pkg))


# ------------------------------------------------------------------------------------- perturbations


def rand_str(rng: s1lib.Rng, alphabet: str, lo: int, hi: int) -> str:
    return "".join(rng.choice(alphabet) for _ in range(rng.randint(lo, hi)))


def mutate_str(rng: s1lib.Rng, s: str, alphabet: str, edits: int | None = None) -> str:
    """A type-preserving perturbation: `edits` random replace/insert/delete operations."""
    out = list(s)
    for _ in range(edits if edits is not None else rng.randint(1, 3)):
        op = rng.below(3)
        if op == 0 and out:
            out[rng.below(len(out))] = rng.choice(alphabet)
        elif op == 1 or not out:
            out.insert(rng.below(len(out) + 1), rng.choice(alphabet))
        else:
            del out[rng.below(len(out))]
    return "".join(out)


def rand_ints(rng: s1lib.Rng, lo: int, hi: int, nlo: int, nhi: int) -> list:
    return [rng.randint(lo, hi) for _ in range(rng.randint(nlo, nhi))]


def mutate_ints(rng: s1lib.Rng, xs: list, lo: int, hi: int, edits: int | None = None) -> list:
    out = list(xs)
    for _ in range(edits if edits is not None else rng.randint(1, 3)):
        op = rng.below(3)
        if op == 0 and out:
            out[rng.below(len(out))] = rng.randint(lo, hi)
        elif op == 1 or not out:
            out.insert(rng.below(len(out) + 1), rng.randint(lo, hi))
        else:
            del out[rng.below(len(out))]
    return out


# ----------------------------------------------------------------------------------------- the build


def call_expr(fn: str, args: tuple) -> str:
    return "%s(%s)" % (fn, ", ".join(s1lib.py_literal(a) for a in args))


def package_files(names: Names, core_src: str, helper_src: str | None, doc: str) -> dict:
    files = {
        "%s/__init__.py" % names.pkg: '"""%s: %s"""\nfrom .%s import %s\n\n__all__ = ["%s"]\n' % (
            names.pkg, doc, names.core, names.fn, names.fn),
        "%s/%s.py" % (names.pkg, names.core): names.sub(core_src),
    }
    if helper_src is not None:
        files["%s/%s.py" % (names.pkg, names.helper)] = names.sub(helper_src)
    return files


def with_twist(core_src: str, fn: str, expr: str) -> str:
    """The reference with a secondary twist: the finished function is wrapped (reference only; hidden)."""
    return core_src.rstrip("\n") + (
        "\n\n\n_inner_%s = %s\n\n\ndef %s(*args, **kwargs):\n    r = _inner_%s(*args, **kwargs)\n"
        "    return %s\n" % (fn, fn, fn, fn, expr))


def select_cases(exprs: list, ref: list, orig: list, n_vis: int, n_hidden: int, vis_ok=None) -> tuple:
    rows = []
    for e, r, o in zip(exprs, ref, orig):
        if not r.get("ok") or not isinstance(r.get("repr"), str) or not r["repr"]:
            continue
        if len(e) + len(r["repr"]) > CASE_MAX_CHARS:
            continue
        same = bool(o.get("ok")) and o.get("repr") == r["repr"]
        rows.append((e, r["repr"], same))
    visible, rest = [], []
    for row in rows:
        if (len(visible) < n_vis and not row[2] and len(row[0]) + len(row[1]) <= VIS_MAX_CHARS
                and (vis_ok is None or vis_ok(row[1]))):
            visible.append(row)
        else:
            rest.append(row)
    max_same = (n_hidden * MAX_SAME_FRAC_NUM) // MAX_SAME_FRAC_DEN
    hidden, n_same = [], 0
    for row in rest:
        if len(hidden) == n_hidden:
            break
        if row[2]:
            if n_same >= max_same:
                continue
            n_same += 1
        hidden.append(row)
    if len(visible) < n_vis or len(hidden) < n_hidden:
        raise s1lib.GenError("not enough usable cases (visible %d/%d, hidden %d/%d from %d candidates)" % (
            len(visible), n_vis, len(hidden), n_hidden, len(exprs)))
    return visible, hidden


def make(ctx: s1lib.Ctx, *, operator: str, original: str, fn_choices: list, params: str, doc: str, core: str,
         helper: str | None, variant, inputs, kind: str | None = None, twists: list | None = None,
         n_candidates: int = 160, vis_ok=None) -> s1lib.Instance:
    """Builds one family-C instance.

    operator    the template's primary operator (one of OPERATORS; becomes a task label)
    original    the verbatim CRUXEval source (`def f(...)`); in dev mode (INIT_S1_DEV=1) the packaged original
                must agree with it on every candidate input
    fn_choices  seeded function names            params  the current parameter list, for the prompt
    doc         one-line package docstring       core/helper  original sources (placeholders __FN__, __HELPER__)
    variant(rng) -> {"change": prompt text, "core": new core src or None, "helper": new helper src or None,
                     "sig": new parameter list or None, "hint": hint line, "state": anything for inputs(),
                     optional "kind"/"twists" overriding the arguments below}
    inputs(rng, state) -> list of argument tuples (seeded candidates, in priority order)
    kind / twists  the result kind for the generic twists ("str", "list", "int") or an explicit twist list
    vis_ok(expected_repr) -> bool  optional: which cases make informative visible examples
    """
    if operator not in OPERATORS:
        raise s1lib.GenError("unknown operator %r" % operator)
    d = ctx.difficulty
    rng = ctx.rng("names")
    names = Names(rng, fn_choices)
    var = variant(ctx.rng("variant"))
    new_core = var.get("core") or core
    new_helper = var.get("helper") or helper
    twist = None
    kind, twists = var.get("kind", kind), var.get("twists", twists)
    if d >= 4:
        menu = twists if twists is not None else TWISTS[kind]
        twist = ctx.rng("twist").choice(menu)
        new_core = with_twist(new_core, "__FN__", twist[1])
    orig_files = package_files(names, core, helper, doc)
    new_files = package_files(names, new_core, new_helper, doc)

    cand_rng = ctx.rng("inputs")
    seen, exprs = set(), []
    for args in inputs(cand_rng, var.get("state")):
        if not isinstance(args, tuple):
            raise s1lib.GenError("inputs() must yield argument tuples")
        e = call_expr(names.fn, args)
        if e not in seen:
            seen.add(e)
            exprs.append(e)
        if len(exprs) >= n_candidates:
            break
    ref = s1lib.py_eval(new_files, names.pkg, exprs)
    orig = s1lib.py_eval(orig_files, names.pkg, exprs)
    if ctx.dev:
        verb = s1lib.py_eval({"crux_original.py": original}, "crux_original",
                             ["f" + e[len(names.fn):] for e in exprs])
        for e, a, b in zip(exprs, orig, verb):
            if a != b:
                raise s1lib.GenError("packaged original differs from CRUXEval on %s: %r vs %r" % (e, a, b))
    visible, hidden = select_cases(exprs, ref, orig, N_VIS[d], N_HIDDEN, vis_ok)

    pkg, fn = names.pkg, names.fn
    test_path = "tests/test_%s.py" % pkg
    where = "`%s/%s.py`" % (pkg, names.core)
    if helper is not None:
        where += " (it uses `%s/%s.py`)" % (pkg, names.helper)
    parts = [
        "The Python package `%s/` in this workspace defines `%s(%s)` in %s. Change its behaviour as "
        "described below." % (pkg, fn, params, where),
        names.sub(var["change"]).strip(),
    ]
    if twist is not None:
        parts.append(twist[0])
    if var.get("sig"):
        parts.append("The new signature is `%s(%s)`. Keep the name `%s`." % (fn, var["sig"], fn))
    parts.append("Apart from this, the function must behave exactly as it does now, including its current "
                 "handling of edge cases.")
    parts.append("Examples of the required behaviour:\n" + "\n".join(
        "    %s == %s" % (e, r) for e, r, _ in visible))
    parts.append("These examples are the tests in `%s` (run `python3 -m unittest %s`). Do not edit anything "
                 "under `tests/`. The function is also checked on other inputs. Use only the Python standard "
                 "library." % (test_path, test_path))
    if d == 1 and var.get("hint"):
        parts.append("Hint: " + names.sub(var["hint"]).strip())
    inst = s1lib.Instance(ctx, prompt="\n\n".join(parts))
    inst.workspace.update(orig_files)
    inst.workspace["README.md"] = "# %s\n\n%s\n\nPublic function: `%s` (in `%s/%s.py`).\n" % (
        pkg, doc[:1].upper() + doc[1:], fn, pkg, names.core)
    inst.notice()
    inst.workspace[test_path] = s1lib.unittest_source(
        pkg, [("example_%d" % i, e, r) for i, (e, r, _) in enumerate(visible)])
    inst.protect(test_path)
    inst.token(pkg)
    for p, src in new_files.items():
        if src != orig_files.get(p):
            inst.solution[p] = src
    cases = [{"name": "v%02d" % i, "expr": e, "expect": r} for i, (e, r, _) in enumerate(visible)]
    cases += [{"name": "h%02d" % i, "expr": e, "expect": r} for i, (e, r, _) in enumerate(hidden)]
    inst.hidden["hidden/cases.json"] = s1lib.dumps(cases)
    inst.step({"kind": "py_cases", "name": fn, "import": pkg, "cases": "hidden/cases.json"})
    inst.labels = ["change", operator.replace("_", "-")]
    return inst
