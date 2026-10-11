# Apache Ossie: cómo construimos sobre el estándar

**Fuente:** `github.com/apache/ossie`, commit `671e7464` (2026-10-11), `[CÓDIGO]` y `[DOCS]`.
- Especificación núcleo: `core-spec/spec.md`, versión **0.2.0.dev0 (borrador, mutable)**, con JSON Schema en `core-spec/ossie-schema.json`.
- Lenguaje de expresiones: `core-spec/expression_language.md`, estado *Proposed Final*. Su grupo de trabajo incluye Snowflake, Databricks, dbt Labs, Cube, Lightdash, ThoughtSpot, Salesforce, Starburst y Malloy.
- Última etiqueta publicada: `osi-0.1.1-rc1`.

**Decisión de base:** compatibilidad con Ossie por diseño (memoria del proyecto; M10 §6.2: "primera apuesta de interoperabilidad semántica").

## Lo que Ossie ya resuelve y adoptamos tal cual
| Ossie | En nuestra plataforma |
|---|---|
| Un documento = un modelo semántico (`version`, `name`, `datasets`, `relationships`, `metrics`) | Un `metrics/<modelo>.yaml` es **un documento Ossie válido** (ADR 0003) |
| `datasets[].source` (tabla, vista o consulta) + `primary_key`/`unique_keys` | Apunta a un mart de `models/` o a una tabla de una conexión en vivo (ADR 0002) |
| `fields[]` con `expression`, `datatype` y `dimension.is_time` (tipo y rol separados) | Dimensiones; el rol temporal es independiente del tipo, como en Snowflake y LookML |
| `relationships[]` (muchos → uno, claves compuestas) | El grafo de uniones del compilador: consultas entre varios datasets sin escribir joins |
| `metrics[]`: expresiones agregadas que pueden cruzar datasets | Medidas del motor semántico |
| `DataType`: String, Integer, Decimal, Float, Boolean, Date, Time, DateTime, DateTimeTz, Opaque | Amplía los tipos del contrato de resultados (hoy: string, integer, decimal, timestamptz) |
| `ai_context` en cada nivel | Instrucciones y sinónimos para el asistente de IA (G5; Rill R14) |
| **`OSSIE_SQL_2026`**: SQL portable con agregaciones, fechas, texto, matemáticas, CASE y ventanas marcadas como REQUIRED o RECOMMENDED | **Lenguaje de autoría de métricas y campos.** El compilador lo traduce a DuckDB y, después, a ClickHouse (ADR 0002). Cubre exactamente el "métricas sin dialecto" que decidimos. |
| `custom_extensions` (`vendor_name` + JSON) | Todo lo que Ossie no cubre, en nuestro espacio de nombres (tabla siguiente) |

**Ventaja estratégica:** el repositorio trae **convertidores** (`converters/`) para Cube, dbt, Microsoft (Power BI), Snowflake, Salesforce (Tableau), Databricks, Omni, Sigma, ThoughtSpot, Hex, GoodData y Holistics. Si nuestros modelos son Ossie nativos, la importación y exportación con esos ecosistemas sale casi gratis.

## Lo que Ossie todavía no tiene: nuestras extensiones
Van en `custom_extensions`, con un `vendor_name` propio. Mientras el producto no tenga nombre, se usa `bi-platform` de forma provisional y se cambia en una sola migración. Cada extensión tiene un esquema versionado.

| Extensión | Origen de la idea | Gate |
|---|---|---|
| Semántica de tiempo: grano mínimo, primer día de la semana o mes del año, zona horaria, rangos ISO 8601 | Rill R3, LookML `dimension_group` | G2 |
| Tipos de medida: derivada, comparación de periodos, ventana, porcentaje del total | Rill R2, Power BI (DAX time intelligence) | G2 |
| Seguridad: acceso, filtro de filas con atributos **enlazados como parámetros**, campos visibles | Rill R4, Power BI RLS/OLS, Looker access filters | G2 |
| Formatos de presentación (moneda, porcentaje, d3) | Rill, Power BI | G2 |
| `connector`: dónde se calcula el modelo | ADR 0002/0004 | G2 |
| Rollups con enrutado automático | Rill R5, Cube pre-aggregations | G4 |
| Revisiones y certificación | M10 | G2/G3 |

## Riesgos y cómo los cubrimos
- **La especificación es borrador (0.2.0.dev0).** Fijamos un commit y aislamos la conversión en un único módulo, con pruebas de ida y vuelta (documento → modelo interno → documento) y una **lista explícita de pérdidas**. M10 punto 37: el adaptador de una versión dev no se declara estable.
- **El enum de dialectos no incluye DuckDB ni ClickHouse.** La autoría va en `OSSIE_SQL_2026` (portable). Si hace falta una expresión nativa de un motor, va en nuestra extensión, nunca como dialecto inventado.
- **Ossie define la declaración, no la consulta** (`expression_language.md`, "Model Declaration vs. Querying"). La API de consultas semánticas es nuestra (M04/M05) y sigue el contrato tipado de G1.
- **Temas abiertos en su hoja de ruta:** grano o entidad como concepto propio, filtros semánticos, cardinalidad explícita y métricas acumuladas. Los cubrimos con extensiones mientras tanto, diseñadas para converger con lo que publique el estándar.

## Primeros pasos en G2
1. Cargador de `metrics/*.yaml` validado contra `ossie-schema.json` (commit fijado), con errores por archivo y línea.
2. Analizador de `OSSIE_SQL_2026`: subconjunto REQUIRED que usan las métricas, con pruebas de propiedades como el compilador de G1.
3. Compilador semántico → **capa de dialectos** → DuckDB, con el mismo contrato exacto y las mismas protecciones (valores enlazados, presupuesto, plazo) que `/api/v1/queries`.
4. Pruebas de ida y vuelta con los ejemplos de `examples/` de Ossie y con el convertidor de dbt o Cube.
