# 0001 — Núcleo con un solo motor analítico: DuckDB

**Estado:** aprobada por el propietario, 2026-10-11 (Issue #25). **Sustituye:** la enmienda I.2 de `DECISIONES_TECNICAS_RC2.md` ("Polars 2.0 de primera clase") y las rutas `polars_rust` de M02/M13.

## Contexto
La base preveía dos caminos de transformación de primera clase, DuckDB SQL y Polars LazyFrame, con publicación en DuckLake a través de una frontera transaccional. El benchmark de G1 (Issue #15, PR #19 cerrado como evidencia) los comparó sobre el mismo fixture y el mismo oráculo.

## Evidencia
- **Rendimiento:** a 1M filas, los dos motores rinden igual. A 10M, DuckDB es 1,5–2,8× más rápido en agregaciones.
- **Exactitud:** Polars suma `Int64` en 64 bits y se desborda sin error. Hay que convertir a `Int128` en cada suma. DuckDB amplía a HUGEINT por sí solo.
- **Publicación:** Polars no escribe en DuckLake. Necesita a DuckDB igualmente, con un Parquet intermedio o un intercambio Arrow.
- **Cadena de suministro:** Polars 0.55.2, la última versión, trae RUSTSEC-2026-0194/0195 (quick-xml) y RUSTSEC-2025-0141 (bincode abandonado), sin versión corregida.
- **Coste:** unos 12 min más por compilación release, más memoria al compilar y un pánico si `collect()` se ejecuta dentro de Tokio.
- **Divergencia:** M13 §7.2 advierte que dos motores con la misma semántica acaban dando cifras distintas.

## Decisión
DuckDB es el único motor del núcleo: consultas, transformaciones (`models/*.sql`) y publicación en DuckLake. Polars no forma parte del servidor ni de la ruta de modelos.

## Alternativas descartadas
- **Mantener Polars como segunda vía con intercambio Arrow:** duplica la semántica y exige `unsafe` o un puente IPC para un beneficio no demostrado.
- **Backend en Python:** el análisis lo ejecutan DuckDB (C++) y Polars (Rust) igualmente. Python empeora la concurrencia, la serialización y el despliegue del servidor.

## Consecuencias
- Un solo lenguaje de transformación (SQL de DuckDB), una sola semántica y una sola batería de exactitud.
- Polars, o Python en general, solo podría volver dentro de un **laboratorio aislado**: proceso o contenedor con límites, publicando en DuckLake por Arrow (`ESTANDARES_Y_PORTABILIDAD.md`, punto 3). Requiere una decisión nueva.
- Los motores **del cliente** (ClickHouse, en 0002) no contradicen esta decisión: se les envían consultas, pero no son nuestro motor.
