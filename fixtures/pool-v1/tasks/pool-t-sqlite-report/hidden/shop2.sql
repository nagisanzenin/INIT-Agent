CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER NOT NULL, status TEXT NOT NULL,
  discount_cents INTEGER NOT NULL DEFAULT 0);
CREATE TABLE order_items (order_id INTEGER NOT NULL, sku TEXT NOT NULL, quantity INTEGER NOT NULL,
  unit_price_cents INTEGER NOT NULL);
INSERT INTO customers VALUES (1, 'Zed'), (2, 'Amy'), (3, 'Max'), (4, 'Lou'), (5, 'Kim'), (6, 'Amy');
INSERT INTO orders VALUES
  (1, 1, 'paid', 0), (2, 2, 'paid', 100), (3, 3, 'shipped', 0), (4, 4, 'refunded', 0),
  (5, 5, 'paid', 0), (6, 5, 'paid', 0), (7, 3, 'cancelled', 0), (8, 6, 'shipped', 0),
  (9, 2, 'paid', 0), (10, 1, 'shipped', 500);
INSERT INTO order_items VALUES
  (1, 'a', 2, 1000), (1, 'b', 1, 500),
  (2, 'a', 1, 1000), (2, 'c', 4, 250),
  (3, 'a', 1, 1900),
  (4, 'b', 9, 500),
  (5, 'c', 1, 250),
  (7, 'a', 50, 1000),
  (8, 'b', 3, 500), (8, 'c', 2, 200),
  (9, 'c', 2, 450),
  (10, 'a', 1, 500);
