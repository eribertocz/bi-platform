# Estado del proyecto

*Escrito por Claude el 2026-10-13, con datos verificados contra el repositorio y GitHub en ese momento, en la rama del corte a roles. Codex lo actualiza al cerrar cada Issue (AGENTS.md); si algo aquí contradice el código o GitHub, mandan ellos.*

## Cómo llegamos aquí

El repositorio se creó el 2026-10-10, desde el diseño de referencia en `docs/arquitectura/` (ver `LEER_PRIMERO.md`). Hasta el 2026-10-13, Claude y Codex trabajaron sin roles fijos: cualquiera de los dos implementaba, y el otro revisaba cada PR antes de integrarlo (revisión cruzada, sin arquitecto ni verificador separado). Con ese método se cerraron G0 y G1, y se avanzó la mayor parte de G2: el motor semántico (CSMIR/SQIR/SEIR), el motor de permisos, la auditoría y la vía semántica única descritos abajo.

El 2026-10-13 el propietario decidió pasar a **roles estrictos** (`DECISIONES.md`): Codex pasa a decidir qué se construye y a verificar cada entrega; Claude solo implementa lo que Codex especifica. Este documento es el inventario en el momento del corte: lo que ya existe y funciona bajo el método anterior, y lo que queda a medias para que Codex lo organice en Issues con el nuevo método.

## Gate actual: G2 — Semántica y seguridad

Según `docs/arquitectura/gobierno/ROADMAP_GLOBAL.md`: modelos semánticos con medidas y *drill* autorizado; criterio de cierre, «usuarios con permisos distintos no cruzan filas, caché ni exportación». G0 y G1 están cerrados (app, CI, datos a gráfico). **Milestone G2 en GitHub: 21 Issues cerrados, 10 abiertos** (los 10 de la tabla de abajo). Falta sobre todo la demo de aislamiento entre dos usuarios y migrar las rutas antiguas al punto único (Issue #69).

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
| **#69** Punto único de permisos | Los permisos ya deciden (#90) y la vía `/explain` ya autoriza (#110), pero `/api/v1/queries` sigue sin pasar por ahí, `/api/v1/models` no exige `view`, y falta la demo de dos usuarios con totales distintos |
| **#48** Adaptador Ossie | Partes 1–3 integradas; falta la parte 4 (exportación CSMIR → Ossie de ida y vuelta) |
| **#75** Metadatos Ossie en Modelos | El propietario ya validó la pantalla; falta lo mismo que #48 parte 4 |
| **#77** Funciones de `OSSIE_SQL_2026` | Bloque 1 (texto/números, #92) y fechas (#95) integrados; falta agregados y funciones de ventana |
| **#71** Auditoría a prueba de manipulación | El encadenamiento existe y se usa (login, vía semántica); Codex dejó recomendaciones pendientes antes de conectar más productores |
| **#101** Límite de memoria por consulta | ADR 0013 integrada (#111): protege la memoria que gestiona DuckDB. No protege toda la memoria del proceso (RSS); ese aislamiento más fuerte, por procesos del sistema operativo, queda pendiente de decisión |
| **#84** Auditoría retroactiva de las leyes | Parcial: el gobierno de métricas, datasets y campos (leyes 1, 3 y 8) está integrado (#89); falta auditar el resto del código contra las 13 leyes y mostrar el aviso de métrica obsoleta en pantalla y API |
| **#100** Caché de modelos compilados | No iniciada (hoy cada petición recompila el proyecto) |
| **#102** MCP y Arrow Flight | G5, no iniciada |
| **#103** Borradores para la comunidad Ossie | No iniciada; la publica el propietario, nunca los agentes |
| **#67** Identificadores estables ante renombrado | G4, no iniciada |

**Candidato a cerrar en el primer triaje de Codex:** el **Issue #46** (planificador y SEIR) tiene sus tres criterios cumplidos por el PR #86 (21 casos deterministas, certificado, sin dependencias de motor). No lo cerré yo: verificar y cerrar issues es ahora tarea del arquitecto/verificador.

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
