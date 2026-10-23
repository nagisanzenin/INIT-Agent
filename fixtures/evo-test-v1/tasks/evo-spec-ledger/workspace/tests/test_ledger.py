"""Examples from SPEC.md. Run from the workspace root: python3 -m unittest discover -s tests"""
import os
import subprocess
import sys
import tempfile
import unittest

import ledger

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

BOOKS = """open 2025-12-01 Assets:Checking
open 2025-12-01 Expenses
2025-12-02 * Groceries
  Expenses:Food   45.5 USD
  Assets:Checking
"""

BOOKS_OUT = """open 2025-12-01 assets:checking

open 2025-12-01 expenses

2025-12-02 * Groceries
  expenses:food                              45.50 USD
  assets:checking                           -45.50 USD
"""


class Dates(unittest.TestCase):
    def test_forms(self):
        for text in ("2025-11-30", "2025/11/30", "30.11.2025", "20251130", "30-Nov-2025"):
            self.assertEqual(ledger.normalize_date(text), "2025-11-30")

    def test_errors(self):
        with self.assertRaises(ledger.LedgerError) as cm:
            ledger.normalize_date("2025-11-31")
        self.assertEqual(str(cm.exception), "invalid date '2025-11-31'")
        with self.assertRaises(ledger.LedgerError) as cm:
            ledger.normalize_date("11/30/2025")
        self.assertEqual(str(cm.exception), "bad date '11/30/2025'")


class Amounts(unittest.TestCase):
    def test_normal_form(self):
        self.assertEqual(ledger.normalize_amount("1500 usd"), "1,500.00 USD")
        self.assertEqual(ledger.normalize_amount("EUR 2.5"), "2.50 EUR")
        self.assertEqual(ledger.normalize_amount("(3) GBP"), "-3.00 GBP")

    def test_aliases(self):
        self.assertEqual(ledger.normalize_amount("10 Yen"), "10 JPY")
        self.assertEqual(ledger.normalize_amount("dollar 3"), "3.00 USD")

    def test_errors(self):
        with self.assertRaises(ledger.LedgerError) as cm:
            ledger.normalize_amount("12 ABC")
        self.assertEqual(str(cm.exception), "unknown currency 'ABC'")
        with self.assertRaises(ledger.LedgerError) as cm:
            ledger.normalize_amount("4..5 EUR")
        self.assertEqual(str(cm.exception), "bad number '4..5'")


class Accounts(unittest.TestCase):
    def test_normal_form(self):
        self.assertEqual(ledger.normalize_account("Expenses:Food:Groceries"), "expenses:food:groceries")

    def test_bad_root(self):
        with self.assertRaises(ledger.LedgerError) as cm:
            ledger.normalize_account("Bank:x")
        self.assertEqual(str(cm.exception), "bad account root 'Bank'")


class Process(unittest.TestCase):
    def test_transaction(self):
        r = ledger.process(BOOKS)
        self.assertEqual(r.errors, [])
        self.assertEqual(r.output, BOOKS_OUT)

    def test_unbalanced(self):
        r = ledger.process("open 2025-12-01 assets\n2025-12-03 * Lunch\n  assets:a 10 USD\n  assets:b -9.5 USD\n")
        self.assertEqual(r.errors, ["line 2: transaction does not balance: 0.50 USD"])
        self.assertEqual(r.output, "open 2025-12-01 assets\n")

    def test_not_open(self):
        r = ledger.process("open 2025-12-01 assets\n2025-12-03 * Fee\n  expenses:bank 1 EUR\n  assets:cash\n")
        self.assertEqual(r.errors, ["line 3: account 'expenses:bank' is not open"])

    def test_comment_and_unknown_directive(self):
        r = ledger.process("; books for 2025\ninclude other.plt\n")
        self.assertEqual(r.output, "; books for 2025\n")
        self.assertEqual(r.errors, ["line 2: unknown directive 'include'"])


class Directives(unittest.TestCase):
    def test_default_currency(self):
        r = ledger.process("open 2025-12-01 assets\ndefault eur\n2025-12-02 * Bus\n  assets:transport 2.4\n"
                           "  assets:cash\n")
        self.assertEqual(r.errors, [])
        self.assertEqual(r.output, "open 2025-12-01 assets\n\ndefault EUR\n\n2025-12-02 * Bus\n"
                                   "  assets:transport                            2.40 EUR\n"
                                   "  assets:cash                                -2.40 EUR\n")

    def test_unit_price(self):
        r = ledger.process("open 2025-12-01 assets\n2025-12-02 * Exchange\n  assets:eur 50 EUR @ 1.2 USD\n"
                           "  assets:usd\n")
        self.assertEqual(r.errors, [])
        self.assertEqual(r.output, "open 2025-12-01 assets\n\n2025-12-02 * Exchange\n"
                                   "  assets:eur                                 50.00 EUR @ 1.20 USD\n"
                                   "  assets:usd                                -60.00 USD\n")

    def test_year_and_short_date(self):
        r = ledger.process("year 2025\nopen 12-01 assets\n")
        self.assertEqual(r.output, "year 2025\n\nopen 2025-12-01 assets\n")

    def test_account_alias(self):
        r = ledger.process("alias bank Assets:Checking\nopen 2025-12-01 bank\n")
        self.assertEqual(r.output, "alias bank assets:checking\n\nopen 2025-12-01 assets:checking\n")


class CommandLine(unittest.TestCase):
    def test_file(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "books.plt")
            with open(path, "w") as f:
                f.write(BOOKS)
            p = subprocess.run([sys.executable, "-m", "ledger", path], cwd=ROOT, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, timeout=30)
        self.assertEqual(p.returncode, 0)
        self.assertEqual(p.stdout.decode(), BOOKS_OUT)
        self.assertEqual(p.stderr.decode(), "")


if __name__ == "__main__":
    unittest.main()
