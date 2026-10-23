"""Units on the command line: stock operations and the totals report (CHANGE.md sections 7 and 8)."""
import unittest

from helpers import TempDirCase

ITEMS = """#stockroom-items v2
sku,name,category,quantity,unit,reorder_level,location,unit_cost
COF-0001,Coffee beans,drinks,4,kg,1,D1,18.00
MUG-0002,Mug,kitchen,30,ea,10,D2,2.50
SUG-0003,Sugar,drinks,500,g,250,D1,0.01
"""


class CliUnitsTest(TempDirCase):
    def setUp(self):
        super().setUp()
        self.write("st/items.csv", ITEMS)

    def test_receive_in_another_unit(self):
        code, out, err = self.cli("receive", "COF-0001", "250", "--unit", "g")
        self.assertEqual((code, out), (0, "COF-0001: 4.25 kg (+0.25 kg)\n"), err)
        item = self.cli_json("show", "COF-0001")
        self.assertEqual((item["quantity"], item["unit"]), (4.25, "kg"))

    def test_wrong_dimension_is_refused(self):
        code, out, err = self.cli("issue", "MUG-0002", "1", "--unit", "kg")
        self.assertEqual((code, err), (2, "error: cannot convert kg to ea\n"))
        self.assertEqual(self.cli_json("show", "MUG-0002")["quantity"], 30)

    def test_totals_in_a_unit(self):
        report = self.cli_json("report", "totals", "--json", "--unit", "g")
        self.assertEqual(report["rows"], [{"category": "drinks", "unit": "g", "items": 2, "quantity": 4500,
                                           "value": "77.00"}])
        self.assertEqual((report["unit"], report["skipped"]), ("g", 1))


if __name__ == "__main__":
    unittest.main()
