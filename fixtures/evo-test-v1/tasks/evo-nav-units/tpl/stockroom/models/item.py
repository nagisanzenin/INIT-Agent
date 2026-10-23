"""The Item record: one stock-keeping unit (SKU) held in the inventory."""
from __future__ import annotations

import re
from dataclasses import dataclass, replace
from decimal import Decimal

from ..util.money import format_money, parse_money, round_money
from ..util.qty import ZERO, format_qty, parse_qty

SKU_RE = re.compile(r"^[A-Z]{3}-[0-9]{4}$")
LOCATION_RE = re.compile(r"^[A-Z][0-9]{1,2}$")
DEFAULT_CATEGORY = "general"

# The fields of an item in their canonical order (records, CSV columns, JSON keys follow it).
#@B
FIELDS = ("sku", "name", "category", "quantity", "reorder_level", "location", "unit_cost")
#@A
FIELDS = ("sku", "name", "category", "quantity", "unit", "reorder_level", "location", "unit_cost")
#@E
QUANTITY_FIELDS = ("quantity", "reorder_level")


@dataclass
class Item:
    sku: str
    name: str = ""
    category: str = DEFAULT_CATEGORY
    quantity: Decimal = ZERO
    reorder_level: Decimal = ZERO
    location: str = ""
    unit_cost: Decimal = Decimal("0.00")
#@A
    unit: str = "ea"
#@E

    def value(self) -> Decimal:
        """Stock value: quantity x unit_cost, rounded to cents."""
        return round_money(self.quantity * self.unit_cost)

    def is_low(self) -> bool:
        """At or below its reorder level."""
        return self.quantity <= self.reorder_level

    def shortfall(self) -> Decimal:
        """How much is missing to reach the reorder level (0 when above it)."""
        return max(ZERO, self.reorder_level - self.quantity)

    def with_changes(self, **changes) -> "Item":
        return replace(self, **changes)

    def to_record(self) -> dict:
        """Field name -> text, in FIELDS order: the form the CSV store and the CSV exporter write."""
        rec = {
            "sku": self.sku,
            "name": self.name,
            "category": self.category,
            "quantity": format_qty(self.quantity),
            "reorder_level": format_qty(self.reorder_level),
            "location": self.location,
            "unit_cost": format_money(self.unit_cost),
#@A
            "unit": self.unit,
#@E
        }
        return {f: rec[f] for f in FIELDS}

    @classmethod
    def from_record(cls, rec: dict) -> "Item":
        """The inverse of to_record. Missing or blank fields take their defaults. Raises ValueError."""

        def get(name, default=""):
            v = rec.get(name)
            if v is None or str(v).strip() == "":
                return default
            return str(v).strip()

        return cls(
            sku=get("sku").upper(),
            name=get("name"),
            category=get("category", DEFAULT_CATEGORY),
            quantity=parse_qty(get("quantity", "0")),
            reorder_level=parse_qty(get("reorder_level", "0")),
            location=get("location").upper(),
            unit_cost=parse_money(get("unit_cost", "0")),
#@A
            unit=get("unit", "ea").lower(),
#@E
        )
