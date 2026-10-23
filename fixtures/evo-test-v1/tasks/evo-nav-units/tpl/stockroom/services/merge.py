"""Merging an ImportResult into an inventory (the `import` command, POST /import).

Every row is checked with the item rules first; a row that breaks one becomes an error of the result at its
line ("<field>: <message>" of the first problem) and is skipped. Then:
  - a new SKU is added as it is;
  - mode "add": the row's quantity is added to the existing item, nothing else changes;
  - mode "replace": the row replaces the existing item.
A movement with reason "import" records every quantity change (none when it is zero); its note is the
result's source (the file name, or "api").
"""
from ..errors import StockroomError
from ..validate.item_rules import validate_item
#@A
from ..errors import UnitError
from ..units import convert, factor_ratio
#@E

MODES = ("add", "replace")


def merge(inv, result, mode="add") -> int:
    """Applies the result's rows; returns how many were applied."""
    if mode not in MODES:
        raise StockroomError("unknown import mode: %s" % mode)
    applied = 0
    for line, incoming in result.rows:
        problems = validate_item(incoming)
        if problems:
            result.error(line, "%s: %s" % (problems[0].field, problems[0].message))
            continue
        current = inv.items.get(incoming.sku)
        if current is None:
            inv.add(incoming)
            if incoming.quantity:
                inv.record(incoming.sku, incoming.quantity, "import", result.source)
            applied += 1
            continue
#@B
        if mode == "add":
            delta = incoming.quantity
            inv.put(current.with_changes(quantity=current.quantity + delta))
        else:
            delta = incoming.quantity - current.quantity
            inv.put(incoming)
#@A
        try:
            if mode == "add":
                delta = convert(incoming.quantity, incoming.unit, current.unit)
                merged = current.with_changes(quantity=current.quantity + delta)
            else:
                factor_ratio(incoming.unit, current.unit)  # same dimension? (the error names the row's unit first)
                delta = incoming.quantity - convert(current.quantity, current.unit, incoming.unit)
                merged = incoming
        except UnitError as e:
            result.error(line, str(e))
            continue
        problems = validate_item(merged)
        if problems:
            result.error(line, "%s: %s" % (problems[0].field, problems[0].message))
            continue
        inv.put(merged)
#@E
        if delta:
            inv.record(incoming.sku, delta, "import", result.source)
        applied += 1
    return applied
