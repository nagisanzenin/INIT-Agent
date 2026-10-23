"""ersgen: shared generator helpers for the Rust family-E templates of pack s1 (lane S1-E-RS).

Written fresh for INIT (INIT licence). Python >= 3.9, stdlib only. Used by every E/e-rs-*/gen/run
(except S1-CORE's example e-rs-rle, which is self-contained):

    import os, sys
    sys.dont_write_bytecode = True
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "_shared_rs"))
    import ersgen                    # also puts <pack>/lib on sys.path
    s1lib = ersgen.s1lib

    def build(ctx):
        rng = ctx.rng()
        ...
        return ersgen.finish(ctx, crate=..., api=..., intro=..., rules=..., stub=..., reference=...,
                             naive=..., uses=..., cases=[...], n_visible=4)

    if __name__ == "__main__":
        s1lib.main(build)

Each instance is a one-crate Cargo workspace:
  Cargo.toml, src/lib.rs (stub), tests/visible.rs (protected), README.md (the task text), NOTICE (MIT)
and its check is one `cargo_test` step of the shared runner over hidden/hidden_s1.rs (the visible
examples plus the seeded hidden cases, one #[test] each, partial credit), built against hidden/Cargo.toml.

A case is (name, expr, want) -> `assert_eq!(expr, want);`, or (name, None, body) -> body verbatim
(several statements, for stateful APIs). The template's reference solution (`ref/`, rendered with the
seeded constants) is the instance's solution; `ref/naive.rs` (the unmutated public behaviour in the same
API) becomes the dev fixture `dev/naive.tar`, which the validator requires to score below 1000.
"""
from __future__ import annotations

import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "lib"))
import s1lib  # noqa: E402

EXERCISM_RUST_COMMIT = "cc099ae090a6acbe6f53f1e36325263046ba333e"
HIDDEN_TEST = "hidden_s1.rs"

MANIFEST = '''[package]
name = "@CRATE@"
version = "0.1.0"
edition = "2021"
build = false
publish = false

[lib]
path = "src/lib.rs"

[dependencies]
'''

FOOTER = (
    "Constraints: use the Rust standard library only (no extra crates). Do not edit `tests/` or "
    "`Cargo.toml`. Run `cargo test` to try the examples. Your code will also be checked on many other "
    "inputs, so implement the rules in general; do not special-case the examples."
)


def manifest(crate: str) -> str:
    return MANIFEST.replace("@CRATE@", crate)


def render(src: str, values: dict) -> str:
    """Replaces @NAME@ placeholders (Rust code is full of braces, so no str.format)."""
    for k, v in values.items():
        src = src.replace("@%s@" % k, str(v))
    left = re.findall(r"@[A-Z][A-Z_0-9]*@", src)
    if left:
        raise s1lib.GenError("unrendered placeholders: %s" % sorted(set(left)))
    return src


def rs_bool(b: bool) -> str:
    return "true" if b else "false"


def rs_char(c: str) -> str:
    if c == "'":
        return "'\\''"
    if c == "\\":
        return "'\\\\'"
    if 0x20 <= ord(c) < 0x7f:
        return "'%s'" % c
    return "'\\u{%x}'" % ord(c)


def rs_str(s: str) -> str:
    return s1lib.rust_str(s)


def rs_string(s: str) -> str:
    return "%s.to_string()" % s1lib.rust_str(s)


def rs_vec(items, f=str) -> str:
    return "vec![%s]" % ", ".join(f(x) for x in items)


def tests_source(uses: str, cases: list) -> str:
    lines = [uses, ""] if uses else []
    for case in cases:
        name, expr, want = case
        lines += ["#[test]", "fn %s() {" % name]
        if expr is None:
            lines += ["    " + ln if ln else "" for ln in want.rstrip("\n").split("\n")]
        else:
            lines.append("    assert_eq!(%s, %s);" % (expr, want))
        lines += ["}", ""]
    return "\n".join(lines)


def example_lines(cases: list) -> str:
    out = []
    for name, expr, want in cases:
        if expr is None:
            out.append("    // %s" % name)
            out += ["    " + ln if ln else "" for ln in want.rstrip("\n").split("\n")]
        else:
            out.append("    assert_eq!(%s, %s);" % (expr, want))
    return "\n".join(out)


def finish(ctx, *, crate: str, api: str, intro: str, rules: str, stub: str, reference: str, naive: str | None,
           uses: str, cases: list, n_visible: int, title: str | None = None):
    """Builds the Instance. `cases` are all graded cases; the first `n_visible` are the visible examples
    (renamed example_N in tests/visible.rs; graded again in the hidden file under their case names)."""
    if not 3 <= n_visible <= 5:
        raise s1lib.GenError("3..5 visible examples")
    hidden_n = len(cases) - n_visible
    if not 20 <= hidden_n <= 40:
        raise s1lib.GenError("20..40 hidden cases, got %d" % hidden_n)
    names = [c[0] for c in cases]
    if len(set(names)) != len(names):
        raise s1lib.GenError("duplicate case names")
    visible = [("example_%d" % (i + 1), e, w) for i, (_, e, w) in enumerate(cases[:n_visible])]
    prompt = (
        "%s\n\nImplement this public API in `src/lib.rs` of the crate `%s`:\n\n%s\n\n"
        "Rules (follow them exactly; they differ from the well-known version of this exercise):\n\n%s\n\n"
        "Examples (the same checks are in `tests/visible.rs`):\n\n%s\n\n%s"
        % (intro, crate, "\n".join("    " + ln if ln else "" for ln in api.strip("\n").split("\n")), rules.strip("\n"),
           example_lines(visible), FOOTER))
    inst = s1lib.Instance(ctx, prompt=prompt, title=title)
    inst.workspace["Cargo.toml"] = manifest(crate)
    inst.workspace["src/lib.rs"] = stub
    inst.workspace["tests/visible.rs"] = tests_source(uses, visible)
    inst.workspace["README.md"] = "# %s\n\n%s\n" % (title or ctx.template["title"], prompt)
    inst.notice()
    inst.marker_files = ["src/lib.rs", "tests/visible.rs", "Cargo.toml"]
    inst.protect("tests/visible.rs", "Cargo.toml")
    inst.solution["src/lib.rs"] = reference
    if naive is not None:
        inst.dev["naive"] = {"src/lib.rs": naive}
    inst.hidden["hidden/Cargo.toml"] = manifest(crate)
    inst.hidden["hidden/" + HIDDEN_TEST] = tests_source(uses, cases)
    inst.step({"kind": "cargo_test", "name": crate, "manifest": "hidden/Cargo.toml",
               "tests": {HIDDEN_TEST: "hidden/" + HIDDEN_TEST}, "names": names})
    return inst
