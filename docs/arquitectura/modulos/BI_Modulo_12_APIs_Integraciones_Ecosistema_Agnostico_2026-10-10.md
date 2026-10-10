# Plataforma BI abierta — Módulo 12: APIs, integraciones y ecosistema
## Auditoría de implementaciones reales, arquitectura de contratos, protocolos, SDKs, extensiones y MCP

**Fecha de referencia:** 10 de octubre de 2026  
**Versión de este informe:** propuesta arquitectónica 1.0  
**Estado:** investigación y diseño; **no** corresponde a código implementado ni a pruebas ejecutadas contra la futura plataforma.  
**Dependencias conceptuales:** módulos 01–11 (sources, models, storage, semántica, serving, experiencias, catálogo e identidad); estos módulos están en proceso de normalización a una plataforma nueva y agnóstica.  
**Fuera de alcance:** implementar un motor SQL, reescribir la semántica, escoger una marca para el producto, obligar a usar `.sqlviz`, definir UI Canvas, desarrollar todos los drivers o convertir a MCP en el motor de negocio.  
**Fuentes:** archivos de implementación abiertos de Rill `v0.90.4`, Cube `master`, Lightdash `main`, Superset `master`, Grafana `main`; especificaciones OpenAPI 3.1.1, AsyncAPI 3.0, Arrow Flight SQL y MCP **2026-07-28**. Las ramas variables no están fijadas por SHA y pueden cambiar; revisar los enlaces en un commit antes de implementar. Se revisó código publicado y especificaciones, no se compiló ni probó cada producto.

---

## 0. Dictamen ejecutivo

La arquitectura recomendada no es «una REST API que expone SQL» sino una **plataforma API-first de recursos y consultas semánticas**, con un único contrato lógico para que web, SDKs, BI externo, agentes y embebidos produzcan **exactamente las mismas métricas con idéntica autorización y versión de datos**.

**Decisiones:**

1. Distinguir **Management API** (definiciones, versiones, publicaciones, permisos) de **Analytics API** (catálogo de medidas, consultas, resultados, exportaciones).
2. Concentrar validación semántica, autorización y ejecución en servicios internos existentes; HTTP/GraphQL/SQL/MCP serán **adaptadores de borde**, no motores analíticos paralelos.
3. Adoptar inicialmente **HTTP JSON + OpenAPI 3.1.1**, consultas síncronas pequeñas y un **Job API** para operaciones pesadas. Webhooks/eventos vendrán después.
4. Crear un **Query Contract** con medidas, dimensiones, filtros tipados, tiempo, paginación, snapshot, política de frescura y límites; prohibir que el frontend o los SDKs construyan SQL de seguridad.
5. Para grandes resultados, habilitar transporte columnar **Arrow IPC** por una ruta separada, sujeto a validación de implementación; **Arrow Flight SQL** se estudiará como adaptador posterior, no requisito central.
6. Permitir **GraphQL** opcional basado en la misma Query API, pero no crear un segundo planificador semántico.
7. Ofrecer en una fase avanzada un endpoint **SQL semántico de solo lectura**, y valorar PostgreSQL wire / Flight SQL con pruebas de drivers reales. Una API SQL no debe ser una puerta trasera al DuckDB servidor.
8. Diseñar MCP con el **protocolo vigente `2026-07-28`** y las capacidades de lectura seguras como primer alcance; escritura y publicación requieren aprobación, ámbito y permisos explícitos.
9. Publicar **SDKs tipados** generados y curados desde el contrato público; admitir TypeScript y Python primero, otros según demanda real.
10. Una **Extension Capability API** mantiene plugins opcionales aislados, registrados con versionado, permisos y límites de recursos, nunca código arbitrario cargado en el núcleo sin control.

**No hacer:** microservicios obligatorios, REST CRUD de tablas físicas, credenciales de origen en las respuestas, GraphQL/SQL/REST implementando su propio RLS, SQL arbitrario sin gobierno, cache cruzada entre actores ni MCP con permisos de administrador por defecto.

---

## 1. Método y grado de evidencia

Se distinguen tres categorías durante todo el documento:

- **[CÓDIGO]**: comportamiento visible en un archivo de repositorio; leer enlaces en §29.
- **[ESTÁNDAR]**: afirmaciones contenidas en la especificación formal publicada.
- **[PROPUESTA]**: decisión de arquitectura para nuestra futura plataforma, todavía no implementada.

No afirmar que dos versiones del mismo proyecto son equivalentes. Rill queda fijado en `v0.90.4`; Lightdash, Cube, Grafana y Superset tienen referencias de ramas mutables. Algunos subsistemas de estos productos pueden tener capacidades de pago, despliegue o edición distintas. El análisis no atribuye funcionalidades enterprise al núcleo open source sin verificar licencia y código exactos.

Una característica documentada no equivale a garantía universal de seguridad, escalabilidad o compatibilidad del driver. La certificación exige pruebas end-to-end.

---

## 2. Límites de la capa: entrada y salida

| Dueño | Entrada/salida | Esta capa **sí** hace | Esta capa **no** hace |
|---|---|---|---|
| Metadata Core | ResourceRef, revision, Draft, Published | CRUD autorizado, ETags y serialización | Ser la autoridad adicional de metadatos |
| Semantic Core | SemanticQuery, SemanticPlan | Validar/adaptar formatos de request | Calcular una medida con lógica distinta |
| Query Serving | QueryRequest, Job, Stream | Manejar HTTP/stream y errores | Reejecutar físicamente SQL en el gateway |
| Identity/Policy | Principal, scope, PolicyDecision | Verificar scopes, delegación, cuotas | Definir RLS por un header confiado al cliente |
| API Layer | schemas, idempotencia, rutas | Entradas contractuales y SDKs | Guardar dimensiones o importes en el router |
| Events | DomainEvent, cursor | Publicar/suscribir de forma gobernada | Ser base transaccional de todos los metadatos |
| MCP Adapter | tools/resources, outputSchema | Exponer operaciones permitidas | Prometer que una LLM sustituye al Semantic Core |
| External SQL | SQL subset, metadata | Traducir a consultas semánticas autorizadas | Permitir DDL/DML arbitrario o bypass de políticas |

**Principio:** todos los caminos externos convergen en el mismo `Application Service` y atraviesan `Authorization` y `Semantic Query` del servidor.

---

## 3. Investigación del código: Rill — APIs como recursos gobernados

**[CÓDIGO]** [`runtime/reconcilers/api.go`](https://github.com/rilldata/rill/blob/v0.90.4/runtime/reconcilers/api.go) registra un reconciliador para `ResourceKindAPI`, asigna `Spec` y `State` y participa en control de acceso transitivo. Es **importante**: el `Reconcile` observado contiene un `TODO` para validar las propiedades del resolver, por lo que no debemos afirmar que ese reconciliador comprueba exhaustivamente cualquier definición de API.

**[DOCS]** [Metrics SQL APIs](https://docs.rilldata.com/developers/build/custom-apis/metrics-sql) presenta consultas basadas en definiciones de Metrics Views en vez de tablas físicas. La [referencia de recursos API](https://docs.rilldata.com/reference/project-files/apis) documenta `metrics_sql`, invocación de otras APIs y acceso a archivos/globs. La documentación puede adelantarse o diferir de la versión del código fijado.

**Qué rescatar:**

1. API como recurso declarativo, versionable y sujeto al catálogo.
2. Consultas por nombres semánticos conocidos en lugar de SQL físico.
3. Políticas heredadas de las definiciones semánticas.
4. Integración del recurso API con ciclo de vida y reconciliación de recursos.

**Qué mejorar:**

- Validar el AST de la consulta, esquema de entrada, tipos, costes y permisos antes de publicar.
- Separar consultas predefinidas por autor del catálogo de consultas ad hoc autorizado para usuarios.
- Publicar su `openapi`/schema de parámetros y result schema con revisión inmutable.
- Tests de no regresión: un recurso API inválido no desplaza la última versión publicada.
- Evitar que un API «compuesto» pueda invocar sub-APIs con más privilegios que el actor.

**No copiar:** la sintaxis `metrics_sql` como lenguaje universal propio. Puede ser un adaptador opcional; el contrato tipado es la fuente de autoridad.

---

## 4. Investigación del código: Cube — múltiples protocolos, un gateway semántico

**[CÓDIGO]** [`packages/cubejs-api-gateway/src/graphql.ts`](https://github.com/cube-js/cube/blob/master/packages/cubejs-api-gateway/src/graphql.ts) crea esquema GraphQL usando metadata y Nexus; el resolver convierte la selección GraphQL a una consulta, llama `apiGateway.load({query, context, apiType:'graphql'})` y adapta el resultado. [`gateway.ts`](https://github.com/cube-js/cube/blob/master/packages/cubejs-api-gateway/src/gateway.ts) concentra la interfaz principal del API Gateway y sus rutas de consulta.

**Qué rescatar:**

- Superficie GraphQL construida desde el catálogo de campos.
- Contexto de request transferido hasta la ejecución.
- Distinción entre metadatos, carga/consulta, plan y resultado.
- Posibilidad de servir varios clientes sin exportar necesariamente tablas físicas.

**Qué mejorar:**

- Definir **un solo DTO de consulta** para toda superficie. GraphQL debe compilarse a él y no al SQL físico.
- Preservar `DECIMAL` y `BIGINT` en el contrato; jamás confiar en que todos los números llegan como `float` JSON.
- El `schema` GraphQL debe ser estable dentro de la revisión y limitado por permisos del actor; introspection no debe revelar campos restringidos.
- Establecer presupuesto de complejidad para profundidad, alias, introspection, cardinalidades y consultas ad hoc.
- No vender «SQL API universal» si faltan capacidades del protocolo, funciones o metadatos JDBC.

**Nota:** la presencia de una API no implica que se soporte automáticamente cualquier dialecto SQL. La extensión SQL se certifica por subconjunto y cliente.

---

## 5. Investigación del código: Lightdash — controladores tipados y OpenAPI generado

**[CÓDIGO]** [`savedChartController.ts`](https://github.com/lightdash/lightdash/blob/main/packages/backend/src/controllers/savedChartController.ts) es un controlador TypeScript del backend. Las guías oficiales del repositorio [CONTRIBUTING.md](https://github.com/lightdash/lightdash/blob/main/.github/CONTRIBUTING.md) y [backend-review.md](https://github.com/lightdash/lightdash/blob/main/.github/rules/backend-review.md) establecen `controllers → services → models → database`, uso de `tsoa` para contrato/rutas y generación de `routes.ts` y `swagger.json`.

**Rescatar:** separación controladores/servicios; tipos compartidos; OpenAPI generado y comprobado en CI; documentación y rutas alineadas; controles de acceso por servicio.

**Mejorar:**

- Contratos **schema-first** o generación controlada: aceptar una sola autoridad y verificar equivalencia entre SDK, OpenAPI y servidor.
- Evitar que endpoints surgidos de necesidades específicas del frontend formen accidentalmente un contrato público perpetuo.
- Tener rutas separadas `internal` versus `public`, con soporte/deprecación explícitos.
- Modelar queries/jobs/results como recursos de primer nivel, no dispersos entre controladores de gráficos y dashboards.
- Las políticas de tokens de servicio deben usar la misma fuente de permisos que los usuarios, evitando divergencia de scopes.

**Riesgo real documentado:** el texto del repositorio sobre cuentas de servicio advierte deriva de permisos entre scopes legados y roles del sistema; esto motiva nuestras pruebas de paridad de autorización ([docs/service-accounts.md](https://github.com/lightdash/lightdash/blob/main/docs/service-accounts.md)). No se afirma una vulnerabilidad universal de todas las versiones.

---

## 6. Investigación del código: Apache Superset — recursos, ChartData y jobs

**[CÓDIGO]** [`superset/dashboards/api.py`](https://github.com/apache/superset/blob/master/superset/dashboards/api.py) expone gestión de dashboards mediante API y comandos de dominio; [`superset/charts/data/api.py`](https://github.com/apache/superset/blob/master/superset/charts/data/api.py) implementa la API de datos con `@protect()`, validación de `ChartDataCommand`, procesamiento de filtros de dashboard y respuesta asíncrona `202` cuando corresponde. Se constatan rutas con formato `/data` y `/<int:pk>/data/` en ese código. La [referencia oficial de REST](https://superset.apache.org/developer-docs/api/) utiliza especificación OpenAPI.

**Qué rescatar:**

- API de gestión y API de ejecución de datos separadas.
- Validación de consulta antes de ejecutar.
- Diferencia entre respuesta de datos inmediata y trabajo asíncrono.
- Comandos de dominio y permisos en rutas de operaciones.

**Qué mejorar:**

- No asociar la consulta semántica obligatoriamente a un `chart_id`; otras apps deben poder usar las métricas sin tener un gráfico guardado.
- Distinguir filtros guardados/valores de sesión de forma consistente.
- Exponer `QueryResult` neutral; la exportación CSV/XLSX/Parquet es un derivado autorizado de ese resultado.
- Un job de descarga tiene la misma identidad, política y revisión que la consulta original.

---

## 7. Investigación del código: Grafana — migración de APIs y versionado explícito

**[CÓDIGO]** [`pkg/api/api.go`](https://github.com/grafana/grafana/blob/main/pkg/api/api.go) muestra registro de rutas, middlewares, cuotas y evaluadores de permisos. La [documentación de HTTP API](https://grafana.com/docs/grafana/latest/developer-resources/api-reference/http-api/) confirma una transición de rutas legadas `/api` hacia `/apis`, con recursos de estructura y versión formalizadas. La [Dashboard API](https://grafana.com/docs/grafana/latest/developer-resources/api-reference/http-api/dashboard/) emplea namespaces y operaciones CRUD.

**Rescatar:**

- Namespaces/identidades estables y permisos como middleware más comprobación de dominio.
- Versionado de recursos y declaración de endpoints.
- Desarrollo gradual de APIs nuevas coexistiendo con legadas sin romper consumidores.
- Catálogo de plugins e integración con recursos existentes.

**Mejorar:**

- Elegir un formato uniforme y sencillo para consumidores (`/v1/workspaces/...`) antes de forzar compatibilidad Kubernetes-style.
- No multiplicar rutas duplicadas solo por necesidades del frontend.
- ETags y errores contractuales comunes, independiente del recurso.
- Compatibilidad de SDK medida por test, no deducida del número de versión.

---

## 8. Matriz comparativa de referentes

| Necesidad | Rill | Cube | Lightdash | Superset | Grafana | Diseño nuevo |
|---|---|---|---|---|---|---|
| Consultas semánticas | Metrics View API | Gateway/GraphQL | Queries de proyecto | Dataset/ChartData | Según datasource | **Core único** |
| APIs declarativas | API resource | Esquema/cubos | Controllers | REST model | Resource API | Resources versionados |
| SDK externo | API de producto | Client packages | API generable | OpenAPI | SDK/API docs | SDKs generados + tests |
| SQL externo | No asumir JDBC pleno | SQL API | Warehouse SQL | SQL Lab distinto | Datasource | Opcional, controlado |
| GraphQL | No requisito | Implementación vista | No requisito | No requisito | No requisito | Adaptador posterior |
| API jobs | Según operación | Orchestrator | Backend jobs | Async ChartData | Tareas según API | Job contract genérico |
| Extensiones | Conectores/producto | Drivers | Integración proyecto | Extensiones | Plugins | Capability registry |
| Acceso agentes | API/MCP según versión | API usable por agentes | API | API | API | MCP read-first |

**Interpretación:** ningún producto domina todas las capacidades; tampoco sería eficiente clonar todos sus protocolos simultáneamente.

---

## 9. Contratos fundamentales: separación Command / Query

**[PROPUESTA]** Tipos canónicos:

```rust
pub struct ActorContext {
    pub principal_id: PrincipalId,
    pub workspace_id: WorkspaceId,
    pub delegated_scopes: Vec<Scope>,
    pub policy_revision: RevisionId,
    pub correlation_id: RequestId,
}

pub struct QueryEnvelope {
    pub request: SemanticQuery,
    pub semantic_revision: RevisionSelector,
    pub snapshot_selector: SnapshotSelector,
    pub requested_format: ResultFormat,
    pub constraints: QueryConstraints,
}

pub struct QueryResultManifest {
    pub query_id: QueryId,
    pub schema: Vec<ColumnDescriptor>,
    pub semantic_revision: RevisionId,
    pub data_revision: DataRevision,
    pub policy_fingerprint: PolicyFingerprint,
    pub row_count: Option<u64>,
    pub truncation: TruncationState,
    pub next_cursor: Option<OpaqueCursor>,
    pub expiry: Option<Timestamp>,
}
```

El `ActorContext` nunca se toma literalmente de JSON entrante: lo fabrica el middleware de identidad. El fingerprint interno de política no se expone como token verificable por el cliente. Los tipos son **ilustrativos**, no una API Rust compilada ni fijación de crates.

**Management commands:** `CreateDraft`, `PatchDefinition`, `ValidateRevision`, `Publish`, `Rollback`, `Archive`, `ManagePolicy`.

**Analytics queries:** `DescribeSemanticModel`, `ListMetrics`, `QueryMetrics`, `ExplainSemanticQuery`, `PreviewQuery`, `ListAllowedValues`, `GetQueryStatus`, `FetchResult`, `ExportResult`.

Todos registran la revisión semántica y la identidad autorizada. `Publish` es operación explícita con controles más fuertes que `SaveDraft`.

---

## 10. Modelo de rutas públicas — propuesta inicial

| Verbo | Ruta | Contrato |
|---|---|---|
| GET | `/api/v1/capabilities` | Versiones, formatos y límites de API |
| GET | `/api/v1/workspaces/{id}/semantic-models` | Descubrir modelos permitidos |
| GET | `/api/v1/semantic-models/{id}/metrics` | Catálogo de medidas autorizado |
| POST | `/api/v1/queries` | Ejecutar semántica o crear job |
| GET | `/api/v1/queries/{id}` | Estado y resumen autorizado |
| GET | `/api/v1/queries/{id}/results` | Descargar resultados con formatos negociados |
| POST | `/api/v1/queries/{id}:cancel` | Cancelación idempotente |
| POST | `/api/v1/exports` | Exportación asíncrona gobernada |
| POST | `/api/v1/resources/{kind}` | Draft de recurso versionado |
| GET | `/api/v1/resources/{kind}/{id}` | Definición/revisión visible |
| POST | `/api/v1/resources/{kind}/{id}:validate` | Diagnósticos sin publicar |
| POST | `/api/v1/resources/{kind}/{id}:publish` | Commit publicado con `If-Match` |
| GET | `/api/v1/resources/{kind}/{id}/revisions` | Historial autorizado |
| GET | `/api/v1/jobs/{id}` | Estado trabajo largo |
| GET | `/api/v1/events` | Suscripción autenticada posterior |

Las rutas son **nuestra propuesta**, no rutas verificadas en Rill/Cube ni endpoints ejecutables.

**Criterio de diseño:** usar path IDs estables, nombres descriptivos y verbos de dominio explícitos para acciones no CRUD. Evitar que `POST /queries` publique modelos o que `GET` modifique el estado.

---

## 11. Ejemplo canónico de Semantic Query sobre HTTP

```http
POST /api/v1/queries
Content-Type: application/json
Authorization: Bearer <token>
Accept: application/json
Idempotency-Key: 92f33c5c-...  # opcional según contrato
```

```json
{
  "query": {
    "semantic_model": "sales",
    "measures": ["amount_billed", "unique_customers"],
    "dimensions": ["billing_month", "category"],
    "filters": {
      "and": [
        {"field": "area", "op": "eq", "value": {"type": "string", "value": "101"}},
        {"field": "billing_month", "op": "between", "values": [
          {"type": "date", "value": "2026-01-01"},
          {"type": "date", "value": "2026-09-01"}
        ]}
      ]
    },
    "order": [{"field": "billing_month", "direction": "asc"}]
  },
  "consistency": {"semantic_revision": "published", "data": "latest_committed"},
  "limits": {"max_rows": 10000, "max_wait_ms": 3000}
}
```

**Claves de seguridad:** el JSON no acepta `user_id`, `allowed_areas`, `tenant_filter_sql` ni `bypass_rls`. La restricción de áreas proviene de la política del actor. La cadena `"101"` conserva el tipo de código y los ceros iniciales cuando existan. `amount_billed` es nombre semántico estable, no SQL dinámico libre.

**Posibles respuestas:** `200` con resultado acotado; `202` con `query_id` para job; `4xx` por validación/permiso; `5xx` por fallas de infraestructura. El servidor no cambia a asíncrono silenciosamente sin mantener la forma contractual documentada.

---

## 12. Tipos, fechas, nulos y unidades: la API no puede destruir la semántica

| Tipo lógico | JSON público | Columnar | Política |
|---|---|---|---|
| `DECIMAL(p,s)` | String canónico con type metadata | Decimal Arrow si es representable | Nunca `float` implícito |
| `INT64/UINT64` | String o número dentro de rango seguro declarado | Tipo entero Arrow | Evitar redondeo JS |
| UUID | String UUID + metadata | Fixed/binary o string | No convertir a decimal |
| Código de cliente | String | String | Preservar `00123` |
| `DATE` | `YYYY-MM-DD` | Date32/Date64 según contrato | Sin conversión TZ |
| `TIMESTAMP` | ISO con offset/UTC explícito | Timestamp con timezone si procede | Separar evento y zona local |
| `NULL` | JSON null | Null bitmap | Nunca `"null"` |
| `BINARY` | Base64 solo si imprescindible | Binary | Límite de payload |
| Categoría/Enum | String + value mapping | Dictionary si aporta | Distinción entre id y label |

Cada columna de resultado incluye `field_id`, nombre, tipo lógico, unidad, formato sugerido, agregación, política de nulos y sensibilidad autorizada. **Formatear moneda** es responsabilidad del consumidor visual; no reemplazar valores DECIMAL por strings `$12,34` sin valor numérico canónico.

---

## 13. Sync, async, streaming y paginación

**A. Consultas pequeñas.** Límite de filas y coste; `200` con `schema`, `data`, `metadata`.

**B. Consultas largas.** `202 Accepted` + `Location` al job; estado `queued/running/succeeded/failed/canceled/expired`, `progress` aproximado, resultados al final. Jobs idempotentes por principal/operación/cuerpo y clave, con TTL.

**C. Streaming.** JSON-lines o Arrow IPC sobre un endpoint de stream, tras autorización y control de backpressure. No usar HTTP Chunked como sustituto automático de cancelación/resumibilidad.

**D. Pagination.** Cursor firmado/opaque que liga identidad autorizada, hash de filtro, orden estable, revisión semántica, data snapshot, página/límite y expiración. Offset solo para pequeños catálogos; para grandes datasets usar paginación estable, o exportación completa por job.

**E. Cancelación.** Propagar cancel al coordinador de consultas y al adaptador de motor cuando soporte cancelación real; si el motor continúa ejecutando, reflejar estado de cancelación solicitada, no éxito falso.

**F. Descarga.** CSV/Parquet/Arrow con manifiesto de autorización. URLs temporales firmadas deben estar restringidas a objeto/actor/expiración y no volverse públicas mediante redireccionamientos.

---

## 14. Caché, revisión y reproducibilidad a través de la API

`query_fingerprint` deberá cubrir estructura canónica del query, revisiones semánticas, esquema, parámetros tipados, identidad/política, dato/snapshot, zona/periodos y capacidades que modifiquen resultado. **No** usar únicamente el texto SQL ni solo `metric_name`.

En respuestas informar cuando corresponda:

```json
{
  "query_id": "q_...",
  "semantic_revision": "rev_...",
  "data_revision": "snapshot_...",
  "cache": {"status": "hit", "fresh_as_of": "2026-10-10T10:00:00Z"},
  "warnings": [],
  "truncated": false
}
```

`ETag` para definiciones; `If-Match` en publicaciones; `If-None-Match` opcional en GET seguros. `Cache-Control` estricto con datos privados; no ubicar payloads confidenciales en CDN compartida. Un preview con permisos superiores nunca se comparte vía el identificador de query con usuarios menos privilegiados.

---

## 15. OpenAPI-first y estándares HTTP

**[ESTÁNDAR]** OpenAPI [3.1.1](https://spec.openapis.org/oas/v3.1.1.html) describe de modo independiente del lenguaje las operaciones HTTP, esquemas y seguridad.

**[PROPUESTA]** Mantener un archivo fuente canónico de especificación OpenAPI, con:

- `operationId` estable y único; `components/schemas` con discriminantes para filtros y resultados;
- ejemplos de éxito, validación, expiración, cuota y conflicto;
- scopes OAuth/OIDC o bearer de cuenta de servicio;
- contratos de error según Problem Details **RFC 9457** si se adopta en implementación;
- versión mayor en ruta solo ante ruptura real y políticas de sunset;
- `Idempotency-Key`, `Retry-After`, `ETag`, `If-Match`, `X-Request-ID` cuando aplique;
- validación request/response en tests contractuales;
- OpenAPI no debe documentar operaciones sensibles no permitidas al cliente.

**Nota:** documentación OpenAPI no ejecuta autorización; controles se implementan y prueban en el servidor. Nunca generar una política de seguridad a partir de anotaciones meramente informativas.

---

## 16. GraphQL: una vista del contrato, no una base de datos paralela

GraphQL agrega valor para aplicaciones que seleccionan campos dinámicamente. La experiencia de Cube (§4) prueba que puede hacerse una conversión desde selecciones GraphQL a un Query DTO común.

**Ventajas:** introspección condicionada, selección de campos, tipado, composición limitada.

**Costes:** explosión combinatoria por alias/joins, complejidad de autorización a nivel de campo, paginación múltiple, contratos de decimal, dos sistemas de caché y evolución de esquema.

**Recomendación:** postergar hasta disponer de semántica estable y adopción real. Si se implementa, `GraphQLResolver → SemanticQuery → QueryServing`; limitar depth, cost, alias y tamaño, impedir acceso a introspection sensible, y garantizar mismas medidas/fechas que REST.

**No** convertir cualquier tabla de origen a un objeto GraphQL sin seguridad semántica y límites de cardinalidad.

---

## 17. SQL externo, compatibilidad PostgreSQL y Arrow Flight SQL

Tres enfoques **diferentes**:

| Superficie | Uso | Ventajas | Costes/riesgos |
|---|---|---|---|
| SQL HTTP tipado | Consulta semántica delimitada | Fácil de operar | No JDBC/ODBC nativo |
| PostgreSQL wire compatible | Clientes JDBC/ODBC/Power BI | Ecosistema conocido | Autenticación, SQL dialect, metadata, prepared statements y sesiones |
| Apache Arrow Flight SQL | Python/R/Java/analítica Arrow | Transporte columnar y metadatos | Servicio gRPC, TLS, drivers y operaciones protocolarias |

**[ESTÁNDAR]** [Arrow Flight SQL](https://arrow.apache.org/docs/format/FlightSql.html) define RPCs para consultas, esquema, metadatos, prepared statements y resultados mediante Flight. No equivale a simplemente descargar un `.arrow` por HTTP.

**Recomendación por fases:**

1. API tipada de métricas y archivos Arrow IPC cuando exista necesidad.
2. SQL semántico de **solo lectura**, con subconjunto documentado: `SELECT` sobre modelos permitidos, filtros, `GROUP BY`, `ORDER`, `LIMIT`, agregaciones autorizadas; compilación de AST y límites.
3. Adaptador PostgreSQL wire **o** Flight SQL según evidencia de los clientes de destino, con matriz de compatibilidad, prepared statements/metadata y pruebas de RLS.

**Prohibiciones:** no exponer el puerto admin del motor OLAP; no permitir `ATTACH`, `INSTALL`, `COPY TO`, DDL/DML, `read_text('/secrets')`, URL remotas arbitrarias o UDF no autorizadas por la API de negocio. El editor de desarrollo (si existe) utiliza permisos y runtime separados.

Una cadena de conexión PostgreSQL **no** convierte al motor DuckDB en PostgreSQL ni garantiza semántica de funciones equivalente.

---

## 18. SDKs públicos: propuesta mínima y calidad verificable

**Iniciales:** TypeScript (Web/Node) y Python (Pandas/Polars/Arrow consumers). Posteriormente Rust, Go, Java y .NET bajo demanda.

Un SDK debe:

- Obtener capabilities del servidor y negociar versiones.
- Soportar autenticación del usuario o cuenta de servicio sin escribir tokens en logs.
- Validar entradas y deserializar tipos lógicos sin pérdida.
- Manejar automáticamente reintentos **solo en operaciones seguras/idempotentes**; honrar `Retry-After`.
- Exponer `cancel`, `status`, iteración por páginas, streaming con cierre de recursos.
- Ofrecer clientes `management`, `semantic`, `query`, `exports` separados, construidos sobre HTTP común.
- Generar tipados desde OpenAPI y conservar una fina capa manual para ergonomía y errores.
- Validar compatibilidad de SDK n-1 con servidores nuevos mediante contract tests.

### Ejemplo Python ilustrativo (no ejecutable todavía)

```python
client = BIClient(base_url="https://bi.example", token=token)
result = client.semantic.query(
    model="sales",
    measures=["amount_billed"],
    dimensions=["billing_month"],
    filters={"area": {"eq": "101"}},
    consistency="latest_committed",
)
arrow_table = result.to_arrow()  # si servidor y SDK lo ofrecen
```

El SDK **nunca** deriva el SQL final ni confía en `display_label` como field ID.

---

## 19. Webhooks y Event API: cambio real frente a polling

**Eventos de dominio recomendados (posteriores al MVP):**

- `resource.published`, `resource.deprecated`, `resource.deleted`;
- `model.run.completed`, `model.run.failed`;
- `semantic.revision.changed`, `policy.revision.changed`;
- `dataset.snapshot.published`, `dashboard.published`;
- `export.completed`, `export.failed`, `alert.triggered`.

**Entrega** at-least-once, con `event_id`, `occurred_at`, schema_version, namespace, resource ID y revision. Consumidores deben deduplicar. El emisor utiliza transactional outbox cuando requiera consistencia con publicaciones; webhook entrega reintentos con backoff/jitter, límite de TTL, firma HMAC, anti-SSRF y DLQ.

**[ESTÁNDAR]** [AsyncAPI 3.0.0](https://www.asyncapi.com/docs/reference/specification/v3.0.0) documenta APIs asíncronas de múltiples protocolos. Adoptar CloudEvents como envoltura puede ser útil, pero no es obligatorio; sus campos y tipos deben mapearse y probarse.

**No** ofrecer un webhook para cambios de un solo filtro de sesión: eso produce alto volumen y exposición de datos. Eventos de gobierno y estado publicados sí tienen valor externo.

---

## 20. MCP actual: especificación `2026-07-28`, no la versión antigua

**[ESTÁNDAR]** La especificación publicada [MCP 2026-07-28](https://blog.modelcontextprotocol.io/posts/2026-07-28/) cambia las versiones anteriores:

- [Transporte Streamable HTTP 2026-07-28](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/basic/transports/streamable-http.mdx) usa endpoint POST; **el GET de streaming de versiones anteriores se eliminó**.
- Se eliminó la sesión a nivel de protocolo y el handshake previo de `initialize`; la solicitud incluye metadatos de versión/capacidades. Una conexión HTTP no equivale a una sesión de usuario.
- [Tools 2026-07-28](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/server/tools.mdx) describe `tools/list`, `tools/call`, esquemas de entrada/salida, anotaciones y resultados estructurados.
- [Autorización 2026-07-28](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/basic/authorization/index.mdx) describe OAuth, metadatos de recurso protegido, auditoría de audiencia del token y restricciones de propagación.
- Los servidores deben validar `Origin` y protegerse contra DNS rebinding.

**Propuesta:** MCP es un **adapter sobre Analytics API**, no un runtime LLM privilegiado. Implementación read-only inicial; inventario de herramientas estable, por ámbito, con esquemas y límite de resultados.

### Tools recomendados

| Nombre | Permiso | Datos |
|---|---|---|
| `bi.list_semantic_models` | read:catalog | Solo recursos visibles |
| `bi.describe_model` | read:catalog | Campos y definiciones autorizados |
| `bi.list_metrics` | read:catalog | Medidas y dimensiones |
| `bi.query_metrics` | query:semantic | Query tipado y cuotas |
| `bi.explain_query` | query:explain | Plan saneado |
| `bi.get_query_status` | read:query | Solo jobs del actor |
| `bi.get_dashboard_summary` | read:dashboard | Metadata accesible |
| `bi.create_export` | export:data | Confirmación y límites explícitos |

**No incluir por defecto:** `run_arbitrary_sql`, `publish_model`, `delete_dataset`, `change_policy`, `connect_to_any_url`.

**Seguridad:** respuesta del modelo puede contener texto externo no confiable; no debe interpretarse como instrucción para elevar privilegios. La LLM no decide el actor, tenant ni scopes. Sus llamadas al servidor requieren autenticación y autorización en cada request, controles de coste, RLS y logging. Las herramientas con efectos laterales necesitarán intervención humana y delegación estricta.

---

## 21. Ecosistema extensible: protocolos versus plugins

No confundir:

1. **External API adapters**: HTTP/GraphQL/SQL/MCP; exponen contratos centrales.
2. **Data connectors**: extraen/leen orígenes; son del M01.
3. **Model execution adapters**: motores SQL; pertenecen a M02/M05.
4. **Visualization adapters**: ECharts/Highcharts; pertenecen a M09.
5. **Import/export bridges**: TMDL, dbt, BI, formatos declarativos; pertenecen al catálogo.
6. **Automation integrations**: jobs, alertas y webhooks; conectan el sistema hacia fuera.

**Extension Manifest propuesto:**

```yaml
api_version: extensions.bi/v1
id: example.flight_exporter
version: 0.1.0
kind: result_transport
requires:
  core_contract: '>=1.0 <2.0'
capabilities:
  - read_authorized_query_result
permissions:
  - query:read
resource_limits:
  memory_mb: 256
  timeout_ms: 30000
```

**Importante:** YAML es ilustrativo, no formato impuesto al producto. Validar firmas/suministro de plugin, ABI/API versionada, negación por defecto, sandbox/proceso aislado cuando corresponda, control de egress y secretos por referencia. No ejecutar código arbitrario de plugins en el mismo proceso Rust de forma automática.

---

## 22. Arquitectura física mínima — monolito modular

```text
                   External clients / internal frontend
           Web · Python SDK · CLI · BI client · MCP · Service
                                 |
                              TLS/Auth
                                 |
                         API Transport Layer
        +------------------------+-------------------------+
        |                        |                         |
     REST v1             Optional GraphQL          Optional SQL/MCP
        +------------------------+-------------------------+
                                 |
                        Application API Core
             ResourceService · SemanticQueryService
                 JobService · ExportService
                                 |
                        Identity / Policy (M11)
                                 |
             +-------------------+-------------------+
             |                   |                   |
        Metadata (M10)      Semantic (M04)      Query Serving (M05)
             |                   |                   |
         Revisions            IR/Plan          Engine/Storage
                                 |
                           Audit/Telemetry
```

**Despliegue inicial:** proceso backend con componentes lógicos separados. Pool de workers para exportaciones/consultas largas solo si la carga lo requiere. Edge Gateway/WAF externo opcional. Evitar un microservicio por protocolo. Separar procesos por aislamiento/rendimiento o dominio de confianza cuando existan benchmarks y riesgos demostrados.

### Crates Rust tentativos (no implementados)

```text
crates/
  platform-api-contracts/     # tipos públicos/serde/versión
  platform-api-http/          # routing, OpenAPI, middleware
  platform-api-application/   # servicios de caso de uso
  platform-api-jobs/          # jobs async, export
  platform-api-policy/        # adapter a M11, sin duplicar políticas
  platform-api-events/        # envelope/outbox opcional
  platform-api-mcp/           # adapter opcional
  platform-api-graphql/       # adapter opcional
  platform-api-sql/           # adapter SQL posterior
```

Los límites son **módulos lógicos**: combinar crates si un workspace excesivamente fragmentado perjudica el desarrollo.

---

## 23. Seguridad de borde y multitenancy

**Amenazas y respuesta mínima:**

| Amenaza | Mitigación |
|---|---|
| JWT de otro audience | Verificar issuer, audience, expiry y signature |
| Impersonación por header | ActorContext solo creado por middleware confiable |
| RLS eludido vía export | Misma ruta semántica/política que lectura interactiva |
| Cache cross-tenant | Llave incluye ámbito y política; tests multi-actor |
| SQL arbitrary / función peligrosa | AST restringido y gateway read-only separado |
| GraphQL cost explosion | Budgets depth, nodes, time, concurrency |
| MCP prompt injection | Resultados no confiables, scopes, tools de lectura, approval |
| SSRF por webhook/plugin | Egress allowlist, DNS/rebinding checks, no IP internas |
| Replay de publicación | Idempotency/If-Match y permisos de publish |
| Paginación ofensiva | Cursors firmados con TTL, cuotas e identidad |
| Leaks por errores | Sanitizar SQL/secrets, IDs de correlación |
| Descarga temporal filtrada | URLs expiran, autorización con ámbito, auditoría |

**Seguridad doble:** `authorize_resource_action(actor, ...)` y `authorize_data_query(actor, semantic_plan, ...)`, sin API alternativa que omita uno. Los tokens de service account deben ser específicos por entorno y no recibir privilegios humanos admin de manera implícita.

---

## 24. SLO, cuotas y observabilidad

Métricas recomendadas: requests por endpoint/actor/tenant; aceptación/rechazo de políticas; p50/p95/p99 por fase (gateway, semántica, espera, motor, transporte), concurrencia, filas/bytes, coste estimado, cancelaciones, timeouts, reintentos, tamaño de exportaciones, intentos MCP, rechazo por cuota y reintentos de webhook.

**Cuotas de ejemplo, no valores de producto:**

- `max_concurrent_queries` por actor/tenant.
- `max_result_rows` y `max_result_bytes`.
- `max_query_wall_time`, `max_memory_estimate` cuando el motor lo informe.
- `max_exports_per_hour` y límite de almacenamiento temporal.
- `max_mcp_tool_calls` por actor y período.
- `max_subscriptions` y política de backpressure.

Implementar OTel tracing con `request_id/trace_id/query_id`, sin registrar tokens, credenciales, datos de filas sensibles ni SQL arbitrario sin saneamiento. Eventos de auditoría separados de telemetría de rendimiento; ambos con retención apropiada.

---

## 25. Publicación, evolución y compatibilidad semántica

Tres ejes distintos de versión:

| Versión | Qué cambia | Ejemplo |
|---|---|---|
| API major/minor | Forma del request/response | `v1` → `v2` |
| Resource schema | Campos de definiciones persistidas | `metrics_view v3` |
| Semantic revision | Fórmula o significado de una medida | `amount_billed@rev12` |

Un SDK que use API v1 puede apuntar a una revisión semántica nueva compatible o explícitamente fijada. Una revisión semántica **no** debe considerarse compatible solo porque el JSON conserva la misma estructura. Cambios en granularidad, unidad, ratio, nulos, cardinalidad y seguridad requieren diagnóstico de breaking-change semántico.

**Estrategia:** mantener soporte n-1 definido por política, `Deprecation`/`Sunset` documentados, tests de contrato entre versiones, diffs de `SemanticModel` y validación de consumidores antes de publicar.

---

## 26. Roadmap de implementación sin burocracia

### Fase A — Vertical slice API semántica

1. Definir Query DTO canónico y `ResultManifest` con tipos, revisiones y política.
2. Implementar `GET /capabilities`, `GET /semantic-models/{id}/metrics`, `POST /queries` y estado.
3. Integrar con M04 + M05 + M11 reales.
4. Test integrador: un KPI coincide exactamente desde SDK Python y frontend.
5. OpenAPI 3.1.1 y client TS/Python generados.

**Salida:** 1 consulta, 2 medidas, 1 dimensión, RLS y rechazo de campo no autorizado funcionando extremo a extremo.

### Fase B — Operaciones y trabajos

6. Draft/validate/publish de recursos via M10, ETag y permisos.
7. Jobs async, cancelación, CSV/Arrow y descarga segura.
8. Paginación por cursor estable, cuota y observabilidad.
9. Tests de versionado de semántica y caché cross-tenant.

### Fase C — Ecosistema de integración

10. SDKs endurecidos y portal de documentación navegable.
11. MCP 2026-07-28 **read-only** con tools estructurados.
12. Webhooks firmados y outbox en publicación si se necesitan.
13. Arrow Flight SQL **solo** con demanda y certificación de drivers.

### Fase D — Superficies avanzadas

14. GraphQL o PostgreSQL wire según evidencia de usuarios/clientes, no ambos sin justificación.
15. SQL semántico de lectura con subconjunto certificado.
16. Marketplace/registro de extensiones auditado, con permisos y sandbox.
17. Federación/embedding avanzado con políticas de compartición y cuotas.

**Criterio de avance:** demostrar funcionalidad real y correctitud de métricas, no contabilizar endpoints o SDKs como logros por sí mismos.

---

## 27. Casos de prueba de aceptación (72)

### A. Contratos y consistencia

1. REST y cliente interno dan el mismo importe para mismas medidas/filtros/revisión.
2. SDK TypeScript y Python no alteran `DECIMAL(18,2)`.
3. `BIGINT` mayor al entero seguro JS llega sin truncamiento.
4. Código `"001"` no se convierte a número.
5. `null` es distinto de string `"null"`.
6. Metadatos incluyen unidades, grano y `field_id`.
7. Query con dimensión inexistente da error tipado, sin SQL expuesto.
8. Query con medida prohibida da denegación, sin filtrar su SQL.
9. Orden temporal estable incluye timezone/rol.
10. Dos clientes obtienen la misma revisión semántica cuando está fijada.
11. Cambiar revisión obliga a key de cache distinta.
12. Respuesta indica truncación si se aplica límite.

### B. Políticas y aislamiento

13. Actor sin permiso no lista un modelo.
14. `describe_model` oculta campo CLS prohibido.
15. `list_values` aplica RLS.
16. Exportación aplica RLS aun sin filtros del dashboard.
17. Actor A no descarga resultado de actor B por adivinar ID.
18. Token de otro audience se rechaza.
19. Token expirado se rechaza.
20. Header `X-User-Id` externo no cambia actor.
21. Cuenta de servicio respeta scopes mínimos.
22. Dos workspaces con mismo nombre de métrica no mezclan referencias.
23. Preview privilegiado no se cachea para usuarios finales.
24. Policy revision cambia invalida resultados afectados.

### C. Jobs y transporte

25. Query rápida retorna estado según contrato.
26. Query larga retorna `202` con URL y job autorizado.
27. Polling de job tiene estados monotónicos válidos.
28. Solicitar cancelación durante ejecución propaga señal si adapter lo soporta.
29. Cancelar job finalizado no borra resultado de otro job.
30. Timeouts devuelven error normalizado.
31. Streaming abortado libera recursos y cuotas.
32. Cursor inválido/alterado es rechazado.
33. Cursor expirado es rechazado.
34. Cursor no sirve si cambia policy o dataset revision.
35. Resultado Arrow y JSON arrojan agregados iguales.
36. Descarga CSV conserva tipos/codificación declarados.
37. URL firmada caducada deja de servir contenido.
38. Exportación masiva se somete a cuotas.

### D. Recursos y publicación

39. `If-Match` antiguo produce conflicto.
40. `Publish` requiere permiso distinto de `Edit`.
41. Draft inválido no sustituye revisión publicada.
42. `Publish` con Idempotency-Key repetida no duplica operación.
43. Revisión publicada presenta audit trail.
44. Cambiar unidad de medida se detecta como riesgo semántico.
45. `Rollback` autorizado restaura revisión apropiada.
46. Borrado referenciado por API externa da diagnóstico.
47. Deprecation comunica fechas y versión afectada.
48. SDK n-1 sigue funcionando en casos compatibles certificados.

### E. GraphQL/SQL/Flight opcionales

49. GraphQL y REST generan SemanticQuery equivalente.
50. Alias/fragmentos repetidos no saltan límites de coste.
51. GraphQL introspection no revela CLS prohibido.
52. SQL API rechaza DDL/DML.
53. SQL API bloquea función que lee archivos locales secretos.
54. SQL API protege RLS en `GROUP BY` y subconsultas permitidas.
55. Adapter SQL emite error explícito por función no soportada.
56. Metadatos del protocolo reportan tipos correctos para cliente certificado.
57. Flight SQL controla autenticación y paginación/streaming.
58. Cancelación de cliente externo libera recursos según contrato.

### F. MCP 2026-07-28 y plugins

59. MCP request valida metadatos de protocolo actual.
60. MCP request con token audience incorrecta se rechaza.
61. MCP transport rechaza Origin no permitido.
62. MCP `tools/list` expone solo tools permitidos por actor.
63. MCP `query_metrics` usa exactamente M04/M05/M11.
64. Texto malicioso devuelto por la consulta no cambia los scopes.
65. `outputSchema` se valida en servidor y cliente de prueba.
66. MCP no permite `run_arbitrary_sql` por defecto.
67. Tool con efecto de publicación exige confirmación y ámbito.
68. Extensión con permiso omitido se rechaza.
69. Extensión incompatible con contract version se rechaza.
70. Plugin con egress prohibido no puede resolver URL interna.
71. Webhook duplicado se deduplica por `event_id`.
72. Webhook fallido reintenta sin detener publicación ni exponer secretos.

**Tests de oro:** numerador/denominador de medidas, `COUNT(DISTINCT)` y total contextual; fixture multi-fact de dos facturas y tres pagos para verificar que ningún protocolo duplica cifras. API tests deben comprobar seguridad y números reales, no solo códigos HTTP.

---

## 28. Preguntas resueltas y abiertas

**Resueltas:**

- ¿Una API por UI? **No; contrato común.**
- ¿REST o GraphQL como base? **REST tipada + OpenAPI inicialmente; GraphQL adaptador opcional.**
- ¿SQL a todos los clientes desde el primer día? **No; primero consultas semánticas autorizadas.**
- ¿MCP independiente del motor semántico? **No; delega en el mismo núcleo.**
- ¿Se puede servir Arrow sin Flight SQL? **Sí, como transporte de resultados por HTTP; no son protocolos equivalentes.**
- ¿El archivo de proyecto determina las URLs API? **No; recurso y API son agnósticos al almacenamiento.**
- ¿JDBC/ODBC garantizado al exponer puerto PostgreSQL? **No; exige certificar soporte de funciones, metadatos, auth, cursores, prepared statements y cancelación.**

**Abiertas para el desarrollo, no bloquean este diseño:**

- ¿Primera carga a soportar: 50, 500 o 5.000 requests/s? Requiere benchmarks.
- ¿Qué consumidores externos justifican Flight SQL vs PostgreSQL wire?
- ¿Se aceptará inicialmente una sola revisión publicada o varias simultáneas por consumidor?
- ¿Dónde ejecutar el worker para exportaciones grandes?
- ¿Qué período formal de compatibilidad de API/SDK se garantizará?
- ¿Qué bibliotecas Rust concretas usará la implementación? Elegir por compatibilidad, mantenimiento y benchmarks.

---

## 29. Fuentes primarias y rutas exactas examinadas

### Proyectos BI — código fuente

1. **Rill, tag fijo** — [runtime/reconcilers/api.go](https://github.com/rilldata/rill/blob/v0.90.4/runtime/reconcilers/api.go), incluye TODO de validación de resolver.
2. **Rill docs** — [Metrics SQL APIs](https://docs.rilldata.com/developers/build/custom-apis/metrics-sql).
3. **Rill docs** — [API YAML reference](https://docs.rilldata.com/reference/project-files/apis).
4. **Cube** — [packages/cubejs-api-gateway/src/graphql.ts](https://github.com/cube-js/cube/blob/master/packages/cubejs-api-gateway/src/graphql.ts): esquema y llamada a `apiGateway.load`.
5. **Cube** — [packages/cubejs-api-gateway/src/gateway.ts](https://github.com/cube-js/cube/blob/master/packages/cubejs-api-gateway/src/gateway.ts): gateway (rama mutable).
6. **Cube** — [packages/cubejs-server-core/src/core/server.ts](https://github.com/cube-js/cube/blob/master/packages/cubejs-server-core/src/core/server.ts): integración del gateway (rama mutable).
7. **Lightdash** — [packages/backend/src/controllers/savedChartController.ts](https://github.com/lightdash/lightdash/blob/main/packages/backend/src/controllers/savedChartController.ts).
8. **Lightdash** — [.github/CONTRIBUTING.md](https://github.com/lightdash/lightdash/blob/main/.github/CONTRIBUTING.md): generación de endpoints API.
9. **Lightdash** — [.github/rules/backend-review.md](https://github.com/lightdash/lightdash/blob/main/.github/rules/backend-review.md): controller-service-model, tsoa/OpenAPI.
10. **Lightdash** — [docs/service-accounts.md](https://github.com/lightdash/lightdash/blob/main/docs/service-accounts.md): alerta sobre paridad de permisos y scopes.
11. **Superset** — [superset/dashboards/api.py](https://github.com/apache/superset/blob/master/superset/dashboards/api.py).
12. **Superset** — [superset/charts/data/api.py](https://github.com/apache/superset/blob/master/superset/charts/data/api.py): ejecución sync/async, seguridad, filtro contexto.
13. **Superset docs** — [REST API Reference](https://superset.apache.org/developer-docs/api/).
14. **Grafana** — [pkg/api/api.go](https://github.com/grafana/grafana/blob/main/pkg/api/api.go): rutas, cuotas y middleware.
15. **Grafana docs** — [HTTP API](https://grafana.com/docs/grafana/latest/developer-resources/api-reference/http-api/): `/api` → `/apis`.
16. **Grafana docs** — [Dashboard HTTP API](https://grafana.com/docs/grafana/latest/developer-resources/api-reference/http-api/dashboard/).

### Estándares actuales y especificaciones

17. [OpenAPI Specification 3.1.1](https://spec.openapis.org/oas/v3.1.1.html).
18. [AsyncAPI Specification 3.0.0](https://www.asyncapi.com/docs/reference/specification/v3.0.0).
19. [Apache Arrow Flight SQL specification](https://arrow.apache.org/docs/format/FlightSql.html).
20. [Flight SQL protocol schema](https://github.com/apache/arrow/blob/master/format/FlightSql.proto).
21. [MCP 2026-07-28 announcement](https://blog.modelcontextprotocol.io/posts/2026-07-28/).
22. [MCP 2026-07-28 transport](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/basic/transports/streamable-http.mdx).
23. [MCP 2026-07-28 tools](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/server/tools.mdx).
24. [MCP 2026-07-28 authorization](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/basic/authorization/index.mdx).
25. [MCP 2026-07-28 changelog](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/changelog.mdx).

### Serie de arquitectura

26. `BI_Modulo_10_Metadata_Architecture_Agnostica_2026-10-10.md` — identidad, revisiones y metadatos sin proveedor obligatorio.
27. `BI_Modulo_11_Identity_Security_Governance_Agnostico_2026-10-10.md` — Actor/Policy/RLS/CLS.
28. M01–M09 — informes anteriores útiles como referencia, sujetos a la revisión de neutralidad antes del ZIP final.

---

## 30. Cierre — criterio de aprobación del módulo

El módulo se considera **diseñado** cuando se haya acordado: (1) contrato Query y Result con tipos; (2) separación Management vs Analytics; (3) autorización obligatoria compartida; (4) REST/OpenAPI v1 como superficie inicial; (5) jobs y límites; (6) SDKs como consumidores del mismo contrato; (7) MCP/GraphQL/SQL/Flight como adaptadores opcionables y no dependencias de base; (8) roadmap y pruebas numéricas/seguridad.

El módulo se considera **implementado** solamente cuando su vertical slice se ejecute y supere tests de integración contra Semantic Core, Query Serving, Identity y Metadata reales. Ninguna especificación sustituye esa comprobación.

**Este informe cubre exclusivamente M12.** No se diseña ni se inicia el siguiente módulo hasta autorización expresa. Los documentos formarán parte del ZIP de la serie tras normalizar los módulos históricos hacia una plataforma BI nueva, sin dependencias de SQLVIZ.

---

*Fin del informe del Módulo 12.*
