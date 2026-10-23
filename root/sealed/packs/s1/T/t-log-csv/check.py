# t-log-csv: report.csv must match; ./log2csv runs on the visible log and on hidden logs.
# All runs share one workspace copy; the first case runs alone and the others then run in parallel, and a
# first run that hangs fails every case, which keeps a hanging tool within the time budget.
import re
import threading

_SHEBANG = re.compile(r"^#![ \t]*(/[^ \t\r\n]+)(?:[ \t]+([^ \t\r\n]+))?[ \t]*$")


def cmd_for(tool):
    """argv for an executable tool. A `#!` script starts through its interpreter, as the kernel would start
    it: macOS scans every freshly copied executable on its first exec, which costs seconds under load."""
    try:
        with open(tool, "rb") as f:
            head = f.read(512).split(b"\n", 1)[0].decode("utf-8")
    except (OSError, UnicodeDecodeError):
        return [tool]
    m = _SHEBANG.match(head)
    if not m or not is_exec_file(m.group(1)):
        return [tool]
    return [m.group(1)] + ([m.group(2)] if m.group(2) else []) + [tool]


def _one(ws, tool, c):
    code, out, err = run(cmd_for(tool), ws, stdin=c["input"], timeout=6)
    if code is None:
        return False, "timeout", True
    if code != 0:
        return False, "exit %s %s" % (code, err[-300:]), False
    try:
        got = out.decode("utf-8")
    except UnicodeDecodeError:
        return False, "output is not UTF-8", False
    if norm_lines(got) != norm_lines(c["want"]):
        return False, "output differs", False
    return True, "", False


def tool_results(cases):
    """[(ok, why)] per case; the visible case (last in spec) runs first."""
    res = [(False, "not run")] * len(cases)
    ws = copy_ws()
    tool = os.path.join(ws, SPEC["tool"])
    if not is_exec_file(tool):
        return [(False, "./%s missing or not executable" % SPEC["tool"])] * len(cases)
    order = list(range(len(cases)))[::-1]
    ok, why, hung = _one(ws, tool, cases[order[0]])
    res[order[0]] = (ok, why)
    if hung:
        return [(False, "first run timed out")] * len(cases)

    def work(i):
        try:
            r = _one(ws, tool, cases[i])
            res[i] = (r[0], r[1])
        except Exception as e:  # noqa: BLE001
            res[i] = (False, "%s: %s" % (type(e).__name__, e))

    threads = [threading.Thread(target=work, args=(i,)) for i in order[1:]]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    return res


def main():
    cases = SPEC["cases"]
    try:
        results = tool_results(cases)
    except Exception as e:  # noqa: BLE001
        results = [(False, "%s: %s" % (type(e).__name__, e))] * len(cases)

    def report_fn():
        got = read_text(ws_path(SPEC["report"]))
        if got is None:
            say("%s missing or unreadable" % SPEC["report"])
            return False
        return norm_lines(got) == norm_lines(SPEC["report_want"])

    def case(i):
        def fn():
            ok, why = results[i]
            if not ok:
                say("%s: %s" % (cases[i]["name"], why))
            return ok
        return fn

    run_checks([("report", report_fn)] + [(c["name"], case(i)) for i, c in enumerate(cases)])
