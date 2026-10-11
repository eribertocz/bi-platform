# 0005 — Modelos SQL estilo SQLMesh, ejecutados en el motor de su conexión

**Estado:** aprobada por el propietario, 2026-10-11. **Amplía:** 0001 (los modelos con conexión ClickHouse se ejecutan en ClickHouse) y M02 (formato de `ModelDefinition`). **Referentes:** SQLMesh (formato y plan), dbt (pruebas, linaje), Rill (recarga en caliente, incrementales, modelos que escriben en ClickHouse).

## Contexto
El propietario prefiere cómo SQLMesh declara los modelos: SQL con una cabecera declarativa y capas staging → intermediate → mart que se referencian por nombre, con el mart creando la tabla en el motor elegido. dbt y SQLMesh son Python y no pueden embeberse en el servidor Rust (M02 desaconseja mezclar su runtime y su programador con el núcleo).

## Decisión
1. **ELT propio en Rust.** Cada modelo es un `models/**/*.sql` con una cabecera `MODEL (...)` **compatible con la de SQLMesh** (subconjunto: `name`, `kind`, `cron`, `grain`, `audits`, `description`, `columns`), más `connector` (opcional; si no, la conexión del proyecto, 0004). Las propiedades desconocidas son un error, no se ignoran.
2. **Tipos de modelo:** `VIEW`, `FULL`, `INCREMENTAL_BY_TIME_RANGE` (con relleno automático de intervalos) e `INCREMENTAL_BY_UNIQUE_KEY`.
3. **Capas por convención:** `models/staging/`, `models/intermediate/` y `models/marts/`. El esquema sale del nombre (`mart.ventas`). El orden de ejecución (DAG) se deduce de las tablas que lee cada SQL, analizadas por **el propio motor**: DuckDB con `json_serialize_sql`; ClickHouse con su analizador (`EXPLAIN AST`).
4. **Cada modelo se ejecuta en el motor de su conexión y se escribe en su dialecto:**

   | Conexión | Ejecuta | Resultado | Publicación |
   |---|---|---|---|
   | DuckLake | nuestro DuckDB | tabla o vista en el lago | atómica, con versión de DuckLake |
   | ClickHouse | ClickHouse del cliente | tabla `MergeTree` con `ORDER BY`/`PARTITION BY` declarables | reemplazo atómico (`EXCHANGE TABLES`) |

   No se traduce SQL arbitrario entre dialectos: esa traducción falla en casos límite (funciones, fechas, tipos). Cada motor valida su propio SQL al guardar. Las **métricas** (`metrics/*.yaml`) sí son neutrales: el motor semántico genera el SQL de cada dialecto (0002).
5. **Un modelo solo referencia modelos de su misma conexión.** Para combinar motores, un modelo de **importación explícita** copia los datos (por ejemplo, de ClickHouse a DuckLake).
6. **Lo mejor de cada referente:**
   - **plan antes de aplicar:** cambios, impacto en modelos, métricas y dashboards, y clasificación en "rompe" o "no rompe";
   - **entornos de desarrollo sin recalcular**, sobre las versiones de DuckLake;
   - **auditorías dentro de la transacción de publicación;**
   - **linaje** de modelos y columnas;
   - **recarga en caliente** al guardar.
7. **Interoperabilidad (más adelante):** importadores de proyectos SQLMesh y dbt (`manifest.json`) para facilitar la migración (M10).

## Alternativas descartadas
- **Embeber dbt o SQLMesh:** exige un runtime Python en el despliegue y su propio estado y programador. Además, las plantillas Jinja de dbt se interpolan dentro del SQL. dbt Fusion (Rust) no es software libre en su totalidad.
- **Escribir todo en un dialecto y traducirlo automáticamente:** resultados distintos en casos límite y errores difíciles de diagnosticar.

## Consecuencias
- G3 implementa el cargador de `models/`, el DAG, el plan, la ejecución en DuckLake y las auditorías. La ejecución en ClickHouse llega cuando se añada esa conexión (0002).
- Las macros de fechas de los incrementales (`@start_ds`, `@end_ds`) solo producen literales calculados por la plataforma, nunca texto del usuario.
