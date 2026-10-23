# stockroom

A small inventory manager: items (SKUs), stock movements, imports, exports, reports, an HTTP-like request
handler layer and config-driven pipelines. Python 3.9, standard library only.

    python3 -m stockroom --store DIR_OR_FILE.json COMMAND ...

This file documents the current behaviour (release 1.6). `CHANGE.md` describes the next change.

## Model

An **item** has: `sku` (`ABC-1234`: three capital letters, a dash, four digits), `name`, `category`
(lower-case, default `general`), `quantity`, `reorder_level`, `location` (`A1` .. `Z99`, or empty) and
`unit_cost` (money, two decimals). Its value is `quantity x unit_cost`, rounded to cents.

**Quantities** are decimals with at most three decimal places. Text form (store files, CSV, ledger, tables,
messages): plain notation, trailing zeros removed (`12`, `2.5`, `0.125`). JSON form (exports, API, reports):
an integer when whole, else a number. Money is text with two decimals (`"12.50"`) everywhere, JSON included.
Rounding is always half away from zero.

A **movement** records one quantity change: `id` (1, 2, ...), `sku`, `delta` (signed), `reason` (`receive`,
`issue`, `adjust`, `import`, `assemble`) and `note`.

A **kit** is an item assembled from other items: each kit line gives a `component` SKU and the `quantity` of it
one kit takes. A **pack** says how a supplier sells an item: `sku`, `supplier`, `quantity` (how much one pack
holds) and `price` (of one pack).

## Stores

`--store PATH` (else `$STOCKROOM_STORE`, else the config's `store`, default `stockroom-data`). A path ending in
`.json` is a JSON store; anything else is a CSV directory store. `stockroom.storage.factory.open_store(path)`
returns the store; `store.load()` gives an `Inventory` (`.items`: SKU -> Item, `.movements`: list),
`store.save(inv)` writes it.

### CSV directory store

`DIR/items.csv`, `DIR/movements.csv`, `DIR/kits.csv` and `DIR/packs.csv` (only items.csv is required; the kits
and packs files are written once the store has kits or packs). The first line names the file kind and its
schema version, the second is the column header:

    #stockroom-items v1
    sku,name,category,quantity,reorder_level,location,unit_cost
    ABC-0001,Hex bolt,fasteners,120,50,A1,0.12

    #stockroom-movements v1
    id,sku,delta,reason,note
    1,ABC-0001,120,receive,initial stock

    #stockroom-kits v1
    kit,component,quantity
    KIT-0001,ABC-0001,4

    #stockroom-packs v1
    sku,supplier,quantity,price
    ABC-0001,Fastenal,100,9.50

Items are written sorted by SKU, movements in id order, kit lines by kit then component, packs by SKU then
supplier. A first line that is not a stockroom header is an error (`items.csv: not a stockroom file (bad first line)`); a version
the loader cannot read is `items.csv: unsupported schema version N` (same for `movements.csv`); a column header
that does not match the declared version is an error too.

### JSON store

    {"schema": 1, "items": [{"sku": ..., "name": ..., "category": ..., "quantity": "120",
     "reorder_level": "50", "location": "A1", "unit_cost": "0.12"}, ...],
     "movements": [{"id": 1, "sku": ..., "delta": "120", "reason": "receive", "note": "..."}, ...],
     "kits": [{"kit": ..., "component": ..., "quantity": "4"}, ...],
     "packs": [{"sku": ..., "supplier": ..., "quantity": "100", "price": "9.50"}, ...]}

Quantities and amounts are strings; two-space indentation, final newline. A file without a `schema` key is
schema 0 (stockroom < 1.2: `items` is an object keyed by SKU and uses `min` and `cost` for the reorder level
and the unit cost); it is upgraded on load by `storage/migrate.py`. An unreadable version is
`inventory.json: unsupported schema version N`. A save always writes the current version.

`stockroom info [--json]` shows each store file with its version and record count; `stockroom upgrade`
rewrites the store at the current versions.

## Commands

Exit status: 0 success; 1 validation problems (printed as `invalid: <problem>` on stderr) or rejected import
rows; 2 any other error (printed as `error: <message>` on stderr) and usage errors.

| command | what it does |
|---|---|
| `init` | create an empty store |
| `add SKU NAME [--category C] [--qty Q] [--reorder R] [--location L] [--cost C]` | add an item; an initial quantity is recorded as a `receive` movement noted `initial stock`; prints `added SKU (Q)` |
| `list [--category C] [--search TEXT] [--sort sku\|name\|quantity\|value] [--json]` | items (by SKU; by name; by quantity, ties by SKU; by value, highest first); text columns SKU NAME CATEGORY QTY REORDER LOCATION COST; `--json`: a list of item objects |
| `show SKU` | one item object (JSON) |
| `remove SKU`, `relocate SKU LOC` | remove an item / change its location |
| `categories [--json]`, `rename-category OLD NEW` | categories with item counts / move items between categories |
| `receive SKU QTY [--note N]` | add stock; prints `SKU: <new qty> (+<delta>)` |
| `issue SKU QTY [--note N]` | take stock out; more than the stock is `error: insufficient stock for SKU: have H, need N` |
| `adjust SKU QTY [--note N]` | set the counted quantity; records the difference (nothing when equal) |
| `count FILE [--dry-run]` | apply a count sheet (CSV `sku,counted[,note]`): an `adjust` per row, note `count` by default; bad rows are `FILE:LINE: message` on stderr (exit 1), the others apply |
| `history SKU [--json \| --csv]` | the item's movements (text table ID REASON DELTA NOTE; `--csv`: columns `id,sku,delta,reason,note`) |
| `undo SKU` | revert the item's latest movement: records its opposite as an `adjust` noted `undo #ID`; prints `SKU: <new qty> (<delta>), undid #ID` |
| `transfer SKU QTY --to STORE` | move stock to another (existing) store: an `issue` here noted `transfer to STORE`, a `receive` there noted `transfer from <this store>`; a SKU the target lacks is created there with the same fields; prints `SKU: <qty> here, <qty> at STORE` |
| `kit-define KIT COMPONENT=QTY ...` | set a kit's lines (KIT and the components must be items) |
| `kit-show KIT [--json]` | the kit's lines and how many kits the stock allows: `KIT <name>: can assemble N`, then `  COMPONENT <name>: QTY` per line |
| `assemble KIT N` | take N x each line's quantity of every component (`assemble` movements noted `kit KIT x N`) and add N to the kit item; all or nothing; prints `assembled N x KIT: KIT now <qty>` |
| `pack-set SKU SUPPLIER QTY PRICE`, `packs SKU [--json]` | record / show the packs of an item (text: `SUPPLIER  QTY per pack  PRICE`) |
| `reorder-mail` | the reorder list (the `reorder` report's items) as a mail body, with the cheapest pack for each |
| `import FILE [--format F] [--mode add\|replace]` | import items (below) |
| `export FILE\|- [--format F] [--category C]` | export items (below) |
| `report NAME [--json]` | a report (below) |
| `validate` | run the configured rulesets; prints each problem, then `ok` or exits 1 |
| `run PIPELINE.json` | run a pipeline (below) |
| `stats [--json]`, `info [--json]`, `upgrade` | summary numbers / store files / rewrite at current versions |

Item object (JSON, `show`, `list --json`, the API):
`{"sku", "name", "category", "quantity", "reorder_level", "location", "unit_cost", "value"}` in that order.
Movement object: `{"id", "sku", "delta", "reason", "note"}`. Kit object (`kit-show --json`, `GET /kits/KIT`):
`{"kit", "name", "capacity", "lines": [{"component", "name", "quantity"}, ...]}`. Pack object:
`{"sku", "supplier", "quantity", "price"}`.

The reorder mail:

    Reorder list (2 items)
    CHK-0002 Chickpeas: order 7.5 (have 2.5, reorder level 5)
      cheapest: 1 x Pulse Co pack of 10 = 32.00
    LNT-0007 Lentils: order 2.75 (have 1.25, reorder level 2)
      no pack on file

For each item it needs (by SKU), the order quantity is the `reorder` report's; for each of its packs, the
number of packs is the order quantity divided by the pack quantity, rounded up, and the cost that number times
the pack price; the cheapest pack is shown (ties: supplier name).

## Importers and exporters

Formats are looked up by name in the registry. Without `--format`, the file extension decides
(`.csv` csv, `.tsv` tsv, `.json` json, `.jsonl` jsonl, `.ledger`/`.txt` ledger).

**csv** (import and export). Import: a header row (names case-insensitive; `qty`, `reorder`, `min`, `cost`,
`price`, `loc`, `bin`, `description` are accepted aliases), `sku` and `quantity` required, unknown columns
ignored; the header is line 1. Export: header `sku,name,category,quantity,reorder_level,location,unit_cost`.

**json** (import and export) and **jsonl** (import). Import: a list of objects or `{"items": [...]}` (json; an
error's line is the object's 1-based position) or one object per line (jsonl; the physical line). Keys `sku`
and `quantity` (or `qty`) required. Export: a list of objects with keys
`sku, name, category, quantity, reorder_level, location, unit_cost` in that order (quantities as numbers).

**ledger** (import and export): the hand-kept text format.

    [HEX-0001]
    name = Hex bolt M6
    qty = 120
    category = fasteners
    reorder = 50
    location = A1
    cost = 0.12

Import: `qty` required, keys case-insensitive, `#`/`;` comments; an unknown key or a line without `=` is an
error at its line; a problem of the record as a whole (bad SKU, missing qty, a bad value) is an error at its
`[SKU]` line. Export: a `# stockroom ledger export` line, then per item a blank line and the keys in the order
above (`location` left out when empty).

**tsv** (export only): the picking list, columns `LOCATION SKU NAME QTY`, sorted by location then SKU, `-` for
no location.

More exporters (export only, described at the top of their modules in `stockroom/exporters/`): `labels`
(shelf labels, text), `shop` (the web shop's product CSV), `snapshot` (on-hand quantities and values, CSV) and
`markdown` (a wiki table; `.md`). Marketplace feeds (export only, always with `--format`): `mkt-bazaar`, `mkt-cartwheel`, `mkt-dockside`, `mkt-emporium`, `mkt-fairground`, `mkt-gallery`, `mkt-hub`, `mkt-isle`, `mkt-jetty`, `mkt-kiosk`, `mkt-ledger-shop`, `mkt-market-row`, each in
the format its marketplace imports (described at the top of each module in `stockroom/exporters/markets/`).

**Partner feeds** (import only, always with `--format`): `vendor-acme`, `vendor-brightline`, `vendor-cargo`,
`vendor-delta`, `vendor-elm`, `vendor-fjord`, `vendor-granite`, `vendor-harbor`, `vendor-iris`,
`vendor-juniper`, `vendor-kestrel`, `vendor-lark`, `vendor-maple`, `vendor-nimbus`, `vendor-orchid`, `vendor-pelican`, `vendor-quarry`, `vendor-raven`, `vendor-sierra`, `vendor-tundra`, `vendor-umber`, `vendor-vale`, `vendor-willow`, `vendor-xenia`, `vendor-yarrow`, `vendor-zephyr`, `vendor-alder`, `vendor-birch`, `vendor-cedar`, `vendor-dune`, `vendor-ember`, `vendor-flint`, `vendor-gale`, and `vendor-heron` read the stock files those suppliers send, each in its own dialect (described at the
top of each module in `stockroom/importers/vendors/`). Their unit-of-measure column holds UN/ECE Recommendation 20 codes; stockroom
only stocks counted goods from partners, so a code other than `EA`, `C62` or `H87` (or blank) is the row error
`unsupported unit of measure 'KGM'`.

**Import merging.** Rows are first checked with the item rules; a failing row is an error
`FILE:LINE: <field>: <message>`. A new SKU is added. Mode `add` (default) adds the row's quantity to the
existing item; mode `replace` replaces the item. Every quantity change records an `import` movement whose note
is the file name. Errors are printed by line; the command prints `imported N items, M errors` and exits 1 when
M > 0.

## Validation

Rulesets (registry names, run in the order of the config's `validate.rulesets`):
- `items`: sku format (`must look like ABC-1234`), name not blank / at most 60 characters, category
  lower-case, quantity, reorder level and unit cost not negative (`must not be negative`), location format
  (`must look like A1 or B12`).
- `movements`: known reason, non-zero delta.
- `inventory`: duplicate movement ids, movements of unknown SKUs.
- `kits`: the kit item exists (`kit KIT-0001: unknown kit item`), every component exists
  (`kit KIT-0001 line ABC-0001 component: unknown sku ABC-0001`) with a positive quantity.

A problem prints as `<subject> <field>: <message>`, e.g. `ABC-0001 quantity: must not be negative` or
`movement 3 reason: unknown reason 'lost'`.

## Reports

`report NAME [--json]`; JSON form `{"report": NAME, "rows": [{...}, ...], ...}`; the text form is a table of
the same columns.

| name | rows | order |
|---|---|---|
| `totals` | `category, items, quantity, value` | category |
| `low-stock` | items with quantity <= reorder level: `sku, name, location, quantity, reorder_level, shortfall` | location, then SKU |
| `valuation` | `location, items, value`, plus `"total"` | location (`-` for none) |
| `movements` | `sku, movements, received, issued, net` | SKU |
| `reorder` | items at or below a positive reorder level: `sku, name, quantity, reorder_level, order` (order = 2 x reorder level - quantity) | SKU |
| `kits` | `kit, name, lines, capacity` (how many kits the stock allows; 0 when a component is missing) | kit |
| `by-location` | items with a location: `location, sku, name, quantity` | location, then SKU |
| `inbound` | per SKU, the `receive` and `import` movements: `sku, receipts, quantity` | SKU |
| `slow-movers` | items in stock nothing was issued or assembled from: `sku, name, quantity, value` | value (highest first), then SKU |
| `by-aisle` | per aisle (the location's letter): `aisle, items, quantity, value` (sums) | aisle |
| `by-family` | per family (the SKU's letters): `family, items, quantity, value` (sums) | family |
| `by-initial` | per initial (of the name): `initial, items, quantity, value` (sums) | initial |
| `by-shelf` | per shelf (the location's number): `shelf, items, quantity, value` (sums) | shelf |
| `by-cost-band` | per band (`under 1`, `1-10`, `10+` by unit cost): `band, items, quantity, value` (sums) | band |
| `by-status` | per status (`low` / `ok`): `status, items, quantity, value` (sums) | status |

## Request handlers

    from stockroom.api.app import App
    from stockroom.api.http import Request
    resp = App(open_store(path)).handle(Request("POST", "/items/ABC-0001/receive", body='{"qty": 5}'))
    resp.status, resp.body, resp.content_type

| route | answer |
|---|---|
| `GET /items?q=&category=&sort=&offset=&limit=` | `{"items": [...], "total": N}` (`sort` as `list --sort`) |
| `POST /items` (item fields) | 201 and the item; the initial quantity is a `receive` movement |
| `GET /items/SKU`, `DELETE /items/SKU` | the item / 204 |
| `PATCH /items/SKU` | changes `name, category, reorder_level, location, unit_cost`; other fields are 400 |
| `POST /items/SKU/receive`, `/issue`, `/adjust` (`{"qty": ..., "note": ...}`) | `{"item": ..., "movement": ...}` |
| `GET /items/SKU/movements` | `{"movements": [...]}` |
| `POST /items/SKU/undo` | as `undo`: `{"item": ..., "movement": ...}` |
| `GET /items/SKU/packs` | `{"packs": [...]}` |
| `GET /kits/KIT`, `POST /kits/KIT/assemble` (`{"count": N}`) | the kit object / `{"kit": <kit item>, "movements": [...]}` |
| `GET /reports/NAME` | the report's JSON form |
| `GET /export?format=F&category=C` | the exporter's text |
| `POST /import?format=F&mode=M` (body: the file's text) | `{"imported": N, "errors": [{"line": L, "message": M}]}` |
| `GET /validate`, `GET /stats`, `GET /info` | problems / summary numbers / store files |
| `GET /v1/stock/SKU`, `POST /v1/stock/SKU/scan` (`{"delta": N}`) | the handheld scanners' legacy routes: `{"sku", "qty", "location"}`; a scan receives (N > 0) or issues (N < 0) with the note `scanner` |

Errors are `{"error": message}`: unknown route 404, wrong method 405, bad JSON or a bad value 400, unknown SKU
404, rule violations 422 (`{"error": "invalid item", "problems": [...]}`), duplicates and insufficient stock
409, unknown report 404.

## Pipelines and the registry

`stockroom run PIPELINE.json`:

    {"input":  {"format": "csv", "path": "in.csv"}                 (or {"store": true})
     "steps":  [{"step": "where", "field": "category", "equals": "bolts"},
                {"step": "relocate", "to": "B2"}],
     "output": {"format": "json", "path": "out.json"}}            (or {"report": "totals", "path": "-"})

Paths are relative to the pipeline file; `-` is standard output; `format` may be left out when the extension
implies it. Steps: `where` (`field` among sku, name, category, location; `equals` or `in`), `relocate`,
`set_category`, `drop_zero`, `sort` (`by` among sku, name, category, location, quantity; ties by SKU;
`reverse`). Importer row errors are printed and make the exit status
1; the rest of the pipeline still runs. A report output writes the report's JSON form (`"as": "text"` for the
table).

Importers, exporters, reports, steps and rulesets are found by name in `stockroom/registry/builtin.py`; a config
file (`--config FILE`, JSON) can add or replace entries under `"plugins"`, and sets `csv.delimiter`,
`import.mode`, `export.format` and `validate.rulesets`.

## Tests

    /usr/bin/python3 -m unittest discover -s tests
