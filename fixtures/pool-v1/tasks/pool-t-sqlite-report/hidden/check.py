"""pool-t-sqlite-report check: report.sql against the visible data and two hidden data sets."""
import os
import sqlite3
import sys

CD = os.environ["INIT_CHECK_DIR"]
REF = """
WITH order_totals AS (
  SELECT o.id, o.customer_id,
         COALESCE((SELECT SUM(i.quantity * i.unit_price_cents) FROM order_items i WHERE i.order_id = o.id), 0)
           - o.discount_cents AS total
  FROM orders o WHERE o.status IN ('paid', 'shipped'))
SELECT c.name, COUNT(t.id), SUM(t.total)
FROM customers c JOIN order_totals t ON t.customer_id = c.id
GROUP BY c.id ORDER BY 3 DESC, c.name ASC
"""


def rows(data_sql, query):
    db = sqlite3.connect(":memory:")
    db.executescript(data_sql)
    return [tuple(r) for r in db.execute(query)]


def main():
    try:
        with open("report.sql") as f:
            query = f.read()
    except OSError:
        query = None
    sets = [("visible_data", open("shop.sql").read() if os.path.isfile("shop.sql") else None),
            ("hidden_data_1", open(os.path.join(CD, "hidden", "shop2.sql")).read()),
            ("hidden_data_2", open(os.path.join(CD, "hidden", "shop3.sql")).read())]
    for name, data in sets:
        ok = False
        if query is not None and data is not None:
            try:
                want = rows(data, REF)
                got = rows(data, query)
                ok = got == want
            except Exception as e:  # noqa: BLE001
                sys.stderr.write("%s: %s\n" % (name, e))
        print("%s %s" % ("PASS" if ok else "FAIL", name))
    # a header row or extra columns would already fail above; also require exactly one statement
    single = False
    if query is not None:
        stripped = query.strip().rstrip(";").strip()
        single = bool(stripped) and sqlite3.complete_statement(stripped + ";") and ";" not in stripped
    print("%s single_statement" % ("PASS" if single else "FAIL"))


main()
