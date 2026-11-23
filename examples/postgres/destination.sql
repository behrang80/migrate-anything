-- Demo 3 destination schema (different names, surrogate keys, FKs)
CREATE TABLE clients (id bigserial PRIMARY KEY, ext_ref text UNIQUE NOT NULL, display_name text NOT NULL, country_code char(2) NOT NULL,
  credit numeric(14,2), is_active boolean NOT NULL, attributes jsonb);
CREATE TABLE purchase_orders (id bigserial PRIMARY KEY, ext_ref text UNIQUE NOT NULL, client_id bigint NOT NULL REFERENCES clients(id),
  order_date date NOT NULL, state text NOT NULL, amount numeric(14,2) NOT NULL, source_created_at timestamptz);
CREATE TABLE lines (id bigserial PRIMARY KEY, ext_ref text UNIQUE NOT NULL, order_id bigint NOT NULL REFERENCES purchase_orders(id),
  product_code text NOT NULL, quantity int NOT NULL, price numeric(12,2) NOT NULL);
