# 0002 — Fuentes modernas, conexiones en vivo y ClickHouse

**Estado:** aprobada por el propietario, 2026-10-11 (Issue #26). **Amplía:** M01 §2.5 (prioridad MVP: Parquet/CSV y PostgreSQL) y M13 §6.1.

## Contexto
El propietario quiere apuntar a formatos modernos y bases de datos, no a "todo tipo de formatos". Rill demuestra el valor de las conexiones en vivo: consulta un DuckLake o un DuckDB sin copiar datos (conector DuckLake desde la v0.86; DuckDB externo solo para desarrollo).

## Decisión
**Almacén oficial:** DuckLake, con catálogo y datos locales o remotos según elija el usuario (0004). Es la única fuente de verdad de los datos que la plataforma importa y transforma. Un `.duckdb` nunca es almacén principal; solo puede servir como caché desechable y medida (M05).

**Fuentes y modos** (cada conjunto de datos elige *en vivo* o *importar*):

| Fuente | En vivo | Importar a DuckLake | Vía |
|---|---|---|---|
| Parquet / Arrow | sí (referencia externa) | sí | lectores nativos de DuckDB |
| PostgreSQL | sí, solo lectura | sí | extensión `postgres` de DuckDB |
| Archivo `.duckdb` del cliente | sí, `READ_ONLY` | sí | `ATTACH` de DuckDB |
| DuckLake externo | sí, solo lectura | sí | `ATTACH 'ducklake:…'` |
| **ClickHouse (segunda fuente)** | **sí, modo principal** | opcional | cliente HTTP oficial; SQL en el dialecto de ClickHouse |
| Otras bases (MySQL, SQL Server, Snowflake, BigQuery…) | según el driver | sí | ADBC con driver probado (M13 §6) |

**Regla práctica:** si los datos solo se analizan en su origen, conexión en vivo. Si hay que cruzarlos con otras fuentes, importación, porque una consulta en vivo la resuelve un solo motor.

**Dónde se declara:** cada conexión es un archivo de `connectors/`. Cada modelo semántico (`metrics/*.yaml`) indica su `connector`, y si no lo hace usa el del proyecto (`project.yaml`), que el usuario elige explícitamente al crearlo (0004). No hay conexión implícita. Los dashboards solo ven métricas: no saben ni necesitan saber dónde se calculan. Rill usa el mismo patrón: `olap_connector` por defecto y `connector` por metrics view.

**Qué motor calcula:**
- En una conexión en vivo a PostgreSQL, a un `.duckdb` o a un DuckLake externo, el que lee es **nuestro DuckDB** (`ATTACH`), aunque los datos sean del cliente.
- En ClickHouse calcula **ClickHouse**: le enviamos el SQL ya traducido a su dialecto.

## Pendiente de confirmar en G3
- CSV con validación estricta y cuarentena. Recomendado: sí.
- Iceberg y Delta Lake.

## Fuera de alcance
Excel y formatos antiguos o propietarios.

## Consecuencias
- **Para G2 (motor semántico):** compilar a través de una **capa de dialectos** (DuckDB primero, ClickHouse después) y declarar las capacidades de cada motor (M11 §463). El compilador semántico nunca genera SQL de DuckDB fijo.
- El contrato de resultados (valores como texto con su tipo) ya cubre UInt64, Decimal256 y DateTime64 de ClickHouse sin pérdida.
- Las credenciales de cada conexión se guardan como referencia a un secreto (M01 §2.1), nunca en los archivos del proyecto.
