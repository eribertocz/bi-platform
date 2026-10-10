# M05 — Query Serving & Optimization

**Plataforma BI nueva · edición normativa RC1 · 2026-10-10**

**Reemplaza las decisiones de producto del informe histórico:** `../investigaciones_historicas/HISTORICO_NO_NORMATIVO_SQLVIZ_Modulo_05_Query_Serving_Optimization_Rill_Lightdash_Cube_Quack_2026-10-10.md`. El análisis de proyectos externos se mantiene como evidencia histórica.

## 0. Estatus, límites y decisiones transversales obligatorias

**Estatus:** edición normativa de arquitectura `RC1` para una plataforma BI NUEVA, aún sin nombre comercial; **no es código implementado ni verificado**. La investigación original de Rill/Lightdash/Cube/Grafana/SQLMesh está preservada aparte en `investigaciones_historicas/` y sirve como evidencia comparativa, **jamás como instrucción de implementación**. Esta edición sustituye la recomendación de producto de la antigua investigación, pero no borra ni falsifica sus hallazgos externos.

**Precedencia:** `DECISIONES_TRANSVERSALES.md` → contratos y ADR aprobados → estos M01–M09 → M10–M17, sujetos a la misma precedencia → investigación histórica. Las contradicciones se registran y resuelven con ADR; un agente no escogerá a escondidas la versión que le convenga.

**Topología única:** Svelte 5/SvelteKit 3/TypeScript (UI) → HTTP API Rust/Axum → autorización y contratos → motor semántico/serving → `duckdb-rs` embebido y Arrow → DuckLake con **catálogo PostgreSQL** + ficheros **Parquet** bajo `DATA_PATH`. `Polars Rust` es auxiliar cuando hay valor medido. `ADBC` sólo si existe driver interoperable validado. Quack es **adaptador remoto optativo**, nunca condición de arranque.

**Fuentes de verdad:** `PostgreSQL BI` gestiona usuarios, proyectos, definiciones/revisiones, ejecuciones y políticas; `PostgreSQL DuckLake catalog` gestiona snapshots/archivos/transacciones del lago; Parquet conserva bytes de los datos; el estado UI es efímero hasta que un comando explícito lo guarda. Pueden coexistir en un clúster PostgreSQL, pero deben ser esquemas/DB, migraciones, credenciales y backups lógicamente independientes. **No usar `.sqlviz` ni SQLite como fuente canónica de producto** y no agregar el legado como dependencia/migración del MVP.

**Confianza y calidad:** toda ruta (SQL, Explore, Pivot, Canvas, API, exportación, IA) aplica identidad y políticas M11 del lado servidor. Consultas SQL arbitrarias no comparten automáticamente los permisos seguros del motor semántico: requieren perfil, aislamiento, validación, límites y tests negativos propios. Preservar `DECIMAL`, `INT64`, `TIMESTAMPTZ` y `NULL`; distinguir datos y metadatos; no inventar optimizaciones sin benchmarks.

**Normas de ingeniería:** componentes modulares sin microservicios innecesarios, interfaces pequeñas, TypeScript estricto, Rust `fmt/clippy`, SQL parametrizado donde corresponda, pruebas unitarias/contrato/integración/E2E; errores tipados; migraciones reversibles o forward-only justificadas; observabilidad sin PII; SemVer sólo al liberar. Cada microentrega sigue Plan → Implementación → Test → Informe conciso → Git.

## 1. Responsabilidad y auditoría de evidencias

**Dueño:** Query Coordination.

**Hallazgo de investigación conservado:** Rill/Cube muestran organización de consultas, límites y caché; la propuesta anterior centrada en Quack no aplica como ruta obligatoria. DuckDB se ejecuta embebido con `duckdb-rs`; separar motor de serving.

**Fuera de alcance:** interfaces de usuario o APIs ajenas a esta capa, aunque la capa proporcione contratos para ellas; no sustituye a M10–M17.

## 2. Decisiones normativas y opciones no seleccionadas

1. `QueryService` Rust es la única entrada a ejecución BI; backend no ofrece acceso DuckDB sin autorización.
2. `AnalyticalRuntime` M13 abstrae implementación `DuckDbEmbedded` inicial y posible `RemoteQuackRuntime`, seleccionados por capacidades, no por default de red.
3. Budget por actor/tenant/clase, colas y admisión para proteger consultas interactivas de refresh batch.
4. Retornar JSON tipado para pequeño volumen y Arrow IPC para resultados columnares justificados; nunca transferir millones de filas sólo por pintar un KPI.
5. Caché de consultas se particiona por política efectiva, revisión semántica, snapshots, zona horaria, parámetros y versión del plan.

**No elegir por defecto:** SQLVIZ como nombre/producto/código, `.sqlviz`, SQLite canónico, Quack como servidor obligatorio, microservicio por submódulo, reimplementación de lógica semántica o APIs de otras capas.

## 3. Contratos tipados independientes del transporte

`QueryRequest { semantic_query_ref, execution_class, deadline_ms, client_request_id, format }`; `QueryHandle { query_id, state, actor_id, plan_fingerprint, snapshot_refs, policy_fingerprint, ttl }`; `QueryOutcome { schema, data_ref|small_rows, provenance, counters, timings }`; `QueryError { category, retryable, correlation_id }`.

M05 es dueño de admission/cancel/cache/retención de resultados. M13 ejecuta DuckDB/Arrow y no concede autorizaciones; M12 define HTTP/OpenAPI.

Los campos anteriores son **esquemas conceptuales**; los DTO definitivos deben establecerse en OpenAPI/JSON Schema con semántica `required/optional`, estados, `schema_version`, referencias opacas, errores tipados y golden fixtures. No son declaraciones Rust compiladas.

## 4. Flujo end-to-end y responsabilidades

1. Autenticación/autorización M11; compilar consulta M04 o validar SQL experto por otra política.
2. Resolver snapshot estable y fingerprint completo.
3. Consultar caché segregada; no cache hit sin revalidar autorización.
4. Admission: presupuesto/concurrencia/timeout y prioridad; ejecutar en pool bloqueante, no bloquear runtime HTTP async.
5. M13 entrega Arrow batches; limitar payload/cursor/paginación y serializar con fidelidad tipada.
6. Emitir resultado/estado/errores; propagar cancelación real y cerrar recursos nativos.

## 5. Seguridad, fiabilidad y fallos

- Timeout HTTP no garantiza cancelación del motor: mostrar requested/confirmed y liberar conexión correctamente.
- `memory_limit` DuckDB no equivale a limitar RSS total; combinar procesos, recursos y pruebas.
- Una consulta con SQL externo peligrosa nunca obtiene permisos del API service account por accidente.
- Parquet/snapshot cambiantes nunca producen caché reutilizable para revisión distinta.

**Invariantes comunes:** actor autorizado, `tenant_id` no confiable desde browser, secretos sólo en secret store, trazabilidad sin PII, presupuestos finitos, reintentos con idempotencia, no degradar silenciosamente una política de seguridad.

## 6. Librerías, dependencias y límites tecnológicos

- Lenguaje de lógica central: **Rust**, con módulos de dominio + traits pequeños (donde aporten sustitución real), `Result` y errores categorizados.
- Runtime analítico: **duckdb-rs** con DuckDB embebido M13. Polars Rust y ADBC como extensiones evaluadas, no rutas obligatorias.
- Persistencia: PostgreSQL BI para revisiones/configuración/runs; DuckLake catalog PostgreSQL para snapshots; Parquet en DATA_PATH para datos materiales.
- Transporte web: M12 — API HTTP/OpenAPI, JSON tipado, Arrow IPC según cardinalidad y SSE si hay progreso.
- SvelteKit/Svelte 5 + TypeScript: UI M17. Librerías visuales adicionales sólo si este módulo las requiere realmente y pasan spike/licencia/a11y.

## 7. Contratos de integración y ownership

**Módulos relacionados:** M04/M10/M11/M12/M13/M15.

La integración atraviesa el API M12 (nunca llamando DuckDB directamente desde navegador); M11 autoriza y M15 observa. Los recursos tienen un solo dueño de escritura y pueden leerse por referencias versionadas/servicios tipados. Una implementación no puede copiar la lógica de otra capa para mejorar artificialmente un benchmark.

## 8. Pruebas de aceptación propuestas (aún no ejecutadas)

1. Dos tenants nunca reciben misma entrada de caché insegura.
2. Consulta suspendida excediendo presupuesto es rechazada con error tipado.
3. Cancelación de consulta antigua no interrumpe consulta nueva en conexión reutilizada.
4. DECIMAL/INT64/TZ se devuelven sin pérdidas.
5. JSON y Arrow contienen valores lógicamente idénticos.
6. Consultas concurrentes no bloquean health/API.
7. Plan lento genera métricas sin volcar SQL sensible.
8. El adaptador Quack ausente no impide arranque local.

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
