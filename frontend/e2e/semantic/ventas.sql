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

-- #261: a SUM wider than its DECIMAL(18, 2) source. Norte only, under the
-- same row policy: SUM = 10000000048735710.80 (DECIMAL(38, 2)), its double
-- 20000000097471421.60 (DECIMAL(39, 2)); Sur reads no row of it.
CREATE OR REPLACE TABLE lake.main.grandes (
    id BIGINT NOT NULL,
    region VARCHAR NOT NULL,
    importe DECIMAL(18, 2) NOT NULL
);
INSERT INTO lake.main.grandes VALUES
    (1, 'Norte', 9999999999999999.99),
    (2, 'Norte', 48735710.81);
