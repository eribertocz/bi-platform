# 0005 — Modelos SQL estilo SQLMesh: motor de cálculo y destino separados

**Estado:** aprobada por el propietario, 2026-10-11. Corregida ese mismo día para separar el motor que calcula del lugar donde se guarda el resultado. **Amplía:** 0001 (un modelo puede calcularse en ClickHouse si el usuario lo elige) y M02 (formato de `ModelDefinition`). **Referentes:** SQLMesh (formato y plan), dbt (pruebas, linaje), Rill (`connector` de entrada frente a `output.connector` y `stage`, en `models.md` v0.90.5). **Matizada por:** [0009](0009-modos-de-datos-y-persistencia-opcional.md) (`cron` pasa a `pipelines/`; capas opcionales).

## Contexto
El propietario prefiere cómo SQLMesh declara los modelos: SQL con una cabecera declarativa y capas staging → intermediate → mart que se referencian por nombre. También exige separar **quién transforma** de **dónde se almacena**, principio del lakehouse. Ejemplo: calcula DuckDB y guarda en DuckLake (catálogo en PostgreSQL, datos en S3) o en ClickHouse. dbt y SQLMesh son Python y no pueden embeberse en el servidor Rust; M02 desaconseja mezclar su runtime y su programador con el núcleo.

## Decisión
1. **ELT propio en Rust.** Cada modelo es un `models/**/*.sql` con una cabecera `MODEL (...)` **compatible con la de SQLMesh** (subconjunto: `name`, `kind`, `cron`, `grain`, `audits`, `description`, `columns`) más dos propiedades:
   - `engine`: **quién calcula** (`duckdb` por defecto, o `clickhouse`). El SQL se escribe **en el dialecto de este motor**.
   - `output`: **dónde se guarda** el resultado; es una conexión de `connectors/`. Si no se indica, se usa la conexión del proyecto (0004).

   Las propiedades desconocidas son un error, no se ignoran.
   ```sql
   MODEL (name mart.ventas_por_region, kind FULL, engine duckdb, output clickhouse);
   SELECT region, sum(importe) AS total FROM intermediate.ventas_limpias GROUP BY region;
   ```
2. **Combinaciones admitidas:**

   | Calcula ↓ / Guarda → | DuckLake | ClickHouse | PostgreSQL |
   |---|---|---|---|
   | **DuckDB** | nativo: transacción y versión de DuckLake | el resultado sale como Arrow y se inserta (`INSERT … FORMAT ArrowStream`) en una tabla nueva que sustituye a la anterior de forma atómica (`EXCHANGE TABLES`) | extensión `postgres` en modo escritura, en una transacción |
   | **ClickHouse** | más adelante: Parquet en S3 registrado en DuckLake por DuckDB (`ducklake_add_data_files`) | nativo (`MergeTree`, con `ORDER BY`/`PARTITION BY`/`TTL` declarables) | — |

   Cuando un destino no puede recibir directamente lo que produce el motor, se usa una **zona intermedia** declarada (por ejemplo, S3), como `stage` en Rill.
3. **Tipos de modelo:** `VIEW`, `FULL`, `INCREMENTAL_BY_TIME_RANGE` (con relleno automático de intervalos) e `INCREMENTAL_BY_UNIQUE_KEY`. Una `VIEW` solo existe en el motor que la calcula.
4. **Capas por convención:** `models/staging/`, `models/intermediate/` y `models/marts/`. El esquema sale del nombre (`mart.ventas`).
5. **Dependencias (DAG):** se deducen de las tablas que lee cada SQL, analizadas por **el propio motor**: DuckDB con `json_serialize_sql`; ClickHouse con `EXPLAIN AST`. Un modelo puede leer todo lo que su motor sabe leer: DuckDB lee DuckLake, PostgreSQL, `.duckdb` y Parquet en S3. El plan comprueba, antes de ejecutar, que cada entrada es accesible desde ese motor.
6. **Sin traducción automática de SQL entre dialectos:** falla en casos límite (funciones, fechas, tipos). Cada motor valida su propio SQL al guardar. Las **métricas** (`metrics/*.yaml`) sí son neutrales: el motor semántico genera el SQL de cada dialecto (0002).
7. **Lo mejor de cada referente:**
   - **plan antes de aplicar:** cambios, impacto en modelos, métricas y dashboards, y clasificación en "rompe" o "no rompe";
   - **entornos de desarrollo sin recalcular**, sobre las versiones de DuckLake;
   - **auditorías antes de publicar:** dentro de la transacción cuando el destino la tiene (DuckLake, PostgreSQL); en ClickHouse, sobre la tabla nueva antes del intercambio atómico;
   - **linaje** de modelos y columnas;
   - **recarga en caliente** al guardar.
8. **Interoperabilidad (más adelante):** importadores de proyectos SQLMesh y dbt (`manifest.json`) para facilitar la migración (M10).

## Alcance ligero, sin sobreingeniería (requisito del propietario)
**Primera entrega en G3, lo imprescindible:**
- cabecera `MODEL` con `name`, `kind`, `engine`, `output`, `cron` y `audits`;
- tipos `VIEW`, `FULL` e `INCREMENTAL_BY_TIME_RANGE`;
- DAG;
- **plan** (qué cambia y a qué afecta) y ejecución con auditorías;
- motor DuckDB con destino DuckLake.

**Solo cuando un caso real lo justifique, cada uno con su propio Issue:** destinos ClickHouse y PostgreSQL, cálculo en ClickHouse, `INCREMENTAL_BY_UNIQUE_KEY`, entornos de desarrollo sin recalcular, linaje por columna e importadores de dbt y SQLMesh. Si una pieza de la sección anterior no cabe en la primera entrega, queda para entonces.

**Al implementar, se revisa el código fuente de SQLMesh** (tag fijado, `[CÓDIGO]`, siguiendo `docs/referentes/README.md`): cómo analiza la cabecera `MODEL`, cómo construye el DAG, cómo calcula el plan y clasifica los cambios, cómo gestiona los intervalos de los incrementales y cómo ejecuta las auditorías. Se toman sus soluciones probadas en versión mínima, y el PR deja constancia de qué se tomó y qué se dejó fuera.

## Alternativas descartadas
- **Embeber dbt o SQLMesh:** exige un runtime Python en el despliegue y su propio estado y programador. Además, las plantillas Jinja de dbt se interpolan dentro del SQL. dbt Fusion (Rust) no es software libre en su totalidad.
- **Conexión única por modelo (calcular y guardar en el mismo sitio):** impide el patrón más útil: transformar con DuckDB y servir desde ClickHouse.
- **Escribir todo en un dialecto y traducirlo automáticamente:** resultados distintos en casos límite y errores difíciles de diagnosticar.

## Consecuencias
- G3 implementa el cargador de `models/`, el DAG, el plan, el cálculo con DuckDB y su destino DuckLake, y las auditorías. Los destinos ClickHouse y PostgreSQL, y el cálculo en ClickHouse, llegan con esas conexiones (0002).
- Las macros de fechas de los incrementales (`@start_ds`, `@end_ds`) solo producen literales calculados por la plataforma, nunca texto del usuario.
