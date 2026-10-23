"""evo-big-forms probe runner: imports the workspace's widgets.py, swaps in a recording renderer and calls
widget functions with probe keyword arguments (inputs only). Writes what it observed (render calls, result,
exception, warnings) as JSON to argv[2]. It never sees expected values.

argv: probes.json out.json workspace_dir. Run as a subprocess with a timeout."""
import inspect
import json
import sys
import warnings

sys.dont_write_bytecode = True
sys.path[:] = [sys.argv[3]] + [p for p in sys.path[1:] if p != sys.argv[3]]


class Recorder:
    def __init__(self):
        self.calls = []
        self.last = None

    def render(self, tokens):
        self.calls.append([t if isinstance(t, str) else repr(t) for t in tokens])
        self.last = object()
        return self.last


def adapt(fn, kwargs):
    try:
        names = set(inspect.signature(fn).parameters)
    except (TypeError, ValueError):
        return kwargs
    return dict((k, v) for k, v in kwargs.items() if k in names or k != "theme")


def run(mod, p):
    res = {"id": p["id"], "calls": [], "exc": None, "warns": [], "ret": False}
    rec = Recorder()
    try:
        mod.RENDERER = rec
        fn = getattr(mod, p["fn"])
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            try:
                out = fn(**adapt(fn, p["kwargs"]))
                res["ret"] = rec.last is not None and out is rec.last
            except BaseException as e:  # noqa: BLE001 - the probe records any exception
                res["exc"] = [type(e).__name__, str(e)[:300]]
        res["warns"] = [[x.category.__name__, str(x.message)[:300]] for x in w]
    except BaseException as e:  # noqa: BLE001
        res["exc"] = ["harness:" + type(e).__name__, str(e)[:300]]
    res["calls"] = rec.calls
    return res


def main():
    with open(sys.argv[1]) as f:
        probes = json.load(f)
    out = {"probes": [], "import": None}
    try:
        import widgets as mod
    except BaseException as e:  # noqa: BLE001 - an import failure fails every item
        out["import"] = [type(e).__name__, str(e)[:200]]
        mod = None
    if mod is not None:
        for p in probes:
            out["probes"].append(run(mod, p))
    with open(sys.argv[2], "w") as f:
        f.write(json.dumps(out, default=repr))


main()
