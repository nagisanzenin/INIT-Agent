"""The catalog: adding, finding, moving and removing items."""
from ..models.item import DEFAULT_CATEGORY, Item
from ..util.money import parse_money
from ..util.qty import parse_qty
from ..util.text import clean
from ..validate.runner import require_valid
#@A
from ..units import convert_item, normalize_unit
#@E


#@B
def add_item(inv, sku, name, category=DEFAULT_CATEGORY, quantity="0", reorder_level="0", location="",
             unit_cost="0"):
#@A
def add_item(inv, sku, name, category=DEFAULT_CATEGORY, quantity="0", reorder_level="0", location="",
             unit_cost="0", unit=None):
#@E
    """Creates an item (ValidationError when it breaks an item rule); an initial quantity is recorded as a
    receive movement with the note "initial stock"."""
    item = Item(
        sku=clean(sku).upper(),
        name=clean(name),
        category=clean(category).lower() or DEFAULT_CATEGORY,
        quantity=parse_qty(quantity),
        reorder_level=parse_qty(reorder_level),
        location=clean(location).upper(),
        unit_cost=parse_money(unit_cost),
#@A
        unit=normalize_unit(unit),
#@E
    )
    require_valid(item)
    inv.add(item)
    if item.quantity:
        inv.record(item.sku, item.quantity, "receive", "initial stock")
    return item


def search(inv, text="", category=None) -> list:
    """Items whose SKU or name contains `text` (case-insensitive) and, when given, in `category`."""
    needle = (text or "").strip().lower()
    out = []
    for item in inv.sorted_items():
        if category and item.category != category.strip().lower():
            continue
        if needle and needle not in item.sku.lower() and needle not in item.name.lower():
            continue
        out.append(item)
    return out


def relocate(inv, sku, location) -> "Item":
    item = inv.get(sku)
    moved = item.with_changes(location=clean(location).upper())
    require_valid(moved)
    inv.put(moved)
    return moved


def remove_item(inv, sku) -> "Item":
    return inv.remove(sku)


def rename_category(inv, old, new) -> int:
    """Moves every item of category `old` into `new`; returns how many moved."""
    old, new = clean(old).lower(), clean(new).lower()
    if not new:
        raise ValueError("the new category must not be blank")
    moved = 0
    for item in inv.sorted_items():
        if item.category == old:
            inv.put(item.with_changes(category=new))
            moved += 1
    return moved


def bulk_relocate(inv, old_location, new_location) -> list:
    """Moves every item at `old_location` to `new_location`; returns the SKUs moved."""
    old, new = clean(old_location).upper(), clean(new_location).upper()
    moved = []
    for item in inv.sorted_items():
        if item.location == old:
            changed = item.with_changes(location=new)
            require_valid(changed)
            inv.put(changed)
            moved.append(item.sku)
    return moved
#@A


def change_unit(inv, sku, unit) -> "Item":
    """Re-expresses an item in another unit of its dimension (no movement is recorded)."""
    item = inv.get(sku)
    changed = convert_item(item, normalize_unit(unit))
    require_valid(changed)
    inv.put(changed)
    return changed
#@E
