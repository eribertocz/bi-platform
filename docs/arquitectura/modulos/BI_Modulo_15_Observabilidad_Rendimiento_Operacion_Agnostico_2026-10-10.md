# Plataforma BI abierta — Módulo 15: Observabilidad, Rendimiento y Operación
## Auditoría de implementaciones reales, arquitectura Rust, presupuestos de recursos, SLO, resiliencia y recuperación

**Fecha de investigación:** 10 de octubre de 2026  
**Edición:** 1.0 — documento de ingeniería, no implementación ejecutada  
**Producto:** plataforma BI nueva, sin nombre y sin dependencia de SQLVIZ ni formato `.sqlviz`.  
**Alcance exclusivo:** telemetría, control operativo, capacidad, perfiles, seguridad de observabilidad, benchmarking, despliegue, mantenimiento y recuperación. No reimplementa el motor semántico, el scheduler, el constructor de dashboards o el runtime analítico.

**Punto de partida acordado, revisable por benchmarks:** backend Rust, DuckDB embebido con `duckdb-rs`, datos en DuckLake con **catálogo PostgreSQL y Parquet**, Polars Rust complementario, Apache Arrow como frontera columnar, ADBC para conectividad apropiada, Quack remoto opcional. PostgreSQL como almacenamiento de metadatos BI es una opción diferente de su rol como catálogo DuckLake. **Ni Quack, ni un archivo `.duckdb`, ni un catálogo SQLite son requisitos para arrancar.**

**Método y límite de evidencia:** se inspeccionaron archivos específicos de Rill `v0.90.4`, Lightdash `main`, Cube `master`, Grafana `main` y CubeStore `master`, más documentación de DuckDB, DuckLake, OpenTelemetry y Prometheus. Ramas `main`/`master` pueden evolucionar: se deberán fijar SHA exactos en la implementación. No se compilaron los repositorios de terceros ni se midieron cargas reales. Cada afirmación de producto está separada de la propuesta nueva.

---

## 0. Decisión ejecutiva: observabilidad como contrato transversal, no como un microservicio obligatorio

**Elegir:** instrumentación desde el primer corte vertical con trazas/logs estructurados Rust (`tracing` y compatibilidad OpenTelemetry), métricas estables y un contrato de diagnóstico por ejecución; colector OTLP y herramientas de visualización enchufables.

**No elegir:** un stack de diez servidores, un lenguaje de métricas propietario, logs de SQL en claro por defecto, rastreo exhaustivo de cada fila, Redis por principio, ni autoscaling por CPU como sustituto de control de consultas.

La operación necesita **dos circuitos distintos**:

1. **Circuito de producto:** `query_id`, `run_id`, `revision_id`, estado, propietario, progreso y explicación de error, visibles de forma autorizada en la UI y API. Estos registros durables pertenecen a los módulos que ejecutan el trabajo, no al colector de trazas.
2. **Circuito de observabilidad:** métricas agregadas, logs y spans de diagnóstico con retención propia, exportables por OTLP/Prometheus; puede degradarse sin corromper consultas ni materializaciones.

**Regla de verdad:** la caída del colector de telemetría no puede impedir que se publique correctamente un modelo; la caída del repositorio transaccional de runs **sí** puede impedir operaciones que necesitan un registro durable por integridad.

**Regla de seguridad:** ninguna operación de Explore, Pivot, Canvas, API, MCP, exportación o job queda exenta de RLS/CLS ni se convierte en acceso libre a diagnósticos. El registro de auditoría de seguridad no es un log debug que se pueda desactivar sin controles.

**Objetivo de una primera versión productiva:** determinar si una consulta tarda por: autorización, compilación semántica, espera por cupo, preparación DuckDB, lectura Parquet, agregación, serialización Arrow/JSON, transporte o renderizado.

---

## 1. Fronteras de este módulo y dependencias

| Capa | Propiedad de la capa | Contrato entregado a M15 | Lo que M15 no debería hacer |
|---|---|---|---|
| Sources / Ingestion (M01) | Conexiones y descargas | `IngestRunStatus`, conteos, bytes, checkpoint | Convertirse en ETL |
| Models (M02) | DAG, plan, validaciones | `BuildRun`, nodos, errores tipados, revisión | Definir estrategias SQL |
| Storage (M03) | Publicación, snapshots | `PublishedDatasetRef`, commit, tiempos, files | Cambiar datos o snapshots |
| Semantic (M04) | Medidas, contexto | `SemanticPlanDigest`, fase planner, fallback | Recalcular métricas |
| Query Serving (M05) | Admission, ejecución, caché | `QueryExecution`, estados y counters | Duplicar scheduler SQL |
| Explore/Pivot/Canvas (M06–M08) | Experiencia analítica | duración end-to-end y errores por panel | Acoplar observabilidad a ECharts |
| Visualization (M09) | Especificación/render | `render_ms`, `update_ms`, fallbacks | Acceder a claves de servidor |
| Metadata (M10) | Revisions/definiciones | eventos versionados | Almacenar series Prometheus |
| Identity & Policy (M11) | Auditoría y autorización | actor y contexto autorizado | Exponer identidad en labels públicos |
| API / MCP (M12) | Protocolos y SDK | `traceparent`, `request_id`, status | Crear políticas independientes |
| Runtime (M13) | DuckDB/Polars/Arrow/ADBC | perfiles, memoria, threads, errores | Reemplazar el motor |
| Automation (M14) | Jobs, delivery, alertas | `job_id`, scheduler lag, retries | Ser una cola nueva |

**Definición estricta de interfaz:** una métrica Prometheus es un agregado; un registro de ejecución es entidad auditable; un span es una observación correlacionada; un log es un evento puntual. No deben tratarse como equivalentes ni tener necesariamente la misma retención.

---

## 2. Auditoría verificable — Rill: controlador y reconciliación

**Fuentes leídas**:
- [`runtime/controller.go` en `v0.90.4`](https://github.com/rilldata/rill/blob/v0.90.4/runtime/controller.go).
- [`runtime/reconcilers/model.go` en `v0.90.4`](https://github.com/rilldata/rill/blob/v0.90.4/runtime/reconcilers/model.go).

**Evidencia de código:** `Controller.Get` y `Controller.List` abren spans mediante `tracer.Start` y adjuntan atributos como instancia, clase de recurso y nombre. `ModelReconciler` utiliza `semaphore.NewWeighted` para limitar ejecuciones, registra mensajes con `observability.ZapCtx(ctx)` y, al completar un modelo, consulta conteos y bytes de la salida y emite telemetría. El reconciliador conserva estado y avisos de particiones. Son hechos localizables, no una deducción de marketing.

**Rescatar:**
- Instrumentación en el controlador de recursos, no solamente en HTTP.
- Contexto correlacionado entre acciones, reconciliación, modelo y consulta.
- Métricas de filas/bytes y resultados después del trabajo, no solamente duración.
- Límites de concurrencia explícitos por tipo de operación.
- Estado de particiones y errores persistido en el recurso.

**Mejorar:**
- Especificar el `trace_id`/`run_id` como vínculo tipado con metadatos de negocio **sin colocar nombres sensibles en labels de métricas**.
- Guardar causas normalizadas de error: `source_unavailable`, `quota_rejected`, `semantic_invalid`, `catalog_conflict`, `data_file_missing`, `cancelled`, `deadline_exceeded`, `query_engine_failure`, `publish_conflict`.
- Separar indicadores de resultado analítico (filas, snapshot) de gastos operativos (latencia, CPU, memoria).
- Exigir la misma correlación también a vistas, pivotes, reportes y MCP; una consulta desde un agente no se trata como un canal privilegiado.

**No copiar:** una estructura de controlador Go ni semáforos globales como única estrategia multitenant. En Rust los límites deben ajustarse por presupuesto de procesos y clases de prioridad.

---

## 3. Auditoría — Cube / CubeStore: descomponer la latencia

**Fuentes leídas:**
- [`QueryQueue.ts`](https://github.com/cube-js/cube/blob/master/packages/cubejs-query-orchestrator/src/orchestrator/QueryQueue.ts).
- [`app_metrics.rs` — métricas Rust de CubeStore](https://github.com/cube-js/cube/blob/master/rust/cubestore/cubestore/src/app_metrics.rs).
- [Arquitectura y tests del Query Orchestrator](https://github.com/cube-js/cube/blob/master/packages/cubejs-query-orchestrator/CLAUDE.md).

**Evidencia:** `QueryQueue` registra/actualiza heartbeats y dispone de lógica de timeout; CubeStore expone métricas diferenciadas de cache hit, cache stale, cache weight, tiempo de consulta, throttle in-flight/queued/rejected y timings de las fases lógicas/físicas del plan. **Matiz:** el archivo de CubeStore pertenece a su motor, no al código de DuckDB, y no prueba que la misma métrica exista en cualquier edición de Cube.

**Rescatar:** taxonomía por fases, colas medibles y capacidad de localizar planificación lenta versus ejecución lenta. Medir consultas rechazadas y espera del scheduler, no esconderla dentro de `db_duration`.

**Mejorar:** agregar `semantic_compile`/`policy_compile`, versiones semánticas y snapshots DuckLake al diagnóstico. Evitar cardinalidad explosiva; el nombre del usuario o el SQL entero no forman parte de un label.

**Precaución:** un timeout lógico (`Promise.race` en parte del código de Cube) no demuestra por sí solo cancelación del trabajo nativo. En el runtime DuckDB embebido, cancelar requiere integrar el mecanismo efectivo del motor y verificar que los recursos regresan al pool.

---

## 4. Auditoría — Grafana y Lightdash: instrumentación operativa

**Fuentes leídas:**
- [`schedule.go` en Grafana](https://github.com/grafana/grafana/blob/main/pkg/services/ngalert/schedule/schedule.go).
- [Lightdash — guía backend de revisión, métricas y logs](https://github.com/lightdash/lightdash/blob/main/.github/rules/backend-review.md).
- [Lightdash — entorno Prometheus `/metrics`](https://github.com/lightdash/lightdash/blob/main/.github/CONTRIBUTING.md).
- [Grafana — monitorización propia mediante `/metrics`](https://github.com/grafana/grafana/blob/main/docs/sources/setup-grafana/set-up-grafana-monitoring.md).

**Evidencia Grafana:** el scheduler publica `BehindSeconds`, el histograma `SchedulePeriodicDuration` y conteos de evaluaciones perdidas cuando un trabajo tarda demasiado. Tiene mecanismos de `Run(context.Context)` y salida ordenada al cancelar. **Rescatar:** evaluar salud del scheduler por retraso y trabajos perdidos, no solamente por una marca `worker_running=true`.

**Evidencia Lightdash:** la guía de backend especifica Prometheus, tiempos de API/consultas/jobs, Winston para logs estructurados y Sentry para errores; su entorno dev documenta un endpoint `/metrics`. La guía confirma una práctica del repositorio, **no** demuestra cobertura perfecta de cada ruta o la semántica de todas sus métricas.

**Rescatar:** endpoint de métricas fácil de integrar, errores categorizados y guías explícitas de instrumentación en revisiones de código.

**Mejorar:** no obligar a Lightdash, Grafana ni a un colector concreto. Cumplir estándares y ofrecer paneles de operación preconfigurados exportables a cualquier backend compatible.

---

## 5. Auditoría — OpenTelemetry Rust: SDK, Collector y madurez

Fuentes:
- [Estado oficial OTel Rust](https://opentelemetry.io/docs/languages/rust/).
- [Código del exportador `opentelemetry-otlp`](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-otlp/src/lib.rs).
- [Exportadores Rust y recomendación de Collector](https://opentelemetry.io/docs/languages/rust/exporters/).
- [Configuración de Collector](https://opentelemetry.io/docs/collector/configuration/).

**Dato importante a octubre de 2026:** la web oficial etiqueta trazas, métricas y logs en Rust como **beta**. No debemos presentarlos como APIs universalmente estables ni fijar versiones de crates sin resolver compatibilidad con Tokio, `tracing`, `opentelemetry` y exporter.

**Diseño:**

```text
UI/API/Worker  --context propagation-->  Rust tracing spans
   |                                     |
   +-- event records durables            +-- OTel exporter (OTLP)
                                             |
                                    OpenTelemetry Collector (optativo)
                                             |
                              traces / metrics / logs backends elegidos
```

- Instrumentar siempre, pero hacer configurable la exportación.
- Distribución local: logs estructurados a stderr/stdout; `/metrics` si existe colector o Prometheus; trazas por OTLP habilitadas por configuración.
- Distribución empresarial: Collector con batching, límite de memoria, redacción y exportación a backends externos; no usarlo como base de datos de ejecuciones del producto.
- Datos sensibles deben eliminarse en origen **y** redaccionarse en el Collector como defensa adicional.
- Si OTel fallara, degradar observabilidad sin modificar la transacción de negocio.

---

## 6. Arquitectura objetivo: un monolito modular más workers cuando se justifique

```text
                  Svelte 5 UI / Public API / MCP
                                |
                  API Gateway + Security (Rust)
                                |
       +------------------------+----------------------+
       |                        |                      |
    Models DAG              Semantic Query           Automations
       |                        |                      |
       +-------- Query Admission / Resource Budgets ---+
                                |
                        duckdb-rs pool
                        (DuckDB in-memory)
                        /              \
             DuckLake catalog       Polars/Arrow
               PostgreSQL          transform branch
                   |                     |
             Parquet DATA_PATH <---------+
             (shared FS/object store)

  DIAGNOSTICS ACROSS MODULES:
     durable product events --> Run/Event Repository (PostgreSQL)
     tracing/metrics/logs  --> OTLP / Prometheus adapters
                             --> Collector optional --> backends
```

**No imponer Kubernetes.** Inicio: un binario Rust, UI estática, PostgreSQL y almacenamiento de Parquet accesible. Si se necesita aislar tareas pesadas: procesos worker separados ejecutando el mismo binario con rol distinto; si hay varias máquinas, cada worker accede a PostgreSQL y a un DATA_PATH compartido. El binario Rust embebe DuckDB, no demanda un servidor DuckDB.

**Importante:** en despliegue local puede utilizarse almacenamiento local Parquet, pero esa ubicación no se vuelve mágicamente compartida al escalar a otra máquina. `DATA_PATH` debe ser accesible por los workers de forma segura y consistente; si no, la plataforma debe rechazar ese despliegue multi-nodo.

---

## 7. Telemetría de tres señales y registros durables

### 7.1 Metrics: agregados bajos en cardinalidad

Latencia en segundos, bytes, cuentas, profundidad de cola, uso y saturación. Series con labels controlados: `operation`, `result`, `engine`, `tier`, `error_class`. Sin `query_id`, `run_id`, `tenant_id` arbitrario, email, ruta a Parquet ni texto SQL.

### 7.2 Traces: causalidad y análisis fino

Spans con `trace_id` propagado: una petición origina múltiples planes, jobs y consultas. Atributos de alta cardinalidad pueden ser útiles en una traza protegida, **no** en series Prometheus. Redactar cadenas sensibles, credenciales y literales.

### 7.3 Logs: eventos estructurados

Campos: `timestamp`, `level`, `service`, `build_version`, `trace_id`, `span_id`, `component`, `event_code`, `error_class`, `duration_ms`, `retryable`, `outcome`. Nunca loguear SQL libre o URL con token de almacenamiento por defecto.

### 7.4 Eventos de producto y auditoría: durabilidad diferente

- `ModelBuildStarted`, `ModelBuildValidated`, `PublishSucceeded`, `PublishRejected`.
- `QueryRejected`, `QueryCancelled`, `ExportGenerated`, `AlertDelivered`, `SecurityPolicyChanged`.
- Persistencia transaccional propia y política de acceso; el usuario puede abrir «Historial de runs» aunque se borren trazas después.
- Conservar referencias de revisión, snapshot y `run_id` y un error normalizado, no necesariamente el dataset ni la sentencia completa.

---

## 8. Esquema de traza end-to-end

```text
HTTP POST /semantic/execute                           (trace)
  +- authenticate                                     4ms
  +- authorize                                        3ms
  +- resolve_published_revision                       2ms
  +- semantic.compile                                14ms
  |    +- resolve_relationships
  |    +- policy.inject
  |    +- optimizer.semantic_rollup_selection
  +- admission.wait                                  42ms
  +- duckdb.execute                                 230ms
  |    +- ducklake.snapshot_resolve
  |    +- parquet_scan  [per-op profiling a demanda]
  |    +- aggregation
  +- result.serialize_arrow                          12ms
  +- response.transport                              10ms
client.render                                          ...
```

*Los milisegundos del ejemplo son ilustrativos y no benchmarks.*

**Invariante:** duración end-to-end no equivale necesariamente a la suma lineal de todos los spans por solapamientos; comparar eventos correlacionados, no sumar spans paralelos. `QueryDuration` incluye definiciones precisas: `queue_wait_ms`, `execution_ms`, `serialization_ms`, `api_total_ms`, `client_render_ms`.

**Asíncrono:** el contexto que pasa de `API` a `Worker` puede conservar la relación causal mediante traceparent/link de OTel; no imponer que un job de horas permanezca como una única petición HTTP abierta.

---

## 9. Contratos Rust propuestos (tipos de arquitectura, no código ya compilado)

```rust
// Ejemplo conceptual: contratos de interoperabilidad, no librería pública final.
pub struct OperationContext {
    pub request_id: RequestId,
    pub trace_id: TraceId,
    pub actor: AuthorizedActorRef,
    pub resource_revision: Option<RevisionId>,
    pub data_snapshot: Option<SnapshotRef>,
    pub deadline: Option<Instant>,
    pub workload: WorkloadClass,
}

pub enum WorkloadClass { Interactive, Preview, Background, Maintenance }

pub struct ResourceBudget {
    pub queue_wait: Duration,
    pub execution_deadline: Duration,
    pub result_max_bytes: u64,
    pub spill_max_bytes: u64,
    pub max_in_flight: usize,
}

pub struct QueryDiagnostic {
    pub query_id: QueryId,
    pub status: QueryStatus,
    pub phase_times: PhaseTimings,
    pub output_schema_hash: Option<String>,
    pub plan_fingerprint: Option<String>,
    pub snapshot: Option<SnapshotRef>,
    pub error: Option<SafeError>,
}

pub trait DiagnosticsSink: Send + Sync {
    fn record_event(&self, event: SafeOperationEvent);
    fn record_metric(&self, metric: LowCardinalityMetric);
}
```

**No filtrar `AuthorizedActor` entero al sink:** el contexto autoriza operaciones de M11; el sink recibe identidad minimizada, IDs opacos y solo campos permitidos. `DiagnosticsSink` no sustituye `AuditRepository`, porque el primero puede estar deshabilitado.

### 9.1 IDs obligatorios

| ID | Generado por | Papel |
|---|---|---|
| `request_id` | gateway | operación cliente única |
| `trace_id` | instrumentación | correlación técnica |
| `query_id` | M05 | consulta admisible al motor |
| `run_id` | M02/M14 | job durable/reintentos |
| `revision_id` | M10 | definición publicada |
| `snapshot_ref` | M03/M13 | versión física consistente |
| `policy_version` | M11 | autorización aplicada |
| `plan_fingerprint` | M04/M05 | diagnóstico de plan sin literales |

Los identificadores se validan y propagan; una petición de cliente no puede elegir un `actor` o `policy_version` alternativo para evadir seguridad.

---

## 10. Métricas operativas mínimas y convenciones

**Convención:** usar segundos y bytes; contadores con `_total`, histogramas para distribuciones y gauges para valores instantáneos. Ver [Prometheus: instrumentation](https://prometheus.io/docs/practices/instrumentation/) y [metric naming](https://prometheus.io/docs/practices/naming/).

| Métrica | Tipo | Labels seguros | Qué responde |
|---|---|---|---|
| `bi_api_requests_total` | counter | route_template, method, status_class | tráfico/errores |
| `bi_api_duration_seconds` | histogram | route_template, result | p95 de API |
| `bi_semantic_compile_duration_seconds` | histogram | mode, result | costo de planificación |
| `bi_query_wait_seconds` | histogram | workload, result | congestión |
| `bi_query_execute_seconds` | histogram | engine, workload, result | tiempo motor |
| `bi_query_inflight` | gauge | workload | concurrencia actual |
| `bi_query_rejected_total` | counter | reason, workload | saturación / permisos |
| `bi_query_cancel_total` | counter | reason, workload | cancelaciones |
| `bi_result_bytes_total` | counter | protocol | volumen enviado |
| `bi_result_serialize_seconds` | histogram | format | costo de serialización |
| `bi_cache_requests_total` | counter | cache_kind, outcome | hit/miss/stale |
| `bi_model_runs_total` | counter | outcome, strategy | estabilidad DAG |
| `bi_model_run_seconds` | histogram | strategy, outcome | tiempo transformaciones |
| `bi_model_publish_total` | counter | outcome, cause | salud publicación |
| `bi_ducklake_snapshot_age_seconds` | gauge | dataset_class | frescura revisión |
| `bi_ducklake_orphan_files` | gauge | environment | limpieza a revisar |
| `bi_ducklake_data_files` | gauge | dataset_class | proliferación archivos |
| `bi_pg_catalog_operation_seconds` | histogram | operation, result | latencia catálogo |
| `bi_worker_jobs_inflight` | gauge | class | capacidad worker |
| `bi_scheduler_lag_seconds` | gauge/histogram | job_class | retraso programación |
| `bi_delivery_attempts_total` | counter | channel, outcome | entrega de alertas |
| `bi_observability_dropped_total` | counter | signal | pérdida de telemetría |

**Distinguir trabajo de producto y operación:** alertas de negocio sobre una medida (M14) no son automáticamente alertas de que el runtime esté fallando. Ambos tipos usan alerting, pero criterios, identidades y permisos son distintos.

---

## 11. SLO y objetivos cuantificables, explícitamente provisionales

**No son prestaciones medidas ni promesas comerciales**; son plantillas a validar con fixtures y hardware de referencia.

| Servicio | SLI sugerido | Objetivo inicial ilustrativo | Ventana / cautela |
|---|---|---|---|
| API health | respuestas correctas | ≥99.9 % | 30 días; definir mantenimiento |
| BI consultas interactivas | p95 de finalización | < 2 s para consultas cubiertas por fixture representativa | no todas las ad hoc |
| Consultas simples | p95 de engine execution | < 500 ms en dataset sintético controlado | no incluye cola/transporte |
| Error de consultas | errores backend / admitidas | <1 % | excluir errores esperados del usuario |
| Materializaciones | revisiones publicadas sin fallo | ≥99 % por día | medir tareas elegibles |
| Frescura dataset | edad desde evento fuente | política por dataset | no prometer tiempo real universal |
| Programador | jitter/lag p95 | <60 s para jobs livianos | excluir saturación voluntaria |
| Report delivery | intentos exitosos definitivos | según canal | separar generar y enviar |

**Error budget:** operaciones de riesgo (migración, compaction, release) se bloquean o ralentizan si consumo del presupuesto justifica congelación; no cargar SLOs desconocidos en el README como verdades verificadas.

**Prioridad:** primero registrar histogramas y diagnósticos en cargas representativas; después acordar umbrales con quienes operarán la plataforma.

---

## 12. Presupuestos: DuckDB no controla por sí solo toda la memoria Rust

Fuentes: [DuckDB limits](https://duckdb.org/docs/current/operations_manual/limits), [OOM guide](https://duckdb.org/docs/preview/guides/performance/oom), [memory management](https://www.duckdb.org/2024/07/09/memory-management).

**Hecho verificado:** `memory_limit` se aplica principalmente al **buffer manager** de DuckDB. No limita necesariamente memoria de Arrow, Polars ni todas las operaciones externas al buffer manager. El valor de fábrica no es un presupuesto apropiado para un servidor con varios componentes embebidos.

**Política de operación propuesta:**

```text
Total RAM disponible para proceso (ejemplo ilustrativo: 32 GiB)
    ├─ DuckDB managed buffer           presupuesto reservado, no 80% por defecto
    ├─ Polars / Arrow                  heap nativo y buffers temporales
    ├─ API + caches / resultados       bounded / TTL
    ├─ OTel + runtime / threads        overhead
    └─ margen sistema / fragmentación y picos
```

El **Resource Governor** calcula límites admisibles por host y workload, sin confiar en que `memory_limit` sea suficiente. Se necesitan perfiles medidos de RSS, allocations, arena buffers, spill disk y cardinalidad de resultados.

**No asignar valores mágicos** (ej. 60% RAM, 8 threads, 10 GiB por query) sin bench. Arrancar con límites conservadores configurables, medir y fijar presets por perfil de despliegue.

**Controles operativos:** DuckDB `SET memory_limit`, `SET threads`, `temp_directory`, `max_temp_directory_size`; cuotas por clase en el coordinador Rust; límite de resultados al serializar Arrow/JSON; deadline; cancelación efectiva; evaluación de cgroups/RSS en Linux al desplegar.

---

## 13. CPU, scheduling y el problema de la sobre-suscripción

DuckDB usa paralelismo nativo; Polars utiliza su propio pool de hilos y el servidor Rust puede usar Tokio. En una sola máquina, **tres pools no multiplican los núcleos físicos**.

**Recomendación inicial:**
- Definir un tope de consultas DuckDB activas condicionado por CPU/RAM y latencia observada.
- Separar `interactive`/`preview` de `background`/`maintenance`.
- Evitar correr cargas CPU-bound bloqueantes en hilos del ejecutor Tokio; utilizar pool dedicado para C++/DuckDB y transformaciones Polars.
- Acotar también las conexiones concurrentes PostgreSQL del catálogo; más conexiones no implican más throughput.
- Habilitar threads DuckDB por consulta con un presupuesto global (sum threads aproximada, no propiedad garantizada).
- El planificador debe reservar recursos de forma justa y poder rechazar una consulta con `RESOURCE_EXHAUSTED` en vez de dejar colapsar el proceso.

**Política de admisión:** prioridad interactiva + fairness por workspace; evitar inanición de jobs. El que haya más trabajadores no debe permitir que un usuario monopolice 100% de CPU.

---

## 14. Cancelación: distinguir timeout, cancelación y finalización real

**Hecho disponible en Rust:** la documentación [DuckDB `Profile and Monitor`](https://duckdb.org/docs/preview/clients/rust/profiling) describe `interrupt_handle()` y `interrupt()` desde otro hilo; verificación de API/versiones debe realizarse en la versión fijada del crate.

**Protocolo propuesto:**
1. Cliente abandona dashboard / cambia filtro → `cancel_requested`.
2. Query Coordinator marca `generation_id` anterior como obsoleto y deja de distribuir su respuesta.
3. Si el trabajo está en espera, retirarlo de la cola.
4. Si la consulta está en ejecución, invocar el mecanismo DuckDB sobre **la conexión correcta**.
5. Confirmar terminación real; limpiar recursos temporales y devolver conexión al pool.
6. Registrar `cancel_requested`, `cancel_acknowledged`, `cancel_effective` por separado.
7. Si el query ya terminó, no reportar falsamente una cancelación efectiva.

**Peligro:** el handle puede afectar la consulta que esté ejecutando esa conexión en ese instante. Las conexiones no deben ser retornadas/reutilizadas para otro actor antes de que finalice la cancelación anterior; utilizar ownership exclusivo y tests de carrera.

**Query timeout ≠ request HTTP timeout.** Ningún frontend puede ser el único encargado de impedir consultas descontroladas.

---

## 15. Perfiles DuckDB y optimización informada

Fuente: [DuckDB profiling](https://www.duckdb.org/docs/current/dev/profiling), [`duckdb-rs` profiling](https://docs.rs/duckdb/latest/duckdb/profiling/index.html).

**Instrumentación por etapas:**
- Siempre: métricas de latencia, errores, bytes, cache, filas devueltas, estimación de scanning cuando esté disponible.
- Consultas lentas muestreadas: perfiles por operador, plan fingerprint, ratio estimado/real, operators costosos (scan/hash join/sort/group).
- Diagnóstico explícito y autorizado: `EXPLAIN` / `EXPLAIN ANALYZE` si aplica; estos pueden ejecutar consultas, por lo que no se deben lanzar ingenuamente cada vez que un usuario ve el dashboard.

La API Rust documenta árboles `ProfilingInfo` **en la versión de crate que soporta la función**; no asumir que un despliegue fijado con cliente anterior tiene la misma interfaz.

**Cuidado con los datos del plan:** nombres de tablas, columnas, filtros o rutas pueden revelar estructura interna; perfil de consulta se trata como información restringida.

**Un `plan_fingerprint` es útil para agrupar rendimiento**, pero no sustituye la versión de datos, el contenido de políticas o la revisión semántica.

---

## 16. Polars y Arrow: observar transformaciones y tránsito columnar

**Polars no es un reemplazo del catálogo transaccional de DuckLake.** Sus métricas deben enfocarse en:
- latencia de construcción del LazyFrame versus `collect`/ejecución;
- operadores/recolección de memoria y tamaño de batches;
- tiempo de conversión DuckDB → Arrow → Polars y Polars → Arrow → DuckDB;
- número de copias de datos y compatibilidad de tipos (decimal, timestamp tz, nulls, utf8 view);
- spilling/concurrency/costo al materializar en DuckLake por el ejecutor compatible.

**Contrato:** toda transformación tiene `input_schema`, `output_schema`, `row_count`, `size_bytes`, `run_id`, `snapshot/source revision` y una estrategia explícita de materialización. El runtime puede instrumentar, pero M02 decide si el resultado cumple reglas de negocio.

**Peligro de rendimiento:** «Arrow zero-copy» no significa que cada operación o conversión en ambas bibliotecas sea siempre sin copias. Probar buffers reales y contadores de memoria con los tipos utilizados.

---

## 17. ADBC: observabilidad de conectores externos

Si se utiliza ADBC para PostgreSQL u otros orígenes, medir:
- tiempo apertura/handshake/conexión;
- tiempo preparación/ejecución/fetch;
- tamaños y número de lotes Arrow;
- cancelación y expiración de deadline soportados o no por el driver;
- reconexiones, fallos TLS, retries y bytes transmitidos;
- capacidad efectiva declarada por driver (no asumir mismas opciones para todos).

**No inventar un panel de 60 métricas por driver** antes de saber cuáles son utilizables. Definir un envelope común y atributos opcionales por conector.

**Estrategia de errores:** drivers externos reportan estados heterogéneos; normalizar a `SourceUnavailable`, `Timeout`, `Unauthorized`, `RateLimited`, `InvalidQuery` y `Unknown`, preservando causa interna solo en diagnóstico protegido.

---

## 18. DuckLake/PostgreSQL: observabilidad de catálogo versus archivos físicos

Fuente: [DuckLake docs](https://ducklake.select/docs/stable/), [recommended maintenance](https://ducklake.select/docs/stable/duckdb/maintenance/recommended_maintenance).

**Dos subsistemas, dos caminos de fallo:**

1. **Catálogo PostgreSQL:** pool saturado, latencias SQL, locks, conflictos transaccionales, tamaño de WAL, backup/PITR, conexiones, schema migration.
2. **Archivos Parquet:** acceso a DATA_PATH, visibilidad entre nodos, número/tamaño de archivos, permisos, latencia de object storage, inconsistencias por borrado, credenciales y coste.

El catálogo conserva metadata y snapshots; no se debe inferir que PostgreSQL guarde el contenido íntegro de las tablas analíticas. M13 puede tener sesiones DuckDB in-memory sin archivos `.duckdb` permanentes.

**Telemetría de negocio y técnica distinta:** tamaño de una tabla no basta para diagnosticar su latencia. Medir `files_scanned`, `bytes_remote`, ratio tiny-file, cache de metadatos, latencia de catálogo, mantenimiento pendiente. Solo guardar valores que la API puede observar realmente; si el origen no informa bytes remotos, dejar `unknown` en lugar de inventar.

---

## 19. DuckLake maintenance: nunca hacer GC improvisado

Fuentes:
- [Expire snapshots](https://ducklake.select/docs/stable/duckdb/maintenance/expire_snapshots).
- [Cleanup of files](https://ducklake.select/docs/stable/duckdb/maintenance/cleanup_of_files).
- [Checkpoint](https://ducklake.select/docs/stable/duckdb/maintenance/checkpoint).

DuckLake permite expirar snapshots y limpiar archivos programados para borrado; **expirar un snapshot no borra inmediatamente sus Parquet**. Existen funciones diferenciadas para archivos huérfanos y archivos programados para limpieza. `CHECKPOINT` puede invocar varias tareas en secuencia según política y versión.

**Decisiones:**
- Jobs de mantenimiento exclusivos/serializados por catálogo y nivel de tablas cuando corresponda.
- Ejecutar siempre `dry_run` cuando sea posible para operaciones peligrosas; generar plan con conteos y bytes estimados.
- Retención compatible con time-travel, backups, consultas largas y links de reportes que fijan snapshots.
- No ejecutar garbage collection agresivo si existen lectores de un snapshot anterior.
- Postergar limpieza durante recuperación, backups no finalizados o incidentes del DATA_PATH.
- Mantener registro de snapshots retenidos por motivos operativos (pin por reporte, rollback, auditoría); revisar si la versión de DuckLake respeta esa política, y **aplicar protección externa si no es nativa**.

**Criterio de aceptación:** borrar una revisión obsoleta jamás deja rota una consulta todavía activa ni un reporte publicado con referencia fija.

---

## 20. Backups y recuperación consistente de PostgreSQL + Parquet

Fuente primaria: [DuckLake Backup and Recovery](https://ducklake.select/docs/stable/duckdb/guides/backups_and_recovery).

**Hecho verificado:** DuckLake separa backup de catálogo y backup de archivos físicos. PostgreSQL puede respaldarse mediante dump o PITR; Parquet requiere backup/versionado/replicación según el filesystem o object store. Restaurar solo PostgreSQL no recupera necesariamente archivos que se perdieron del DATA_PATH.

**Runbook propuesto:**
1. Definir RPO/RTO por despliegue; objetivo negociado, nunca derivado de «usar PostgreSQL».
2. Mantener backup continuo del catálogo PostgreSQL si se requiere PITR.
3. Mantener versionado/backup del almacenamiento Parquet.
4. Registrar checkpoint operativo / snapshot consistente como punto de recuperación.
5. Coordinar pausa o exclusión de compaction, expiración de snapshots y GC durante la captura/restore.
6. Restaurar PostgreSQL y Parquet a un par temporal coherente.
7. Validar DATA_PATH, presencia de archivos y checks de snapshots.
8. Ejecutar consultas de control y revisar totales de fixtures, esquema y permisos.
9. Rehabilitar workers y nuevas escrituras con control explícito.
10. Ejecutar simulacros trimestrales o según criticidad; medir recuperación y documentar diferencias.

**Advertencia:** ni siquiera un «backup exitoso» garantiza consistencia temporal entre el catálogo y los blobs cuando ambos se copiaron sin coordinación. Se debe probar recuperación real, no solamente existencia de ficheros backup.

**Separar los metadatos BI:** si el BI Metadata Store también usa PostgreSQL, puede vivir en esquemas/bases distintos, con permisos y backups propios; no confundirse con tablas internas DuckLake.

---

## 21. Concurrencia y alta disponibilidad: qué significa realmente

**Proceso único, monolito modular:** sencillo y eficaz mientras los SLO se cumplan.

**Multiproceso en un host:** workers embeben DuckDB separadamente y coordinan las operaciones compartidas a través de DuckLake/PostgreSQL; limitación global de trabajo se implementa en el coordinator y/o leases durables, no con un `Semaphore` Rust de un solo proceso.

**Multihost:** todos los procesos necesitan acceso a los mismos objetos Parquet; PostgreSQL y almacenamiento deben soportar la disponibilidad requerida. Pruebas de transaction conflict, lectores en snapshot anterior y fallos parciales son obligatorias.

**No confundir HA:** dos instancias de API detrás de un balanceador no hacen altamente disponible PostgreSQL, ni preservan jobs si las instancias usan memoria local sin repositorio durable.

**Estrategia de scale-out:** extraer workers de ejecución pesada antes de dividir en microservicios separados la semántica, metadata y Canvas. Medir si API, consulta o mantenimiento es el cuello de botella; luego aislar solo ese dominio.

**Prueba mínima:** un worker muere durante ejecución de reporte y la API continúa respondiendo; otra instancia recupera el job durable sin duplicar envío final ni publicar resultados incompletos.

---

## 22. Health checks: separar liveness, readiness y diagnósticos

| Endpoint ilustrativo | Qué debe comprobar | Qué no debería hacer |
|---|---|---|
| `/health/live` | Proceso activo y event loop/servicio responde | Consultar PostgreSQL en cada tick |
| `/health/ready` | Capacidad operativa de servir clase de consultas configurada | Exigir que servicios opcionales (OTel) estén activos |
| `/health/startup` | Migraciones/bootstrap/extension check completados | Ejecutar transformaciones gigantes |
| `/admin/diagnostics` | Estado autorizado de pools/colas/runs/catalog/artefactos | Exponer secretos o SQL de clientes |

**Readiness por capacidad:** si DuckLake/PostgreSQL está caído, la API administrativa de diagnósticos puede seguir disponible, pero la API de consultas debe declarar no lista para servir datos. Un endpoint universal con estado `OK` para todo conduce a falsas garantías.

**Liveness sin dependencia a DB:** evita reinicios en bucle cuando PostgreSQL tiene una interrupción transitoria; readiness refleja la dependencia fallida. Decidir degradación y balanceador de forma explícita.

---

## 23. Despliegue: perfiles, no tecnologías obligatorias

| Perfil | Topología mínima | Ventajas | Límites |
|---|---|---|---|
| **Local desarrollo** | Rust embebido + PostgreSQL dev + Parquet local + UI | poca fricción | sin HA; path solo para host |
| **Equipo pequeño** | 1 app Rust + PostgreSQL respaldado + objeto Parquet compartido | operaciones simples | punto único de fallo app |
| **Empresa** | 2+ API Rust + pool workers + PostgreSQL HA + shared object storage + telemetry | aislamiento, scale-out | requiere pruebas de concurrencia/DR |
| **Regulado/on-prem** | mismo contrato; storage compatible / red privada / IAM | control de datos | red/storage y cumplimiento específico |

**Open source por defecto:** Rust, OpenTelemetry, Prometheus, Grafana, Jaeger/Tempo, Loki o similares son opciones. No se deben incluir todos para que el servidor «se vea enterprise». Elegir el conjunto mínimo que permita medir SLO y resolver incidentes.

**Infra reproducible:** builds pinneados, migraciones reversibles o forward-only bien documentadas, credenciales por Secret Manager y variables explícitas; versiones exactas de DuckDB, extensión DuckLake y driver Postgres verificadas en arranque.

**Kubernetes:** opcional cuando operación/multi-nodo lo exija; `docker compose`/Podman apropiado para prototipo o una instalación pequeña. Ninguno implica HA por sí mismo.

---

## 24. Seguridad operacional y privacidad en telemetría

Fuentes: [OTel handling sensitive data](https://opentelemetry.io/docs/security/handling-sensitive-data/), [Prometheus labels](https://prometheus.io/docs/practices/naming/).

**Threats:** SQL con literales de usuario, headers Authorization, DSN PostgreSQL, tokens object store, filtros de consumidor, valores de celdas, nombres privados de paneles, query plans con rutas y cardinalidades.

**Política propuesta:**
- Logs por defecto **sin** consultas SQL completas ni datos de resultado.
- Fingerprint canónico de SQL y parámetros, no texto original; para debugging explícito, acceso restringido, cifrado y expiración.
- Ningún email, usuario, consumidor, SQL, dataset id no acotado o run id como label Prometheus.
- Traces de errores pueden contener `error_class` y un identificador de soporte; no secretos.
- Telemetría entra por conexión autenticada/cifrada; collector y `/metrics` no abiertos al mundo.
- Separar evidencia de auditoría de seguridad (append-only/controles) de logs operativos efímeros.
- Políticas de retención y solicitudes de eliminación donde correspondan.
- Diagnóstico de planes/descripciones sujeto a RBAC de M11.

**Protección multicapa:** redacción en el backend, redacción en Collector, control de acceso al backend de observabilidad, y análisis de muestras de logs en CI para detectar secretos.

---

## 25. Presupuesto y política de cardinalidad de métricas

Prometheus recomienda evitar labels cuya cardinalidad crece ilimitadamente. Una métrica con etiquetas por usuario, tenant, query y dashboard puede multiplicar millones de series.

**Allowlist:** `operation`, `status_class`, `workload_class`, `engine`, `deployment_role`, `environment`, `error_class` con valores finitos documentados. `tenant` solo en un agregado opt-in y bajo límite conocido; por defecto no.

**CardinalityBudget:**
- máximo de series por familia configurable;
- lint de nuevas métricas en CI;
- inspección de `count by(__name__)`/series activas;
- no generar el nombre de métrica dinámicamente;
- limitación de histogramas de buckets y tags;
- costo de almacenamiento/retención monitoreado.

**Peligro:** usar `query_id` como label facilita buscar consultas pero rompe el sistema de monitoreo. Guardar `query_id` en la base de eventos o en spans restringidos.

---

## 26. Rendimiento end-to-end: metodología de benchmarking

**Matriz de cargas, versiones y tipos**:

| Carga | Dataset sintético sugerido | Pregunta |
|---|---|---|
| KPI single-table | 12M filas/año, filtros 6–24 meses | latencia y pruning |
| Fact table grande | 60M–120M filas histórico | scan/aggregation |
| Muchas categorías | cardinalidades 10, 1k, 100k | memoria/group-by |
| Joins fact/dim | N:1 correcto, fact/fact controlado | planificación y duplicación |
| Pivots | 10, 100, 1k columnas potenciales | límites y paginación |
| Explore filtros | 1, 10, 40 paneles | fan-out y deduplicación |
| Concurrencia | 1, 5, 20, 50 usuarios simulados | saturación y fairness |
| Ingest + queries | refresh durante lectura | snapshot consistency |
| Polars conversion | batches pequeños/grandes | copias y presión memoria |
| Storage local vs objeto | latencia y throughput variados | sensibilidad remota |
| Backup/recovery | fallo de catálogo y de Parquet | RTO real |

**Los tamaños son perfiles de prueba, no capacidad garantizada.** Generar datasets con skew, nulos, decimals, timestamps, filas tardías, particiones pequeñas y cambios de esquema; el benchmark de una única tabla ordenada no revela comportamientos empresariales.

### 26.1 Metodología reproducible

1. Fijar hardware/VM/container límites y sistema operativo.
2. Registrar versiones DuckDB, DuckLake, Polars, PostgreSQL y Rust.
3. Fijar semilla de datos y SQL/consulta semántica.
4. Separar cold cache y warm cache; definir cache coherente por revisión.
5. Calentar hasta estabilizar, luego medir un número de iteraciones suficiente.
6. Registrar p50/p95/p99, throughput, RSS pico, CPU, spill, bytes leídos, errores.
7. Capturar perfil DuckDB para los casos lentos y cargas con anomalía.
8. Comparar p95 con bandas de variación y distinguir regresión de ruido.
9. Medir resultados con RLS y sin RLS para ver costo real de políticas.
10. Repetir bajo competencia real de jobs y dashboards; el rendimiento de consulta aislada no es el rendimiento del sistema.

### 26.2 Test de performance no es solo velocidad

Una consulta que termina en 40 ms pero suma mal facturación/cobranza **fracasa**. El benchmark valida primero checksum, precisión DECIMAL, nulos, totales semánticos, RLS, temporalidad y snapshot; luego mide tiempo.

---

## 27. Diagnóstico y recomendación automática de optimizaciones

**No introducir un auto-tuner que cambie modelos sin aprobación.** Proponer inicialmente un `Performance Advisor` de lectura, con reglas explicables:

- `HighQueueWait`: aumentar cap solo si memoria/CPU dejan margen o priorizar clases.
- `ManySmallFiles`: sugerir compaction DuckLake, después de comprobar retención y coste.
- `ParquetFullScan`: comprobar particionamiento y pushdown de filtros.
- `HighResultSerialization`: reducir datos devueltos, paginar, usar Arrow según protocolo.
- `HighSemanticCompile`: investigar replanificaciones por revisiones/cache, no cachear planes con políticas incorrectas.
- `HighRSS`: limitar concurrencia, threads o workloads Polars/DuckDB, evaluar spill.
- `LongPostgresCatalogLock`: revisar mantenimiento y número de escritores, no escalar la API a ciegas.
- `HighCacheMiss`: revisar versiones/snapshots de cache keys antes de ampliar TTL.

**La recomendación siempre incluirá:** evidencia, plan fingerprint, coste estimado, riesgos, tests de equivalencia y botón para solicitar cambio (fuera del M15), no mutación silenciosa.

---

## 28. Incidentes: respuesta y runbooks prácticos

### A. PostgreSQL catálogo DuckLake inaccesible

- Detectar readiness de consultas, observar errores de catálogo y conexiones.
- Pausar escrituras y compaction; no continuar sirviendo datos de un snapshot supuesto.
- Mantener API de diagnóstico protegida; responder error recuperable a consultas.
- Restaurar servicio PostgreSQL y confirmar consistencia de versiones antes de reabrir.

### B. DuckDB OOM o el proceso recibe SIGKILL

- Registrar RSS/picos y workload concurrente; no asumir que `memory_limit` evitaba la caída.
- Recuperar trabajos durables M14, liberar leases y verificar publicaciones candidatas sin commit.
- Reducir simultaneidad; revisar Polars/Arrow/results/spill; capturar perfil repro.

### C. Almacenamiento Parquet parcialmente inaccesible

- Clasificar `file_not_found`/I/O/auth; alertar incluso si PostgreSQL funciona.
- No borrar snapshots/revisiones automáticamente.
- Validar DATA_PATH compartido por worker, credenciales, expiración y replication.

### D. Pequeños archivos acumulados

- Medir file-count/tamaño medio y p95 scans; simular `CHECKPOINT`/merge bajo retención.
- Agendar mantenimiento cuando no compita con tráfico interactivo; comprobar mejora.

### E. Collector OTel caído

- Aislar pérdida de telemetría del servicio de producto.
- Contabilizar dropped signals; poner logs sanitizados a stdout si el despliegue lo configura.
- No bloquear queries; recuperar exporter con backoff y memoria acotada.

### F. Dashboards lentos con motor rápido

- Investigar estado UI, dedupe, requests repetidos, payload y render; no atribuir todo a DuckDB.
- Comparar `api_total_ms`, `serialization_ms`, `client_render_ms` y número de paneles concurrentes.

---

## 29. Orden de implementación sin sobreingeniería

### Corte 0 — Instrumentación base

- Logger Rust estructurado con trace IDs, redacción por defecto, errores tipados.
- `request_id`, `run_id`, `query_id`, `snapshot_ref` obligatorios en contratos pertinentes.
- Health `live` / `ready` separados y diagnósticos protegidos.
- Métricas esenciales de consulta y jobs; historial durable ya previsto en módulos anteriores.

**Salida:** se puede diagnosticar consulta fallida sin leer datos privados.

### Corte 1 — Query Observatory

- Desglose semántica/cola/ejecución/serialización/render.
- Profiling lento en duckdb-rs de forma opt-in y acotada.
- Rate limits, budgets por workload y cancelación real comprobada.
- Pantalla de «Query diagnostics» disponible para administradores autorizados.

**Salida:** ubicar cuellos de botella y bloquear saturación de un solo usuario.

### Corte 2 — Jobs, Storage y DuckLake

- Telemetría catálogo Postgres y Parquet, snapshots, tiny files, errores lectores.
- Medir trabajos, lag de scheduler, retries y delivery.
- Plan de GC/maintenance con dry-run y ventanas seguras.
- Runbooks y prueba de recuperación consistente.

**Salida:** fallas de refresh/publicación no destruyen la revisión activa.

### Corte 3 — Entorno empresarial

- Collector OTLP y dashboards de métricas opcionales.
- Worker isolation, limitaciones globales distribuidas cuando haya multinodo.
- Métricas de SLO, budgets, alerting de sistema, load tests y canary releases.

**Salida:** el comportamiento se conoce bajo degradación, no solo bajo éxito.

### Corte 4 — Optimización inteligente (posterior)

- Advisor explicable de planes, archivos y recursos.
- Benchmark continuo y detección de regresiones.
- No auto-modificar medidas ni seguridad por sugerencias de rendimiento.

---

## 30. Pruebas de aceptación (90 casos propuestos)

### A. Trazas y logs — 1 a 15

1. Una consulta Svelte→API→semántica→DuckDB propaga trace ID válido.
2. `query_id` se relaciona con `request_id` sin depender del Collector.
3. Un job asíncrono preserva relación causal al cambiar de worker.
4. Logout/cambio de usuario no reutiliza actor ni contexto de la consulta anterior.
5. No aparece token Bearer en logs por defecto.
6. No aparece DSN PostgreSQL con contraseña en spans.
7. No aparece texto SQL completo con literales privados por defecto.
8. El output de un query Arrow no entra a telemetría.
9. La pérdida de exporter OTel no cambia el resultado de una consulta válida.
10. El Collector rechaza configuración inválida con diagnóstico claro.
11. Redacción de atributo user.email funciona en muestra controlada.
12. El nivel debug tiene caducidad y acceso autorizado en producción.
13. El histórico durable de runs sobrevive a pérdida de logs.
14. `QueryDiagnostic` de otro actor se rechaza por autorización.
15. La API permite descargar un diagnóstico sin incluir datos de celdas.

### B. Métricas y cardinalidad — 16 a 25

16. Labels de métricas pertenecen a allowlist.
17. `query_id` nunca aparece en label Prometheus.
18. `tenant_id` y emails no generan series ilimitadas.
19. Se inicializa a cero el contador que debe existir antes de primer evento.
20. Counter de consultas admite clasificación de error estable.
21. Histogramas usan segundos, no mezcla ms/s.
22. Gauge de workers activos vuelve a cero al cerrar.
23. El endpoint de métricas no es público por defecto.
24. El presupuesto de series detecta nueva métrica de alta cardinalidad.
25. Exporter sobrecargado limita memoria y registra dropped signals.

### C. DuckDB, Polars y recursos — 26 a 45

26. `SET memory_limit` se verifica sin asumir que limita RSS total.
27. Query grande reporta RSS pico real y spill.
28. DuckDB no consume todo el CPU cuando se reservan workers interactivos.
29. Polars CPU-bound no bloquea API Tokio.
30. Arrow conversion registra tamaño y latencia en fixture de DECIMAL.
31. TIMESTAMPTZ conserva tipo y zona tras conversión soportada.
32. Cancellation request retira query pendiente sin ejecutarlo.
33. Interrupt en consulta activa produce cancelación real.
34. Interrupt de conexión antigua no cancela una query nueva de otro actor.
35. Timeout API no deja query nativa activa indefinidamente.
36. Deadline excedido queda categorizado distinto de error sintáctico.
37. Query con resultado enorme se corta antes de agotar RAM.
38. Spill supera cuota y se rechaza sin matar a toda la API.
39. Pool connections tras error reutiliza solo sesiones limpias.
40. Perfil por operador de una consulta lenta se persiste de forma restringida.
41. Usuario no autorizado no descarga perfil con nombres privados.
42. Query p95 comparado con baseline se reproduce en fixture.
43. 5–50 queries concurrentes no causan starvations indefinidas.
44. Workload background cede capacidad al tráfico interactivo según política.
45. Al sobrepasar límite de admisión devuelve error explícito y recuperable.

### D. DuckLake/PostgreSQL y mantenimiento — 46 a 65

46. Catálogo PostgreSQL sin SQLite funciona desde startup limpio.
47. DuckDB in-memory recupera tablas DuckLake tras reinicio.
48. PostgreSQL accesible + Parquet inaccesible produce readiness correcta.
49. DATA_PATH incorrecto en uno de dos workers se detecta antes de servir.
50. Refresh no expone tabla materializada parcialmente publicada.
51. Catalog transaction conflict queda logueado sin secretos.
52. Un reader activo mantiene snapshot lógico consistente mientras otro publica.
53. Dos escritores concurrentes no producen revisión fantasma.
54. Se contabilizan filas/archivos/bytes publicados según capacidad disponible.
55. Conteo de small files no se confunde con bytes totales.
56. Dry-run de expiración predice snapshots afectados.
57. GC evita snapshots fijados por política de retención aprobada.
58. Limpieza no borra Parquet que un lector activo requiere.
59. Los archivos huérfanos se diagnostican separadamente de snapshots antiguos.
60. CHECKPOINT no se ejecuta repetidamente sobre el mismo catálogo en paralelo.
61. Backup PostgreSQL restaura metadatos de snapshot seleccionado.
62. Backup Parquet restaura todos los archivos requeridos por ese snapshot.
63. Restauración del catálogo sin blobs necesarios falla validación, no declara éxito.
64. Simulacro DR entrega RTO/RPO reales medidos.
65. Se verifica versión compatible de extensión DuckLake antes de ejecutar migración.

### E. Operación/HA — 66 a 80

66. `/health/live` responde durante interrupción transitoria de PostgreSQL.
67. `/health/ready` reporta incapacidad real de servir consultas.
68. `/health/startup` bloquea habilitación prematura antes de migraciones.
69. Collector caído no afecta readiness de serving.
70. Worker muerto durante job es detectado y recuperable.
71. Reintento de job no publica dos veces el mismo run.
72. Report delivery no se duplica después de retry de canal.
73. Rollback de aplicación conserva revisión de datos publicada.
74. Migración de PostgreSQL no expone esquema incompatible temporalmente.
75. Secret rotado invalida conexión antigua sin filtrar el secreto.
76. Proceso Rust se detiene ordenadamente y libera conexiones.
77. API recibe SIGTERM y deja de admitir trabajo nuevo, drena lo permitido.
78. Job de mantenimiento no bloquea indefinidamente interactive queries.
79. Arranque multi-nodo detecta storage local no compartido.
80. Read replica/failover PostgreSQL sigue política de recuperación probada.

### F. Rendimiento, SLO y asesoramiento — 81 a 90

81. Fixture KPI correcto coincide con oráculo SQL antes de medir velocidad.
82. DISTINCT y ratios no se falsean por agregaciones en frontend.
83. Benchmark cold versus warm cache entrega histogramas separados.
84. Benchmark con 1, 5, 20 usuarios muestra p50/p95/p99 y throughput.
85. Benchmark identifica diferencia de consultas interactivas frente a batch.
86. Mismo test fija engine, extensión, hardware y semilla.
87. Una regresión de serialización Arrow se identifica sin culpar a DuckDB.
88. Advisory `ManySmallFiles` propone cambio pero no lo ejecuta sin aprobación.
89. Error budget usa denominador documentado y excluye casos del cliente definidos.
90. Dashboard administrativo de operaciones distingue métricas del sistema de métricas BI de negocio.

**Condición adicional obligatoria para todas las pruebas:** verificar seguridad y corrección de datos antes de velocidad. No aceptar una implementación que acelere una query saltándose la política de M11.

---

## 31. Riesgos de diseño y correcciones preventivas

| Riesgo | Consecuencia | Acción |
|---|---|---|
| Presuponer que DuckDB es client-server | arquitectura innecesaria | `duckdb-rs` embebido; Quack solo adaptador opcional |
| Querer evitar todo disco | OOM / lentitud | spill controlado y datos persistentes en Parquet |
| Reusar `memory_limit` como techo global | OOM del proceso | RSS/cgroups + budgets por workload |
| Registrar SQL crudo | secretos/datos expuestos | fingerprints y diagnóstico autorizado |
| Prometheus con IDs únicos como labels | millones de series | allowlists y cardinality budget |
| Enviar todos los perfiles a producción | sobrecarga y filtración | slow sampling + opt-in de administradores |
| Escalar API sin shared DATA_PATH | fallos inconsistentes | preflight de accesibilidad multi-worker |
| Restaurar solo Postgres | referencias a Parquet ausente | DR coordinado catálogo+objetos |
| GC previo a backup | pérdida de rollback/time travel | retención y exclusive maintenance window |
| Asumir éxito de cancellation por timeout HTTP | CPU sigue consumiéndose | confirmación nativa de interrupt |
| Exigir Grafana/Loki/Tempo de inicio | stack costoso | instrumentación neutral y collector optativo |
| Meter observabilidad en un módulo SQL de negocio | dependencias invertidas | contratos transversales y política de logs |
| SLO sin fixture/benchmark | promesas falsas | objetivos explícitos provisionales |
| Decir «autoscaling» cuando PostgreSQL sigue caído | baja disponibilidad real | readiness por capacidad y DR |
| OTel Rust como SDK 1.0 sin riesgos | actualizaciones rotas | fijar versiones, compatibility smoke test |

---

## 32. Contratos entregados y criterio de cierre

**Entregables de diseño para módulos anteriores:** `OperationContext`, `SafeOperationEvent`, `QueryDiagnostic`, `WorkloadClass`, `ResourceBudget`, `ResourceGovernor`, `DiagnosticsSink`, `AuditRepository` (existente), `HealthStatus`, `SloDefinition`, `MaintenanceRun` y `BackupVerification`.

**Puertas antes de declarar operativo:**

1. Una petición completa tiene rastro y latencias por fase.
2. Una consulta demasiado costosa es rechazada/cancelada sin romper las demás.
3. Permisos y datos no se filtran en logs, perfiles ni métricas.
4. Publicación y snapshots siguen consistentes frente a error/kill.
5. Se demuestra recuperación coordinada PostgreSQL + Parquet.
6. Un benchmark reproducible explica capacidad y regresiones.
7. Una configuración local funciona sin Quack, `.duckdb` persistente ni Redis.
8. La exportación a OTel/Prometheus es intercambiable y degradable.
9. La recomendación de optimización no altera datos, métricas ni políticas sin aprobación.
10. Los puntos de fallo se han probado, no solamente descrito.

**Estado real al emitir este documento:** arquitectura investigada/propuesta; no se han creado crates, probado conexiones reales de producción, establecido SLO empíricos ni ejecutado tests. La revisión futura del producto deberá fijar versiones de DuckDB, DuckLake, Polars, ADBC, OTel y PostgreSQL antes de implementar.

---

## 33. Bibliografía auditable de código, estándares y documentación

### Implementaciones estudiadas

1. **Rill `v0.90.4`** — [runtime/controller.go](https://github.com/rilldata/rill/blob/v0.90.4/runtime/controller.go) — spans `Controller.Get`/`List` y atributos del recurso.
2. **Rill `v0.90.4`** — [runtime/reconcilers/model.go](https://github.com/rilldata/rill/blob/v0.90.4/runtime/reconcilers/model.go) — semáforo, logs contextuales, conteos y telemetría de builds.
3. **Cube `master`** — [QueryQueue.ts](https://github.com/cube-js/cube/blob/master/packages/cubejs-query-orchestrator/src/orchestrator/QueryQueue.ts) — heartbeat, timeouts, cola.
4. **CubeStore `master`** — [app_metrics.rs](https://github.com/cube-js/cube/blob/master/rust/cubestore/cubestore/src/app_metrics.rs) — cache, throttle, planner, latencias y métricas nativas Rust.
5. **Cube `master`** — [Query Orchestrator architecture](https://github.com/cube-js/cube/blob/master/packages/cubejs-query-orchestrator/CLAUDE.md) — registro del paquete, drivers, tests y cola.
6. **Grafana `main`** — [schedule.go](https://github.com/grafana/grafana/blob/main/pkg/services/ngalert/schedule/schedule.go) — scheduler lag, tick timing, missed evaluation.
7. **Grafana `main`** — [instrumentación `/metrics`](https://github.com/grafana/grafana/blob/main/docs/sources/setup-grafana/set-up-grafana-monitoring.md).
8. **Lightdash `main`** — [backend-review.md](https://github.com/lightdash/lightdash/blob/main/.github/rules/backend-review.md) — estándares internos para métricas, logs y errores.
9. **Lightdash `main`** — [CONTRIBUTING.md](https://github.com/lightdash/lightdash/blob/main/.github/CONTRIBUTING.md) — entorno Prometheus en desarrollo.
10. **OpenTelemetry Rust `main`** — [OTLP exporter lib.rs](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-otlp/src/lib.rs) — soporte de señales y protocolos.

### Motor, almacenamiento y operación

11. [DuckDB — Profiling](https://www.duckdb.org/docs/current/dev/profiling).
12. [DuckDB Rust — Profile and Monitor (documentación preview, verificar versión)](https://duckdb.org/docs/preview/clients/rust/profiling).
13. [`duckdb-rs` — profiling, docs.rs](https://docs.rs/duckdb/latest/duckdb/profiling/index.html).
14. [DuckDB — Limits](https://duckdb.org/docs/current/operations_manual/limits).
15. [DuckDB — OOM](https://duckdb.org/docs/preview/guides/performance/oom).
16. [DuckDB — Workload tuning](https://www.duckdb.org/docs/current/guides/performance/how_to_tune_workloads).
17. [DuckDB — Memory management](https://www.duckdb.org/2024/07/09/memory-management).
18. [DuckLake — Overview y especificación](https://ducklake.select/docs/stable/).
19. [DuckLake — Backup and Recovery](https://ducklake.select/docs/stable/duckdb/guides/backups_and_recovery).
20. [DuckLake — Maintenance](https://ducklake.select/docs/stable/duckdb/maintenance/recommended_maintenance).
21. [DuckLake — Expire Snapshots](https://ducklake.select/docs/stable/duckdb/maintenance/expire_snapshots).
22. [DuckLake — Cleanup of Files](https://ducklake.select/docs/stable/duckdb/maintenance/cleanup_of_files).
23. [DuckLake — Checkpoint](https://ducklake.select/docs/stable/duckdb/maintenance/checkpoint).

### Estándares abiertos

24. [OpenTelemetry Rust — estado SDK](https://opentelemetry.io/docs/languages/rust/).
25. [OTLP exporters Rust](https://opentelemetry.io/docs/languages/rust/exporters/).
26. [OpenTelemetry Collector — Configuration](https://opentelemetry.io/docs/collector/configuration/).
27. [OpenTelemetry — Sensitive Data](https://opentelemetry.io/docs/security/handling-sensitive-data/).
28. [Prometheus — Instrumentation](https://prometheus.io/docs/practices/instrumentation/).
29. [Prometheus — Metric and label naming](https://prometheus.io/docs/practices/naming/).
30. [OpenTelemetry Collector — Processors](https://opentelemetry.io/docs/collector/components/processor/).

### Coherencia con informes anteriores

31. `BI_Modulo_10_Metadata_Architecture_Agnostica_2026-10-10.md` — recursos y metadatos.
32. `BI_Modulo_11_Identity_Security_Governance_Agnostico_2026-10-10.md` — autorización, auditoría y RLS.
33. `BI_Modulo_12_APIs_Integraciones_Ecosistema_Agnostico_2026-10-10.md` — contratos, APIs y propagación de contexto.
34. `BI_Modulo_13_Runtime_Analitico_Rust_DuckDB_DuckLake_PostgreSQL_Polars_ADBC_2026-10-10.md` — runtime elegido.
35. `BI_Modulo_14_Automation_Scheduling_Alerts_Reporting_Agnostico_2026-10-10.md` — scheduler, jobs, alertas y reporting.
36. M01–M09 — fuentes útiles que **deben normalizarse** eliminando la antigua dependencia de SQLVIZ/Quack obligatorio al construir el ZIP final; no fingir que ya están normalizadas.

---

**Fin del Módulo 15.** Se investiga una capa por vez. Queda pendiente la auditoría final de coherencia transversal de todos los módulos y la preparación del ZIP general cuando el usuario indique cerrar la serie.
