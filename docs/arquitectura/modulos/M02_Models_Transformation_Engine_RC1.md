# M02 — Models & Transformation Engine

**Plataforma BI nueva · edición normativa RC1 · 2026-10-10**

**Reemplaza las decisiones de producto del informe histórico:** `../investigaciones_historicas/HISTORICO_NO_NORMATIVO_SQLVIZ_Modulo_02_Models_Transformation_Engine_Rill_Lightdash_SQLMesh_2026-10-10.md`. El análisis de proyectos externos se mantiene como evidencia histórica.

## 0. Estatus, límites y decisiones transversales obligatorias

**Estatus:** edición normativa de arquitectura `RC1` para una plataforma BI NUEVA, aún sin nombre comercial; **no es código implementado ni verificado**. La investigación original de Rill/Lightdash/Cube/Grafana/SQLMesh está preservada aparte en `investigaciones_historicas/` y sirve como evidencia comparativa, **jamás como instrucción de implementación**. Esta edición sustituye la recomendación de producto de la antigua investigación, pero no borra ni falsifica sus hallazgos externos.

**Precedencia:** `DECISIONES_TRANSVERSALES.md` → contratos y ADR aprobados → estos M01–M09 → M10–M17, sujetos a la misma precedencia → investigación histórica. Las contradicciones se registran y resuelven con ADR; un agente no escogerá a escondidas la versión que le convenga.

**Topología única:** Svelte 5/SvelteKit 3/TypeScript (UI) → HTTP API Rust/Axum → autorización y contratos → motor semántico/serving → `duckdb-rs` embebido y Arrow → DuckLake con **catálogo PostgreSQL** + ficheros **Parquet** bajo `DATA_PATH`. `Polars Rust` es auxiliar cuando hay valor medido. `ADBC` sólo si existe driver interoperable validado. Quack es **adaptador remoto optativo**, nunca condición de arranque.

**Fuentes de verdad:** `PostgreSQL BI` gestiona usuarios, proyectos, definiciones/revisiones, ejecuciones y políticas; `PostgreSQL DuckLake catalog` gestiona snapshots/archivos/transacciones del lago; Parquet conserva bytes de los datos; el estado UI es efímero hasta que un comando explícito lo guarda. Pueden coexistir en un clúster PostgreSQL, pero deben ser esquemas/DB, migraciones, credenciales y backups lógicamente independientes. **No usar `.sqlviz` ni SQLite como fuente canónica de producto** y no agregar el legado como dependencia/migración del MVP.

**Confianza y calidad:** toda ruta (SQL, Explore, Pivot, Canvas, API, exportación, IA) aplica identidad y políticas M11 del lado servidor. Consultas SQL arbitrarias no comparten automáticamente los permisos seguros del motor semántico: requieren perfil, aislamiento, validación, límites y tests negativos propios. Preservar `DECIMAL`, `INT64`, `TIMESTAMPTZ` y `NULL`; distinguir datos y metadatos; no inventar optimizaciones sin benchmarks.

**Normas de ingeniería:** componentes modulares sin microservicios innecesarios, interfaces pequeñas, TypeScript estricto, Rust `fmt/clippy`, SQL parametrizado donde corresponda, pruebas unitarias/contrato/integración/E2E; errores tipados; migraciones reversibles o forward-only justificadas; observabilidad sin PII; SemVer sólo al liberar. Cada microentrega sigue Plan → Implementación → Test → Informe conciso → Git.

## 1. Responsabilidad y auditoría de evidencias

**Dueño:** Model Planner / DAG.

**Hallazgo de investigación conservado:** Rill distingue controlador, `ModelExecutor` y `ModelManager`; SQLMesh aporta ideas de planes, intervalos, auditoría y entornos. Son referencias para interfaces y tests, no dependencia de un executor remoto concreto.

**Fuera de alcance:** interfaces de usuario o APIs ajenas a esta capa, aunque la capa proporcione contratos para ellas; no sustituye a M10–M17.

## 2. Decisiones normativas y opciones no seleccionadas

1. SQL DuckDB es el lenguaje principal de transformación; `Polars Rust LazyFrame` complemento opcional para operadores que no convenga expresar en SQL.
2. No crear un DSL de transformaciones ni exigir que el usuario escriba YAML propietario; la UI visual genera definiciones tipadas que compilan a SQL/plan estable.
3. `ModelDefinition` versionada y `BuildPlan` inmutable permiten vista previa `dry-run` sin mutar storage.
4. DAG tipado con ciclos prohibidos, dependencias vinculadas a revisiones, fingerprints de código/datos y validadores de calidad separados.
5. SQLMesh se considera opcional por integración futura; no mezclar su scheduler, Python runtime o storage con el Rust core sin pruebas de idempotencia.

**No elegir por defecto:** SQLVIZ como nombre/producto/código, `.sqlviz`, SQLite canónico, Quack como servidor obligatorio, microservicio por submódulo, reimplementación de lógica semántica o APIs de otras capas.

## 3. Contratos tipados independientes del transporte

`ModelDefinition { id, revision, inputs: DatasetRef[], language: duckdb_sql|polars_rust, definition_ref, output_schema, kind, quality_rules }`; `ModelPlan { nodes, dependency_revisions, proposed_writes, impacts, checks }`; `BuildRun { run_id, model_revision, input_snapshots, candidate_ref, state, attempts, quality_result }`.

`polars_rust` describe un tipo de tarea de transformación implementada por el producto, **no** ejecución arbitraria de código compilado del usuario en un proceso privilegiado.

Los campos anteriores son **esquemas conceptuales**; los DTO definitivos deben establecerse en OpenAPI/JSON Schema con semántica `required/optional`, estados, `schema_version`, referencias opacas, errores tipados y golden fixtures. No son declaraciones Rust compiladas.

## 4. Flujo end-to-end y responsabilidades

1. Validar sintaxis, schema, dependencias y autorización contra catálogo M10/M11.
2. Construir DAG y ejecutar `plan()` sin mutar datos publicados.
3. Reservar presupuesto y ejecutar tareas con M13, por defecto DuckDB embebido.
4. Recolectar manifiesto/estadísticas y ejecutar quality gates independientes del runtime.
5. Pasar candidato verificado a `DatasetPublisher` M03.
6. Registrar ejecución y sólo marcar éxito tras publicación confirmada.

## 5. Seguridad, fiabilidad y fallos

- DAG circular, referencia inexistente o cambios de schema incompatibles producen error localizado.
- Incremental: distinguir ventana cubierta, watermark, snapshot fuente y commit publicado.
- No hacer `DROP/REPLACE` de dataset publicado antes de validar candidato.
- Deshabilitar código Rust/Python de terceros dentro del proceso API; plugins deben tener aislamiento.

**Invariantes comunes:** actor autorizado, `tenant_id` no confiable desde browser, secretos sólo en secret store, trazabilidad sin PII, presupuestos finitos, reintentos con idempotencia, no degradar silenciosamente una política de seguridad.

## 6. Librerías, dependencias y límites tecnológicos

- Lenguaje de lógica central: **Rust**, con módulos de dominio + traits pequeños (donde aporten sustitución real), `Result` y errores categorizados.
- Runtime analítico: **duckdb-rs** con DuckDB embebido M13. Polars Rust y ADBC como extensiones evaluadas, no rutas obligatorias.
- Persistencia: PostgreSQL BI para revisiones/configuración/runs; DuckLake catalog PostgreSQL para snapshots; Parquet en DATA_PATH para datos materiales.
- Transporte web: M12 — API HTTP/OpenAPI, JSON tipado, Arrow IPC según cardinalidad y SSE si hay progreso.
- SvelteKit/Svelte 5 + TypeScript: UI M17. Librerías visuales adicionales sólo si este módulo las requiere realmente y pasan spike/licencia/a11y.

## 7. Contratos de integración y ownership

**Módulos relacionados:** M01/M03/M04/M10/M11/M13/M14/M15.

La integración atraviesa el API M12 (nunca llamando DuckDB directamente desde navegador); M11 autoriza y M15 observa. Los recursos tienen un solo dueño de escritura y pueden leerse por referencias versionadas/servicios tipados. Una implementación no puede copiar la lógica de otra capa para mejorar artificialmente un benchmark.

## 8. Pruebas de aceptación propuestas (aún no ejecutadas)

1. DAG con ciclo y modelo huérfano son rechazados.
2. `plan()` no modifica datos ni catálogo DuckLake.
3. El mismo modelo y snapshots generan fingerprint determinista.
4. Calidad fallida mantiene publicada revisión anterior.
5. Retry tras interrupción no duplica datos.
6. Cambios en upstream invalidan dependientes necesarios, no todo el workspace.
7. SQL y Polars optativo producen tipos lógicos correctos con Arrow.
8. Sólo revisiones autorizadas pueden publicarse.

**Gate de implementación:** PR pequeño con contract tests y suite negativa cuando afecte seguridad, formateo/lint y evidencia de un slice real. Documentar mediciones sólo si se ejecutaron, nunca prometer latencia por intuición.

## 9. Orden mínimo de implementación

1. Acordar y revisar contratos DTO mínimos; golden fixture positiva y negativa.
2. Probar un caso feliz in-process con dependencia real (o test fake estrictamente diferenciado).
3. Cubrir un fallo real, concurrencia/retry si aplica, y permisos.
4. Conectar la UI sólo cuando la API sea tipada y probada; medir antes de optimizar.
5. Publicar una microtarea por PR con registro de tests y Git.

## 10. Compatibilidad histórica y decisión de cierre

El informe anterior se conserva íntegro como **investigación histórica NO NORMATIVA**, incluyendo las comparaciones de Rill, Lightdash, Cube, SQLMesh o Grafana. Las menciones a formatos SQLVIZ, Quack obligatorio, migración del repo SQLVIZ y SQLite como repositorio del producto NO se trasladan al nuevo proyecto. Si un algoritmo del informe anterior resulta útil se reespecifica y prueba bajo el contrato vigente.

**Estado final del documento:** `normalizado-documentalmente` para la baseline RC1. **No verificado en implementación.** Requiere ADR y CI para pasar de arquitectura a código; no afirmar rendimiento/HA ni seguridad por documento.
