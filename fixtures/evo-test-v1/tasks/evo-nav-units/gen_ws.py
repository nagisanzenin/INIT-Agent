"""evo-nav-units: render the stockroom package from tpl/ into a workspace.

    gen_ws.py <dir> before    the initial package (workspace.gen)
    gen_ws.py <dir> after     the reference change applied (solve.sh)

Every tpl/ file is one source for both variants. Marker lines (alone on their line, any indentation):
    #@B   lines up to the next marker exist only before the change
    #@A   lines up to the next marker exist only after the change
    #@E   end of a variant block
A file that renders to nothing for a variant is not written (and is removed when rendering `after`).
Then gen_more.py adds the generated partner feeds, marketplace exporters and grouped reports.
Deterministic: sorted walk, "\n" line ends, mode 0644. Python 3.9, stdlib only.
"""
import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TPL = os.path.join(HERE, "tpl")
MARKERS = ("#@B", "#@A", "#@E")


def render(text, variant, name):
    out, mode = [], None
    for n, line in enumerate(text.split("\n"), 1):
        s = line.strip()
        if s in MARKERS:
            if s == "#@E":
                if mode is None:
                    raise SystemExit("%s:%d: #@E outside a block" % (name, n))
                mode = None
            else:
                mode = s[2]
            continue
        if mode is None or (mode == "B") == (variant == "before"):
            out.append(line)
    if mode is not None:
        raise SystemExit("%s: unterminated #@%s block" % (name, mode))
    return "\n".join(out)


def main(argv):
    if len(argv) != 3 or argv[2] not in ("before", "after"):
        raise SystemExit("usage: gen_ws.py <dir> before|after")
    dest, variant = argv[1], argv[2]
    for dirpath, dirnames, filenames in os.walk(TPL):
        dirnames.sort()
        for fn in sorted(filenames):
            if fn.endswith(".pyc") or fn == ".DS_Store":
                continue
            src = os.path.join(dirpath, fn)
            rel = os.path.relpath(src, TPL)
            with open(src, "r", encoding="utf-8") as f:
                body = render(f.read(), variant, rel)
            out = os.path.join(dest, rel)
            if not body.strip():
                if os.path.exists(out):
                    os.unlink(out)
                continue
            os.makedirs(os.path.dirname(out), exist_ok=True)
            with open(out, "w", encoding="utf-8", newline="\n") as f:
                f.write(body)
            os.chmod(out, 0o644)
    spec = importlib.util.spec_from_file_location("gen_more", os.path.join(HERE, "gen_more.py"))
    gen_more = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gen_more)
    gen_more.generate(dest, variant)


if __name__ == "__main__":
    main(sys.argv)
