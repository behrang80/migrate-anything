-- Demo 3 source schema and synthetic data
CREATE TABLE customers (cust_no int PRIMARY KEY, cust_name varchar(80) NOT NULL, country char(2) NOT NULL,
  credit_limit numeric(12,2), active boolean NOT NULL DEFAULT true, meta jsonb);
CREATE TABLE orders (order_no bigint PRIMARY KEY, cust_no int NOT NULL REFERENCES customers(cust_no),
  ordered_on date NOT NULL, status varchar(12) NOT NULL, total numeric(14,2) NOT NULL, created_at timestamptz NOT NULL);
CREATE TABLE order_items (item_id serial PRIMARY KEY, order_no bigint NOT NULL REFERENCES orders(order_no),
  sku text NOT NULL, qty int NOT NULL, unit_price numeric(12,2) NOT NULL);
INSERT INTO customers SELECT g, 'Customer ' || g, (ARRAY['SE','DE','US','JP'])[1 + g % 4], (g * 137 % 100000) / 4.0,
  g % 9 <> 0, jsonb_build_object('segment', (ARRAY['smb','ent'])[1 + g % 2], 'score', g % 7 + 0.5) FROM generate_series(1, 300) g;
INSERT INTO orders SELECT 100000 + g, 1 + g % 300, date '2024-01-01' + (g % 365), (ARRAY['NEW','SHIPPED','CANCELLED'])[1 + g % 3],
  (g * 7919 % 1000000) / 100.0, timestamptz '2024-01-01 08:00:00+01' + g * interval '37 minutes' FROM generate_series(1, 1500) g;
INSERT INTO order_items (order_no, sku, qty, unit_price) SELECT 100000 + 1 + g % 1500, 'SKU-' || (g % 97), 1 + g % 5, (g * 31 % 10000) / 100.0
  FROM generate_series(1, 4000) g;
