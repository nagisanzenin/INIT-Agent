"""The CSV directory store across schema versions (CHANGE.md section 3)."""
import unittest

from stockroom.storage.factory import open_store

from helpers import TempDirCase

V1_ITEMS = ("#stockroom-items v1\nsku,name,category,quantity,reorder_level,location,unit_cost\n"
            "TAP-0001,Duct tape,packing,12,4,B3,2.10\n")
V1_MOVES = "#stockroom-movements v1\nid,sku,delta,reason,note\n1,TAP-0001,12,receive,initial stock\n"


class CsvStoreTest(TempDirCase):
    def test_v1_files_load_in_ea(self):
        self.write("st/items.csv", V1_ITEMS)
        self.write("st/movements.csv", V1_MOVES)
        inv = open_store(self.path("st")).load()
        self.assertEqual(inv.get("TAP-0001").unit, "ea")
        self.assertEqual(inv.movements[0].unit, "ea")

    def test_save_writes_v2(self):
        self.write("st/items.csv", V1_ITEMS)
        self.write("st/movements.csv", V1_MOVES)
        code, out, err = self.cli("issue", "TAP-0001", "2")
        self.assertEqual(code, 0, err)
        self.assertEqual(self.read("st/items.csv"),
                         "#stockroom-items v2\nsku,name,category,quantity,unit,reorder_level,location,unit_cost\n"
                         "TAP-0001,Duct tape,packing,10,ea,4,B3,2.10\n")
        self.assertEqual(self.read("st/movements.csv"),
                         "#stockroom-movements v2\nid,sku,delta,unit,reason,note\n"
                         "1,TAP-0001,12,ea,receive,initial stock\n2,TAP-0001,-2,ea,issue,\n")

    def test_v2_units_are_normalized(self):
        self.write("st/items.csv", "#stockroom-items v2\nsku,name,category,quantity,unit,reorder_level,location,unit_cost\n"
                                   "WAX-0002,Wax,supplies,1.5, Kg ,0.5,B4,9.00\n")
        inv = open_store(self.path("st")).load()
        self.assertEqual(inv.get("WAX-0002").unit, "kg")


if __name__ == "__main__":
    unittest.main()
