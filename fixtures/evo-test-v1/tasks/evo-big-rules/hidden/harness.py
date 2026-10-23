"""evo-big-rules probe runner: imports the workspace's rules.py and calls rule functions on probe records and
contexts (inputs only). Writes the results, the exceptions and a dump of RULES as JSON to argv[2]. It never
sees expected values.

argv: probes.json out.json workspace_dir. Run as a subprocess with a timeout."""
import inspect
import json
import sys

sys.dont_write_bytecode = True
sys.path[:] = [sys.argv[3]] + [p for p in sys.path[1:] if p != sys.argv[3]]


def plain(x):
    if isinstance(x, (str, int, float, bool)) or x is None:
        return x
    if isinstance(x, dict):
        return {"__dict__": [[plain(k), plain(v)] for k, v in x.items()]}
    if isinstance(x, (list, tuple)):
        return [plain(v) for v in x]
    return {"__repr__": repr(x)[:200]}


def arity(fn):
    try:
        ps = [p for p in inspect.signature(fn).parameters.values()
              if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD, p.VAR_POSITIONAL)]
    except (TypeError, ValueError):
        return 2
    if any(p.kind == p.VAR_POSITIONAL for p in ps):
        return 2
    return 2 if len(ps) >= 2 else 1


def run(mod, probe):
    res = {"id": probe["id"], "res": None, "exc": None}
    try:
        fn = getattr(mod, probe["fn"])
        rec = json.loads(json.dumps(probe["rec"]))
        ctx = dict(probe["ctx"])
        out = fn(rec, ctx) if arity(fn) == 2 else fn(rec)
        res["res"] = plain(out)
    except BaseException as e:  # noqa: BLE001 - the probe records any exception
        res["exc"] = [type(e).__name__, str(e)[:200]]
    return res


def main():
    with open(sys.argv[1]) as f:
        probes = json.load(f)
    out = {"probes": [], "rules": None, "import": None}
    try:
        import rules as mod
    except BaseException as e:  # noqa: BLE001 - an import failure fails every item
        out["import"] = [type(e).__name__, str(e)[:200]]
        mod = None
    if mod is not None:
        for p in probes:
            out["probes"].append(run(mod, p))
        try:
            reg = {}
            for k, v in mod.RULES.items():
                fn, cat = v
                reg[str(k)] = [getattr(fn, "__name__", None), getattr(mod, getattr(fn, "__name__", "") or "", None) is fn,
                               plain(cat)]
            out["rules"] = reg
        except BaseException as e:  # noqa: BLE001
            out["rules"] = {"__error__": [type(e).__name__, str(e)[:200]]}
    with open(sys.argv[2], "w") as f:
        f.write(json.dumps(out, default=repr))


main()
