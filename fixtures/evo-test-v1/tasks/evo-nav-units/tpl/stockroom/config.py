"""Configuration: built-in defaults, overridden by an optional JSON file (`--config FILE`).

    {
      "store": "stockroom-data",              where the CLI finds the store (--store and $STOCKROOM_STORE win)
      "csv": {"delimiter": ","},              options of the csv importer and exporters
      "import": {"mode": "add"},              default merge mode of `import` (add | replace)
      "export": {"format": "csv"},            default exporter when the file extension says nothing
      "validate": {"rulesets": [...]},        the rulesets `validate` runs, in order
      "plugins": {"exporter": {"name": "module:function"}, ...}   extra registry entries
    }

Nested objects are merged key by key; any other value replaces the default.
"""
import copy
import json

from .errors import ConfigError

DEFAULTS = {
    "store": "stockroom-data",
    "csv": {"delimiter": ","},
    "import": {"mode": "add"},
    "export": {"format": "csv"},
    "validate": {"rulesets": ["items", "movements", "inventory", "kits"]},
    "plugins": {},
}


def deep_merge(base, override):
    out = copy.deepcopy(base)
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = deep_merge(out[key], value)
        else:
            out[key] = copy.deepcopy(value)
    return out


def load_config(path=None):
    """DEFAULTS merged with the JSON object in `path` (if any)."""
    if not path:
        return copy.deepcopy(DEFAULTS)
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
    except OSError as e:
        raise ConfigError("cannot read config %s: %s" % (path, e.strerror or e))
    except ValueError as e:
        raise ConfigError("config %s is not valid JSON: %s" % (path, e))
    if not isinstance(data, dict):
        raise ConfigError("config %s must be a JSON object" % path)
    return deep_merge(DEFAULTS, data)
