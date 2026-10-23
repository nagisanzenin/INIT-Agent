"""Inventory: the items (by SKU), the movement history, the kits and the supplier packs a store holds."""
from __future__ import annotations

from ..errors import DuplicateError, NotFound
from .item import Item
from .movement import Movement


class Inventory:
    def __init__(self):
        self.items = {}  # sku -> Item
        self.movements = []  # Movement, in id order
        self.kits = {}  # kit sku -> [KitLine], in component order
        self.packs = {}  # sku -> [Pack], in supplier order

    def __len__(self):
        return len(self.items)

    def __contains__(self, sku):
        return sku in self.items

    def get(self, sku) -> Item:
        try:
            return self.items[sku]
        except KeyError:
            raise NotFound("unknown sku: %s" % sku) from None

    def add(self, item: Item) -> None:
        if item.sku in self.items:
            raise DuplicateError("duplicate sku: %s" % item.sku)
        self.items[item.sku] = item

    def put(self, item: Item) -> None:
        """Adds or replaces."""
        self.items[item.sku] = item

    def remove(self, sku) -> Item:
        item = self.get(sku)
        del self.items[sku]
        return item

    def sorted_items(self) -> list:
        return [self.items[k] for k in sorted(self.items)]

    def categories(self) -> list:
        return sorted({i.category for i in self.items.values()})

    def next_movement_id(self) -> int:
        return max((m.id for m in self.movements), default=0) + 1

    def record(self, sku, delta, reason, note="") -> Movement:
        """Appends a movement of `delta` (in the item's own quantity terms) to the history."""
        unit = self.items[sku].unit if sku in self.items else "ea"
        m = Movement(id=self.next_movement_id(), sku=sku, delta=delta, reason=reason, note=note, unit=unit)
        self.movements.append(m)
        return m

    def history(self, sku) -> list:
        return [m for m in self.movements if m.sku == sku]

    def kit_lines(self) -> list:
        return [line for kit in sorted(self.kits) for line in self.kits[kit]]

    def pack_list(self) -> list:
        return [p for sku in sorted(self.packs) for p in self.packs[sku]]

    def set_kit(self, kit, lines) -> None:
        self.kits[kit] = sorted(lines, key=lambda line: line.component)

    def set_pack(self, pack) -> None:
        packs = [p for p in self.packs.get(pack.sku, []) if p.supplier != pack.supplier] + [pack]
        self.packs[pack.sku] = sorted(packs, key=lambda p: p.supplier)
