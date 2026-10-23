"""pool-t-patch-port check: the patch's behaviour in the current src/ layout, the newer --lines and char_count
kept, no leftovers from a failed patch run."""
import json
import os
import re
import subprocess
import sys

WS = os.getcwd()
SCRATCH = os.environ["S1_SCRATCH"]
ENV = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": SCRATCH, "LC_ALL": "C", "PYTHONHASHSEED": "0",
       "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": os.path.join(WS, "src")}
TEXT = "The cat's hat, the CAT's mat.\nA hat; a cat!\n\nend-of-file 42\n"


def py(code):
    p = subprocess.run([sys.executable, "-S", "-s", "-B", "-c", code], cwd=SCRATCH, env=ENV,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
    return p.returncode, p.stdout.decode("utf-8", "replace")


def cli(args):
    p = subprocess.run([sys.executable, "-S", "-s", "-B", "-m", "textstats.cli"] + args, cwd=SCRATCH, env=ENV,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
    return p.returncode, p.stdout.decode("utf-8", "replace")


def say(name, ok):
    print("%s %s" % ("PASS" if ok else "FAIL", name))


def main():
    path = os.path.join(SCRATCH, "in.txt")
    with open(path, "w") as f:
        f.write(TEXT)
    ws = [w.lower() for w in re.findall(r"[A-Za-z']+", TEXT)]
    code, out = py("import json; from textstats.core import words; print(json.dumps(words(%r)))" % TEXT)
    say("words_lowercase", code == 0 and out.strip() == json.dumps(ws))
    code, out = py("from textstats.core import unique_words; print(unique_words(%r))" % TEXT)
    say("unique_words", code == 0 and out.strip() == str(len(set(ws))))
    code, out = py("from textstats.core import char_count, line_count; print(char_count('ab\\ncd'), line_count('a\\nb\\n'))")
    say("newer_helpers_kept", code == 0 and out.strip() == "4 2")
    say("cli_default", cli([path]) == (0, "words: %d\n" % len(ws)))
    say("cli_unique", cli([path, "--unique"]) == (0, "words: %d\nunique: %d\n" % (len(ws), len(set(ws)))))
    say("cli_lines_and_unique", cli(["--unique", path, "--lines"]) ==
        (0, "words: %d\nlines: %d\nunique: %d\n" % (len(ws), len(TEXT.splitlines()), len(set(ws)))))
    left = []
    for dp, dn, fn in os.walk(WS):
        dn[:] = [x for x in dn if x != ".git"]
        left += [os.path.join(dp, x) for x in fn if x.endswith((".rej", ".orig"))]
    old = os.path.exists(os.path.join(WS, "lib"))
    say("no_leftovers", not left and not old)


main()
