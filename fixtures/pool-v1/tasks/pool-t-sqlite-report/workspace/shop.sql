-- Sample data for the monthly revenue report.
CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE orders (
  id INTEGER PRIMARY KEY,
  customer_id INTEGER NOT NULL REFERENCES customers(id),
  status TEXT NOT NULL,          -- 'paid', 'shipped', 'cancelled' or 'refunded'
  discount_cents INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE order_items (
  order_id INTEGER NOT NULL REFERENCES orders(id),
  sku TEXT NOT NULL,
  quantity INTEGER NOT NULL,
  unit_price_cents INTEGER NOT NULL
);
INSERT INTO customers VALUES (1, 'Ada'), (2, 'Bo'), (3, 'Cy'), (4, 'Di');
INSERT INTO orders VALUES
  (10, 1, 'paid', 0), (11, 1, 'shipped', 150), (12, 2, 'cancelled', 0),
  (13, 2, 'paid', 0), (14, 3, 'refunded', 0), (15, 1, 'paid', 0);
INSERT INTO order_items VALUES
  (10, 'pen', 3, 120), (10, 'pad', 1, 450),
  (11, 'ink', 2, 300),
  (12, 'pen', 10, 120),
  (13, 'pad', 2, 450), (13, 'pen', 1, 120),
  (14, 'ink', 5, 300);
