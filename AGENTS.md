# AGENTS.md — reglas de trabajo (Claude Code y Codex)

Dos ingenieros full-stack, cada uno en su propio clon y rama. Ninguno tiene un área fija (backend/frontend): los Issues se toman por disponibilidad y dependencias, y cada capacidad se entrega de extremo a extremo (Rust + UI + pruebas). Coordinación únicamente con Git, GitHub Issues, Pull Requests, comentarios y GitHub Actions. No crear scripts ni protocolos propios de coordinación.

## Referencia técnica

- Arquitectura: `LEER_PRIMERO.md`, `docs/arquitectura/INDICE_17_MODULOS.md`, `docs/arquitectura/DECISIONES_TECNICAS_RC2.md`, `docs/arquitectura/gobierno/ROADMAP_GLOBAL.md`; consultar el módulo pertinente a cada tarea.
- **Decisiones vigentes: `docs/decisiones/` (ADR). Prevalecen sobre `docs/arquitectura/` donde la contradigan.** Toda desviación de la base se registra como ADR; las de producto, con la aprobación del propietario.
- `docs/arquitectura/` es la base histórica original: no se modifica (hashes en `INVENTARIO.md`).
- Contrato HTTP vigente: `docs/api/openapi.json`. Los archivos de `contratos_referencia/` son solo muestras.

## Flujo

1. Se avanza por gates del roadmap en orden (G0, G1, …). No se inicia un gate sin cumplir los criterios funcionales del anterior.
2. Cada tarea es un Issue con criterios de aceptación y dependencias, dentro del milestone del gate. Para tomarlo: etiqueta `agent:claude` o `agent:codex` y comentario breve.
3. Rama desde `main` actualizado: `claude/<n>-descripcion` o `codex/<n>-descripcion`. Nunca push directo ni force-push a `main`.
4. Un PR pequeño por Issue, con `Closes #n`, qué cambia, cómo se probó y riesgos abiertos.
5. Revisión cruzada solo donde un fallo es caro: seguridad y sesiones, exactitud de los datos (también su presentación en la UI), motor y concurrencia, y contrato de la API. El autor indica en el PR si la necesita. Es una sola ronda; al verificar correcciones solo se mira lo pedido. Lo demás (estilos, textos, documentación, CI, pruebas y cambios pequeños) se integra con CI en verde, sin revisión. Ambos usamos la misma cuenta GitHub, por eso no existe "Approve": se aprueba con `gh pr review --comment` escribiendo **Aprobado**, o se piden cambios concretos.
6. Merge (squash) por el autor con CI en verde y, si requiere revisión, con la aprobación del otro agente; después se borra la rama.
7. Todo comentario en GitHub empieza con **[Claude]** o **[Codex]**.
8. Commits con la identidad del agente configurada en su clon, nunca la del propietario.
9. Decisiones técnicas rutinarias: las toman los agentes. Decisiones fundamentales de producto: se consultan al propietario (@eribertocz) en el Issue.

## Calidad

- Software funcionando y pruebas reales; nada simulado que aparente funcionalidad.
- Antes de abrir o actualizar un PR deben pasar localmente formato, lint, tipos, pruebas y build del área tocada (los mismos comandos que ejecuta CI).
- Rust: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`. Sin llamadas bloqueantes (DuckDB/FFI) dentro de executores async.
- Tipos exactos: `DECIMAL` e `INT64` no se redondean en JS; se preservan `TIMESTAMPTZ` y `NULL`.
- Seguridad: autorización en el servidor; nunca secretos en Git, logs ni comentarios.
- Dependencia nueva: una línea en el PR con problema, paquete y licencia, alternativa y motivo.
- **Módulos desacoplados (ADR 0008):** cada módulo define qué recibe y qué entrega, sin saber quién lo llama. Los núcleos (`sef-*`…) no dependen de HTTP, almacenamiento, motores ni de otros módulos; solo los adaptadores (`bi-server`, `bi-runtime`) lo hacen. CI lo vigila (`deny.toml` y `backend.yml`).
- Rendimiento: en rutas críticas, medir antes y después (`docs/arquitectura/PERFORMANCE_DESDE_G0.md`).
- Documentación nueva solo cuando algo no se pueda expresar en código, contrato o pruebas.

### Nivel exigido (como para Apple, Google o Microsoft)

Cada PR, de cualquiera de los dos, cumple esto o explica por qué no aplica:
- Diseño justificado: problema, alternativas consideradas y motivo de la elección, contrastado con el módulo correspondiente.
- Pruebas en todos los niveles que toque: unitarias, integración con PostgreSQL/DuckLake reales, e2e en navegador; exactitud contra el oráculo cuando hay datos.
- Pruebas basadas en propiedades (`proptest`/`fast-check`) para parsers, compiladores de consultas y código de seguridad.
- Piezas de seguridad: modelo de amenazas breve en el PR (qué se protege, de quién y cómo se prueba).
- Observabilidad desde el diseño: `tracing` estructurado con `request_id` y métricas preparadas para OpenTelemetry (M15).
- Contratos estables: un cambio incompatible de la API se versiona; nunca se rompe un cliente sin aviso.
- UI: accesibilidad verificada (axe) y tamaño de bundle medido por pantalla.
- Causas raíz, nunca parches que oculten síntomas; sin `TODO` ni código muerto al integrar.
- Referentes: antes de diseñar una capacidad nueva, revisar cómo la resuelven 2–4 herramientas de referencia (`docs/referentes/README.md`), **leyendo su código fuente cuando es abierto** (ELT: SQLMesh y Rill; motor semántico: Cube, Malloy, MetricFlow y Rill; visualización: Rill, cuya UI es Svelte, Superset y Grafana), y dejar en el PR qué se toma, qué se descarta y por qué.
- **Nivel mínimo: Atlas BI.** Cada capacidad (visualización, ELT, motor semántico, filtros, trabajos…) alcanza al menos la profundidad de diseño del documento equivalente de Atlas BI (`E:\bi-platform\docs\`, tabla en `docs/referentes/atlasbi.md`), con implementación propia: sin Cube ni SQLMesh como dependencias.
- Revisión (cuando aplica): el revisor lee el cambio y ejecuta las pruebas afectadas para comprobar el comportamiento. No reconstruye imágenes ni repite lo que CI ya ejecuta.
