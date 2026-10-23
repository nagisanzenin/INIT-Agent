"""Pipeline config checks (README.md, "Pipelines"). Everything wrong with a config is a ConfigError."""
import json

from roomkit.errors import ConfigError

TOP_KEYS = ("input", "rooms", "steps", "output", "report")


def _need_dict(value, what):
    if not isinstance(value, dict):
        raise ConfigError("%s must be an object" % what)
    return value


def _need_text(d, key, what):
    v = d.get(key)
    if not isinstance(v, str) or not v:
        raise ConfigError("%s needs \"%s\"" % (what, key))
    return v


def check_pipeline(cfg):
    _need_dict(cfg, "a pipeline config")
    for key in cfg:
        if key not in TOP_KEYS:
            raise ConfigError("unknown key '%s'" % key)
    inp = _need_dict(cfg.get("input"), "input")
    if "store" in inp:
        _need_text(inp, "store", "input")
    else:
        _need_text(inp, "format", "input")
        _need_text(inp, "path", "input")
    if "rooms" in cfg and (not isinstance(cfg["rooms"], str) or not cfg["rooms"]):
        raise ConfigError("rooms must be a store path")
    steps = cfg.get("steps", [])
    if not isinstance(steps, list):
        raise ConfigError("steps must be a list")
    for i, spec in enumerate(steps, 1):
        if not isinstance(spec, dict) or not isinstance(spec.get("step"), str):
            raise ConfigError("step %d needs \"step\"" % i)
    if ("output" in cfg) == ("report" in cfg):
        raise ConfigError("give exactly one of \"output\" and \"report\"")
    if "output" in cfg:
        out = _need_dict(cfg["output"], "output")
        _need_text(out, "format", "output")
        _need_text(out, "path", "output")
        _need_dict(out.get("options", {}), "output options")
    else:
        rep = _need_dict(cfg["report"], "report")
        _need_text(rep, "name", "report")
        _need_text(rep, "path", "report")
        _need_dict(rep.get("options", {}), "report options")
    return cfg


def load_config(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            cfg = json.load(f)
    except OSError as e:
        raise ConfigError("%s: %s" % (path, e.strerror or e))
    except ValueError as e:
        raise ConfigError("%s: bad JSON (%s)" % (path, e))
    return check_pipeline(cfg)


def text_options(options):
    """Option values as text (configs may give numbers)."""
    return {k: v if isinstance(v, str) else json.dumps(v) for k, v in (options or {}).items()}
