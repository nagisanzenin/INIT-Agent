#!/usr/bin/env python3
"""failclass: the $0 failure-class script of docs/m7-design.md section 3.7 step 2 (the DEMO row of 3.8).

It classifies the incumbent's FAILING visible-pool runs into exactly one of five classes, from logs
and records that already exist (no model call, no network, nothing it reads is ever executed):

    stopped_tests_failing  path_confusion  cap_hit_exploring  edit_tool_mismatch  other

and names the m7 3.7 candidate for the largest class (stopped_tests_failing -> "verify-on-finish",
path_confusion -> "workspace-map", anything else -> null).

Run it as   /usr/bin/python3 -I scripts/m7/failclass.py [--home H] [--artifacts D] [--manifest M] [--json]
(Python 3.9 stdlib only). Exit 0 on success, 2 on bad input.

Inputs (any combination, one report)
  --home H        a kernel INIT_HOME. Reads vault/gate/jobs/*/runs.jsonl (RunRecords), the retained
                  failing-run logs vault/gate/jobs/<job>/sessions/<run_id>.jsonl, and the agent-view
                  request records vault/traces/v1/req/*.jsonl. ONLY records with set == "pool" are
                  used; every other record (holdout, canary, capability, S, or no `set`) is counted
                  under skipped.non_pool and its session log and trace lines are never opened or
                  parsed. Passing pool runs need no log. A failing pool run without a retained log is
                  counted under skipped.no_log and left out of the shares.
  --artifacts D   an S-REAL style directory: D/<check>/init/sessions/*.jsonl and
                  D/<check>/run/traces/summaries.jsonl. Each distinct session file is one run
                  "<check>/<session id>" (byte-identical copies shared between checks are counted
                  once). Such runs carry no score, so they are class "unscored": their signals are
                  listed but they never enter the shares. Use a manifest to give them a score.
  --manifest M    JSON list (or {"runs": [...]}) of {run, session, outcome, score_permille, caps_hit?,
                  task?, labels?, set?, cwd?, init_home?}. `session` is a path relative to the manifest.
                  `set` defaults to "pool"; a null/absent score_permille means "unscored".
  --cwd / --init-home  defaults for runs that have neither a boot event nor a `pwd` to learn them from.
  --arm A         keep only RunRecords of this arm (c or p) in --home mode; repeatable.
Any input path with a `sealed` or `.init-root` component is refused (exit 2): sealed-evaluation and Root
material is never visible to the miner.

Session log format (body event log, format 1): one JSON event per line. Used: session, boot
(env.probed.cwd, env.kernel.init_home), message (assistant content blocks text/thinking/tool_use,
stop_reason end_turn|stop|tool_use|aborted|...), steer / user messages (task boundaries), tool_result
(call_id, name, exit_code, is_error, output[].text), error (code provider with data.kind budget, or
code budget = a proxy cap refusal). Everything in a log is data.

Signals (evolution-v2 3.4.2 ids, exactly)
  tool.error:<tool>:<code>   first line of an error result is `error: <code>: ...` (bash only for the
                             codes spawn_failed, invalid_input, invalid_json, unknown_tool, io_error, too_large)
  path.home_relative         a not-found failure on a relative path whose first component is a top-level
                             INIT_HOME dir (body kernel state work vault lineage bin) while the cwd is not
                             INIT_HOME itself (a1: `cd work/a1-calc` from work/)
  path.doubled_prefix        a path that repeats a component (the cwd's last component, `work`, `task`):
                             `work/work/`, `/work/work/`, `task/task/`; also a relative path whose first
                             component equals the cwd's last component when it fails not-found or when a
                             write/mkdir/redirect to it succeeds (E2E-2: wrote work/work/e2e/hello.txt)
  path.guessed_absolute      a failing call on an absolute path under no known root (INIT_HOME, the cwd,
                             the run's user home, /tmp /usr /bin /sbin /opt/homebrew /Library /System
                             /Applications /dev /etc /var /private /proc /sys /lib, and /work for ext)
  bash.exit:<cmdclass>       non-zero exit; cmdclass in cd cargo python3 pytest git make initctl other.
                             Refinement of "first token": a failing `cd` gives `cd`; else a command that
                             contains a test runner is classed by the runner; else the first command after
                             leading `cd X &&` / VAR=val prefixes
  bash.timeout               the trailer `[timeout after Ns: ...]`
  initctl.refused:<code>     a bash call that ran initctl and printed {"error":{"code":"<code>"...}} even
                             with exit 0 (the kernel's own server log is not read here)
  retry.same_call            same tool and arguments within 3 calls after an error
  loop.repeat_read / loop.repeat_cmd   same path read / same command run 3 times, no write in between
  cap.hit:<cap>              an error event (code budget, or code provider with data.kind budget), the
                             RunRecord's caps_hit / outcome cost_capped, or a 429 request record with a cap
  finish.untested            final answer after the last write/edit with no test command run afterwards
  finish.tests_failing       final answer when the last test command failed
  task.fail                  RunRecord score_permille < 1000
Skipped on purpose (need data this script is not given): owner.* (interrupt, steer, followup),
cost.spike, turns.spike, cache.break, bash.bg_killed.

Expected failures are excluded: a failing test command is dropped (no bash.exit signal, no
finish.tests_failing) when, after an edit, the same test command (same runner and arguments) passes.
A test command is pytest, python -m pytest|unittest, cargo test|nextest, make test|check, npm|yarn|pnpm
test, go test, tox, ctest, or a test script (`bash run_tests.sh`, `./check.sh`, `python3 test_x.py`) at the
start of a statement; its outcome comes from failure/success markers in the output (so `pytest | tail` is
still judged) and falls back to the exit code.

Decision list (first match wins; applied to runs with score_permille < 1000)
  1. cap_hit_exploring    a cap signal exists (cap.hit:*, outcome cost_capped, caps_hit set) AND the run
                          was still exploring: at least one tool call before the cap and either no file
                          change before it (no successful write/edit, no file-changing bash) or at least
                          2/3 of its last 5 tool calls were read-only (read; bash made only of ls cat find
                          grep sed -n head tail pwd wc stat jq diff git status|log|diff|show ... without
                          redirects to files). A cap with no tool call at all is `other`.
  2. stopped_tests_failing  a finish.tests_failing signal.
  3. path_confusion       a FATAL path.* signal (no later successful call on the intended path: the
                          resolved path with the prefix fixed; for guessed_absolute the last three path
                          components) or at least 2 path.* instances (one per call and kind).
  4. edit_tool_mismatch   a FATAL tool.error edit:{no_match,ambiguous_match,overlapping_edits,no_change,
                          invalid_input}, write:invalid_input, any invalid_json or unknown_tool (fatal =
                          no later successful write/edit of the same file, or later successful call of the
                          same tool; a mutating bash that names the file counts), or at least 2 such errors.
  5. other                else, with a reason: cap_hit_while_editing, cap_hit_before_first_tool_call,
                          timeout, crash, no_ready, infra, finish.untested, no_final_answer, no_signal.
Passing runs (score 1000) get their signals and class "pass".

cwd / INIT_HOME of a run: the first boot event's env (env.probed.cwd, env.kernel.init_home), then the
output of a leading `pwd`, then the manifest/--cwd/--init-home values (an artifacts session without any
of these inherits the last boot of the previous session in its directory); INIT_HOME is derived from a
cwd ending in /work or /work/task if still unknown. The script never touches the file system of the run.

--json prints one line of ICJ v1 JSON (sorted keys, integers only, no booleans; ratios in permille):
  {"v":1,"runs":[{run,task,labels,set,score_permille,outcome,class,reason,signals:[{signal,ref,detail}]}],
   "counts":{failing,pass,unscored,deduped},"totals":{class:n},"share_permille":{class:p},
   "largest":class|null,"candidate":"verify-on-finish"|"workspace-map"|null,"skipped":{non_pool,no_log}}
detail is at most 200 bytes, ref is event:<id>, run:<gate_id>/<run_id> or trace:<req_id>. Same input
gives byte-identical output. The default output is a short table.

Assumptions to re-check against real pool logs: the cap field of an agent-view 429 record (`cap` or
`x_init_cap`, else outcome cap*/budget* gives cap "unknown") is a guess; all shell parsing is static and
heuristic (variable expansion of simple VAR=value, leading `cd`, no globs); `pass` and `unscored` runs are
counted in "counts", never inside the five classes.
"""
from __future__ import annotations

import argparse
import glob
import hashlib
import json
import os
import posixpath
import re
import shlex
import sys

CLASSES = ("stopped_tests_failing", "path_confusion", "cap_hit_exploring", "edit_tool_mismatch", "other")
CANDIDATES = {"stopped_tests_failing": "verify-on-finish", "path_confusion": "workspace-map"}

TOP_DIRS = ("body", "kernel", "state", "work", "vault", "lineage", "bin")
SYSTEM_ROOTS = ("/tmp", "/usr", "/bin", "/sbin", "/opt/homebrew", "/Library", "/System", "/Applications",
                "/dev", "/etc", "/var", "/private", "/proc", "/sys", "/lib", "/lib64")
SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
MAX_LOG_BYTES = 64 << 20

NOTFOUND_RE = re.compile(r"No such file or directory|os error 44|\bnot_found\b|does not exist|"
                         r"cannot find the (?:path|file)|ENOENT", re.I)
DENIED_RE = re.compile(r"permission denied|permission_denied|operation not permitted|read-only file system|"
                       r"os error 13|os error 30", re.I)
ERRLINE_RE = re.compile(r"^error: ([a-z_][a-z0-9_]*): ?(.*)$")
BASH_ERR_CODES = frozenset(["spawn_failed", "invalid_input", "invalid_json", "unknown_tool", "io_error", "too_large"])
EDIT_FATAL = frozenset([("edit", "no_match"), ("edit", "ambiguous_match"), ("edit", "overlapping_edits"),
                        ("edit", "no_change"), ("edit", "invalid_input"), ("write", "invalid_input")])
ANY_FATAL = frozenset(["invalid_json", "unknown_tool"])
WRITE_KEYS = frozenset(["out", "output", "dest", "dst", "to", "write", "target"])


class BadInput(Exception):
    pass


def check_safe_path(path):
    """Refuse sealed-evaluation / Root material (evolution-v2 3.4.1: never read)."""
    real = os.path.realpath(path)
    if any(part in ("sealed", ".init-root") for part in real.split(os.sep)):
        raise BadInput("refusing to read %s: sealed or Root material" % path)


# ----------------------------------------------------------------------------------------------
# small helpers
# ----------------------------------------------------------------------------------------------

def clip(s, n=200):
    s = " ".join(str(s).split())
    b = s.encode("utf-8", "replace")
    if len(b) <= n:
        return b.decode("utf-8")
    return b[:n].decode("utf-8", "ignore")


def text_of(output):
    if isinstance(output, str):
        return output
    parts = []
    if isinstance(output, list):
        for x in output:
            if isinstance(x, dict) and isinstance(x.get("text"), str):
                parts.append(x["text"])
            elif isinstance(x, str):
                parts.append(x)
    return "".join(parts)


def jloads(line):
    try:
        return json.loads(line)
    except (ValueError, RecursionError):
        return None


def permille(num, den):
    if den <= 0:
        return 0
    return (2 * num * 1000 + den) // (2 * den)


def is_int(x):
    return isinstance(x, int) and not isinstance(x, bool)


def read_events(path):
    """The events of a session log (list of dicts, corrupt lines skipped) or None if unreadable."""
    try:
        if os.path.getsize(path) > MAX_LOG_BYTES:
            return None
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            raw = f.read()
    except OSError:
        return None
    events = []
    for n, line in enumerate(raw.split("\n"), 1):
        line = line.strip()
        if not line:
            continue
        e = jloads(line)
        if isinstance(e, dict):
            e["_n"] = n
            events.append(e)
    return events


def resolve(p, cwd):
    if not isinstance(p, str) or not p or "\x00" in p or p.startswith("~"):
        return None
    if p.startswith("/"):
        return posixpath.normpath(p)
    if cwd:
        return posixpath.normpath(posixpath.join(cwd, p))
    return None


def doubled_names(cwd):
    names = set(["work", "task"])
    if cwd:
        b = posixpath.basename(cwd.rstrip("/"))
        if b:
            names.add(b)
    return names


def has_doubled(path, names):
    parts = path.split("/")
    for i in range(len(parts) - 1):
        if parts[i] and parts[i] == parts[i + 1] and parts[i] in names:
            return True
    return False


def collapse_doubled(path, names):
    parts = path.split("/")
    out = []
    i = 0
    done = False
    while i < len(parts):
        if (not done and i + 1 < len(parts) and parts[i] and parts[i] == parts[i + 1]
                and parts[i] in names):
            out.append(parts[i])
            i += 2
            done = True
        else:
            out.append(parts[i])
            i += 1
    return "/".join(out)


def tail(path, n):
    parts = [p for p in path.split("/") if p]
    return "/".join(parts[-n:])


def user_home(ih):
    if not ih:
        return None
    m = re.match(r"^(/Users/[^/]+|/home/[^/]+)(?:/|$)", ih)
    return m.group(1) if m else None


def derive_home(cwd):
    if not cwd:
        return None
    c = cwd.rstrip("/")
    if c.endswith("/work/task"):
        return posixpath.dirname(posixpath.dirname(c))
    if c.endswith("/work"):
        return posixpath.dirname(c)
    return None


def under(path, root):
    root = root.rstrip("/")
    return path == root or path.startswith(root + "/")


# ----------------------------------------------------------------------------------------------
# bash command analysis (all static, nothing is executed)
# ----------------------------------------------------------------------------------------------

HEREDOC_RE = re.compile(r"(?<!<)<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1")
QUOTED_RE = re.compile(r'"(?:[^"\\]|\\.)*"|\'[^\']*\'', re.S)
REDIR_RE = re.compile(r"(?<![<>&=\-\d])(\d?)(>>?|&>)(?!&)\s*([^\s;&|<>()]+)")
STMT_SPLIT_RE = re.compile(r"&&|\|\||;|\n|\||(?<!>)&(?![>&])|[()]")
ASSIGN_WORD_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
TEST_PREFIX = r"(?:(?:time|exec|sudo|env)\s+|timeout\s+\S+\s+|(?:uv|poetry|pipenv)\s+run\s+)*"
TEST_START_RE = re.compile(
    "^" + TEST_PREFIX + r"(?:(?:python[0-9.]*|py)\s+-m\s+(?:pytest|unittest)|pytest|py\.test|"
    r"cargo\s+(?:test|nextest)|make\s+(?:test|check)|npm\s+(?:run\s+)?test|yarn\s+test|pnpm\s+(?:run\s+)?test|"
    r"go\s+test|tox|ctest|"
    r"(?:bash|sh|zsh)\s+(?:\./)?(?:[\w./-]*/)?(?:run[_-]?)?(?:tests?|check)[\w-]*\.sh|"
    r"\./(?:[\w-]+/)*(?:run[_-]?)?(?:tests?|check)[\w-]*(?:\.sh)?|"
    r"python[0-9.]*\s+(?:[\w./-]*/)?(?:run[_-]?)?tests?[\w-]*\.py)(?:\s|$)")
PY_WRITE_RE = re.compile(r"write_text\(|\.write\(|\bopen\([^)]*(?:,|mode=)\s*['\"](?:[wax]|r\+)b?\+?['\"]|"
                         r"os\.(?:remove|rename|replace|makedirs|mkdir|unlink)\(|shutil\.(?:copy|move|rmtree)|"
                         r"writeFileSync|File\.write|fs\.write")
MUT_CMDS = frozenset(["tee", "mkdir", "touch", "rm", "rmdir", "mv", "cp", "ln", "install", "chmod", "chown",
                      "truncate", "patch", "dd", "rsync"])
GIT_MUT = frozenset(["add", "commit", "apply", "checkout", "reset", "restore", "stash", "merge", "rebase",
                     "cherry-pick", "rm", "mv", "clean", "pull", "worktree", "am", "switch", "revert", "tag"])
GIT_RO = frozenset(["status", "log", "diff", "show", "branch", "rev-parse", "ls-files", "remote", "describe",
                    "blame", "grep", "config", "rev-list", "shortlog", "ls-tree", "cat-file"])
RO_CMDS = frozenset(["ls", "cat", "find", "grep", "egrep", "fgrep", "rg", "ag", "head", "tail", "pwd", "wc",
                     "stat", "file", "which", "whereis", "type", "echo", "printf", "jq", "diff", "cmp", "tree",
                     "du", "df", "env", "printenv", "true", "false", "test", "[", "cd", "sort", "uniq", "cut",
                     "tr", "column", "basename", "dirname", "realpath", "readlink", "date", "uname", "id",
                     "whoami", "hostname", "od", "xxd", "hexdump", "strings", "nl", "less", "more", "awk",
                     "sleep", ":", "set", "export", "ll", "tac", "rev", "fold", "expand", "md5sum", "shasum",
                     "sha256sum", "lsof", "ps", "git", "sed"])
PYLIKE = ("python", "node", "perl", "ruby")
_BASH_CACHE = {}


def blank_heredocs(cmd):
    """Blank heredoc bodies with spaces (offsets preserved). Returns (blanked, [(op_offset, body, body_offset)])."""
    out = []
    bodies = []
    offset = 0
    pending = []  # (tag, offset of the << operator)
    cur = None
    for line in cmd.split("\n"):
        if pending:
            tag, hstart = pending[0]
            if cur is None:
                cur = [offset, []]
            if line.strip() == tag:
                bodies.append((hstart, "\n".join(cur[1]), cur[0]))
                pending.pop(0)
                cur = None
            else:
                cur[1].append(line)
            out.append(" " * len(line))
        else:
            for m in HEREDOC_RE.finditer(line):
                pending.append((m.group(2), offset + m.start()))
            out.append(line)
        offset += len(line) + 1
    if pending and cur is not None:
        bodies.append((pending[0][1], "\n".join(cur[1]), cur[0]))
    return "\n".join(out), bodies


def blank_quotes(s):
    def rep(m):
        t = m.group(0)
        return t[0] + "".join(c if c == "\n" else "_" for c in t[1:-1]) + t[-1]
    return QUOTED_RE.sub(rep, s)


def first_word(stmt):
    words = stmt.split()
    i = 0
    while i < len(words) and ASSIGN_WORD_RE.match(words[i]):
        i += 1
    return words[i:] if i < len(words) else []


def norm_test_key(stmt):
    s = re.sub(r"\d*>&\d+|\d*>>?\s*\S+|&>\s*\S+", " ", stmt)
    words = [t for t in s.split() if not ASSIGN_WORD_RE.match(t)]
    s = re.sub("^" + TEST_PREFIX, "", " ".join(words))
    toks = [t for t in s.split() if not t.startswith("-")]
    if toks and re.match(r"^(?:.*/)?(python[0-9.]*|py)$", toks[0]):
        toks[0] = "python3"
    return " ".join(toks)


def bash_info(cmd):
    """Static facts about a bash command string: mutation, read-only, test statement, class helpers."""
    if cmd in _BASH_CACHE:
        return _BASH_CACHE[cmd]
    blanked, bodies = blank_heredocs(cmd)
    scan = blank_quotes(blanked)
    stmts = []  # (start, scan_text, blanked_text)
    last = 0
    for m in STMT_SPLIT_RE.finditer(scan):
        stmts.append((last, scan[last:m.start()], blanked[last:m.start()]))
        last = m.end()
    stmts.append((last, scan[last:], blanked[last:]))
    stmts = [s for s in stmts if s[1].strip()]
    mut_pos = None
    ro = True
    test = None
    names = []
    for start, st, raw in stmts:
        words = first_word(st)
        if not words:
            continue
        name = posixpath.basename(words[0])
        names.append(name)
        rest = words[1:]
        mutating = False
        if name in MUT_CMDS:
            mutating = True
        elif name == "sed" and any((w.startswith("-") and not w.startswith("--") and "i" in w[1:])
                                   or w == "--in-place" for w in rest):
            mutating = True
        elif name == "perl" and any(w.startswith("-") and "i" in w[1:] for w in rest):
            mutating = True
        elif name == "git":
            sub = None
            j = 0
            while j < len(rest):
                if rest[j] in ("-C", "-c"):
                    j += 2
                    continue
                if rest[j].startswith("-"):
                    j += 1
                    continue
                sub = rest[j]
                break
            if sub in GIT_MUT:
                mutating = True
            elif sub not in GIT_RO:
                ro = False
        elif name in ("pip", "pip3", "npm", "yarn", "pnpm") and ("install" in rest or "add" in rest):
            mutating = True
        elif name == "cargo" and any(w in ("fmt", "fix", "add", "remove", "update") for w in rest[:1]):
            mutating = True
        if mutating:
            ro = False
            mut_pos = start if mut_pos is None else min(mut_pos, start)
        if name not in RO_CMDS:
            ro = False
        if name == "sed" and mutating:
            ro = False
        if TEST_START_RE.match(" ".join(words)):
            test = {"pos": start, "key": norm_test_key(raw.strip()), "stmt": " ".join(raw.split())}
    for m in REDIR_RE.finditer(scan):
        target = m.group(3)
        if target in ("/dev/null", "/dev/stderr", "/dev/stdout", "/dev/tty"):
            continue
        ro = False
        mut_pos = m.start() if mut_pos is None else min(mut_pos, m.start())
    for hstart, body, _bstart in bodies:
        # which interpreter owns the heredoc: the statement that contains the `<<`
        owner = None
        for start, st, _raw in stmts:
            if start <= hstart:
                owner = first_word(st)
        if owner and posixpath.basename(owner[0]).startswith(PYLIKE) and PY_WRITE_RE.search(body):
            ro = False
            mut_pos = hstart if mut_pos is None else min(mut_pos, hstart)
        elif owner and posixpath.basename(owner[0]) not in RO_CMDS:
            ro = False
    if any(n.startswith(PYLIKE) for n in names):
        ro = False
        if PY_WRITE_RE.search(blanked) and not bodies:
            mut_pos = 0 if mut_pos is None else mut_pos
    if test is not None:
        ro = False
    info = {"mut_pos": mut_pos, "ro": ro and bool(stmts), "test": test, "names": names, "stmts": stmts}
    _BASH_CACHE[cmd] = info
    return info


def expand_vars(cmd, env):
    """Textual expansion of simple VAR=value assignments and $VAR / ${VAR} / $(pwd) uses."""
    env = dict(env)
    assign = re.compile(r"(?:^|[;&|(\n]\s*|\s)(?:export\s+|local\s+)?([A-Za-z_][A-Za-z0-9_]*)="
                        r"(\"[^\"\n]*\"|'[^'\n]*'|[^\s;&|()\"']*)")

    def sub(s):
        s = re.sub(r"\$\{(\w+)\}|\$(\w+)", lambda m: env.get(m.group(1) or m.group(2), m.group(0)), s)
        if "PWD" in env:
            s = s.replace("$(pwd)", env["PWD"]).replace("`pwd`", env["PWD"])
        return s

    for m in assign.finditer(cmd):
        val = m.group(2)
        if len(val) >= 2 and val[0] in "\"'" and val[-1] == val[0]:
            val = val[1:-1]
        env[m.group(1)] = sub(val)
    return sub(cmd)


def pathy(a):
    if not a or len(a) > 400 or re.search(r"[$*?\[\]{}|<>`\n\t]", a):
        return False
    if " " in a and not a.startswith("/"):
        return False
    if a.startswith("~") or re.match(r"^[a-z]+://", a):
        return False
    if "/" in a:
        return True
    return bool(re.search(r"\.[A-Za-z0-9]{1,6}$", a)) and not re.fullmatch(r"[\d.]+", a)


def bash_uses(cmd, cwd, ih, text):
    """[(literal, resolved or None, kind)] path uses of a bash command; kind in read|write|cd."""
    blanked, _bodies = blank_heredocs(cmd)
    env = {}
    if ih:
        env["INIT_HOME"] = ih
    if cwd:
        env["PWD"] = cwd
    expanded = expand_vars(blanked.replace("\\\n", " "), env).replace("\n", " ; ")
    try:
        lex = shlex.shlex(expanded, posix=True, punctuation_chars=True)
        lex.whitespace_split = True
        lex.commenters = ""
        toks = list(lex)
    except ValueError:
        toks = expanded.split()
    stmts = []
    cur = {"words": [], "w": [], "r": []}
    i = 0
    while i < len(toks):
        t = toks[i]
        if re.fullmatch(r"[;&|()]+", t):
            if cur["words"] or cur["w"] or cur["r"]:
                stmts.append(cur)
            cur = {"words": [], "w": [], "r": []}
        elif re.fullmatch(r"[<>&|;()]+", t) and ("<" in t or ">" in t):
            if cur["words"] and re.fullmatch(r"\d+", cur["words"][-1]):
                cur["words"].pop()
            target = toks[i + 1] if i + 1 < len(toks) else None
            if target is not None:
                if t in (">", ">>", "&>", ">|", "&>>"):
                    cur["w"].append(target)
                elif t == "<":
                    cur["r"].append(target)
                i += 1
        else:
            cur["words"].append(t)
        i += 1
    if cur["words"] or cur["w"] or cur["r"]:
        stmts.append(cur)
    out = []
    here = cwd
    for st in stmts:
        words = list(st["words"])
        while words and ASSIGN_WORD_RE.match(words[0]):
            words.pop(0)
        if words:
            name = posixpath.basename(words[0])
            args = [a for a in words[1:] if not a.startswith("-")]
            if name == "cd":
                if args:
                    tgt = args[0]
                    out.append((tgt, resolve(tgt, here), "cd"))
                    if ("cd: %s" % tgt) in text or ("cd: %s:" % tgt) in text:
                        break
                    here = resolve(tgt, here)
                continue
            if name in ("echo", "printf"):
                args = []
            elif name in ("grep", "egrep", "fgrep", "rg", "ag", "sed", "awk") and args:
                args = args[1:]
            kind = "write" if name in MUT_CMDS else "read"
            for a in args:
                if pathy(a):
                    out.append((a, resolve(a, here), kind))
        for w in st["w"]:
            if w != "/dev/null" and pathy(w):
                out.append((w, resolve(w, here), "write"))
        for r in st["r"]:
            if pathy(r):
                out.append((r, resolve(r, here), "read"))
    return out


def walk_strings(o, key=""):
    if isinstance(o, str):
        yield key, o
    elif isinstance(o, dict):
        for k in sorted(o):
            for x in walk_strings(o[k], str(k)):
                yield x
    elif isinstance(o, list):
        for v in o:
            for x in walk_strings(v, key):
                yield x


# ----------------------------------------------------------------------------------------------
# test outcome detection
# ----------------------------------------------------------------------------------------------

FAIL_MARK = re.compile(
    r"^FAILED \(|^(?:ERROR|FAIL): \S+ \(|^FAILED |^ERROR collecting|^=+ .*\b(?:failed|error)\b.* =+$|\b[1-9]\d* failed\b|"
    r"\b[1-9]\d* errors?\b|test result: FAILED|error: could not compile|^error(?:\[E\d+\])?:|error: test failed|"
    r"^make: \*\*\*|npm ERR!|Tests:\s+[1-9]\d* failed|^--- FAIL|^FAIL\s", re.M)
PASS_MARK = re.compile(r"^OK\b|\b\d+ passed\b|test result: ok\.|^ok\s|Tests:\s+\d+ passed|^Ran \d+ tests? in .*\n+OK", re.M)


def test_outcome(text, exit_code):
    if FAIL_MARK.search(text):
        return "fail"
    if exit_code == 0:
        return "pass"
    if exit_code is None:
        return "unknown"
    return "pass" if PASS_MARK.search(text) else "fail"


# ----------------------------------------------------------------------------------------------
# session parsing
# ----------------------------------------------------------------------------------------------

def event_id(e, idx):
    i = e.get("id")
    if isinstance(i, str) and i:
        return i
    return "L%d" % e.get("_n", idx + 1)


def boot_ctx(e):
    env = e.get("env") if isinstance(e.get("env"), dict) else {}
    kernel = env.get("kernel") if isinstance(env.get("kernel"), dict) else {}
    probed = env.get("probed") if isinstance(env.get("probed"), dict) else {}
    facts = e.get("facts") if isinstance(e.get("facts"), dict) else {}
    cwd = probed.get("cwd") or facts.get("cwd")
    ih = kernel.get("init_home")
    if not ih:
        fk = facts.get("kernel") if isinstance(facts.get("kernel"), dict) else {}
        fe = fk.get("env") if isinstance(fk.get("env"), dict) else {}
        ih = fe.get("init_home")
    return (cwd if isinstance(cwd, str) and cwd.startswith("/") else None,
            ih if isinstance(ih, str) and ih.startswith("/") else None)


def parse_session(events):
    boots = []
    calls = []
    pending = {}
    finals = []
    errs = []
    seg = 0
    for idx, e in enumerate(events):
        t = e.get("type")
        eid = event_id(e, idx)
        if t == "boot":
            boots.append((idx,) + boot_ctx(e))
        elif t == "steer":
            c = e.get("content")
            if text_of(c if isinstance(c, list) else [{"text": c}] if isinstance(c, str) else []).strip():
                seg += 1
        elif t == "message":
            role = e.get("role")
            content = e.get("content")
            if isinstance(content, str):
                content = [{"type": "text", "text": content}]
            if not isinstance(content, list):
                content = []
            blocks = [b for b in content if isinstance(b, dict)]
            if role == "user":
                if any(b.get("type") == "text" and str(b.get("text", "")).strip() for b in blocks):
                    seg += 1
            elif role == "assistant":
                uses = [b for b in blocks if b.get("type") == "tool_use"]
                for b in uses:
                    inp = b.get("input") if isinstance(b.get("input"), dict) else {}
                    cid = b.get("call_id")
                    pending[cid if isinstance(cid, str) else None] = {"name": str(b.get("name", "")), "input": inp,
                                                                      "mid": eid}
                txt = "".join(str(b.get("text", "")) for b in blocks if b.get("type") == "text")
                if e.get("stop_reason") in ("end_turn", "stop") and not uses and txt.strip():
                    finals.append({"seg": seg, "idx": idx, "eid": eid, "text": txt})
        elif t == "tool_result":
            cid = e.get("call_id")
            cid = cid if isinstance(cid, str) else None
            p = pending.pop(cid, None) or {"name": str(e.get("name", "")), "input": {}, "mid": None}
            text = text_of(e.get("output"))
            ec = e.get("exit_code")
            ec = ec if is_int(ec) else None
            err = bool(e.get("is_error"))
            calls.append({"seq": len(calls), "name": p["name"] or str(e.get("name", "")), "input": p["input"],
                          "call_id": cid, "mid": p["mid"], "rid": eid, "idx": idx, "err": err, "exit": ec,
                          "text": text, "seg": seg, "failed": err or (ec not in (None, 0))})
        elif t == "error":
            data = e.get("data") if isinstance(e.get("data"), dict) else {}
            errs.append({"idx": idx, "eid": eid, "code": str(e.get("code", "")), "data": data,
                         "message": str(e.get("message", ""))})
    return {"boots": boots, "calls": calls, "finals": finals, "errors": errs}


def pwd_ctx(calls):
    """cwd from the output of a bash call whose command starts with `pwd`."""
    for c in calls:
        if c["name"] != "bash" or c["failed"]:
            continue
        cmd = str(c["input"].get("command", ""))
        if re.match(r"^\s*pwd\s*(?:;|&&|\n|$)", cmd):
            line = c["text"].split("\n", 1)[0].strip()
            if line.startswith("/"):
                return line
    return None


def assign_ctx(parsed, explicit_cwd, explicit_home, inherited):
    """Give every call its cwd / INIT_HOME (boot > pwd > explicit > inherited; home derived from cwd)."""
    boots = parsed["boots"]
    pcwd = pwd_ctx(parsed["calls"])
    first = boots[0][1:] if boots else (None, None)
    for c in parsed["calls"]:
        cur = None
        for b in boots:
            if b[0] < c["idx"]:
                cur = b[1:]
        cur = cur or first
        cwd = cur[0] or pcwd or explicit_cwd or inherited[0]
        ih = cur[1] or explicit_home or inherited[1] or derive_home(cwd)
        c["cwd"] = cwd
        c["ih"] = ih
    return (boots[-1][1] if boots else None, boots[-1][2] if boots else None)


# ----------------------------------------------------------------------------------------------
# signal extraction
# ----------------------------------------------------------------------------------------------

class Sig(object):
    __slots__ = ("sid", "ref", "detail", "fatal", "group")

    def __init__(self, sid, ref, detail, fatal=False, group=""):
        self.sid = sid
        self.ref = ref
        self.detail = clip(detail)
        self.fatal = fatal
        self.group = group

    def out(self):
        return {"detail": self.detail, "ref": self.ref, "signal": self.sid}


def call_uses(c):
    if "_uses" in c:
        return c["_uses"]
    n = c["name"]
    inp = c["input"]
    out = []
    if n in ("read", "write", "edit"):
        p = inp.get("path")
        if isinstance(p, str) and p:
            out.append((p, resolve(p, c["cwd"]), "read" if n == "read" else "write"))
    elif n == "ext":
        for key, s in walk_strings(inp.get("input")):
            if pathy(s):
                out.append((s, resolve(s, c["cwd"]), "write" if key in WRITE_KEYS else "read"))
    elif n == "bash":
        cmd = inp.get("command")
        if isinstance(cmd, str):
            out = bash_uses(cmd, c["cwd"], c["ih"], c["text"])
    c["_uses"] = out
    return out


def cmd_of(c):
    cmd = c["input"].get("command")
    return cmd if isinstance(cmd, str) else ""


def mutates(c):
    n = c["name"]
    if n in ("write", "edit"):
        return not c["failed"]
    if n == "bash":
        return bash_info(cmd_of(c))["mut_pos"] is not None
    if n == "ext":
        return any(k in WRITE_KEYS for k, _s in walk_strings(c["input"].get("input")))
    return False


def readonly(c):
    if c["name"] == "read":
        return True
    if c["name"] == "bash":
        return bash_info(cmd_of(c))["ro"]
    return False


def cmd_class(c):
    cmd = cmd_of(c)
    if re.search(r"\bcd: .*(?:No such file|Not a directory|Permission denied)", c["text"]):
        return "cd"
    info = bash_info(cmd)
    if info["test"]:
        first = first_word(info["test"]["stmt"])
        for w in first:
            b = posixpath.basename(w)
            if b in ("time", "exec", "sudo", "env"):
                continue
            first = b
            break
        else:
            first = ""
        if isinstance(first, str):
            if re.match(r"^(python[0-9.]*|py)$", first):
                return "python3"
            if first in ("pytest", "py.test"):
                return "pytest"
            if first in ("cargo", "make"):
                return first
    for name in info["names"]:
        if name in ("cd", "export", "set", ":", "true"):
            continue
        if re.match(r"^python[0-9.]*$", name):
            return "python3"
        if name in ("cargo", "git", "make", "initctl", "pytest"):
            return name
        if name == "py.test":
            return "pytest"
        return "other"
    return "other"


PATHTOK_RE = re.compile(r"[\w./@+~-]*/[\w./@+~-]*")


def path_tokens(text):
    return PATHTOK_RE.findall(text[:20000])


def path_findings(c):
    """[(signal id, [(literal, intended or None)])] for one call; one entry per (signal id)."""
    cwd, ih = c["cwd"], c["ih"]
    names = doubled_names(cwd)
    failed, text = c["failed"], c["text"]
    notfound = failed and bool(NOTFOUND_RE.search(text))
    denied = failed and bool(DENIED_RE.search(text))
    found = {}

    def add(sid, lit, intended):
        lst = found.setdefault(sid, [])
        if lit not in [x[0] for x in lst]:
            lst.append((lit, intended))

    dbl_tokens = []
    if failed:
        for tok in path_tokens(text):
            if has_doubled(tok, names):
                dbl_tokens.append(tok)
        if dbl_tokens:
            tok = dbl_tokens[0]
            intended = collapse_doubled(tok, names)
            if c["name"] == "ext" and ih and (intended == "/work" or intended.startswith("/work/")):
                intended = ih.rstrip("/") + intended  # the ext sandbox mounts INIT_HOME/work at /work
            add("path.doubled_prefix", tok, intended)
    roots = [r for r in (ih, cwd, user_home(ih)) if r] + list(SYSTEM_ROOTS)
    if c["name"] == "ext":
        roots.append("/work")
    base = posixpath.basename(cwd.rstrip("/")) if cwd else ""
    for lit, ab, kind in call_uses(c):
        is_abs = lit.startswith("/")
        rel = not is_abs and not lit.startswith("~")
        tool_path = c["name"] in ("read", "write", "edit")
        evidence = tool_path or lit in text or bool(ab and ab in text)
        first = lit.split("/", 1)[0]
        if any(lit in tok for tok in dbl_tokens):
            continue  # already explained by the doubled path in the failure output
        lit_dbl = has_doubled(lit, names)
        impl_dbl = bool(rel and cwd and first and first == base)
        if failed and evidence and lit_dbl:
            add("path.doubled_prefix", lit, collapse_doubled(ab or lit, names))
        elif (failed and notfound and evidence and rel and first in TOP_DIRS and ih and cwd
              and posixpath.normpath(cwd) != posixpath.normpath(ih)):
            add("path.home_relative", lit, resolve(lit, ih))
        elif failed and notfound and evidence and impl_dbl:
            add("path.doubled_prefix", lit, collapse_doubled(ab or lit, names))
        elif not failed and kind == "write" and (lit_dbl or impl_dbl):
            add("path.doubled_prefix", lit, collapse_doubled(ab or lit, names))
        elif (failed and evidence and is_abs and ih and (notfound or denied or tool_path)
              and not any(under(posixpath.normpath(lit), r) for r in roots)):
            add("path.guessed_absolute", lit, None)
    return found


def hits_target(d, flagged, sid, lit, intended):
    """Does the successful call d use the intended path of a path finding (and not itself a flagged use)?"""
    for ulit, uab, _k in call_uses(d):
        if (d["seq"], ulit) in flagged:
            continue
        if sid == "path.guessed_absolute":
            t = tail(lit, 3)
            if tail(uab or ulit, len(t.split("/"))) == t:
                return True
        elif intended is not None and intended.startswith("/"):
            if uab is not None and (uab == intended or uab.startswith(intended.rstrip("/") + "/")):
                return True
        else:
            ref = intended if intended is not None else lit
            n = min(2, len([x for x in ref.split("/") if x]))
            if n and tail(uab or ulit, n) == tail(ref, n):
                return True
    return False


def writes_target(later, ab):
    """Does a later call write/edit (or a mutating bash naming) the resolved path ab?"""
    if later["failed"]:
        return False
    if later["name"] in ("write", "edit"):
        return resolve(later["input"].get("path"), later["cwd"]) == ab
    if later["name"] == "bash" and ab:
        cmd = cmd_of(later)
        if bash_info(cmd)["mut_pos"] is not None:
            if posixpath.basename(ab) in cmd:
                return True
            return any(k == "write" and a == ab for _l, a, k in call_uses(later))
    return False


def analyze(parsed, meta):
    """Signals of one run. meta: gate_id/run ref, outcome, score, caps_hit, trace_caps."""
    calls = parsed["calls"]
    sigs = []
    ref_run = meta["ref_run"]

    # test runs, mutations, expected failures
    for c in calls:
        c["test"] = None
        if c["name"] == "bash":
            info = bash_info(cmd_of(c))
            if info["test"]:
                c["test"] = {"key": info["test"]["key"], "pos": info["test"]["pos"], "stmt": info["test"]["stmt"],
                             "outcome": test_outcome(c["text"], c["exit"])}
        c["expected"] = False
    for i, c in enumerate(calls):
        t = c["test"]
        if not t or t["outcome"] != "fail":
            continue
        for j in range(i + 1, len(calls)):
            d = calls[j]
            if d["seg"] != c["seg"]:
                break
            dt = d["test"]
            if not dt or dt["key"] != t["key"] or dt["outcome"] != "pass":
                continue
            between = any(mutates(calls[k]) for k in range(i + 1, j))
            own = d["name"] == "bash" and bash_info(cmd_of(d))["mut_pos"] is not None and \
                bash_info(cmd_of(d))["mut_pos"] <= dt["pos"]
            if between or own:
                c["expected"] = True
                break

    # per-call signals
    path_inst = []  # (call, sid, targets)
    for c in calls:
        ref = "event:" + c["rid"]
        first = c["text"].split("\n", 1)[0]
        m = ERRLINE_RE.match(first)
        if m and (c["name"] != "bash" or m.group(1) in BASH_ERR_CODES):
            code = m.group(1)
            fatal = (c["name"], code) in EDIT_FATAL or code in ANY_FATAL
            sigs.append(Sig("tool.error:%s:%s" % (c["name"], code), ref, first, fatal, "edit" if fatal else ""))
        if c["name"] == "bash":
            if re.search(r"\[timeout after \d+", c["text"]):
                tm = re.search(r"\[timeout after[^\]]*\]", c["text"])
                sigs.append(Sig("bash.timeout", ref, tm.group(0) if tm else "timeout"))
            if c["exit"] not in (None, 0) and not c["expected"]:
                sigs.append(Sig("bash.exit:" + cmd_class(c), ref,
                                "exit %d: %s" % (c["exit"], " ".join(cmd_of(c).split())[:120])))
            if "initctl" in cmd_of(c):
                for em in re.finditer(r'"error"\s*:\s*\{\s*"code"\s*:\s*"([^"]+)"', c["text"]):
                    sigs.append(Sig("initctl.refused:" + em.group(1), ref, "initctl: " + em.group(1)))
                    break
        for sid, targets in sorted(path_findings(c).items()):
            lit, intended = targets[0]
            extra = " (+%d more)" % (len(targets) - 1) if len(targets) > 1 else ""
            sigs.append(Sig(sid, ref, "%s%s -> %s" % (lit, extra, intended if intended else "?"), False, "path"))
            path_inst.append((c, sid, targets, sigs[-1]))

    # path fatality: no later successful call touches the intended target
    flagged = set((c["seq"], t[0]) for c, _s, ts, _g in path_inst for t in ts)
    for c, sid, targets, sig in path_inst:
        later = [d for d in calls if d["seq"] > c["seq"] and not d["failed"]]
        for lit, intended in targets:
            if not any(hits_target(d, flagged, sid, lit, intended) for d in later):
                sig.fatal = True

    # edit-tool errors: fatal only if never fixed later
    for s in sigs:
        if s.group != "edit":
            continue
        c = next(x for x in calls if "event:" + x["rid"] == s.ref)
        _tool, code = s.sid.split(":")[1], s.sid.split(":")[2]
        if code in ANY_FATAL:
            s.fatal = not any(d["seq"] > c["seq"] and d["name"] == c["name"] and not d["failed"] for d in calls)
        else:
            ab = resolve(c["input"].get("path"), c["cwd"])
            s.fatal = not any(d["seq"] > c["seq"] and writes_target(d, ab) for d in calls)

    # retries and loops
    for i, c in enumerate(calls):
        if not c["failed"] or c["expected"]:
            continue
        h = hashlib.sha256(json.dumps(c["input"], sort_keys=True).encode("utf-8")).hexdigest()
        for d in calls[i + 1:i + 4]:
            if d["name"] == c["name"] and hashlib.sha256(
                    json.dumps(d["input"], sort_keys=True).encode("utf-8")).hexdigest() == h:
                sigs.append(Sig("retry.same_call", "event:" + d["rid"], "%s repeated after an error" % c["name"]))
                break
    for seg in sorted(set(c["seg"] for c in calls)):
        reads = {}
        cmds = {}
        for c in [x for x in calls if x["seg"] == seg]:
            if c["name"] == "read":
                k = resolve(c["input"].get("path"), c["cwd"]) or str(c["input"].get("path"))
                reads[k] = reads.get(k, 0) + 1
                if reads[k] == 3:
                    sigs.append(Sig("loop.repeat_read", "event:" + c["rid"], "read %s 3 times" % k))
            elif c["name"] == "bash" and not mutates(c):
                k = " ".join(cmd_of(c).split())
                cmds[k] = cmds.get(k, 0) + 1
                if cmds[k] == 3:
                    sigs.append(Sig("loop.repeat_cmd", "event:" + c["rid"], "ran 3 times: " + k))
            if mutates(c):
                reads.clear()
                cmds.clear()

    # finish.*
    for fin in parsed["finals"]:
        seg_calls = [c for c in calls if c["seg"] == fin["seg"] and c["idx"] < fin["idx"]]
        tests = [c for c in seg_calls if c["test"] and c["test"]["outcome"] in ("pass", "fail")]
        if tests and tests[-1]["test"]["outcome"] == "fail":
            t = tests[-1]
            sigs.append(Sig("finish.tests_failing", "event:" + fin["eid"],
                            "last test `%s` exited %s" % (t["test"]["stmt"][:100], t["exit"]), True, "finish"))
        muts = [c for c in seg_calls if mutates(c)]
        if muts:
            lm = muts[-1]
            tested = any(c["test"] for c in seg_calls if c["idx"] > lm["idx"])
            if not tested and lm["test"]:
                mp = bash_info(cmd_of(lm))["mut_pos"] if lm["name"] == "bash" else None
                tested = mp is not None and lm["test"]["pos"] >= mp
            if not tested:
                sigs.append(Sig("finish.untested", "event:" + fin["eid"], "final answer after the last change, no test run"))

    # cap.hit
    cap_idx = None
    seen_caps = set()
    for er in parsed["errors"]:
        data = er["data"]
        if er["code"] == "budget" or (er["code"] == "provider" and data.get("kind") == "budget"):
            cap = data.get("cap") or data.get("x_init_cap")
            if not isinstance(cap, str) or not cap:
                mm = re.search(r"x-init-cap[:=]\s*([A-Za-z_]+)|budget\.([A-Za-z_]+)", er["message"])
                cap = (mm.group(1) or mm.group(2)) if mm else (meta.get("caps_hit") or "unknown")
            sigs.append(Sig("cap.hit:" + cap, "event:" + er["eid"], "%s %s" % (er["code"], er["message"][:120])))
            seen_caps.add(cap)
            cap_idx = er["idx"] if cap_idx is None else min(cap_idx, er["idx"])
    for cap, rq in meta.get("trace_caps", []):
        sigs.append(Sig("cap.hit:" + cap, "trace:" + rq, "429 with x-init-cap " + cap))
        seen_caps.add(cap)
    if meta.get("caps_hit") and meta["caps_hit"] not in seen_caps:
        sigs.append(Sig("cap.hit:" + meta["caps_hit"], "run:" + ref_run, "RunRecord caps_hit " + meta["caps_hit"]))
        seen_caps.add(meta["caps_hit"])
    if meta.get("outcome") == "cost_capped" and not seen_caps:
        sigs.append(Sig("cap.hit:cost", "run:" + ref_run, "RunRecord outcome cost_capped"))

    score = meta.get("score")
    if score is not None and score < 1000:
        sigs.append(Sig("task.fail", "run:" + ref_run, "outcome=%s score_permille=%d" % (meta.get("outcome"), score)))
    return sigs, cap_idx


def dedupe(sigs):
    seen = set()
    out = []
    for s in sigs:
        k = (s.sid, s.ref, s.detail)
        if k not in seen:
            seen.add(k)
            out.append(s)
    out.sort(key=lambda s: (s.sid, s.ref, s.detail))
    return out


# ----------------------------------------------------------------------------------------------
# classification
# ----------------------------------------------------------------------------------------------

def exploring(calls, cap_idx):
    pre = [c for c in calls if cap_idx is None or c["idx"] < cap_idx]
    if not pre:
        return None
    if not any(mutates(c) for c in pre):
        return "no_file_change"
    last5 = pre[-5:]
    ro = sum(1 for c in last5 if readonly(c))
    if ro * 3 >= 2 * len(last5):
        return "readonly_tail=%d/%d" % (ro, len(last5))
    return ""


def classify(meta, parsed, sigs, cap_idx):
    ids = [s.sid for s in sigs]
    cap_sigs = [s for s in sigs if s.sid.startswith("cap.hit:")]
    capped = bool(cap_sigs) or meta.get("outcome") == "cost_capped" or bool(meta.get("caps_hit"))
    capname = cap_sigs[0].sid.split(":", 1)[1] if cap_sigs else (meta.get("caps_hit") or "cost")
    if capped:
        ex = exploring(parsed["calls"], cap_idx)
        if ex:
            return "cap_hit_exploring", "cap.hit:%s;%s" % (capname, ex)
    if "finish.tests_failing" in ids:
        return "stopped_tests_failing", "finish.tests_failing"
    paths = [s for s in sigs if s.sid.startswith("path.")]
    fatal = [s for s in paths if s.fatal]
    inst = len(set((s.ref, s.sid) for s in paths))
    if fatal:
        return "path_confusion", "path.fatal:" + fatal[0].sid
    if inst >= 2:
        return "path_confusion", "path.instances=%d" % inst
    edits = [s for s in sigs if s.group == "edit"]
    efatal = [s for s in edits if s.fatal]
    if efatal:
        return "edit_tool_mismatch", "edit.fatal:" + efatal[0].sid
    if len(edits) >= 2:
        return "edit_tool_mismatch", "edit.errors=%d" % len(edits)
    outcome = meta.get("outcome")
    if capped:
        why = "cap_hit_before_first_tool_call" if exploring(parsed["calls"], cap_idx) is None else "cap_hit_while_editing"
        return "other", "other:" + why
    for o in ("timeout", "crash", "no_ready", "infra"):
        if outcome == o:
            return "other", "other:" + o
    if "finish.untested" in ids:
        return "other", "other:finish.untested"
    if not parsed["finals"]:
        return "other", "other:no_final_answer"
    return "other", "other:no_signal"


# ----------------------------------------------------------------------------------------------
# inputs
# ----------------------------------------------------------------------------------------------

def new_run(run, session, **kw):
    r = {"run": run, "ref_run": run, "session": session, "task": None, "labels": [], "set": "pool",
         "score": None, "outcome": "unknown", "caps_hit": None, "cwd": None, "init_home": None,
         "trace_caps": [], "inherited": (None, None), "required_log": False}
    r.update(kw)
    return r


def load_manifest(path, defaults):
    try:
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError) as ex:
        raise BadInput("manifest %s: %s" % (path, ex))
    if isinstance(data, dict):
        data = data.get("runs")
    if not isinstance(data, list):
        raise BadInput("manifest %s: expected a list of runs" % path)
    base = os.path.dirname(os.path.abspath(path))
    out = []
    for i, ent in enumerate(data):
        if not isinstance(ent, dict) or not isinstance(ent.get("run"), str) or not ent["run"]:
            raise BadInput("manifest %s: entry %d needs a string `run`" % (path, i))
        score = ent.get("score_permille")
        if score is not None and not is_int(score):
            raise BadInput("manifest %s: entry %d: score_permille must be an integer" % (path, i))
        labels = ent.get("labels", [])
        if not isinstance(labels, list) or not all(isinstance(x, str) for x in labels):
            raise BadInput("manifest %s: entry %d: labels must be a list of strings" % (path, i))
        sess = ent.get("session")
        if sess is not None and not isinstance(sess, str):
            raise BadInput("manifest %s: entry %d: session must be a path string" % (path, i))
        if isinstance(sess, str):
            sess = os.path.normpath(os.path.join(base, sess))
            check_safe_path(sess)
        for k in ("task", "outcome", "caps_hit", "set", "cwd", "init_home"):
            if ent.get(k) is not None and not isinstance(ent[k], str):
                raise BadInput("manifest %s: entry %d: %s must be a string" % (path, i, k))
        out.append(new_run(ent["run"], sess, task=ent.get("task"), labels=sorted(labels),
                           set=ent.get("set") or "pool", score=score, outcome=ent.get("outcome") or "unknown",
                           caps_hit=ent.get("caps_hit"), cwd=ent.get("cwd") or defaults[0],
                           init_home=ent.get("init_home") or defaults[1]))
    return out


def load_artifacts(root, defaults):
    if not os.path.isdir(root):
        raise BadInput("--artifacts %s is not a directory" % root)
    check_safe_path(root)
    files = sorted(glob.glob(os.path.join(root, "*", "init", "sessions", "*.jsonl")))
    if not files:
        files = sorted(glob.glob(os.path.join(root, "*", "*", "init", "sessions", "*.jsonl")))
    runs = []
    seen = {}
    deduped = 0
    summaries = {}
    prev_boot = {}
    for f in files:
        sdir = os.path.dirname(f)
        check = os.path.basename(os.path.dirname(os.path.dirname(sdir)))
        sid = os.path.basename(f)[:-len(".jsonl")]
        try:
            with open(f, "rb") as fh:
                digest = hashlib.sha256(fh.read()).hexdigest()
        except OSError:
            continue
        if digest in seen:
            deduped += 1
            continue
        seen[digest] = f
        r = new_run("%s/%s" % (check, sid), f, set="artifact", cwd=defaults[0], init_home=defaults[1])
        r["inherited"] = prev_boot.get(sdir, (None, None))
        r["_sdir"] = sdir
        r["_check"] = check
        r["_sid"] = sid
        runs.append(r)
        # remember this session's boot ctx for the next session of the same directory
        ev = read_events(f)
        if ev:
            bs = [boot_ctx(e) for e in ev if e.get("type") == "boot"]
            if bs:
                prev_boot[sdir] = bs[-1]
    for r in runs:
        sp = os.path.join(os.path.dirname(os.path.dirname(r["_sdir"])), "run", "traces", "summaries.jsonl")
        if sp not in summaries:
            rows = {}
            try:
                with open(sp, "r", encoding="utf-8", errors="replace") as fh:
                    for line in fh.read().split("\n"):
                        row = jloads(line) if line.strip() else None
                        if isinstance(row, dict) and isinstance(row.get("session"), str):
                            rows.setdefault(row["session"], []).append(row)
            except OSError:
                pass
            summaries[sp] = rows
        rows = summaries[sp].get(r["_sid"], [])
        for row in sorted(rows, key=lambda x: str(x.get("req_id"))):
            if r["task"] is None and isinstance(row.get("task"), str):
                r["task"] = row["task"]
            cap = cap_of_record(row)
            if cap:
                r["trace_caps"].append((cap, str(row.get("req_id"))))
        for k in ("_sdir", "_check", "_sid"):
            r.pop(k, None)
    return runs, deduped


def cap_of_record(rec):
    """The cap name of an agent-view request record that is a 429 cap refusal, else None."""
    if rec.get("status") != 429:
        return None
    for k in ("cap", "x_init_cap", "x-init-cap"):
        v = rec.get(k)
        if isinstance(v, str) and v:
            return v
    out = rec.get("outcome")
    if isinstance(out, str) and (out.startswith("cap") or out.startswith("budget")):
        return "unknown"
    return None


def load_home(home, arms, skipped, defaults):
    if not os.path.isdir(home):
        raise BadInput("--home %s is not a directory" % home)
    check_safe_path(home)
    jobs = os.path.join(home, "vault", "gate", "jobs")
    runs = []
    seen = {}
    try:
        names = sorted(os.listdir(jobs))
    except OSError:
        names = []
    for job in names:
        if not SAFE_NAME.match(job):
            continue
        rp = os.path.join(jobs, job, "runs.jsonl")
        try:
            with open(rp, "r", encoding="utf-8", errors="replace") as f:
                lines = f.read().split("\n")
        except OSError:
            continue
        for line in lines:
            rec = jloads(line) if line.strip() else None
            if not isinstance(rec, dict):
                continue
            if rec.get("set") != "pool":
                skipped["non_pool"] += 1
                continue
            if arms and rec.get("arm") not in arms:
                continue
            run_id = rec.get("run_id")
            gate_id = rec.get("gate_id") if isinstance(rec.get("gate_id"), str) else job
            if not isinstance(run_id, str) or not SAFE_NAME.match(run_id) or not SAFE_NAME.match(gate_id):
                continue
            score = rec.get("score_permille")
            labels = rec.get("labels") if isinstance(rec.get("labels"), list) else []
            sess = os.path.join(jobs, job, "sessions", run_id + ".jsonl")
            r = new_run("%s/%s" % (gate_id, run_id), sess, task=rec.get("task") if isinstance(rec.get("task"), str) else None,
                        labels=sorted(x for x in labels if isinstance(x, str)), set="pool",
                        score=score if is_int(score) else None,
                        outcome=rec.get("outcome") if isinstance(rec.get("outcome"), str) else "unknown",
                        caps_hit=rec.get("caps_hit") if isinstance(rec.get("caps_hit"), str) else None,
                        cwd=defaults[0], init_home=defaults[1])
            r["_scope"] = "%s/run/%s" % (gate_id, run_id)
            r["_home"] = home
            seen[r["run"]] = r
    runs = [seen[k] for k in sorted(seen)]
    attach_traces(home, runs)
    return runs


def attach_traces(home, runs):
    """429 cap refusals of agent-view request records, for pool run scopes only."""
    scopes = dict((r["_scope"], r) for r in runs)
    tdir = os.path.join(home, "vault", "traces", "v1", "req")
    try:
        files = sorted(glob.glob(os.path.join(tdir, "*.jsonl")))
    except OSError:
        files = []
    scope_re = re.compile(r'"scope"\s*:\s*"([^"]*)"')
    start = {}
    for f in files:
        try:
            with open(f, "r", encoding="utf-8", errors="replace") as fh:
                lines = fh.read().split("\n")
        except OSError:
            continue
        for line in lines:
            m = scope_re.search(line)
            if m is None:
                # an `end` record has no scope: it is matched through its req_id, see below
                mid = re.search(r'"req_id"\s*:\s*"([^"]*)"', line)
                if mid is None or mid.group(1) not in start or '"t":"end"' not in line.replace(" ", ""):
                    continue
                rec = jloads(line)
                if isinstance(rec, dict):
                    cap = cap_of_record(rec)
                    if cap:
                        start[mid.group(1)]["trace_caps"].append((cap, mid.group(1)))
                continue
            if m.group(1) not in scopes:
                continue
            rec = jloads(line)
            if not isinstance(rec, dict) or rec.get("vis") not in (None, "agent"):
                continue
            rid = rec.get("req_id")
            if rec.get("t") == "start" and isinstance(rid, str):
                start[rid] = scopes[m.group(1)]
            elif rec.get("t") == "end" and isinstance(rid, str):
                cap = cap_of_record(rec)
                if cap:
                    scopes[m.group(1)]["trace_caps"].append((cap, rid))


# ----------------------------------------------------------------------------------------------
# pipeline
# ----------------------------------------------------------------------------------------------

def process(run, home_root):
    """-> result dict, or None when the run is skipped for want of a log (caller counts it)."""
    score = run["score"]
    failing = score is not None and score < 1000
    events = None
    sess = run["session"]
    if sess:
        ok = True
        if home_root is not None:
            real = os.path.realpath(sess)
            ok = under(real, os.path.realpath(home_root))
        events = read_events(sess) if ok else None
    if events is None:
        if failing:
            return None
        events = []
    parsed = parse_session(events)
    inherited = run["inherited"]
    assign_ctx(parsed, run["cwd"], run["init_home"], inherited)
    meta = {"ref_run": run["ref_run"], "outcome": run["outcome"], "score": score, "caps_hit": run["caps_hit"],
            "trace_caps": sorted(set(run["trace_caps"]))}
    sigs, cap_idx = analyze(parsed, meta)
    sigs = dedupe(sigs)
    if score is None:
        cls, reason = "unscored", "no score_permille"
    elif not failing:
        cls, reason = "pass", "score_permille=%d" % score
    else:
        cls, reason = classify(meta, parsed, sigs, cap_idx)
    return {"run": run["run"], "task": run["task"], "labels": run["labels"], "set": run["set"],
            "score_permille": score, "outcome": run["outcome"], "class": cls, "reason": reason,
            "signals": [s.out() for s in sigs]}


def build_report(results, skipped, deduped):
    totals = dict((c, 0) for c in CLASSES)
    counts = {"failing": 0, "pass": 0, "unscored": 0, "deduped": deduped}
    for r in results:
        if r["class"] in totals:
            totals[r["class"]] += 1
            counts["failing"] += 1
        elif r["class"] == "pass":
            counts["pass"] += 1
        else:
            counts["unscored"] += 1
    failing = counts["failing"]
    share = dict((c, permille(totals[c], failing)) for c in CLASSES)
    largest = None
    if failing:
        largest = sorted(CLASSES, key=lambda c: (-totals[c], CLASSES.index(c)))[0]
    return {"v": 1, "runs": sorted(results, key=lambda r: r["run"]), "counts": counts, "totals": totals,
            "share_permille": share, "largest": largest, "candidate": CANDIDATES.get(largest),
            "skipped": dict(skipped)}


def render_table(rep):
    lines = []
    c = rep["counts"]
    lines.append("failure classes: %d failing, %d pass, %d unscored | skipped non_pool=%d no_log=%d"
                 % (c["failing"], c["pass"], c["unscored"], rep["skipped"]["non_pool"], rep["skipped"]["no_log"]))
    lines.append("%-24s %5s %8s" % ("class", "runs", "share_pm"))
    for k in CLASSES:
        lines.append("%-24s %5d %8d" % (k, rep["totals"][k], rep["share_permille"][k]))
    lines.append("largest: %s | candidate: %s" % (rep["largest"], rep["candidate"]))
    bad = [r for r in rep["runs"] if r["class"] in CLASSES]
    if bad:
        lines.append("failing runs:")
        for r in bad:
            ids = sorted(set(s["signal"] for s in r["signals"]))
            lines.append("  %s  %s  %s  [%s]" % (r["run"], r["class"], r["reason"], ", ".join(ids)))
    return "\n".join(lines) + "\n"


def emit(text):
    buf = getattr(sys.stdout, "buffer", None)
    if buf is not None:
        sys.stdout.flush()
        buf.write(text.encode("utf-8", "replace"))
        buf.flush()
    else:
        sys.stdout.write(text)


def main(argv=None):
    ap = argparse.ArgumentParser(prog="failclass", description="classify failing visible-pool runs (m7 3.7 step 2)")
    ap.add_argument("--home", help="kernel INIT_HOME with vault/gate/jobs/*/runs.jsonl")
    ap.add_argument("--artifacts", help="S-REAL style dir: */init/sessions/*.jsonl")
    ap.add_argument("--manifest", action="append", help="JSON list of runs (repeatable)")
    ap.add_argument("--json", action="store_true", help="print ICJ-like JSON")
    ap.add_argument("--cwd", help="default cwd for runs with no boot event or pwd")
    ap.add_argument("--init-home", help="default INIT_HOME for runs with no boot event")
    ap.add_argument("--arm", action="append", help="--home: only this arm (c or p)")
    try:
        args = ap.parse_args(argv)
    except SystemExit as ex:
        return ex.code if isinstance(ex.code, int) else 2
    if not (args.home or args.artifacts or args.manifest):
        sys.stderr.write("failclass: give at least one of --home, --artifacts, --manifest\n")
        return 2
    skipped = {"non_pool": 0, "no_log": 0}
    defaults = (args.cwd, args.init_home)
    try:
        runs = []
        deduped = 0
        home_root = args.home
        if args.home:
            runs += load_home(args.home, set(args.arm or []), skipped, defaults)
        if args.artifacts:
            ar, deduped = load_artifacts(args.artifacts, defaults)
            runs += ar
        for m in args.manifest or []:
            runs += load_manifest(m, defaults)
        ids = [r["run"] for r in runs]
        if len(set(ids)) != len(ids):
            dup = sorted(set(i for i in ids if ids.count(i) > 1))[0]
            raise BadInput("duplicate run id %s" % dup)
        results = []
        for r in runs:
            if r["set"] != "pool" and r["set"] != "artifact":
                skipped["non_pool"] += 1
                continue
            try:
                res = process(r, home_root if r.get("_home") else None)
            except Exception as ex:  # a log shape this script cannot read: leave the run out, say so on stderr
                sys.stderr.write("failclass: %s: cannot analyse (%s)\n" % (r["run"], type(ex).__name__))
                res = None
            if res is None:
                skipped["no_log"] += 1
                continue
            results.append(res)
    except BadInput as ex:
        sys.stderr.write("failclass: %s\n" % ex)
        return 2
    rep = build_report(results, skipped, deduped)
    if args.json:
        # ICJ v1 (log-context 3.8.1): sorted keys, no whitespace, non-ASCII written raw as UTF-8
        emit(json.dumps(rep, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n")
    else:
        emit(render_table(rep))
    return 0


if __name__ == "__main__":
    sys.exit(main())
