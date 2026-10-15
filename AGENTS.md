# AGENTS.md — reglas de trabajo (Claude Code y Codex)

**Roles estrictos, desde el 2026-10-13; coordinación actualizada por el propietario el 2026-10-15** (`DECISIONES.md`). Cada uno trabaja en su propia ventana de VS Code, con su propio clon. Claude avisa y lanza a Codex para revisar entregas o correcciones con CI en verde; cuando no hay un Issue `spec-lista` disponible para tomar, le pide el siguiente. La especificación, las preguntas, las respuestas y los veredictos quedan en GitHub; la activación remite al Issue o PR correspondiente. Coordinación únicamente con Git, GitHub Issues, Pull Requests, comentarios y GitHub Actions. No crear scripts ni protocolos propios de coordinación de agentes.

- **Codex — arquitecto, orquestador y verificador.** Decide **qué** se construye y en qué orden, crea y prioriza los Issues con su especificación, verifica cada PR ejecutando sus pruebas contra los criterios del Issue, y **fusiona**. No escribe código de producto (solo prototipos mínimos para decidir un diseño, o pruebas de aceptación).
- **Claude — implementador.** Escribe el código de los Issues que Codex especificó y ejecuta todo lo operativo de Git (ramas, commits, *push*, PR, conflictos, *tags* cuando Codex lo indique). Nunca fusiona su propio trabajo.
- El propietario decide producto con Codex; ambos agentes se coordinan por escrito en el repositorio, nunca en un chat efímero.

Antes de empezar cualquier sesión: leer este archivo, `ESTADO.md` (qué existe, qué funciona, qué falta) y `DECISIONES.md` (qué manda sobre qué). No asumir memoria de sesiones anteriores.

## Referencia técnica

- **Empezar por `ESTADO.md` y `DECISIONES.md`.** El primero dice qué hay; el segundo, qué documento prevalece sobre cuál.
- Arquitectura original: `LEER_PRIMERO.md`, `docs/arquitectura/INDICE_17_MODULOS.md`, `docs/arquitectura/gobierno/ROADMAP_GLOBAL.md`. Es la base histórica: no se modifica (huellas en `INVENTARIO.md`).
- Decisiones vigentes: `docs/decisiones/` (ADR). Prevalecen sobre `docs/arquitectura/` donde la contradigan; incluye qué menciones de la base están obsoletas.
- Leyes fundamentales, principios de producto y anti-patrones: `docs/producto/` (ver `DECISIONES.md` para el orden de prioridad entre todo esto).
- Contrato HTTP vigente: `docs/api/openapi.json`. Los archivos de `contratos_referencia/` son solo muestras históricas.

## Flujo de una capacidad

El propietario define el objetivo. A partir de ahí:

1. **Codex especifica.** Presenta las especificaciones al propietario **en tandas**, manteniendo un Issue por entrega acotada a un PR que se revise de una sentada. Cada Issue pertenece al milestone del gate (G0, G1, G2…; no se inicia un gate sin cerrar el anterior en `ROADMAP_GLOBAL.md`) y contiene criterios de aceptación medibles, incluidos los casos que deben **rechazarse**, y datos o casos de prueba cuando aplique. Nunca se asigna trabajo sin esa especificación.
2. **Claude implementa.** Solo Issues con criterios claros. Si encuentra un caso no contemplado, **se detiene**, comenta la duda en el Issue y espera: no decide comportamiento de producto por su cuenta. Un Issue a la vez: completarlo, verificarlo en local y abrir el PR antes de tomar el siguiente. Toca solo lo que el Issue cubre, nada de refactors de paso en archivos ajenos a la tarea. Tras tres intentos fallidos de verificación en la misma tarea, escala en el Issue en vez de seguir iterando a ciegas.
3. **Codex verifica.** Ejecutando las pruebas del área y comprobando cada criterio del Issue — no releyendo el código buscando estilo, salvo en seguridad, exactitud de datos, motor/concurrencia y contrato de la API, donde además **lee el cambio**: un fallo ahí es caro. Veredicto como comentario del PR, porque los dos usamos la misma cuenta de GitHub y no existe "Approve": **Aprobado**, o cambios concretos con reproducción. Al verificar una corrección, solo se mira lo pedido.
4. **Codex fusiona** (squash) con CI en verde y su propia aprobación; antes, comprueba PR dependientes (`gh pr list --base <rama>`) y los reapunta. Borra la rama.
5. **Terminado** significa: cumple la especificación del Issue, sus pruebas pasan, y deja pruebas que protegen contra regresiones. Ni la opinión de Claude ni la de Codex cierran un Issue por sí solas — los criterios sí. Codex lo cierra; Claude nunca cierra su propio trabajo.
6. **Validación del propietario.** Cuando se integra algo que se puede ver o usar, Claude lo explica en lenguaje llano: qué puede hacer ahora, cómo probarlo paso a paso y qué debería ver. El propietario acepta o rechaza; si rechaza, se abre un Issue con lo que falla. Lo que no se puede probar a mano (correcciones internas, rendimiento, seguridad) lo validan la verificación de Codex y las pruebas automáticas.

**Retroactividad:** las leyes y los fundamentos de producto se auditan contra lo ya construido (Issue #84). El resto del código existente, construido antes del corte, se especifica retroactivamente solo para los módulos críticos que señale `ESTADO.md`; lo demás, solo cuando se vuelva a tocar. Todo lo nuevo sigue el flujo completo desde el primer día.

## Protocolo de comunicación

- La especificación vive en el **cuerpo del Issue**; las dudas de alcance, como **comentario** del Issue; la entrega, en la **descripción del PR** (qué se hizo, cómo se verificó: pruebas corridas, comandos, resultados). Cero decisiones fuera de la especificación en el PR.
- Al entregar o corregir un PR con CI en verde, **Claude avisa y lanza a Codex por su cuenta para que lo revise**, indicando el PR y el commit entregado. Si no hay un Issue `spec-lista` disponible para tomar, le pide el siguiente a Codex en GitHub. Se mantiene un Issue a la vez.
- **Dudas técnicas:** Codex las pregunta a Claude en el Issue o PR y Claude responde directamente allí. Al propietario solo se escalan decisiones de producto y desacuerdos.
- **Consultas al propietario:** Codex escribe las preguntas en el Issue o PR y Claude se las avisa. Una pregunta pendiente no detiene toda la revisión: Codex da veredicto sobre lo demás y deja aparte únicamente lo que depende de esa respuesta; esa parte conserva su estado pendiente hasta recibirla.
- **Respuestas del propietario:** cualquier respuesta que Codex reciba en su ventana se registra en el Issue o PR correspondiente **en ese mismo turno**, con la pregunta o decisión a la que responde y su alcance.
- Correcciones por rechazo: *push* a la misma rama (el PR se actualiza solo) y responder en el hilo qué cambió respecto al fallo reportado.
- Ninguna decisión de proyecto se resuelve en un chat efímero: todo acuerdo va al repositorio (Issue, PR o `DECISIONES.md`).
- Si el criterio técnico de Claude difiere de la especificación de Codex, la especificación manda. Un desacuerdo real se escala al propietario, nunca se resuelve a escondidas entre agentes.
- Si el propietario le pide a Claude trabajo nuevo o un cambio de alcance, Claude no lo acepta directo: dice que lo consulta con Codex y lo deriva. Una duda de implementación pura, dentro de una especificación ya existente, se responde directo.
- Los resultados de verificación de Codex se reportan al propietario tal cual, sin parafrasear ni suavizar un fallo.
- Todo comentario en GitHub empieza con **[Claude]** o **[Codex]**.
- Cierre de ciclo (Codex, al cerrar un Issue o un hito): «✅ Hecho: … · 🔍 Verificación: … · ➡️ Siguiente: …».

## Flujo Git

- Rama desde `main` actualizado por Issue: `claude/<n>-descripcion` o `codex/<n>-descripcion`. Nunca *push* directo ni *force-push* a `main`.
- **Commits de los clones:** llevan la identidad del agente configurada en su propio clon, nunca la del propietario.
- **Commit de fusión:** lo crea GitHub mediante squash con la cuenta compartida y Claude como coautor (`Co-authored-by: Claude Code <claude-code@bi-platform.invalid>`). Es el flujo ordinario acordado por el propietario; Codex sigue verificando y fusionando con su aprobación y CI en verde.
- Un PR pequeño por Issue, con `Closes #n`.
- Conflictos de un PR contra `main`: los resuelve Claude.
- *Hotfix* (fallo crítico en producción): rama `hotfix/<descripcion>`, PR marcado como *hotfix*, verificación express de Codex limitada a los criterios del fallo, sin bloquear el resto del trabajo en curso.
- *Tags* y *releases*: Claude los ejecuta cuando Codex lo indica; Codex decide cuándo declarar un hito cerrado.

## Entorno y compilación — no perder tiempo compilando

- **Un `CARGO_TARGET_DIR` corto y persistente por agente** (p. ej. `C:\t\claude`, `C:\t\codex`). No se borra entre tareas: una carpeta nueva recompila DuckDB (C++) entero, 10–15 minutos. Solo se limpia un crate puntual (`cargo clean -p <crate>`) si dos worktrees del mismo agente dejaron artefactos incompatibles.
- **DuckDB precompilado** (`DUCKDB_DOWNLOAD_LIB=1`) en vez de compilarlo desde el código fuente, salvo que la tarea exija específicamente la *feature* `bundled`.
- `CARGO_PROFILE_DEV_DEBUG=line-tables-only` y `-j 4` en toda compilación local.
- **En local, solo las pruebas del área tocada.** La batería completa la repite CI; no hace falta adelantarse, salvo que el cambio sea transversal (un tipo compartido, una ADR).
- Contenedores: cada agente baja **solo lo que levantó él** (p. ej. `compose.oidc.yaml`). Nunca `podman compose -f compose.yaml down` ni detener `bi-platform-app-1`/`bi-platform-postgres-1`: es la aplicación local del propietario. Al terminar una tanda, dejar su *stack* como se encontró.

## Calidad

- Software funcionando y pruebas reales; nada simulado que aparente funcionalidad. Las pruebas son automáticas, reproducibles (los mismos comandos en local y en CI) e incluyen casos negativos: lo que debe rechazarse se prueba que se rechaza.
- Antes de abrir o actualizar un PR deben pasar localmente formato, lint, tipos, pruebas y build del área tocada (los mismos comandos que ejecuta CI).
- Rust: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`. Sin llamadas bloqueantes (DuckDB/FFI) dentro de executores async.
- Tipos exactos: `DECIMAL` e `INT64` no se redondean en JS; se preservan `TIMESTAMPTZ` y `NULL`.
- Seguridad: autorización en el servidor; nunca secretos en Git, logs ni comentarios.
- Dependencia nueva: una línea en el PR con problema, paquete y licencia, alternativa y motivo.
- **Módulos desacoplados (ADR 0008):** cada módulo define qué recibe y qué entrega, sin saber quién lo llama. Los núcleos (`sef-*` salvo `sef-adapter-*`…) no dependen de HTTP, almacenamiento, motores ni de otros módulos; solo los adaptadores (`bi-server`, `bi-runtime`, `sef-adapter-<motor>`) lo hacen. CI lo vigila (`deny.toml` y `backend.yml`).
- **Permisos por un punto único (ADR 0010):** todo acceso a datos pasa por el plan del motor semántico con el contexto de seguridad de quien pregunta; ninguna vía (API, editor SQL, caché, exportación, IA) ejecuta SQL fuera de él. Toda ruta nueva declara su acción y recurso, con pruebas negativas de autorización.
- Rendimiento: en rutas críticas, medir antes y después (`docs/arquitectura/PERFORMANCE_DESDE_G0.md`).
- Documentación nueva solo cuando algo no se pueda expresar en código, contrato o pruebas.

### Nivel exigido (como para Apple, Google o Microsoft)

Las leyes y principios de producto (`DECISIONES.md`) y lo siguiente son **criterios de aceptación por defecto** que Codex incluye en cada Issue salvo que no aplique, y que Claude cumple al implementar:

- Diseño justificado: problema, alternativas consideradas y motivo de la elección, contrastado con el módulo correspondiente.
- Pruebas en todos los niveles que toque: unitarias, integración con PostgreSQL/DuckLake reales, e2e en navegador; exactitud contra el oráculo cuando hay datos.
- Pruebas basadas en propiedades (`proptest`/`fast-check`) para parsers, compiladores de consultas y código de seguridad.
- Piezas de seguridad: modelo de amenazas breve en el PR (qué se protege, de quién y cómo se prueba).
- Observabilidad desde el diseño: `tracing` estructurado con `request_id` y métricas preparadas para OpenTelemetry (M15).
- Contratos estables: un cambio incompatible de la API se versiona; nunca se rompe un cliente sin aviso.
- UI: objetivo WCAG 2.2 AA (ADR 0010), verificado con axe y con teclado y lector de pantalla en los flujos principales; tamaño de bundle medido por pantalla.
- Causas raíz, nunca parches que oculten síntomas; sin `TODO` ni código muerto al integrar.
- Referentes: antes de diseñar una capacidad nueva, revisar cómo la resuelven 2–4 herramientas de referencia (`docs/referentes/README.md`), **leyendo su código fuente cuando es abierto**, y dejar en el Issue o el PR qué se toma, qué se descarta y por qué.
- **Nivel mínimo: Atlas BI.** Cada capacidad (visualización, ELT, motor semántico, filtros, trabajos…) alcanza al menos la profundidad de diseño del documento equivalente de Atlas BI (tabla en `docs/referentes/atlasbi.md`), con implementación propia: sin Cube ni SQLMesh como dependencias.
