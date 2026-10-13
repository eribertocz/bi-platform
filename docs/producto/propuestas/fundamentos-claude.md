# Evaluación de la "Documentación Fundamental" (8 documentos) — Claude

*Evaluación de Claude, 2026-10-11, independiente de la de Codex. **Cerrada:** la versión conjunta, con las 10 decisiones aprobadas por el propietario el 2026-10-12, está en [`version-comun.md`](version-comun.md). Este documento queda como la evaluación original, para seguir la discusión.*

**Leyenda:** ✅ ya lo cumplimos (con evidencia) · ➕ adoptar · ⚠ choca con una decisión vigente (con recomendación) · ⬆ mejora nuestra.

## Veredicto general
- Es un marco **muy bueno y coherente con la dirección del proyecto**: casi todo encaja con las ADR vigentes.
- Choca en **cuatro puntos**. En todos recomiendo mantener lo decidido y tomar la idea de fondo:
  1. la estructura de carpetas;
  2. el formato YAML propio de métricas;
  3. "solo Arrow" en la API;
  4. las políticas como plantillas de texto.
- Le faltan cinco cosas importantes:
  1. totales calculados por el motor;
  2. clave de caché con el contexto de seguridad;
  3. modo avanzado de gráficos sin funciones ni HTML;
  4. exactitud decimal en el navegador;
  5. accesibilidad.

## Documento 1 — Leyes
✅ Ya incorporadas y mejoradas en `docs/producto/leyes-bi.md` (PR #82), con tres leyes más (exactitud, apertura, accesibilidad), son retroactivas y tienen la auditoría #84.

## Documento 2 — Principios y anti-patrones
- ✅ Todos los principios coinciden con lo vigente:
  - *fail loud* = corpus y motor de corrección: rechaza, nunca aproxima (#83);
  - determinismo = huellas canónicas;
  - mínimo privilegio = ADR 0010;
  - inmutabilidad histórica = DuckLake y auditoría encadenada (#81).
- ➕ **Adoptar la lista de anti-patrones como lista de comprobación obligatoria** en AGENTS.md, junto a las leyes.
- ⬆ **"Silent failures: devolver 0 o null sin explicación" necesita un matiz:**
  - un `NULL` puede ser el resultado **correcto** (la suma de un conjunto vacío es NULL según SQL y según el corpus, caso 14);
  - convertirlo en 0 sería un error de exactitud;
  - la regla buena es "nunca un NULL o un vacío **sin explicar**": la pantalla dice "sin datos para este filtro", no pone un 0.

## Documento 3 — Capacidades y patrones
| Capacidad core | Estado |
|---|---|
| Modelo semántico versionado | ✅ Ossie en Git (ADR 0003 y 0006) |
| Resolución semántica única | 🔄 Punto único (ADR 0010); motor de corrección (#83) y planificador (#46) |
| RLS + CLS | 🔄 RLS con políticas tipadas; CLS añadido a #69 |
| Lineage y explicabilidad | 🔄 *Explain* del planificador (§26); ⏳ linaje hasta la columna en G3/G4 |
| Roles y permisos granulares | ⏳ #69 (RBAC + ABAC, denegación por defecto) |
| Freshness visible | ⏳ #84 |
| Caché semántica | ⏳ Bloqueada hasta el punto único (ADR 0010) |
| Audit log | 🔄 #71 (base aprobada) |
| Direct Query + Importado | ✅ Decidido en la ADR 0009 (en vivo, guardar resultados o pipeline) |
| Exportación segura | ⏳ Bloqueada hasta el punto único (ADR 0010) |

- ➕ **Avanzadas que aún no están en la hoja de ruta:** *what-if* / escenarios, *time intelligence* avanzada (YoY, MoM, YTD) y *embedded analytics*. Propongo darles un Issue cada una en G4 o G5. La *time intelligence* es la más urgente: es lo primero que pide un usuario de negocio.
- ✅ Todos los patrones coinciden con lo decidido:
  - capa semántica central (ADR 0010);
  - métricas como código (ADR 0003);
  - *headless* BI (ADR 0011);
  - compilador y runtime (CSMIR → SQIR → SEIR → adaptador);
  - *pushdown* (adaptadores);
  - doble almacenamiento (ADR 0009);
  - promoción de entornos (propuesta ELT y estudio).
- ➕ Adoptar sus **criterios de aceptación rápidos** como parte de la plantilla de PR.

## Documento 4 — Decisiones por capa
**ELT**
- ✅ Modelos como código, `plan` + `apply`, staging y marts (ADR 0005).
- ✅ Incrementales desde la primera entrega (ADR 0005, G3).
- ➕ **"La lógica de métricas no vive en el ELT":** el ELT prepara datos; las métricas solo están en la capa semántica. Lo adoptamos tal cual: evita dos definiciones del mismo número.
- ➕ "El frontend no dispara transformaciones ad-hoc": en el estudio, ejecutar un modelo es una acción del plan, auditada y con permisos.

**Almacenamiento**
- ✅ ADR 0002 y 0009.

**Motor semántico**
- ✅ Responsabilidades y flujo coinciden con SEF: validar → seguridad → plan → optimizar → ejecutar → caché.
- ⬆ Nuestra consulta (SQIR) es más rica que `{ metrics, dimensions, filters, time_range }`: varios bloques con granos distintos (necesarios para totales correctos), roles de relación, filtros antes y después de agregar, y orden con nulos.
- ➕ Adoptar en la **respuesta**: `freshness`, `queryId`, `cacheHit` y `explanation`. Les añado el **recibo** (huellas y versión de datos, ley 2).

**API**
- ✅ Tipada y versionada (`/api/v1`), con autenticación antes del motor.
- ✅ Hoy **no** existe ningún endpoint que acepte SQL libre: `POST /api/v1/queries` es una consulta tipada sobre un dataset permitido. Pendiente: que pase por el motor semántico (ADR 0010, #84).
- ➕ Consultas asíncronas y *rate limiting* por usuario o rol, con el motor de permisos.

**Frontend**
- ➕ Adoptar tal cual:
  - separar estado de datos, configuración visual y estado de UI;
  - **nunca construir la consulta desde el `option` de ECharts**;
  - guardar por separado la consulta semántica y la configuración visual.
- ⬆ Lo convertimos en el contrato `VisualSpec` (G4).

**Artefactos**
- ✅ Todo versionado en archivos (ADR 0003).
- ➕ Permisos y políticas como configuración versionada, con revisión.

## Documento 5 — Estructura y contratos
- ⚠ **La estructura de carpetas propuesta** (`crates/semantic-engine`, `arrow-runtime`, `security`, `api-server`, `packages/…`, `apps/web`) **choca** con la vigente: `backend/crates/{bi-*, sef-*}` y `frontend/`.
  - La nuestra ya separa núcleo y adaptadores, y **CI lo vigila** (ADR 0008).
  - Reorganizar costaría semanas y no daría ninguna garantía nueva.
  - **Recomendación:** mantener la estructura y tomar sus ideas buenas:
    - un módulo **`echarts-safe`** propio (`frontend/src/lib/echarts-safe/`) que valide y limpie el `option`;
    - el **cliente TypeScript generado** del contrato, que ya existe (`generated.ts` desde OpenAPI).
- ✅ Reglas de dependencias: ya las impone CI (ADR 0008). La web solo usa el cliente generado.
- ⚠ **"Datos como Arrow" en la respuesta al navegador:**
  - Arrow es ideal para grandes volúmenes y ya es nuestro formato entre capas (ADR 0008);
  - pero en JavaScript, `apache-arrow` convierte los `Decimal128` y los `Int64` a números de coma flotante con facilidad, y eso **violaría la ley de exactitud**.
  - **Recomendación:** JSON tipado con valores exactos como texto (el contrato actual de G1) para resultados de pantalla, y Arrow IPC para volúmenes grandes. En Arrow, los decimales se leen como `BigInt` o texto, nunca como `number`, con una prueba contra el oráculo.
- ✅ Contratos versionados (`/v1`, `/v2`) y cliente generado.

## Documento 6 — Seguridad
- ➕ **Roles base** `viewer`, `analyst`, `modeler` y `admin` con su tabla de permisos por recurso: los adopto como **conjunto inicial** del motor de permisos (#69), sobre RBAC + ABAC.
- ⚠ **El ejemplo de RLS `"orders.region IN ${user.regions}"` es una plantilla de texto**:
  - insertar atributos del usuario en el SQL abre la puerta a la **inyección**;
  - nuestras políticas usan **parámetros tipados** (`ContextSlot`) que nunca se convierten en texto SQL (probado en #72 y #83);
  - **Recomendación:** mantener los parámetros tipados, aceptando la misma idea de "región del usuario".
- ➕ **CLS con tres niveles** (mostrar, enmascarar, eliminar) y columnas `sensitive` o `restricted`: se añade a #69 tal cual.
- ➕ API keys con *scopes* para cuentas de servicio y *embeds*, más tokens cortos: después de OIDC (#70).
- ✅ Sesiones con caducidad (G1), auditoría (#71), accesos denegados registrados (#71) y SAML después de G2.

## Documento 7 — Motor semántico
- ⚠ **El formato de métrica YAML propio** choca con la decisión de usar **Apache Ossie** como estándar (ADR 0006 y fichas de Ossie):
  - un YAML propio nos encerraría;
  - Ossie lo leen más de 15 herramientas;
  - **Recomendación:** mantener Ossie y llevar sus campos que faltan a la extensión `bi-platform`:

    | Campo propuesto | Dónde queda |
    |---|---|
    | `label`, `description` | ✅ Ossie |
    | `type: sum` + `sql` | ✅ La expresión de Ossie |
    | `filters` (filtro intrínseco) | ✅ Ossie con `FILTER (WHERE …)` cuando se publique, o la extensión |
    | `owner`, `tags`, `status` (active / deprecated, con fecha y reemplazo) | ➕ Extensión `bi-platform` (#84) |
    | `dimensions` permitidas | ⬆ Las calcula el motor de corrección por los caminos de relaciones. Opcionalmente, una **lista restrictiva** de gobierno en la extensión |
- ✅ El flujo de 10 pasos coincide con SEF. Lo nuestro añade obligaciones demostradas y certificado del plan.
- ⚠ **"Arrow en todo el camino caliente, evitar JSON":** sí entre motor, adaptadores y ejecución; hacia el navegador, ver el documento 5.
- ➕ **Time intelligence:**
  - rangos relativos y absolutos;
  - periodo anterior, YoY y MoM;
  - YTD, MTD y QTD.

  Es un hueco real del motor. Propongo un Issue para SQIR y el motor de corrección, con su regla clave: comparar periodos exige el mismo calendario y la misma zona.
- ✅ Pruebas: compilación, políticas, consistencia, *golden files* de planes (ya tenemos los de obligaciones; los de SEIR llegarán con #46) y rendimiento.
- ✅ Extensibilidad: adaptadores tras contratos (ADR 0008); ningún plugin se salta el punto único (ADR 0010).

## Documento 8 — UX y 3 modos
- ➕ Adoptar todo:
  - los tres modos;
  - tipo de gráfico sugerido;
  - freshness, *skeletons*, errores accionables y *empty states* útiles;
  - exportación de primera clase;
  - *responsive* y modo oscuro;
  - el **widget persistido** `{ semanticQuery, visualMode, visualConfig, … }`, que nunca guarda solo el `option`.
- ⬆ **Mejoras:**
  - **Accesibilidad WCAG 2.2 AA**, que el documento no menciona y ya es obligatoria (ADR 0010): gráficos con alternativa en tabla, teclado y lector de pantalla.
  - **Modo avanzado: solo JSON**, sin funciones ni HTML evaluable (riesgo XSS). Se valida con una lista de propiedades permitidas en `echarts-safe`.
  - **Cambiar de modo sin perder nada:** los tres modos son vistas del mismo `VisualSpec`.
  - **Idioma:** la interfaz en español y en inglés desde el diseño (textos externalizados).

## El orden de prioridad del cierre
El documento fija, para resolver conflictos: 1) leyes, 2) anti-patrones, 3) seguridad, 4) decisiones del motor y por capa, 5) capacidades y patrones, 6) estructura y contratos, 7) UX.

- ✅ **Lo comparto en lo esencial:** las leyes primero, después lo que no se debe hacer, la seguridad por encima de cualquier decisión técnica, y la estructura y la UX al final.
- ⬆ **Mejoras:**
  1. **Dónde encajan las ADR vigentes.** Son decisiones ya aprobadas por el propietario. Propongo este orden:
     - leyes;
     - anti-patrones;
     - seguridad;
     - **ADR vigentes**;
     - el resto de los documentos.

     Si un documento (por ejemplo, la estructura del documento 5) choca con una ADR, el conflicto no se resuelve solo por la tabla: lo decide el propietario y queda en una ADR nueva. Así ninguna decisión aprobada se deroga sin que nadie lo sepa.
  2. **La accesibilidad no puede quedar en el último puesto** por pertenecer a la UX (documento 8). Ya es ley (13), así que sube con las leyes. Lo mismo vale para la exactitud (ley 11).
  3. **Los principios del documento 2 no aparecen en el orden**, solo sus anti-patrones. Propongo situarlos justo después de los anti-patrones, como criterio de diseño cuando nada más decide.
  4. **La instrucción final** ("antes de implementar cualquier feature, verifica…") la convertimos en regla de AGENTS.md: todo PR de producto declara qué leyes toca y que no introduce anti-patrones (ya en #82).

## Decisiones para el propietario
1. **Estructura de carpetas:** mantener la actual (`backend/crates`, `frontend`), ya vigilada por CI, y crear solo `echarts-safe`. *Recomendado.*
2. **Formato de métricas:** mantener Apache Ossie y añadir `owner`, `tags`, `status` y la lista restrictiva de dimensiones en la extensión `bi-platform`, en lugar de un YAML propio. *Recomendado.*
3. **Datos hacia el navegador:** JSON exacto para pantallas y Arrow para grandes volúmenes, con decimales siempre exactos. *Recomendado.*
4. **Seguridad:** adoptar los roles `viewer`, `analyst`, `modeler` y `admin`, CLS de tres niveles y API keys con *scopes*, manteniendo las políticas como parámetros tipados (nunca plantillas de texto). *Recomendado.*
5. **Nuevas capacidades en la hoja de ruta:** *time intelligence* (G2/G4), *what-if* (G5) y *embedded analytics* (con ADR 0011).
6. **Orden de prioridad:** leyes → anti-patrones → seguridad → **ADR vigentes** → principios → resto de documentos. Accesibilidad y exactitud cuentan como leyes. Un choque con una ADR lo decide el propietario.
