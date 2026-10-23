# t-config-migrate: out/<name>.json must hold the v2 document of each visible config, and ./migrate must
# convert hidden inis. JSON is compared after parsing, type-strict (true is not 1, 5432 is not 5432.0).
# All tool runs share one workspace copy; the first runs alone and the others then run in parallel, and a
# first run that hangs fails every case (keeps a hanging tool within the time budget).
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


def same(a, b):
    if type(a) is not type(b):
        return False
    if isinstance(a, dict):
        return set(a) == set(b) and all(same(a[k], b[k]) for k in a)
    if isinstance(a, list):
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    return a == b


def parse(text):
    try:
        return True, json.loads(text)
    except ValueError:
        return False, None


def _one(ws, argv, c):
    code, out, err = run(argv + [c["arg"]], ws, timeout=6)
    if code is None:
        return False, "timeout", True
    if code != 0:
        return False, "exit %s %s" % (code, err[-300:]), False
    try:
        ok, doc = parse(out.decode("utf-8"))
    except UnicodeDecodeError:
        return False, "output is not UTF-8", False
    if not ok:
        return False, "output is not JSON", False
    if not same(doc, c["want"]):
        return False, "document differs", False
    return True, "", False


def tool_results(cases):
    res = [(False, "not run")] * len(cases)
    ws = copy_ws()
    tool = os.path.join(ws, SPEC["tool"])
    if not is_exec_file(tool):
        return [(False, "./%s missing or not executable" % SPEC["tool"])] * len(cases)
    for c in cases:
        write_file(os.path.join(ws, c["arg"]), c["input"])
    argv = cmd_for(tool)
    ok, why, hung = _one(ws, argv, cases[0])
    res[0] = (ok, why)
    if hung:
        return [(False, "first run timed out")] * len(cases)

    def work(i):
        try:
            r = _one(ws, argv, cases[i])
            res[i] = (r[0], r[1])
        except Exception as e:  # noqa: BLE001
            res[i] = (False, "%s: %s" % (type(e).__name__, e))

    threads = [threading.Thread(target=work, args=(i,)) for i in range(1, len(cases))]
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

    def outputs():
        for o in SPEC["outputs"]:
            got = read_text(ws_path(o["path"]))
            if got is None:
                say("%s missing or unreadable" % o["path"])
                return False
            ok, doc = parse(got)
            if not ok or not same(doc, o["want"]):
                say("%s: %s" % (o["path"], "not JSON" if not ok else "document differs"))
                return False
        return True

    def case(i):
        def fn():
            ok, why = results[i]
            if not ok:
                say("%s: %s" % (cases[i]["name"], why))
            return ok
        return fn

    run_checks([("outputs", outputs)] + [(c["name"], case(i)) for i, c in enumerate(cases)])
