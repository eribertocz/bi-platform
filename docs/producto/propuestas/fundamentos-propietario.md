# Documentación Fundamental – Herramienta BI Open Source (aportada por el propietario, 2026-10-11)

**Stack:** ELT propio (estilo SQLMesh) · Motor Semántico (Rust + Apache Arrow) · Frontend (Svelte 5 + ECharts)

8 documentos base para usar como contexto y reglas de proyecto.

## Documento 1 – Leyes Fundamentales
(Ya incorporadas y mejoradas en `docs/producto/leyes-bi.md`, PR #82.)
1. Verdad única: el motor semántico es la única fuente de métricas, dimensiones y relaciones; sin cálculos ad-hoc en el frontend; cada métrica con nombre canónico, descripción, fórmula, owner e historial; los usuarios no técnicos nunca escriben SQL crudo.
2. Transparencia: cada visualización muestra query generada, definición, filtros (incluidos los de seguridad) y fecha de los datos; lineage tabla → modelo → métrica → visualización; cambios versionables y reversibles; registro de quién consultó qué.
3. Seguridad por diseño: RLS y CLS en el motor o la capa de acceso, nunca solo en el frontend; roles y permisos granulares; mínimo privilegio; SSO (OIDC/SAML); el modo avanzado de ECharts no se salta la seguridad.
4. Consistencia: misma métrica y filtros, mismo resultado; frescura visible; modelos deterministas; caché semántica.
5. Separación de capas: ELT, almacenamiento, motor semántico, API, frontend; el frontend nunca conoce tablas físicas.
6. Experiencia por niveles: básico, intermedio, avanzado (ECharts amplio sobre datos ya filtrados).
7. Rendimiento predecible: estimar costo; timeout y gobierno de recursos por usuario/rol; caché semántica; consultas asíncronas y materializaciones.
8. Gobierno del modelo: Git; dev → staging → prod; PR; tests de métricas; deprecación controlada.
9. Observabilidad: uso, rendimiento, errores; logs estructurados; reproducir consultas.
10. Extensibilidad controlada.

## Documento 2 – Principios de Diseño + Anti-Patrones
Principios: Progressive Disclosure; Fail Fast & Fail Loud (nunca números incorrectos en silencio); Semantic First, Visualization Second; Least Privilege by Default; Determinism; Human-Readable Semantics; Separation of Concerns; Immutability of Historical Data; Observability by Default; Extensibility without Corruption.

Anti-patrones prohibidos: SQL en el frontend; métricas calculadas solo en el gráfico; seguridad solo en el frontend; God Dashboard; Magic Configuration (métricas solo en UI sin versionado); Silent Failures (devolver 0 o null sin explicación); Over-fetching; acoplamiento frontend ↔ tablas físicas; permisos binarios (solo admin/viewer); ignorar freshness; bypass del motor semántico en modo avanzado; cambios destructivos sin control (borrar métricas en uso sin deprecación).

Prioridad: 1) Leyes, 2) Anti-patrones, 3) Principios.

## Documento 3 – Capacidades Core + Patrones de Arquitectura
Core (obligatorias): modelo semántico versionado; resolución semántica única; RLS + CLS; lineage y explicabilidad; roles y permisos granulares; freshness visible; caché semántica; audit log; multi-modo de acceso (Direct Query + Importado); exportación que respeta seguridad.

Avanzadas (diferenciadoras): alertas basadas en métricas; what-if / escenarios; embedded analytics; time intelligence avanzado; collaborative editing; versionado de dashboards; metric tests / asserts; self-service modeling con gobernanza; API pública estable (headless BI).

Patrones: Semantic Layer Central; Metrics as Code; Headless BI; separación de capas; Compiler + Runtime (petición semántica → plan → ejecutar); Pushdown Computation; Dual Storage (Direct Query + Importado); Progressive UI Modes; Environment Promotion (dev → staging → prod); Observability First.

Criterios de aceptación: no viola Leyes; no introduce Anti-Patrones; pasa por el motor semántico; aplica seguridad; es observable; comportamiento claro ante errores.

## Documento 4 – Decisiones Técnicas por Capa
- ELT: modelos como código; dev → staging → prod; Git; dependencias, materialización y tests declarados; `plan` + `apply`; incrementales desde el día 1; separar staging y marts. Prohibido: que el frontend dispare transformaciones ad-hoc o que la lógica de métricas viva en el ELT.
- Almacenamiento: Direct Query e Importado nativos; el modo se decide por fuente o por modelo; el motor abstrae el modo al frontend; escrituras de importados auditables; formatos columnares (Parquet + Arrow).
- Motor semántico (Rust + Arrow): responsabilidades exclusivas: resolución de métricas y dimensiones, RLS + CLS, generación y optimización del plan, caché semántica, lineage y explicación, validación de permisos. Recibe peticiones semánticas, nunca SQL crudo. Entrada `{ metrics, dimensions, filters, time_range, user_context }`; salida Arrow + explicación + freshness; Compiler + Runtime (plan lógico → optimización → ejecución). RLS/CLS dentro del motor con user_context; caché segura por contexto de usuario/rol; invalidación por cambio de métrica, datos o políticas.
- API de consulta: único punto de entrada al motor; tipada y versionada (`/v1/...`); autentica y autoriza antes de llamar al motor; devuelve datos + metadatos + freshness + explicación (según permisos); síncrona y asíncrona; rate limiting y gobierno de recursos. Prohibido: endpoints que acepten SQL libre del usuario final.
- Frontend (Svelte 5 + ECharts): solo conoce el modelo semántico; todo dato pasa por la API; los 3 modos son de configuración visual, no de acceso a datos; separar estado de datos, configuración visual y estado de UI; nunca construir la query desde el `option` de ECharts; guardar por separado definición semántica y configuración visual; mostrar siempre freshness, empty states y errores legibles.
- Seguridad transversal: OIDC; autorización en motor + API; el frontend solo oculta UI; audit log de acceso y de cambios.
- Observabilidad: p50/p95/p99, cache hit ratio, errores, uso por métrica/dashboard; trazas reproducibles; OpenTelemetry.
- Artefactos: modelos ELT, semantic model, dashboards (definición semántica + config visual) y permisos/políticas versionados; caché efímera.

## Documento 5 – Estructura de Repositorios + Contratos
Monorepo propuesto: `crates/{semantic-engine, arrow-runtime, security, api-server}`, `elt/{models, tests}`, `packages/{semantic-client, ui, echarts-safe}`, `apps/web`, `docs/architecture`, `scripts`.

Contrato Frontend → API:
```typescript
interface SemanticQuery { metrics: string[]; dimensions?: string[]; filters?: Filter[]; timeRange?: TimeRange; orderBy?: OrderBy[]; limit?: number; offset?: number; }
interface SemanticQueryResult { data: ArrowTable | RecordBatch; schema: Field[]; meta: { freshness: string; metrics: MetricMeta[]; queryId: string; cacheHit: boolean; explanation?: Explanation; }; }
```
Contrato API → Motor (Rust): `SemanticRequest { metrics, dimensions, filters, time_range, user_context: UserContext, options }` → `SemanticResponse { batches: Vec<RecordBatch>, meta }`.
Motor → Storage: un plan de ejecución (pushdown SQL, lectura Arrow/Parquet materializado o híbrido); el storage nunca ve el modelo completo.
Versionado: `/v1`, `/v2`; cambios breaking con versión nueva; cliente TS generado del schema.
Dependencias: `apps/web` solo de `semantic-client` y `ui`; el cliente solo habla con la API; el motor no conoce Svelte ni ECharts; el ELT no conoce el frontend.

## Documento 6 – Modelo de Seguridad
- Seguridad evaluada en el motor y reforzada en la API; el frontend solo controla visibilidad de UI; mínimo privilegio; todo auditable.
- Autenticación: OIDC; API keys con scopes para service accounts y embeds; sesiones con expiración y refresh rotativo.
- Roles base: viewer, analyst, modeler (propone cambios con review), admin; permisos granulares por recurso: métrica (view, use_in_dashboard), dashboard (view, edit, share, delete), semantic model (view, propose_change, approve, publish), fuente (connect, configure), usuario/rol (manage).
- RLS: políticas por entidad o métrica evaluadas con `user_context`; ejemplo `filter: "orders.region IN ${user.regions}"`; el motor las inyecta y el usuario no puede quitarlas.
- CLS: columnas `sensitive`/`restricted`; según rol se muestran, enmascaran o eliminan; nunca llegan al frontend sin permiso.
- Modo avanzado de ECharts: solo afecta la visualización; no altera la query ni la seguridad; sanitizar expresiones evaluables.
- Audit log: login/logout; consultas (usuario, métricas, dashboard, fecha, duración, caché); cambios del modelo; cambios de permisos; accesos denegados.
- Embeds y service accounts: tokens de corta duración o API keys con scopes limitados; RLS/CLS con el contexto del token.

## Documento 7 – Diseño del Motor Semántico (DSL + Arrow)
Única fuente de verdad para métricas y dimensiones, resolución de consultas, seguridad, planes, caché y explicación.

Metrics as code (ejemplo):
```yaml
metric:
  name: revenue
  label: "Ingresos"
  description: "Suma de amount de órdenes completadas"
  type: sum
  sql: "orders.amount"
  filters: ["orders.status = 'completed'"]
  dimensions: [date, region, product_category]
  owner: finance-team
  tags: [core, finance]
  status: active
```
Flujo: 1) API recibe SemanticQuery + user_context; 2) valida permisos y métricas; 3) aplica RLS/CLS; 4) compila a plan lógico; 5) optimiza (projection pruning, filter pushdown, constant folding); 6) decide ejecución (pushdown SQL, Arrow/Parquet materializado o híbrido); 7) ejecuta a Arrow RecordBatch; 8) post-agregaciones; 9) caché semántica; 10) devuelve datos + meta (freshness, explicación, cache hit).
Arrow en todo el camino caliente; evitar JSON en el hot path; Arrow IPC o Flight.
Time intelligence: time_range relativo y absoluto; comparaciones (periodo anterior, YoY, MoM); granos; YTD/MTD/QTD.
Testing: compilación de métricas; políticas RLS/CLS; consistencia; golden files de planes; rendimiento básico.
Extensibilidad: funciones de agregación y time intelligence por plugins internos; conectores tras un `DataSource` trait; ningún plugin salta la seguridad.

## Documento 8 – UX y los 3 Modos de Visualización
- Básico: selector de métricas (búsqueda y descripciones) y dimensiones; filtros predefinidos y time range claro; tipo de gráfico sugerido automáticamente; sin opciones de ECharts; freshness visible; empty states con explicación; exportar en un clic (CSV/imagen).
- Intermedio: tipo de gráfico, ejes, series, apilado, leyenda, tooltip, colores, orden, límites, formato de números, eje doble, líneas de meta; guarda configuración estructurada (no el option crudo); solo opciones validadas.
- Avanzado: editor del `option` (JSON o UI asistida), features avanzadas (graphic, visualMap, dataZoom); el option solo define la representación; se guardan por separado la Semantic Query y el option sanitizado.
- Reglas transversales: freshness visible; loading y skeletons; errores legibles y accionables; empty states útiles; filtros obvios y reversibles; drill-down predecible; exportación de primera clase; responsive real (móvil); modo claro/oscuro.
- Widget persistido: `{ id, semanticQuery, visualMode, visualConfig, title, description }`; nunca solo el option sin la Semantic Query.

## Cierre
Prioridad en conflicto: 1) Leyes, 2) Anti-patrones, 3) Seguridad, 4) Decisiones del motor y capas (docs 4 y 7), 5) Capacidades y patrones, 6) Estructura y contratos, 7) UX.
Instrucción: antes de implementar cualquier feature, verificar que no viola las Leyes, no introduce Anti-Patrones y respeta la seguridad y las decisiones por capa. El frontend nunca calcula métricas ni aplica seguridad real. Todo dato pasa por el motor semántico.
