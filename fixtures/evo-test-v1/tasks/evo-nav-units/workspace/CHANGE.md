# CHANGE 1.7: units of measure

Status: approved. Until now a quantity was a bare number in an implicit unit. From 1.7 every item has a
**unit of measure**, and the whole package -- every file format, store, command, request handler, report,
registry entry and pipeline step -- has to respect it. This document states the rules; it does not list the
places they apply to. Everything it does not change keeps its 1.6 behaviour (README.md): every existing key,
column, message, exit status and order.

**What is a quantity.** An item's quantity and reorder level, a movement's delta, a kit line's quantity, a
pack's quantity, a quantity given to a command or a request, and every value computed from those (sums,
shortfalls, order quantities, converted values). Not quantities: counts of things (items, movements, lines,
kits, packs, capacity), money (unit cost, value, price) and ids.

## 1. Units

| code | dimension | factor (base unit of the dimension) |
|---|---|---|
| `ea` | count | 1 (base) |
| `dz` | count | 12 |
| `g` | mass | 1 (base) |
| `kg` | mass | 1000 |
| `lb` | mass | 453.59237 |
| `oz` | mass | 28.349523125 |
| `ml` | volume | 1 (base) |
| `l` | volume | 1000 |

New module `stockroom/units.py`, with at least:

- `UNITS`: the table above, code -> (dimension, factor).
- `normalize_unit(text) -> str`: surrounding whitespace removed, lower-cased; `None` or a blank string gives
  the default unit `ea`; anything not in the table raises `UnitError("unknown unit 'xx'")` where `xx` is the
  text as given, trimmed (Python `%r` of the trimmed string).
- `convert(qty, from_unit, to_unit) -> Decimal`: `qty x factor(from) / factor(to)`, rounded to three decimal
  places, half away from zero. Converting between dimensions raises `UnitError("cannot convert kg to ea")`
  (from, then to). Converting a unit to itself returns the quantity unchanged.

`UnitError` is added to `stockroom/errors.py` and is a `StockroomError`.

Unit codes are accepted in any case and with surrounding spaces wherever they are read, and are always stored,
written and shown in canonical lower-case form. A missing or blank unit means `ea` -- in old files, in
inputs that create items, and in request bodies -- except where a rule below says "the item's own unit".

**Converting an item** into another unit of its dimension: quantity and reorder level with `convert`; the unit
cost becomes the price of one new unit, `unit_cost x factor(new) / factor(old)` rounded to cents half up. In
reports, an item's value is always computed from its stored quantity and unit cost.

## 2. Where units go

- `Item.unit` (default `"ea"`): an item's quantity and reorder level are in it, its unit cost is per one of it.
  In an item's field order `unit` comes right after `quantity`:
  `sku, name, category, quantity, unit, reorder_level, location, unit_cost`.
- Movements, kit lines and packs carry the unit their quantity is in (`unit`, default `"ea"`), right after
  their quantity field. A movement is always recorded in its item's unit at the time it is recorded. A kit line
  or a pack keeps the unit it was defined with (it is not converted when defined); defined without a unit it is
  in the item's own unit at that time. Defining one in a unit of another dimension than its item's is refused
  (section 5).
- **JSON.** Every JSON object the package emits (command output, request handler bodies, exports, store files,
  report rows) that holds one or more quantities gets a `"unit"` key -- the unit of those quantities --
  placed right after the first quantity key in the object's key order.
- **Delimited files.** Every delimited file the package writes (CSV, TSV, ...) that has a quantity column gets a
  unit column right after its first quantity column, named `unit` -- `UNIT` when every name in its header is
  upper-case.
- **Ledger.** The ledger format gets a `unit = CODE` line right after each record's `qty` line.
- **Text.** Every quantity the package writes as text -- messages, tables, sheets, mails, and every export
  that is neither delimited nor JSON nor ledger -- is followed by a space and its unit code (`2.5 kg`,
  `+0.5 kg`, `12 ea`). Two exceptions:
  report text tables, which show the unit as the report's `unit` column; and error messages, which keep their
  1.6 wording with the numbers in the item's unit (`insufficient stock for FLR-0001: have 2, need 2.5`).

## 3. Stores and schema versions

Every kind of store file moves to schema version **2**: each file of the CSV directory store, and
inventory.json. Its records carry units as section 2 says.

- Loading accepts the old versions and the new one. Everything read from an old version is in `ea`. In a
  version-2 file a blank cell, a missing key or `null` also means `ea`; a unit code is normalized (case,
  spaces), but a store does **not** reject unknown codes on load -- `validate` reports them.
- Each file carries its own version: a directory store whose files are at different versions loads.
- A save always writes version 2 for every file it writes, whatever was loaded (so any command that saves
  upgrades the store, and `info`/`upgrade` treat the old versions as outdated).
- Versions above 2 are rejected with the existing message (`items.csv: unsupported schema version 3`).

## 4. Reading quantities

- **Importers** accept a unit per row; a missing or blank one is `ea`; an unknown one makes that row an error
  at its line (ledger: the record's `[SKU]` line, as for any bad value) with the message `unknown unit 'xx'`;
  the other rows still import.
  - csv: an optional `unit` column; the header alias `uom` also names it.
  - json and jsonl: an optional `"unit"` key (`null` means `ea`).
  - ledger: an optional `unit = CODE` line, and the `qty` value may carry the unit after the number, with or
    without a space (`qty = 2.5 kg`, `qty = 750g`, `qty = 12 EA`). When both are given and differ (after
    normalization) the record is an error at its `[SKU]` line: `unit mismatch (kg vs g)` -- the `qty` unit
    first.
  - Partner feeds now accept goods measured in any unit: their Recommendation 20 codes map as `EA`, `C62`,
    `H87` -> `ea`; `DZN` -> `dz`; `GRM` -> `g`; `KGM` -> `kg`; `LBR` -> `lb`; `ONZ` -> `oz`; `MLT` -> `ml`;
    `LTR` -> `l` (codes in any case). A blank code is `ea`; any other code is the row error `unknown unit 'xx'`
    (as given, trimmed).
- **Quantities for an existing item.** Every command that takes a quantity for an existing item takes
  `--unit U`, and every request body that gives one accepts `"unit"`; the quantity is converted from
  U into the item's unit before it is used (`pack-set` excepted: a pack keeps its unit, section 2). Without a
  unit it is in the item's own unit. A count sheet may have a `unit` column (blank: the item's own unit).
  `kit-define` lines may carry the unit after the number (`FLR-0001=250g`, `FLR-0001=0.25kg`).
- **New items.** `add --unit U` and `"unit"` in `POST /items` give the new item's unit (default `ea`).

## 5. Where units meet

Whenever quantities in different units meet, each is first converted into the unit of the item it applies to
(with `convert`, so rounded to three places), and only then added, multiplied, divided, summed or compared:

- receiving, issuing, adjusting, counting, transferring and undoing in another unit (a transfer arrives in
  the target store's item unit; an undo reverts a movement recorded in an older unit);
- importing into an existing item: mode `add` adds the converted quantity and keeps the item's unit; mode
  `replace` replaces the item, unit included, and its `import` movement's delta is the new quantity minus the
  old quantity converted into the new unit; in both modes a row of another dimension than the existing item is
  the error `cannot convert <row unit> to <item unit>`;
- kits: one kit takes the line quantity converted into the component's unit; N kits take N times that; the
  capacity is the component quantity divided by that, rounded down, over the lowest line (the `kits` report
  shows capacity 0 for a kit with a line it cannot convert, as for a missing component);
- packs: the number of packs is the order quantity divided by the pack quantity converted into the item's
  unit, rounded up; a pack of another dimension than its item is never offered.

Quantities of different dimensions never meet: anything that would convert between them fails with
`cannot convert X to Y` where X is the unit of the incoming quantity (the row, the argument, the kit line or
pack being defined or used, the movement) and Y the unit of the item it applies to (for a transfer's arrival:
the source item's unit, then the target item's). As a command: exit 2, `error: <message>`; as a request:
422 `{"error": "<message>"}` (every `UnitError` answers 422); as a row of an import or a count sheet: an error
at its line, the other rows still apply.

**Re-checking.** Every operation that changes stock or an item's unit checks the item rules (section 6) on
every item it would change, before it changes anything. If one breaks a rule, the whole operation is refused
and nothing changes: a command prints `invalid: <problem>` and exits 1; a request answers 422
`{"error": "invalid item", "problems": [...]}`; an import or count-sheet row is an error `<field>: <message>`
at its line (the other rows still apply).

## 6. Rules

- Ruleset `items`: an item's `unit` must be a known code -- `ABC-0001 unit: unknown unit 'stone'`. For the
  count units (`ea`, `dz`) the quantity and the reorder level, expressed in `ea`, must be whole numbers --
  `ABC-0001 quantity: must be a whole number of ea` (and `reorder_level` likewise): `1.5 dz` is fine, `0.1 dz`
  and `2.5 ea` are not. Mass and volume quantities may be fractional.
- Rulesets `movements` and `kits`: a movement or kit line must carry a known code, and one of its item's
  dimension when that item exists and has a known unit: `movement 4 unit: unknown unit 'xx'`,
  `movement 5 unit: kg does not match item unit ea`, `kit KIT-0001 line FLR-0001 unit: unknown unit 'xx'`,
  `kit KIT-0001 line FLR-0001 unit: ml does not match item unit kg`.

## 7. Reports

The reports that show quantities show the unit of every row (section 2). They accept a unit option (CLI
`report NAME --unit U`, `GET /reports/NAME?unit=U`, a pipeline report output's `"unit"`) and their JSON form
gets a top-level `"unit"` key: the normalized option, or `null`. An unknown option is an error (exit 2 /
422). Reports without quantities ignore the option entirely (an unknown one included) and do not get the key.

- Reports that sum the quantities of several items per group (totals per category, and every other grouped
  report): without a unit, one row per (group, unit) pair, sorted by group then unit; quantities are only
  summed within one unit. With a unit U: only items of U's dimension count, each item's quantity converted into
  U before summing, one row per group; the number of items left out is the top-level `"skipped"` (0 without a
  unit). Values do not change.
- Reports with one row per item: selection and order do not change. With a unit U, the rows of items of U's
  dimension show their quantities converted into U, each value converted on its own; the other rows keep their
  own unit.
- Reports that sum movement deltas per SKU: every delta is converted from the unit it was recorded in into the
  row's unit before summing. A row's own unit is its item's current unit or, for a SKU no longer in the
  inventory, the unit of its latest movement among those the report sums. The row's unit is that own unit --
  or U, when a unit U is given and the own unit is of U's dimension. A delta of another dimension than the
  row's unit is left out (not counted).

## 8. Everything else

- Export: every export path takes an optional target unit (`export --unit U`, `GET /export?unit=U`) that
  converts the exported items of U's dimension into U (section 1); other items are exported unchanged; the
  store does not change. The pipeline step `to_unit` (`{"step": "to_unit", "unit": "lb"}`) does the same to
  the items passing through it. An unknown U is an error.
- `PATCH /items/SKU` accepts `"unit"`: the item is converted into the new unit first (section 1), then the
  other fields of the same body are applied as given (in the new unit). No movement is recorded.
- The pipeline `where` step can filter on `unit`; the wanted values are normalized.
- Ordering by quantity (`list --sort quantity`, `GET /items?sort=quantity`, the `sort` step with
  `"by": "quantity"`): items of count units first, then mass, then volume, then unknown codes; within a
  dimension by the quantity in the base unit (`ea`, `g`, `ml`); ties by SKU; `reverse` reverses the whole
  order.
- `stats --json` and `GET /stats` get `"units"`: unit code -> number of items, keys sorted.
- An unknown unit given to a command or in a request (arguments, options, query parameters, body keys) is an
  error and nothing changes: exit 2 `error: unknown unit 'xx'` / 422 `{"error": "unknown unit 'xx'"}`.
