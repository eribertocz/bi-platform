-- The rows of #147's oracle, written once into the server's DuckLake
-- (`lake` attached exactly as the server attaches it). Norte = 100.30 / 2,
-- Sur = 200.00 / 2, total = 300.30 / 4.
CREATE OR REPLACE TABLE lake.main.ventas (
    id BIGINT NOT NULL,
    region VARCHAR NOT NULL,
    ciudad VARCHAR NOT NULL,
    importe DECIMAL(18, 2) NOT NULL,
    instante TIMESTAMPTZ NOT NULL,
    nota VARCHAR
);
INSERT INTO lake.main.ventas VALUES
    (1, 'Norte', 'La Paz', 100.10, '2026-01-01T00:00:00Z', NULL),
    (2, 'Norte', 'El Alto', 0.20, '2026-01-02T00:00:00Z', 'n'),
    (3, 'Sur', 'Tarija', 200.20, '2026-01-01T00:00:00Z', 's'),
    (4, 'Sur', 'Sucre', -0.20, '2026-01-02T00:00:00Z', NULL);
