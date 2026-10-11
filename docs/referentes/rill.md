# Rill: qué rescatamos

**Fuente:** código y referencia oficial de Rill `v0.90.5` (tag del 2026-10-09, commit `b9ea8c6f`), `[CÓDIGO]` y `[DOCS]`. Enlaces base:
- referencia de archivos de proyecto: `https://github.com/rilldata/rill/tree/v0.90.5/docs/docs/reference/project-files`
- runtime: `https://github.com/rilldata/rill/tree/v0.90.5/runtime`

**Estado de cada punto:** *propuesto*. Pasa a *aceptado* cuando el propietario lo aprueba, y entonces se planifica en su gate. Ya decidido: proyecto como archivos (ADR 0003), conexiones en vivo (ADR 0002).

## Lo que tomamos

| # | Qué hace Rill | Qué tomamos y cómo lo mejoramos | Gate / módulo |
|---|---|---|---|
| R1 | **Recursos reconciliados con recarga en caliente.** Cada archivo es un recurso tipado. Un *reconciler* por tipo compara el estado deseado (el código) con el real y solo rehace lo que depende del archivo cambiado (`runtime/reconcilers/`, `controller.go`). | Mismo modelo: grafo de recursos, estado por recurso (pendiente, ejecutando, listo, error) y error anclado al archivo y la línea. **Mejora:** cada revisión publicada es inmutable y el puntero se cambia de forma atómica (M10 §0.7), así un borrador inválido nunca rompe lo publicado. | G2 (M10) |
| R2 | **Metrics view:** dimensiones (`column`/`expression`, `unnest`, `uri`, tipo geo, tiempo o categórica); medidas simples, derivadas, de ventana (`window`, `per`, `requires`) y de comparación temporal; `valid_percent_of_total`; formatos con preset o d3 (`metrics-views.md`). | Ese repertorio de medidas y dimensiones, expresado como **Apache Ossie + extensiones con espacio de nombres**, no como YAML propietario. **Mejora:** valores exactos de punta a punta (contrato de texto tipado). | G2 (M04) |
| R3 | **Semántica de tiempo:** `timeseries`, `smallest_time_grain`, `first_day_of_week`, `first_month_of_year`, `watermark`, `max_query_time_range` (ISO 8601) y zonas horarias bloqueables. | Todas estas propiedades. **Mejora:** rangos con ISO 8601 (duraciones e intervalos) más una gramática corta y documentada para "últimos 7 días hasta hoy", en lugar de una sintaxis propietaria. | G2 (M04) |
| R4 | **Seguridad en la métrica:** `access`, `row_filter`, `include`/`exclude` de campos y reglas condicionales con atributos del usuario. **"Ver como"** usuarios de prueba (`mock_users` en `rill.yaml`). | Las mismas capacidades, más usuarios de prueba para validar políticas antes de publicar. **Mejora de seguridad:** Rill inserta los atributos del usuario en el texto SQL del filtro (*templated*). Nosotros los **enlazamos como parámetros tipados**, lo que hace imposible la inyección, como ya hace el compilador de G1. | G2 (M11) |
| R5 | **Rollups:** tablas pre-agregadas a las que se enrutan las consultas automáticamente cuando dimensiones, medidas, grano y zona encajan. | Enrutado automático y transparente. **Mejora (más adelante):** proponer rollups a partir de las consultas reales observadas (M15), como las agregaciones automáticas de Power BI. | G4 (M05) |
| R6 | **Explore autogenerado:** al definir una métrica aparece un dashboard de exploración con leaderboards por dimensión, serie temporal con comparación, pivot y filtros incluir/excluir. El estado vive en la URL (`explore-dashboards.md`). | **Tiempo hasta el primer dashboard ≈ 0:** definir métricas y explorar sin diseñar nada. Estado compartible por URL y marcadores. | G2/G4 (M06) |
| R7 | **Canvas:** dashboards por filas de componentes (KPI, gráficos, tablas, markdown), filtros, variables y tema (`canvas-dashboards.md`, `component.md`, `themes.md`). | Composición declarativa en `dashboards/*.yaml`, editable también desde la UI. | G4 (M08, M17) |
| R8 | **Modelos:** `refresh` (cron), incrementales con `state` o `partitions`, `partitions_concurrency`, `change_mode` (reset, manual, patch), `retry`, `timeout`, `pre_exec`/`post_exec` (`models.md`). | Incrementales y particiones sobre DuckLake, con reintentos y plazos. **Mejora:** cada ejecución publica una revisión de DuckLake de forma atómica; si falla, nada cambia. | G3 (M02/M03) |
| R9 | **Pruebas de datos en el modelo:** `assert` por fila o `sql` que devuelve las filas que fallan (`models.md`, `tests`). | Igual. **Mejora:** las pruebas se ejecutan **dentro de la transacción de publicación**; si alguna falla, se deshace y no se publica nada. | G3 (M02) |
| R10 | **Entornos:** claves `dev:`/`prod:` en cualquier archivo, variables `env`, `.env` sin versionar, muestras de datos en desarrollo. | Igual, con secretos solo como referencias (nunca en Git). | G2/G3 (M10) |
| R11 | **Perfilado de columnas** en el editor: nulos, cardinalidad, mínimo y máximo, distribución (`runtime/queries`). | Panel de perfil al previsualizar fuentes y modelos. | G3 (M01/M06) |
| R12 | **APIs sobre métricas:** `metrics_sql` (SQL contra métricas, no contra tablas), con OpenAPI y seguridad por API (`apis.md`). | SQL semántico seguro hacia el exterior, con el mismo compilador y las mismas políticas. | G4–G6 (M12) |
| R13 | **Alertas e informes:** condiciones sobre una consulta de métricas, intervalos, `renotify`, `on_recover`/`on_fail`/`on_error`; exportaciones programadas (`alerts.md`, `reports.md`). | Igual, con el registro de trabajos en PostgreSQL (M14). | G5 (M14) |
| R14 | **IA:** `ai_instructions` por proyecto y por métrica, `ai_prompts` sugeridos y exposición de métricas a agentes (MCP, agent skills). | Preguntas en lenguaje natural **sobre la capa semántica**, nunca SQL libre sobre tablas; métricas expuestas por MCP con las mismas políticas. | G5 (M16) |

## Lo que no copiamos
- **Almacenamiento interno en DuckDB:** usamos DuckLake (ADR 0002). Rill tuvo que aislar instancias por fuente para evitar bloqueos (notas 0.37).
- **Plantillas de texto dentro del SQL** para atributos de usuario: sustituidas por parámetros tipados (R4).
- **Formato YAML propio de métricas** como estándar: el nuestro es Ossie + extensiones (R2).
- **Nombres y sintaxis de marca** (`rill.yaml`, `rilltime`): `project.yaml`, ISO 8601.
