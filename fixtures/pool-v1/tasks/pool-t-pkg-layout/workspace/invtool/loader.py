"""Reads the inventory CSV (columns: sku, name, category, qty, price)."""
import csv
import os


def load(path):
    full = os.path.join(os.path.dirname(__file__), path)
    items = []
    with open(full, newline="") as f:
        for row in csv.DictReader(f):
            whole, _, frac = row["price"].partition(".")
            items.append({"sku": row["sku"], "name": row["name"], "category": row["category"],
                          "qty": int(row["qty"]), "price_cents": int(whole) * 100 + int((frac + "00")[:2])})
    return items
