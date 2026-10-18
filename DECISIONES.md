# Decisiones

*Índice de todo lo decidido: qué manda sobre qué. No duplica el contenido de cada documento, solo dice dónde está y en qué orden se aplica. Escrito por Claude el 2026-10-13, para el corte a roles.*

## Orden de prioridad cuando dos documentos chocan

**Decidido por el propietario, 2026-10-12** (`docs/producto/propuestas/version-comun.md`, decisión 10):

```
1. Leyes fundamentales      docs/producto/leyes-bi.md (13 leyes; prioridad 1, 2, 3 y 5)
2. Anti-patrones            fundamentos-propietario.md, «Documento 2»
3. Seguridad                ADR 0010 y 0012; AGENTS.md
4. ADR vigentes             docs/decisiones/ (0001-0015)
5. Principios de producto    AGENTS.md («facilidad, velocidad, modernidad...»)
6. Resto de documentos       docs/arquitectura/, docs/semantica/, docs/referentes/...
```

- La accesibilidad y la exactitud cuentan como leyes (11 y 13 de `leyes-bi.md`).
- Si una ley choca con una ADR, decide el propietario y queda en una ADR **nueva**; ninguna ADR se anula en silencio.
- Las leyes y los fundamentos son **retroactivos**: lo ya construido se audita contra ellos (Issue #84).

## Las piezas, cada una en su sitio

| Qué | Dónde | Quién lo aprueba |
|---|---|---|
| **13 leyes fundamentales** | [`docs/producto/leyes-bi.md`](docs/producto/leyes-bi.md) | Propietario |
| **Anti-patrones y principios de diseño** | [`docs/producto/propuestas/fundamentos-propietario.md`](docs/producto/propuestas/fundamentos-propietario.md), «Documento 2» | Propietario (adoptado como objetivo, decisión 9 abajo) |
| **15 ADR** (0001–0015) | [`docs/decisiones/README.md`](docs/decisiones/README.md) | Propietario (producto) o rutinaria (técnica) |
| **10 decisiones de producto aprobadas el 2026-10-12** (ELT, Estudio, bibliotecas, fundamentos, orden de prioridad...) | [`docs/producto/propuestas/version-comun.md`](docs/producto/propuestas/version-comun.md), apartado 5 | Propietario |
| **Blueprint del propietario** (visión original) | [`docs/producto/BLUEPRINT_2026-10-11.md`](docs/producto/BLUEPRINT_2026-10-11.md) | — (ADR manda donde contradiga) |
| **Base histórica de arquitectura** (no se modifica) | [`docs/arquitectura/`](docs/arquitectura/INDICE_17_MODULOS.md), huellas en `INVENTARIO.md` | — (ADR la sustituye donde aplica) |
| **Diseño normativo del motor semántico** | [`docs/semantica/`](docs/semantica/README.md) | — (ADR manda donde contradiga) |
| **Reglas de trabajo y calidad** | [`AGENTS.md`](AGENTS.md) | Propietario |
| **Estado y evidencia de entregas** | [Issues](https://github.com/eribertocz/bi-platform/issues), [PR](https://github.com/eribertocz/bi-platform/pulls) y [milestones](https://github.com/eribertocz/bi-platform/milestones); [#128](https://github.com/eribertocz/bi-platform/issues/128) define el orden de G2 | Codex especifica/verifica; Claude implementa; GitHub registra abierto, cerrado e integrado |

## Decisión: una fuente por dato y retirada del archivo de estado (2026-10-18)

**Aprobada por el propietario**, con aplicación de una modificación por vez ([registro y entrega acotada en #214](https://github.com/eribertocz/bi-platform/issues/214)). Sustituye la obligación de leer y actualizar el archivo de estado del proyecto:

- El Issue contiene alcance y criterios; el PR, implementación, pruebas y veredicto. El estado abierto/cerrado/integrado se consulta en GitHub, sin transcribirlo. #128 conserva prioridades y dependencias; no es otra copia de los estados o las especificaciones. Su simplificación es una entrega posterior.
- Los contratos y pruebas del área permiten verificar el producto; la arquitectura, las leyes y las ADR conservan su autoridad. Un comportamiento existente contrario a ellas es un defecto, no una decisión que las sustituya.
- Las versiones siguen en ADR 0014 y la dirección posterior en su propuesta/Issues; los pendientes mantienen sus Issues/gates. La investigación original y sus huellas no se modifican. El inventario y los hallazgos del archivo retirado conservan trazabilidad en #214 y en Git; no se crea otro documento de estado.
- Codex prepara las referencias pertinentes al asignar una entrega. Claude consulta reglas, Issue/PR y fuentes del área; no reconstruye la arquitectura ni el orden de trabajo. Los Issues/PR ya iniciados conservan su alcance y prioridad.
- Este primer cambio solo retira el archivo y actualiza sus referencias. La depuración de #128, de los Issues pendientes y del resto de instrucciones, así como cualquier cambio de CI, se hace por separado. No reduce funcionalidades, calidad ni auditorías, no cierra gates y no autoriza releases.

## Decisión: roles estrictos (2026-10-13)

**Aprobada por el propietario.** Hasta el 2026-10-12, Claude y Codex trabajaban con revisión cruzada, sin roles fijos. Desde el 2026-10-13:

- **Codex** es arquitecto, orquestador y verificador: crea y prioriza los Issues con sus criterios de aceptación, puede escribir pruebas de aceptación (nunca código de producto), verifica cada PR ejecutando sus pruebas, y **integra** (el verificador cierra, el autor nunca fusiona su propio trabajo).
- **Claude** es el implementador: solo trabaja issues que Codex especificó, escribe el código y ejecuta todo lo operativo de Git.
- Cada uno en su propia ventana de VS Code y con su propio clon. La regla inicial de no lanzarse entre agentes se actualiza por la decisión del propietario del 2026-10-15 recogida a continuación.
- El trabajo en vuelo antes del corte se cerró con las reglas anteriores (PR #104–#111); este corte empieza con `main` en ese punto.
- **No sustituye ninguna ley, ADR ni decisión de producto anterior:** es una decisión de proceso, sobre cómo se trabaja, no sobre qué se construye.

## Decisión: activación por Claude, consultas y autoría de fusiones (2026-10-15)

**Aprobada expresamente por el propietario.** Se mantienen los roles estrictos: Codex especifica, verifica y fusiona; Claude implementa y ejecuta lo operativo de Git en su clon, sin fusionar su propio trabajo. Esta decisión actualiza la coordinación y la interpretación de la identidad del commit de fusión; no cambia el alcance de producto, las especificaciones aceptadas ni los criterios de calidad.

- **Aviso y activación:** al entregar o corregir un PR con CI en verde, Claude avisa y lanza a Codex por su cuenta para que lo revise. Cuando no hay un Issue `spec-lista` disponible para tomar, le pide el siguiente. Toda la coordinación sigue en GitHub: la activación remite al Issue o PR donde están la entrega, las preguntas y los veredictos. Se mantienen los clones separados, un Issue a la vez y la prohibición de crear scripts o protocolos propios de coordinación.
- **Dudas técnicas:** Codex se las pregunta a Claude en GitHub y Claude responde directamente. Al propietario solo le llegan decisiones de producto y desacuerdos.
- **Revisión mientras se consulta:** una pregunta al propietario no detiene la revisión completa. Codex da veredicto sobre todo lo demás y deja aparte únicamente lo que depende de la respuesta, sin dar por resuelta esa parte mientras esté pendiente.
- **Preguntas al propietario:** se escriben en el Issue o PR correspondiente y Claude se las avisa.
- **Respuestas en la ventana de Codex:** toda respuesta del propietario recibida allí se registra en el Issue o PR correspondiente en ese mismo turno, con su contexto y alcance.
- **Especificaciones:** Codex las presenta al propietario en tandas; cada entrega conserva su Issue acotado, sus criterios medibles y su PR, conforme a `AGENTS.md`.
- **Identidad:** la regla de usar la identidad del agente, nunca la del propietario, aplica a los commits creados en los clones. El commit de fusión squash lo crea GitHub con la cuenta compartida y Claude como coautor. Este es el flujo ordinario de fusión; la aprobación de Codex y el CI verde siguen siendo requisitos. La autorización puntual aplicada al PR #119 queda concretada por esta regla general para las fusiones siguientes.

Se actualiza `AGENTS.md` para reflejar esta decisión y se sustituye la prohibición inicial de que Claude active a Codex. La activación no sustituye el registro de la coordinación en GitHub.

## Decisión: entregas pequeñas para trabajo antiguo y futuro (2026-10-17)

**Instrucción del propietario** ([registrada en #128](https://github.com/eribertocz/bi-platform/issues/128#issuecomment-6237486314)), que precisa la regla vigente de `AGENTS.md` y el acuerdo de proceso de #128. Es una decisión de proceso, no de producto:

- Los Issues antiguos y los nuevos se organizan como entregas pequeñas o medianas, con un resultado revisable por PR. Ningún paraguas se toma como una sola implementación.
- Codex prepara cada entrega antes de asignarla (dependencias, alcance, aceptación medible, negativos, datos y referentes cuando apliquen) y conserva la aprobación de producto al dividir. El comportamiento nuevo necesita la conformidad del propietario, en tandas, antes de `spec-lista`.
- Claude propone la división en el Issue cuando encuentra otro frente independiente, y Codex la resuelve en GitHub. Un Issue a la vez: una espera de revisión o de CI no adelanta dependencias sin resolver.
- Cada pregunta al propietario lleva recomendación, motivo y consecuencias en lenguaje sencillo.
- No fija un tamaño por líneas ni por horas, no crea scripts ni protocolos nuevos, no cambia los roles ni reduce las pruebas.
- No aprueba capacidades futuras, no permite tocar `main` directamente ni saltarse gates, y no cambia prioridades ni aplazamientos pendientes.

## Decisión: versiones por gate (2026-10-13)

**Aprobada por el propietario** ([conformidad en #116](https://github.com/eribertocz/bi-platform/issues/116#issuecomment-6159458610)). El plan vigente de versiones está en [ADR 0014](docs/decisiones/0014-versiones-por-gate.md): cortes alpha en G2 y G3, beta del alcance BI inicial desde G4 y objetivo `v1.0.0-rc.1` tras G6, cada uno condicionado a la evidencia que la ADR enumera. La propuesta histórica de publicar `v0.2.0` sin sufijo al cerrar G2 queda sustituida por un corte alpha; `ROADMAP_GLOBAL.md` se conserva intacto con su huella en `INVENTARIO.md`.

- Un número de versión no cierra un gate ni permite iniciar el siguiente; si falta evidencia de release, el corte sigue en alpha o beta.
- No autoriza tags ni releases: el propietario aprueba publicar cada candidato concreto y Claude ejecuta tag y release por instrucción registrada de Codex.
- No cambia capacidades, orden de gates ni criterios de aceptación.

## Decisión: navegador, contenedores y ubicación de los datos (2026-10-13)

**Aprobada por el propietario** ([conformidad en #117](https://github.com/eribertocz/bi-platform/issues/117#issuecomment-6159822059)). Está en [ADR 0015](docs/decisiones/0015-navegador-contenedores-y-ubicacion-de-datos.md): el navegador es la experiencia de autores y viewers, y los contenedores oficiales versionados la distribución principal, en equipos de la empresa o en la nube que ella elija; dónde corre el servidor, quién entra y dónde están los datos son tres elecciones independientes; publicar comparte una revisión dentro de la instancia, sin copiar datos ni ceder los permisos del autor.

- Es dirección y límites: las funciones se especifican y prueban en G3, G4 y G6, y ninguna está implementada todavía.
- No promete un servicio operado por nosotros, un gateway propio ni latencia cero.
- Conserva ADR 0003, 0004 y 0009; prevalece sobre las menciones de M08 a PostgreSQL como formato exclusivo de dashboards (tabla «Menciones obsoletas» de `docs/decisiones/README.md`).

## Decisión: documentación limpia antes del corte (2026-10-13)

**Auditoría de `docs/`** (propietario): estados desactualizados corregidos (ADR 0013, README), propuestas cerradas señaladas como tales, índice de referentes reordenado por módulo, y un listado único de menciones obsoletas de Polars en documentos congelados. Detalle: `docs/decisiones/README.md`, sección «Menciones obsoletas».
