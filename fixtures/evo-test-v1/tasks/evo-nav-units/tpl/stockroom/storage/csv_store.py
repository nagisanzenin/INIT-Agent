"""The CSV directory store: <dir>/items.csv, movements.csv, kits.csv and packs.csv.

Each file starts with a line naming its kind and schema version ("#stockroom-items v1"), then the CSV header
row, then one record per row (items sorted by SKU, movements in id order, kit lines by kit then component,
packs by SKU then supplier). Rows read from an older version are upgraded by storage.migrate. Only items.csv
is required: a missing movements.csv, kits.csv or packs.csv means none.
"""
import csv
import io
import os

from ..errors import DuplicateError, StorageError
from ..models.inventory import Inventory
from ..models.item import Item
from ..models.kit import KitLine
from ..models.movement import Movement
from ..models.pack import Pack
from . import migrate, schema
from .base import Store, atomic_write

# the column header row of each schema version
ITEM_COLUMNS = {
    1: ["sku", "name", "category", "quantity", "reorder_level", "location", "unit_cost"],
#@A
    2: ["sku", "name", "category", "quantity", "unit", "reorder_level", "location", "unit_cost"],
#@E
}
MOVEMENT_COLUMNS = {
    1: ["id", "sku", "delta", "reason", "note"],
#@A
    2: ["id", "sku", "delta", "unit", "reason", "note"],
#@E
}
KIT_COLUMNS = {
    1: ["kit", "component", "quantity"],
#@A
    2: ["kit", "component", "quantity", "unit"],
#@E
}
PACK_COLUMNS = {
    1: ["sku", "supplier", "quantity", "price"],
#@A
    2: ["sku", "supplier", "quantity", "unit", "price"],
#@E
}


class CsvStore(Store):
    kind = "csv"

    @property
    def items_path(self):
        return os.path.join(self.path, "items.csv")

    @property
    def movements_path(self):
        return os.path.join(self.path, "movements.csv")

    def exists(self) -> bool:
        return os.path.isfile(self.items_path)

    def load(self) -> Inventory:
        if not self.exists():
            raise StorageError("no store at %s (run `stockroom init`)" % self.path)
        inv = Inventory()
        for line, rec in self._rows("items.csv", self.items_path, ITEM_COLUMNS):
            try:
                inv.add(Item.from_record(rec))
            except (ValueError, DuplicateError) as e:
                raise StorageError("items.csv line %d: %s" % (line, e))
        if os.path.isfile(self.movements_path):
            for line, rec in self._rows("movements.csv", self.movements_path, MOVEMENT_COLUMNS):
                try:
                    inv.movements.append(Movement.from_record(rec))
                except ValueError as e:
                    raise StorageError("movements.csv line %d: %s" % (line, e))
        kits = {}
        if os.path.isfile(self._path("kits.csv")):
            for line, rec in self._rows("kits.csv", self._path("kits.csv"), KIT_COLUMNS):
                try:
                    kl = KitLine.from_record(rec)
                except ValueError as e:
                    raise StorageError("kits.csv line %d: %s" % (line, e))
                kits.setdefault(kl.kit, []).append(kl)
        for kit, lines in kits.items():
            inv.set_kit(kit, lines)
        if os.path.isfile(self._path("packs.csv")):
            for line, rec in self._rows("packs.csv", self._path("packs.csv"), PACK_COLUMNS):
                try:
                    inv.set_pack(Pack.from_record(rec))
                except ValueError as e:
                    raise StorageError("packs.csv line %d: %s" % (line, e))
        return inv

    def _path(self, filename):
        return os.path.join(self.path, filename)

    def _rows(self, filename, path, columns):
        with open(path, encoding="utf-8", newline="") as f:
            version = schema.parse_csv_header(f.readline(), filename)
            reader = csv.reader(f)
            header = next(reader, None)
            if header != columns[version]:
                raise StorageError("%s: columns do not match schema version %d" % (filename, version))
            for row in reader:
                line = reader.line_num + 1
                if not row:
                    continue
                if len(row) != len(header):
                    raise StorageError("%s line %d: expected %d fields, got %d" % (filename, line, len(header),
                                                                                   len(row)))
                yield line, migrate.upgrade(filename, version, dict(zip(header, row)))

    def save(self, inv: Inventory) -> None:
        os.makedirs(self.path, exist_ok=True)
        items = [i.to_record() for i in inv.sorted_items()]
        moves = [m.to_record() for m in inv.movements]
        atomic_write(self.items_path, _render("items.csv", ITEM_COLUMNS[schema.ITEMS_CSV], items))
        atomic_write(self.movements_path, _render("movements.csv", MOVEMENT_COLUMNS[schema.MOVEMENTS_CSV], moves))
        kits = [kl.to_record() for kl in inv.kit_lines()]
        packs = [p.to_record() for p in inv.pack_list()]
        for filename, columns, records in (("kits.csv", KIT_COLUMNS[schema.KITS_CSV], kits),
                                           ("packs.csv", PACK_COLUMNS[schema.PACKS_CSV], packs)):
            if records or os.path.isfile(self._path(filename)):
                atomic_write(self._path(filename), _render(filename, columns, records))


def _render(filename, columns, records) -> str:
    buf = io.StringIO()
    buf.write(schema.csv_header(filename) + "\n")
    writer = csv.writer(buf, lineterminator="\n")
    writer.writerow(columns)
    for rec in records:
        writer.writerow([rec[c] for c in columns])
    return buf.getvalue()
