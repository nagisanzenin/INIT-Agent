"""Supplier packs: how a supplier sells an item (so much per pack, at a price per pack)."""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from ..util.money import format_money, parse_money
from ..util.qty import format_qty, parse_qty


@dataclass
class Pack:
    sku: str
    supplier: str
    quantity: Decimal  # how much of the item one pack holds
    price: Decimal  # the price of one pack
    unit: str = "ea"

    def to_record(self) -> dict:
        rec = {"sku": self.sku, "supplier": self.supplier, "quantity": format_qty(self.quantity),
               "price": format_money(self.price)}
        rec["unit"] = self.unit
        return rec

    @classmethod
    def from_record(cls, rec: dict) -> "Pack":
        return cls(
            sku=str(rec.get("sku", "")).strip().upper(),
            supplier=" ".join(str(rec.get("supplier", "")).split()),
            quantity=parse_qty(rec.get("quantity", "")),
            price=parse_money(rec.get("price", "0")),
            unit=(str(rec.get("unit") or "").strip().lower() or "ea"),
        )
