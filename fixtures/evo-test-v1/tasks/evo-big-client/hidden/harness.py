"""evo-big-client probe runner: imports the workspace's client.py and runs probes (method name + keyword
arguments) against a recording transport. Writes what it observed (transport calls, exception, warnings,
whether the transport's response was returned) as JSON to argv[2]. It never sees expected values.

argv: probes.json out.json workspace_dir. Run as a subprocess with a timeout."""
import inspect
import json
import sys
import warnings

sys.dont_write_bytecode = True
sys.path[:] = [sys.argv[3]] + [p for p in sys.path[1:] if p != sys.argv[3]]
ALIAS = {"account_id": "account_ref", "account_ref": "account_id"}
DROPPABLE = ("cursor", "limit", "page", "per_page", "page_size")


def plain(x):
    if isinstance(x, (str, int, float, bool)) or x is None:
        return x
    if isinstance(x, dict):
        return {str(k) if not isinstance(k, str) else k: plain(v) for k, v in x.items()}
    if isinstance(x, (list, tuple)):
        return [plain(v) for v in x]
    return {"__repr__": repr(x)}


class Recorder:
    def __init__(self):
        self.calls = []
        self.last = None

    def request(self, verb, path, params=None, json=None, retries=0):
        self.calls.append([plain(verb), plain(path), plain(dict(params or {})), plain(dict(json or {})),
                           plain(retries)])
        self.last = {"seq": len(self.calls), "token": object()}
        return self.last


def adapt(fn, kwargs):
    try:
        sig = inspect.signature(fn)
    except (TypeError, ValueError):
        return kwargs
    names = set(sig.parameters)
    if any(p.kind == p.VAR_KEYWORD for p in sig.parameters.values()):
        return kwargs
    out = {}
    for k, v in kwargs.items():
        if k in names:
            out[k] = v
        elif ALIAS.get(k) in names:
            out[ALIAS[k]] = v
        elif k in DROPPABLE:
            continue
        else:
            out[k] = v
    return out


def run(client_cls, probe):
    rec = Recorder()
    res = {"id": probe["id"], "calls": [], "exc": None, "warns": [], "ret": False}
    try:
        obj = client_cls(rec)
        fn = getattr(obj, probe["name"])
        kwargs = adapt(fn, probe["kwargs"])
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            try:
                out = fn(**kwargs)
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
    results = []
    try:
        import client
        cls = client.Client
    except BaseException as e:  # noqa: BLE001 - an import failure fails every probe
        results = [{"id": p["id"], "calls": [], "exc": ["import:" + type(e).__name__, str(e)[:300]],
                    "warns": [], "ret": False} for p in probes]
        cls = None
    if cls is not None:
        for p in probes:
            results.append(run(cls, p))
    with open(sys.argv[2], "w") as f:
        f.write(json.dumps(results, default=repr))


main()
