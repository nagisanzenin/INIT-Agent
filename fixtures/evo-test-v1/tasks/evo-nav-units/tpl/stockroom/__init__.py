"""stockroom: a small inventory manager (items, stock movements, imports, exports, reports).

Layers, bottom to top (a module only imports from its own layer or the ones above it in this list):

    errors, util       shared exceptions and helpers (quantities, money, text tables)
    models             Item, Movement, Inventory
    storage            the CSV-directory store and the JSON-file store, schema versions, migrations
    importers          parse external files into items (csv, json, jsonl, ledger)
    exporters          write items to external files (csv, tsv picking list, json, ledger)
    validate           item, movement and inventory rules, grouped in named rulesets
    services           stock operations (receive, issue, adjust), the catalog, import merging
    reports            totals, low-stock, valuation, movement summary
    registry           the plugin tables (importers, exporters, reports, steps, rulesets) and pipelines
    api                HTTP-like request handlers: pure functions from a Request to a Response
    cli                the `stockroom` command line (python3 -m stockroom)
"""

__version__ = "1.6.0"
