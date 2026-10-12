# AGENTS.md — reglas de trabajo (Claude Code y Codex)

Dos ingenieros full-stack, cada uno en su propio clon y rama. Ninguno tiene un área fija (backend/frontend): los Issues se toman por disponibilidad y dependencias, y cada capacidad se entrega de extremo a extremo (Rust + UI + pruebas). Coordinación únicamente con Git, GitHub Issues, Pull Requests, comentarios y GitHub Actions. No crear scripts ni protocolos propios de coordinación.

## Referencia técnica

- Arquitectura: `LEER_PRIMERO.md`, `docs/arquitectura/INDICE_17_MODULOS.md`, `docs/arquitectura/DECISIONES_TECNICAS_RC2.md`, `docs/arquitectura/gobierno/ROADMAP_GLOBAL.md`; consultar el módulo pertinente a cada tarea.
- **Decisiones vigentes: `docs/decisiones/` (ADR). Prevalecen sobre `docs/arquitectura/` donde la contradigan.** Toda desviación de la base se registra como ADR; las de producto, con la aprobación del propietario.
- `docs/arquitectura/` es la base histórica original: no se modifica (hashes en `INVENTARIO.md`).
- Producto: `docs/producto/` (blueprint del propietario y cómo se incorpora: capacidades, fronteras de seguridad, siguientes entregas).
- Contrato HTTP vigente: `docs/api/openapi.json`. Los archivos de `contratos_referencia/` son solo muestras.

## Flujo

Cada capacidad sigue cinco pasos: **el propietario define el objetivo → un agente implementa → el otro revisa → las pruebas automáticas lo verifican → el propietario valida lo que se puede ver o usar.**

1. Se avanza por gates del roadmap en orden (G0, G1, …). No se inicia un gate sin cumplir los criterios funcionales del anterior.
2. El propietario define el objetivo; los agentes lo convierten en Issues con criterios de aceptación (incluidos los casos que deben rechazarse) y dependencias, dentro del milestone del gate. Para tomarlo: etiqueta `agent:claude` o `agent:codex` y comentario breve.
3. Rama desde `main` actualizado: `claude/<n>-descripcion` o `codex/<n>-descripcion`. Nunca push directo ni force-push a `main`.
4. Un PR pequeño por Issue, con `Closes #n`, qué cambia, cómo se probó y riesgos abiertos. El autor entrega código, pruebas automáticas y la documentación necesaria.
5. Todo PR lo revisa el otro agente, en una sola ronda: errores, seguridad, arquitectura y rendimiento. Revisión **completa** (lee el cambio y ejecuta las pruebas afectadas) donde un fallo es caro: seguridad y sesiones, exactitud de los datos (también su presentación en la UI), motor y concurrencia, y contrato de la API. Revisión **ligera** (lectura del diff, sin compilar) en estilos, textos, documentación, CI y pruebas. Al verificar correcciones solo se mira lo pedido. Ambos usamos la misma cuenta GitHub, por eso no existe "Approve": se aprueba con `gh pr review --comment` escribiendo **Aprobado**, o se piden cambios concretos.
6. Merge (squash) por el autor con CI en verde y la aprobación del otro agente; después se borra la rama.
7. Todo comentario en GitHub empieza con **[Claude]** o **[Codex]**.
8. Commits con la identidad del agente configurada en su clon, nunca la del propietario.
9. Decisiones técnicas rutinarias: las toman los agentes. Decisiones fundamentales de producto: se consultan al propietario (@eribertocz) en el Issue.
10. **Validación del propietario.** Cuando se integra algo que se puede ver o usar, Claude se lo explica en lenguaje llano: qué puede hacer ahora, cómo probarlo paso a paso y qué debería ver. El propietario lo acepta o lo rechaza; si lo rechaza, se abre un Issue con lo que falla. Lo que no se puede probar a mano (correcciones internas, rendimiento, seguridad) se le informa, pero lo validan la revisión y las pruebas automáticas.

## Principios de producto (todas las capas y módulos)

Definidos por el propietario (2026-10-11). Cada capacidad rescata lo mejor de las herramientas BI de referencia y **va más allá**: la documentación y los referentes son el suelo, no el techo. Todo diseño y todo PR explica cómo cumple cada principio, o por qué no aplica.

| Principio | Qué significa | Cómo se comprueba |
|---|---|---|
| **Facilidad** | La persona dice qué necesita, no cómo hacerlo. Hay valores por defecto sensatos, el usuario puede anularlos y el sistema explica cada decisión que toma por él | El caso común se hace sin configurar nada; la elección automática es visible y anulable |
| **Velocidad** | Respuesta inmediata en lo interactivo y ningún trabajo repetido sin necesidad | Medida antes y después en las rutas críticas (`PERFORMANCE_DESDE_G0.md`) |
| **Modernidad** | Estándares abiertos y la mejor técnica disponible (Ossie, Arrow, DuckDB y DuckLake, OIDC, OpenTelemetry), nunca la copia de un producto anterior | El PR nombra qué estándar o técnica usa y qué hace mejor que los referentes |
| **Reproducibilidad** | Lo mismo de entrada da lo mismo de salida: definiciones en Git, versiones de datos (DuckLake) y huellas deterministas | Repetir con la misma versión da los mismos bytes o el mismo resultado; probado |
| **Trazabilidad** | Cada número sabe de dónde viene (linaje hasta la fuente), con qué definición y versión se calculó, y quién hizo qué (auditoría) | Desde un valor en pantalla se llega a su definición, su versión de datos y su plan |
| **Exactitud demostrable** | Nunca un número equivocado en silencio: o se demuestra correcto, o se rechaza con un motivo claro | Pruebas contra el oráculo, corpus normativo y casos negativos |
| **Seguridad por diseño** | Permisos en un punto único, denegación por defecto y ningún secreto expuesto (ADR 0010) | Modelo de amenazas y pruebas negativas de autorización |
| **Apertura** | Sin encierro: entra y sale en formatos estándar; el motor también se usa fuera (ADR 0011) | Importar y exportar sin pérdida, o con la pérdida declarada |

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

Cada PR, de cualquiera de los dos, cumple esto o explica por qué no aplica:
- Diseño justificado: problema, alternativas consideradas y motivo de la elección, contrastado con el módulo correspondiente.
- Pruebas en todos los niveles que toque: unitarias, integración con PostgreSQL/DuckLake reales, e2e en navegador; exactitud contra el oráculo cuando hay datos.
- Pruebas basadas en propiedades (`proptest`/`fast-check`) para parsers, compiladores de consultas y código de seguridad.
- Piezas de seguridad: modelo de amenazas breve en el PR (qué se protege, de quién y cómo se prueba).
- Observabilidad desde el diseño: `tracing` estructurado con `request_id` y métricas preparadas para OpenTelemetry (M15).
- Contratos estables: un cambio incompatible de la API se versiona; nunca se rompe un cliente sin aviso.
- UI: objetivo WCAG 2.2 AA (ADR 0010), verificado con axe y con teclado y lector de pantalla en los flujos principales; tamaño de bundle medido por pantalla.
- Causas raíz, nunca parches que oculten síntomas; sin `TODO` ni código muerto al integrar.
- Referentes: antes de diseñar una capacidad nueva, revisar cómo la resuelven 2–4 herramientas de referencia (`docs/referentes/README.md`), **leyendo su código fuente cuando es abierto** (ELT: SQLMesh y Rill; motor semántico: Cube, Malloy, MetricFlow y Rill; visualización: Rill, cuya UI es Svelte, Superset y Grafana), y dejar en el PR qué se toma, qué se descarta y por qué.
- **Nivel mínimo: Atlas BI.** Cada capacidad (visualización, ELT, motor semántico, filtros, trabajos…) alcanza al menos la profundidad de diseño del documento equivalente de Atlas BI (`E:\bi-platform\docs\`, tabla en `docs/referentes/atlasbi.md`), con implementación propia: sin Cube ni SQLMesh como dependencias.
- Revisión completa (regla 5): el revisor lee el cambio y ejecuta las pruebas afectadas para comprobar el comportamiento. No reconstruye imágenes ni repite lo que CI ya ejecuta.
