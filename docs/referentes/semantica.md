# Motor semántico: cómo lo resuelven los grandes

Estudio previo a G2 (Issue #41), según ADR 0006 (SEF) y la regla de referentes de AGENTS.md: código fuente cuando es abierto y documentación oficial cuando no.

## Fuentes
| Herramienta | Tipo | Versión o commit | Dónde mirar |
|---|---|---|---|
| Cube | `[CÓDIGO]` | `cube-js/cube` `e15efc5` (2026-10-09) | `rust/cube/cubesqlplanner` (Tesseract, Rust): `logical_plan/aggregate_multiplied_subquery.rs`, `keys_subquery.rs`, `planner/join_tree.rs`, `planner/multi_fact_join_groups.rs`; `packages/cubejs-schema-compiler/src/compiler/JoinGraph.ts` |
| Malloy | `[CÓDIGO]` | `malloydata/malloy` `b90c216` (2026-10-07) | `packages/malloy/src/dialect/dialect.ts` y `duckdb/duckdb.ts` (`sqlSumDistinct`, `sqlAggDistinct`), `lang/ast/expressions/expr-asymmetric.ts` |
| dbt MetricFlow | `[CÓDIGO]` | `dbt-labs/metricflow` `a8d263d` (2026-09-29), Apache-2.0 | `metricflow_semantics/.../semantic_model_join_evaluator` (`_VALID_ENTITY_JOINS`), `metricflow/validation/dataflow_join_validator.py` |
| Rill | `[CÓDIGO]` | v0.90.5 | `runtime/metricsview` (ver `rill.md`) |
| Looker | `[DOCS]` | — | *Symmetric aggregates*, `access_filter`, *aggregate awareness* |
| Power BI | `[DOCS]` | — | Relaciones (cardinalidad, dirección del filtro, relaciones inactivas), contexto de filtro de DAX y `CALCULATE`, RLS/OLS, *aggregations* |
| Tableau | `[DOCS]` | — | *Relationships* (capa lógica), expresiones LOD (`FIXED`/`INCLUDE`/`EXCLUDE`) |
| Snowflake, Databricks | `[DOCS]` | — | *Semantic views*, *metric views* |

## Capacidad por capacidad

| Capacidad (caso del corpus) | Cómo lo resuelven | Qué hacemos en SEF |
|---|---|---|
| ***Fanout*** (CASE-004): una unión uno-a-muchos duplica la medida del lado "uno" | **Looker:** *symmetric aggregates* con `SUM(DISTINCT hash(pk)·K + valor) − SUM(DISTINCT hash(pk)·K)`; necesita clave primaria y pierde precisión con decimales grandes. **Cube:** marca como *multiplicado* el cubo alcanzable por una arista `hasMany`/`belongsTo` (`findMultiplicationFactorFor`), saca sus claves únicas (`KeysSubQuery`) y agrega la medida aparte uniéndola por clave: exacto. **Malloy (DuckDB):** agrega los pares distintos `{clave, valor}` en una sola pasada (`list(distinct …)` + `UNNEST`): exacto. **MetricFlow:** prohíbe las uniones que multiplican; solo une hacia un lado único. **Tableau:** agrega cada tabla en su propio nivel de detalle antes de combinar. **Rill:** una sola tabla desnormalizada por métrica; evita el problema limitando el modelo. | El motor de corrección **demuestra** el grano y la ausencia de *fanout* antes de planificar (SEF 04 §8). Estrategia base, portable y exacta: **pre-agregar cada hecho en su grano y unir después**, como Cube, Tableau y MetricFlow. Optimización por dialecto: pares distintos de Malloy en DuckDB, marcada como `EmulatedExact` y probada contra la base. **Nunca** el *hash* de Looker: no es exacto con DECIMAL. |
| **Camino de uniones** (CASE-005, CASE-016) | **Cube:** camino más corto (Dijkstra) entre cubos; si hay varios caminos, elige uno en silencio. **Power BI:** una sola relación activa entre dos tablas; las demás se activan explícitamente (`USERELATIONSHIP`). **MetricFlow:** resuelve por entidades tipadas (primaria, única, foránea, natural). | Determinista y **explícito**: si hay más de un camino válido, se **rechaza** con diagnóstico (CASE-016), salvo que la métrica o la consulta elijan el camino. Relaciones con cardinalidad declarada (Ossie `relationships` + extensión). |
| **Varios hechos** (CASE-009 mismo grano, CASE-010 distinto grano) | **Cube:** agrupa las medidas por conjuntos de uniones (`multi_fact_join_groups`) y combina con `FULL OUTER JOIN` sobre las dimensiones (`full_key_aggregate`). **MetricFlow:** agrega cada medida por separado y combina por las claves de agrupación. **Tableau:** relaciones contextuales. | El mismo patrón: un bloque por hecho, en su grano, y una alineación final demostrada. Un grano más fino nunca se inventa desde uno más grueso (SEF 04 §8.2). |
| **Tipos de métrica** (CASE-006 ratio, CASE-008 distintos, CASE-021 acumulada, CASE-007 semiaditiva) | **MetricFlow:** `simple`, `ratio`, `cumulative`, `derived` y `conversion` (embudos). **Cube:** medidas `rolling_window` y multietapa con desplazamiento temporal. **Power BI:** inteligencia de tiempo de DAX (`DATESYTD`, `SAMEPERIODLASTYEAR`) y saldos semiaditivos (`LASTNONBLANK`). **Rill:** `derived`, `time_comparison` y ventanas. | Álgebra de métricas de SEF: los ratios se **recalculan** (nunca se suman ratios), los conteos distintos no son aditivos, hay acumuladas y semiaditivas. **Candidatos a casos nuevos del corpus**, por ADR: comparación con el periodo anterior, métrica de conversión (embudo) y dimensión con varios roles (fecha de pedido y fecha de envío sobre el mismo calendario, como las relaciones inactivas de Power BI). |
| **Contexto de filtro** | **Power BI:** contexto de filtro y de fila; `CALCULATE` lo modifica. **Atlas BI 14:** AST de filtros (filtro del autor ≠ clic del usuario). **Cube:** filtros sobre medidas en `HAVING`. | Filtros tipados (AST) distinguiendo los de fila (antes de agregar) de los de agregado (después, que SEF ya incluye como predicados posteriores a la agregación), y el filtro del autor del de la sesión. |
| **Seguridad** (CASE-015) | **Power BI:** RLS con filtros DAX por rol y OLS. **Looker:** `access_filter` con atributos del usuario. **Cube:** *security context* y reescritura de consulta. **Rill:** `row_filter` con plantillas de texto. | Políticas obligatorias que el planificador no puede saltarse (CASE-015). Atributos del usuario **enlazados como parámetros**, nunca interpolados como texto (mejora sobre Rill y Looker). Visibilidad de campos (OLS). |
| **Pre-agregaciones** (CASE-018) | **Cube:** el optimizador de Tesseract sustituye un subplan por una pre-agregación compatible (`pre_aggregation_override`). **Rill:** *rollups*. **Power BI:** *aggregations*. **Looker:** *aggregate awareness*. | Solo se usa una materialización si es **semánticamente equivalente** (CASE-018) y el resultado no cambia. Llega en G4. |
| **Dialectos** (CASE-019) | **Malloy:** una clase `Dialect` con indicadores de capacidad (`supportsSumDistinctFunction`, `supportsAggDistinct`, `supportsQualify`…) y una implementación por motor (no incluye ClickHouse). **Cube:** controladores por motor, ClickHouse incluido. | Capacidades por motor como en SEF (`NativeExact`, `EmulatedExact`, `Unsupported`) más indicadores al estilo Malloy. DuckDB primero y ClickHouse después (ADR 0002). |

## Conclusiones para G2
1. **SEF ya cubre el problema con más rigor que cualquiera de estas herramientas.** Ninguna demuestra la corrección antes de planificar: Cube elige caminos en silencio, Looker aproxima con *hash*, MetricFlow y Rill prohíben o limitan el modelo. Mantenemos el plan de las fases 1 y 2 de SEF.
2. **Tomamos tres implementaciones probadas:**
   - la descomposición de Cube (claves únicas y medida aparte) como estrategia de *fanout* portable;
   - los pares distintos de Malloy como optimización exacta en DuckDB;
   - los indicadores de capacidad por dialecto de Malloy.
3. **Mejoramos en tres puntos:**
   - rechazo de caminos ambiguos en lugar de elegirlos en silencio;
   - exactitud DECIMAL de punta a punta, sin el *hash* de Looker;
   - atributos de seguridad enlazados como parámetros.
4. **Propuesta de 3 casos nuevos para el corpus** (se añaden por ADR, con sus datos y su resultado esperado): comparación con el periodo anterior, métrica de conversión (embudo) y dimensión con varios roles.
