# Plataforma BI abierta — Módulo 17: Frontend Architecture, Design System y conexión con Rust
## Investigación de SvelteKit 3, UX empresarial, contratos frontend/backend, resultados Arrow, visualización, seguridad, despliegue y pruebas

**Fecha de investigación:** 10 de octubre de 2026  
**Edición:** 1.0 — documento de arquitectura; no se ha implementado ni probado el producto  
**Producto:** plataforma BI empresarial nueva, independiente de SQLVIZ, `.sqlviz` y su código previo.  
**Ámbito:** arquitectura de aplicación web, sistema de diseño, composición de experiencias M06–M09, comunicación con Rust y pruebas de integración.  
**Estado de las decisiones:** recomendadas, sujetas a pilotos, validación de licencias y benchmarks; no confundir diseño con mediciones.

**Continuidad:** se respetan Rust + `duckdb-rs` embebido, DuckLake con catálogo PostgreSQL y Parquet, Arrow como límite columnar, Polars Rust opcional, ADBC por conector y Quack remoto **solo opcional**. El PostgreSQL de metadatos BI y el catálogo transaccional de DuckLake representan dominios lógicos diferentes. No exigir SQLite ni un archivo `.duckdb` persistente. No hay obligación de reutilizar repositorios SQLVIZ.

**Hallazgo relevante a esta fecha:** [SvelteKit 3 se lanzó el 1 de octubre de 2026](https://svelte.dev/blog/sveltekit-3-is-here). Los documentos y ejemplos heredados de SvelteKit 2 deben revisarse, especialmente la configuración de Vite/adapter, no copiarse mecánicamente. La referencia oficial de configuración actual es [adapter-static](https://svelte.dev/docs/kit/adapter-static), [adapter-node](https://svelte.dev/docs/kit/adapter-node) y [guía de migración](https://svelte.dev/docs/kit/migrating-to-sveltekit-3).

---

## 0. Dictamen ejecutivo: una sola plataforma, un solo backend de negocio

**Elegir:**

1. **Svelte 5 + SvelteKit 3 + TypeScript** como experiencia web, rutas y composición.
2. **Rust + Axum** como API de negocio, control de identidad/permisos, contratos semánticos, coordinación y ejecución.
3. **Mismo origen HTTP** como ruta por defecto: `/` para la aplicación y `/api/v1/*` para API Rust. **No hay Node.js permanente** en el despliegue estático inicial; Node sí es herramienta de construcción/desarrollo.
4. **OpenAPI como contrato público verificable** y cliente TypeScript generado; códigos HTTP y payloads tipados como superficie común entre web, SDK y agentes.
5. **JSON para control y resultados moderados; Arrow IPC para lotes columnares cuando aporte; SSE para eventos de estado/progreso; WebSocket únicamente por casos bidireccionales reales.**
6. **Apache ECharts como renderer inicial**, independiente del `VizSpec` del M09. Highcharts no se distribuye en el núcleo libre por su licencia comercial.
7. **CSS propio + Tailwind CSS 4 + tokens**, accesibilidad y componentes headless en vez de un tema rígido de tercero.
8. **Estado local en Svelte; estado de servidor en cliente API/query cache donde agregue valor; estado de negocio durable en Rust/PostgreSQL.**

**No elegir:** segundo backend empresarial en Node, GraphQL por principio, REST que permite ejecutar SQL sin autorización, navegador conectado a DuckLake/PostgreSQL, inferencia semántica en ECharts, exportar millones de registros al navegador, WebSocket para cualquier consulta, diseño dependiente de una biblioteca visual cerrada ni un DSL nuevo.

**Regla inmutable:** el frontend *orquesta la experiencia*; M04/M05/M11/M13 *determinan el significado, la autorización y el resultado*. Ninguna operación del navegador puede alterar ese reparto.

---

## 1. Arquitectura completa y responsabilidades

```text
                           NAVEGADOR
    ┌───────────────────────────────────────────────┐
    │ Svelte 5 + SvelteKit 3 + TypeScript           │
    │ AppShell / Navigation / Explore / Pivot       │
    │ Canvas / Dashboards / SQL Editor / AI         │
    │ Design System / VizSpec adapters / ECharts    │
    │ API client + session UI + query state         │
    └───────────────────┬───────────────────────────┘
                        │ mismo origen /api/v1
           JSON/REST    │ Arrow IPC    │ SSE
                        ▼
    ┌───────────────────────────────────────────────┐
    │ Rust / Axum HTTP boundary                     │
    │ OIDC/session, CSRF, RBAC, scopes, budgets      │
    │ OpenAPI DTO / audit / tracing / validation    │
    └───────────────────┬───────────────────────────┘
                        ▼
    ┌───────────────────────────────────────────────┐
    │ M04 Semantic + M05 Query Serving + M10/M11    │
    │ canonical query → authorization → execution  │
    │ snapshots/caches/jobs/query ownership         │
    └──────────────┬────────────────┬───────────────┘
                   │                │
             ┌─────▼──────┐  ┌──────▼───────────────┐
             │ M13 DuckDB │  │ Polars Rust, Arrow    │
             │ duckdb-rs  │  │ ADBC optional         │
             └─────┬──────┘  └──────────────────────┘
                   │
             ┌─────▼──────────────────────────┐
             │ DuckLake: PostgreSQL catalog   │
             │ Parquet data; snapshots        │
             └────────────────────────────────┘

    M14 Jobs/notifications  M15 Observability  M16 AI tools
       ← operate through the SAME Rust contracts →
```

### 1.1 Qué hace cada extremo

| Preocupación | Navegador Svelte | Rust API/servicios | DuckDB/DuckLake |
|---|---|---|---|
| Identidad | muestra sesión | autentica y autoriza | no decide sesiones web |
| Filtros | compone selección | valida tipos, contexto, RLS | ejecuta predicados autorizados |
| Medidas | solicita y visualiza | compila semántica | calcula agregados |
| SQL editor | edita texto y parámetros | valida rol/modo, costes, límites, audita | ejecuta en contexto controlado |
| Dashboard | layout/edición temporal | versiona, valida y publica | no conoce posición de panel |
| Pivot | árbol y virtualización | planifica e interroga | agrega y totaliza correctamente |
| IA | chat y aprobaciones | herramientas, políticas, planes | nunca acepta privilegios del LLM |
| Exportar | inicia/descarga | ejecuta job y firma respuesta | produce datos/exportables |
| Operaciones | estado de jobs/error | reintentos, auditoría, progreso | errores físicos y perfiles |

**Frase contractual:** la autoridad de cada dato no puede residir simultáneamente en navegador y backend. La UI posee el estado visual; Rust posee las definiciones de negocio persistentes y sus transiciones autorizadas.

---

## 2. Auditoría comparativa de referentes reales

### 2.1 Rill: aplicación web modular y experiencia exploratoria

- [Repositorio Rill](https://github.com/rilldata/rill), con carpetas `web-admin`, `web-common`, `web-integration`, `web-local` y runtime aparte; el proyecto declara SvelteKit en sus tecnologías.
- **Rescatar:** experiencias de exploración/desarrollo separadas de runtime, componentes web compartidos, preview y filtros sincronizados con semántica.
- **No copiar:** backend Go, disposición exacta de carpetas, configuración/cloud, ni asumir que todas las experiencias del producto comercial están en una edición libre.
- **Mejorar:** unificar contrato semántico de Explore, Canvas, Pivot, API, IA y descargas; proporcionar un AppShell empresarial altamente configurable.

### 2.2 Lightdash: frontend mantenible, API y datos de servidor

- [README y stack oficial](https://github.com/lightdash/lightdash): frontend React/Mantine/Vite/TanStack Query y backend Node/Express/PostgreSQL.
- [Documento de arquitectura del repositorio](https://github.com/lightdash/lightdash/blob/main/CLAUDE.md) menciona Monaco, cliente de consultas y paquetes de tipos compartidos.
- **Rescatar:** separación de frontend/API, gestión de carga/error/caché, editor SQL, esquema compartido.
- **No copiar:** React/Mantine/Node como requisitos ni contratos duplicados entre frontend y servidor.
- **Mejorar:** cliente generado desde OpenAPI y única lógica de negocio en Rust.

### 2.3 Grafana: extensibilidad visual y registro de capacidades

- [Tipos de plugins y navegación de Grafana](https://github.com/grafana/grafana/blob/main/packages/grafana-data/src/types/plugin.ts) y [Canvas plugin](https://github.com/grafana/grafana/blob/main/public/app/plugins/panel/canvas/plugin.json).
- **Rescatar:** registry de paneles con capacidades, configuraciones por módulo, aislamiento del plugin y navegación basada en permisos.
- **No copiar:** arquitectura React ni cargar JavaScript arbitrario de plugins sin aislamiento y permisos; los plugins de terceros requieren política de seguridad propia.
- **Mejorar:** tipo común `VizSpec` y adaptadores intercambiables con lifecycles verificables.

### 2.4 Tecnología verificable del ecosistema Svelte

| Herramienta | Evidencia de producto/documentación | Decisión |
|---|---|---|
| SvelteKit 3 | [Lanzamiento 01-10-2026](https://svelte.dev/blog/sveltekit-3-is-here) | adoptar y fijar patch probado |
| Svelte 5 | [Reactividad `$state`, `$derived`](https://svelte.dev/docs/svelte/%24derived) | sí, estado de UI |
| Tailwind CSS 4 | [`@theme` y variables CSS](https://tailwindcss.com/docs/theme) | sí, sin prohibir CSS nativo |
| Bits UI | [Primitives headless Svelte 5](https://bits-ui.com/docs/introduction) | sí donde ahorre accesibilidad |
| shadcn-svelte | [Código poseído y modificable](https://www.shadcn-svelte.com/docs) | recetas selectivas, no sistema central |
| TanStack Table | [Adapter Svelte, documentación v9](https://tanstack.com/table/latest/docs/framework/svelte) | piloto; validar v9 vs v8 por estabilidad |
| TanStack Virtual | [Svelte virtualization](https://tanstack.com/virtual/latest/docs/framework/svelte) | sí para grillas/matrices grandes |
| TanStack Query | [Adapter Svelte](https://tanstack.com/query/latest/docs/framework/svelte/overview) | por caso, no cache duplicada global |
| Apache ECharts | [Canvas y SVG](https://echarts.apache.org/handbook/en/best-practices/canvas-vs-svg/) | renderer predeterminado |
| Highcharts | [Licenciamiento de empresas](https://www.highcharts.com/blog/news/our-new-eula-makes-free-usage-clearer/) | plugin comercial separado |
| Apache Arrow JS | [`tableFromIPC`](https://arrow.apache.org/js/current/) | piloto de transporte columnar |
| Axum SSE | [API pública de `Sse`](https://docs.rs/axum/latest/axum/response/sse/) | eventos del servidor |

**Cautela de versiones:** no afirmar que las últimas versiones de todas estas bibliotecas son compatibles entre sí. Congelar lockfiles, configurar CI y construir una matriz de compatibilidad contra SvelteKit 3; TanStack Table v9 es reciente, por tanto la decisión de integrarlo exige prueba de estabilidad.

---

## 3. Dos modos de despliegue, con una recomendación inequívoca

### Modo A — recomendado al comienzo: frontend estático servido desde Rust

```text
Build-time: SvelteKit 3 + Vite → HTML shell + JS/CSS/asset bundles
Production:
  https://bi.example.test/             → HTML de Svelte
  https://bi.example.test/_app/...      → assets compilados versionados
  https://bi.example.test/api/v1/...    → Axum / Rust
  https://bi.example.test/api/v1/.../events → Axum SSE
  PostgreSQL/DuckLake                  → solo accesibles por backend
```

- `@sveltejs/adapter-static` con fallback SPA controlado; Rust, Nginx o reverse proxy puede servir el resultado del build.
- SvelteKit documenta que la SPA afecta SEO y rendimiento inicial. En una aplicación empresarial autenticada, sin indexación pública, ese coste **puede ser aceptable**, pero se valida con Lighthouse/Web Vitals y dispositivos reales; no ignorar la advertencia oficial.
- Node.js **no es proceso de servidor** en la instalación resultante; Vite/Node/npm/pnpm se utilizan para construir la UI y durante desarrollo.
- Configuración de SvelteKit 3 conforme a `vite.config.*` y documentación de su adapter, no copiar archivos `svelte.config.js` de proyectos 2025.
- Fallback SPA **solo** para rutas de navegación HTML conocidas; una ruta `/api/*` inexistente responde 404/405/Problem Details, nunca `index.html`.
- Assets con hash: `Cache-Control: public,max-age=31536000,immutable` si la infraestructura garantiza nombres content-addressed; HTML shell: caché corta/revalidación.
- Encabezados de seguridad y compresión adecuados; recursos privados nunca bajo el directorio público.

**Por qué escogerlo:** un proceso de negocio principal, menos despliegues, autenticación por cookie del mismo origen y menos puntos de fallo. Cumple la preferencia por minimalismo operativo sin sacrificar separación de código.

### Modo B — opcional: SvelteKit SSR/Node separado

```text
Web/SSR SvelteKit Node 3 → /api gateway → Rust Axum
                          session forwarding/OIDC
                                     ↓
                         Semantic + Query Serving
```

Justificado si los productos requieren renderizado inicial por usuario, páginas públicas SEO, middleware SSR complejo o composición BFF con auténtico valor. Si se adopta:

- Node **solo hace render/adapter y BFF selectivo**, jamás el dominio semántico.
- No crear un segundo login o roles en Node; validar identidad en Rust y propagar sesión por una frontera segura.
- Eludir doble fetching SSR+híbrido y evitar guardar datos de usuarios en variables globales compartidas por el servidor.
- Añade procesos, requisitos de monitoreo, latencia, despliegue y pruebas; no es el modo por defecto.

### Modo C — escritorio y embebido

No es otro backend: UI estática + backend Rust empaquetable / Tauri si hay necesidad real de escritorio, y `/embed/...` con permisos de recurso, framing/CSP y orígenes autorizados cuando se habilite embedding. No adoptarlo por anticipado.

---

## 4. Diseño físico del repositorio y build

**Estructura propuesta, independiente de nombres comerciales:**

```text
project/
  apps/
    web/                        # SvelteKit 3, TypeScript
      src/
        routes/                  # navegación y carga de páginas
        lib/
          app/                   # AppShell, navegación, comandos
          design/                # tokens, primitives, recipes
          features/
            sources/
            models/
            explore/
            pivot/
            canvas/
            visualization/
            sql-editor/
            agents/
            operations/
          data/                  # clients + tipos generados + request state
          auth/                  # UI sesión, nunca motor de autorización
          widgets/               # Viz adapters, grilla, editor, layout
      static/
      vite.config.ts
      package.json
  crates/
    api/                         # Axum, auth y transporte HTTP
    application/                 # casos de uso, consultas, recursos
    semantic/                    # M04 (ya diseñado)
    serving/                     # M05
    runtime/                     # M13, duckdb-rs/Polars/Arrow
    metadata/                    # M10
    security/                    # M11
    jobs/                        # M14
    observability/               # M15
    ai/                          # M16
  contracts/
    openapi/                     # especificación pública canónica
    fixtures/                    # requests/resultados tipados testables
  tests/
    contract/
    e2e/
    visual/
    performance/
```

**Esto es separación lógica orientativa, no orden de crear todos los crates en el primer commit.** Empezar con monolito Rust modular y extraer crates cuando añadan límites reales. La UI también debe comenzar con features en un solo paquete; no construir 20 librerías npm internas.

**Regla CI:** cambios de contrato Rust deben regenerar Typescript en el mismo PR, ejecutar diff, pruebas y bloqueo de incompatibilidad no versionada.

---

## 5. Cómo se desarrolla localmente y cómo se despliega

### Desarrollo (dos procesos, cero complejidad innecesaria)

```text
Puerto frontend Vite/SvelteKit  → http://localhost:5173/
Puerto Axum Rust                → http://127.0.0.1:8000/api/v1/
Vite proxy /api/v1              → backend local
UI en navegador                → requests /api/v1 (mismo origen aparente)
```

- Proxy Vite configurable solo en desarrollo; no copiar sus reglas como seguridad productiva.
- Una variable de entorno pública puede indicar **ruta relativa de API** `/api/v1`, no URL ni secretos del catálogo.
- Un script de desarrollo arranca web y Rust sin exigir Docker para Node. PostgreSQL sí debe estar disponible para catálogo DuckLake/metadata persistente según entorno.
- No exponer `duckdb-rs`, `:memory:`, archivos Parquet internos o cadenas de conexión a bundles JS.

### Producción

1. Construir y verificar bundle web; lockfile y hash de artefacto.
2. Compilar Rust backend; artefacto versionado y migraciones controladas.
3. Incorporar bundle web a imagen/instalación Rust o servirlo mediante proxy inverso con rutas públicas separadas.
4. Arrancar Rust con secretos externos y probar PostgreSQL/DuckLake/Parquet; verificar readiness del backend.
5. Exponer única entrada HTTPS; cookies seguras; registro/auditoría.
6. Probar rollbacks de frontend y API con la política de compatibilidad de contratos.

**No** enviar credenciales de PostgreSQL al frontend ni convertir el PostgreSQL del catálogo DuckLake en una API pública.

---

## 6. Un contrato HTTP para web, SDKs, integraciones y agentes

**M12 conserva la propiedad de OpenAPI; M17 define su ergonomía web y pruebas de uso.**

- Rutas estables `/api/v1/*`; nombres/versionado gobernados por M12.
- `operationId` y esquemas discriminados; ids opacos de recursos y revisiones.
- `application/json` para operaciones comunes; `application/problem+json` para errores; `application/vnd.apache.arrow.stream` para IPC donde se pruebe compatible.
- `ETag` + `If-Match` al editar recursos; revisiones inmutables al publicar.
- `Idempotency-Key` para operaciones como publicar, iniciar jobs y exportar, con retención y semántica documentada por M12.
- `X-Request-ID` o identificador trazable; no confiar en un ID de cliente como autoridad.
- `Cache-Control: private,no-store` para respuestas privadas por defecto; permitir caché controlada solo cuando M05/M11 aprueben una clave dependiente de actor, políticas, revisión semántica y snapshot.
- Generated SDK → frontend; no escribir `fetch` arbitrario con 15 formas de error distintas por módulo.

**El backend puede usar `utoipa`/`utoipa-axum` como apoyo a generación de esquema**, pero la adopción debe preservar las garantías de OpenAPI y una sola definición canónica. [Documentación de utoipa_axum](https://docs.rs/utoipa-axum/latest/utoipa_axum/).

### 6.1 Ejemplo de API (contrato ilustrativo, NO endpoints implementados)

| Método | Ruta propuesta | Uso |
|---|---|---|
| `GET` | `/api/v1/me` | sesión y capacidades permitidas |
| `GET` | `/api/v1/workspaces/{id}` | contexto workspace |
| `GET` | `/api/v1/semantic/models/{id}` | dimensiones y medidas permitidas |
| `POST` | `/api/v1/queries` | ejecutar/enqueue consulta semántica |
| `GET` | `/api/v1/queries/{id}` | progreso/estado/datos de procedencia |
| `GET` | `/api/v1/queries/{id}/result` | JSON o Arrow autorizado |
| `GET` | `/api/v1/queries/{id}/events` | estado SSE |
| `POST` | `/api/v1/queries/{id}/cancel` | cancelar consulta propia |
| `GET` | `/api/v1/dashboards/{id}` | definición publicada autorizada |
| `PATCH` | `/api/v1/dashboards/{id}/draft` | cambios en borrador con `If-Match` |
| `POST` | `/api/v1/dashboards/{id}/publish` | transición autorizada y auditable |
| `POST` | `/api/v1/exports` | trabajo de exportación M14 |
| `POST` | `/api/v1/ai/sessions/{id}/messages` | petición IA validada M16 |

**Prohibido:** rutas frontend que interpreten ids como SQL, construir parámetros mediante interpolación, o exponer conexiones DB directamente.

### 6.2 Ejemplo del request tipado (ilustración del contrato)

```json
{
  "model_id": "sales_fact",
  "measures": ["net_revenue"],
  "dimensions": ["year_month", "region"],
  "filters": {
    "op": "and",
    "items": [
      {"field": "year_month", "op": "gte", "value": {"type": "month", "value": "2026-01"}}
    ]
  },
  "order_by": [{"field": "year_month", "direction": "asc"}],
  "max_rows": 1500,
  "preferred_format": "json"
}
```

- Este DTO es **una representación de ejemplo**, no la definición oficial de M04; sus campos se deben reconciliar en M18 con el contrato semántico definitivo.
- El usuario web nunca remite atributos efectivos de `tenant_id`/`role` que puedan usarse como permisos: el backend los obtiene de identidad y políticas.
- `max_rows` del cliente es una **solicitud**, el servidor impone su propio límite menor si corresponde.
- `filters` representa filtros de negocio; **no puede borrar RLS**. Cambiar visualizaciones no cambia el cálculo de una medida.

### 6.3 Resultado con procedencia, esquema y precisión

```json
{
  "query_id": "qry_01",
  "status": "succeeded",
  "data_revision": "snap_0142",
  "semantic_revision": "sem_006",
  "schema": [
    {"name":"year_month", "logical_type":"month", "nullable":false},
    {"name":"net_revenue", "logical_type":"decimal(18,2)", "nullable":true}
  ],
  "rows": [
    {"year_month":"2026-01", "net_revenue":"1250432.75"}
  ],
  "row_count": 1,
  "truncated": false
}
```

El `DECIMAL` se envía como **cadena decimal canónica**, no `Number` de JavaScript a ciegas. Similarmente, `INT64` fuera de rango seguro, `TIMESTAMPTZ`, `DATE`, nulos y zonas horarias mantienen un contrato tipado según M12. Los valores/datos de este ejemplo son ficticios y no sirven de fixture contable real.

---

## 7. ¿JSON o Arrow? Separar control plane y data plane

| Flujo | Transporte inicial | Razón |
|---|---|---|
| Menús, workspaces, autorizaciones visibles | JSON | payload pequeño y fácil |
| Definiciones `VizSpec`, Canvas, Pivot | JSON | esquema versionado/configuración |
| KPI, series agregadas, tabla pequeña | JSON | menos coste de decodificación |
| Tabla/matriz con varios miles de filas y tipos importantes | Arrow IPC opcional | evita expansión textual y conserva esquema |
| Descarga masiva | job CSV/Parquet/Arrow | no bloquear UI ni inflar memoria |
| Eventos de progreso, IA, jobs | SSE (JSON por evento) | mensajes pequeños secuenciales |

**Apache Arrow es formato de buffers columnar, no motor de visualización.** DuckDB devuelve Arrow por backend; Axum serializa Stream IPC, y cliente web `apache-arrow` decodifica lotes. El propio renderer ECharts suele necesitar estructuras de series; **esa transformación puede copiar datos**. No prometer zero-copy end-to-end.

### 7.1 Política de negociación

- El API ofrece `Accept: application/json` o `Accept: application/vnd.apache.arrow.stream` en endpoints compatibles.
- Respuesta incluye `Content-Type`, versión de esquema, límites, timeout y snapshot.
- No habilitar Arrow por defecto para 3 puntos de KPI.
- Limitar bytes y memoria del navegador; `fetch`/streams requieren cancelación con `AbortController` y coordinación real en Rust.
- Para grandes tablas: paginación/virtualización por backend, streaming acotado, no `table.toArray()` de millones de filas.
- Archivos exportados usan jobs; el browser nunca debe almacenar todo el data lake.
- La ganancia real se determina con bench por tamaño de datos, overhead de decodificación, CPU web, memoria y compatibilidad Decimal/BigInt/Date.

### 7.2 Conversión Arrow hacia gráfico

```text
Arrow IPC bytes
    → Arrow JS Table / RecordBatch
    → ChartDataMapper (tipado, consciente de null/decimal)
    → ChartData (series limitadas, puntos ordenados)
    → VizSpec validator
    → EChartsAdapter.render/update
```

El gráfico no evalúa expresiones empresariales; formateadores, tooltips y cálculos visuales no sustituirán sumas, tasas, DISTINCT o totales semánticos. M09 tiene la última palabra sobre gramática visual.

---

## 8. Consultas largas, SSE y cancelación real

**Regla:** una pantalla no debe congelarse esperando silenciosamente un cálculo; tampoco el frontend puede ser el único que cancela trabajo nativo.

### 8.1 Máquina de estados

```text
created → queued → running → succeeded
                 ↘ failed
                 ↘ cancelled
                 ↘ expired
```

- `POST /queries` puede responder `200` con payload pequeño o `202` con `query_id`, conforme a M12. No cambiar semántica solo por tamaño del resultado de forma impredecible.
- `GET /queries/{id}` brinda estado durable y errores tipados.
- `GET /queries/{id}/events` usa SSE para cambios; heartbeat, reconexión, duración y control de recursos.
- SSE puede transportar `progress`, `status`, `warning`, `completed`, `failed`. Se ordenan por id monotónico por ejecución cuando el almacén de eventos garantice replay.
- El cliente no transforma cierre TCP en `succeeded`; confirma estado por API o evento terminal validado.
- `cancel` exige sesión, propiedad de query y operación autorizada; Rust propaga interrupción hacia M05/M13 cuando el driver lo permita.
- `Last-Event-ID` se usa **solo si** el backend implementa log reproducible; de otro modo al reconectar se consulta estado. No inventar entrega exactamente una vez.

### 8.2 SSE vs WebSocket vs polling

- **SSE:** mejor para `server → browser`, progreso y tokens de IA. Ejemplo API [Axum Sse](https://docs.rs/axum/latest/axum/response/sse/).
- **WebSocket:** sesiones bidireccionales de colaboración o presencia con negociación, heartbeat y autorización; postergado.
- **Polling:** respaldo robusto si SSE está bloqueado por proxy o no se necesita gran frecuencia.
- **Seguridad EventSource:** evitar tokens en query string, preferir cookie `HttpOnly` del mismo origen; la instancia estándar `EventSource` no admite cabeceras arbitrarias como un `fetch` común; verificar autorización al conectar y al transmitir/reconectar. [MDN EventSource](https://developer.mozilla.org/en-US/docs/Web/API/EventSource/withCredentials).

### 8.3 Actualización de dashboard por filtros

1. El usuario cambia `region` o fecha en la UI.
2. `FilterController` genera estado canónico para la página; URL opcionalmente guarda filtros compartibles *no sensibles*.
3. Las consultas afectadas reciben nueva huella; las anteriores se abortan/cancelan cuando tiene sentido.
4. El cliente solicita resultados via `SemanticQuery` con políticas del servidor.
5. Paneles muestran pending/partial/error sin bloquear otras visualizaciones.
6. ECharts recibe `ChartData` ya validado y actualiza su instancia sin recrearla si es seguro.
7. Cada respuesta incluye snapshot/revisión; no combinar panes de versiones incompatibles como un único resultado consistente sin indicarlo.

---

## 9. Autenticación, sesión, RLS y límites del navegador

**Decisión:** preferir HTTPS + cookie de sesión `HttpOnly`, `Secure` en producción y `SameSite` coherente con el flujo de identidad; OIDC/OAuth o SSO conectados por Rust M11. El navegador no conserva secretos de bases de datos ni refresh tokens en `localStorage` por defecto.

- La sesión es del backend. El frontend `GET /me` solo conoce principal y capacidades **para mostrar** UI. Ocultar un botón **no** es autorización.
- Cada llamada de API, incluidas AI/SSE/exportaciones/queries, vuelve a aplicar autenticación, ámbito y RLS/CLS. Un usuario restringido no puede obtener datos por autocomplete, caching compartido o descarga.
- Mutaciones con cookies exigen defensa CSRF (validación de origen y/o token anti-CSRF según amenazas) además de cookies `SameSite`; CORS por sí solo no impide CSRF.
- Cookies se restringen por `Domain`/`Path`; sesiones expiradas devuelven `401` y UI presenta re-login sin reintentos infinitos.
- `403` indica identidad conocida sin privilegio y conserva UI útil; jamás convertirlo automáticamente en "sin datos".
- Impedir XSS y ejecución de scripts dentro de HTML de tooltip/Markdown de IA; sanitizar, CSP, Trusted Types cuando sea viable, escapar valores.
- La identidad del usuario se deriva en el servidor, no de encabezados `X-User-ID` arbitrarios remitidos por el browser.
- Si el frontend SSR usa Node, reenviar auth solo bajo origen y política conocidos; no reimplementar controles de permisos en Node.
- Datos auditables: actor, acción, recurso, versión, resultado, hora, `request_id`; no registrar secretos en trazas M15.

**Contrato de seguridad:** una consulta por Explore, Pivot, Canvas, SQL editor, REST, MCP, IA, reportes o exportación ejecuta exactamente las mismas políticas de recurso/dato; solo varían las capacidades autorizadas explícitamente.

---

## 10. Gestión del estado Reactivo y sincronización

### 10.1 Cuatro tipos que deben estar separados

| Tipo de estado | Ejemplos | Dueño |
|---|---|---|
| UI efímero | sidebar, modal, foco, tamaños | `$state` por componente/feature |
| Navegación compartible | pestaña, modo, filtros simples | URL + estado de ruta |
| Estado de consulta de servidor | lista de modelos, resultados, loading, error | API client + cache (si conviene TanStack Query) |
| Estado de negocio durable | dashboard publicado, drafts, permisos, tareas | Rust + PostgreSQL |

- Usar runes Svelte 5 `$state` y `$derived` para derivaciones puras; `$effect` para efectos externos explícitos, no para crear cadenas de escritura reactiva sin control. [Svelte `$derived`](https://svelte.dev/docs/svelte/%24derived).
- En SSR no colocar información mutable de usuario en módulo global: riesgo de contaminación entre requests. [SvelteKit State Management](https://svelte.dev/docs/kit/state-management).
- Solo guardar en URL filtros compatibles con privacidad y longitud; tokens/sesión/queries sensibles no son parámetros públicos.
- `QueryCacheKey` debe incluir al menos recurso/revisión, query normalizada, contexto de actor/política o referencia opaca emitida por backend, snapshot de datos y formato. El frontend **no puede garantizar** por sí solo la autorización de la caché.
- Invalidación por eventos de publicación/refresco de M03/M05 y acciones explícitas; TTL como respaldo, no verdad única.
- `AbortController` evita actualizar vistas obsoletas; Rust decide cancelación efectiva de ejecución.

### 10.2 Orden de implementación

Comenzar con estado nativo Svelte, cliente fetch único y caché ligera para metadatos. Introducir TanStack Query donde concurrencia, invalidación, retries y stale data se vuelvan suficientemente complejos. Evitar apilar stores, RxJS, Zustand y TanStack para la misma entidad.

---

## 11. AppShell y experiencia de usuario completa

### 11.1 Estructura de producto

```text
Topbar: [Workspace]  Search / Command Palette      Jobs  Help  Profile
Sidebar:
  HOME        Overview / Favorites / Recent
  DATA        Sources / Catalog / Models / Lineage
  ANALYZE     Explore / SQL / Pivot
  BUILD       Dashboards / Reports / AI Assistant
  OPERATE     Job history / Freshness / Observability
  ADMIN       Members / Policies / Integrations

Content:
  Breadcrumbs → Page title → contextual actions
  Tabs/workspace → Filters/controls → main panel
  Optional inspector/right rail (fields, formatting, lineage)
  Bottom optional query/debug drawer for authorized users
```

- Navegación responde a capacidades permitidas, no reglas de nombres de roles en frontend.
- Deep links restauran recurso y contexto permitido; estados no autorizados retornan 403 o vista segura.
- Sidebar colapsable, densidad configurable, tabs con URL semántica, barra de comandos accesible.
- Editor de dashboard en modo `view` y `edit`; no presentar controles destructivos en lectura.
- Preservar scroll y selección al volver; no duplicar solicitudes ni disparar 12 queries al abrir menú.
- Layout responsive real: desktop administración completa; tablet consumo y edición limitada; móvil consumo simplificado, no forzar editor Canvas de 1920px en pantalla pequeña.

### 11.2 Ejemplo conceptual de áreas

| Área | Ruta ilustrativa | Experiencia |
|---|---|---|
| Inicio | `/w/:workspace/home` | salud de datos, dashboards recientes |
| Modelo | `/w/:workspace/models/:id` | campos, SQL/Polars, lineage, validación |
| Explore | `/w/:workspace/explore/:id` | dimensiones, medidas, comparación, drill |
| Pivot | `/w/:workspace/pivots/:id` | filas/columnas/totales/virtualización |
| Dashboard | `/w/:workspace/dashboards/:id` | view/edit, cross-filter, publicación |
| SQL | `/w/:workspace/sql` | editor avanzado autorizado |
| Agente | `/w/:workspace/assistant` | conversación y evidencias |
| Ops | `/w/:workspace/operations` | ejecuciones, freshness, errores |

**No convertir el módulo frontend en definición de productos M06–M09.** Cada feature implementa su propia lógica de UI; M17 gobierna navegación, estado transversal, comunicación, accesibilidad y diseño común.

---

## 12. Design System: premium, accesible y sin marca obligatoria

**Tokens de origen en CSS:** color, superficies, tipografía, altura de controles, densidad, radios, bordes, sombras, z-index, animación, foco, spacing, chart palette, semantic colors y breakpoints. Tailwind 4 consume tokens donde aporte productividad; no prohíbe CSS escrito a mano.

```css
/* CONTRATO DE TOKENS ILUSTRATIVO; no es tema final */
@import "tailwindcss";
@theme {
  --color-brand-500: oklch(0.58 0.17 245);
  --radius-panel: 0.75rem;
}
:root {
  --surface-canvas: #f7f8fa;
  --surface-panel: #ffffff;
  --text-primary: #17202c;
  --focus-ring: var(--color-brand-500);
  --density-row-height: 36px;
}
[data-theme="dark"] {
  --surface-canvas: #10151d;
  --surface-panel: #1a212b;
  --text-primary: #f3f5f8;
}
```

**Principios:**

- Priorizar HTML semántico, WCAG 2.2 AA, foco visible, navegación por teclado, etiquetas accesibles y alternativas textuales a información visual.
- Bits UI para primitives headless; shadcn-svelte solamente como base de componentes *propios*. No imponer plantilla visual ajena ni clonar todas las recetas.
- Estilos de errores, vacíos, cargando, sin acceso, sin conexión y consulta cancelada como ciudadanos de primera clase.
- Paleta de gráficos separada de estados operativos; significado por icono/texto, no solo color.
- Temas claro/oscuro y paletas accesibles probados mediante screenshots y contraste; `prefers-reduced-motion` para gráficos y componentes.
- Paneles densos y grandes con jerarquía visual consistente: contenido antes de decoración.
- Localización de fecha, número, moneda y unidades; formatos visuales no alteran valores crudos ni precisión fiscal.

### 12.1 Componentes de plataforma con una sola implementación

1. `AppShell`, `WorkspaceSwitcher`, `Breadcrumbs`, `CommandPalette`.
2. `DataState` (loading/empty/error/partial/retry) y `PermissionState`.
3. `FilterBar`, `DateRange`, `FieldPicker`, `DimensionMemberSelector`.
4. `ResourcePicker`, `SaveDraft`, `PublishDialog`, `RevisionDiff`.
5. `ResultsTable`, `VirtualGrid`, `DataTypeCell`, `PrecisionFormatter`.
6. `VizHost`, `ChartToolbar`, `PanelInspector`, `ExportMenu`.
7. `QueryRunPanel`, `JobTimeline`, `ErrorDetails`.
8. `AiChatPanel`, `EvidenceCard`, `ApprovalDialog`.

La lista es **inventario de componentes**, no obligación de construir todo antes del primer dashboard funcional.

---

## 13. Explore, Pivot, Canvas y Visualization: integración sin duplicar motores

### M06 Explore

- `ExploreState` incluye selección de medidas/dimensiones, filtros, orden y vistas guardadas.
- La API convierte la selección en `SemanticQuery`; la tabla y el gráfico muestran la **misma respuesta autorizada**.
- Drill-down recalcula en backend; no usar datos agregados locales para fingir un drill arbitrario.

### M07 Pivot/Matrix

- TanStack Table/Virtual o grilla propia controlan viewport; Rust + M04 hacen pivote y totales con granularidad correcta.
- Virtualización de filas/columnas, árbol de jerarquías, scroll sincronizado, subtotales/sticky headers, exportación servidor.
- El frontend no suma porcentajes/ratios por su cuenta: totales recalculados conforme a M04.

### M08 Canvas

- Modelo `Dashboard → Pages/Tabs → Layout → Panels → Bindings/Interactions`.
- Separar `published definition`, `draft editing` y `ephemeral interaction state`.
- Drag/drop, resize, guides, undo/redo en estado temporal; publicar bajo `ETag`/revisión.
- Persistir referencias a `SemanticQuery`/`VizSpec`, **no** instancias ECharts ni estado DOM.
- Efectos entre paneles expresados como eventos tipados (`FilterChanged`, `DrillRequested`, `SelectionChanged`); policy M11 reaplicada.

### M09 Visualization

- `VizSpec` neutral y versionado; `VizRegistry` mapea tipo de gráfico → `EChartsAdapter` y editores de configuración.
- Render adapter define `mount`, `update`, `resize`, `destroy`, `exportCapabilities`, `events`.
- `ResizeObserver` con debounce/coalescing y limpieza al desmontar; no crear charts acumulativos tras navegar tabs.
- ECharts Canvas vs SVG se elige por perfil del gráfico, accesibilidad/impresión y benchmark, no por una regla absoluta.
- No permitir opciones arbitrarias que introduzcan HTML/JS ejecutable; validar overrides contra esquema y capacidades del plugin.
- Alt-text y tabla alternativa para accesibilidad de contenidos BI importantes.

**Regla:** `SemanticQuery` produce resultado, `ChartDataMapper` adapta buffers y `VizSpec` define intención visual; `ECharts` pinta. Ninguna capa debería mezclar los tres modelos.

---

## 14. SQL Editor profesional: sin abrir un agujero de seguridad

- Editor principal: Monaco si se demuestra bundle, accesibilidad y licencia compatibles; alternativa CodeMirror 6 si entrega UX/rendimiento superiores en SvelteKit 3.
- SQL DuckDB de primer nivel para profesionales: autocompletado desde catálogo permitido, snippets, lint, formato, explicación, parámetros y resultados.
- Polars existe como otra modalidad de transformación respaldada por M02/M13; no prometer que código Polars Rust arbitrario se ejecuta en la sesión del navegador.
- M04 y M11 gobiernan recursos semánticos; el modo SQL avanzado puede tener un perímetro de permisos/aislamiento específico. **Nunca confiar en un parser SQL frontend para seguridad.**
- SQL ejecutado por usuarios no confiables necesita sandbox físico/de proceso y cuotas; un `SELECT` puede invocar funciones, extensiones u operaciones costosas según configuración. Los permisos de motor, filesystem, red y recursos se comprueban en el backend.
- Preparación y preview con `EXPLAIN` controlado; no ejecutar `EXPLAIN ANALYZE` automáticamente pues puede correr trabajo real.
- Botón `Run` inicia job, `Cancel` llama endpoint, historial con política de retención y ocultación de SQL sensible, tabs con resultados desacoplados del editor.
- No convertir SQL libre en única forma de construir dashboards: Explore/Canvas también deben ser first-class para usuarios no técnicos.

---

## 15. UX de IA y agentes (integración con M16)

- La IA aparece como panel lateral contextual en Explore/Canvas y como página de trabajo especializada.
- Entrada: pregunta, contexto semántico permitido y recurso explícito. Rust resuelve identidad, metas, políticas y herramientas.
- Salida tipada: explicación, consulta propuesta, evidencia, enlaces de procedencia, `VizSpec` sugerido o **cambio de borrador** sujeto a revisión.
- Streaming de tokens/progreso por SSE; evento terminal verificado, reconexión, cancelación y costes.
- Edición propuesta de dashboard incluye diff y confirmación; agente no publica ni distribuye sin permiso/flujo M14.
- Mostrar siempre qué consulta y snapshot respaldan cifras; no narrar valores fabricados por el modelo.
- El cliente no recibe claves de modelos externos ni las adjunta a prompts visibles; proveedores configurados por backend.
- Prompt injection se trata como datos no fiables; el frontend muestra respuestas saneadas y no ejecuta scripts retornados.

---

## 16. Manejo de errores y resiliencia end-to-end

**Una pantalla debe distinguir:**

- `401` sesión expirada → volver a autenticar.
- `403` sin permisos → explicar acceso denegado, nunca mostrar datos previamente cacheados de otro contexto.
- `404` recurso no existe/no visible → decisión de política para evitar enumeración.
- `409` conflicto de edición/publicación → merge/reload de draft, preservar cambios locales.
- `412` precondición fallida (`If-Match`) → diff y resolución de versión.
- `422` consulta inválida → señalar campo/operador con mensajes tipados.
- `429` cuota → backoff basado en `Retry-After`, no loops en UI.
- `499`/cancelled (si es código interno/proxy) → contrato estable de estado, no atribuir semántica estándar a 499.
- `5xx` fallo transitorio → error y `request_id`, reintento **solo** en operaciones idempotentes.
- Arrow incompatible/decodificación → error tipado y fallback JSON si endpoint lo garantiza; no ocultar corrupción de datos.

**Problem Details** según [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) cuando lo adopte M12; cada error ofrece código de máquina, descripción segura, `request_id`, `retryable`, `field_errors` cuando aplica. Frontend jamás muestra stacktraces internos o DSNs.

---

## 17. Rendimiento e interacción a escala BI

### 17.1 Prohibiciones de producto

- No cargar un millón de filas como un array JS para una tabla paginada.
- No mantener múltiples copias JSON + Arrow + ECharts + tabla de un mismo lote sin presupuesto conocido.
- No reconsultar todo por cambiar el ancho del sidebar.
- No renderizar un componente ECharts por cada fila de una tabla.
- No usar polling cada 100 ms para todas las queries.
- No persistir un snapshot completo en `localStorage` por conveniencia.

### 17.2 Técnicas permitidas por evidencia

- Code splitting y rutas lazy; librerías pesadas (Monaco, mapas, Canvas editor) se cargan por feature.
- Virtualización de filas y columnas; page requests de servidor con orden estable y cursor.
- Windowing de resultados y decimación/proyección cuando la intención visual lo permita; preservar agregados correctos.
- `ResizeObserver`, cleanup de listeners, streaming de resultados con cancelación.
- Workers web para decodificación/proyecciones extensas cuando benchmarks evidencien menor bloqueo; no poner allí el motor de seguridad ni prometer transferencia sin copia universal.
- `IntersectionObserver` para evitar renderizar paneles que no están visibles si no necesitan refresco activo.
- Debounce de inputs y cambios rápidos, sin pérdida de accesibilidad.
- Perfil de render `mount/update/resize`, memoria pico, tamaño de JS, latencia del waterfall API/Arrow/ECharts.

### 17.3 Objetivos orientativos de rendimiento (NO medidos)

| Experiencia | Objetivo de presupuesto inicial | Prueba que debe demostrarlo |
|---|---|---|
| Navegación de shell cacheado | respuesta visual ágil, sin pantalla blanca | RUM / Web Vitals / perfil móvil |
| Cambio de filtro | feedback visible inmediato | medición UI y telemetría end-to-end |
| Dashboard de KPI | priorizar respuesta p95 y cambios consistentes | 1/5/20 usuarios, cold/warm |
| Matriz con gran cardinalidad | scroll estable sin cargar todas las filas | benchmark virtualizado |
| Cambio de tema | no reconsulta backend innecesaria | inspección de red y FPS |
| Sesión larga de análisis | memoria estable y charts liberados | heap snapshots/repetición de rutas |

No prometer umbrales exactos antes de medir hardware, datasets, navegador, red y query real. M15 es dueño de SLO y benchmarking; M17 aporta métricas web y pruebas por interfaz.

---

## 18. Accesibilidad y localización profesional

- Conformidad objetivo: WCAG 2.2 AA; WCAG es referencia, no garantía de cumplimiento sin auditoría.
- Navegación por teclado en Canvas y Pivot; atajos anunciados, desactivables si interfieren.
- Focus management correcto tras diálogos, rutas, paginación y paneles; tabindex coherente.
- `aria-live` para finalización/cancelación, evitando narración excesiva de streaming.
- Colores de estado y tooltips acompañados por texto y patrones; tabla alternativa para gráficos esenciales.
- Soporte de preferencias del sistema: claro/oscuro, zoom 200–400%, reduced motion, alto contraste donde aplique.
- Número, fecha, decimales, zonas horarias, moneda y unidades formateados por contexto, con valores sin redondeo interno.
- Traducciones y copy de errores centralizados, no incrustados dentro de decenas de widgets.
- Escenarios móviles/tablet y desktop con pruebas reales, especialmente el editor de dashboards con drag/resize.

---

## 19. Flujos integrales que deben funcionar antes de declarar integración terminada

### Flujo A: Dashboard normal

```text
Browser GET /dashboards/:id
  → Rust sesión/autorización
  → definición versión publicada M08/M10
  → Svelte distribuye paneles según layout
  → cada panel emite SemanticQuery normalizada
  → Rust M04/M11 comprueban contrato y política
  → M05 atiende caché/admission/concurrencia
  → M13 ejecuta DuckDB sobre snapshots DuckLake
  → API retorna JSON/Arrow tipado + revisión
  → ChartDataMapper → M09 VizSpec → ECharts
  → filtros cruzados repiten solo consultas afectadas
```

### Flujo B: Pivot de alta cardinalidad

```text
Pivot UI selecciona filas/columnas/medidas
  → POST query tipada + ventana visible
  → backend calcula jerarquía, subtotales y orden
  → devuelve batch/página Arrow o JSON
  → virtual grid pinta SOLO viewport
  → scroll demanda siguientes bloques estables
  → export total usa job de servidor
```

### Flujo C: SQL avanzado

```text
SQL Editor → Run
  → Rust valida identidad, scope y política SQL
  → budget/admission/aislamiento apropiado
  → DuckDB proceso controlado y consultas autorizadas
  → 200 pequeño o 202 job
  → SSE/poll estado; cancelación verificable
  → resultado tipado, esquema, snapshot, perfil autorizado
```

### Flujo D: Copiloto conversacional

```text
Pregunta → API Rust/M16 → herramientas autorizadas
  → M04 plan semántico → M05 ejecución
  → respuesta con evidencia y VizSpec sugerido
  → UI presenta gráfico propuesto / explicación
  → editar/persistir solo tras decisión del usuario
```

### Flujo E: Publicación concurrente de Canvas

```text
A y B editan mismo dashboard
  → ambos basados en revisión 14
  → A publica revisión 15
  → B intenta If-Match:14
  → backend 412 + revisión nueva
  → UI ofrece diff/merge, no pisa A
  → revalidación seguridad y publicación aprobada
```

---

## 20. Observabilidad frontend y experiencia de errores (M15)

- Una navegación y cada consulta llevan `request_id`, `traceparent` cuando aplique y correlación con run/query.
- Medir latencia UI, networking, JSON/Arrow decode, ChartDataMapper, ECharts mount/update, DOM paint y p95 de cambio de filtro.
- Traces/sessions jamás incluyen valores sensibles ni SQL crudo por defecto.
- Errores JS tienen fingerprint y source maps restringidos; no enviar source maps de producción públicamente si exponen fuente o secretos.
- Eventos UI con cardinalidad controlada: tipo `filter_change`, `query_loaded`, `chart_render_failed` sin IDs de cliente como labels de Prometheus.
- Collector caído no impide guardar dashboard ni atender datos; códigos de error de producto no dependen de observabilidad.

---

## 21. Arquitectura de plugins, extensiones y personalización

**Fase inicial:** registries de gráficos **internos** (estáticos). La extensibilidad externa tiene riesgos que requieren una decisión futura, no introducir carga de scripts remotos desde el inicio.

- `VisualizationDescriptor`: nombre, versión, capacidades, esquema de opciones, soporte export/accessibility.
- Renderer ECharts base; Highcharts solo plugin bajo licencia adecuada; posibilidad futura D3/MapLibre si el caso lo requiere.
- `VizSpec` no guarda opciones internas literales de Highcharts/ECharts salvo secciones de override sanitizadas y versionadas por proveedor.
- CSS / tokens del producto tienen temas versionables por tenant, evitando CSS inseguro arbitrario importado desde UI.
- Embedding externo exige contexto de identidad/recurso/expiración, no `iframe` público con un ID adivinable.
- Un plugin de tercero con JS ejecutable requiere aislamiento, permisos, firma, CSP, control de salida/red y auditoría; **no asumir aislamiento porque está dentro de un componente Svelte**.

---

## 22. Antipatrones y decisiones que se descartan

| Antipatrón | Motivo de rechazo | Alternativa |
|---|---|---|
| React adicional para gráficos principales | duplica frameworks y estado | ECharts adapter Svelte |
| Backend Node que vuelve a calcular métricas | divergencia semántica | Rust M04/M05 |
| Browser conectado a PostgreSQL/DuckLake | fuga de credenciales/políticas | HTTP API autorizada |
| Todo en JSON aunque sea matriz columnar grande | bytes, tipos y memoria | Arrow bajo umbral medido |
| Todo en Arrow aunque sea un menú | overhead innecesario | JSON pequeño |
| WebSocket para una consulta | complejidad y autorización | HTTP+SSE |
| `localStorage` para access/refresh tokens | superficie de XSS | cookie `HttpOnly` de servidor |
| Estado mutable global en SSR | cruza sesiones | scope por request/usuario |
| `SELECT *` masivo para ChartData | bloqueo UI | agregación servidor |
| SQL que IA ejecuta como root | RLS y seguridad comprometidas | plan autorizado y sandbox |
| Highcharts por defecto en núcleo libre | coste/licencia comercial | ECharts Apache 2.0 |
| GraphQL sin requisito validado | mayor complejidad y superficie | OpenAPI HTTP |
| Copiar `svelte.config.js` de Kit 2 sin revisar | incompatibilidad Kit 3 | config oficial Kit 3 |
| Crear paquete npm por botón | burocracia y releases extra | features co-localizadas |

---

## 23. Contratos definitivos que M17 pide a módulos vecinos

| Módulo | Entrada que M17 consume | Evento/resultado que M17 necesita |
|---|---|---|
| M04 Semantic | modelos, campos, semántica, filtros tipados | validación, contexto, gramática de consulta |
| M05 Serving | query normalizada, revision/actor context | query ID, estado, paginado, formatos, cancel |
| M06 Explore | estado de análisis y presets | cambios de filtro/drill y referencia guardada |
| M07 Pivot | layout/ejes + viewport | matrix typed windows/totals |
| M08 Canvas | draft/published, layout, bindings | revisión, publish, conflicts |
| M09 Visualization | `VizSpec`, capabilities, edits | render adapter/lifecycle/export |
| M10 Metadata | recursos versionados, ownership | catálogo, referencias y comparaciones |
| M11 Security | actor, permisos efectivos | authorize; security errors |
| M12 APIs | OpenAPI, error envelope, content types | TS generated client y contract test |
| M13 Runtime | Arrow batch, DuckDB errors y metrics | tipado y streaming controlado |
| M14 Automation | run/job IDs, approval/delivery | progression y estado durable |
| M15 Operations | correlation/traces/SLO | métricas UI y diagnóstico autorizado |
| M16 IA | prosa, plan, evidencia y sugerencias | SSE, approvals, diff y provenance |

Los M01–M09 actuales contienen material relevante pero algunos documentos antiguos todavía nombran SQLVIZ/Quack como requisitos. **M17 no los hereda**; M18 deberá reconciliarlos contra los contratos aprobados y producir versiones agnósticas finales.

---

## 24. Estrategia de pruebas del frontend y de su conexión con Rust

Cuatro niveles, con casos y fixtures de negocio idénticos:

1. **Component/unit:** reactividad, filtrado local, formatters, widgets, focus y lifecycle.
2. **Contract:** OpenAPI generated TS ↔ Rust DTO; JSON Schema, Problem Details, Decimal/Arrow e IPC.
3. **Integration:** Rust + PostgreSQL/DuckLake/fixtures; M04/M05/M11 y SSE/cancel/publish.
4. **E2E:** Playwright o equivalente para navegación, filtro, tabla, Canvas, jobs, seguridad, móvil, accesibilidad; benchmarks y snapshots visuales.

### A. Contratos, tipos y formatos (01–15)

01. Generación de types desde OpenAPI no introduce `any` en contratos críticos.
02. DTO `SemanticQuery` válido para Explore, Canvas y Pivot.
03. Campos desconocidos/operadores inválidos fallan validación Rust.
04. `DECIMAL(18,2)` exacto en JSON y Arrow.
05. `INT64` mayor que `Number.MAX_SAFE_INTEGER` no redondea en JS.
06. Fechas, zonas horarias y nulos conservan significado.
07. `429` preserva `Retry-After` y errores estables.
08. `401` no se traduce a tabla vacía.
09. `403` nunca revela metadatos no autorizados.
10. Error `application/problem+json` muestra código y request id.
11. `ETag` y `If-Match` controlan versión de borrador.
12. Resultado Arrow equivale numéricamente al JSON del mismo snapshot.
13. Un Arrow stream truncado se detecta, no muestra éxito falso.
14. Versión semántica y snapshot están presentes en provenance.
15. OpenAPI/TypeScript y Rust pasan test de compatibilidad en CI.

### B. Autenticación y aislamiento (16–30)

16. UI no contiene secrets ni cadenas PostgreSQL en bundle.
17. Cookie HttpOnly/Secure correctamente configurada en producción.
18. Mutación cross-origin sin protección CSRF falla.
19. Usuario A no accede a query_id del usuario B.
20. SSE reconectado comprueba identidad/autorización.
21. `GET /me` con sesión expirada muestra re-login.
22. RLS se aplica a Explore, Pivot, Canvas, SQL, AI y export.
23. `REMOVEFILTERS` de negocio no elimina RLS.
24. Cambiar filtro en URL no inyecta tenant ni rol.
25. Cache data de A no persiste en B tras logout/login.
26. Autocomplete de dimensiones no revela miembros prohibidos.
27. Preview de dashboard no salta permisos de versión.
28. CSRF/XSS y HTML de tooltips están cubiertos por tests negativos.
29. Sin permisos de publicación, API rechaza publish aunque UI muestre botón.
30. El frontend no puede generar URLs de exportación públicas arbitrarias.

### C. Navegación, diseño y accesibilidad (31–45)

31. Deep link a dashboard preserva revisión/contexto autorizado.
32. Ruta `/api` desconocida devuelve 404, no SPA shell.
33. Sidebar colapsable no modifica consultas.
34. Back/forward mantiene filtros compartibles.
35. Theme toggle no remonta 20 consultas ni duplica chart instances.
36. Contraste de texto/controles en temas claros/oscuros.
37. Focus visible en menú, modal, chart toolbar y Pivot.
38. Keyboard-only permite filtros, guardar y publicar.
39. Resize de navegador libera y reconfigura ECharts.
40. Zoom 200% evita corte de controles esenciales.
41. Reduced motion desactiva animación innecesaria.
42. Responsive tablet conserva consumo de dashboards.
43. Vacío/cancelado/error distingue sus estados.
44. Textos i18n cambian sin perder cifras originales.
45. Tooltip y tabla accesible no dependen solo de hover.

### D. Explore, Pivot, Canvas y gráficos (46–65)

46. Selección dimensión/medida construye una consulta válida.
47. Explore tabla y gráfico devuelven mismos agregados.
48. Drill-down respeta grain/contexto.
49. Matriz devuelve totales correctos para ratios no aditivos.
50. Virtual grid no carga millones de filas en el DOM.
51. Scroll cambia viewport/cursor sin duplicar datos.
52. Dashboard usa `VizSpec` neutral.
53. Cambiar EChartsAdapter no exige cambios en SemanticQuery.
54. Paneles muestran carga progresiva y error independiente.
55. Cross-filter solo invalida paneles dependientes.
56. Editor Canvas guarda draft sin publicar involuntariamente.
57. Dos editores concurrentes producen conflicto controlado.
58. Undo/redo no escribe snapshots de DB por cada pixel.
59. ECharts se destruye al desmontar/navegar.
60. 100 ciclos mount/destroy no exhiben fuga de instancias.
61. Gráfico de DECIMAL no altera cálculo contable.
62. ChartDataMapper respeta orden, nulos y timezone.
63. Render SVG/Canvas funciona según capacidad declarada.
64. Export visual no revela datos excluidos por política.
65. El editor de propiedades invalida overrides incompatibles.

### E. Red, streaming, fallos y concurrencia (66–80)

66. POST query pequeña entrega JSON correcto.
67. Query larga entrega `202` y estado reproducible.
68. Eventos SSE exponen `run_id/query_id` correctos.
69. Cierre de EventSource no equivale a éxito.
70. Reconexion SSE reconsulta estado y no duplica publicación.
71. Cancel request llega a coordinador Rust/driver.
72. Timeout HTTP no deja job incontrolado.
73. `AbortController` evita pintar respuesta obsoleta.
74. Error de Arrow IPC no se silencia como vacío.
75. Red lenta presenta feedback, sin bloquear shell.
76. API caído mantiene mensaje claro y recuperación manual.
77. Retry de lectura no duplica mutaciones.
78. Refresh de snapshot invalida paneles apropiados.
79. Dos pestañas no producen sesiones cruzadas.
80. Worker/join/transformación pesada no bloquea loop Tokio del API.

### F. IA, operación y rendimiento (81–95)

81. Agente solo ve herramientas y campos autorizados.
82. AI stream está vinculado a sesión/capacidad/cancelación.
83. Propuesta de cambio AI necesita aprobación y diff.
84. Respuesta IA muestra query/snapshot/evidencias si presenta valores.
85. Prompt malicioso no ejecuta HTML/JS en renderer.
86. OpenTelemetry correlaciona UI request con Rust query.
87. Collector caído no afecta al usuario final.
88. Error ID diagnostica backend sin revelar SQL secreto.
89. Bundle inicial no carga Monaco/Canvas editor si no se visita.
90. Filtro rápido no produce N requests inútiles sin límite.
91. Matriz scroll perfilada en memoria y latencia.
92. ECharts recibe número acotado de puntos según visualización.
93. p95 del dashboard separa backend, red y render.
94. Carga fría/caliente con 1/5/20 usuarios se mide en CI/bench reproducible.
95. Resultado autorizado se compara con oráculo SQL antes de optimizar latencia.

**Condición global:** seguridad + exactitud semántica prevalecen sobre velocidad. Un test E2E visual que “pinta bonito” no prueba cifras correctas.

---

## 25. Plan de entrega por cortes verticales, sin burocracia

**Corte 1: producto mínimo con identidad y una métrica**

- Rust/Axum con OpenAPI y sesión básica robusta.
- SvelteKit 3 AppShell, themes, navegación, control de error.
- `GET /me`, lista de modelos, `POST /queries`, KPI/gráfico ECharts.
- Fixture DuckLake/PostgreSQL/Parquet y prueba de precisión.
- Pruebas contractuales/E2E desde este corte.

**Corte 2: Explore y tablas**

- Selección semántica, filtros, drill, JSON con procedencia.
- Virtualización y piloto Arrow donde exista ganancia.
- Debounce, request cancellation, métricas p95.

**Corte 3: Canvas y Pivot**

- Grilla, matriz con totales servidor, drafts, publicación versionada.
- Layout de dashboard, cross-filter, M09 renderer.

**Corte 4: profesional y operativo**

- SQL editor sujeto a sandbox/políticas, job progress SSE, exportaciones.
- Observabilidad útil para usuario y operador.

**Corte 5: IA y expansiones**

- Conversational BI, herramientas M16, evidencias y aprobación.
- SSR opcional, plugins comerciales, colaboración en tiempo real **solo si su valor está probado**.

La numeración de cortes es orden **de implementación orientativo**, no creación de nuevos módulos arquitectónicos ni obligación de implementar todos simultáneamente.

---

## 26. Riesgos y medidas preventivas

| Riesgo | Impacto | Mitigación |
|---|---|---|
| Adoptar SvelteKit 3 sin probar librerías | incompatibilidades | matriz y PoC antes de fijar UI stack |
| SPA sin optimizar shell | rendimiento inicial | bundles lazy, SSR solo donde amerite |
| SSR Node duplicando semántica | dos verdades de negocio | contrato Rust único |
| Falta de schema contract test | ruptura silenciosa | OpenAPI source + generated TS CI |
| Arrow indiscriminado | mayor coste para KPIs | JSON por defecto, Arrow por benchmark |
| ECharts bajo filtrado muy rápido | instancias/cpu | diffs, throttling, cleanup |
| Filtrar solo visualmente | exposición de datos | M11 en cada consulta/export |
| Exposición de SQL sin aislamiento | acceso filesystem/CPU | sandbox y capabilities |
| `localStorage` con credenciales | XSS/secuestro | sesión HttpOnly |
| WSS sin necesidad | complejidad de operaciones | SSE + HTTP primero |
| Drag/drop difícil en accesibilidad | usuarios excluidos | edición por teclado/formularios |
| Cierre de sesión deja caché | fuga entre cuentas | limpiar estado cliente y validar servidor |
| Versiones de dataset inconsistentes | KPIs contradictorios | snapshot/revision y provenance |
| Highcharts incluido por defecto | problemas de licencia | ECharts OSS, plugin explícito |
| Implementar todo el roadmap de entrada | producto no validado | vertical slices con pruebas |

---

## 27. Decisiones que deben quedar escritas para M18

| ID | Decisión | Estado |
|---|---|---|
| M17-ADR-01 | SvelteKit 3 + Svelte 5 + TypeScript | recomendada |
| M17-ADR-02 | Rust/Axum backend único de negocio | recomendada |
| M17-ADR-03 | producción inicial SPA assets estáticos vía Rust | recomendada, benchmark UX pendiente |
| M17-ADR-04 | SvelteKit SSR Node no obligatorio | aceptada |
| M17-ADR-05 | OpenAPI + generated TS contract | recomendada |
| M17-ADR-06 | JSON simple; Arrow IPC selectivo | recomendada, benchmark pendiente |
| M17-ADR-07 | SSE para progreso; WebSocket condicional | recomendada |
| M17-ADR-08 | cookie backend HttpOnly/Secure + defensa CSRF | recomendada, diseñar flujo de identidad |
| M17-ADR-09 | ECharts renderer inicial | recomendada |
| M17-ADR-10 | Highcharts plugin opcional y sujeto a licencia | aceptada |
| M17-ADR-11 | tokens CSS + Tailwind 4 + Bits UI selectivo | recomendada |
| M17-ADR-12 | TanStack Table/Virtual evaluado, v9 según compatibilidad | piloto pendiente |
| M17-ADR-13 | `VizSpec` separado de `ChartData`/`SemanticQuery` | heredada M09, ratificada |
| M17-ADR-14 | no SQL sin perímetro de ejecución y autorización | heredada M11/M13, ratificada |
| M17-ADR-15 | M06–M09 ownership de lógica de producto | ratificada |

**Bloqueos deliberados para M18:** fijar versiones exactas compatibles, determinar un único esquema formal de filtros/revisiones/formatos con M04/M05/M12, diseñar el OIDC/session flow definitivo y validar modo SPA frente a SSR con un prototipo vertical real. No presentar esos pendientes como “ya verificados”.

---

## 28. Fuentes primarias consultadas (con propósito)

### Framework y frontend

1. [SvelteKit 3 — lanzamiento del 1 de octubre de 2026](https://svelte.dev/blog/sveltekit-3-is-here).
2. [SvelteKit 3 — migración](https://svelte.dev/docs/kit/migrating-to-sveltekit-3).
3. [SvelteKit — static adapter](https://svelte.dev/docs/kit/adapter-static).
4. [SvelteKit — SPA](https://svelte.dev/docs/kit/single-page-apps).
5. [SvelteKit — Node adapter](https://svelte.dev/docs/kit/adapter-node).
6. [SvelteKit — estado](https://svelte.dev/docs/kit/state-management).
7. [SvelteKit — autenticación](https://svelte.dev/docs/kit/auth).
8. [Svelte 5 — $derived](https://svelte.dev/docs/svelte/%24derived).
9. [Tailwind CSS 4 — theme variables](https://tailwindcss.com/docs/theme).
10. [Bits UI — primitives](https://bits-ui.com/docs/introduction).
11. [shadcn-svelte — code ownership](https://www.shadcn-svelte.com/docs).
12. [TanStack Table Svelte](https://tanstack.com/table/latest/docs/framework/svelte).
13. [TanStack Virtual Svelte](https://tanstack.com/virtual/latest/docs/framework/svelte).
14. [TanStack Query Svelte](https://tanstack.com/query/latest/docs/framework/svelte/overview).

### Transporte, render y backend

15. [Axum — SSE](https://docs.rs/axum/latest/axum/response/sse/).
16. [Apache Arrow JavaScript](https://arrow.apache.org/js/current/).
17. [Apache ECharts — Canvas y SVG](https://echarts.apache.org/handbook/en/best-practices/canvas-vs-svg/).
18. [Highcharts — EULA/comercial](https://www.highcharts.com/blog/news/our-new-eula-makes-free-usage-clearer/).
19. [tower-http ServeDir](https://docs.rs/tower-http/latest/tower_http/services/struct.ServeDir.html).
20. [utoipa_axum — documentar Axum con OpenAPI](https://docs.rs/utoipa-axum/latest/utoipa_axum/).
21. [RFC 9457 — HTTP Problem Details](https://www.rfc-editor.org/rfc/rfc9457).
22. [MDN EventSource credentials](https://developer.mozilla.org/en-US/docs/Web/API/EventSource/withCredentials).

### Referentes BI reales

23. [Rill repo — web-local/web-admin/web-common](https://github.com/rilldata/rill).
24. [Lightdash repo — stack real](https://github.com/lightdash/lightdash).
25. [Lightdash developer architecture](https://github.com/lightdash/lightdash/blob/main/CLAUDE.md).
26. [Grafana panel plugin contract](https://github.com/grafana/grafana/blob/main/packages/grafana-data/src/types/plugin.ts).
27. [Grafana Canvas plugin](https://github.com/grafana/grafana/blob/main/public/app/plugins/panel/canvas/plugin.json).

### Documentos de arquitectura propios que deben reconciliarse

- `BI_Modulo_10_Metadata_Architecture_Agnostica_2026-10-10.md`.
- `BI_Modulo_11_Identity_Security_Governance_Agnostico_2026-10-10.md`.
- `BI_Modulo_12_APIs_Integraciones_Ecosistema_Agnostico_2026-10-10.md`.
- `BI_Modulo_13_Runtime_Analitico_Rust_DuckDB_DuckLake_PostgreSQL_Polars_ADBC_2026-10-10.md`.
- `BI_Modulo_14_Automation_Scheduling_Alerts_Reporting_Agnostico_2026-10-10.md`.
- `BI_Modulo_15_Observabilidad_Rendimiento_Operacion_Agnostico_2026-10-10.md`.
- `BI_Modulo_16_IA_Agentes_BI_Conversacional_Agnostico_2026-10-10.md`.
- Documentos originales M06 Explore, M07 Pivot, M08 Canvas y M09 Visualization: aportan diseño técnico, **pero no deben tomarse como ya normalizados** si aún citan SQLVIZ, SQLite, `.sqlviz` o Quack obligatorio.

---

## 29. Conclusión y criterio de cierre

La integración definitiva del producto no es “conectar Svelte a DuckDB”. Es:

**SvelteKit 3 → HTTP tipado (OpenAPI) → Rust/Axum → políticas + semántica + serving → DuckDB/DuckLake → JSON/Arrow → adapters de visualización en Svelte.**

El diseño permite un producto BI completo con SQL para expertos y Explore/Canvas/IA para no técnicos, sin obligar a inventar un nuevo lenguaje ni a desplegar Node.js como segundo servidor de negocio.

**M17 está documentado, no implementado.** Se considera revisable y listo para M18 cuando estén identificadas sus fronteras y casos de prueba, no cuando sus dependencias se hayan construido. M18 deberá hacer la **auditoría final de consistencia transversal de 17 módulos** y normalizar M01–M09 antes de preparar cualquier ZIP definitivo.

**Fin del Módulo 17.**
