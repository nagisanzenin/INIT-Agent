"""evo-big-schema probe runner: imports the workspace's models.py and builds record instances from probe
inputs (constructor keyword arguments or from_dict payloads). Writes what it observed (to_dict() result,
attributes, exception, warnings, isinstance) as JSON to argv[2]. It never sees expected values.

argv: probes.json out.json workspace_dir. Run as a subprocess with a timeout."""
import inspect
import json
import sys
import warnings

sys.dont_write_bytecode = True
sys.path[:] = [sys.argv[3]] + [p for p in sys.path[1:] if p != sys.argv[3]]
SWAP = {"_km": "_mi", "_mi": "_km", "_kg": "_lb", "_lb": "_kg"}
MISSING = {"__missing__": True}


def plain(x):
    if isinstance(x, (str, int, float, bool)) or x is None:
        return x
    if isinstance(x, dict):
        return {"__dict__": [[plain(k), plain(v)] for k, v in x.items()]}
    if isinstance(x, (list, tuple)):
        return [plain(v) for v in x]
    return {"__repr__": repr(x)[:200]}


def adapt(cls, kwargs):
    try:
        names = set(inspect.signature(cls).parameters)
    except (TypeError, ValueError):
        return kwargs
    out = {}
    for k, v in kwargs.items():
        alt = k[:-3] + SWAP[k[-3:]] if k[-3:] in SWAP else None
        if k not in names and alt in names:
            out[alt] = v
        else:
            out[k] = v
    return out


def run(mod, p):
    res = {"id": p["id"], "exc": None, "dict": None, "attrs": {}, "warns": [], "isinst": None}
    try:
        cls = getattr(mod, p["cls"])
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            try:
                if p["kind"] == "new":
                    obj = cls(**adapt(cls, p["kwargs"]))
                else:
                    obj = cls.from_dict(json.loads(json.dumps(p["data"])))
            except BaseException as e:  # noqa: BLE001 - the probe records any exception
                res["exc"] = [type(e).__name__, str(e)[:300]]
                obj = None
        res["warns"] = [[x.category.__name__, str(x.message)[:300]] for x in w]
        if obj is not None:
            for a in p["attrs"]:
                res["attrs"][a] = plain(getattr(obj, a)) if hasattr(obj, a) else MISSING
            if p.get("isinst"):
                res["isinst"] = isinstance(obj, getattr(mod, p["isinst"]))
            try:
                res["dict"] = plain(obj.to_dict())
            except BaseException as e:  # noqa: BLE001
                res["dict"] = {"__error__": [type(e).__name__, str(e)[:200]]}
    except BaseException as e:  # noqa: BLE001
        res["exc"] = ["harness:" + type(e).__name__, str(e)[:300]]
    return res


def main():
    with open(sys.argv[1]) as f:
        probes = json.load(f)
    out = {"probes": [], "import": None}
    try:
        import models as mod
    except BaseException as e:  # noqa: BLE001 - an import failure fails every item
        out["import"] = [type(e).__name__, str(e)[:200]]
        mod = None
    if mod is not None:
        for p in probes:
            out["probes"].append(run(mod, p))
    with open(sys.argv[2], "w") as f:
        f.write(json.dumps(out, default=repr))


main()
