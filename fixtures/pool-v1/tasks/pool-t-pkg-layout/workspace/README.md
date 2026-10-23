# invtool

Inventory reports from a CSV file (columns sku, name, category, qty, price).

    python3 -m invtool report data/inventory.csv
    python3 -m invtool total data/inventory.csv [--category CATEGORY]

`report` prints the table sorted by SKU; `total` prints the stock value (qty x price) as `D.CC`.
The CSV path is relative to the directory you run the command from, or absolute.
