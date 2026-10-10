# Plataforma BI abierta — Módulo 14: Automatización, Scheduling, Alertas y Reporting
## Auditoría de código de Rill, Lightdash, Grafana y Superset; diseño de ejecución fiable en Rust y PostgreSQL

**Fecha de referencia:** 10 de octubre de 2026  
**Edición:** arquitectura propuesta 1.0, independiente de un producto previo  
**Estado:** diseño técnico fundamentado; **no** es implementación ni prueba end-to-end  
**Límite estricto:** programación, disparadores, evaluación, generación y distribución de informes, gestión de trabajos, recuperación, cumplimiento de permisos y observabilidad. Consume contratos de Models, Storage, Semantic Query, Security y APIs, pero no rediseña esas capas.  
**Decisión de runtime proveniente de M13:** backend Rust; DuckDB embebido (`duckdb-rs`), DuckLake con catálogo PostgreSQL y Parquet; Polars opcional para transformaciones; Quack no obligatorio. Este documento **no** convierte a DuckDB o a Polars en un scheduler.

> **Dictamen:** usar un **Automation Core** en Rust con definiciones versionadas, scheduler orientado a ocurrencias, ejecutor de trabajos durables, evaluador de alertas y pipeline de reportes. **PostgreSQL** coordina trabajos y estados de operación, sin mezclar sus tablas con el catálogo interno DuckLake. Un proceso de workers separado es una *modalidad de despliegue* de los mismos contratos, no una arquitectura nueva. El estado se confirma transaccionalmente; los efectos externos son *at-least-once* y requieren idempotencia, deduplicación y seguimiento por destinatario.

---

## 0. Objetivos y decisiones de producto

1. Permitir programación periódica, ejecución manual y disparadores por evento (`model_published`, `data_fresh`, `quality_failed`).
2. Reutilizar las **mismas métricas, filtros y políticas** del motor semántico para alertas, informes y consultas interactivas.
3. Separar **evaluación** de alertas de **entrega** de notificaciones, siguiendo la lección de Grafana/Alertmanager.
4. Separar **generación** de informes de su distribución: el fallo de correo no vuelve a ejecutar innecesariamente el SQL.
5. Hacer explícitos zona horaria, ocurrencia nominal, ventana de negocio, watermark, política DST y corrección de datos históricos.
6. Evitar una cadena obligatoria de Celery, Redis, Kubernetes, Temporal y microservicios en una instalación básica.
7. Permitir escalar a workers aislados y soluciones externas cuando la carga o el SLA lo justifiquen, sin alterar las definiciones de recursos.
8. Tratar exportaciones, capturas, correo y webhooks como **superficies de seguridad**, no como operaciones inocuas.
9. Mantener un rastro auditable: por qué se disparó, qué versión de datos consultó, qué resultado produjo y a quién se intentó notificar.
10. No prometer *exactly-once* sobre SMTP o webhooks de terceros: garantizar consistencia interna y aplicar deduplicación cuando la integración lo permita.

**No entra en este módulo:** arquitectura ETL propia, composición visual Canvas, catálogo de métricas, manejo de credenciales generales ni renderizado interactivo. Los usa por contrato.

---

## 1. Auditoría de referentes: mapa de código comprobado

| Proyecto | Archivos/enlaces inspeccionados | Hecho técnico observado | Límite o mejora planteada |
|---|---|---|---|
| **Rill v0.90.4** | [`runtime/reconcilers/alert.go`](https://github.com/rilldata/rill/blob/v0.90.4/runtime/reconcilers/alert.go) | `AlertReconciler`: disparos ad hoc, por horario, cambios de referencias y definiciones; estado e historial | Dar al trabajo ID duradero y *ledger* separado de la definición |
| **Rill v0.90.4** | [`runtime/reconcilers/report.go`](https://github.com/rilldata/rill/blob/v0.90.4/runtime/reconcilers/report.go) | `ReportReconciler`: programación, watermarks, intervalos pendientes y reintento | Separar calcular/representar/entregar e idempotencia por destinatario |
| **Rill v0.90.4** | [`runtime/reconcilers` contrato del controller](https://pkg.go.dev/github.com/rilldata/rill%40v0.90.4/runtime/reconcilers) | El controller reconcilia cambios y retrigger; no ejecuta en paralelo dos reconciliaciones del mismo nombre | Evitar trasladar reglas de negocio al scheduler; conservar contratos de eventos |
| **Lightdash main** | [`SchedulerService.ts`](https://github.com/lightdash/lightdash/blob/main/packages/backend/src/services/SchedulerService/SchedulerService.ts) | Servicio de definición, targets, timezone, permisos, jobs, formatos y Google Sheets | Mejorar auditoría por entrega y seguridad de destinos |
| **Grafana main** | [`pkg/services/ngalert/schedule/schedule.go`](https://github.com/grafana/grafana/blob/main/pkg/services/ngalert/schedule/schedule.go) | Scheduler con ticker, evaluación de reglas, jitter, coordinación de rutinas y métricas | Separación clara de políticas para alertas de negocio |
| **Grafana main** | [`pkg/services/ngalert/state/manager.go`](https://github.com/grafana/grafana/blob/main/pkg/services/ngalert/state/manager.go) | Administra instancias y transiciones de estados, incluidos estados ausentes | Persistencia independiente de cambios de proceso |
| **Prometheus Alertmanager** | [`alertmanager.md`](https://github.com/prometheus/alertmanager/blob/main/docs/alertmanager.md) | Agrupación, deduplicación, inhibiciones, silences, ruteo | Reutilizar ideas; no introducir un Alertmanager como obligación |
| **Superset master** | [`tasks/scheduler.py`](https://github.com/apache/superset/blob/master/superset/tasks/scheduler.py), [configuración](https://github.com/apache/superset/blob/master/docs/admin_docs/configuration/alerts-reports.mdx) | Cron, Celery scheduler + workers; CSV/XLSX/PDF/PNG; budgets y reintentos | Menor complejidad base; asegurar que una captura no agote toda la ejecución |
| **Apalis Rust** | [`apalis`](https://github.com/apalis-dev/apalis), [`apalis-postgres`](https://github.com/apalis-dev/apalis-postgres) | Librería Rust para trabajos con Postgres, heartbeat y recuperación | Evaluar en benchmarks; no acoplar el dominio a su API |

### 1.1 Grado de certeza y advertencias

- Rill está fijado al tag `v0.90.4`; sus implementaciones citadas son reproducibles en esa versión.
- Lightdash, Grafana, Superset y Apalis se revisaron en ramas publicadas (`main`/`master`) **sin fijar SHA**: el equipo deberá fijarlo para un desarrollo y una auditoría reproducibles.
- La lectura del código y documentación **no equivale a probar** su despliegue, tiempos de ejecución, semántica de reintentos, seguridad o rendimiento.
- Algunas decisiones de este informe (transacciones, locks, contratos, APIs) son propuestas nuevas, **no afirmaciones** de que estén implementadas literalmente en los proyectos examinados.

---

## 2. Hallazgos: Rill — automatización declarativa, watermarks y disparadores

### 2.1 `AlertReconciler` no es un simple `cron()`

En `alert.go` se observa que:

- Se valida borrado/desactivación antes de evaluar.
- Distingue `specHash` de `refsHash`: editar la definición de alerta no tiene la misma semántica que actualizar el modelo de origen.
- Determina `adhocTrigger`, cambio de definición, actualización de refs y `NextRunOn` vencido.
- Para un disparo puramente programado utiliza la hora nominal, no necesariamente la hora real tardía de ejecución.
- Recalcula `NextRunOn`; conserva estado/historial y notifica a distintos conectores.
- Incluye distinciones de status y posibilidad de notificar recuperación (`NotifyOnRecover`).

**Rescatar:** recursos declarativos, triggers múltiples, hashes causales y hora lógica de ejecución.

**Mejorar:** `ScheduleDefinition` y `JobOccurrence` separados; el trigger se convierte en una ocurrencia durable identificable. No reiniciar indiscriminadamente el estado operacional cuando cambia una descripción no semántica; distinguir `definition_hash`, `evaluation_hash` y `delivery_hash`.

### 2.2 `ReportReconciler`: intervalos y watermarks

En `report.go`, `executeAllWrapped` obtiene un watermark del trigger o de referencias, calcula un watermark anterior a partir de ejecuciones y crea intervalos para procesar los pendientes. Contempla ausencia de intervalos y errores de referencias. La ejecución está sujeta a un timeout.

**Rescatar:** no confundir `ahora` con `datos disponibles`; consultar por periodos de negocio de forma reproducible.

**Mejorar:** almacenar `source_revision`, `semantic_revision` y `logical_window` dentro de cada `JobOccurrence`; no inferir progreso solamente de las últimas 25 ejecuciones de un recurso. Historial operativo paginado, con retención configurada.

### 2.3 Riesgo: mezclar notificación y ejecución

En el recorrido de Rill, el reconciliador resuelve la consulta y emite notificaciones. Aunque es un diseño válido, para una plataforma BI nueva proponemos **separar** `EvaluationRun`, `ReportArtifact` y `DeliveryAttempt`: la caída de un proveedor SMTP no debe disparar una segunda evaluación costosa de datos.

---

## 3. Hallazgos: Lightdash — entregas programadas como producto

`SchedulerService.ts` muestra elementos de producto valiosos:

1. Vincula el scheduler con el recurso concreto (chart, dashboard, SQL guardado o aplicación).
2. Valida frecuencia y zona horaria en el backend; no confía solo en el formulario.
3. Maneja targets, formatos, filtros guardados y permisos para crear o disparar entregas.
4. Regenera trabajos al actualizar o habilitar una programación.
5. Comprueba permisos en la ejecución manual (`send-now`) y sobre recursos referenciados.
6. Prevé destinos de colaboración (Slack, Google Chat, Microsoft Teams) y Google Sheets con comprobación de permisos.
7. Permite consultar ejecuciones y estados de entregas.

**Rescatar:** UX de entrega, múltiples destinos, send-now, validación de filtros, zona por proyecto y auditoría.

**Mejorar:** un `DeliveryTarget` versionado tipado, con prueba de conexión; desacoplar persistencia de los trabajos de la biblioteca concreta del scheduler; controlar cuotas y carga de exportaciones grandes.

### 3.1 Lección de seguridad verificada en Lightdash

El [issue de seguridad #24389](https://github.com/lightdash/lightdash/issues/24389), divulgado en 2026 y marcado como corregido, documenta un riesgo de **SSRF** en destinos webhook de entregas. El código actual de `SchedulerService.ts` muestra validaciones `validateSchedulerWebhookTargets` en entradas relevantes. La lección para nuestra plataforma no es asumir que el fallo siga abierto, sino:

- Aplicar validación en alta, edición **y envío**; defenderse de DNS rebinding y redirecciones.
- Rechazar IP locales, privadas, link-local y endpoints de metadatos de infraestructura por defecto.
- Aplicar restricciones de salida desde la red/worker además de la validación de URL.
- Registrar el destino lógico sin guardar tokens/credenciales en logs o JSON exportable.

### 3.2 UX de entregas ≠ semántica de informes

Una entrega puede ser una captura visual, un extracto de datos, una tabla HTML o un fichero. El scheduler no debe calcular por sí mismo el SQL de medidas ni la seguridad RLS. Solo solicita una consulta a los módulos existentes con un actor válido.

---

## 4. Hallazgos: Grafana y Alertmanager — máquina de estados y ruido controlado

En [`schedule.go`](https://github.com/grafana/grafana/blob/main/pkg/services/ngalert/schedule/schedule.go) Grafana:

- Reúne reglas que deben evaluarse según intervalos;
- mantiene estado de ejecución y métricas;
- incorpora tolerancias como jitter/retries;
- separa generación de alertas de `AlertsSender`;
- considera retrasos y evaluaciones que no pudieron procesarse a tiempo.

En [`state/manager.go`](https://github.com/grafana/grafana/blob/main/pkg/services/ngalert/state/manager.go), el estado de instancias tiene semántica propia, distinta de la definición del dashboard.

En [Alertmanager](https://github.com/prometheus/alertmanager/blob/main/docs/alertmanager.md), las reglas no entregan directamente un correo por cada evaluación: antes se aplican grouping, silences, inhibition, routing y deduplicación.

**Rescatar:** separar condiciones y ruteo; cada dimensión de negocio puede ser una instancia de alerta distinta; estados `pending`, `firing`, `resolved`; suppression y agrupaciones para evitar 500 correos.

**Mejorar para BI:** incluir `no_data`, `stale_data`, `evaluation_error` y `insufficient_history` como estados independientes. Una actualización de datos puede justificar reevaluación aunque no haya transcurrido la hora nominal.

**No copiar:** tener múltiples componentes distribuidos antes de necesitar HA real, ni aplicar sin más las reglas de alertado de infraestructura a métricas comerciales con cierres mensuales.

---

## 5. Hallazgos: Superset — amplitud de reporting y coste operativo

La configuración oficial y `tasks/scheduler.py` utilizan un scheduler Celery y trabajos de workers para ejecutar informes programados. Soportan formatos PDF, PNG, CSV y XLSX, plazos de ejecución y reintentos de entrega. Las capturas visuales requieren un navegador sin interfaz y más recursos que un CSV.

**Rescatar:** presupuesto global de ejecución, reservas de tiempo para render/entrega, separación de exportación tabular y visual, y aislamiento de workers de capturas pesadas.

**Mejorar:** no hacer Redis/Celery/Chrome obligatorios en instalación mínima; iniciar con tareas Rust y Postgres. Admitir renderizado headless solo cuando se necesite PDF visual fiel. Los reportes HTML/tabulares pueden construirse sin navegador.

**Advertencia:** la documentación y las incidencias de distintas versiones de Superset presentan diferencias en despliegues de scheduler; tomar el archivo de la versión exacta al replicar una prueba, no mezclar consejos de Helm antiguos con una nueva versión.

---

## 6. Comparación de estrategias de orquestación

| Solución | Rust nativo | Persistencia | Fortaleza | Coste/riesgo | Dictamen |
|---|---|---|---|---|---|
| **Postgres + Tokio + SQLx** | Sí, dominio propio | PostgreSQL | Control contractual y pocas dependencias | Mantener un scheduler y leases correctos es trabajo serio | **Elegir contrato; motor simple v1** |
| **Apalis + apalis-postgres** | Sí | PostgreSQL | Worker, retries, heartbeats, middleware | Cambios de API / semantics específicas | **Evaluar como implementación de worker** |
| **Apalis + PGMQ** | Sí | PostgreSQL/PGMQ | Cola PostgreSQL integrada | Extensión/paquetes extra a evaluar | Opcional |
| **Temporal** | SDK Rust: verificar madurez exacta | Servicio y DB propios | Workflows largos y reanudables | Infraestructura y operación mayores | Solo si workflows justifican |
| **Restate** | Rust y servidor | Backend de ejecución durable | Workflows/reintentos con buen DX | Acoplamiento y operación | Evaluación futura |
| **Celery + Redis** | No en núcleo Rust | Redis + workers Python | Ecosistema amplio | Runtime adicional, duplicación | No obligatorio |
| **Kubernetes CronJob** | Indiferente | Kubernetes | Cron infraestructural | Menor control de estado de producto | Solo para despliegues K8s |
| **cron local** | Sí | Ninguna durable | Muy simple | Sin HA, estado ni catch-up | Solo tareas no críticas |

**Elección:** definir interfaces `JobStore`, `Scheduler`, `WorkerTransport`. Una implementación inicial puede usar SQLx+Postgres; Apalis se permite como adaptador de workers solo tras ensayos de orden, recuperación, cancelación y compatibilidad. **No reimplementar su stack de middleware si ofrece exactamente las garantías buscadas.**

### 6.1 Por qué no confundir “todo nativo en Rust” con “sin dependencias de infraestructura”

Un worker embebido vive en un proceso Rust; si ese proceso se apaga, las ocurrencias pendientes deben permanecer en PostgreSQL. El mismo binario puede ejecutar `api`, `scheduler` y `worker` como roles diferentes, sin crear desde el principio proyectos/servicios inconexos.

---

## 7. Arquitectura objetivo y fronteras

```text
                   BI API / UI / CLI / MCP
                           |
                    Automation API
                           |
                  Schedule / Trigger Core
                           |
                Occurrence Ledger (Postgres)
                           |
          +----------------+----------------+
          |                |                |
     Model/Refresh     Alert Evaluator   Report Pipeline
          |                |                |
        M02/M03           M04/M05          M04/M05
          |                |                |
     Published Rev     Alert Instances  Rendered Artifact
          |                |                |
          +----------- Event Bus ------------+
                           |
                  Delivery Outbox
                           |
                  Notification Router
                   /       |       \
                 Email  Webhook  Chat/Links
                           |
                Delivery status / audit
```

**Contratos externos obligatorios:**

- **M02 Models:** `request_build(model_ref, build_window)` devuelve un `BuildRunRef`, no SQL oculto.
- **M03 Storage:** `PublishedDatasetRef`, snapshot/epoch y evento de publicación **confirmada**.
- **M04 Semantics:** `SemanticQuery` validada en un `SecurityContext` inmutable.
- **M05 Query Serving:** `QueryRequest`, result stream, timeout, cuotas, snapshot y cancelación.
- **M08 Canvas:** `DashboardRevisionRef`, renderizado serializable y filtros guardados.
- **M09 Visualization:** `VizSpec`; si se necesita screenshot, la herramienta de captura está fuera del scheduler.
- **M10 Metadata:** `ScheduleDefinition`, `AlertRuleDefinition`, `ReportDefinition`, revisiones.
- **M11 Security:** actor, permisos y política efectiva del destinatario.
- **M12 API:** create/pause/run-now/history/acknowledge, sin rutas particulares obligatorias.
- **M13 Runtime:** DuckDB embebido/DuckLake vía Execution adapter, **no** acceso directo de scheduler a `postgres_query` o `ATTACH`.

---

## 8. Responsabilidades internas propuestas

| Componente | Hace | No hace |
|---|---|---|
| `DefinitionValidator` | Revisa cron, referencias, ventanas, tipos y destinos | Ejecutar SQL |
| `OccurrencePlanner` | Proyecta ejecuciones futuras con timezone/DST | Enviar correos |
| `TriggerRouter` | Consume cron, manual y eventos publicados | Aplicar filtros RLS |
| `JobLedger` | Estado durable, lease, reintentos, fencing, historial | Guardar datos analíticos |
| `JobDispatcher` | Presupuestos, prioridad y límites por tenant | Calcular medidas |
| `AlertEvaluator` | Solicita consultas semánticas y evalúa reglas tipadas | Contactar Slack directamente |
| `AlertStateStore` | Persiste instancias, pending/firing/resolved, no-data | Persistir dashboards |
| `ReportBuilder` | Construye dataset/reporte/snapshot en artefacto | Ejecutar plugins sin sandbox |
| `ReportRenderer` | HTML, PDF, PNG, CSV, XLSX según capacidades | Cambiar métricas de negocio |
| `ArtifactStore` | Persistencia temporal con TTL y ACL | Ser el catálogo DuckLake |
| `DeliveryOutbox` | Intenciones de entrega por destinatario | Asegurar exactly-once SMTP |
| `NotifierAdapter` | SMTP, webhook, chat; reintentos y respuestas | Decidir quién puede ver datos |
| `AutomationObservability` | Métricas, trazas, logs, auditoría | Guardar secretos en claro |

---

## 9. Recursos públicos declarativos

### 9.1 `ScheduleDefinition`

```yaml
apiVersion: bi.open/v1    # ejemplo de contrato propuesto, NO estándar aprobado
kind: Schedule
metadata:
  id: sch_monthly_revenue
  name: Facturación mensual
spec:
  trigger:
    type: cron
    expression: "0 8 3 * *"
    timezone: America/La_Paz
    dst_policy: skip_nonexistent
    ambiguous_policy: first
  condition:
    type: dataset_revision_ready
    datasets: [facturacion_gold]
    freshness_max_age: PT24H
    wait_timeout: PT12H
  action:
    type: generate_report
    report_ref: commercial_monthly
  concurrency_policy: forbid_overlap
  missed_run_policy: latest_within_window
  max_lateness: P2D
  retry:
    max_attempts: 3
    backoff: exponential_jitter
```

**Interpretación:** cron programa una *oportunidad*, la condición de frescura determina si es seguro ejecutar; ninguna condición permite omitir reglas RLS.

### 9.2 `AlertRuleDefinition`

```yaml
apiVersion: bi.open/v1
kind: AlertRule
metadata:
  id: alert_collection_drop
spec:
  query_ref: metric.collection_ratio
  dimensions: [area_id]
  condition:
    op: lt
    value: "0.80"
    value_type: decimal
  evaluation:
    trigger: after_publish
    dataset_ref: cobranza_gold
    window: current_closed_period
    pending_successive_evaluations: 2
    no_data: mark_no_data
    stale_data: mark_stale
  routing_policy_ref: commercial_alerts
```

### 9.3 `ReportDefinition`

```yaml
apiVersion: bi.open/v1
kind: Report
metadata:
  id: commercial_monthly
spec:
  source:
    type: dashboard
    resource_ref: commercial_executive
    revision_policy: published_compatible
  filters:
    area_id: ["001", "002"]
  render:
    format: pdf
    page_size: A4
    orientation: landscape
    locale: es-BO
  delivery_policy_ref: commercial_management
  failure_policy:
    partial_panels: reject
    delivery_retry: enabled
```

**Nota:** las estructuras YAML son propuestas. Los editores visuales pueden escribirlas mediante el repositorio de metadatos elegido; no se crea un DSL obligatorio ni un formato `.sqlviz`.

---

## 10. Scheduler correcto: ocurrencia lógica ≠ hora real

Una programación contiene, al menos:

- `schedule_id` estable y `definition_revision`.
- `nominal_local_time` con zona IANA.
- `scheduled_at_utc` calculado e inmutable para esa ocurrencia.
- `logical_window_start` y `logical_window_end` para el periodo BI.
- `expected_data_revision` o criterio para esperar publicación.
- `trigger_cause` (`cron`, `event`, `manual`, `retry`, `backfill`).
- `occurrence_key`: idempotencia por acción/revisión/periodo, **no** por hora de inicio del worker.

### 10.1 Hora local y DST

No utilizar simplemente el offset `-04:00` como sustituto de zona IANA. Incluso si el caso inicial Bolivia no tiene cambio estacional, un SaaS internacional sí lo tendrá.

**Políticas para una hora inexistente:** `skip`, `shift_forward`, `error`.  
**Políticas para una hora ambigua:** `first`, `second`, `both`, `error`.  
**Default sugerido:** `skip_nonexistent` + `first_ambiguous`, configurable; emitir advertencia en preview.

### 10.2 Semántica de catch-up

| Política | Comportamiento | Uso |
|---|---|---|
| `skip_missed` | descartar ejecuciones no iniciadas durante corte | alertas de alta frecuencia |
| `latest_only` | ejecutar solo última ocurrencia pendiente | snapshots ejecutivos |
| `bounded_catchup` | recuperar hasta N ocurrencias/horas | reports diarios |
| `all_missed` | recuperar cada ventana con control de cuota | series históricas/financieras |

**Regla:** nunca inferir catch-up solo a partir de la hora actual; registrar las ocurrencias planificadas y su causa.

### 10.3 Periodos de negocio

`2026-09` de facturación no equivale al mes en que se ejecutó el reporte. La generación debe incorporar `business_calendar`, `time_role`, `closed_period`, `data_revision` y `as_of` para reproducibilidad.

---

## 11. Motor de eventos: trabajos por publicación de datos

Secuencia propuesta:

```text
M02 termina transformación de cobranza
  -> M03 publica revisión validada R52
  -> evento durable DataRevisionPublished(R52, dataset, watermark)
  -> TriggerRouter detecta dependencias explícitas
  -> crea JobOccurrence(report collection_daily, R52, period D)
  -> Worker solicita SemanticQuery con snapshot R52
  -> evaluaciones y reportes usan los mismos datos
  -> Outbox entrega resultados autorizados
```

**No disparar** sobre `model_started`, archivos Parquet a medio escribir, recepción de manifest incompleto, cambios de catálogo sin commit o una tabla parcial.

### 11.1 Transactional outbox para publicación

Idealmente el productor M03 registra un evento de publicación y el puntero de versión en una misma transacción **cuando pertenecen a la misma autoridad transaccional**. Si DuckLake y la metadata de BI residen en esquemas/bases/transacciones independientes, **no asumir atomicidad distribuida**: reconciliar por `published_revision` y entregar eventos idempotentes.

### 11.2 Evitar tormentas de triggers

- Coalesce por dataset+revision+window.
- Ventana de enfriamiento por regla.
- Dependencias explícitas, nunca comparar solo nombres de tablas.
- Priorización para los informes críticos.
- Backpressure: se aceptan y registran eventos mientras los workers están saturados, con límites y alarmas operativas.

---

## 12. Modelo de persistencia operativa en PostgreSQL

**Decisión:** catálogo DuckLake y ledger de automatización pueden utilizar el **mismo servidor PostgreSQL**, pero con bases o esquemas separados, roles distintos, pools distintos, aislamiento de cargas y políticas de backup/retención independientes.

- Catálogo DuckLake: mantenido por la extensión/contrato DuckLake; no modificar tablas internas.
- Metadatos BI: definiciones y revisiones, mediante `DefinitionRepository`.
- Automatización: `automation.schedule_occurrence`, `automation.job`, `automation.alert_instance`, `automation.report_artifact`, `automation.delivery_outbox`.
- Telemetría: exportada a OTel y sistema de métricas, no guardada indefinidamente en tablas de jobs.

**No se usa SQLite.** Los ficheros temporales del runtime Rust no se convierten por eso en catálogos de datos.

### 12.1 DDL de referencia (reducido, orientativo)

```sql
CREATE SCHEMA IF NOT EXISTS automation;

CREATE TABLE automation.job (
    job_id              uuid PRIMARY KEY,
    tenant_id           uuid NOT NULL,
    occurrence_key      text NOT NULL,
    job_kind            text NOT NULL,
    payload             jsonb NOT NULL,
    state               text NOT NULL CHECK (
                          state IN ('queued','leased','running','succeeded',
                                    'retry_wait','failed','cancelled','dead_letter')),
    attempt             integer NOT NULL DEFAULT 0,
    not_before          timestamptz NOT NULL,
    lease_owner         text,
    lease_expires_at    timestamptz,
    fencing_token       bigint NOT NULL DEFAULT 0,
    scheduled_at        timestamptz NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, occurrence_key)
);

CREATE INDEX job_claim_idx ON automation.job (not_before, scheduled_at)
WHERE state IN ('queued','retry_wait');

CREATE TABLE automation.delivery_outbox (
    delivery_id         uuid PRIMARY KEY,
    tenant_id           uuid NOT NULL,
    occurrence_key      text NOT NULL,
    recipient_ref       text NOT NULL,
    artifact_ref        text NOT NULL,
    channel             text NOT NULL,
    state               text NOT NULL,
    attempt             integer NOT NULL DEFAULT 0,
    idempotency_key     text NOT NULL,
    provider_message_id text,
    next_attempt_at     timestamptz NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE(tenant_id, idempotency_key)
);
```

**Notas de ingeniería:** añadir `CHECK` de canales/estados, FKs hacia recursos bajo la autoridad apropiada, índices por tenant y vencimiento de leases, particiones/TTL según escala, auditoría de cambios y restricciones de acceso por rol SQL. El DDL no es una migración lista para producción.

---

## 13. Concurrencia, leases, fencing y reintentos

### 13.1 Política de ejecución

- `forbid_overlap`: como máximo una ocurrencia activa por schedule/model/dataset crítico.
- `queue`: encolar la siguiente ejecución hasta que termine la previa.
- `replace`: cancelar la previa si es seguro; solo para trabajos con side effects controlados.
- `allow_bounded(n)`: paralelismo limitado por tipo, tenant y origen.

### 13.2 Reclamación en PostgreSQL

Concepto transaccional:

```sql
WITH claim AS (
  SELECT job_id
  FROM automation.job
  WHERE state IN ('queued', 'retry_wait')
    AND not_before <= now()
  ORDER BY scheduled_at, created_at
  LIMIT 1
  FOR UPDATE SKIP LOCKED
)
UPDATE automation.job AS j
SET state = 'leased',
    lease_owner = $1,
    lease_expires_at = now() + interval '60 seconds',
    attempt = attempt + 1,
    fencing_token = fencing_token + 1,
    updated_at = now()
FROM claim
WHERE j.job_id = claim.job_id
RETURNING j.*;
```

**Para producción:** incluir `tenant_id`, prioridad, fairness, prevención de starvation y políticas por `job_kind`, y usar un *reaper* de leases vencidos. Un worker debe presentar `(job_id, lease_owner, fencing_token)` al confirmar estado; si el lease caducó y otro lo obtuvo, su confirmación debe ser rechazada.

### 13.3 At-least-once + idempotencia

- Reintentos del mismo `occurrence_key` no recrean datos ni duplican modelos.
- La representación de PDF puede repetirse si el worker muere; usar `artifact_digest` y claves deterministas para deduplicar.
- La entrega SMTP no ofrece una transacción atómica junto con PostgreSQL: una caída tras el envío y antes del `UPDATE` puede duplicar correos.
- Guardar `provider_message_id` cuando el canal lo entregue; usar `Idempotency-Key` con proveedores que lo soportan.
- Prefijar el asunto o enlace con un identificador estable de reporte puede ayudar a los receptores a identificar duplicados, pero no equivale a garantía de exactamente una entrega.

### 13.4 Clasificación de errores

| Error | Política |
|---|---|
| `temporary_db_disconnect` | retry jitter acotado |
| `network_timeout` | retry, si operación idempotente o deduplicable |
| `rate_limited` | respetar `Retry-After` + cuota |
| `auth_forbidden` | fail permanente, auditoría |
| `invalid_definition` | bloqueo antes de enqueue/publicación |
| `security_policy_revoked` | cancelar entrega y destruir enlace vigente |
| `artifact_too_large` | fail con recomendación de formato/link |
| `missing_data` | política de alerta específica, no siempre error técnico |
| `ducklake_revision_unavailable` | retry/catch-up o fail auditado según retención |
| `cancelled_by_user` | no retry automático |

---

## 14. Diseñar el scheduler como máquina de estados

```text
Schedule ACTIVE
    | plan occurrence
    v
Occurrence PLANNED -> WAITING_FOR_DATA -> READY
                                         |
                                         v
                                      QUEUED
                                         |
                                         v
                                      LEASED -> RUNNING
                                                |
                             +------------------+------------------+
                             |                  |                  |
                          SUCCEEDED         RETRY_WAIT          FAILED
                             |                  |                  |
                         OUTBOX READY       QUEUED AGAIN        DEAD_LETTER
                             |
                             v
                       DELIVERING
                        /     \
                     SENT     RETRY / FAILED
```

- `Schedule` = definición versionada; `Occurrence` = ventana; `Job` = acción concreta; `Delivery` = destinatario y canal.
- La pausa del Schedule no implica borrar ejecuciones ya emitidas salvo opción explícita.
- Cancelación de un Job no elimina el historial ni rompe revisiones publicadas.
- Al cambiar definición, conservar ejecuciones anteriores asociadas a su revisión original.
- El scheduler y el worker pueden ejecutarse en el mismo proceso en modo local o separados en cluster.

---

## 15. Alert Engine: regla semántica, no SQL arbitrario

Pipeline:

1. Validar `AlertRuleDefinition` y que sus medidas/campos sean permitidos.
2. Resolver actor efectivo y contexto de seguridad.
3. Elegir revisión/snapshot y período de negocio.
4. Ejecutar `SemanticQuery` en M04/M05 con presupuestos.
5. Evaluar condición tipada (`DECIMAL`, `TIMESTAMP`, `NULL`), por dimensión.
6. Aplicar retardo, umbral, histéresis y estado anterior.
7. Escribir `AlertEvaluation` + transición de estado de forma transaccional.
8. Encolar `NotificationIntent` solo si corresponde a cambio o repetición autorizada.
9. Aplicar políticas de agrupación, silencios, canales, horarios y destinatarios.
10. Guardar resultado de entrega sin perder evidencia de la evaluación.

### 15.1 Estados de instancia

| Estado | Significado | ¿Enviar? |
|---|---|---|
| `normal` | condición no cumplida | resolución si antes firing |
| `pending` | primer/segundo incumplimiento, aún no confirmado | no, salvo política |
| `firing` | incumplimiento confirmado | sí, bajo routing/dedup |
| `no_data` | consulta válida, sin observaciones suficientes | política explícita |
| `stale_data` | consulta sobre revisión demasiado antigua | advertencia distinta |
| `evaluation_error` | fallo técnico o de SQL/permissions | alerta operacional separada |
| `paused` | usuario administrativamente suspendió regla | no |
| `suppressed` | estado evaluado, notificación silenciada | no |

### 15.2 Histéresis y evaluación por dimensión

Ejemplo: disparar si `ratio_cobranza < 0.80` durante dos revisiones consecutivas; resolver cuando `ratio_cobranza >= 0.82` durante dos evaluaciones válidas. `area_id="002"` y `area_id="101"` son instancias independientes. No convertir a cadena valores tipados ni confundir ausencia de fila con valor cero.

### 15.3 Cambios históricos

Si se corrige cobranza de septiembre luego del cierre, permitir `recompute` para ese período; **no reenviar indiscriminadamente** todos los correos históricos. Generar `revised_alert` solo bajo política aprobada y con vínculo al evento previo.

### 15.4 Notificación de recuperación

Una recuperación requiere evaluación actual válida; no resolver una alerta simplemente porque el origen dejó de responder. En ese caso usar `stale_data` o `evaluation_error`.

---

## 16. Notification Router: canal desacoplado del evaluador

Funciones:

- Resolver `DeliveryPolicy` (condición → destinatario/canal/severidad/horario).
- Agrupar instancias por etiquetas (`team`, `severity`, `area`, `period`) sin mezclar tenants.
- Soportar `silences` manuales, inhibiciones y quiet hours.
- Deducir `dedup_key` por estado + revisión + destinatario + canal.
- Limitar tasa por tenant, política, proveedor y destinatario.
- Mostrar `pending`, `delivered`, `partial_delivery`, `failed`, `unknown`.
- Conservar evidencia del valor, la ventana y la revisión de dato.

**La notificación no debe incluir por defecto todo el dataset**. Usar resumen mínimo y enlace autenticado a Explore; adjuntos con datos sensibles requieren permiso expreso.

---

## 17. Report Engine: dos rutas de generación distintas

### 17.1 Exportación de datos (sin navegador)

`SemanticQuery` → streaming Arrow/record batches → `CSV`, `XLSX`, `Parquet`, JSON opcional. Emplear streaming/chunks y límites de filas, bytes, celdas y CPU.

- CSV: UTF-8, separador/localización definidos; escapar fórmulas iniciadas con `=`, `+`, `-`, `@`, controles y variantes peligrosas para evitar CSV formula injection.
- XLSX: `rust_xlsxwriter` como alternativa Rust a **evaluar**, con precisión de DECIMAL/BigInt controlada; Excel tiene limitaciones de precisión numérica, por lo que debe existir política para serializar como texto si corresponde.
- Parquet: Arrow/Parquet columnar; conservar tipos y metadatos, sin convertir todo a JSON.
- JSON: serializar `DECIMAL`, `BIGINT`, fechas y NaN de forma acordada, nunca pasar silenciosamente a `f64`.

### 17.2 Captura de dashboard (navegador controlado)

`DashboardRevision` + snapshot y filtros fijados → HTML/Svelte render en URL interna autenticada → navegador headless aislado → PNG/PDF → `ArtifactRef`.

**No prometer PDF fiel solo por usar un crate Rust.** El backend puede seguir siendo Rust mientras Chromium se ejecuta como *worker auxiliar* aislado bajo presupuesto de memoria y tiempo. Desacoplar `CaptureAdapter` para sustituirlo.

**Headless seguridad:** origen allowlist, token efímero de un solo uso, red restringida, sin acceso a API pública sensible, CSP, límites de tiempo y tamaño, prohibición de navegación arbitraria, deshabilitar ejecución de contenido no confiable o sanear Markdown.

### 17.3 Generación y distribución desacopladas

Una ocurrencia puede producir un artefacto persistido una vez y luego N intentos de entrega. Si falla SMTP para un destinatario, reintentar solo ese envío. Si falla captura, ningún canal debe recibir un enlace afirmando que el PDF está listo.

---

## 18. Rendimiento de informes: presupuestos y presión de memoria

| Tipo de trabajo | Pool | Principal recurso |
|---|---|---|
| Evaluación de alerta KPI | Query workers | Latencia SQL y cuota |
| Export CSV/Parquet grande | Streaming workers | I/O, Arrow batches, red |
| XLSX | Export workers | CPU/memoria y límites de celdas |
| Dashboard PDF/PNG | Browser workers | CPU/RAM del navegador |
| SMTP/Webhooks | Delivery workers | Red, rate limits |
| Model refresh | Model workers | DuckDB/DuckLake y transacciones |

**Recomendación:** aun con un único binario Rust, separar semáforos de concurrencia y presupuestos por job kind. No usar un pool de conexiones DuckDB destinado a consultas interactivas como si fuera infinito. El scheduler no debe competir sin límite con queries de Explore.

### 18.1 Cuotas

- Por tenant, por usuario, por schedule y por destino.
- Límite de ejecuciones por hora; preview de coste antes de programar 10.000 informes.
- Número de destinatarios por reporte; fanout controlado.
- Bounded result sizes; no truncar silenciosamente datos regulatorios: fallar o producir exportación paginada/manifiesto.
- Tiempos límite separados para SQL, render, serialización y entrega.
- Retención y cleanup de artefactos huérfanos.

---

## 19. Seguridad y autorización de reportes programados

### 19.1 ¿Con identidad de quién se ejecuta?

Tres modos compatibles, con política explícita:

1. `run_as_owner`: identidad del creador/editor; permisos revalidados en cada ejecución.
2. `run_as_service_principal`: cuenta de servicio con scopes mínimos; requiere aprobación administrativa.
3. `run_as_recipient`: cada receptor recibe datos filtrados según sus permisos actuales; si varios destinatarios tienen políticas diferentes, no reutilizar el mismo artefacto privado.

**Default recomendado:** ejecución con identidad del propietario actual y entrega solo a destinatarios explícitamente autorizados; cuando se requiere segmentar por receptor, `run_as_recipient` y generación/caché por fingerprint de política. Un `service_principal` nunca es `admin` implícito.

### 19.2 Regla invariante

`effective_policy = actor_permissions ∩ resource_policy ∩ recipient_entitlement ∩ delivery_channel_policy`.

No se serializan credenciales en definiciones, jobs ni archivos de informe. Una revisión de dashboard accesible no confiere acceso a los datos que contiene si el actor ha perdido permiso al momento de ejecución.

### 19.3 Enlaces temporales

- Enlace firmado expira tras ventana breve; alcance por artefacto, destinatario y tenant.
- Posibilidad de revocación antes del vencimiento.
- No revelar bucket key real en URLs públicas.
- Auditoría de descarga y comprobación de políticas a la descarga cuando es factible.
- Archivos con datos personales requieren controles de retención y canales cifrados.

### 19.4 Webhooks y SSRF

- Permitir HTTPS; destinos internos solo mediante conectores explícitos aprobados por administrador.
- Resolver y comprobar DNS/IP al conectar; bloquear redirección a rangos prohibidos.
- No enviar `Authorization` o tokens a dominios distintos.
- TLS y hostname verificados; límites por payload y respuesta.
- Logging saneado; rotación de secretos; replay protection para mensajes entrantes.

---

## 20. Consistencia de datos en envíos automáticos

### 20.1 Reproducibilidad semántica

Cada `ReportArtifact` guarda:

```text
report_definition_revision
semantic_model_revision
source_dataset_revisions
snapshot_id / as_of policy
business_window
filter_bindings
policy_fingerprint
actor_scope
generation_version
renderer_version
artifact_digest
```

Un PDF sin revisión de datos puede ser visualmente correcto pero históricamente irreproducible.

### 20.2 DuckLake + catálogo PostgreSQL

El scheduler **no** manipula las tablas internas del catálogo DuckLake. Solicita al Query Engine una revisión/snapshot publicada y mantiene la referencia mientras el reporte dure. La retención debe contemplar reportes en curso y enlaces vigentes. No supone que una transacción de la BD operacional PostgreSQL cubra simultáneamente el commit DuckLake y el envío de correo.

### 20.3 Diferencias entre frescura, puntualidad y consistencia

- **Frescura:** `data_watermark >= requerido`.
- **Puntualidad:** ejecución dentro del SLA horario.
- **Consistencia:** todos los paneles del reporte consultan un corte compatible.
- **Completitud:** el conjunto esperado de remesas/particiones/entidades está cerrado.

Un reporte puede ser puntual pero incorrecto porque faltan remesas. La condición de publicación exige controles de calidad definidos en M02/M03, no solo un timestamp reciente.

---

## 21. Contratos Rust propuestos

Las firmas son pseudocódigo tipado cercano a Rust. No están compiladas ni fijan una dependencia concreta.

```rust
pub struct OccurrenceKey {
    pub schedule_id: ScheduleId,
    pub definition_revision: RevisionId,
    pub scheduled_at_utc: DateTime<Utc>,
    pub logical_window: BusinessWindow,
    pub deduplication_scope: String,
}

pub enum Trigger {
    Cron { expression: String, timezone: Tz, dst: DstPolicy },
    DataPublished { dataset: DatasetRef, min_revision: Option<RevisionId> },
    Manual { actor: ActorId },
    WebhookEvent { event_id: EventId },
}

#[async_trait]
pub trait OccurrencePlanner: Send + Sync {
    async fn due(&self, schedule: &ScheduleDefinition, now: DateTime<Utc>)
        -> Result<Vec<PlannedOccurrence>, PlanningError>;
}

#[async_trait]
pub trait JobLedger: Send + Sync {
    async fn enqueue_once(&self, occurrence: PlannedOccurrence)
        -> Result<JobRef, JobError>;
    async fn claim(&self, worker: WorkerId, limits: LeaseLimits)
        -> Result<Option<LeasedJob>, JobError>;
    async fn heartbeat(&self, job: JobRef, token: FencingToken)
        -> Result<(), JobError>;
    async fn complete(&self, job: JobRef, token: FencingToken, result: JobResult)
        -> Result<(), JobError>;
}

#[async_trait]
pub trait ReportRenderer: Send + Sync {
    fn capabilities(&self) -> RendererCapabilities;
    async fn render(&self, input: ReportRenderRequest)
        -> Result<ArtifactRef, RenderError>;
}

#[async_trait]
pub trait DeliveryAdapter: Send + Sync {
    fn channel(&self) -> DeliveryChannel;
    async fn deliver(&self, request: DeliveryRequest)
        -> Result<DeliveryReceipt, DeliveryError>;
}
```

### 21.1 Frontera esencial

`JobLedger` no contiene consultas a modelos ni sabe de `.duckdb`. Los handlers especializados usan `SemanticQueryService`, `ModelRunService`, `ArtifactStore` y `PolicyEvaluator` mediante interfaces.

### 21.2 Polars, ADBC y Arrow en este módulo

- **Polars:** opcional para composición/transformación de resultados cuando es mejor que SQL; el scheduler no lo necesita para programar.
- **ADBC:** opcional para conectores/consultas externas, nunca para sustituir leases de trabajo.
- **Arrow:** transporte óptimo para datasets de reportes y exportación tabular en lote/stream.
- **DuckDB:** ejecuta SQL analítico; cron no se almacena como tabla DuckDB.
- **DuckLake/Postgres:** snapshot y catálogo de datos; estado operacional del scheduler en namespace separado.
- **Quack:** no participa salvo que el adaptador M13 remoto sea elegido expresamente.

---

## 22. API de producto y experiencia de usuario

Pantalla `Automation` (no necesita una marca concreta):

- **Schedules:** nombre, objetivo, cron visual, preview de próximas 5 ejecuciones en zona seleccionada, habilitar/pausar, SLA y última ejecución.
- **Reports:** destino, formato, filtros, vista previa con permisos del actor, enlace/correo, periodo y opción `wait_for_fresh_data`.
- **Alerts:** medida, umbral, ventana, desgloses, condición de recuperación, estado no-data y canales.
- **Runs:** id, trigger causal, revisión semántica, revisión de datos, step timings, retries, progreso, destinatarios, error y action `retry/cancel` autorizada.
- **Delivery Inbox:** confirmación, error del proveedor, silencios, opt-out, expiración de enlaces.

**UX mínima correcta:** una prueba de programación calcula las próximas ocurrencias sin guardar ni ejecutar; `send_test` se marca como prueba y no altera la ocurrencia productiva; ejecución manual tiene `initiated_by` y no modifica cron.

### 22.1 Endpoints ilustrativos, no contrato REST obligatorio

```text
POST   /api/automation/schedules
GET    /api/automation/schedules/{id}/preview?count=5
POST   /api/automation/schedules/{id}/run
POST   /api/automation/schedules/{id}/pause
GET    /api/automation/jobs?status=failed
GET    /api/automation/jobs/{job_id}
POST   /api/automation/jobs/{job_id}/retry
POST   /api/automation/jobs/{job_id}/cancel
GET    /api/automation/alerts/{id}/instances
POST   /api/automation/reports/{id}/test
```

Los nombres son ejemplos; el núcleo Rust no depende de REST, GraphQL o MCP. Todas las acciones invocan autorización servidor M11.

---

## 23. Observabilidad y SLO

### 23.1 Métricas operativas

- `schedule_lag_seconds`: actual - nominal; histograma por tipo.
- `job_queue_depth`, `job_wait_seconds`, `job_run_seconds`, `retry_count`.
- `job_lease_expired_total`, `job_dead_letter_total`, `schedule_missed_total`.
- `alert_evaluation_latency`, `alert_state_transition_total`.
- `report_query_seconds`, `report_render_seconds`, `artifact_bytes`.
- `delivery_attempts_total`, `delivery_success_total`, `delivery_latency_seconds`.
- `data_freshness_wait_seconds`, `missing_data_occurrences_total`.

**Cardinalidad:** no usar `job_id`, usuario, email o UUID de cada destinatario como etiqueta métrica de alta cardinalidad. IDs van a traces/logs estructurados.

### 23.2 Tracing

`trigger_event_id` → `occurrence_id` → `job_id` → `query_id` → `artifact_id` → `delivery_id`. Conservar causa y contexto de seguridad sin incluir tokens ni columnas sensibles.

### 23.3 SLO iniciales a fijar con benchmarks

No inventar porcentajes del producto antes de probar. Proponer objetivos por clase: trabajos críticos de cierre con ejecución/entrega rastreable, desfase máximo por programación, recuperación tras crash, verificación del porcentaje de envíos y presupuesto de memoria por worker. Publicar en operación real los percentiles medidos.

---

## 24. Casos de negocio para probar la semántica

### 24.1 Facturación diaria por remesa

- Una remesa se carga tarde; `DataPublished` se recibe tras cierre de calidad.
- El informe debe generar la remesa correcta, no la del reloj del servidor.
- Reprocesar la misma remesa no genera un segundo reporte salvo revisión/corrección configurada.
- Si la remesa se corrige, `revised_report` relaciona revisión nueva y anterior.

### 24.2 Cobranza de varias cohortes

- El reporte filtra fecha de pago, pero calcula deuda por cohortes de emisión.
- Un mismo pago puede registrarse después de la fecha de emisión.
- Se comparan snapshots del mismo corte lógico para evitar cifras inconsistentes.
- Las alertas de ratio se calculan mediante el Semantic Core, no un `SUM` local de widgets.

### 24.3 Dashboard ejecutivo automático

- Tres KPIs, una serie temporal y segmentadores de área.
- El PDF programado fija el estado de filtros y la revisión.
- Un error de gráfico no produce un PDF aparentemente válido por defecto.
- Entrega por link autenticado a destinatarios autorizados.

### 24.4 Multitenant

- Workspace A y B tienen horarios iguales; no comparten estado, UUID semántico, artefactos o emails.
- Tenant con muchas exportaciones no debe bloquear las alertas de otro; aplicar fairness.
- Los recibos de entrega no revelan direcciones de otros tenants.

---

## 25. Plan de implementación por verticales pequeñas

### Etapa 0 — Validación de contratos

- Alinear recursos M10, identidad M11, SemanticQuery M04, Query Serving M05 y snapshots M13.
- Definir cinco invariantes: ocurrencia única, lease con fencing, actor explícito, revisión de datos, entrega auditable.
- Pruebas con reloj inyectable y PostgreSQL real; evitar mocks que oculten race conditions.

### Etapa 1 — Scheduler durable mínimo

- `ScheduleDefinition`, cron IANA, preview, `JobLedger` PostgreSQL, worker Rust, lease, reaper, retries, cancelación.
- Ejecución manual y evento `DataPublished`.
- Demo: job que calcula KPI, guarda resultado y puede recuperarse tras crash.

### Etapa 2 — Alerting útil

- Alertas sobre medida única, una dimensión, estados normal/pending/firing/recovered/no-data, SQL semántico central.
- Outbox, email/webhook seguro, validación de destinatarios, cooldown y auditoría.
- Prueba de RLS por dos usuarios y dos tenants.

### Etapa 3 — Reporting de datos

- CSV y XLSX en streaming acotado, artefactos con retención, descarga firmada.
- Pruebas de precisión BigInt/Decimal/fechas; policy fingerprint.
- Separación de query/generate/deliver.

### Etapa 4 — Dashboard PDF/PNG

- Worker de captura aislado y headless opcional.
- Captura reproducible por DashboardRevision y snapshot.
- Presupuesto de rendering, fallback a error explícito y preview.

### Etapa 5 — Enterprise y resiliencia

- Routing avanzado, silences, escalaciones, destinatarios por políticas, consolidación de alertas.
- Multiworker con fairness, cuotas, SLO, recuperación tras particiones, cancelación cruzada.
- Comparar Apalis vs worker custom en benchmark antes de extraer un proceso/servicio independiente.

**No construir toda la etapa 5 por anticipado.**

---

## 26. Criterios de aceptación (90 pruebas propuestas)

### A. Scheduling, timezone y missed runs

1. Crear cron semanal válido y generar próximas cinco ocurrencias deterministas.
2. Cron inválido se rechaza y no se publica.
3. Zona IANA inválida se rechaza.
4. Preview muestra UTC y hora local, sin confundir offsets.
5. DST inexistente aplica `skip` sin duplicado.
6. DST inexistente aplica `shift_forward` cuando se configura.
7. DST ambiguo con `first` ejecuta una vez.
8. DST ambiguo con `both` genera dos ocurrencias diferentes.
9. Reinicio de proceso no pierde ocurrencia planificada.
10. `latest_only` no ejecuta todas las ventanas antiguas.
11. `bounded_catchup` respeta límite N.
12. Una ejecución manual no cambia la próxima ocurrencia cron.
13. Pausar schedule evita nuevas ocurrencias sin borrar el historial.
14. Reanudar no dispara backlog prohibido.
15. Editar definición conserva ejecuciones vinculadas a revisión previa.

### B. JobLedger, leases y errores

16. Dos workers reclaman un mismo job y solo uno obtiene el lease vigente.
17. `SKIP LOCKED` evita bloqueo global entre workers.
18. Job reclamado aparece con `fencing_token` incrementado.
19. Worker antiguo no puede confirmar con token obsoleto.
20. Lease vence y job recupera estado ejecutable según política.
21. Worker cae antes de `start` y job puede reintentarse.
22. Worker cae durante consulta y estado queda auditable.
23. Worker cae después de finalizar cálculo y antes de confirmar; idempotencia evita duplicar artefacto lógico.
24. `retry_wait` no se ejecuta antes de `not_before`.
25. Jitter evita todos los reintentos simultáneos.
26. Error permanente termina en failed/dead-letter sin bucle infinito.
27. Cancelación autorizada detiene trabajo y no reintenta.
28. Cancelación no autorizada devuelve Forbidden.
29. El orden del trabajo prioriza SLA sin starvation prolongada.
30. Límite de concurrencia por tenant se respeta.
31. `forbid_overlap` bloquea segunda ejecución en curso.
32. `queue` conserva la segunda para más adelante.
33. `replace` no se ofrece a jobs sin cancelación segura.
34. Reaper no recupera lease válido por error de reloj del cliente.
35. Cambios concurrentes de estado no borran logs ni finalizaciones.

### C. Data freshness y snapshots

36. `DataPublished` tras commit dispara exactamente una ocurrencia lógica.
37. Duplicado del mismo evento no duplica job.
38. Evento de publicación parcial no dispara reporte.
39. Modelo actualizado sin calidad aprobada bloquea reporte.
40. Reporte espera dato fresco hasta timeout definido.
41. Timeout de frescura no envía PDF con cifras viejas por defecto.
42. Varias revisiones publicadas rápidamente obedecen política de coalesce.
43. Corrige mes histórico sin confundirlo con mes actual.
44. Dos paneles del mismo PDF fijan snapshots compatibles.
45. Referencia a snapshot expirado genera error explícito y auditado.
46. Se conserva `semantic_revision` en artefacto.
47. Estado BI PostgreSQL y estado DuckLake tienen reconciliación ante fallo parcial.

### D. Alert Engine

48. Alerta decimal evita comparación con `float` que altere umbral.
49. `COUNT(DISTINCT)` proviene de M04, no del frontend.
50. Estado pending necesita N evaluaciones válidas consecutivas.
51. Firing se produce solo tras cumplir condición.
52. Histeresis evita oscilaciones rápidas.
53. Estado resuelto se notifica bajo política.
54. `no_data` no se interpreta como cero.
55. `stale_data` no se interpreta como recovered.
56. `evaluation_error` es visible como fallo operacional.
57. Dos `area_id` generan estados separados.
58. Datos corregidos de una ventana pasada no envían duplicados salvo política.
59. Silence bloquea notificaciones, pero no oculta evaluación.
60. Agrupación no mezcla alertas de tenants diferentes.
61. Rate-limit evita tormenta de correos.
62. RLS se aplica incluso en consultas temporales de evaluación.

### E. Reporting y formato

63. CSV grande se emite por chunks sin materialización total en memoria.
64. CSV escapa valores susceptibles de formula injection.
65. XLSX conserva como texto los valores numéricos que exceden precisión Excel.
66. `DECIMAL(18,2)` no pasa silenciosamente a `f64`.
67. UUID y códigos como `"001"` conservan ceros iniciales.
68. DATE mantiene fecha civil correcta en distintas zonas.
69. Archivo Parquet conserva esquema y tipos esperados.
70. PDF falla si panel obligatorio no renderizó.
71. PDF utiliza revisión publicada no draft mutable.
72. Worker de PDF no puede acceder a host privado arbitrario.
73. Artifacts tienen TTL y se limpian sin borrar referencia de auditoría.
74. Enlace firmado expira y puede revocarse.
75. Artefacto no se distribuye hasta completar validación.

### F. Delivery, security y observabilidad

76. Retry SMTP no vuelve a ejecutar SQL de reporte ya generado.
77. Outbox mantiene intento por destinatario.
78. Provider error 429 aplica `Retry-After` o política backoff.
79. Webhook a `127.0.0.1` se rechaza.
80. Webhook a dirección privada alcanzable tras redirect se rechaza.
81. Prueba DNS rebinding bloquea conexión privada resultante.
82. Contraseña en URL no aparece en logs.
83. Un actor sin permiso no puede programar entrega.
84. Revocación de permiso antes de delivery bloquea envío.
85. Artefactos de dos receptores con RLS diferente no se reutilizan por error.
86. Envío duplicado por fallo SMTP se registra como entrega incierta cuando procede; no se promete exactly-once.
87. Logs conectan evento→job→query→artifact→delivery.
88. Métricas OTel no incorporan email como etiqueta de alta cardinalidad.
89. Un tenant saturado no impide indefinidamente tareas críticas de otro.
90. `send_test` no modifica el estado de producción del schedule.

**Pruebas obligatorias reales:** PostgreSQL bajo dos o más workers, pérdida de conexión durante lease, DuckLake y DuckDB con snapshots compatibles, y simulación de error de correo. Los tests unitarios de cron no prueban consistencia distribuida.

---

## 27. Preguntas cerradas, abiertas y criterios de elección

### Decisiones cerradas en diseño

- M14 es **una capa propia**: no se mezclará con Query Serving ni con Model Engine.
- La programación usará zona IANA y ocurrencias lógicas versionadas.
- PostgreSQL será el backend inicial del JobLedger y Outbox; sin SQLite.
- Backend y workers Rust; sin Celery ni Redis obligatorio.
- Se distinguen evaluar alerta, generar artefacto y notificar destinatario.
- `DataPublished` es un trigger de primera clase, al lado de cron/manual.
- Se reutiliza M04/M05 para datos y M11 para permisos; ningún scheduler tiene privilegios semánticos especiales.
- Quack no es requisito; DuckDB embebido del M13 sigue siendo el ejecutor inicial.
- Exactly-once se limita a propiedades transaccionales internas; efectos externos serán idempotentes/deduplicables según proveedor.

### Decisiones abiertas que dependen de pruebas

1. ¿Apalis-postgres o worker SQLx propio? Comparar crash/recovery, cancelación, carga y madurez antes de escoger.
2. ¿Postgres esquema `automation` junto con metadatos BI o base operacional separada? Evaluar backup, límites de pools y aislamiento.
3. ¿Qué política predeterminada de PDF y tamaño de adjuntos? Depende del despliegue y seguridad corporativa.
4. ¿Qué servicio de correo y chat se prioriza? Depende de destinatarios reales.
5. ¿Qué backend de artefactos? Disco local en single-node, objeto S3-compatible en multi-node.
6. ¿Cómo se implementa `run_as_recipient` en un reporte que contiene múltiples fuentes? Requiere verificación del Security Core.
7. ¿Cuál es el intervalo máximo y mínimo permitido por tenant? Validar SLO y coste.
8. ¿Puede revocarse token de artefacto después de descarga? Solo para descargas futuras; no se controla copia externa.
9. ¿Qué data-quality gates habilitan cierre/periodo? Se definen con M02/M03 y responsables del dato.
10. ¿Requeriremos alertas con dependencia entre reglas? Evitar hasta que exista caso de uso y semántica de ciclos.

---

## 28. Migración conceptual de M01–M13 a esta arquitectura

- **M02 (Models):** emite `BuildCompleted`/`BuildFailed`; no incorpora un scheduler de notificaciones propio.
- **M03 (Storage):** comunica `DatasetPublished` con revisión y watermark; no es autoridad del correo.
- **M04 (Semántica):** recibe contexto actor de job y produce resultado tipado.
- **M05 (Queries):** comparte presupuestos de ejecución entre jobs e interacciones, con prioridades explícitas.
- **M06 (Explore):** mantiene búsquedas interactivas; puede crear una `AlertDefinition` desde una medida autorizada.
- **M07 (Pivot):** exportación respeta cálculo correcto de subtotales.
- **M08 (Canvas):** entrega `DashboardRevision`/layout y binding de filtros.
- **M09 (Visualización):** entrega renderer/adapter; no inicia jobs por su cuenta.
- **M10 (Metadatos):** es propietario de definiciones y revisiones, no del histórico operativo de cada intento.
- **M11 (Seguridad):** evalúa actor, receptor y permisos de descarga.
- **M12 (APIs):** expone control del scheduler sin generar SQL alternativo.
- **M13 (Runtime):** presta motor SQL/Arrow, DuckLake/Parquet; no almacena `JobLedger` en `.duckdb`.

La orientación de módulos M01–M09 hacia la antigua marca SQLVIZ no condiciona estas interfaces. El cierre del proyecto documental requerirá normalizar nombres y contratos de toda la serie, como ya se acordó.

---

## 29. Propuesta de estructura física: monolito modular, no colección de microservicios

```text
crates/
  bi-automation-contracts/       # IDs, errores, definiciones, eventos y capabilities
  bi-automation-core/            # trigger routing, cron planner, alert state machine
  bi-automation-postgres/        # job ledger, leases, outbox, migrations
  bi-reporting/                  # builders, validators, artifact lifecycle
  bi-delivery/                   # SMTP/webhook/chat adapters, routing, retry
apps/
  bi-server/                     # REST/gRPC/SDK-facing API; puede alojar worker local
  bi-worker/                     # mismo dominio, pools y plugins según deployment
  bi-scheduler/                  # opcional separación de despliegue, no producto
packages/
  studio/                        # UI Svelte 5 para schedules/alerts/reports/runs
```

**No crear todos los crates desde el día 1**: se puede iniciar un crate `bi-automation` con módulos internos y extraer adaptadores cuando su tamaño o pruebas lo justifiquen. Preservar interfaces y tests de contrato primero.

---

## 30. Fuentes primarias y enlaces para auditoría reproducible

### Rill (tag inmutable v0.90.4)

1. [AlertReconciler — `runtime/reconcilers/alert.go`](https://github.com/rilldata/rill/blob/v0.90.4/runtime/reconcilers/alert.go)
2. [ReportReconciler — `runtime/reconcilers/report.go`](https://github.com/rilldata/rill/blob/v0.90.4/runtime/reconcilers/report.go)
3. [Reconciler package/controller contract](https://pkg.go.dev/github.com/rilldata/rill%40v0.90.4/runtime/reconcilers)

### Lightdash (`main`, fijar commit antes de implementación)

4. [SchedulerService — validación, targets, permisos y jobs](https://github.com/lightdash/lightdash/blob/main/packages/backend/src/services/SchedulerService/SchedulerService.ts)
5. [Incidente de SSRF en webhook de scheduled deliveries, con estado de corrección](https://github.com/lightdash/lightdash/issues/24389)
6. [Discusión sobre adjuntos cifrados en programaciones](https://github.com/lightdash/lightdash/issues/24550)

### Grafana y Alertmanager (`main`)

7. [Grafana alert scheduler — `schedule.go`](https://github.com/grafana/grafana/blob/main/pkg/services/ngalert/schedule/schedule.go)
8. [Grafana state manager — `manager.go`](https://github.com/grafana/grafana/blob/main/pkg/services/ngalert/state/manager.go)
9. [Grafana — diseño de notificaciones](https://github.com/grafana/grafana/blob/main/docs/sources/alerting/fundamentals/notifications/_index.md)
10. [Prometheus Alertmanager — grouping, silences, deduplication](https://github.com/prometheus/alertmanager/blob/main/docs/alertmanager.md)

### Superset (`master`)

11. [Superset scheduler tasks](https://github.com/apache/superset/blob/master/superset/tasks/scheduler.py)
12. [Superset — Alerts and Reports](https://github.com/apache/superset/blob/master/docs/admin_docs/configuration/alerts-reports.mdx)

### Rust workers y Postgres

13. [Apalis — Rust background jobs](https://github.com/apalis-dev/apalis)
14. [Apalis PostgreSQL backend](https://github.com/apalis-dev/apalis-postgres)
15. [Apalis PGMQ adapter](https://github.com/apalis-dev/apalis-pgmq)

### Referencias internas de la serie

16. `BI_Modulo_10_Metadata_Architecture_Agnostica_2026-10-10.md`
17. `BI_Modulo_11_Identity_Security_Governance_Agnostico_2026-10-10.md`
18. `BI_Modulo_12_APIs_Integraciones_Ecosistema_Agnostico_2026-10-10.md`
19. `BI_Modulo_13_Runtime_Analitico_Rust_DuckDB_DuckLake_PostgreSQL_Polars_ADBC_2026-10-10.md`
20. Módulos 01–09: contexto conceptual en proceso de normalización hacia plataforma BI agnóstica.

---

## 31. Criterio de cierre y decisión final

**Se considera definido este módulo cuando** el diseño permite que una métrica se evalúe y se reporte automáticamente con actor autorizado, revisión consistente de datos, programación IANA reproducible y persistencia capaz de recuperarse de un fallo de proceso, y cuando la notificación puede reintentarse sin rehacer el cálculo de datos.

**Implementación mínima demostrable:** un schedule diario y otro disparado por `DataPublished`; alerta con estado `pending→firing→resolved`, un reporte CSV generado desde consulta semántica, entrega por correo/webhook mediante outbox; PostgreSQL real; dos workers; pruebas de lease, permisos y duplicados.

**Síntesis:** la opción moderna y proporcional es **automatización propia en Rust con contratos claros, PostgreSQL como ledger durable, eventos de publicación, políticas de alertado inspiradas en Grafana y reportes inspirados en Rill/Lightdash/Superset**. Apalis merece una prueba seria como backend de workers; Temporal/Restate no se justifican por su sola novedad. DuckDB, DuckLake y Polars siguen en la capa de datos, no reemplazan al scheduler.

**Este documento cubre solo el Módulo 14.** El siguiente informe se iniciará exclusivamente tras autorización del usuario. Al cerrar la serie, las catorce entregas y sus revisiones de coherencia se empaquetarán en un ZIP común.
