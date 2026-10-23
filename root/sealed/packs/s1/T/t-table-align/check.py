# t-table-align: run the workspace's ./align on the visible example and on hidden tables.


def main():
    ws = copy_ws()
    tool = os.path.join(ws, SPEC["tool"])

    def case(c):
        def fn():
            if not is_exec_file(tool):
                say("./%s missing or not executable" % SPEC["tool"])
                return False
            code, out, err = run([tool], ws, stdin=c["input"], timeout=5)
            if code != 0:
                say("%s: exit %s %s" % (c["name"], code, err[-300:]))
                return False
            try:
                got = out.decode("utf-8")
            except UnicodeDecodeError:
                return False
            ok = norm_lines(got) == norm_lines(c["want"]) and got.endswith("\n")
            if not ok:
                say("%s: output differs" % c["name"])
            return ok
        return fn

    run_checks([(c["name"], case(c)) for c in SPEC["cases"]])
