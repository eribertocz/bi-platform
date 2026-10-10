# DECISIONES TRANSVERSALES — baseline normativa RC2

**Referencia de decisiones técnicas del diseño RC2 (2026-10-10).** No presupone que exista código implementado ni sustituye los contratos y tareas actuales del repositorio.

## A. Modelo de producto

- Producto BI NUEVO, sin marca definitiva. No reutilizar bases SQLVIZ, `*.sqlviz`, `sqlviz-*`, migraciones heredadas, ni SQLite como persistencia del producto. Las auditorías históricas de código de terceros son referencias, no dependencias.
- Los 17 módulos son áreas de responsabilidad de un **monolito modular Rust + frontend Svelte**, no 17 servicios. Se separan procesos/workers sólo cuando cargas, seguridad o benchmarks lo justifiquen.
- El **único** nombre provisional del repositorio será `bi-platform` (placeholder de ingeniería), sin hacer afirmaciones de marca/registrabilidad; un cambio de marca no cambia contratos.

## B. Stack principal

- Backend: Rust estable de toolchain **fijada en archivo** (`rust-toolchain.toml`) + Axum + Tokio para red y coordinación; operaciones CPU y llamadas nativas bloqueantes fuera de executores async. Serde/OpenAPI, tracing + OTel exportable. Crates exactos se fijan después de una compilación/CI, no por especulación.
- Analytical: DuckDB C++ embebido por `duckdb-rs`, Arrow como interfaz columnar; Polars Rust LazyFrame como ruta de transformación analítica de primera clase, activada según cada caso; ADBC si driver/driver manager soportan caso de uso. Quack remoto **opcional**.
- Lake: DuckLake mediante extensión DuckDB, catálogo PostgreSQL separado lógicamente de PostgreSQL BI, archivos Parquet con `DATA_PATH` explícito; desarrollo single-node local autorizado, despliegue multi-nodo exige storage compartido. No `.duckdb` persistent warehouse como obligación.
- Front: Svelte 5 + SvelteKit 3 + TypeScript estricto; Vite y versiones mínimas compatibles con SvelteKit 3; Tailwind CSS 4 + CSS variables + HTML semántico; Bits UI/shadcn-svelte candidatos por necesidad, no imposición. ECharts predeterminado, TypeScript DTO generados OpenAPI. Server-render only donde aporta, componentes DOM-heavy lazy/onMount.
- Comunicación: same-origin HTTP/OpenAPI; JSON tipado pequeño; Arrow IPC por volumen real; SSE para operaciones unidireccionales; WebSocket sólo colaboración bidireccional. UI jamás se conecta a catálogo DuckLake ni ejecuta duckdb directa.

## C. Derechos de decisión

1. Seguridad/roles/políticas del servidor M11 preceden cualquier filtro de usuario, medida, cache, export o AI.
2. Modelo semántico M04 es único dueño de las medidas BI; serving M05 coordina; runtime M13 ejecuta; UI M06–M09 no recalcula gobernanza.
3. PostgreSQL BI almacena definiciones/revisiones/ejecuciones; catálogo DuckLake almacena snapshots; Parquet almacena datos. Backup/recovery coordinado y dual-state reconciliable.
4. Contratos OpenAPI v1 y tipos lógicos explícitos; `schema_version` en definiciones persistidas; cambios incompatibles por migración/ADR.
5. `DECIMAL` y `INT64` no se redondean al convertir JS; preservar `TIMESTAMPTZ`, NULL y currency.
6. SQL editor experto es superficie separada con autorización/sandbox y no habilitado libremente en un endpoint de producción.
7. Cualquier nuevo paquete debe aportar función concreta, licencia revisada, spike de compatibilidad, mantenimiento y plan de salida.
8. SSR shell no vuelve SSR nativo a GridStack, S2, Monaco ni ECharts interactivas. No introducir Node server en local salvo caso verificable.


## F. Decisiones condicionadas a spikes (no bloquean el G0 pequeño)

- SQLMesh vs orquestación propia (M02), worker aislado para SQL experto, componentes S2 vs tabla virtual, Svelte Flow vs AntV X6, GridStack integración Svelte y SSR, biblioteca de DTO/ORM PostgreSQL, cache/queue en producción y performance targets.
- UI frameworks complementarios: Bits UI/shadcn-svelte/TanStack/Monaco/S2/Arrow JS; versión exacta y bundles se fijan al introducir funcionalidad, no como 17 dependencias iniciales.
- Licencia/marca/nombre legal requieren verificación externa, no se declaran libres por intuición.

## G. Referencias técnicas verificadas

- DuckLake catálogo PostgreSQL: https://ducklake.select/docs/stable/duckdb/usage/choosing_a_catalog_database
- SvelteKit v3 migración y versiones mínimas: https://svelte.dev/docs/kit/migrating-to-sveltekit-3
- Apache ECharts SSR límites: https://echarts.apache.org/handbook/en/how-to/cross-platform/server/
- Axum docs oficiales: https://docs.rs/axum/latest/axum/


## H. Enmiendas RC2 de seguridad e integración (normativas)

1. **Autenticación humana antes de usuarios reales:** identidad de desarrollo/mocks **solo para fixtures sintéticos**. Antes de exponer fuentes reales: OIDC Authorization Code + PKCE o alternativa equivalente revisada; sesión de servidor con cookie `Secure`, `HttpOnly` y `SameSite` apropiado; expiración/revocación, CSRF en operaciones mutadoras, pruebas 401/403 y tenant isolation. Sesiones y autorización son responsabilidad Rust/M11; Svelte no aporta claims confiables. API keys de servicio tienen scopes, expiración/rotación, auditoría y almacenamiento seguro separados de sesiones web.
2. **Conectores y secretos:** el usuario nunca puede inyectar rutas de archivos arbitrarias, SQL externo sin presupuesto ni secrets mediante campos de origen. Almacenar credenciales en secret store apropiado o cifrado a nivel aplicación con gestión de claves; PostgreSQL guarda referencias/metadata, no secretos en claro. Validar SSRF/egress y rutas permitidas, permisos por conector y rotación.
3. **Exposición temporal:** endpoints de pruebas (ej. `/api/v1/dev/fixtures/...`) solo en modo desarrollo y explícitamente inaccesibles en paquetes/distribuciones de producción. Proteger todos los endpoints semánticos mediante identidad **derivada en el servidor** y autorización de cada operación y recurso; RLS/CLS nunca solo en browser.
4. **Contratos:** `docs/arquitectura/contratos_referencia/openapi-rc2-referencia.json` es una **referencia RC2**, no una API ejecutada. El repositorio nuevo definirá su propio OpenAPI vigente antes de integrar código cliente/servidor. DTO públicos derivan de un único OpenAPI; `ActorContext` y `AuthorizedSemanticQuery` son internos, no aceptables como payload usuario. `DECIMAL`, `INT64`, timestamp y NULL conservan exactitud. Toda ampliación se versiona y se prueba por contrato.
5. **Límite seguridad DuckDB embebido:** restringir funciones/extensiones, file access, conexiones remotas, consultas lentas, spills y concurrencia según threat model; SQL experto de usuarios no confiables exige aislamiento a nivel de proceso/OS o bloqueo de esa función. No asumir que un parser, readonly o `SET memory_limit` son sandbox completo.
6. **Ejecución y HTTP:** no bloquear Tokio con llamadas FFI DuckDB/Polars; cancelar no debe reutilizar/interrupt una conexión perteneciente a otra consulta. APIs síncronas pequeñas y jobs/streaming para grandes. Las queries aplican admisión, deadlines, límites y errores tipados del lado servidor. Sin Node.js servidor obligatorio en despliegue estático, aunque Node se usa para compilar frontend.


## I. Enmiendas normativas: calidad, Polars, sistemas operativos y versiones

1. **Calidad desde G0:** aplicar los principios de calidad descritos en este documento y en [`PERFORMANCE_DESDE_G0.md`](PERFORMANCE_DESDE_G0.md); la arquitectura ligera y modular se evalúa por contratos, exactitud, seguridad, portabilidad, rendimiento y facilidad de cambio, no número de crates/servicios.
2. **Polars 2.0 de primera clase:** reemplazar la lectura «Polars solo auxiliar» en documentos históricos y en M13 por dos caminos autorizados DuckDB SQL/Polars Rust LazyFrame, con testing/lineage, Arrow y publicación DuckLake mediante frontera transaccional; detalles en [`ESTANDARES_Y_PORTABILIDAD.md`](ESTANDARES_Y_PORTABILIDAD.md). Python Polars no confiable solo en sandbox separado si se habilita.
3. **Portabilidad:** Linux, Windows y macOS son OS objetivo; CI matrix y pruebas sobre plataforma, sin asumir portabilidad por compilación.
4. **Versiones:** Polars 2.0 estable candidato para carril principal; DuckDB 2.0-dev/alpha solo carril experimental hasta release estable y compatibilidad verificadas. No actualizar por rumor ni fecha. `duckdb-rs`, DuckLake y C ABI son gates.
5. **Performance day zero:** baselines en G0 antes de G1; no se inventan SLO. Controles M15 aplican desde el primer corte; ver [`PERFORMANCE_DESDE_G0.md`](PERFORMANCE_DESDE_G0.md).
