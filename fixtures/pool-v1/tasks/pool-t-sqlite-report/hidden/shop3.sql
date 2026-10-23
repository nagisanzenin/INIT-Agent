CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER NOT NULL, status TEXT NOT NULL,
  discount_cents INTEGER NOT NULL DEFAULT 0);
CREATE TABLE order_items (order_id INTEGER NOT NULL, sku TEXT NOT NULL, quantity INTEGER NOT NULL,
  unit_price_cents INTEGER NOT NULL);
INSERT INTO customers VALUES (1, 'Ann'), (2, 'Ben'), (3, 'Cal');
INSERT INTO orders VALUES (1, 1, 'refunded', 0), (2, 2, 'cancelled', 0), (3, 3, 'paid', 0), (4, 3, 'paid', 0);
INSERT INTO order_items VALUES (1, 'x', 1, 999), (2, 'x', 1, 999), (3, 'y', 7, 3);
