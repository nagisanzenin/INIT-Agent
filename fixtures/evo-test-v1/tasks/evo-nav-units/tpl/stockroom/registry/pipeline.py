"""Pipelines: a JSON description of input -> steps -> output, run by `stockroom run FILE`.

    {
      "input":  {"format": "csv", "path": "in.csv", "options": {...}}      or {"store": true}
      "steps":  [{"step": "where", "field": "category", "equals": "bolts"}, ...],
      "output": {"format": "json", "path": "out.json", "options": {...}}
             or {"report": "totals", "path": "totals.json", "as": "json" | "text"}
    }

Paths are relative to the pipeline file's directory; an output path "-" means standard output. "format" may
be left out when the path's extension implies it (stockroom.formats). Every name -- importer, step, exporter,
report -- is looked up in the registry. Rows the importer rejects are reported as errors and dropped; the rest
of the pipeline still runs.
"""
import json
import os

from ..errors import ConfigError, StorageError
from ..formats import guess_format
from ..models.inventory import Inventory
from . import builtin


class PipelineResult:
    def __init__(self):
        self.count_in = 0
        self.count_out = 0
        self.errors = []
        self.stdout = ""


def load_pipeline(path) -> dict:
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
    except OSError as e:
        raise ConfigError("cannot read pipeline %s: %s" % (path, e.strerror or e))
    except ValueError as e:
        raise ConfigError("pipeline %s is not valid JSON: %s" % (path, e))
    if not isinstance(data, dict):
        raise ConfigError("pipeline %s must be a JSON object" % path)
    return data


def run_pipeline(config, base_dir=".", store=None) -> PipelineResult:
    res = PipelineResult()
    inp = config.get("input")
    if not isinstance(inp, dict):
        raise ConfigError("pipeline: missing input")
    movements = []
    if inp.get("store"):
        if store is None:
            raise ConfigError("pipeline: input store needs a store")
        inv = store.load()
        items, movements = inv.sorted_items(), list(inv.movements)
    else:
        path = inp.get("path")
        if not path:
            raise ConfigError("pipeline: input needs a path")
        fmt = inp.get("format") or guess_format(path)
        if not fmt:
            raise ConfigError("pipeline: cannot tell the input format of %s" % path)
        parse = builtin.importer(fmt)
        try:
            with open(os.path.join(base_dir, path), encoding="utf-8", newline="") as f:
                text = f.read()
        except OSError as e:
            raise StorageError("cannot read %s: %s" % (path, e.strerror or e))
        result = parse(text, inp.get("options") or {})
        items = result.items
        res.errors = result.error_lines(path)
    res.count_in = len(items)
    for spec in config.get("steps") or []:
        if not isinstance(spec, dict) or "step" not in spec:
            raise ConfigError("pipeline: every step needs a \"step\" name")
        params = {k: v for k, v in spec.items() if k != "step"}
        items = builtin.step(spec["step"])(items, params)
    res.count_out = len(items)
    out = config.get("output")
    if not isinstance(out, dict):
        raise ConfigError("pipeline: missing output")
    if "report" in out:
        inv = Inventory()
        for item in items:
            inv.put(item)
        inv.movements = movements
#@B
        report = builtin.report(out["report"])(inv, {})
#@A
        report = builtin.report(out["report"])(inv, {"unit": out.get("unit")})
#@E
        if out.get("as", "json") == "text":
            text = report.to_text()
        else:
            text = json.dumps(report.to_json(), indent=2) + "\n"
    else:
        path = out.get("path", "-")
        fmt = out.get("format") or guess_format(path)
        if not fmt:
            raise ConfigError("pipeline: cannot tell the output format of %s" % path)
        text = builtin.exporter(fmt)(items, out.get("options") or {})
    target = out.get("path", "-")
    if target == "-":
        res.stdout = text
    else:
        with open(os.path.join(base_dir, target), "w", encoding="utf-8", newline="") as f:
            f.write(text)
    return res
