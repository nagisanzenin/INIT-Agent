"""Hidden check inputs for evo-dbg-diff (author tool; `dev.py cases` runs them through the reference).

ITEMS = [(item_name, [case, ...])]; case = {"files": {rel: text}, "argv": [...], "stdin"?: str,
"outfiles"?: [rel, ...]}. Inputs are hand-written or generated from fixed seeds; patches are made with difflib
(independent of the workspace) or written by hand."""
import difflib
import random

WORDS = ("alpha bravo cedar delta ember fable garnet harbor island jasper kettle lantern meadow nectar orbit "
         "pepper quartz raven saddle timber umber velvet willow xenon yonder zephyr amber birch cobalt dune "
         "fern grove heron iris juniper kelp lilac maple nutmeg onyx").split()
CODE = ["", "", "", "}", "{", "    return x;", "    x += 1;", "    if (x) {", "    } else {", "    break;",
        "int x = 0;", "// note", "    y = f(x);", "    }", "x = 1", "pass"]


def text(lines, eol=True):
    s = "".join(line + "\n" for line in lines)
    return s if eol or not s else s[:-1]


def uniq_lines(rng, n, tag="line"):
    return ["%s %d: %s %s" % (tag, i + 1, rng.choice(WORDS), rng.choice(WORDS)) for i in range(n)]


def code_lines(rng, n):
    return [rng.choice(CODE) for _ in range(n)]


def mutate(rng, lines, edits, pool):
    """Random deletions, insertions and replacements; new lines come from `pool` (a list or a callable)."""
    out = list(lines)
    for _ in range(edits):
        op = rng.randrange(3)
        i = rng.randrange(len(out) + 1)
        new = pool() if callable(pool) else rng.choice(pool)
        if op == 0 and i < len(out):
            del out[i]
        elif op == 1 or i >= len(out):
            out.insert(i, new)
        else:
            out[i] = new
    return out


def fresh(rng, prefix):
    counter = [0]

    def make():
        counter[0] += 1
        return "%s %d %s" % (prefix, counter[0], rng.choice(WORDS))
    return make


def keep_ends(t):
    """The lines of a text that ends with a newline, each with its newline (only \\n ends a line)."""
    return [part + "\n" for part in t.split("\n")[:-1]]


def udiff(old, new, la="a/file.txt", lb="b/file.txt", n=3):
    """A unified diff made by difflib (both texts must end with a newline)."""
    out = "".join(difflib.unified_diff(keep_ends(old), keep_ends(new), la, lb, n=n))
    assert out, "no difference"
    return out


def D(old, new, *opts, **kw):
    names = kw.get("names", ("old.txt", "new.txt"))
    files = {}
    if old is not None:
        files[names[0]] = old
    if new is not None:
        files[names[1]] = new
    c = {"files": files, "argv": ["diff"] + list(opts) + list(kw.get("operands", names))}
    if "stdin" in kw:
        c["stdin"] = kw["stdin"]
    return c


def P(target, patch, *opts, **kw):
    fname = kw.get("fname", "file.txt")
    pname = kw.get("pname", "fix.patch")
    files = {fname: target, pname: patch}
    files.update(kw.get("extra", {}))
    outs = list(kw.get("outfiles", [fname]))
    if kw.get("rej"):
        outs.append(kw.get("rejname", fname + ".rej"))
    operands = kw.get("operands", [fname, pname])
    c = {"files": files, "argv": ["patch"] + list(opts) + operands, "outfiles": outs}
    if "stdin" in kw:
        c["stdin"] = kw["stdin"]
    return c


def S(patch, *opts):
    return {"files": {"x.patch": patch}, "argv": ["stat"] + list(opts) + ["x.patch"]}


ITEMS = []


def item(name, cases):
    ITEMS.append((name, cases))


# ------------------------------------------------------------------------------------------------- diff

rng = random.Random(1101)
base = uniq_lines(rng, 12, "row")
item("diff_identical_and_brief", [
    D(text(base), text(base)),
    D("", ""),
    D(text(base, False), text(base, False)),
    D(text(base), text(base[:5] + ["changed"] + base[6:]), "-q", names=("a.txt", "b.txt")),
    D(text(base), text(base), "--brief"),
])

rng = random.Random(1202)
a = uniq_lines(rng, 20)
b = list(a)
b[9] = "line 10: replaced"
c2 = list(a)
c2[2], c2[16] = "third line rewritten", "seventeenth line rewritten"
amb_old = ["int main() {", "    int x = 0;", "    x += 1;", "    return x;", "}", "", "int g() {", "    return 1;",
           "}"]
amb_new = ["int main() {", "    int x = 0;", "    return x;", "    x += 1;", "}", "", "int g() {", "    x += 1;",
           "    return 1;", "}"]
item("diff_replace_lines", [
    D(text(a), text(b)),
    D(text(a), text(c2)),
    D(text(amb_old), text(amb_new)),
])

item("diff_alignment_tiebreak", [
    D("a\nb\n", "b\na\n"),
    D("x\ny\nx\ny\n", "y\nx\ny\nx\n"),
    D(text(["{", "a", "}", "{", "b", "}", "{", "c", "}"]), text(["{", "b", "}", "{", "a", "}", "{", "c", "}"])),
    D(text(["one", "two", "three", "four", "five"]), text(["five", "one", "two", "three", "four"])),
])

item("diff_block_order", [
    D(text(["p", "q", "r", "s", "t"]), text(["q", "p", "X", "s", "r", "t"])),
    D(text(["a", "1", "b", "2", "c", "3"]), text(["a", "2", "b", "1", "c", "3", "a"])),
    D(text(["", "x", "", "y", ""]), text(["x", "", "", "y", "", "x"])),
])

rng = random.Random(1303)
ctx_old = code_lines(rng, 40)
ctx_new = mutate(rng, ctx_old, 7, CODE)
item("diff_context_sizes", [
    D(text(ctx_old), text(ctx_new), "-U", "0"),
    D(text(ctx_old), text(ctx_new), "-U1"),
    D(text(ctx_old), text(ctx_new), "-U", "2"),
    D(text(ctx_old), text(ctx_new), "--unified=4"),
    D(text(ctx_old), text(ctx_new), "-U", "10"),
    D(text(ctx_old), text(ctx_new), "-U", "1", "-u"),
])


def gapped(n_lines, changes, seed):
    r = random.Random(seed)
    old = uniq_lines(r, n_lines, "entry")
    new = list(old)
    for i in changes:
        new[i] = old[i] + " (edited)"
    return text(old), text(new)


g1 = gapped(20, [5, 8], 1)        # gap 2
g2 = gapped(24, [4, 9], 2)        # gap 4
g3 = gapped(30, [6, 13], 3)       # gap 6
g4 = gapped(30, [6, 14], 4)       # gap 7
g5 = gapped(40, [3, 10, 18], 5)   # gaps 6 and 7
item("diff_merge_rule", [
    D(g1[0], g1[1], "-U", "1"),
    D(g2[0], g2[1], "-U2"),
    D(g3[0], g3[1]),
    D(g4[0], g4[1]),
    D(g5[0], g5[1]),
])

rng = random.Random(1404)
r9 = uniq_lines(rng, 9, "item")
item("diff_range_forms", [
    D(text(r9), text(["first"] + r9[1:]), "-U0"),
    D(text(r9), text(r9[:-1]), "-U0"),
    D(text(r9), text(["new top"] + r9), "-U0"),
    D(text(r9), text(r9 + ["new end 1", "new end 2"]), "-U0"),
    D(text(r9), text(r9[2:]), "-U0"),
    D(text(["x", "a", "x", "b", "x"]), text(["a", "x", "b", "x", "a"]), "-U0"),
    D("head\np\nq\ntail\n", "head\nq\np\ntail\n", "-U0"),
    D("only\n", "other\n"),
])

item("diff_empty_files", [
    D("", "a\nb\nc\n"),
    D("a\nb\nc\n", ""),
    D(None, "first\nsecond\n", "-N", names=("missing.txt", "new.txt")),
    D("gone\n", None, "--new-file", names=("old.txt", "absent.txt")),
    D("", "x"),
])

rng = random.Random(1505)
e = ["a", "b", "c", "d", "e", "f", "g"]
item("diff_file_edges", [
    D(text(e), text(["A"] + e[1:6] + ["G"])),
    D(text(e), text(["x"] + e + ["x"])),
    D(text(["x", "x", "y"]), text(["x", "y", "x", "y"])),
    D(text(["b", "a", "b", "c", "d"]), text(["a", "b", "c", "d", "b"]), "-U", "1"),
    D(text(["second", "first"] + e), text(["first", "second"] + e)),
])

item("diff_no_newline", [
    D(text(e), text(e, False)),
    D(text(e, False), text(e)),
    D(text(e, False), text(e[:2] + ["C"] + e[3:], False)),
    D(text(e, False), text(e[:-1] + ["G"], False)),
    D(text(["x", "y", "x"], False), text(["x", "y", "x", "y", "x"], False)),
    D(text(e[:3] + ["q", "p", "last"], False), text(e[:3] + ["p", "q", "last"], False), "-U", "2"),
])

rng = random.Random(1606)
lo = code_lines(rng, 18)
ln = mutate(rng, lo, 4, CODE)
item("diff_labels_and_operands", [
    D(text(lo), text(ln), "-L", "a/src/main.c", "-L", "b/src/main.c"),
    D(text(lo), text(ln), "--label", "before", operands=["old.txt", "--label=after", "new.txt"]),
    D(text(lo), text(ln), "--label=only-old"),
    D(text(lo), text(ln), operands=["-U", "1", "--", "-old.txt", "new.txt"], names=("-old.txt", "new.txt")),
    D(text(lo), text(ln), "-U", "0", "-U", "2"),
])

rng = random.Random(1707)
so = code_lines(rng, 15)
sn = mutate(rng, so, 4, CODE)
item("diff_stdin", [
    D(None, text(sn), operands=["-", "new.txt"], stdin=text(so)),
    D(text(so), None, "-U1", operands=["old.txt", "-"], stdin=text(sn)),
    D(None, "b\na\nc\n", operands=["-", "new.txt"], stdin="a\nb\nc\n"),
])

rng = random.Random(1808)
rand_cases = []
for k in range(5):
    old = code_lines(rng, rng.randrange(25, 60))
    new = mutate(rng, old, rng.randrange(4, 10), CODE)
    rand_cases.append(D(text(old, rng.random() < 0.85), text(new, rng.random() < 0.85)))
ff_old = code_lines(rng, 30)
ff_old[10:10] = ["\x0c"]
ff_old[22:22] = ["// page 2\x0c"]
rand_cases.append(D(text(ff_old), text(mutate(rng, ff_old, 5, CODE))))
item("diff_random_texts", rand_cases)

rng = random.Random(1909)
rand_cases = []
for k, n in enumerate(["0", "1", "2", "5"]):
    old = code_lines(rng, rng.randrange(30, 60))
    new = mutate(rng, old, rng.randrange(4, 9), CODE)
    rand_cases.append(D(text(old), text(new), "-U", n))
item("diff_random_context", rand_cases)

item("diff_errors", [
    D("a\n", None, names=("a.txt", "nope.txt")),
    D("a\n", "b\n", "-U", "x"),
    D("a\n", "b\n", "-U-1"),
    D("a\n", "b\n", "-L", "1", "-L", "2", "-L", "3"),
    D("a\n", "b\n", operands=["old.txt"]),
    D("a\n", "b\n", "-x"),
    {"files": {}, "argv": ["diff", "-", "-"], "stdin": "a\n"},
    {"files": {}, "argv": ["compare", "a", "b"]},
])

ig_old = ["def total(items):", "    result = 0", "    for item in items:", "        result += item.price",
          "    return result", "", "def count(items):", "    return len(items)"]
ig_case = ["DEF total(items):", "    Result = 0", "    for item in items:", "        result += item.price",
           "    return result", "", "def count(items):", "    return len(items) + 0"]
ig_space = ["def total(items):", "  result  =  0  ", "    for item in items:", "\tresult += item.price",
            "    return result", "", "def count(items):", "    return len (items)"]
ig_all = ["def total(items):", "    result=0", "    for item in items :", "        result += item.price",
          "    return result", "", "def count( items ):", "    return 1"]
ig_swap = ["DEF TOTAL(ITEMS):", "    for item in items:", "    result = 0", "        result += item.price",
           "    return result", "", "def count(items):", "    return len(items)"]
item("diff_ignore_options", [
    D(text(ig_old), text(ig_case), "-i"),
    D(text(ig_old), text(ig_space), "-b"),
    D(text(ig_old), text(ig_space), "--ignore-all-space"),
    D(text(ig_old), text(ig_all), "-w", "-U", "1"),
    D(text(ig_old), text(ig_swap), "-i", "-b"),
    D(text(ig_old), text([x.upper() for x in ig_old]), "--ignore-case", "-q"),
])

# ------------------------------------------------------------------------------------------------ patch


def multi_edit(seed, n_lines, positions, tag="line"):
    """old text and a new text with edits at the given old line indexes: ('r', i) replace, ('d', i, k) delete k
    lines, ('i', i, k) insert k lines before i."""
    r = random.Random(seed)
    old = uniq_lines(r, n_lines, tag)
    new = list(old)
    for op in sorted(positions, key=lambda p: -p[1]):
        if op[0] == "r":
            new[op[1]] = old[op[1]] + " (changed)"
        elif op[0] == "d":
            del new[op[1]:op[1] + op[2]]
        else:
            new[op[1]:op[1]] = ["added %d.%d" % (op[1], j) for j in range(op[2])]
    return old, new


m1 = multi_edit(2101, 40, [("i", 5, 2), ("d", 15, 1), ("r", 25), ("i", 34, 3)])
m2 = multi_edit(2102, 50, [("d", 3, 2), ("d", 14, 3), ("i", 26, 1), ("r", 38), ("i", 46, 2)])
m3 = multi_edit(2103, 30, [("i", 2, 1), ("i", 12, 1), ("r", 22)])
item("patch_multi_hunk_clean", [
    P(text(m1[0]), udiff(text(m1[0]), text(m1[1]))),
    P(text(m2[0]), udiff(text(m2[0]), text(m2[1]))),
    P(text(m3[0]), udiff(text(m3[0]), text(m3[1]))),
])

rng = random.Random(2202)
rt = []
for k, n in enumerate([3, 3, 2, 1, 3]):
    old = uniq_lines(rng, rng.randrange(40, 80), "rec")
    new = mutate(rng, old, rng.randrange(6, 12), fresh(rng, "new"))
    rt.append(P(text(old), udiff(text(old), text(new), n=n)))
ff = uniq_lines(rng, 50, "rec")
ff[12:12] = ["\x0c"]
ff[30] = ff[30] + "\x0b(vt)"
ffn = mutate(rng, ff, 6, fresh(rng, "new"))
rt.append(P(text(ff), udiff(text(ff), text(ffn))))
item("patch_roundtrip_random", rt)

o1 = multi_edit(2301, 40, [("i", 6, 2), ("d", 17, 2), ("r", 30)])
o2 = multi_edit(2302, 36, [("r", 8), ("i", 18, 1), ("r", 28)])
o3 = multi_edit(2303, 45, [("d", 5, 1), ("i", 15, 2), ("r", 25), ("r", 38)])
o3_target = list(o3[0])
o3_target[20:20] = ["inserted between the hunks"]
item("patch_offset_messages", [
    P(text(["# header %d" % i for i in range(5)] + o1[0]), udiff(text(o1[0]), text(o1[1]))),
    P(text(o2[0][1:]), udiff(text(o2[0]), text(o2[1]))),
    P(text(o3_target), udiff(text(o3[0]), text(o3[1]))),
    P(text(["# one more line"] + m3[0]), udiff(text(m3[0]), text(m3[1]))),
])

blk = ["{", "  open()", "  read()", "  close()", "}"]
pad = ["filler %d" % i for i in range(12)]
eq_target = pad[:2] + blk + pad[2:7] + blk + pad[7:]
eq_patch = ("--- a\n+++ b\n@@ -8,5 +8,6 @@\n {\n   open()\n   read()\n+  check()\n   close()\n }\n")
near_target = pad[:5] + blk + pad[5:8] + blk + pad[8:]
near_patch = ("--- a\n+++ b\n@@ -7,5 +7,5 @@\n {\n   open()\n-  read()\n+  read_all()\n   close()\n }\n")
fl = multi_edit(2401, 30, [("r", 4), ("r", 12)])
floor_target = list(fl[0])
floor_patch = udiff(text(fl[0]), text(fl[1]), n=2)
fz_old = uniq_lines(random.Random(2402), 30, "cfg")
fz_new = list(fz_old)
fz_new[10] = "cfg 11: replaced"
fz_target = list(fz_old)
fz_target[8] = "cfg 9: drifted"
fz_target[24:24] = fz_old[7:14]
o4 = multi_edit(2403, 40, [("i", 4, 3), ("i", 14, 2), ("r", 26)])
o4_target = list(o4[0])
o4_target[20:20] = ["extra a", "extra b"]
item("patch_offset_search_order", [
    P(text(eq_target), eq_patch),
    P(text(near_target), near_patch),
    P(text(fz_target), udiff(text(fz_old), text(fz_new))),
    P(text(o4_target), udiff(text(o4[0]), text(o4[1]))),
])

dup_unit = ["section", "  key = 1", "  value = 2", "end"]
dup_old = ["title", "intro"] + dup_unit + ["middle"] * 1 + ["sep %d" % i for i in range(4)] + dup_unit + ["tail"]
dup_new = list(dup_old)
dup_new[1:1] = ["added 1", "added 2", "added 3"]
dup_new[18] = "  value = 3"
dup_new[8:8] = ["more 1", "more 2"]
dup_patch = udiff(text(dup_old), text(dup_new), n=1)
dup_target = ["prefix"] * 2 + dup_old
z_old = ["z%d" % i for i in range(1, 21)]
z_new = list(z_old)
z_new[15:15] = ["ins B1", "ins B2"]
z_new[8:8] = ["ins A"]
z_new[2:4] = []
z_new[0:0] = ["top 1", "top 2"]
k_old, k_new = multi_edit(2501, 60, [("i", 5, 4), ("i", 16, 2), ("d", 30, 1), ("r", 44), ("r", 52)])
tie_body = ["def test_case(self):", "    self.setup()", "    self.run()", "    self.check()", ""]
tie_pre = ["# tests %d" % i for i in range(6)]
tie_old = tie_pre + ["import unittest", "", "class T(unittest.TestCase):", ""] + ["pad %d" % i for i in range(12)]
tie_new = list(tie_old)
tie_new[1:1] = ["# added 1", "# added 2"]
tie_target = (["# tests 0", "# tests 1", "# tests 2", "# tests 3", "# tests 4", "# tests 5", "import unittest"] +
              tie_body + ["", "class T(unittest.TestCase):", ""] + tie_body + ["pad %d" % i for i in range(6)])
tie_patch = ("--- a/t.py\n+++ b/t.py\n@@ -1,3 +1,5 @@\n # tests 0\n+# added 1\n+# added 2\n # tests 1\n # tests 2\n"
             "@@ -12,5 +14,5 @@\n def test_case(self):\n     self.setup()\n-    self.run()\n+    self.run(fast=True)\n"
             "     self.check()\n \n")
item("patch_carried_offset", [
    P(text(tie_target), tie_patch),
    P(text(dup_target), dup_patch),
    P(text(z_old), udiff(text(z_old), text(z_new), n=0)),
    P(text(["pre %d" % i for i in range(3)] + k_old), udiff(text(k_old), text(k_new))),
])

f_old = uniq_lines(random.Random(2601), 30, "opt")
f_new = list(f_old)
f_new[15] = "opt 16: tuned"
f_t1 = list(f_old)
f_t1[12] = "opt 13: local edit"
f_t2 = list(f_old)
f_t2[18] = "opt 19: local edit"
f_t3 = list(f_old)
f_t3[12], f_t3[13], f_t3[17], f_t3[18] = "e1", "e2", "e3", "e4"
f3_old, f3_new = multi_edit(2602, 45, [("i", 5, 2), ("d", 14, 2), ("r", 30)])
f3_target = list(f3_old)
f3_target[27] = "line 28: drifted"
item("patch_fuzz_symmetric", [
    P(text(f_t1), udiff(text(f_old), text(f_new))),
    P(text(f_t2), udiff(text(f_old), text(f_new))),
    P(text(f_t3), udiff(text(f_old), text(f_new))),
    P(text(f3_target), udiff(text(f3_old), text(f3_new))),
])

ed = uniq_lines(random.Random(2701), 16, "row")
ed_end_new = ed[:-1] + ["row 16: last changed"]
ed_end_target = list(ed)
ed_end_target[12] = "row 13: drifted"
ed_top_new = ["row 1: first changed"] + ed[1:]
ed_top_target = list(ed)
ed_top_target[3] = "row 4: drifted"
ed_near_new = ed[:14] + ["row 15: changed"] + ed[15:]
ed_near_target = list(ed)
ed_near_target[11], ed_near_target[12] = "row 12: drifted", "row 13: drifted"
item("patch_fuzz_file_edges", [
    P(text(ed_end_target), udiff(text(ed), text(ed_end_new))),
    P(text(ed_top_target), udiff(text(ed), text(ed_top_new))),
    P(text(ed_near_target), udiff(text(ed), text(ed_near_new))),
    P(text(ed_end_target), udiff(text(ed), text(ed_end_new)), "-F", "0", rej=True),
])

lim_old = uniq_lines(random.Random(2801), 24, "val")
lim_new = list(lim_old)
lim_new[11] = "val 12: new"
lim_t1 = list(lim_old)
lim_t1[8] = "val 9: other"
lim_t2 = list(lim_old)
lim_t2[8], lim_t2[9] = "val 9: other", "val 10: other"
lim_t3 = list(lim_old)
lim_t3[8], lim_t3[9], lim_t3[10] = "x", "y", "z"
end_old = uniq_lines(random.Random(2802), 14, "end")
end_new = end_old[:12] + ["end 13: changed", end_old[13]]
end_t = list(end_old)
end_t[9], end_t[10] = "end 10: drifted", "end 11: drifted"
item("patch_fuzz_limits", [
    P(text(lim_t1), udiff(text(lim_old), text(lim_new)), "-F", "0", rej=True),
    P(text(lim_t2), udiff(text(lim_old), text(lim_new)), "-F1", rej=True),
    P(text(lim_t2), udiff(text(lim_old), text(lim_new)), "--fuzz=2"),
    P(text(lim_t3), udiff(text(lim_old), text(lim_new)), "-F", "3"),
    P(text(end_t), udiff(text(end_old), text(end_new))),
])

rj_old, rj_new = multi_edit(2901, 40, [("i", 4, 2), ("d", 14, 3), ("r", 27)])
rj_target = list(rj_old)
rj_target[27] = "line 28: conflicting edit"
two_old, two_new = multi_edit(2902, 20, [("r", 3), ("r", 15)])
two_target = list(two_old)
two_target[3], two_target[15] = "conflict a", "conflict b"
one_old = ["alpha", "beta", "gamma", "delta"]
one_patch = "--- a/w.txt\n+++ b/w.txt\n@@ -2,1 +2,1 @@\n-beta\n+BETA\n"
nl_patch = "--- a/w.txt\n+++ b/w.txt\n@@ -3,2 +3,2 @@\n gamma\n-delta\n\\ No newline at end of file\n+DELTA\n"
item("patch_rejects", [
    P(text(rj_target), udiff(text(rj_old), text(rj_new)), rej=True),
    P(text(two_target), udiff(text(two_old), text(two_new), n=2), rej=True),
    P(text(["alpha", "BETA?", "gamma", "delta"]), one_patch, fname="w.txt", rej=True),
    P(text(["alpha", "beta", "gamma", "delta!"], False), nl_patch, fname="w.txt", rej=True),
    P(text(f_t1), udiff(text(f_old), text(f_new), "a/My Settings/options.txt", "b/My Settings/options.txt"), "-F0",
      rej=True),
])

zz = ["zc%d" % i for i in range(1, 16)]
zz_new = list(zz)
zz_new[9:11] = []
zz_new[4:4] = ["new zc"]
zz2 = list(zz_new)
zz2[0:0] = ["head 1", "head 2"]
zz2.insert(14, "near end")
item("patch_zero_context", [
    P(text(zz), "--- a/f\n+++ b/f\n@@ -6,0 +7,2 @@\n+inserted one\n+inserted two\n"),
    P(text(zz), "--- a/f\n+++ b/f\n@@ -3,2 +2,0 @@\n-zc3\n-zc4\n"),
    P(text(zz), udiff(text(zz), text(zz2), n=0)),
    P(text(["k%d" % i for i in range(5)] + ["dup"] + ["m%d" % i for i in range(3)] + ["dup"] + ["n%d" % i for i in range(4)]),
      "--- a/f\n+++ b/f\n@@ -8 +7,0 @@\n-dup\n"),
])

item("patch_create_and_empty", [
    P("", "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,3 @@\n+one\n+two\n+three\n", fname="new.txt"),
    P("one\ntwo\nthree\n", "--- a/old.txt\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-one\n-two\n-three\n", fname="old.txt"),
    P("", "--- a/old.txt\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-one\n-two\n-three\n", "-R", fname="old.txt"),
])

nl_a = ["first", "second", "third"]
item("patch_no_newline", [
    P(text(nl_a, False), "--- a\n+++ b\n@@ -2,2 +2,2 @@\n second\n-third\n\\ No newline at end of file\n+third\n"),
    P(text(nl_a), "--- a\n+++ b\n@@ -2,2 +2,2 @@\n second\n-third\n+third\n\\ No newline at end of file\n"),
    P(text(nl_a, False), "--- a\n+++ b\n@@ -1,3 +1,3 @@\n-first\n+FIRST\n second\n third\n"
                         "\\ No newline at end of file\n"),
    P(text(nl_a, False), "--- a\n+++ b\n@@ -0,0 +1 @@\n+zeroth\n"),
    P(text(["head", "drifted", "b", "c", "d"], False),
      "--- a\n+++ b\n@@ -2,4 +2,4 @@\n a\n b\n c\n-d\n\\ No newline at end of file\n+D\n\\ No newline at end of file\n"),
])

cc_big = uniq_lines(random.Random(3601), 30, "fn")
cc_big[8:8] = ["\x0c"]
cc_big[20:20] = ["\x0c"]
cc_big_new = list(cc_big)
cc_big_new[26] = cc_big[26] + " # tuned"
ls_old, ls_new = multi_edit(3701, 20, [("r", 4), ("i", 12, 1)])
ls_patch = udiff(text(ls_old), text(ls_new), "a/My Documents/notes.txt", "b/My Documents/notes.txt")
rv_old, rv_new = multi_edit(3101, 40, [("i", 3, 2), ("d", 12, 2), ("r", 22), ("i", 33, 1)])
item("patch_reverse", [
    P(text(rv_new), udiff(text(rv_old), text(rv_new)), "-R"),
    P(text(nl_a, False), "--- a\n+++ b\n@@ -2,2 +2,2 @@\n second\n-third\n+third\n\\ No newline at end of file\n",
      "--reverse"),
    P(text(zz_new), "--- a/f\n+++ b/f\n@@ -4,0 +5 @@\n+new zc\n", "-R"),
    P(text(["x %d" % i for i in range(4)] + ["    pass"] + ["y %d" % i for i in range(3)] + ["    pass"] + ["z"] * 2),
      "--- a/f\n+++ b/f\n@@ -6,0 +7 @@\n+    pass\n", "-R"),
    P(text(cc_big_new), udiff(text(cc_big), text(cc_big_new)), "-R", fname="src.py"),
])

op_old, op_new = multi_edit(3201, 36, [("d", 3, 1), ("i", 12, 3), ("r", 24), ("r", 31)])
op_patch = udiff(text(op_old), text(op_new))
op_bad = list(op_old)
op_bad[31] = "line 32: conflicting"
item("patch_output_options", [
    P(text(op_old), op_patch, "-o", "out.txt", outfiles=["file.txt", "out.txt"]),
    P(text(op_bad), op_patch, "-r", "custom.rej", rej=True, rejname="custom.rej"),
    P(text(op_old), op_patch, "-b", outfiles=["file.txt", "file.txt.orig"]),
    P(text(op_bad), op_patch, "--dry-run", outfiles=["file.txt"]),
    P(text(op_bad), op_patch, "-s", "-r", "quiet.rej", rej=True, rejname="quiet.rej"),
    P(text(lim_t1), udiff(text(lim_old), text(lim_new)), "--dry-run", "-F", "0", outfiles=["file.txt"]),
    P(text(ed_end_target), udiff(text(ed), text(ed_end_new)), "-o", "fixed.txt", outfiles=["file.txt", "fixed.txt"]),
    P(text(ls_old), ls_patch, "-p1", "-b", fname="My Documents/notes.txt", operands=["fix.patch"],
      outfiles=["My Documents/notes.txt", "My Documents/notes.txt.orig"]),
])

lb_old, lb_new = multi_edit(3301, 24, [("r", 5), ("i", 15, 1)])
lb_patch = udiff(text(lb_old), text(lb_new), "a/src/app.txt", "b/src/app.txt")
garbage = ("diff --git a/src/app.txt b/src/app.txt\nindex 1a2b3c4..5d6e7f8 100644\n" +
           lb_patch.replace("--- a/src/app.txt\n", "--- a/src/app.txt\t2024-01-01 10:00:00\n"))
item("patch_file_from_label", [
    P(text(lb_old), lb_patch, "-p1", fname="src/app.txt", operands=["fix.patch"]),
    P(text(lb_old), garbage, "-p", "1", fname="src/app.txt", operands=["fix.patch"]),
    P("", "--- /dev/null\n+++ b/docs/new.md\n@@ -0,0 +1,2 @@\n+# New\n+text\n", "-p1", fname="docs/new.md",
      operands=["fix.patch"]),
    P("bye\n", "--- old/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n", "-p1", fname="gone.txt",
      operands=["fix.patch"]),
    P("a\nb\n", "--- a/docs/user guide.txt\n+++ b/docs/user guide.txt\n@@ -1,2 +1,2 @@\n a\n-b\n+B\n", "-p1",
      fname="docs/user guide.txt", operands=["fix.patch"]),
])

mal_t = "a\nb\nc\nd\ne\n"
item("patch_malformed", [
    P(mal_t, "just some text\nno header here\n"),
    P(mal_t, "--- a\n@@ -1 +1 @@\n-a\n+A\n"),
    P(mal_t, "--- a\n+++ b\n@@ -1 +1 @\n-a\n+A\n"),
    P(mal_t, "--- a\n+++ b\n@@ -2,3 +2,3 @@\n b\n-c\n+C\n"),
    P(mal_t, "--- a\n+++ b\n@@ -1,2 +1,2 @@\n a\n*b\n"),
    P(mal_t, "--- a\n+++ b\n@@ -3 +3 @@\n-c\n+C\n@@ -2 +2 @@\n-b\n+B\n"),
    P(mal_t, "--- a\n+++ b\n@@ -1,2 +1,2 @@\n a\n b\n"),
    P(mal_t, "--- a\n+++ b\n@@ -0,1 +1 @@\n-a\n+A\n"),
])


rb_fill = ["note %d: %s" % (i, w) for i, w in enumerate(WORDS[:30])]
rb_block = ["[server]", "host = localhost", "port = 8080", ""]
rb1 = rb_fill[:4] + rb_block + rb_fill[4:10] + rb_block + rb_fill[10:]
rb1_patch = "--- a/c.ini\n+++ b/c.ini\n@@ -10,4 +10,4 @@\n [server]\n host = localhost\n-port = 8080\n+port = 9090\n \n"
rb2 = rb_fill[:6] + ["    return None"] + rb_fill[6:9] + ["    return None"] + rb_fill[9:]
rb2_patch = "--- a/m.py\n+++ b/m.py\n@@ -9 +8,0 @@\n-    return None\n"
rb3 = rb_fill[:3] + rb_block + rb_fill[3:5] + rb_block + rb_fill[5:14] + rb_block + rb_fill[14:16] + rb_block + rb_fill[16:]
rb3_patch = ("--- a/c.ini\n+++ b/c.ini\n@@ -1,2 +1,3 @@\n note 0: alpha\n+note 0b: extra\n note 1: bravo\n"
             "@@ -26,4 +27,4 @@\n [server]\n-host = localhost\n+host = example.org\n port = 8080\n \n")
item("patch_repeated_blocks", [
    P(text(rb1), rb1_patch, fname="c.ini"),
    P(text(rb2), rb2_patch, fname="m.py"),
    P(text(rb3), rb3_patch, fname="c.ini"),
])

cc_old = ["def first():", "    return 1", "", "\x0c", "", "def second():", "    return 2", "", "\x0c", "",
          "def third():", "    return 3"]
cc_new = list(cc_old)
cc_new[6] = "    return 22"
crlf_old = [x + "\r" for x in ["[core]", "editor = vim", "pager = less", "[user]", "name = Ann"]]
crlf_new = list(crlf_old)
crlf_new[4] = "name = Bob\r"
item("diff_control_chars", [
    D(text(cc_old), text(cc_new)),
    D(text(["title", "a\rb", "c", "d"]), text(["title", "a\rb", "c", "D"])),
    D(text(crlf_old), text(crlf_new)),
    D(text(["x\x0by", "1", "2", "3", "4", "5", "6", "7"]), text(["x\x0by", "1", "2", "3", "4", "5", "6", "seven"]), "-U", "1"),
])

cc_ctx_new = list(cc_old)
cc_ctx_new[5] = "def second(x):"
item("patch_control_chars", [
    P(text(cc_big), udiff(text(cc_big), text(cc_big_new)), fname="src.py"),
    P(text(cc_old), udiff(text(cc_old), text(cc_ctx_new)), fname="src.py"),
    P(text(crlf_old), udiff(text(crlf_old), text(crlf_new)), fname="config"),
    P(text(["a\rb", "c", "d", "e"]), "--- a/x\n+++ b/x\n@@ -2,3 +2,3 @@\n c\n-d\n+D\n e\n", fname="x.txt"),
])

ls_bad = list(ls_old)
ls_bad[4] = "line 5: edited elsewhere"
item("patch_labels_with_spaces", [
    P(text(ls_old), ls_patch, "-p1", fname="My Documents/notes.txt", operands=["fix.patch"]),
    P(text(ls_bad), ls_patch, "-p", "1", fname="My Documents/notes.txt", operands=["fix.patch"], rej=True),
    P("", "--- /dev/null\n+++ b/release notes.md\n@@ -0,0 +1 @@\n+# Release notes\n", "-p1",
      fname="release notes.md", operands=["fix.patch"]),
    S(ls_patch),
])

sf_old = uniq_lines(random.Random(3801), 26, "key")
sf_new = list(sf_old)
sf_new[12] = "key 13: strict"
sf_fz = list(sf_old)
sf_fz[9] = "key 10: drifted"
sf_off = ["# moved"] * 2 + sf_old
sf2_old, sf2_new = multi_edit(3802, 30, [("r", 5), ("r", 20)])
sf2_t = list(sf2_old)
sf2_t[17] = "line 18: drifted"
item("patch_strict_fuzz", [
    P(text(sf_old), udiff(text(sf_old), text(sf_new)), "-F", "0"),
    P(text(sf_fz), udiff(text(sf_old), text(sf_new)), "-F", "0", rej=True),
    P(text(sf_off), udiff(text(sf_old), text(sf_new)), "-F0"),
    P(text(sf2_t), udiff(text(sf2_old), text(sf2_new)), "--fuzz=0", rej=True),
])


def section(old, new, name, n=3):
    return udiff(text(old), text(new), "a/" + name, "b/" + name, n=n)


mf_a_old, mf_a_new = multi_edit(3901, 30, [("i", 4, 2), ("r", 20)])
mf_b_old, mf_b_new = multi_edit(3902, 24, [("r", 6), ("d", 15, 1)])
mf_c_old, mf_c_new = multi_edit(3903, 20, [("r", 10)])
mf_patch = ("diff --git a/a.txt b/a.txt\n" + section(mf_a_old, mf_a_new, "a.txt") + "diff --git a/b.txt b/b.txt\n" +
            section(mf_b_old, mf_b_new, "b.txt") + "diff --git a/c.txt b/c.txt\n" + section(mf_c_old, mf_c_new, "c.txt"))
mf_a_off = ["# license line %d" % i for i in range(3)] + mf_a_old
mf_b_bad = list(mf_b_old)
mf_b_bad[6] = "line 7: local change"


def MF(a, b, c, *opts, **kw):
    files = {"a.txt": text(a), "b.txt": text(b), "c.txt": text(c), "fix.patch": kw.get("patch", mf_patch)}
    outs = ["a.txt", "b.txt", "c.txt"] + kw.get("more", [])
    return {"files": files, "argv": ["patch"] + list(opts) + ["-p1", "fix.patch"], "outfiles": outs}


item("patch_multi_file", [
    MF(mf_a_old, mf_b_old, mf_c_old),
    MF(mf_a_off, mf_b_old, mf_c_old),
    MF(mf_a_off, ["top"] + mf_b_old, mf_c_old),
])

item("patch_multi_file_rejects", [
    MF(mf_a_old, mf_b_bad, mf_c_old, more=["b.txt.rej"]),
    MF(mf_a_off, mf_b_bad, mf_c_old, more=["b.txt.rej"]),
    MF(mf_a_old, mf_b_bad, mf_c_old, "--dry-run"),
])

st_old, st_new = multi_edit(3501, 30, [("i", 3, 2), ("d", 12, 2), ("r", 22)])
item("stat_summary", [
    S(udiff(text(st_old), text(st_new))),
    S("--- a/f\n+++ b/f\n@@ -6,0 +7,2 @@\n+x\n+y\n@@ -9,1 +11,0 @@\n-z\n"),
    S("--- a/f\n+++ b/f\n@@ -6,0 +7,2 @@\n+x\n+y\n@@ -9,1 +11,0 @@\n-z\n", "-R"),
    S("--- a/w\n+++ b/w\n@@ -3,1 +3,1 @@\n-c\n+C\n"),
    S("--- old/Read Me.txt\t2024-05-01\n+++ new/Read Me.txt\n@@ -1 +1 @@\n-x\n+y\n"),
    S(mf_patch),
])


def add_case(name, case):
    for n, cases in ITEMS:
        if n == name:
            cases.append(case)
            return
    raise KeyError(name)


add_case("patch_reverse", MF(mf_a_new, ["top"] + mf_b_new, mf_c_new, "-R"))
add_case("patch_output_options", MF(mf_a_off, mf_b_bad, mf_c_old, "-b",
                                    more=["a.txt.orig", "b.txt.orig", "c.txt.orig", "b.txt.rej"]))
add_case("patch_strict_fuzz", MF(mf_a_old, mf_b_bad, ["x"] + mf_c_old, "-F", "0", more=["b.txt.rej"]))
add_case("patch_labels_with_spaces", {"files": {"my a.txt": text(mf_a_off), "my c.txt": text(mf_c_old),
         "fix.patch": section(mf_a_old, mf_a_new, "my a.txt") + section(mf_c_old, mf_c_new, "my c.txt")},
         "argv": ["patch", "-p1", "fix.patch"], "outfiles": ["my a.txt", "my c.txt"]})
