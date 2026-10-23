# t-cron-next: next_runs.txt must match; ./cronnext runs on the visible crontab and on hidden crontabs.
# All runs share one workspace copy and run in parallel, so a slow or hanging tool costs one timeout at most.
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


def _one(ws, argv, c):
    code, out, err = run(argv + ["--now", c["now"]], ws, stdin=c["input"], timeout=8)
    if code is None:
        return False, "timeout"
    if code != 0:
        return False, "exit %s %s" % (code, err[-300:])
    try:
        got = out.decode("utf-8")
    except UnicodeDecodeError:
        return False, "output is not UTF-8"
    if norm_lines(got) != norm_lines(c["want"]):
        return False, "output differs"
    return True, ""


def tool_results(cases):
    res = [(False, "not run")] * len(cases)
    ws = copy_ws()
    tool = os.path.join(ws, SPEC["tool"])
    if not is_exec_file(tool):
        return [(False, "./%s missing or not executable" % SPEC["tool"])] * len(cases)
    argv = cmd_for(tool)

    def work(i):
        try:
            res[i] = _one(ws, argv, cases[i])
        except Exception as e:  # noqa: BLE001
            res[i] = (False, "%s: %s" % (type(e).__name__, e))

    threads = [threading.Thread(target=work, args=(i,)) for i in range(len(cases))]
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

    def file_fn():
        got = read_text(ws_path(SPEC["file"]))
        if got is None:
            say("%s missing or unreadable" % SPEC["file"])
            return False
        return norm_lines(got) == norm_lines(SPEC["file_want"])

    def case(i):
        def fn():
            ok, why = results[i]
            if not ok:
                say("%s: %s" % (cases[i]["name"], why))
            return ok
        return fn

    run_checks([("file", file_fn)] + [(c["name"], case(i)) for i, c in enumerate(cases)])
