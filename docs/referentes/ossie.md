# Apache Ossie: cómo construimos sobre el estándar

**Fuente:** `github.com/apache/ossie`, commit `671e7464` (2026-10-11), `[CÓDIGO]` y `[DOCS]`.
- Especificación núcleo: `core-spec/spec.md`, versión **0.2.0.dev0 (borrador, mutable)**, con JSON Schema en `core-spec/ossie-schema.json`.
- Lenguaje de expresiones: `core-spec/expression_language.md`, estado *Proposed Final*. Su grupo de trabajo incluye Snowflake, Databricks, dbt Labs, Cube, Lightdash, ThoughtSpot, Salesforce, Starburst y Malloy.
- Última etiqueta publicada: `osi-0.1.1-rc1`.
- Estructura completa del estándar, lenguaje de expresiones, propuestas en curso y lo que le falta: [`ossie-especificacion.md`](ossie-especificacion.md).

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

## Estudio a fondo (2026-10-11, mismo commit)
Leídos: `core-spec/spec.md`, `core-spec/expression_language.md`, `ontology/ontology.md`, `validation/validate.py`, `converters/README.md`, `converters/cube/README.md` y `ROADMAP.md`.

### Detalles normativos que el adaptador debe cumplir
- **Documento:** `version`, `name` y `datasets` no vacío son obligatorios; no se admiten propiedades adicionales. Un documento contiene un solo modelo; la forma antigua `semantic_model: [...]` se rechaza y se migra a archivos separados, **nunca** quedándose en silencio con el primer modelo.
- **YAML:** las claves duplicadas se rechazan. Un YAML normal se queda con la última en silencio, y eso cambiaría una métrica sin aviso.
- **Identificadores:** los no entrecomillados no distinguen mayúsculas y se normalizan a mayúsculas; los entrecomillados (`"…"`) son exactos. `"ID"` coincide con `id`, pero `"id"` no. Las referencias multiparte (`dataset.campo`) siguen el SQL estándar. Máximo 128 caracteres.
- **`OSSIE_SQL_2026`** se basa en ANSI SQL:2003 Core y es el dialecto por defecto.
  - **Admite:** operadores, `BETWEEN`, `IN` (solo listas), `LIKE`/`ILIKE`, `IS [NOT] NULL`, `IS [NOT] DISTINCT FROM`, `CASE`, agregados, ventanas y funciones escalares REQUIRED de fecha, texto, matemáticas, condicionales y `CAST`; además `TRY_CAST`, regex y aproximados, que son RECOMMENDED.
  - **No admite:** `SELECT`, `FROM`, `JOIN`, `GROUP BY` (lo decide el grano), `WHERE` (filtros aparte), subconsultas (solo `EXISTS_IN()`), CTEs, operaciones de conjuntos ni DDL/DML.
- **Descomponibilidad de los agregados** (base de las pre-agregaciones y del *fanout*):
  - distributivos: `SUM`, `COUNT`, `MIN`, `MAX`;
  - algebraicos: `AVG`, `STDDEV`, `VARIANCE`;
  - holísticos: `MEDIAN`, percentiles, `COUNT DISTINCT`;
  - de bocetos: `APPROX_*`, aproximados por definición, que **nunca** sustituyen a un exacto sin que la métrica lo declare.
- **Relaciones:** siempre **muchos → uno** (`from` es el lado "muchos" y `to` el "uno"), con claves compuestas ordenadas. No hay cardinalidad explícita ni muchos-a-muchos: un puente es un dataset más, que SEF trata con asignación (CASE-011). Varias relaciones entre el mismo par son posibles, y SEF **rechaza** la ambigüedad (CASE-016).
- **Validación oficial** (`validate.py`), que replicamos en Rust: esquema JSON (Draft 2020-12); nombres únicos de datasets, campos, métricas y relaciones; referencias de relación válidas; misma aridad en `from_columns` y `to_columns`; y sintaxis SQL.

### El *fanout* no es cosa del estándar, es cosa del motor
El convertidor de Cube lo explica: una expresión de Ossie es **SQL estático**, y `SUM(orders.amount)` no sabe si una unión duplicará filas. La corrección la tiene que poner el **motor**: Cube con su reescritura por claves primarias, nosotros con el motor de corrección de SEF.

Lista útil del convertidor: **agregados idempotentes ante duplicación** (seguros con *fanout*): `MIN`, `MAX`, `APPROX_COUNT_DISTINCT`, `BOOL_OR`/`BOOL_AND`, `BIT_OR`/`BIT_AND` y **cualquier agregado sobre `DISTINCT`**. Es una lista de permitidos, nunca de prohibidos: el conjunto de funciones está abierto, y una lista de prohibidos declara seguro en silencio `STDDEV` o `MEDIAN`. La seguridad se juzga **por agregado y por dataset sobre la expresión completamente expandida**, no por el tipo declarado de la métrica.

### La capa de ontología (futuro, IA)
`ontology.md` define conceptos (`EntityType`/`ValueType`), relaciones con roles y multiplicidades (`ManyToOne`, `OneToOne`), identificadores preferentes (`identify_by`), derivaciones y documentos de **mapeo** que enlazan un modelo semántico con una ontología. Encaja con el asistente IA (M16, G5): no entra en G2, pero CSMIR no debe impedirlo.

### Oportunidad estratégica
La hoja de ruta de Ossie, en "Future Efforts", pide un **lenguaje de consulta semántica estándar y un motor de referencia**:
- compilador Ossie → SQL;
- manejo canónico de uniones, agregaciones y filtros;
- **una batería de conformidad entre implementaciones**.

Es lo que es SEF, con su corpus de 21 casos. Con Ossie nativo y el corpus como conformidad, bi-platform puede posicionarse como **implementación de referencia** e incluso aportar el corpus al proyecto Apache. Decisión de producto pendiente del propietario: si y cuándo publicarlo.

### Otras implementaciones (verificadas el 2026-10-11)
| Implementación | Qué hace | Verificación |
|---|---|---|
| **`iqea-ai/duckdb-ossie`** (C++, MIT, commit `f8cf057`) | Extensión comunitaria de DuckDB: `ossie_load`, `ossie_query`, `ossie_compile`, MCP. | `[CÓDIGO]` leído (`src/compile/compile.cpp`, `src/model/graph.cpp`). **Correcta rechazando:** solo une por aristas que no duplican filas desde el dataset del grano; rechaza caminos ambiguos, métricas de **varios granos** en una consulta ("not supported yet") y agregados sin columna (`COUNT(*)` sin calificar). **Deduce la cardinalidad de las claves declaradas**, lo que adoptamos. SEF va más allá: alinea varios hechos, asigna en puentes y trata las semiaditivas. |
| dbt Core ≥ 1.12 | Lee documentos Ossie de `osi/`. | `[DOCS]`; solo versiones **0.1.0/0.1.1**, no 0.2.0.dev0. |
| Lightdash | Importación y exportación Ossie anunciadas. | `[DOCS]` |
| `Dactopus/dactopus-ossie-clickhouse` (Python, Apache-2.0) | Modelo Ossie sobre ClickHouse. | Existe; útil al llegar ClickHouse (ADR 0002). |
| **Dosi** de Datus (Rust) | Motor Ossie nativo: valida contra el esquema y compila métricas, dimensiones y filtros a SQL de 16 dialectos; CLI, MCP, REST/Arrow. | **Existe y es propietario:** `Datus-ai/dosi-dist` solo distribuye binarios ("Dosi is proprietary; no source is published here"). No se puede estudiar su código; como mucho, compararlo como caja negra. Es la competencia directa en Rust. |
| `spiculedata/ossie` (Java, Apache-2.0) | Librería JVM para leer y escribir Ossie, con generación de SQL vía Apache Calcite. | Existe. |

**Debate abierto sobre cómo consultar** ([datalakehousehub](https://datalakehousehub.com/blog/ossie-semantic-query-interface-agents)): un modelo de consulta restringido frente a SQL normal con un agregado `MEASURE()`. El grupo de trabajo converge hacia un diseño **por capas que mantiene ambos**. Lo mismo haremos: SQIR (#44) como consulta estructurada, y después SQL sobre métricas con `MEASURE()`, como Cube, Rill `metrics_sql` y Atlas BI A2.

### Convergencia de nuestras extensiones con su hoja de ruta
| Su trabajo abierto | Nuestra extensión hoy | Regla |
|---|---|---|
| Métricas derivadas y acumuladas; grano y entidades explícitos | Tipos de medida y grano de CSMIR | Mismos nombres que proponga el estándar al publicarse; migración por ADR |
| Filtros semánticos reutilizables | AST de filtros (Atlas BI A1) | Igual |
| `is_time` → `dimension_type`, jerarquías, calendarios | Semántica de tiempo (Rill R3) | Igual |
| Identificadores estables; métricas certificadas | `StableId` y huellas de CSMIR; certificación (M10) | Igual |

## Primeros pasos en G2 (Issue #48)
1. **Carga:** YAML o JSON con rechazo de claves duplicadas, a estructuras tipadas que reflejan `ossie-schema.json` con `deny_unknown_fields`. Paridad comprobada con los casos de `validation/tests` y los ejemplos `tpcds` y `flights`. Errores por archivo, línea y ruta.
2. **Validación** equivalente a `validate.py`: nombres únicos, referencias, aridad y expresiones.
3. **Expresiones `OSSIE_SQL_2026`:** se analizan con un parser SQL probado (no uno propio) y se recorre el AST para:
   - aceptar solo las construcciones permitidas;
   - normalizar identificadores;
   - resolver referencias a campos y métricas;
   - clasificar cada agregado (distributivo, algebraico, holístico, idempotente ante duplicación).

   Propiedades con proptest.
4. **Ossie → CSMIR:** relaciones muchos-a-uno con cardinalidad *declarada sin verificar*, métricas descompuestas en medidas primitivas y nuestras extensiones (`vendor_name: bi-platform`) para tiempo, unidades, políticas, formatos y conexión.
5. **CSMIR → Ossie** para exportar, con **lista explícita de pérdidas**. Prueba de ida y vuelta sobre los modelos del corpus: la misma huella de CSMIR.

## Cobertura por fases (objetivo del propietario, 2026-10-11)
Lista de comprobación viva de lo que el motor entiende de Ossie 0.2.0.dev0.

**Leyenda:**
- ✅ **Hecho:** con pruebas, en `main` o en un PR abierto.
- 🔄 **En curso.**
- ⏳ **Pendiente,** con su Issue.

**Formatos:** YAML y JSON, en lectura y en escritura (`sef_ossie::load`, `to_yaml`, `to_json`; YAML 1.2 contiene JSON). La prueba `tests/conformance.rs` valida cada documento emitido contra `ossie-schema.json` y comprueba que vuelve igual desde YAML y desde JSON.

| Fase | Capacidad | Estado |
|---|---|---|
| Básica | `version`, `name`, `datasets`, `source`, `fields`, `metrics`, `expression.dialects`; validación contra el esquema oficial | ✅ (#51, #54) |
| Básica | Métrica simple (`SUM(orders.amount)`) | ✅ Ossie → CSMIR (#63) |
| Básica | Calcular `ANSI_SQL` cuando falte `OSSIE_SQL_2026` | ⏳ #74 |
| Básica | SQL generado desde el `source` del dataset | ⏳ planificador #46 y adaptador DuckDB #47 |
| Intermedia | `description`; `primary_key`; `unique_keys`; `relationships` (también compuestas); `datatype` (comprobado contra el catálogo) | ✅ (#63) |
| Intermedia | `dimension.is_time`; `ai_context` (texto o `instructions`/`synonyms`/`examples`) | ✅ validados (#51). ⏳ Llegan a la API, la UI y la IA con #75. |
| Intermedia | Varios dialectos | ✅ se conservan. Solo se ejecuta el portable (`OSSIE_SQL_2026` y, con #74, `ANSI_SQL`); los de otros motores nunca se ejecutan en DuckDB. |
| Intermedia | *Joins* automáticos por `relationships`; GROUP BY y filtros por dimensión | 🔄 consulta semántica (#73). ⏳ Corrección #45, planificador #46 y ejecución #47. |
| Avanzada | Ratios, CASE, ventanas acumuladas, métricas entre datasets, campos calculados, `custom_extensions`, validación semántica, `Opaque` | ✅ los 21 casos del corpus (#63, #72) |
| Avanzada | SQL sobre métricas (estilo Rill/MetricFlow) | ⏳ se compila a SQIR (#73) |
| Avanzada | Exportación e importación (MetricFlow, Cube, LookML…) | ⏳ CSMIR → Ossie (#48, parte 4) y, después, los conversores del proyecto Ossie |
| Completa | Seguridad por filas | ✅ en el modelo (#72). ⏳ Aplicada con #69 (ADR 0010). |
| Completa | Linaje, frescura y confianza | ✅ dependencias tipadas en CSMIR. ⏳ Frescura por instantánea de DuckLake y certificación (M10). |
| Completa | Pre-agregaciones | ⏳ G4 (medidas de *rollup* ya modelables, caso 018) |
| Completa | Lenguaje de expresiones completo, varios modelos y ontología | ⏳ cuando el estándar lo publique; se seguirá su hoja de ruta (tabla anterior) |
