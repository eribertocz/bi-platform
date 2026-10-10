# Roadmap general — construcción de una plataforma BI moderna

**Objetivo:** llevar un núcleo BI correcto y pequeño hasta plataforma extensible. Cada gate produce un entregable que puede mostrarse y probarse, no una colección de carpetas. Las funciones avanzadas se incorporan sin reescribir el motor semántico ni la seguridad.

## Secuencia de gates, no 17 proyectos paralelos

| Gate | Resultado visible / útil | Backend | Frontend | Aprobación (DoD) |
|---|---|---|---|---|
| **G0 — Fundaciones** | App shell y API con health y CI | Rust workspace, Axum, PostgreSQL dev, migraciones, logging | SvelteKit3, TS6, Tailwind4, navegación mínima, client tipado | Inicio local 1 comando/reproducible; CI y e2e health |
| **G1 — Datos a gráfico** | Usuario consulta datos de prueba y ve KPI + gráfico | DuckDB embebido, Parquet, DuckLake/PostgreSQL spike, auth básica, typed query | tabla mínima, KPI y ECharts con filtro | Misma consulta SQL/resultado esperado, errores y cancel documentados |
| **G2 — Semántica y seguridad** | Modelos semánticos con medidas y drill autorizado | RLS/CLS, SemanticQuery, tipos correctos, revisión/snapshot, caché autorizada | filtros, selector dimensiones/medidas, SQL editor acotado | Usuarios con permisos distintos no cruzan filas/cache/export |
| **G3 — Gestión de datos** | Fuente, transformación, build y publicación versionada | Sources, Model DAG, materialización DuckLake, validation, jobs | Sources UI, editor SQL, estados build y logs | Publicación/rollback/retry idempotente; datos reproducibles |
| **G4 — Experiencia BI** | Explore, tabla pivot, editor de dashboard multi página | Query serving maduro y export | TanStack, S2/fallback, GridStack, ECharts, design system | Drill, selección, filtros globales, resize, publish draft/publicación |
| **G5 — Operación e IA** | Reportes/alertas, diagnóstico y asistente seguro | scheduler, telemetry, tools agente con autorización | jobs, reports, observability, chat | Fail/retry/idempotency; agent no bypass policy |
| **G6 — Distribución / empresa** | Producto publicable con deployment local/empresa | backup/restore, DR, OIDC, multi-user, HA en despliegues que lo ameriten | accesibilidad, bundle/performance, instalador y branding | Benchmarks reproducibles, seguridad, licencias, documentación operacional |

**Dependencia esencial:** G2 seguridad y semántica antes de permitir Canvas/IA sobre datos corporativos. G1 sólo puede usar dataset sintético y accesos de desarrollo. G3 y G4 pueden iterar incrementalmente una vez fijadas las interfaces principales.

## Forma de avanzar

1. Abrir un GitHub Issue concreto, con criterios de aceptación y dependencias.
2. Aplicar P-I-V-R-G (plan, implementación, verificación, informe, Git).
3. No comenzar muchas funciones superpuestas: máximo un objetivo observable por PR.
4. No iniciar gate siguiente hasta obtener la prueba E2E del gate anterior y registrar riesgos abiertos.
5. Si falla un probe de librería, registrar resultado y usar fallback; no rehacer el producto entero.
6. Mantener una única fuente de contratos: versionados en repositorio y generables desde Rust/OpenAPI.

## Milestones de release propuestos, revisables por código real

- **v0.1.0**: primer corte G0+G1 operativo (no necesariamente empresarial).
- **v0.2.0**: semántica y políticas básicas G2, seguridad probada.
- **v0.3.0**: M01–M03 funcionales para datasets/materialización y publicación.
- **v0.4.0**: Explore/Pivot/Canvas funcionales y accesibles.
- **v0.5.0**: scheduler/reporting/observabilidad y primeros agentes controlados.
- **v1.0.0**: únicamente tras calidad operativa y compatibilidad acordada, no por pasar seis números en un cronograma.

**Versiones propuestas, no compromiso automático:** el agente sugiere la siguiente release al completar un gate con tests; la decisión de publicar necesita aprobación.

## Entregables de arquitectura/ingeniería mínimos por etapa

- Base: `AGENTS.md`, `README`, decisiones técnicas, Git y CI. No introducir sistemas propios de coordinación.
- G1: contrato query JSON y fixture oráculo, spike DuckLake/Postgres y dataset sintético.
- G2: policy model, contratos de filtros, negative tests y semantic smoke end-to-end.
- G3: ADR para forma definitiva de models/orchestration/materialization y migraciones.
- G4: `VizSpec`, `DashboardRevision`, UI/interaction contract tests y tests visuales.
- G5/G6: runbooks mínimos, benchmark suites, seguridad, evaluación IA, backup/restore, release manifest.

No escribir 17 documentos adicionales por rutina. Crear documentación nueva **cuando una decisión no pueda expresarse claramente en código, contrato o pruebas**.

