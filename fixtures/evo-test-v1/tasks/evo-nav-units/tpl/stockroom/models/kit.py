"""Kits (bills of materials): a kit is itself an item; assembling one consumes its components."""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from ..util.qty import format_qty, parse_qty


@dataclass
class KitLine:
    kit: str
    component: str
    quantity: Decimal  # of the component, per kit
#@A
    unit: str = "ea"
#@E

    def to_record(self) -> dict:
        rec = {"kit": self.kit, "component": self.component, "quantity": format_qty(self.quantity)}
#@A
        rec["unit"] = self.unit
#@E
        return rec

    @classmethod
    def from_record(cls, rec: dict) -> "KitLine":
        return cls(
            kit=str(rec.get("kit", "")).strip().upper(),
            component=str(rec.get("component", "")).strip().upper(),
            quantity=parse_qty(rec.get("quantity", "")),
#@A
            unit=(str(rec.get("unit") or "").strip().lower() or "ea"),
#@E
        )
