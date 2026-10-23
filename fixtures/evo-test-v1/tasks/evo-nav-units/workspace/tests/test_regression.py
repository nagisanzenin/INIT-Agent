"""1.6 behaviour that must keep working (README.md)."""
import unittest

from helpers import TempDirCase


class RegressionTest(TempDirCase):
    def setUp(self):
        super().setUp()
        self.assertEqual(self.cli("init")[0], 0)
        self.cli("add", "NUT-0001", "Wing nut", "--category", "fasteners", "--qty", "8", "--reorder", "10",
                 "--location", "A2", "--cost", "0.30")
        self.cli("add", "SAW-0002", "Hand saw", "--category", "tools", "--qty", "3", "--location", "B1",
                 "--cost", "14.00")

    def test_insufficient_stock(self):
        code, out, err = self.cli("issue", "SAW-0002", "5")
        self.assertEqual((code, err), (2, "error: insufficient stock for SAW-0002: have 3, need 5\n"))

    def test_low_stock_and_valuation(self):
        low = self.cli_json("report", "low-stock", "--json")
        self.assertEqual([r["sku"] for r in low["rows"]], ["NUT-0001"])
        self.assertEqual(low["rows"][0]["shortfall"], 2)
        val = self.cli_json("report", "valuation", "--json")
        self.assertEqual(val["total"], "44.40")

    def test_import_errors_keep_their_wording(self):
        self.write("in.csv", "sku,name,quantity\nTAP-0003,Tape,abc\nGLU-0004,Glue,2\n")
        code, out, err = self.cli("import", self.path("in.csv"))
        self.assertEqual(code, 1)
        self.assertEqual(err, "%s:2: bad quantity 'abc'\n" % self.path("in.csv"))
        self.assertEqual(out, "imported 1 item, 1 error\n")


if __name__ == "__main__":
    unittest.main()
