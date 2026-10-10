# M03 — Storage & Materialization

**Plataforma BI nueva · edición normativa RC1 · 2026-10-10**

**Reemplaza las decisiones de producto del informe histórico:** `../investigaciones_historicas/HISTORICO_NO_NORMATIVO_SQLVIZ_Modulo_03_Storage_Materialization_Rill_Lightdash_SQLMesh_DuckLake_2026-10-10.md`. El análisis de proyectos externos se mantiene como evidencia histórica.

## 0. Estatus, límites y decisiones transversales obligatorias

**Estatus:** edición normativa de arquitectura `RC1` para una plataforma BI NUEVA, aún sin nombre comercial; **no es código implementado ni verificado**. La investigación original de Rill/Lightdash/Cube/Grafana/SQLMesh está preservada aparte en `investigaciones_historicas/` y sirve como evidencia comparativa, **jamás como instrucción de implementación**. Esta edición sustituye la recomendación de producto de la antigua investigación, pero no borra ni falsifica sus hallazgos externos.

**Precedencia:** `DECISIONES_TRANSVERSALES.md` → contratos y ADR aprobados → estos M01–M09 → M10–M17, sujetos a la misma precedencia → investigación histórica. Las contradicciones se registran y resuelven con ADR; un agente no escogerá a escondidas la versión que le convenga.

**Topología única:** Svelte 5/SvelteKit 3/TypeScript (UI) → HTTP API Rust/Axum → autorización y contratos → motor semántico/serving → `duckdb-rs` embebido y Arrow → DuckLake con **catálogo PostgreSQL** + ficheros **Parquet** bajo `DATA_PATH`. `Polars Rust` es auxiliar cuando hay valor medido. `ADBC` sólo si existe driver interoperable validado. Quack es **adaptador remoto optativo**, nunca condición de arranque.

**Fuentes de verdad:** `PostgreSQL BI` gestiona usuarios, proyectos, definiciones/revisiones, ejecuciones y políticas; `PostgreSQL DuckLake catalog` gestiona snapshots/archivos/transacciones del lago; Parquet conserva bytes de los datos; el estado UI es efímero hasta que un comando explícito lo guarda. Pueden coexistir en un clúster PostgreSQL, pero deben ser esquemas/DB, migraciones, credenciales y backups lógicamente independientes. **No usar `.sqlviz` ni SQLite como fuente canónica de producto** y no agregar el legado como dependencia/migración del MVP.

**Confianza y calidad:** toda ruta (SQL, Explore, Pivot, Canvas, API, exportación, IA) aplica identidad y políticas M11 del lado servidor. Consultas SQL arbitrarias no comparten automáticamente los permisos seguros del motor semántico: requieren perfil, aislamiento, validación, límites y tests negativos propios. Preservar `DECIMAL`, `INT64`, `TIMESTAMPTZ` y `NULL`; distinguir datos y metadatos; no inventar optimizaciones sin benchmarks.

**Normas de ingeniería:** componentes modulares sin microservicios innecesarios, interfaces pequeñas, TypeScript estricto, Rust `fmt/clippy`, SQL parametrizado donde corresponda, pruebas unitarias/contrato/integración/E2E; errores tipados; migraciones reversibles o forward-only justificadas; observabilidad sin PII; SemVer sólo al liberar. Cada microentrega sigue Plan → Implementación → Test → Informe conciso → Git.

## 1. Responsabilidad y auditoría de evidencias

**Dueño:** Lake Storage / Publish.

**Hallazgo de investigación conservado:** Rill abstrae administración de resultados; SQLMesh aporta planificación virtual/física; DuckLake ofrece catálogo transaccional y snapshots. Ninguna de esas piezas equivale a guardar parquet arbitrario como una tabla DuckLake.

**Fuera de alcance:** interfaces de usuario o APIs ajenas a esta capa, aunque la capa proporcione contratos para ellas; no sustituye a M10–M17.

## 2. Decisiones normativas y opciones no seleccionadas

1. `DuckLake + PostgreSQL catalog + DATA_PATH(Parquet)` es la ruta persistente canónica. DuckDB `:memory:` embebido accede a ese catálogo sin archivo `.duckdb` obligatorio.
2. Políticas: `EXTERNAL_LIVE`, `LOGICAL_VIEW`, `MATERIALIZED_DUCKLAKE`; `.duckdb` local sólo caso explícito de test/caché/diagnóstico, no opción canónica de producto.
3. Una publicación se hace con candidato validado, commit DuckLake, registro transaccional de `PublishedDatasetRef` y reconciliación idempotente de dual-state entre catálogo DuckLake y PostgreSQL BI.
4. No prometer atomicidad cross-database inexistente: diseñar estado `prepared/committed/registered/reconciled` y un protocolo recuperable.
5. GC/retención respaldan lectores activos, time-travel, auditoría, restauración; expirar snapshot y borrar archivos son pasos separados.

**No elegir por defecto:** SQLVIZ como nombre/producto/código, `.sqlviz`, SQLite canónico, Quack como servidor obligatorio, microservicio por submódulo, reimplementación de lógica semántica o APIs de otras capas.

## 3. Contratos tipados independientes del transporte

`DatasetDefinition { id, ownership, kind, schema, storage_policy }`; `CandidateManifest { run_id, input_refs, schema_hash, row_count, partitions, checksums, quality_report }`; `PublishedDatasetRef { id, revision, ducklake_catalog_ref, snapshot_id, schema_hash, published_at, actor_id }`; `PublishTransaction { id, candidate_ref, phase, commit_token, error }`.

El catálogo DuckLake sólo es dueño de los snapshots/datos; PostgreSQL BI registra recursos y revisiones del producto. Ambos respaldos requieren recuperación consistente, no confundir una tabla con otra.

Los campos anteriores son **esquemas conceptuales**; los DTO definitivos deben establecerse en OpenAPI/JSON Schema con semántica `required/optional`, estados, `schema_version`, referencias opacas, errores tipados y golden fixtures. No son declaraciones Rust compiladas.

## 4. Flujo end-to-end y responsabilidades

1. Crear candidatos aislados, nunca editar publicada in-place de forma destructiva.
2. Validar manifiesto y reglas M02, calcular su identidad inmutable.
3. Confirmar cambio transaccional DuckLake y capturar snapshot/commit.
4. Registrar `PublishedDatasetRef` en PostgreSQL BI con clave de idempotencia.
5. Ante fallo entre pasos, reconciliar desde run duradero; no inventar éxito ni borrar archivos válidos.
6. Notificar invalidación selectiva a M05/semántica.
7. Mantener retención/backup/GC por políticas autorizadas con ensayos de recuperación.

## 5. Seguridad, fiabilidad y fallos

- PostgreSQL BI no disponible después de commit DuckLake: reconciliar sin doble publicación.
- Snapshot publicado con archivos faltantes: fallar lectura, alertar, no declarar recuperado.
- Disco local sólo válido para single-node; multi-nodo exige DATA_PATH compartido y prueba de consistencia.
- No hacer delete de Parquet según cron sin observar los snapshots y leases activos.

**Invariantes comunes:** actor autorizado, `tenant_id` no confiable desde browser, secretos sólo en secret store, trazabilidad sin PII, presupuestos finitos, reintentos con idempotencia, no degradar silenciosamente una política de seguridad.

## 6. Librerías, dependencias y límites tecnológicos

- Lenguaje de lógica central: **Rust**, con módulos de dominio + traits pequeños (donde aporten sustitución real), `Result` y errores categorizados.
- Runtime analítico: **duckdb-rs** con DuckDB embebido M13. Polars Rust y ADBC como extensiones evaluadas, no rutas obligatorias.
- Persistencia: PostgreSQL BI para revisiones/configuración/runs; DuckLake catalog PostgreSQL para snapshots; Parquet en DATA_PATH para datos materiales.
- Transporte web: M12 — API HTTP/OpenAPI, JSON tipado, Arrow IPC según cardinalidad y SSE si hay progreso.
- SvelteKit/Svelte 5 + TypeScript: UI M17. Librerías visuales adicionales sólo si este módulo las requiere realmente y pasan spike/licencia/a11y.

## 7. Contratos de integración y ownership

**Módulos relacionados:** M01/M02/M04/M05/M10/M11/M13/M14/M15.

La integración atraviesa el API M12 (nunca llamando DuckDB directamente desde navegador); M11 autoriza y M15 observa. Los recursos tienen un solo dueño de escritura y pueden leerse por referencias versionadas/servicios tipados. Una implementación no puede copiar la lógica de otra capa para mejorar artificialmente un benchmark.

## 8. Pruebas de aceptación propuestas (aún no ejecutadas)

1. Dos publicaciones concurrentes no corrompen puntero publicado.
2. Fallo tras commit y antes del registro se repara en retry.
3. Rollback lógico conserva snapshot anterior cuando sigue retenido.
4. La pérdida de catálogo o ficheros se detecta en restore.
5. Tablas externas no se copian sin petición.
6. Catálogo usa PostgreSQL, no SQLite.
7. Snapshot viejo no se borra mientras lector autorizado depende de él.
8. Prueba multi-proceso no se declara válida con DATA_PATH privado.

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
