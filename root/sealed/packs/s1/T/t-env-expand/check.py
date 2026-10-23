# t-env-expand: run `sh run.sh print-config` in private copies of the workspace, with the hidden env-file sets
# (config/ replaced) and caller environments, and compare stdout with the expected KEY=value lines.


def scenario(case):
    def fn():
        ws = copy_ws("ws-" + case["name"])
        if case["files"] is not None:
            cfg = os.path.join(ws, "config")
            if os.path.islink(cfg) or os.path.isfile(cfg):
                os.remove(cfg)
            else:
                shutil.rmtree(cfg, ignore_errors=True)
            for rel, text in sorted(case["files"].items()):
                write_file(os.path.join(ws, rel), text)
        script = os.path.join(ws, "run.sh")
        if read_bytes(script) is None:
            say("run.sh missing or not a regular file")
            return False
        code, out, _err = run(["/bin/sh", "run.sh", "print-config"], ws, timeout=6, env=base_env(case["env"]))
        if code != 0:
            say("%s: exit %s" % (case["name"], code))
            return False
        try:
            got = out.decode("utf-8")
        except UnicodeDecodeError:
            return False
        ok = norm_lines(got) == norm_lines(case["want"])
        if not ok:
            g, w = norm_lines(got), norm_lines(case["want"])
            say("%s: %d line(s) vs %d expected, first difference at line %d" % (
                case["name"], len(g), len(w), next((i for i, (a, b) in enumerate(zip(g, w)) if a != b),
                                                   min(len(g), len(w))) + 1))
        return ok
    return fn


def main():
    run_checks([(c["name"], scenario(c)) for c in SPEC["cases"]])
