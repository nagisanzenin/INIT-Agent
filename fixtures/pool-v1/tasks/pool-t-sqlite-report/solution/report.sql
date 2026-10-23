WITH order_totals AS (
  SELECT o.id, o.customer_id,
         COALESCE((SELECT SUM(i.quantity * i.unit_price_cents) FROM order_items i WHERE i.order_id = o.id), 0)
           - o.discount_cents AS total
  FROM orders o
  WHERE o.status IN ('paid', 'shipped')
)
SELECT c.name, COUNT(t.id) AS orders, SUM(t.total) AS total_cents
FROM customers c JOIN order_totals t ON t.customer_id = c.id
GROUP BY c.id
ORDER BY total_cents DESC, c.name ASC;
