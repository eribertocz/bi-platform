# 0014 — Versiones tentativas, beta desde G4 y candidato a producción tras G6

**Estado:** aprobada por el propietario el 2026-10-13 ([conformidad](https://github.com/eribertocz/bi-platform/issues/116#issuecomment-6159458610)); integrada mediante el PR documental de #116. No autoriza publicación ni implementación de otro gate.

**Sustituye:** la correspondencia de versiones de «Milestones de release propuestos» de `docs/arquitectura/gobierno/ROADMAP_GLOBAL.md`. La base histórica y su huella en `INVENTARIO.md` se conservan. No sustituye capacidades, criterios de aceptación ni secuencia de G0–G6.

## Contexto

El propietario pide un roadmap con versiones tentativas y planteó terminar G0–G6 todavía en beta. Tras consultar esa propuesta, exige que Codex analice y recomiende con criterio propio, en vez de trasladar automáticamente su idea al plan. Tras revisar la recomendación y su justificación, aprueba la correspondencia propuesta: beta del alcance BI inicial desde G4, validación durante G5/G6 y objetivo de candidato a producción al cerrar G6, condicionado a evidencia.

G0/G1 están cerrados; G2 sigue abierto. Solo se ha publicado el tag de proceso `corte-roles-2026-10-13`: no hay tags de versión de producto ni GitHub Releases. La propuesta anterior de publicar `v0.2.0` al cerrar G2 no es una decisión aprobada ni una release existente; esta ADR la sustituye por un corte alpha.

Un gate prueba una capacidad mediante sus criterios y evidencia. Una release fija y distribuye un commit. La declaración de preparación para producción requiere además un alcance de soporte y evidencia operativa. El cierre de G6 no es una autorización automática para publicar `1.0.0`.

## Decisión

### Versiones tentativas y momento de publicación

| Momento | Versión tentativa | Qué debe poder probar el propietario |
|---|---|---|
| G0/G1, ya cerrados | Sin publicación retroactiva prevista | El corte existente no se vuelve a etiquetar sin preparar y verificar expresamente una release. |
| Primer flujo protegido de G2 | `v0.2.0-alpha.1` | #113 y #115 integrados; entregas acotadas de #69 que protejan Modelos, bloqueen el bypass antiguo y permitan consultar el mismo KPI con dos usuarios que ven datos distintos; exactitud y negativos RLS/CLS/auditoría aprobados. Arranque y prueba reproducibles con datos sintéticos. |
| Cierre verificado de G2 | `v0.2.0-alpha.2` | Semántica, políticas y aislamiento cumplen el gate; vías anunciadas protegidas y vías pendientes bloqueadas con pruebas; riesgos y limitaciones registrados. El sufijo aumenta si hubo más alphas intermedias. |
| Cierre verificado de G3 | `v0.3.0-alpha.1` | Fuentes, transformación y publicación de datos con pruebas de idempotencia, retry y rollback del gate. |
| Cierre verificado de G4 y aptitud para piloto | `v0.4.0-beta.1` | G2/G3/G4 ofrecen el recorrido BI inicial: datos publicados, métricas y permisos, exploración, pivot y dashboards utilizables. Flujos de extremo a extremo aprobados, entorno de piloto reproducible y sin fallos críticos conocidos; se acuerdan usuarios, tareas, soporte y límites del alcance anunciado. |
| Cierre verificado de G5 | `v0.5.0-beta.1` | La beta incorpora trabajos, informes/alertas, diagnóstico e IA dentro de permisos, con negativos y recuperación del gate; se repiten las pruebas del recorrido BI inicial. No exponer funciones incompletas como parte de la beta anunciada. |
| Pilotos y validación durante G4–G6 | Siguientes betas del corte correspondiente | Uso, problemas y mediciones del conjunto generan Issues y correcciones verificadas mientras se construyen las capacidades de G5/G6, sin saltar la secuencia de gates. |
| Cierre verificado de G6, con aptitud para producción demostrada | **Objetivo: `v1.0.0-rc.1`** | Alcance estable y contratos/soporte acordados, resultados aceptados de pilotos anteriores y evidencia completa de preparación operativa. No hay bloqueadores conocidos; quedan validaciones finales del commit candidato y aprobación de publicación. |
| G6 cerrado pero con evidencia de release pendiente | `v0.6.0-beta.1` y siguientes | Si cumple el gate pero faltan pilotos o condiciones adicionales de preparación para la release, continúa la beta y se registran los pendientes. Si falta un criterio propio de G6, el gate tampoco se cierra. No se publica rc por calendario ni por número de gate. |
| Primera versión estable | `v1.0.0` | Candidato verificado, contratos/soporte definidos y evidencia operativa aceptada; publicación aprobada por el propietario. |

Estos números son una orientación, no tags reservados ni compromisos de publicar todos los cortes. El sufijo alpha/beta/rc describe la madurez del corte anunciado. No reduce exigencias de seguridad, exactitud, accesibilidad o calidad de código, ni implica comparabilidad demostrada con herramientas competidoras.

Se recoge feedback desde las alphas con un alcance controlado. G4 es el primer punto propuesto de beta porque ya integra el recorrido BI inicial sobre la gestión de datos de G3 y los permisos de G2; esperar hasta G6 para comenzar los pilotos retrasaría descubrir problemas de uso e integración. La beta es completa respecto a su alcance anunciado, no respecto a todas las capacidades futuras. G5 amplía ese alcance y G6 completa las capacidades operativas. La primera alpha de G2 usa datos sintéticos; cualquier ampliación a datos reales requiere su especificación, autorización y pruebas de permisos pertinentes.

La recomendación no presupone que ese recorrido funcionará al llegar al número G4: se demuestra con sus criterios y con la aptitud del piloto. Si no está listo para usuarios, el corte continúa en alpha. Del mismo modo, G6 contiene distribución, backup/restore, recuperación, benchmarks, seguridad y documentación operacional; por eso se propone llegar allí a un candidato a producción tras pilotos desde G4, no empezar automáticamente la beta ni publicar 1.0 estable.

### Pronóstico de fechas

La tabla fija condiciones observables, no fechas inventadas. El seguimiento del gate activo incluirá versión objetivo, dependencias pendientes, evidencia de avance, rango de fecha cuando sea estimable, supuestos y próxima revisión. Las fechas se estiman tras desglosar el trabajo, conocer dependencias y contar con entregas verificadas; los gates posteriores pueden figurar «por estimar» con su motivo.

Al cerrar cada entrega que afecte al corte se revisa el pronóstico. Codex informa aquí en el repositorio y al propietario en lenguaje llano sobre cambios de alcance o de previsión. No se cambia el criterio para cumplir una fecha. Al integrar esta ADR no hay fecha de calendario fiable para alpha.1: #113 está integrado, #115 está en verificación y faltan las especificaciones de ejecución/UI de #69. El seguimiento del cierre de G2 está en #128.

### Preparación para producción, desde la beta y al cerrar G6

Antes de iniciar la beta en G4 se especifican y consultan casos, cargas, entornos soportados, duración/volumen pertinentes y resultados de aceptación. La evidencia se acumula y revalida durante G5/G6; se amplía cuando cambia el alcance. No se inventan hoy SLA, umbrales ni una duración fija de piloto.

Para proponer rc/1.0 se exige evidencia aceptada de:

- uso de extremo a extremo y pilotos controlados con tareas y resultados representativos;
- exactitud de datos y ausencia de cruces de permisos en todas las vías anunciadas;
- revisión de seguridad acorde al riesgo, sin fallos críticos o altos abiertos en el alcance que se publica;
- latencia, concurrencia, memoria y estabilidad bajo cargas acordadas, con presupuestos y resultados reproducibles;
- instalación desde cero, actualización desde los cortes beta soportados y recuperación/rollback probados;
- backup/restauración y objetivos de recuperación acordados, observabilidad y diagnóstico que permitan operar el producto;
- accesibilidad y flujos principales aceptados por sus usuarios;
- compatibilidad de API, proyectos, configuración y datos persistidos, límites y política de actualización/soporte documentados.

Estas verificaciones complementan las exigidas en G6 con evidencia del uso y de los entornos acordados; no autorizan dejar pruebas operativas de G6 para después ni garantizan soporte universal. Si falta una condición de la release, se mantiene beta; si falta un criterio de G6, el gate sigue abierto. Las correcciones de fallos descubiertos siguen el flujo Issue → PR → verificación y su prioridad de riesgo. «Beta» no justifica publicar un flujo anunciado con un fallo crítico conocido de seguridad o exactitud.

### Roles y trazabilidad de publicaciones

Codex planifica, prepara el alcance/número de versión y verifica el corte. Antes de cada especificación o modificación sustancial, presenta al propietario un resumen entendible con su recomendación, motivos, límites y puntos donde aportar. Los acuerdos quedan en el cuerpo del Issue y sus comentarios. El propietario aprueba publicar el candidato concreto; Claude ejecuta tag, push y release por instrucción registrada de Codex.

Cada publicación parte de un commit exacto integrado en main, con CI de ese commit aprobado, evidencia pertinente, pasos reproducibles, notas de cambios, limitaciones y compatibilidad/actualización. El tag de producto se anota, se firma si existe una clave del agente configurada y se mantiene inmutable; correcciones generan otra versión. No se sustituyen silenciosamente artefactos de una versión publicada ni se usa una etiqueta mutable como única referencia reproducible.

La versión de producto no sustituye las versiones independientes de API/esquemas/migraciones ni exige igualar automáticamente todos los crates internos. La preparación de cada release especifica qué artefactos distribuye y qué contratos cubre. En 0.x la compatibilidad sigue protegida por las ADR vigentes; en 1.x los incrementos mayor/menor/parche comunican compatibilidad según SemVer.

## Alternativas consideradas

- **Publicar versiones normales 0.x al cerrar cada gate**, como en la propuesta histórica. Válido en SemVer, pero menos claro para comunicar la intención del propietario de mantener el producto en pruebas durante todo el roadmap. Se prefieren sufijos de prelanzamiento.
- **Etiquetar 1.0 al cerrar G6.** Descartado como automatismo: capacidades completas no sustituyen la evidencia de uso, soporte y contratos acordados.
- **No publicar hasta terminar el roadmap.** Descartado: dificulta pruebas, feedback y reproducir los cortes intermedios.
- **Comenzar la beta al cerrar G6.** Alternativa conservadora y válida si antes no hay un recorrido apto para usuarios. No se recomienda como objetivo: G4 ya debería ofrecer BI usable y perderíamos feedback importante durante G5/G6. Se mantiene como resultado posible si la evidencia no permite avanzar.
- **Publicar rc al cerrar G6 sin pilotos previos.** Descartado: cumplir pruebas locales/CI no demuestra uso sostenido ni todos los riesgos de instalación/operación. El objetivo rc requiere pilotos desde G4 y evidencia del alcance final; si no se obtienen, continúa beta.

## Referencias

- [SemVer 2.0.0](https://semver.org/lang/es/): versiones 0.x, prelanzamientos, estabilidad de contratos y contenido inmutable. SemVer no certifica por sí mismo preparación para producción.
- [Google SRE, Reliable Product Launches](https://sre.google/sre-book/reliable-product-launches/): preparación operativa del lanzamiento y validación de riesgos. Se toma la evaluación por evidencia; no se impone su infraestructura ni escala.
- [Mozilla, Pocket Guide: Shipping Firefox](https://firefox-source-docs.mozilla.org/contributing/pocket-guide-shipping-firefox.html): estabilización en beta, decisiones QA y candidato antes de release. Se toma la maduración y validación por corte; no se copian su cadencia, estructura de ramas ni controles propios de un navegador.
- ADR 0008/0010/0011 y las leyes del producto: contratos, permisos, exactitud, accesibilidad y distribución.

## Consecuencias

La planificación vigente de versiones se consulta desde esta ADR y los Issues de release; se conserva el roadmap histórico. El objetivo es beta desde G4 y rc tras G6 con aptitud para producción demostrada. La etiqueta real depende de evidencia: G4 puede continuar en alpha y G6 en beta. La primera versión estable exige validación del candidato y aprobación del propietario. Ningún número de versión permite cerrar gates ni iniciar el siguiente sin sus criterios. `DECISIONES.md` y el índice de ADR enlazan esta ADR sin copiar su tabla; el orden de entregas de G2 se consulta en #128.
