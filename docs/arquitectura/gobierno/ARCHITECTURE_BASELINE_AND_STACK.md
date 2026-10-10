# Baseline tecnológico y reglas de arquitectura — plataforma BI nueva

**Estado RC2:** baseline propuesto para los primeros slices, no evidencia de integración ejecutada.  
**Autoridad:** este baseline prevalece temporalmente sobre decisiones **incompatibles** de M01–M09; cuando se implemente, convertirlo en ADR versionado, no ocultar contradicciones.

## 1. Principios de producto y arquitectura

1. **Una plataforma nueva**, sin imports, formatos ni nombres obligatorios de SQLVIZ; referencias Rill/Lightdash/Cube/Grafana son investigación comparativa.
2. **Monolito modular Rust primero** y workers del mismo runtime cuando convenga. Microservicios adicionales sólo ante métricas de necesidad.
3. **Separar verdad de negocio de visualización:** Semantic Query única, implementada/validada en servidor; todos los gráficos, pivotes, agentes y exportaciones utilizan la misma semántica.
4. **Un backend de negocio:** Axum/Rust. SvelteKit aporta composición/routing/frontend, nunca duplica autorizaciones, reglas de medidas ni políticas de consulta. Deploy local preferido: Rust sirve archivos web estáticos; SSR Node sólo si justificado por caso y coste.
5. **Almacenamiento separado:** PostgreSQL para metadatos BI; PostgreSQL como catálogo de DuckLake (esquema/DB separados según pruebas), datos en Parquet; DuckDB embebido para ejecución. No `.duckdb` persistente obligatorio, no SQLite, no Quack requerido.
6. **Datos via contrato**, no acoplamiento UI→DB: JSON HTTP para definiciones/estado/consultas acotadas, Arrow IPC para lotes grandes soportados, SSE para eventos unidireccionales; WebSocket diferido hasta colaboración real.
7. **Recursos versionados:** publicacion/edición separadas, snapshots pinneados, optimistic concurrency (`If-Match`), queries reproducibles y errores tipados.
8. **Secure by default:** identidad efectiva del servidor, ACL/RLS/CLS antes de lectura/cache/export/AI; nunca confiar en selector/JSON del navegador para permisos.
9. **Rendimiento verificable:** comparar oráculo SQL, p95, memoria RSS y bytes de red; no inferir velocidad por adjetivos como “Rust/Arrow/zero-copy”.
10. **Modernidad sostenible:** estándares abiertos, mantenimiento, compatibilidad, pruebas reproducibles, diseño extensible y pocas dependencias; “lo más nuevo” sin estabilizar no es criterio suficiente.

## 2. Herramientas por módulo, con grado de compromiso

| M | Backend / infra | Frontend / UI | Estado y límites |
|---|---|---|---|
| 01 Ingestion | Rust; DuckDB native readers; PostgreSQL/Parquet; ADBC **por conector**; dlt como integración futura, no paso obligatorio | Svelte form/table; Svelte Flow **evaluate** cuando exista editor de flujo | `ADOPT` infraestructura general; drivers concretos `PROBE` |
| 02 Models | Rust DAG; DuckDB SQL; SQLMesh **evaluate** como planificador externo opcional; Polars Rust 2.0 como ruta de modelado first-class (gates G1.12,G1.13,G3.12,G3.13) | SQL editor Monaco lazy; Svelte Flow **evaluate** para DAG visual | Evitar duplicar scheduler, DSL nuevo o parser que no soporte dialecto |
| 03 Storage | DuckLake extensión de DuckDB, catálogo PostgreSQL, Parquet y `DATA_PATH`; metadata BI PostgreSQL independiente | Estado/jobs, lineage y logs virtualizados | `ADOPT`; no terminal xterm.js sólo para logs |
| 04 Semantic | Rust domain model; DuckDB dialect; AST SQL/validator en probe; PostgreSQL versiones | Builder de métricas/filtros con Svelte, Monaco opcional | `ADOPT` contrato y casos; parser/proveedor `PROBE` |
| 05 Query Serving | Axum/Tokio; `duckdb-rs`; Arrow; control de recursos/cancel; cache en memoria o PostgreSQL de metadata solo si conviene | Query status, cancel, error tipado; filtros visuales Svelte | `ADOPT`; Redis/Quack no por defecto |
| 06 Explore | API semántica; query coordinator | `@tanstack/svelte-table`, TanStack Virtual, ECharts; Svelte state | `ADOPT` con pagination/filter/sort backend |
| 07 Pivot | Rust plan pivot; DuckDB aggregation, Arrow/result matrix | AntV S2 **PROBE**; fallback Svelte virtual matrix | Ningún cliente agrega totales empresariales por su cuenta |
| 08 Canvas | Recursos/revisiones PostgreSQL, query templates | GridStack.js **PROBE**; paneles reales Svelte; DnD Svelte si necesario | GridStack controla geometría, no HTML serializado ni semántica |
| 09 Viz | `VizSpec` y `ChartData` tipados, separados del serving | Apache ECharts **ADOPT**; Highcharts **DEFER** por licencia/alcance | SSR SVG selectivo, interactividad client-only |
| 10 Metadata | PostgreSQL + migraciones SQL / query layer, Rust | Forms Svelte, tablas y lineage visual selectivo | Evitar segunda fuente YAML/SQLite viva |
| 11 Security | Axum middleware; OIDC/OAuth2 mediante library verificada; cookies/session; RLS/CLS en policy engine | HTML accesible, Bits UI/shadcn-svelte **evaluate/adopt targeted** | No introducir framework auth sólo por popularidad |
| 12 APIs | Axum; `utoipa` / alternativa OpenAPI **PROBE**; `serde`, Arrow; SSE | Cliente TypeScript generado/verificado desde OpenAPI; fetch | No GraphQL/Flight SQL obligatorio en v0 |
| 13 Runtime | `duckdb-rs`, DuckLake extension, Polars Rust 2.0 first-class LazyFrame, Arrow Rust; ADBC por driver verificado | Arrow-JS para datasets grandes **PROBE** | El runtime no depende de JS; DuckDB es C++ integrado |
| 14 Automation | Tokio workers + PostgreSQL durable state; scheduler lib simple/evaluada; correo webhooks adaptadores | Calendarios y programador Svelte; select libre/inputs HTML | Cron UI no obliga cron string a ser modelo de negocio |
| 15 Observability | `tracing`, OTel **opt-in**, Prometheus (según topología); health | ECharts para métricas; visor logs virtualizado | Collector opcional, no autoridad de `run_id` |
| 16 AI | Tool gateway tipado Rust; model provider interfaces; RAG sólo autorizado; SSE | Chat Svelte nativo, diffs/edit proposal, Monaco lazily | IA no define medidas ni ejecuta SQL privilegiado |
| 17 Frontend | Axum static hosting; OpenAPI; CORS/same origin | SvelteKit **3**, Svelte **5**, TypeScript **6**, Vite **8**, Tailwind **4**, HTML/CSS; ECharts; TanStack; Bits UI | Node >=22.17 / TypeScript >=6 / Svelte >=5.57.1 / Vite >=8.0.12 / vite-plugin-svelte >=7; verificar y fijar lockfile según guía oficial; lockfile |

`ADOPT` = elección inicial. `PROBE` = requiere spike comparable antes de fijar dependencia. `DEFER` = fuera de versión inicial. **No instalar cada librería simultáneamente**.

### Tecnologías compartidas, no “una por módulo”
- **Rust backend:** `axum`, `tokio`, `serde`, `tracing`, `duckdb`, `arrow`; comparar migrador PostgreSQL y cliente (`sqlx` o `tokio-postgres`) mediante spike real. No son compromisos de versiones exactas; fijarlas en `Cargo.lock`.
- **SQL/BI:** SQL DuckDB existente, diseño semántico tipado; `sqlparser-rs` sólo como candidato — su análisis de SQL no equivale a sandbox ni cobertura total del dialecto DuckDB.
- **Almacenamiento:** DuckLake/PostgreSQL/Parquet. Datasets fuente externos también pueden ser leídos directamente sin copiar.
- **Frontend:** SvelteKit3/Svelte5/TS6/Vite8/Tailwind4; `vite.config.ts` (sin `svelte.config.js`), imports `#lib` y `$app/state` (no `$lib` ni `$app/stores`); CSS custom properties, HTML semántico, `@tanstack/svelte-table`, TanStack Virtual, ECharts6 (fijar versión soportada); Monaco/GridStack/Svelte Flow/S2 como plugins sólo en pantallas pertinentes.
- **Testing JS:** `svelte-check`, TypeScript strict, ESLint compatible + Prettier/plugin Svelte, Vitest y Playwright; evaluar alternativas sólo con ADR, no por novedad. Accessibility con axe-core/Playwright donde sea viable.
- **Testing Rust:** `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` (ajustar feature gates por plataforma), `cargo test`, pruebas integración; `cargo audit`/`cargo deny` en CI según compatibilidad verificada.
- **Infra:** Podman o Docker Compose para entorno reproducible; PostgreSQL y el ejecutable Rust, sin Kubernetes ni Kafka ni Redis obligatorios al inicio.

## 3. Frontera frontend / backend

```text
+----------------------- Browser -------------------------+
| Svelte 5 components, SvelteKit routing, TypeScript     |
| HTML/CSS/Tailwind; ECharts; virtualized table/matrix   |
| Editor/Canvas/Chat lazy-mounted after hydration        |
+--------------------------+-----------------------------+
                           | HTTPS same origin
             JSON OpenAPI / Arrow IPC / SSE
                           v
+----------------------- Rust API ------------------------+
| Axum auth → application service → authorized planner   |
|   |                |                    |              |
| metadata PG     job state PG        DuckDB embedded    |
|                                          |             |
|                                      DuckLake/PG      |
|                                          |             |
|                                        Parquet         |
+--------------------------------------------------------+
```

**Regla:** jamás transferir filas masivas al navegador para obtener un KPI o para ordenar toda la tabla. Arrow IPC es una optimización para datasets resultantes apropiados, no reemplazo de agregación en DuckDB. API pública no debe exponer `PreparedSql` ni `PrincipalContext` generado por cliente.

### Estrategia SSR
- Deployment sencillo: SvelteKit `adapter-static` con rutas compatibles y fallback apropiado; Rust sirve build, HTTP API same-origin; protección de rutas en Rust y datos, no sólo en fallback.
- Si es necesario SSR dinámico: despliegue Node/SvelteKit BFF opcional para HTML, pero ninguna política de negocio va en BFF. Debe tratar SSR con credenciales/session server-to-server y política CSRF.
- Monaco, GridStack, S2 y ECharts interactive se importan/montan sólo en navegador, disponen de placeholder accesible y `destroy/dispose` real.
- ECharts SVG SSR no se implementa automáticamente porque Rust sirva HTML estático; ejecutar renderer JS aparte tendría un coste de infraestructura. Debe ganarse mediante benchmark/UX.

## 4. Reglas de seguridad y tipos que no se negocian

- `DECIMAL(18,2)` no se convierte a `number` JS sin justificación de rango/precisión. `BIGINT` usa transferencia canónica/string tipado o BigInt apropiado.
- `TIMESTAMP WITH TIME ZONE` mantiene instant, zona/filtro y serialización acordados. `NULL` no se convierte a 0 por defecto.
- Backend construye contexto de actor y versionado. Cache key incluye actor/policies/snapshot/model/filtros; no usar key sólo de SQL.
- Transferencias HTTP con límites de tamaño, timeouts, cancelación, idempotencia y paginación/cursor tipados. UI descarta respuestas obsoletas.
- No permitir al agente, usuario SQL o plugin sortear RLS/CLS, ni cargar extensiones arbitrarias en DuckDB. Limitar I/O y acceso a secretos con controles de sistema operativo si existen consultas no confiables.
- Secrets en entorno/secret manager, nunca Git, logs o mensajes de agentes. Credenciales externas con referencias opacas y mínimo privilegio.

## 5. Regla de incorporación de dependencias

Para cada propuesta, el agente documenta en una línea: problema → paquete+licencia → alternativa sin dependencia → compatibilidad Rust/TS/Svelte/SSR → mantenimiento → footprint → seguridad → prueba → elección. Necesita **probar** compatibilidad y licencia de la versión exacta; una web de marketing no basta. Evitar paquetes abandonados y polyfills innecesarios. Cada dependencia añadida por primera vez exige prueba de build en CI y razón en el PR. Una librería para módulos avanzados no se descarga en todas las rutas del navegador.

## 6. Decisiones aún abiertas, no bloqueadoras de G0

- Marca final/trademark y dominio (proceso separado del codename).
- Estrategia concreta de UI de Pivot: S2 o fallback propia después de prototipo.
- GridStack vs solución propia para layouts: prueba Drag/Resize/móviles/a11y.
- SQLMesh/dlt como integraciones opcionales (no requerimientos iniciales).
- Identity provider por defecto (OIDC capability primero).
- Distribución final local single-node frente a empresarial multi-instance.
- Licencia OSS/enterprise y política de terceros; resolver antes de publicar repositorios/paquetes.

## 7. Especificaciones externas verificadas

- SvelteKit 3 release: https://svelte.dev/blog/sveltekit-3-is-here
- Requisitos SvelteKit 3: https://svelte.dev/docs/kit/migrating-to-sveltekit-3
- ECharts SSR tradeoffs: https://echarts.apache.org/handbook/en/how-to/cross-platform/server/
- TanStack pagination/virtualization: https://tanstack.com/table/latest/docs/framework/svelte/guide/pagination y https://tanstack.com/table/latest/docs/framework/svelte/guide/virtualization
- GridStack DOM binding: https://github.com/gridstack/gridstack.js/blob/master/src/gridstack.ts


### Enmienda de stack vinculante para dos motores y tres OS

Polars Rust y DuckDB SQL son **dos rutas analíticas de primera clase**; Polars no queda reservado a adaptador ocasional. Parquet/Arrow y publicación DuckLake se comunican por interfaces explícitas sin manipulación directa del catálogo desde Polars. Linux/Windows/macOS: CI por OS, compatibilidad de extensiones y pruebas. DuckDB 2.0-dev se evalúa en carril experimental; la rama estable queda fijada por compatibilidad `duckdb-rs`/extensión DuckLake y benchmarks antes de actualizar. Ver `../ESTANDARES_Y_PORTABILIDAD.md` y `../PERFORMANCE_DESDE_G0.md`.
