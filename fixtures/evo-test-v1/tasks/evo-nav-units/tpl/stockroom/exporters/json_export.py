"""The json exporter: a list of objects, two-space indentation, final newline.

Each object has the item's fields in FIELDS order; quantities are JSON numbers (an integer when whole), the
unit cost a string with two decimals. The output reads back with the json importer.
"""
import json

from ..util.money import format_money
from ..util.qty import qty_json


def write_json(items, options=None) -> str:
    return json.dumps([item_object(i) for i in items], indent=2) + "\n"


def item_object(item) -> dict:
    obj = {
        "sku": item.sku,
        "name": item.name,
        "category": item.category,
        "quantity": qty_json(item.quantity),
    }
#@A
    obj["unit"] = item.unit
#@E
    obj["reorder_level"] = qty_json(item.reorder_level)
    obj["location"] = item.location
    obj["unit_cost"] = format_money(item.unit_cost)
    return obj
