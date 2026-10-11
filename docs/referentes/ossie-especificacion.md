# Apache Ossie: especificación completa y estado del estándar

**Qué es:** el estudio de referencia de Apache Ossie para implementar bi-platform. Recoge la estructura completa del estándar, sus reglas, su lenguaje de expresiones y su ecosistema, y también lo que todavía falta o está en desarrollo en el proyecto, para tenerlo en cuenta en el futuro. El documento [`ossie.md`](ossie.md) explica *cómo construimos sobre* Ossie; este explica *qué es* Ossie.

**Fuente estudiada (2026-10-11):**
- `github.com/apache/ossie`, commit `8e378b2`: `core-spec/spec.md`, `ossie-schema.json`, `spec.yaml`, `expression_language.md`, `ontology/ontology.md`, `validation/validate.py`, `converters/README.md`, `ROADMAP.md`, `docs/working_groups.md`, `bi-sql-examples/`, `cli/` y `python/`;
- los 40 PR abiertos de ese día. Se citan por número; los que son propuestas, **no** son estándar.

---

## 1. Ficha

| | |
|---|---|
| Nombre | Apache Ossie (incubating), antes *Open Semantic Interchange* (OSI) |
| Versión | `0.2.0.dev0`, **borrador mutable** ("do not depend on this version in production"). Última publicada: `0.1.1` (2025-12-11) |
| Licencia y gobierno | Apache-2.0, en incubación en la ASF. Los cambios a la especificación se discuten en `dev@ossie.apache.org` y se votan |
| Grupos de trabajo activos | 1) Lenguaje de métricas y relaciones (Will Pugh, Snowflake); 2) Catálogo (Atlan); 3) Ontología (RelationalAI); 4) Semántica común de servicios financieros (Snowflake) |
| Participantes del lenguaje de expresiones | Snowflake, Malloy, AtScale, Salesforce, dbt Labs, RelationalAI, Databricks, Cube, ThoughtSpot, Lightdash, Starburst, Denodo, ASF |
| Formatos | **YAML y JSON**: misma estructura, un documento = un modelo |

## 2. Arquitectura por capas (propuesta #452, en revisión)

| Capa | Nombre | Qué define | Estado |
|---|---|---|---|
| 1 | Definición del modelo | El formato (`spec.md`) y el lenguaje de expresiones (`expression_language.md`) | **Vigente** (borrador 0.2) |
| 2 | Interfaz de consulta relacional | SQL normal con `MEASURE(nombre)` para agregar cada medida a su propio grano (Hyde y Fremlin, *Measures in SQL*) | Propuesta #354 |
| 3 | Interfaz de consulta "tabla ancha" | Consultas declarativas (dimensiones, medidas, filtros) con resolución de *joins*; compila a la capa 2. Base semántica: #246 | Propuestas #246, #529 (MCP) y #568 (HTTP + Arrow) |
| 4 | Ontología | Conceptos de negocio por encima del modelo lógico | Especificación en `ontology/` (borrador) |

La capa 1 es **declaración**, no consulta: registrar un dialecto no significa que Ossie permita consultar en él.

## 3. Estructura completa del documento (capa 1)

### 3.1 Formatos y dialectos
El mismo modelo se escribe en YAML o en JSON. **El dialecto se declara en cada expresión**, dentro de `expression.dialects`, una lista de pares `dialect` + `expression`. Una misma métrica o campo puede llevar varias versiones:

```yaml
- name: email_normalized
  expression:
    dialects:
      - dialect: ANSI_SQL
        expression: LOWER(email)
      - dialect: SNOWFLAKE
        expression: LOWER(email)::VARCHAR
```

```json
{"name": "email_normalized",
 "expression": {"dialects": [
   {"dialect": "ANSI_SQL", "expression": "LOWER(email)"},
   {"dialect": "SNOWFLAKE", "expression": "LOWER(email)::VARCHAR"}]}}
```

No existe dialecto por documento ni por dataset: se eliminó en 0.2, y la discusión #16 y el issue #52 lo debaten.

### 3.2 Objetos y propiedades (`ossie-schema.json`, JSON Schema 2020-12)
En todos los objetos, `additionalProperties: false`: una clave desconocida invalida el documento. La excepción es `ai_context` en forma de objeto.

**Raíz (`SemanticModel`)**

| Propiedad | Tipo | Oblig. | Restricción y significado |
|---|---|---|---|
| `version` | string | Sí | `const: "0.2.0.dev0"` |
| `name` | string | Sí | `minLength: 1`; identificador del modelo |
| `description` | string | No | Texto para personas |
| `ai_context` | `AIContext` | No | Contexto para IA |
| `datasets` | `Dataset[]` | Sí | `minItems: 1` |
| `relationships` | `Relationship[]` | No | |
| `metrics` | `Metric[]` | No | Agregaciones a nivel de modelo; pueden cruzar datasets |
| `custom_extensions` | `CustomExtension[]` | No | |

**`Dataset`**: un dataset lógico (hecho o dimensión).

| Propiedad | Tipo | Oblig. | Restricción y significado |
|---|---|---|---|
| `name` | string | Sí | `minLength: 1` |
| `source` | string | Sí | `minLength: 1`. Tabla o vista (`db.schema.tabla`) o consulta |
| `primary_key` | string[] | No | Simple o compuesta |
| `unique_keys` | string[][] | No | Cada una simple o compuesta |
| `description`, `ai_context`, `custom_extensions` | | No | |
| `fields` | `Field[]` | No | |

**`Field`**: atributo por fila para agrupar, filtrar y usar en métricas.

| Propiedad | Tipo | Oblig. | Restricción y significado |
|---|---|---|---|
| `name` | string | Sí | Único dentro del dataset |
| `expression` | `Expression` | Sí | **Escalar**, sin agregados: columna o expresión calculada |
| `dimension` | `{is_time?: bool}` | No | Marca de rol (ver §3.4) |
| `label` | string | No | "Label for categorization" |
| `description`, `ai_context`, `custom_extensions` | | No | |
| `datatype` | `DataType` | No | Tipo lógico (ver §3.3) |

**`Relationship`**: siempre de **muchos** (`from`) a **uno** (`to`).

| Propiedad | Tipo | Oblig. | Restricción y significado |
|---|---|---|---|
| `name` | string | Sí | `minLength: 1` |
| `from`, `to` | string | Sí | Nombres de dataset |
| `from_columns`, `to_columns` | string[] | Sí | `minItems: 1`; misma longitud y correspondencia por posición (claves compuestas) |
| `ai_context`, `custom_extensions` | | No | |

**`Metric`**

| Propiedad | Tipo | Oblig. | Restricción y significado |
|---|---|---|---|
| `name` | string | Sí | `minLength: 1` |
| `expression` | `Expression` | Sí | Expresión **agregada** |
| `description`, `datatype`, `ai_context`, `custom_extensions` | | No | |

**Tipos auxiliares**
- `Expression` = `{dialects: DialectExpression[]}`, con `minItems: 1`.
- `DialectExpression` = `{dialect: Dialect, expression: string}`, ambos obligatorios.
- `CustomExtension` = `{vendor_name: string, data: string}`, donde `data` es **JSON dentro de un string**. El PR #522 lo cambia a objeto (ruptura).
- `AIContext` = `string` **o** `{instructions?: string, synonyms?: string[], examples?: string[], …}`, que admite claves extra.
- `Vendor`: texto libre. Ejemplos conocidos: `COMMON`, `SNOWFLAKE`, `SALESFORCE`, `DBT`, `DATABRICKS`, `GOODDATA`, `HONEYDEW`, `WISDOM`, `POWER_BI`, `SIGMA`, `THOUGHTSPOT`, `HEX`.

### 3.3 Enumeraciones
- **`Dialect`:** `ANSI_SQL`, `SNOWFLAKE`, `MDX`, `TABLEAU`, `DATABRICKS`, `MAQL`, `BIGQUERY`, `SIGMA`, `THOUGHTSPOT`, `DAX`, `HOLISTICS_AQL` y `OSSIE_SQL_2026`. Propuestas: `CLICKHOUSE` (#558) e `ICEBERG` (#491).
  - Solo **`OSSIE_SQL_2026`** tiene semántica definida.
  - Los demás son opacos, y una implementación **MAY** ignorarlos.
  - Si hay varios, la elección debe ser **determinista**.
- **`DataType`:** `String`, `Integer`, `Decimal`, `Float`, `Boolean`, `Date`, `Time`, `DateTime` (civil, sin zona), `DateTimeTz` (instante) y `Opaque` (conocido pero fuera del vocabulario portable).
  - Sin precisión, escala, ancho ni nulabilidad.
  - Si el tipo es desconocido, se omite.

### 3.4 Tipo frente a rol (`datatype` y `is_time`)
- `is_time` es un **rol**: "trátalo como dimensión de tiempo". Es independiente del tipo.
- **Por defecto** vale `true` si `datatype` es `Date`, `Time`, `DateTime` o `DateTimeTz`, y `false` en otro caso. Un `is_time` explícito siempre manda.
- Un año entero (`Integer`) o un trimestre `"Q1"` (`String`) pueden ser tiempo. Un `created_at` de auditoría puede no serlo (`is_time: false`).
- Para preguntas de **tipo**, se usa `datatype`. Para preguntas de **rol**, el `is_time` resuelto.

### 3.5 Reglas del documento
- **Un documento = un modelo.** Sin paquetes ni referencias entre modelos.
- **Migración desde 0.1:**
  - el envoltorio `semantic_model: [...]` se rechaza: se pasa a la raíz y, si hay varios modelos, se separan en varios archivos;
  - nunca se toma el primero en silencio;
  - se eliminan `dialects` y `vendors` de la raíz.
- **Identificadores** (`expression_language.md`):
  - nomenclatura ANSI, de 128 caracteres como máximo;
  - sin comillas no distinguen mayúsculas; normalizados, pasan a mayúsculas;
  - entre comillas dobles (`"…"`) son exactos;
  - `"ID"` coincide con `id`, pero `"id"` no;
  - referencias de varias partes: `dataset.campo`.
- **Resolución de nombres** (propuesta #486, no es estándar todavía):
  - la expresión de un campo nombra **columnas de su `source`**;
  - `primary_key`, `unique_keys`, `from_columns` y `to_columns` nombran **campos** del dataset;
  - una métrica nombra **`dataset.campo`**;
  - nunca se "atraviesa" un dataset hasta sus columnas.
  - Las referencias campo → campo, métrica → métrica y dataset sobre dataset quedan "para más adelante".

## 4. Lenguaje de expresiones `OSSIE_SQL_2026` (estado: *Proposed Final*)
- **Base:** ANSI SQL:2003 Core.
- **Objetivo:** declarar métricas, campos y filtros (capa lógica).
- **Niveles de cumplimiento:**
  - **REQUIRED** = **MUST**: el mínimo portable;
  - **RECOMMENDED** = SHOULD;
  - las extensiones de dialecto = MAY;
  - **EXPERIMENTAL**: puede cambiar.

**Construcciones admitidas:**
- referencias a columnas y métricas;
- `+ - * / %`, `= <> != < > <= >=`, `AND OR NOT`;
- `BETWEEN`, `IN` / `NOT IN` (solo listas);
- `LIKE` / `ILIKE`, `IS [NOT] NULL`, `CASE`;
- agregados, ventanas, funciones escalares y paréntesis.

**No admitidas:**
- `SELECT`, `FROM`, `JOIN`;
- `GROUP BY` ("lo controla el grano");
- `WHERE` ("usa la propiedad de filtro");
- subconsultas (solo un `EXISTS_IN()` no definido todavía);
- CTE, `UNION`, `INTERSECT` y `EXCEPT`, DDL y DML.

**Precedencia, de mayor a menor:** paréntesis; unarios; `* / %`; `+ -`; comparaciones, `LIKE`, `IN`, `BETWEEN` e `IS NULL`; `AND`; `OR`.

| Categoría | Funciones | Nivel |
|---|---|---|
| Agregados básicos | `SUM`, `COUNT`, `COUNT(*)`, `COUNT(DISTINCT)`, `AVG`, `MIN`, `MAX` | REQUIRED |
| Estadísticos | `STDDEV`, `STDDEV_POP`, `STDDEV_SAMP`, `VARIANCE`, `VAR_POP`, `VAR_SAMP` | REQUIRED |
| Percentiles | `MEDIAN`, `PERCENTILE_CONT(p) WITHIN GROUP (ORDER BY x)`, `PERCENTILE_DISC(…)` | REQUIRED |
| Aproximados | `APPROX_COUNT_DISTINCT`, `APPROX_PERCENTILE` | RECOMMENDED |
| Condicionales en agregados | `SUM(DISTINCT x)`, `COUNT(DISTINCT x)`, `SUM(CASE WHEN … END)`; `FILTER (WHERE …)` en propuesta #382 | REQUIRED |
| Fecha actual | `CURRENT_DATE`, `CURRENT_TIMESTAMP`, `CURRENT_TIME` | REQUIRED |
| Extracción | `YEAR`, `QUARTER`, `MONTH`, `DAY`, `DAYOFYEAR`, `HOUR`, `MINUTE`, `SECOND`, `EXTRACT(… FROM …)`, `DATE_PART('…', …)` | REQUIRED |
| Truncado y aritmética de fechas | `DATE_TRUNC('year'…'second', x)` (la semana empieza en lunes), `DATEADD(part, n, x)`, `DATEDIFF(part, a, b)` | REQUIRED |
| Construcción de fechas | `DATE '…'`, `TIMESTAMP_NTZ '…'`, `TIME '…'`, `CAST('…' AS DATE)`, `TO_DATE(s)`, `TO_TIMESTAMP(s)` (ISO 8601) | REQUIRED |
| Fechas con formato | `TO_DATE(s, fmt)`, `TO_TIMESTAMP(s, fmt)`, `TO_CHAR(x, fmt)` (tokens `YYYY`, `MM`, `DD`, `HH24`…) | EXPERIMENTAL |
| Texto | `CONCAT`, `\|\|`, `LENGTH`, `LOWER`, `UPPER`, `TRIM`, `LTRIM`, `RTRIM`, `LEFT`, `RIGHT`, `SUBSTRING`, `REPLACE`, `SPLIT_PART`, `POSITION`, `CHARINDEX`, `CONTAINS`, `STARTSWITH`, `ENDSWITH`, `LIKE`, `ILIKE`, `REGEXP_LIKE` | REQUIRED |
| Expresiones regulares | `REGEXP_EXTRACT`, `REGEXP_REPLACE`, `REGEXP_COUNT` | RECOMMENDED |
| Matemáticas | `ABS`, `ROUND`, `FLOOR`, `CEIL`, `CEILING`, `TRUNC`, `TRUNCATE`, `MOD`, `SIGN`, `POWER`, `SQRT`, `EXP`, `LN`, `LOG(base, x)`, `LOG10`, `GREATEST`, `LEAST` | REQUIRED |
| Trigonometría | `SIN`, `COS`, `TAN`, `ASIN`, `ACOS`, `ATAN`, `ATAN2`, `RADIANS`, `DEGREES`, `PI` | RECOMMENDED |
| Condicionales | `CASE` (buscado y simple), `IF`, `IFF`, `NULLIF`, `COALESCE`, `IFNULL`, `NVL`, `NVL2`, `ZEROIFNULL`, `NULLIFZERO` | REQUIRED |
| Ventanas | `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `NTILE`, `PERCENT_RANK`, `CUME_DIST`, `LAG`, `LEAD`, `FIRST_VALUE`, `LAST_VALUE`, `NTH_VALUE` y todo agregado con `OVER`. Marcos: `ROWS`/`RANGE` desde `UNBOUNDED PRECEDING` hasta `CURRENT ROW`, o `n PRECEDING` hasta `n FOLLOWING` | REQUIRED |
| Conversión | `CAST(x AS VARCHAR/STRING/INTEGER/INT/BIGINT/DECIMAL/NUMERIC/FLOAT/DOUBLE/BOOLEAN/DATE/TIMESTAMP/TIME)` | REQUIRED |
| Conversión tolerante | `TRY_CAST` | RECOMMENDED |
| Comparación segura con nulos | `IS [NOT] DISTINCT FROM` | — |

**Descomponibilidad**, para agregar en varias etapas o pre-agregar:
- distributivas: `SUM`, `COUNT`, `MIN` y `MAX`;
- algebraicas: `AVG`, `STDDEV` y `VARIANCE`;
- holísticas: `MEDIAN`, los percentiles y `COUNT DISTINCT`;
- con *sketches*: `APPROX_*`.

Incluye también **tablas de equivalencias** con Tableau, Looker Studio, DAX y Sigma. Señala diferencias reales: DAX `DISTINCTCOUNT` cuenta el BLANK y el SQL no; `COUNT` de un conjunto vacío da 0 en SQL y BLANK en DAX.

## 5. Semántica de consulta propuesta (#246 *Foundational Semantics*, no es estándar)
Es la parte más relevante para nuestro motor de corrección (#45). Busca que **dos motores conformes den el mismo resultado**.
- **Cinco garantías:**
  1. ninguna fila de hechos se pierde en silencio: las claves rotas van a un grupo NULL, con LEFT de hecho a dimensión;
  2. ninguna fila se cuenta dos veces por *fanout*;
  3. en consultas con varios hechos, ningún hecho pierde grupos: FULL OUTER sobre dimensiones compartidas, con igualdad segura ante NULL;
  4. no hay reagregaciones inseguras: distributivas, algebraicas y holísticas;
  5. nunca hay una respuesta incorrecta silenciosa: si no hay plan seguro, error tipado.
- **Cardinalidad inferida de las claves:** `to` coincide con PK/UK → N:1; ambos lados → 1:1; ninguno → N:N, que es lo conservador.
- **Caminos:** si hay varios, `E_AMBIGUOUS_PATH`; si no hay ninguno, `E_NO_PATH`; dentro de una consulta, acíclico.
- **Muchos a muchos:** reescritura por tabla puente o por "costura" de dimensiones compartidas; si no, error.
- **Nulos y vacíos según ANSI:** `COUNT` de nada = 0; los demás, NULL. `AVG` ignora nulos.
- **Códigos de error normativos:**
  - de caminos: `E_AMBIGUOUS_PATH`, `E_NO_PATH`;
  - de reagregación y grano: `E_UNSAFE_REAGGREGATION`, `E_AMBIGUOUS_MEASURE_GRAIN`, `E_UNAGGREGATED_FINER_GRAIN_REFERENCE`;
  - de muchos a muchos: `E3012_MN_NO_SAFE_REWRITE`, `E3013_NO_STITCHING_DIMENSION`;
  - de agregados mal situados: `E_AGGREGATE_IN_WHERE`, `E_AGGREGATE_IN_FIELD`, `E_NON_AGGREGATE_IN_HAVING`;
  - de ventanas: `E_WINDOW_IN_WHERE`, `E_WINDOWED_METRIC_COMPOSITION`, `E_WINDOW_OVER_FANOUT_REWRITE`;
  - de nombres: `E_NAME_COLLISION`, `E_NAME_NOT_FOUND`;
  - de funciones aplazadas: `E_DEFERRED_*`.
- **Fuera de alcance por ahora:**
  - INNER por integridad referencial declarada;
  - elegir un tipo de *join* por métrica;
  - `natural_grain`;
  - relaciones *as-of* o por rango;
  - filtros declarados en el modelo;
  - agregación anidada;
  - ventanas con `GROUPS`.

**Consultas en la práctica** (`bi-sql-examples/`): SQL real que ThoughtSpot y Tableau generan con `MEASURE()`. Hay comparaciones de periodos, top-N sin desempate y medidas de ventana declaradas como dimensión, que Databricks y Snowflake evalúan distinto bajo un filtro. Son las formas que una interfaz SQL sobre Ossie tendrá que resolver.

## 6. Validación y conformidad
- **`validation/validate.py`:**
  - JSON Schema 2020-12;
  - claves YAML duplicadas rechazadas;
  - nombres únicos (datasets, campos por dataset, métricas y relaciones);
  - referencias de relaciones válidas;
  - misma aridad en `from_columns` y `to_columns`;
  - sintaxis SQL con **sqlglot**: `ANSI_SQL` y `OSSIE_SQL_2026` con el analizador por defecto; Snowflake, Databricks y BigQuery con el suyo; MDX, Tableau, MAQL, ThoughtSpot y DAX no se comprueban.
- **`compliance/`:** vacío ("This is for compliance tests"). La hoja de ruta pide una **batería de conformidad entre implementaciones**, y #529 propone un kit para MCP.

## 7. Ontología (capa 4, `ontology/ontology.md`)
- **Conceptos:**
  - `EntityType`: objetos del mundo real referenciados por otra información;
  - `ValueType`: datos con semántica añadida;
  - incorporados: `Any`, `Boolean`, `Date`, `DateTime`, `Decimal`, `Float`, `Integer`, `String`;
  - `extends` (subtipos), `identify_by` (identificador preferente), `derived_by` (derivación con expresiones), `requires` (restricciones) e `iri` (IRI o QName con `prefixes`).
- **Relaciones dentro del concepto que juega el primer rol** (por ejemplo `Person.earns`):
  - `roles`;
  - `multiplicity` (`ManyToOne`, `OneToOne`);
  - `verbalizes` (obligatorio: frases como "{Person} earns {Salary}");
  - semántica de **conjunto**, sin nulos.
- **Documentos de mapeo** (`mapping.json`): unen un modelo semántico con una ontología mediante `concept_mappings`, con *object mappings* (expresiones SQL o *referent mappings*) y *link mappings* en árbol.
  - El mapeo embebido `ontology_mappings` está **obsoleto**.
  - El soporte de lectura de los mapeos independientes **aún no existe**.

## 8. Ecosistema
- **Conversores** (modelo radial con Ossie en el centro):
  - Snowflake (Cortex Analyst), Salesforce/Tableau, dbt (MetricFlow), Databricks (Metric Views), Omni, WisdomAI, NVIDIA Auto Ontology, Sigma, ThoughtSpot (TML), Cube, Hex, Holistics, GoodData, Microsoft (Power BI/TMSL), Polaris, OrionBelt y Honeydew;
  - en PR: SAP CSN, Lightdash, Hologres, Solid, LinkML, RelationalAI y ClickHouse.
  - Regla de los conversores: elegir el dialecto del destino y, **si no está, `ANSI_SQL`**.
  - El PR #523 propone moverlos a un repositorio aparte.
- **CLI en Go:** `ossie convert`, `ossie validate` y *plugins* (`install`, `list`, `remove`).
- **Paquete Python:** modelos Pydantic v2, base de los conversores.
- **Otras implementaciones:** ver [`ossie.md`](ossie.md) (`duckdb-ossie`, dbt Core ≥ 1.12, Dosi, `spiculedata/ossie`).

## 9. Lo que falta o está en desarrollo (para el futuro)

| Tema | Estado en Ossie | Referencia |
|---|---|---|
| Métricas por dataset (`datasets[].metrics`), con nombres en su ámbito | PR abierto | #343, discusión #29 |
| `FILTER (WHERE …)` en agregados | PR con votación | #382 |
| Filtros semánticos reutilizables | Hoja de ruta | discusión #5 |
| Entidad y grano como conceptos; `natural_grain` | Hoja de ruta y propuesta sin publicar | discusiones #12 y #27; #246 §6.2 |
| Cardinalidad explícita, INNER por integridad, relaciones complejas o no equi | Hoja de ruta | discusiones #50, #11, #4 y #24 |
| Métricas derivadas, acumuladas y árboles de métricas | Hoja de ruta | discusiones #39 y #40 |
| Semántica de consulta (capa 3) y códigos de error | PR (borrador grande) | #246 |
| Consulta SQL con `MEASURE()` (capa 2) | PR | #354 |
| Ejecución por MCP y por HTTP + Arrow | PR (propuestas opcionales) | #529, #568 |
| Álgebra de operaciones BI seguras | PR | #262 |
| Resolución de nombres (campos, columnas, métricas) | PR | #486 |
| `custom_extensions[].data` como objeto en vez de string | PR (**ruptura**) | #522 |
| Orígenes de tipo archivo (Parquet…) estructurados | PR (borrador) | #338, issue #104 |
| Datasets lógicos y fuentes estructuradas; reutilizar datasets entre modelos | Esfuerzo futuro | discusiones #49, #61, #23, #109 y #103 |
| Dialectos `CLICKHOUSE` e `ICEBERG`; estrategia de dialectos; plantillas (Jinja) | PR y hoja de ruta | #558, #491; discusiones #16, #28, #62 y #6; issue #52 |
| Jerarquías, grupos de dimensiones, `is_time` → `dimension_type`, calendarios, *date spine* | Esfuerzo futuro | discusiones #21, #20, #17, #44 y #47; issue #84 |
| Unidades, monedas, PII, confidencialidad y metadatos físicos | Mejora | discusiones #42, #43, #55 y #110; issues #58 y #59 |
| Metadatos extendidos (`display_format`, `default_aggregation`, `desired_direction`, `default_sort`, `semantic_type`…) | PR | #287, issue #100 |
| IA: consultas verificadas, controlar qué ve la IA, guías de `ai_context` | Esfuerzo futuro | discusiones #82, #14, #32 y #9 |
| IDs estables, certificación, gobierno, LinkML y versionado del esquema | Esfuerzo futuro | discusiones #31, #53, #13, #67 y #35; issues #102 y #87 |
| Catálogo y registro semántico (Polaris) | Grupo de trabajo activo | issue #107 |
| Geoespacial, audiencias y segmentos | Mejora | discusiones #69, #114 y #51 |
| Batería de conformidad | Vacía | `compliance/` |

**Incoherencias que detectamos en el borrador actual** (útiles para no depender de ellas):
1. `expression_language.md` dice que `OSSIE_SQL_2026` es el dialecto **por defecto** "si no se elige uno", pero el esquema **exige** `dialect` en cada expresión, y todos los ejemplos de `spec.md` usan `ANSI_SQL`, con un comentario que lo llama "Default".
2. Remite a una "propiedad de filtro" y a una "especificación de grano" que **no existen** en el esquema.
3. `EXISTS_IN()` se menciona pero no se define. Los *namespaces* remiten a un documento externo (Google Docs).
4. `CAST(… AS TIMESTAMP)` no aclara si es con zona o sin ella; en cambio, `TIMESTAMP_NTZ` sí se define para los literales.

## 10. Implicaciones para bi-platform

| Punto | Situación | Acción |
|---|---|---|
| Lectura, escritura y validación de la capa 1 (YAML y JSON, esquema, unicidad, referencias, aridad, sintaxis) | Hecho (#54) | — |
| `ANSI_SQL` como lengua común de facto (ejemplos, conversores) | Solo se calcula `OSSIE_SQL_2026` | #74 |
| **Funciones REQUIRED** (texto, matemáticas, extracción y aritmética de fechas, estadísticos, percentiles, ventanas de ranking y desplazamiento, `SUM(DISTINCT)`, `%`) | El validador las acepta, pero la conversión a CSMIR las rechaza (`not_representable`) | Issue nuevo: ampliar CSMIR y el planificador para cumplir el **MUST** |
| `is_time`, descripciones y `ai_context` | Validados; no llegan a la API ni a la UI | #75 |
| Referencias métrica → métrica (`ingresos / pedidos`) | Las admitimos; #486 las deja "para más adelante" y #246 las usa | Es **extensión nuestra**: al exportar a herramientas sin soporte, expandirlas (parte 4 de #48) |
| Semántica de consulta y errores de #246 | Nuestro motor de corrección (#45) cubre lo mismo | **Adoptar sus códigos de error** junto a los nuestros, para ser compatibles |
| `custom_extensions` como objeto (#522, ruptura) | Leemos `data` como string | Aceptar string y objeto cuando se integre |
| Dialecto `CLICKHOUSE` (#558) | ClickHouse es nuestra segunda fuente en vivo (ADR 0002) | Usarlo cuando se publique |
| Orígenes de tipo archivo (#338) | Encaja con "consultar sin copiar" (ADR 0009) | Seguir la propuesta |
| `MEASURE()` (#354) y la capa 3 (#246, #529, #568) | Nuestra SQIR (#73) es una consulta estructurada como la capa 3 | El SQL sobre métricas se compila a SQIR; vigilar los contratos MCP y Arrow |
| Batería de conformidad (vacía en Ossie) | Tenemos el corpus de 21 casos con resultado exacto | Oportunidad: aportarlo (decisión del propietario, ver `ossie.md`) |
