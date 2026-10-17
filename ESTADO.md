# Estado del proyecto

*Inventario del corte escrito por Claude el 2026-10-13. Actualizado por Codex el mismo día tras verificar y cerrar #46 y hacer el triaje de G2 sobre `main` aa35372 y GitHub; el 2026-10-14 se registra la aceptación de la especificación #118. Codex lo actualiza al cerrar cada Issue (AGENTS.md); si algo aquí contradice el código o GitHub, mandan ellos.*

## Cómo llegamos aquí

El repositorio se creó el 2026-10-10, desde el diseño de referencia en `docs/arquitectura/` (ver `LEER_PRIMERO.md`). Hasta el 2026-10-13, Claude y Codex trabajaron sin roles fijos: cualquiera de los dos implementaba, y el otro revisaba cada PR antes de integrarlo (revisión cruzada, sin arquitecto ni verificador separado). Con ese método se cerraron G0 y G1, y se avanzó la mayor parte de G2: el motor semántico (CSMIR/SQIR/SEIR), el motor de permisos, la auditoría y la vía semántica única descritos abajo.

El 2026-10-13 el propietario decidió pasar a **roles estrictos** (`DECISIONES.md`): Codex pasa a decidir qué se construye y a verificar cada entrega; Claude solo implementa lo que Codex especifica. Este documento es el inventario en el momento del corte: lo que ya existe y funciona bajo el método anterior, y lo que queda a medias para que Codex lo organice en Issues con el nuevo método.

## Gate actual: G2 — Semántica y seguridad

**Al entregar #115 (Claude, 2026-10-16, conteos comprobados en GitHub):** G2 tiene **26 Issues cerrados y 14 abiertos**. Desde la actualización siguiente se integró #122 (PR #123, `8e35e55`: Claude activa a Codex al entregar con CI verde, consultas al propietario por GitHub y autoría de fusiones en `DECISIONES.md`/`AGENTS.md`), y Codex abrió #124 (Modelos: exigir `view` del catálogo completo), con la decisión del propietario «1A y 2A» registrada y `spec-lista`. El PR de #115 queda en revisión.

**Actualización verificada por Codex, 2026-10-15:** G2 tiene **25 Issues cerrados y 13 abiertos**; no hay PR abiertos. #120 está integrado mediante #121 en `f59a7687b0f897c7748b18c9d5d623699fb41d5c` (rustls 0.23.46). **#113 quedó cerrado al fusionar #119 por squash en `94cd6f66d1cf2334ed4cc3051a2d8fa377528904`**, tras la aprobación de Codex sobre `e0f8876` y los siete checks de CI en verde. Entrega: rechazo de JSON ambiguo en PostgreSQL, verificación de integridad posterior a contenido inválido, CLI `audit-verify --report` y clasificación/observabilidad segura de SQLSTATE 22023. La verificación y sus regresiones constan en el [veredicto final](https://github.com/eribertocz/bi-platform/pull/119#issuecomment-6204248684); las dos últimas regresiones pasaron con PostgreSQL 18.6 real en Docker. El propietario autorizó expresamente para #119 la autoría automática de GitHub con Claude como coautor; autorización registrada en el PR. #71 y G2 siguen abiertos.

Según `docs/arquitectura/gobierno/ROADMAP_GLOBAL.md`: modelos semánticos con medidas y *drill* autorizado; criterio de cierre, «usuarios con permisos distintos no cruzan filas, caché ni exportación». G0 y G1 están cerrados (app, CI, datos a gráfico). Quedan los nueve paraguas originales de G2 y #115–#118, cuyas especificaciones están consultadas y aceptadas por el propietario. El conteo de cerrados incluye #114 como **no planificado**, retirado tras consultar al propietario: exigir calidad y arquitectura del código en cada especificación sin crear nuevas reglas; no se entregó funcionalidad ni se integró documentación por ese Issue. Falta sobre todo la demo de aislamiento entre dos usuarios y migrar las rutas antiguas al punto único (Issue #69).

**Secuencia vigente, un Issue a la vez:** [#115](https://github.com/eribertocz/bi-platform/issues/115) entregado por Claude, en revisión de Codex. Después: #116 → #117 → #118 → [#124](https://github.com/eribertocz/bi-platform/issues/124), sin ampliar sus especificaciones aceptadas. Continúa el orden de los paraguas [#71 → #69 → #84 → #48 → #75 → #77 → #100 → #101 → #103](https://github.com/eribertocz/bi-platform/issues/71#issuecomment-6158911952), que se dividirán en entregas acotadas antes de implementarse. #115 es la siguiente entrega de #71 y #118 la primera pendiente de #69. Modelos, ejecución semántica y Explorar/demo de dos usuarios requieren especificaciones separadas. La investigación del riesgo RSS de #101 y la comprobación de las leyes prioritarias siguen vigentes. No se inicia G3 antes de cerrar G2.

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
| **#115** Rol de metadatos y migraciones separadas | Entregado por Claude, en revisión de Codex: `bi-server migrate`, modo restringido por defecto y desarrollo solo explícito; inspección del rol real (también roles alcanzables por herencia o `SET ROLE`) y del esquema sin DDL; un único punto de admisión (`metadata_not_ready`) con revalidación tras perder la conexión; permisos documentados en `backend/crates/bi-server/admin/runtime-grants.sql` y probados con PostgreSQL 18 y Keycloak reales |
| **#116** Versiones y madurez del producto | Plan consultado y aceptado, con `spec-lista`, pendiente de su PR documental/ADR 0014: beta desde G4 y objetivo rc tras G6 condicionado a pilotos y preparación operativa demostrados; no inicia gates posteriores ni autoriza tags |
| **#117** Experiencia web, despliegue y ubicación de datos | Especificación documental consultada y aceptada, con spec-lista y pendiente de PR/ADR 0015: navegador y contenedores, instalaciones propias, datos locales/remotos/híbridos y compartir sin copia automática; después de #116. No inicia implementación de G3/G4/G6 |
| **#118** Cerrar la vía de consultas G1 fuera de desarrollo sintético | Especificación consultada y aceptada el 2026-10-14, con spec-lista: bloquear POST /api/v1/queries fuera del modo local sintético verificado, incluso para admin/OIDC; conservar la demo local y auditar los bloqueos con plazo total finito. Incluye rechazar marcas de fixture sin contenido acreditado y comprobar admisión antes de extraer JSON. Depende de #113/#115 integrados y sigue a #116/#117. No añade ejecución semántica ni cierra #69/G2 |
| **#124** Modelos: `view` del catálogo completo | Decisión del propietario «1A y 2A» registrada y `spec-lista`: inspección completa de definiciones solo para identidades autorizadas (modeler/admin por políticas base; el resto por Cedar), sin conceder acceso a datos. Implementación después de #115 y #118 |
| **#69** Punto único de permisos | Los permisos ya deciden (#90) y la vía `/explain` ya autoriza (#110), pero `/api/v1/queries` sigue sin pasar por ahí, `/api/v1/models` no exige `view`, y falta la demo de dos usuarios con totales distintos |
| **#48** Adaptador Ossie | Partes 1–3 integradas; falta la parte 4 (exportación CSMIR → Ossie de ida y vuelta) |
| **#75** Metadatos Ossie en Modelos | El propietario ya validó la pantalla; falta lo mismo que #48 parte 4 |
| **#77** Funciones de `OSSIE_SQL_2026` | Bloque 1 (texto/números, #92) y fechas (#95) integrados; falta agregados y funciones de ventana |
| **#71** Auditoría a prueba de manipulación | Encadenamiento y correcciones de frontera #113 integrados; separación de roles y comprobación de privilegios entregadas en #115 (en revisión) antes de ampliar ejecución; después, productores restantes, endpoint paginado del auditor y coste por consulta |
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
