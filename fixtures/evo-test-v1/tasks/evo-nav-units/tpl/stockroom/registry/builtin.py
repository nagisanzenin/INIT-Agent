"""The built-in plugin tables, and the lookups everything else goes through.

    importer  fn(text, options) -> importers.base.ImportResult
    exporter  fn(items, options) -> str
    report    fn(inventory, options) -> reports.base.Report
    step      fn(items, params) -> items           (pipeline steps, see registry/steps.py)
    ruleset   fn(inventory, config) -> [Problem]

A config's "plugins" section adds entries or replaces built-in ones: {"exporter": {"xml": "pkg.mod:fn"}}.
"""
from ..errors import ConfigError
from .core import Registry

BUILTINS = {
    "importer": {
        "csv": "stockroom.importers.csv_import:parse_csv",
        "json": "stockroom.importers.json_import:parse_json",
        "jsonl": "stockroom.importers.json_import:parse_jsonl",
        "ledger": "stockroom.importers.ledger_import:parse_ledger",
        "vendor-acme": "stockroom.importers.vendors.acme:parse_acme",
        "vendor-brightline": "stockroom.importers.vendors.brightline:parse_brightline",
        "vendor-cargo": "stockroom.importers.vendors.cargo:parse_cargo",
        "vendor-delta": "stockroom.importers.vendors.delta:parse_delta",
        "vendor-elm": "stockroom.importers.vendors.elm:parse_elm",
        "vendor-fjord": "stockroom.importers.vendors.fjord:parse_fjord",
        "vendor-granite": "stockroom.importers.vendors.granite:parse_granite",
        "vendor-harbor": "stockroom.importers.vendors.harbor:parse_harbor",
        "vendor-iris": "stockroom.importers.vendors.iris:parse_iris",
        "vendor-juniper": "stockroom.importers.vendors.juniper:parse_juniper",
        # partner feeds: generated
    },
    "exporter": {
        "csv": "stockroom.exporters.csv_export:write_csv",
        "tsv": "stockroom.exporters.csv_export:write_tsv",
        "json": "stockroom.exporters.json_export:write_json",
        "ledger": "stockroom.exporters.ledger_export:write_ledger",
        "labels": "stockroom.exporters.labels:write_labels",
        "shop": "stockroom.exporters.shop:write_shop",
        "snapshot": "stockroom.exporters.snapshot:write_snapshot",
        "markdown": "stockroom.exporters.markdown:write_markdown",
        # marketplaces: generated
    },
    "report": {
        "totals": "stockroom.reports.totals:build",
        "low-stock": "stockroom.reports.low_stock:build",
        "valuation": "stockroom.reports.valuation:build",
        "movements": "stockroom.reports.movements:build",
        "reorder": "stockroom.reports.reorder:build",
        "kits": "stockroom.reports.kits:build",
        "by-location": "stockroom.reports.by_location:build",
        "inbound": "stockroom.reports.inbound:build",
        "slow-movers": "stockroom.reports.slow_movers:build",
        # grouped reports: generated
    },
    "step": {
        "where": "stockroom.registry.steps:where",
        "relocate": "stockroom.registry.steps:relocate",
        "set_category": "stockroom.registry.steps:set_category",
        "drop_zero": "stockroom.registry.steps:drop_zero",
        "sort": "stockroom.registry.steps:sort_items",
#@A
        "to_unit": "stockroom.registry.steps:to_unit",
#@E
    },
    "ruleset": {
        "items": "stockroom.validate.item_rules:check_items",
        "movements": "stockroom.validate.movement_rules:check_movements",
        "inventory": "stockroom.validate.inventory_rules:check_inventory",
        "kits": "stockroom.validate.kit_rules:check_kits",
    },
}

_REGISTRIES = {}


def _boot():
    for kind, table in BUILTINS.items():
        reg = Registry(kind)
        for name, target in table.items():
            reg.register(name, target)
        _REGISTRIES[kind] = reg


_boot()


def registry(kind) -> Registry:
    try:
        return _REGISTRIES[kind]
    except KeyError:
        raise ConfigError("unknown plugin kind: %s" % kind) from None


def install_plugins(config) -> None:
    """Registers the config's "plugins" entries (replacing built-ins of the same name)."""
    plugins = (config or {}).get("plugins") or {}
    if not isinstance(plugins, dict):
        raise ConfigError("plugins must be an object")
    for kind in sorted(plugins):
        table = plugins[kind]
        if not isinstance(table, dict):
            raise ConfigError("plugins.%s must be an object" % kind)
        for name in sorted(table):
            registry(kind).register(name, table[name], replace=True)


def importer(name):
    return _REGISTRIES["importer"].get(name)


def exporter(name):
    return _REGISTRIES["exporter"].get(name)


def report(name):
    return _REGISTRIES["report"].get(name)


def step(name):
    return _REGISTRIES["step"].get(name)


def ruleset(name):
    return _REGISTRIES["ruleset"].get(name)
