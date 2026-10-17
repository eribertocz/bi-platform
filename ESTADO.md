# Estado del proyecto

*Inventario del corte escrito por Claude el 2026-10-13. Actualizado por Codex el mismo día tras verificar y cerrar #46 y hacer el triaje de G2 sobre `main` aa35372 y GitHub; el 2026-10-14 se registra la aceptación de la especificación #118; el 2026-10-17 Claude actualiza estado y conteos al entregar #116, #117 y #118. Codex lo actualiza al cerrar cada Issue (AGENTS.md); si algo aquí contradice el código o GitHub, mandan ellos.*

## Cómo llegamos aquí

El repositorio se creó el 2026-10-10, desde el diseño de referencia en `docs/arquitectura/` (ver `LEER_PRIMERO.md`). Hasta el 2026-10-13, Claude y Codex trabajaron sin roles fijos: cualquiera de los dos implementaba, y el otro revisaba cada PR antes de integrarlo (revisión cruzada, sin arquitecto ni verificador separado). Con ese método se cerraron G0 y G1, y se avanzó la mayor parte de G2: el motor semántico (CSMIR/SQIR/SEIR), el motor de permisos, la auditoría y la vía semántica única descritos abajo.

El 2026-10-13 el propietario decidió pasar a **roles estrictos** (`DECISIONES.md`): Codex pasa a decidir qué se construye y a verificar cada entrega; Claude solo implementa lo que Codex especifica. Este documento es el inventario en el momento del corte: lo que ya existe y funciona bajo el método anterior, y lo que queda a medias para que Codex lo organice en Issues con el nuevo método.

## Gate actual: G2 — Semántica y seguridad

**Al entregar #116 (Claude, 2026-10-17, comprobado con `gh issue list --milestone`):** G2 tiene **21 Issues cerrados y 15 abiertos**, contando solo Issues; los conteos anteriores de este documento salían del milestone, que también cuenta los PR. Desde la actualización de Codex del 2026-10-15:

- se integró #122 (PR #123, `8e35e55`): Claude activa a Codex al entregar con CI verde, las consultas al propietario van por GitHub y queda registrada la autoría de las fusiones en `DECISIONES.md`/`AGENTS.md`;
- Codex abrió #124 (Modelos: exigir `view` del catálogo completo), con la decisión del propietario «1A y 2A» registrada y `spec-lista`;
- Codex abrió y cerró #126: PR #127 integrado en `f235512`, duckdb 1.10506.1 sin la dependencia zip vulnerable;
- el PR #125 de #115 se corrigió tras la revisión de Codex y está en verificación;
- Codex abrió #128, el plan de cierre de G2 y del candidato `v0.2.0-alpha.2` para el propietario; es de seguimiento y no lleva `spec-lista`.

**Al entregar #117 (Claude, 2026-10-17):** G2 tiene **22 Issues cerrados y 14 abiertos**. #116 quedó cerrado al fusionar el PR #129 en `48b0c09` (ADR 0014). El PR #125 de #115 está aprobado por Codex sobre `d7dcaa3`, con CI en verde, y pendiente de su fusión.

**Estado vigente (Claude, 2026-10-17, tras la fusión de #125, comprobado con `gh issue list --milestone`):** G2 tiene **23 Issues cerrados y 13 abiertos**, contando solo Issues. #115 quedó cerrado al fusionar el PR #125 por squash en `cf4eb22`, tras la aprobación de Codex sobre `d7dcaa3` y los siete checks de CI en verde.

**Al entregar #118 (Claude, 2026-10-17, comprobado con `gh issue list --milestone`):** G2 tiene **24 Issues cerrados y 12 abiertos**. #117 quedó cerrado al fusionar el PR #130 en `99f711c` (ADR 0015). El PR de #118 queda en revisión.

**Al entregar #124 (Claude, 2026-10-17, comprobado con `gh issue list --milestone`):** G2 tiene **26 Issues cerrados y 30 abiertos**, contando solo Issues; el plan de #128 añadió #134–#151 como entregas acotadas. #118 quedó cerrado al fusionar el PR #132 en `8cec10d`. El PR de #124 (parte servidor) queda en revisión.

**Actualización verificada por Codex, 2026-10-15:** G2 tiene **25 Issues cerrados y 13 abiertos**; no hay PR abiertos. #120 está integrado mediante #121 en `f59a7687b0f897c7748b18c9d5d623699fb41d5c` (rustls 0.23.46). **#113 quedó cerrado al fusionar #119 por squash en `94cd6f66d1cf2334ed4cc3051a2d8fa377528904`**, tras la aprobación de Codex sobre `e0f8876` y los siete checks de CI en verde. Entrega: rechazo de JSON ambiguo en PostgreSQL, verificación de integridad posterior a contenido inválido, CLI `audit-verify --report` y clasificación/observabilidad segura de SQLSTATE 22023. La verificación y sus regresiones constan en el [veredicto final](https://github.com/eribertocz/bi-platform/pull/119#issuecomment-6204248684); las dos últimas regresiones pasaron con PostgreSQL 18.6 real en Docker. El propietario autorizó expresamente para #119 la autoría automática de GitHub con Claude como coautor; autorización registrada en el PR. #71 y G2 siguen abiertos.

Según `docs/arquitectura/gobierno/ROADMAP_GLOBAL.md`: modelos semánticos con medidas y *drill* autorizado; criterio de cierre, «usuarios con permisos distintos no cruzan filas, caché ni exportación». G0 y G1 están cerrados (app, CI, datos a gráfico). Quedan abiertos los nueve paraguas originales de G2; #117, #118 y #124, cuyas especificaciones están consultadas y aceptadas por el propietario; y el plan de seguimiento #128. El conteo de cerrados incluye #114 como **no planificado**, retirado tras consultar al propietario: exigir calidad y arquitectura del código en cada especificación sin crear nuevas reglas; no se entregó funcionalidad ni se integró documentación por ese Issue. Falta sobre todo la demo de aislamiento entre dos usuarios y migrar las rutas antiguas al punto único (Issue #69).

**Versiones:** el plan vigente está en [ADR 0014](docs/decisiones/0014-versiones-por-gate.md). El primer flujo protegido de G2 apunta a `v0.2.0-alpha.1` y su cierre verificado a `v0.2.0-alpha.2`; ambos dependen de la evidencia que exige la ADR y de que el propietario apruebe publicarlos. Todavía no hay tags de versión de producto ni GitHub Releases.

**Secuencia vigente, un Issue a la vez:**

- [#115](https://github.com/eribertocz/bi-platform/issues/115): integrado (PR #125 en `cf4eb22`).
- [#116](https://github.com/eribertocz/bi-platform/issues/116): integrado (PR #129, ADR 0014).
- [#117](https://github.com/eribertocz/bi-platform/issues/117): integrado (PR #130, ADR 0015).
- [#118](https://github.com/eribertocz/bi-platform/issues/118): integrado (PR #132 en `8cec10d`).
- [#124](https://github.com/eribertocz/bi-platform/issues/124): parte servidor entregada en su PR, en revisión; la presentación en la UI es #134 (división aprobada por Codex en #124).
- Después: [#131](https://github.com/eribertocz/bi-platform/issues/131) (documental), #134 y las tandas A–D de #128. Continúa el orden de los paraguas [#71 → #69 → #84 → #48 → #75 → #77 → #100 → #101 → #103](https://github.com/eribertocz/bi-platform/issues/71#issuecomment-6158911952), que se dividirán en entregas acotadas antes de implementarse. #115 integró la separación de roles de #71 y #118 es la primera entrega de #69. Modelos, ejecución semántica y Explorar/demo de dos usuarios requieren especificaciones separadas. La investigación del riesgo RSS de #101 y la comprobación de las leyes prioritarias siguen vigentes. No se inicia G3 antes de cerrar G2.

## Qué existe y funciona

**Backend** (`backend/crates/`), un monolito modular en Rust (Axum):

| Crate | Qué hace | Evidencia |
|---|---|---|
| `sef-model-ir` | CSMIR: modelo semántico tipado, huellas canónicas, diferencia entre versiones (`diff.rs`) | Corpus de 21 casos normativos |
| `sef-ossie` | Lee y escribe Apache Ossie (`OSSIE_SQL_2026`), conversión a CSMIR, posiciones de error en el documento | Prueba de conformidad contra el esquema oficial |
| `sef-query-ir` | SQIR: consulta vinculada, tipada, con huella | |
| `sef-correctness` | Motor de corrección: grano, *fanout*, caminos, políticas | |
| `sef-exec-ir` | SEIR: plan dirigido por contenido | |
| `sef-planner` | Planificador determinista con certificado verificado de forma independiente | 21 casos, certificado reproducible |
| `sef-authz` | Permisos: Cedar, RBAC + ABAC, CLS en tres niveles (denegación ante cualquier error) | ADR 0012 |
| `sef-adapter-duckdb` | Ejecuta el SEIR certificado en DuckDB; exactitud hasta 76 dígitos | 21 casos con resultado exacto |
| `bi-runtime` | Grupo de trabajadores DuckDB con plazos, cancelación, admisión y memoria aislada por consulta | ADR 0013 |
| `bi-audit` | Auditoría append-only encadenada (hash), a prueba de manipulación | ADR 0010 |
| `bi-server` | Axum: sesiones (desarrollo y OIDC), modelos del proyecto, **vía semántica única** (`/api/v1/semantic/explain`) | ADR 0010/0012 |
| `bi-fixtures` | Datos sintéticos deterministas y su oráculo exacto, para probar contra un resultado conocido | Usado por las pruebas de `bi-server` y `sef-adapter-duckdb` |

**Vía semántica única** (lo más reciente, PR #110): toda consulta se vincula, autoriza en dos fases, valida y planifica antes de responder con decisión, plan certificado y recibo de huellas. **No ejecuta todavía**: eso es la siguiente pieza (ver Issue #69).

**Frontend** (`frontend/src/routes/`): `login`, `modelos` (catálogo de métricas con avisos localizados), `explorar`, `estado`. SvelteKit 5, ECharts.

## Lo que está a medias (evidencia en Issues abiertos)

| Issue | Qué falta exactamente |
|---|---|
| **#118** Cerrar la vía de consultas G1 fuera de desarrollo sintético | Integrado (PR #132 en `8cec10d`): `POST /api/v1/queries` responde 403 `legacy_query_disabled` antes de leer el cuerpo salvo en la demo sintética local (build `dev-auth`, modo `development`, usuarios sintéticos, origen loopback, sin OIDC, sesión local y fixture acreditado por el propio arranque, comparado fila por fila al reutilizar el lago). Cada rechazo autenticado se audita con plazo total (503 `audit_unavailable` si no puede registrarse). No añade ejecución semántica ni cierra #69/G2 |
| **#124** Modelos: `view` del catálogo completo | Parte servidor entregada por Claude, en revisión: `GET /api/v1/models` decide `view` sobre `Model::"project:catalog"` con `policies/` del proyecto antes de leer `metrics/` (base: `modeler`/`admin`; el resto, por Cedar explícito); 403 `models_forbidden` sin nada del catálogo, 503 `policies_invalid` si el acceso no se puede usar, y cada decisión se audita antes de responder (503 `audit_unavailable`). La demo asigna `modeler` a `synthetic-modeler`. La UI de denegación y recuperación es #134 |
| **#128** Plan de cierre de G2 | Seguimiento de Codex para el propietario: qué falta hasta `v0.2.0-alpha.2`, dependencias y estimación revisable. No es una tarea de implementación ni lleva `spec-lista` |
| **#69** Punto único de permisos | Los permisos ya deciden (#90) y la vía `/explain` ya autoriza (#110), `/api/v1/queries` queda cerrada fuera de la demo sintética local (#118), `/api/v1/models` exige `view` del catálogo (#124, en revisión), y falta la demo de dos usuarios con totales distintos |
| **#48** Adaptador Ossie | Partes 1–3 integradas; falta la parte 4 (exportación CSMIR → Ossie de ida y vuelta) |
| **#75** Metadatos Ossie en Modelos | El propietario ya validó la pantalla; falta lo mismo que #48 parte 4 |
| **#77** Funciones de `OSSIE_SQL_2026` | Bloque 1 (texto/números, #92) y fechas (#95) integrados; falta agregados y funciones de ventana |
| **#71** Auditoría a prueba de manipulación | Encadenamiento y correcciones de frontera #113 integrados. Separación de roles y comprobación de privilegios integradas por #115 (PR #125, `cf4eb22`): `bi-server migrate` con plazo total, modo restringido por defecto y desarrollo solo explícito; inspección del rol real (también roles alcanzables por herencia o `SET ROLE`) y del esquema sin DDL; un único punto de admisión (`metadata_not_ready`) con revalidación tras perder la conexión y validación de cada conexión nueva antes de servir; permisos documentados en `backend/crates/bi-server/admin/runtime-grants.sql` y probados con PostgreSQL 18 y Keycloak reales. Falta: productores restantes, endpoint paginado del auditor y coste por consulta |
| **#101** Límite de memoria por consulta | ADR 0013 integrada (#111): protege la memoria que gestiona DuckDB. No protege toda la memoria del proceso (RSS); ese aislamiento más fuerte, por procesos del sistema operativo, queda pendiente de decisión |
| **#84** Auditoría retroactiva de las leyes | Parcial: el gobierno de métricas, datasets y campos (leyes 1, 3 y 8) está integrado (#89); falta auditar el resto del código contra las 13 leyes y mostrar el aviso de métrica obsoleta en pantalla y API |
| **#100** Caché de modelos compilados | No iniciada (hoy cada petición recompila el proyecto) |
| **#102** MCP y Arrow Flight | G5, no iniciada |
| **#103** Borradores para la comunidad Ossie | No iniciada; la publica el propietario, nunca los agentes |
| **#67** Identificadores estables ante renombrado | G4, no iniciada |

**#46 verificado y cerrado por Codex (2026-10-13):** los tres criterios se cumplen en #86 y en `main` aa35372. Corpus de 21 casos: 18 planes/certificados deterministas y 3 rechazos normativos; ramas agregadas antes de alinear; sin dependencias de motor ni SQL. Ejecutadas 74 pruebas (0 fallos, 2 mediciones ignoradas), formato, Clippy y árbol de dependencias; leído el motor/verificador. [Evidencia y comandos](https://github.com/eribertocz/bi-platform/issues/46#issuecomment-6158878048). Los Issues #67 y #102 de la tabla pertenecen a G4 y G5, respectivamente, y no se inician durante G2.

## Huecos de diseño sin spec ni Issue (hallazgo, no orden de prioridad)

Comparado con Power BI/Tableau/Cube (`docs/referentes/README.md`), estas capacidades no tienen todavía ni Issue ni documento de diseño propio — solo la mención genérica de la base histórica (`docs/arquitectura/modulos/M0*`). **Decidir cuándo abordar cada una es tarea de Codex**; el roadmap (`ROADMAP_GLOBAL.md`) dice no empezar un gate sin cerrar el anterior, así que G2 (ver tabla de arriba) va antes que G4.

| Hueco | Gate | Por qué urge definirlo con tiempo |
|---|---|---|
| **`VisualSpec`** (contrato visual de los tres niveles de edición) | G4 | Se nombra como «lo diferencial» en 6 documentos (`leyes-bi.md`, ADR 0008...), pero ningún campo, tipo ni esquema está escrito. Sin él, Pivot/Canvas/Visualización no tienen sobre qué construirse |
| Tabla dinámica (Pivot, M07) | G4 | Sin diseño |
| Canvas / composición de dashboards (M08) | G4 | Sin diseño |
| Programación, alertas e informes (M14) | G5 | Sin diseño |
| Conectores en vivo más allá de DuckDB (ClickHouse, otros; ADR 0002 ya decidió la dirección) | G3 | Decidido en ADR, cero código ni Issue de implementación |
| ELT propio (Studio, modelos SQL; ADR 0005, 10 decisiones de `version-comun.md`) | G3 | Decidido en documentos, cero código |

## Decisiones que ya mandan sobre la documentación histórica

Ver `DECISIONES.md` y, en detalle, `docs/decisiones/README.md` (incluye qué menciones de `docs/arquitectura/` están obsoletas, como Polars).

## Cómo verificar este documento

```sh
gh issue list --state open          # issues que siguen abiertos
gh pr list --state merged --limit 20  # lo último integrado
cd backend && cargo test             # qué realmente pasa
```
