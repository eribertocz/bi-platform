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
