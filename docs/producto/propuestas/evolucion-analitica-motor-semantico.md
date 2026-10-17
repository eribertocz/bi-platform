# Evolución analítica del motor semántico

**Tipo:** propuesta de producto y diseño. **Estado:** propuesta de dirección; el propietario aprobó integrarla al repositorio el 2026-10-17 ([decisión](https://github.com/eribertocz/bi-platform/issues/131#issuecomment-6230828958)). **Autor:** Codex. **Revisión:** 3, 2026-10-17.

**Presentación y revisión:** [Issue #131](https://github.com/eribertocz/bi-platform/issues/131). **Seguimiento de G2:** [Issue #128](https://github.com/eribertocz/bi-platform/issues/128). El propietario aprobó integrar este documento como propuesta de dirección y aceptó como dirección las recomendaciones de §6.1. Su incorporación conserva la condición de propuesta: no equivale a aprobar todas las capacidades ni a cambiar gates, contratos o criterios de cierre.

**Contenido:** [resumen](#1-resumen-y-alcance) · [estado y evidencia](#2-punto-de-partida-y-límites-de-evidencia) · [capacidades](#3-capacidades-propuestas-y-demostraciones) · [arquitectura](#4-integración-en-sef-y-alternativas) · [casos y calidad](#5-casos-de-referencia-y-condiciones-de-calidad) · [prioridades](#6-prioridad-dependencias-y-decisiones-pendientes) · [presentación](#7-presentación-e-incorporación-al-repositorio) · [revisión](#8-condiciones-de-revisión-de-esta-propuesta) · [comparativa y fuentes](#9-comparativa-detallada-con-referentes-y-fuentes).

## 1. Resumen y alcance

Proponemos ampliar la expresividad y la experiencia de nuestra plataforma BI general sobre SEF. Las definiciones deben servir a diferentes negocios, reutilizarse en los distintos consumidores y mantener el mismo significado, exactitud y permisos. Los ejemplos de regiones, entidades o ventas son datos de demostración; el producto no incorpora reglas particulares de esos negocios.

La selección reúne las ideas iniciales y la comparación posterior en seis familias. Los nombres de las capacidades principales se mantienen explícitos para que sean fáciles de encontrar.

| Familia | Capacidades incluidas | Resultado para quien usa la plataforma |
|---|---|---|
| **S1 — Cálculos componibles** | **Contexto de filtros**, **nivel de detalle (LOD)**, **grupos de cálculo** y composición multietapa | Definir porcentajes, agregaciones anidadas y elementos reutilizables sin duplicar medidas por cada variante |
| **S2 — Comparar selecciones** | Contextos analíticos independientes, filtros comunes y valores posibles | Contrastar dos selecciones y explorar alternativas dentro de los datos autorizados |
| **S3 — Inteligencia temporal** | Períodos comparables, acumulados, ventanas y calendarios de negocio | Comparar intervalos equivalentes con reglas explícitas de calendario, zona y cobertura |
| **S4 — Autoría gobernada** | Creación visual, validación, vista previa, impacto y publicación de métricas | Crear medidas reutilizables y entender qué cambia antes de publicarlas |
| **S5 — Aceleración equivalente** | Preagregaciones, selección de rutas y coordinación de frescura | Mejorar rendimiento sin alterar pregunta, exactitud ni permisos |
| **S6 — Explicar y contrastar** | Recibo comprensible, comparación de resultados y futura interpretación mediante IA | Entender una cifra y verificar cómo se obtuvo o qué cambió |

**Recomendación:** completar primero el recorrido protegido vigente de G2. La primera ampliación analítica propuesta es contexto de filtros y LOD; después, los operadores temporales y grupos de cálculo que los reutilizan. La autoría, exploración avanzada, aceleración e IA se incorporan conforme a sus dependencias y gates acordados.

Este documento define dirección, alternativas y condiciones de evaluación. La comparación detallada con Power BI, Tableau, Looker, Qlik, Sigma, ThoughtSpot y Cube se conserva al final, en la sección 9. Las especificaciones ejecutables se prepararán por entregas acotadas cuando su alcance se acuerde.

## 2. Punto de partida y límites de evidencia

| Área | Base identificada | Pendiente que debe conservarse visible |
|---|---|---|
| Definiciones y gobierno | Modelo semántico tipado, Ossie, métricas reutilizables, versiones y diff semántico | Experiencia completa de creación/publicación, catálogo permitido y consumo |
| Corrección | Grano, relaciones, protección contra fanout, alineación de hechos, ratios y reglas semiaditivas en un dominio acotado | Aplicar y demostrar todas las obligaciones en la consulta ejecutada; ampliar el corpus al añadir operaciones |
| Filtros y seguridad | Filtros tipados antes/después de agregar, filtros intrínsecos y políticas obligatorias separados; autorización y CLS | Recorrido HTTP/UI completo y contexto analítico general por subexpresión |
| Ejecución y procedencia | Plan certificado, adaptador DuckDB y procedencia del resultado en componentes internos | Contrato HTTP de ejecución y recibo real comprensible en pantalla |
| Tiempo y rendimiento | Tipos temporales, algunas ventanas y límites del runtime | Comparaciones temporales generales, calendarios fiscales ejecutables y aceleración operativa completa |

**Base** indica contratos/código y evidencias anteriores de un dominio concreto. **Parcial** indica piezas que aún requieren composición o ampliación. **Propuesta** indica una capacidad general pendiente de diseño/aceptación en el alcance revisado. Ninguna etiqueta sustituye la verificación de una entrega.

La investigación comparó tipos y archivos concretos con main `99f711c1c648ca2ed454bc7ff43cffb7492863c1` y consultó fuentes primarias el 2026-10-17. No ejecutó pruebas nuevas de producto, instalaciones comerciales ni benchmarks. Las pruebas históricas conservan su commit y alcance; esta propuesta no acredita paridad ni superioridad frente a productos maduros. El detalle reproducible de lo inspeccionado está en §9.5.

## 3. Capacidades propuestas y demostraciones

Son familias de capacidad para desglosar en entregas, no seis Issues gigantes. Su efecto potente viene de compartir una semántica correcta y una experiencia sencilla.

### 3.1 S1 — Contexto de filtros, nivel de detalle y grupos de cálculo

**Valor:** permitir preguntas que hoy requieren duplicar modelos, escribir SQL específico o crear muchas variantes de medidas. Ejemplos: participación del total seleccionado, participación del universo autorizado, promedio de totales por entidad, ranking y transformación temporal reutilizada sobre varias métricas.

**Qué rescatar:** expresividad de Power BI, LOD de Tableau y etapas de Cube. **Qué integrar:** operadores de contexto y grano con identidad propia, dependencias explícitas, diagnóstico y composición en SEF. Los grupos de cálculo son una capacidad identificable sobre esa base; no un alias de cualquier fórmula derivada.

#### Contexto de filtros: qué población usa cada cálculo

La capacidad permite evaluar una subexpresión bajo filtros analíticos expresamente conservados, añadidos o retirados. La definición distingue el universo autorizado, la selección exterior y el contexto de cada cálculo. Las políticas obligatorias permanecen aplicadas; un filtro intrínseco forma parte de la identidad de la métrica y no desaparece por una selección de pantalla.

Ejemplo: al seleccionar una región, se puede calcular su participación respecto del total seleccionado o del total autorizado. Son denominadores diferentes y deben poder elegirse y explicarse. Cambiar el denominador no debe añadir a la tabla filas que la consulta exterior no pidió.

Antes de fijar el contrato se resolverán el origen de cada filtro, intersección/reemplazo sobre un mismo campo, expresiones OR con varios campos y composición entre etapas. No se asume que una operación denominada «quitar filtro» sea suficiente para todos esos casos.

#### Nivel de detalle (LOD): a qué grano se calcula

La capacidad permite fijar, ampliar o reducir las dimensiones de agrupación de una subexpresión. Por ejemplo, calcular primero el total por entidad y después el promedio de esos totales, aunque el gráfico muestre otra agrupación.

Retirar una dimensión de agrupación y retirar su filtro son decisiones distintas. El motor verifica la recomposición y rechaza bajar a un detalle que la medida no puede producir. Los totales y subtotales también se calculan en el servidor conforme a la clase de métrica.

#### Grupos de cálculo: elementos reutilizables sobre medidas

Un grupo contiene elementos de cálculo reutilizables sobre métricas gobernadas. No requiere crear una medida distinta para cada combinación ni adoptar el lenguaje DAX.

Ejemplo: un grupo «Comparación temporal» ofrece actual, año anterior, diferencia y variación porcentual; puede aplicarse a varias métricas cuyas propiedades lo permitan. No se presupone que todas admitan todos los elementos: un saldo no se acumula necesariamente como un flujo y un conteo distinto no se obtiene sumando conteos parciales.

El diseño deberá resolver:

- identidad estable, versiones, elementos y medidas compatibles;
- dependencias y rechazo de ciclos;
- composición entre grupos y selección vacía, única o múltiple;
- interacción con filtros, grano, calendario, nulos y denominadores cero;
- tipo y unidad resultantes, con formato separado del valor exacto;
- definición canónica compartida por editor visual, archivos/API y futura IA;
- autorización de todas las dependencias, huellas y explicación del elemento aplicado.

Primero se implementan y verifican los operadores que necesita cada elemento. Tener grupos no demuestra que ya existan año anterior o calendarios fiscales.

**Decisiones difíciles:** conservar/intersectar/reemplazar filtros del mismo campo; distinguir selección exterior, filtro de autor, contexto de cada etapa, filtro intrínseco y política; uniones OR que mencionan varios campos; compatibilidad de unidades; población de agregación frente a filas visibles; límites de profundidad/coste y ciclos.

**Demostración exigida:** la misma definición de «porcentaje del total seleccionado» funciona en tabla, pivot y API; cambiar la agrupación no cambia su denominador por accidente. Un grupo puede producir actual y variación sobre dos métricas compatibles sin crear sus cuatro medidas manualmente. Los permisos limitan cada dependencia, incluso las que no aparecen en el resultado.

**Pruebas decisivas:** ejemplo 100/400=25 % y 100/100=100 % según autorización; promedio 40 frente a 60 según grano; dimensión NULL; campo retirado del filtro que no figura en la agrupación; medida enmascarada usada solo como denominador; precedencias/ciclos inválidos; equivalencia entre las representaciones visual y de archivo. Porcentajes con expansión decimal infinita deben declarar escala/redondeo o rechazo exacto: no basta comparar floats.

**Esfuerzo/riesgo:** alto; afecta IR, huellas, corrección, planificación, adaptador y autoría. **Prioridad:** primera ampliación de expresividad propuesta después de la base G2; contexto y grano antes de exponer grupos que dependan de ellos. No implementar un conjunto de funciones sin su álgebra.

### 3.2 S2 — Comparación de selecciones y exploración de valores posibles

**Valor:** mantener dos contextos analíticos en la misma página y cambiar uno sin perder el otro. Ver valores disponibles y alternativas ayuda a descubrir información, en vez de limitarse a recibir gráficos vacíos.

**Qué rescatar:** estados alternativos y selección asociativa de Qlik, con contexto de Power BI como segundo referente. **Qué integrar:** contextos nombrados, filtros tipados, operaciones de comparación y facetas autorizadas servidas por SEF. Empezar con dos contextos y facetas concretas; no prometer un clon del motor asociativo completo.

**Demostración exigida:** una selección A agrupa entidades por condición X y otra B por condición Y; se comparan las mismas métricas, con filtros comunes expresamente declarados. Entidades que están en ambas selecciones se cuentan una vez en su unión, no sumando conteos. El conjunto «resto» se calcula respecto del universo autorizado definido, no de todas las filas físicas.

**Pruebas decisivas:** solapamiento de conjuntos, combinación de OR/AND, deshacer/rehacer, límites de alta cardinalidad, revocación al siguiente acceso y paginación de facetas. Un valor oculto por seguridad no aparece como «excluido», ni mediante su etiqueta, conteo o alternativa. No se supone una protección estadística general por ocultar el nombre.

**Esfuerzo/riesgo:** alto; seguridad de metadatos, consultas adicionales y estado reproducible. **Prioridad:** después de estabilizar contextos y catálogo permitido; experiencia candidata al trabajo de exploración avanzada. Es una comparación analítica dentro de una sesión, no la demo de aislamiento entre dos identidades que exige G2.

### 3.3 S3 — Inteligencia temporal y calendarios de negocio

**Valor:** comparar intervalos equivalentes, no simplemente restar 365 días. Reutilizar calendario, rol temporal y reglas de corte en todas las métricas compatibles.

**Qué rescatar:** inteligencia temporal de Power BI y series/etapas de Cube. La documentación de Power BI consultada sigue marcando su inteligencia basada en calendarios como **preview**; aquí se toma la idea, no se presenta como una garantía universal de ese producto.

**Qué integrar:** calendario explícito y versionado; intervalo y zona definidos; comparación por rol temporal; reglas para períodos completos/incompletos y cobertura real de datos. Calendarios fiscales y patrones como 4-4-5 son ampliaciones con casos normativos propios, no opciones decorativas. Separar fecha del evento, estado a una fecha y versión de datos que se está leyendo.

**Demostración exigida:** un acumulado hasta el tercer día de un período se compara con el corte equivalente declarado del anterior, no con el período anterior completo. Una métrica de saldo selecciona el cierre correcto y no suma saldos diarios. Cambiar zona o calendario queda visible en el recibo.

**Pruebas decisivas:** febrero bisiesto, frontera de año ISO, semana 53, transición de horario de verano, ausencia de hechos, datos incompletos y falta de observaciones. Cero solo se introduce cuando el contrato permite distinguirlo de «desconocido». Una consulta de año anterior sin rol temporal/calendario suficiente pide precisión o se rechaza.

**Esfuerzo/riesgo:** alto; semántica y exactitud. **Prioridad:** segunda ampliación propuesta, reutilizada por S1. No exige terminar todos los calendarios antes del primer grupo de cálculo útil.

### 3.4 S4 — Autoría, validación y publicación de métricas

**Valor:** que un autor pueda construir una medida avanzada desde la UI y reutilizarla, sin depender de un desarrollador para cada cambio ni crear una verdad distinta por dashboard.

**Qué rescatar:** autoría y métricas reutilizables de Sigma, navegación/definiciones de Looker y grupos de Power BI. **Qué integrar:** definición canónica común, borrador → validar → vista previa → publicar, propietario, versiones, uso y dependencias. Se apoya en la dirección de Studio/Atlas BI ya aceptada; la ampliación es concretar el recorrido analítico y su impacto medible.

**Demostración exigida:** crear visualmente una medida del conjunto soportado, abrir la misma definición como archivo y volver al editor sin pérdida. Antes de publicar, ver dependencias afectadas y cifras antes/después para consultas seleccionadas, sobre las mismas versiones de datos y permisos. Una medida publicada conserva su identidad en consumidores que la referencian conforme a su contrato de compatibilidad.

**Pruebas decisivas:** igualdad de definición/huella desde diferentes autores, validación y autorización en servidor, publicación concurrente sin sobrescritura silenciosa, rollback compatible, objetos inaccesibles no expuestos en el impacto y negativa de ciclos/unidades inválidas. El informe de impacto declara las consultas que evaluó; una muestra no garantiza que ningún otro resultado cambie.

**Esfuerzo/riesgo:** alto; ciclo de vida, concurrencia y accesibilidad. **Prioridad:** experiencia de autoría de su gate, después de estabilizar las operaciones que expone. No bloquea publicar medidas mediante el mecanismo vigente mientras tanto.

### 3.5 S5 — Preagregaciones y aceleración equivalente

**Valor:** interacción rápida sobre volúmenes grandes, sin cambiar la pregunta ni obligar al autor a escoger manualmente una tabla resumida.

**Qué rescatar:** aggregate awareness de Looker, preagregaciones de Cube y selección de modos de lectura de Power BI. **Qué integrar:** seleccionar la fuente de ejecución compatible, con política de frescura y explicación de la decisión. La dirección «consultar/acelerar/conservar» ya está aceptada; esto no es una nueva promesa inventada por la comparación.

La capacidad Direct Lake de Power BI pertenece a su arquitectura Fabric/OneLake. No se equipara con leer Parquet mediante DuckDB ni se propone reproducir VertiPaq. Para nosotros, lectura local/remota y materialización siguen detrás de los adaptadores y del trabajo de gestión de datos acordado.

**Demostración exigida:** la misma consulta y contexto pasan de la ruta base a una preagregación y mantienen resultado exacto, permisos y frescura declarada; el recibo muestra la ruta usada. Un agregado incompatible causa fallback seguro o rechazo definido, nunca degradación silenciosa de exactitud. Si se combina agregado histórico con datos recientes, las particiones/versiones deben demostrar ausencia de huecos, doble conteo y snapshots incoherentes.

**Pruebas decisivas:** distintos no aditivos, ratios recalculados, políticas modificadas, usuarios diferentes, actualización durante lectura, fallo de refresco y caducidad. Comparar contra oráculo y ruta base sobre versiones coherentes. Medir p50/p95, memoria, concurrencia y coste de construcción/refresco en un equipo y dataset publicados; no prometer «subsegundo» sin esa evidencia.

**Esfuerzo/riesgo:** muy alto; almacenamiento, publicación, invalidación y coste. **Prioridad:** conforme a los gates de datos y aceleración y a mediciones del recorrido. Evitar confundir caché de compilación, caché de resultados y materializaciones: tienen claves, seguridad y costes distintos.

### 3.6 S6 — Explicación, comparación de resultados y futura IA

**Valor:** pasar de «el gráfico dice 120» a conocer definición, filtros, cambios y límites de reproducción; posteriormente permitir preguntas en lenguaje natural con la misma trazabilidad.

**Qué rescatar:** exploración de explicaciones de Tableau, vocabulario/ejemplos de ThoughtSpot y respuestas verificadas de Power BI. Nuestra ley del recibo y el certificado dan una base propia; no son por sí solos una explicación de causas de negocio.

**Qué integrar en etapas:** primero recibo legible y navegación autorizada del número, como parte del trabajo ya exigido; después comparación entre resultados/definiciones y descomposición aritmética con regla declarada; finalmente interpretación conversacional contra catálogo y consultas canónicas verificables. Un ejemplo aprobado enseña intención, no permite servir el resultado de otra identidad ni eludir autorización.

**Demostración exigida:** para una suma que pasa de 100 a 120, los cambios por particiones disjuntas reconcilian exactamente con +20. Para un ratio, no se inventa una contribución sumando porcentajes: se declara la descomposición elegida o se limita la función. Una pregunta ambigua solicita aclaración y el usuario puede inspeccionar la consulta semántica que la IA propone.

**Pruebas decisivas:** diferencias causadas por filtro/modelo/datos/permisos se distinguen; versión expirada se marca solo explicable; una explicación de negocio no se presenta como causalidad demostrada. Evaluación conversacional con ejemplos autorizados positivos, ambiguos y denegados; consultas generadas atraviesan el mismo punto único y los límites de recursos. Cualquier uso de proveedores externos debe respetar la política de envío de datos que se especifique, sin habilitarlo implícitamente.

**Esfuerzo/riesgo:** medio/alto para hacer utilizable el recibo existente; alto para comparaciones y evaluación de IA. **Prioridad:** recibo básico en G2; experiencia ampliada y asistente en sus gates posteriores. No sustituir pruebas por una respuesta convincente del modelo.

## 4. Integración en SEF y alternativas

### 4.1 Responsabilidades

La recomendación es evolucionar la composición existente de SEF. Cada capacidad conserva una definición semántica propia y un único recorrido autorizado.

| Capa | Responsabilidad en la evolución |
|---|---|
| Modelo semántico | Definir métricas, operaciones reutilizables, dependencias, compatibilidad, unidades y calendarios |
| Consulta | Expresar selecciones, filtros, contextos y resultados solicitados sin depender de tablas físicas |
| Autorización | Resolver permisos y restricciones a partir de la identidad verificada y las políticas vigentes, mediante el autorizador existente |
| Corrección y planificación | Validar significado y grano; demostrar la aplicación de restricciones obligatorias, rechazar ambigüedades y emitir obligaciones verificables |
| Adaptador y runtime | Traducir y ejecutar exactamente lo soportado; aplicar límites, cancelación y comprobaciones del origen |
| API y aplicaciones | Exponer contratos estables y editar/presentar las mismas definiciones, con autorización en servidor |

La composición multietapa sostiene los operadores de contexto, grano y tiempo; no constituye un motor paralelo. La ubicación exacta de tipos, campos y versiones se decidirá mediante el diseño técnico de cada entrega.

Ossie sigue siendo la frontera de intercambio. Cuando su perfil no represente una capacidad, se deberá diseñar una extensión versionada o un rechazo explícito; no anunciar conformidad por conservar solo parte del significado. Los cambios de semántica o huellas requieren la versión correspondiente.

### 4.2 Invariantes

- Las políticas obligatorias se conservan en todas las etapas, incluidas dependencias invisibles, facetas, drill, caché y preagregaciones.
- La lógica de negocio y los totales se resuelven en el motor; el navegador presenta valores y definiciones.
- La exactitud y los límites de cada adaptador son explícitos: soporte exacto, emulación exacta probada o rechazo previo a ejecutar.
- Las definiciones visuales, de archivo/API y de futura IA comparten validación y ciclo de vida.
- El certificado acredita sus obligaciones concretas; la verdad del resultado también depende de datos, adaptador y ejecución.
- Una fuente en vivo o una versión expirada tiene límites de reproducción declarados en el recibo.

### 4.3 Alternativas consideradas

Adoptar varios lenguajes completos como DAX, LookML o expresiones Qlik añadiría semánticas y herramientas de autoría paralelas. La propuesta toma sus capacidades y términos reconocibles, con contratos propios coherentes, sin prometer compatibilidad de lenguaje.

Resolver cada necesidad con fórmulas particulares del gráfico multiplicaría definiciones y complicaría consistencia y seguridad. La operación reutilizable pertenece al modelo; una selección local debe tener alcance explícito y gobernado.

Sustituir SEF por Cube u otro motor contradice la dirección acordada. Se inspecciona código abierto como referencia; la implementación es propia. Las demás alternativas descartadas para esta tanda se comparan en §9.4.

Mandatos: [leyes del producto](../leyes-bi.md), [ADR 0006](../../decisiones/0006-motor-semantico-sef.md), [ADR 0008](../../decisiones/0008-modulos-desacoplados-por-contratos.md), [ADR 0010](../../decisiones/0010-seguridad-del-blueprint-en-g2.md) y [ADR 0012](../../decisiones/0012-motor-de-permisos-cedar.md). [Atlas BI](../../referentes/atlasbi.md), en particular A1–A4, sigue marcando la profundidad mínima. Esta propuesta no modifica su orden de prioridad ni los aplazamientos normativos vigentes.

## 5. Casos de referencia y condiciones de calidad

La selección se mantiene solo si produce estas demostraciones, con pruebas automáticas en sus capas y una experiencia que el propietario pueda probar:

- **Expresividad:** preguntas diferentes se resuelven combinando operaciones generales, sin introducir nombres o reglas del negocio en el código de producto.
- **Consistencia:** mismo modelo, datos, permisos y consulta producen el mismo valor exacto en todos los consumidores. Un segundo motor se anuncia solo tras conformidad real.
- **Seguridad:** ninguna subexpresión, faceta, drill, agregado, recibo o herramienta IA amplía acceso. Una revocación se aplica conforme al contrato acordado.
- **Explicación:** el usuario distingue fórmula, selección, universo autorizado, grano, período y origen; los detalles de políticas que revelen información protegida no se muestran a quien carece de permiso.
- **Autoría:** el recorrido principal es accesible con teclado y lector de pantalla, tiene validación en servidor y conserva definiciones al cambiar de editor.
- **Rendimiento:** presupuestos y escenarios publicados, medición antes/después en cambios críticos, límites de complejidad y cancelación. Un ejemplo rápido no acredita el comportamiento bajo carga.

### 5.1 Ejemplos mínimos con resultados esperados

| Caso | Resultado o rechazo que se debe demostrar |
|---|---|
| Porcentaje del total autorizado | A puede ver X=100 e Y=300. Selecciona X; retira solo ese filtro analítico del denominador: 25 %. B solo puede ver X: 100 %. B nunca utiliza los 300 de Y |
| Población y filas visibles | Quitar un filtro al denominador puede ampliar lo que este agrega; no añade por sí solo filas al resultado exterior ni retira políticas |
| Promedio por grano | Entidad A con filas 10 y 10; B con una fila 100. Promedio de filas=40; promedio de totales por entidad=60. La definición determina cuál se calcula |
| Comparación de conjuntos | A={1,2} y B={2,3}: conteo distinto de la unión=3. Sumar los conteos daría 4 y no satisface esa definición |
| Variación temporal | Períodos completos de 100 y 120: diferencia=20 y variación=20 %. Cero, nulos y período incompleto tienen resultado o rechazo explícito en su especificación |
| Grupos de cálculo | Un elemento se aplica coherentemente a métricas compatibles; ciclos, precedencias ambiguas y tipos/unidades incompatibles se rechazan |
| Drill autorizado | Conserva selección y restricciones; un total autorizado no concede campos ni registros de detalle por sí solo |
| Aceleración | Ruta base y preagregación coinciden contra un resultado esperado independiente; un agregado incompatible no se utiliza |
| Explicación de cambios | Para una suma 100→120, contribuciones por particiones disjuntas reconcilian +20; para ratios se requiere una regla de descomposición válida |

Los ejemplos orientan especificaciones futuras. No son nuevas pruebas ejecutadas ni autorizan cambiar la verdad esperada del corpus existente.

### 5.2 Evidencia y conformidad

La matriz de cobertura distinguirá diseñado, implementado y demostrado: contrato, código, prueba/comando, resultado observado, commit/entorno y limitaciones. Una prueba inspeccionada no cuenta como ejecutada. Cada capacidad exacta anunciada tendrá cobertura de su dominio; la emulación identificará su estrategia y cumplirá la misma conformidad.

El segundo adaptador se comparará con el mismo oráculo independiente: que dos motores coincidan no basta si comparten un error. Mantener la frontera de dependencias tampoco demuestra portabilidad real sin ese adaptador.

El modelo de amenazas tratará inferencia entre agregados según datos, adversario y operaciones permitidas. Un mínimo de grupo no se anunciará como garantía estadística general. Los recibos, facetas, informes de impacto y explicaciones forman parte de la superficie de autorización.

## 6. Prioridad, dependencias y decisiones pendientes

| Bloque | Valor esperado | Dependencia real | Condición para avanzar |
|---|---|---|---|
| **G2 vigente** | Base segura utilizable | Entregas aceptadas y composición API/UI | Pruebas del gate y aceptación del recorrido; no aumentar ocultamente su alcance |
| **S1: contexto y LOD** | Mayor aumento inmediato de expresividad general | Núcleo actual y nuevos casos normativos | Resolver composición y negativos; limitar primera entrega y versionar contratos |
| **S3 + grupos de S1** | Tiempo comparable y reutilización de cálculos | Contextos, calendario y operadores compatibles | Demostración de un subconjunto útil; no exigir toda la inteligencia temporal a la vez |
| **S2: comparar selecciones** | Exploración más flexible y descubrimiento | Catálogo permitido, contextos y facetas | Cero exposición de valores no autorizados y coste medido de facetas |
| **S4: autoría y publicación** | Autoservicio real y reutilización | Operadores estables y ciclo de publicación de su gate | Definición única, control de concurrencia y recorrido accesible; UI en G4 según planificación |
| **S5: aceleración** | Latencia/coste predecibles a escala | Gestión/publicación de datos de G3 y equivalencia | Mediciones justifican cada materialización; servicio de aceleración en su gate |
| **S6: explicación ampliada e IA** | Confianza y acceso sencillo al análisis | Recibo, versiones, catálogo y herramientas autorizadas | Evidencia real, comparación honesta y evaluación; IA/MCP en G5 según dirección vigente |

Las filas posteriores a G2 son planificación propuesta, no permiso para comenzar otro gate. La prioridad entre S2, S4 y aceleración se recalibra con casos de uso y mediciones. S6 no pospone el recibo obligatorio de G2 ni exige desarrollar un asistente antes de hacer útil la plataforma.

Para cada bloque se preparará una tanda con problema, alternativas, contratos afectados, 2–4 referentes, casos exactos/negativos, observabilidad, coste y desglose en PR. No se convierte esta tabla directamente en siete Issues `spec-lista` sin ese diseño. La incorporación al producto requiere aceptar alcance y gate; la posterior implementación sigue a cargo de Claude, un Issue por vez.

### 6.1 Dirección aceptada y decisiones pendientes

El propietario aceptó el 2026-10-17 como dirección las recomendaciones de esta tabla ([decisión](https://github.com/eribertocz/bi-platform/issues/131#issuecomment-6230828958)). Cerrar G2 tiene prioridad absoluta: nada de esta propuesta entra en el alcance de G2 ni retrasa el tramo 2. Aceptar la dirección no concede implementar ninguna capacidad. Cada bloque se presentará en su momento en tandas, con alternativas, referentes, casos exactos y negativos, dividido en entregas pequeñas revisables de una sentada ([#128](https://github.com/eribertocz/bi-platform/issues/128)). Lo que indica la última columna sigue pendiente hasta su especificación.

| Decisión | Dirección aceptada (recomendación de Codex) | Qué requiere acordar |
|---|---|---|
| Primera ampliación | Después de cerrar G2: contexto de filtros y LOD sobre composición multietapa | Alcance inicial, gate y casos exactos que se anunciarán |
| Reutilización y tiempo | Después: grupos de cálculo con operadores compatibles y un subconjunto temporal verificable | Primeros elementos, calendarios y reglas de períodos incompletos |
| Exploración y autoría | Comparación de selecciones y autoría gobernada (ciclo de publicación) conforme madure la base | Prioridad relativa y alcance de cada experiencia |
| Rendimiento | Acelerar solo cuando mediciones y presupuesto lo justifiquen | Dataset, equipo, cargas, frescura y criterio de aceptación |
| Explicación y futura IA | Recibo básico en G2; comparación ampliada, interpretación e IA en sus gates (G5) | Garantías, evaluación y políticas de uso de datos antes de habilitar proveedores externos |

Cohortes, simulaciones y jerarquías avanzadas quedan como candidatos para evaluar la expresividad general. No se presupone un módulo de negocio por cada caso. El drill básico autorizado mantiene su obligación actual.

## 7. Presentación e incorporación al repositorio

Este documento es la **fuente de la propuesta de producto**. El flujo recomendado conserva un documento único y entregas pequeñas:

| Artefacto | Qué se presenta y quién lo mantiene |
|---|---|
| Este documento | Capacidades, alternativas, prioridades propuestas y comparación. Codex prepara su contenido para revisión del propietario |
| Issue documental acotado | Alcance y criterios para incorporar esta propuesta al repositorio, junto con un enlace de descubrimiento. Su integración no aprueba la implementación de las capacidades |
| PR documental de Claude | Incorpora el documento y el enlace acordado en una rama propia. Codex verifica alcance, contenido, enlaces y CI antes de fusionar |
| Issue #128 | Resumen de la relación con G2 y enlaces al Issue/PR/documento. Evita mantener otra versión editable del contenido completo |
| ADR posteriores | Decisiones arquitectónicas aceptadas y cambios normativos concretos; no convertir toda la comparación en una ADR |
| Issues de producto posteriores | Una entrega por Issue, con contratos, criterios medibles, negativos y pruebas. Se presentan en tandas y se implementan según dependencias |
| ESTADO.md | Situación y enlaces cuando corresponde actualizar el estado; no repite esta propuesta ni presenta capacidades como terminadas |

La entrega documental se integra sin interrumpir un Issue en curso. La cola de implementación vigente no cambia por publicar una propuesta. Su ubicación en el milestone actual identifica el trabajo documental de planificación; no incorpora las capacidades posteriores como requisitos ocultos de G2.

## 8. Condiciones de revisión de esta propuesta

El contenido está listo para discutir alcance si mantiene visibles las capacidades nombradas en §1, distingue evidencia y recomendación, conserva ejemplos con resultados, expone dependencias/coste y enlaza las fuentes primarias. La comparación debe poder revisarse sin conocer el chat que la originó.

Para su integración documental se comprobarán enlaces relativos, tablas, formato y correspondencia con la revisión presentada. No hacen falta pruebas artificiales de producto para un cambio exclusivamente documental; el PR conserva los checks aplicables del repositorio.

Al aprobar una capacidad se elaborará su diseño técnico y el desglose de entregas, con observabilidad, seguridad, propiedades, integración y e2e según las capas afectadas. La aprobación documental, la aceptación de una capacidad y la verificación de su implementación son estados diferentes y se registran por separado.

## 9. Comparativa detallada con referentes y fuentes

### 9.1 Qué se compara y cómo se decide

Encargo del propietario: comparar las herramientas BI de referencia con nuestra plataforma y seleccionar las capacidades más potentes y útiles para incorporarlas con diseño propio. El alcance de esta revisión es **motor semántico, ejecución y experiencia analítica que depende de ellos**. No pretende evaluar toda la suite de conectores, administración, diseño visual, distribución o precios de cada fabricante.

Se estudian Power BI, Tableau, Looker, Qlik Sense, Sigma y ThoughtSpot; Cube es una referencia técnica adicional de capa semántica, no una suite idéntica a las anteriores. Se eligen entre dos y cuatro referentes por familia de capacidad. La selección se basa en capacidades documentadas; no es un ranking de cuota de mercado ni un benchmark competitivo.

Una candidata pasa a la selección principal si:

1. Resuelve una familia de preguntas generales, no un dashboard de un negocio particular.
2. Aporta algo que falta o hace utilizable una base existente; no renombra lo que ya funciona.
3. Puede expresarse en definiciones/consultas gobernadas y compartirse entre consumidores.
4. Conserva exactitud, permisos, portabilidad declarada y trazabilidad comprobables.
5. Tiene una demostración concreta, límites identificados y dependencias compatibles con nuestra arquitectura.

El efecto visible importa, pero también el coste continuo de semántica, seguridad, ejecución y compatibilidad. No se asignan puntuaciones numéricas ni tiempos de entrega sin mediciones.

**Límite de evidencia:** las afirmaciones sobre los referentes proceden de las fuentes primarias enlazadas, consultadas el 2026-10-17, y del código fijado de Cube. No se probaron instalaciones comerciales. Las recomendaciones de diseño para SEF son análisis propio, no afirmaciones de que esas herramientas carezcan de capacidades equivalentes. Las ediciones, estados de preview y límites documentados se conservan.

### 9.2 Qué tomaría de cada herramienta, frente a lo nuestro

| Herramienta / evidencia | Aporte especialmente útil | Comparación con nuestro estado | Qué tomar y qué adaptar |
|---|---|---|---|
| **Power BI** — [CALCULATE](https://learn.microsoft.com/en-us/dax/calculate-function-dax), [ALLSELECTED](https://learn.microsoft.com/en-us/dax/allselected-function-dax), [grupos de cálculo](https://learn.microsoft.com/en-us/analysis-services/tabular-models/calculation-groups) | Modificar contexto por expresión, distinguir total seleccionado y total general, reutilizar elementos sobre medidas explícitas | Tenemos separación de filtros y políticas, ratios y referencias; no el contrato general de contexto por subexpresión ni grupos | Incorporar esa expresividad con operadores tipados, ámbitos explícitos y grupos de cálculo. No reproducir todo DAX ni hacer que el significado dependa de un comportamiento implícito del gráfico |
| **Tableau** — [LOD](https://help.tableau.com/current/pro/desktop/en-us/calculations_calculatedfields_lod.htm), [varios hechos](https://help.tableau.com/current/pro/desktop/en-us/datasource_mfr_multiple_base_tables.htm), [Explain Data](https://help.tableau.com/current/pro/desktop/en-us/explain_data_explained.htm) | Separar detalle del cálculo y del visual, relacionar hechos mediante dimensiones compartidas y explorar posibles explicaciones | Tenemos grano, alineación de hechos y certificados; falta autoría general de LOD y experiencia completa | Rescatar LOD y su facilidad de exploración. Mantener nuestra estrategia de corrección. Tratar sugerencias estadísticas como hipótesis: la propia documentación distingue relación de causalidad |
| **Looker** — [symmetric aggregates](https://docs.cloud.google.com/looker/docs/best-practices/understanding-symmetric-aggregates), [drill_fields](https://docs.cloud.google.com/looker/docs/reference/param-field-drill-fields), [aggregate awareness](https://docs.cloud.google.com/looker/docs/aggregate_awareness), [datagroup](https://docs.cloud.google.com/looker/docs/reference/param-model-datagroup) | Medidas resistentes a duplicación por joins, navegación declarada, selección de agregados y coordinación de frescura | Corrección de fanout y definiciones gobernadas existentes; faltan recorrido de drill y aceleración operativa | Tomar navegación y coherencia de frescura. Conservar agregación exacta por hecho; no adoptar como base el empaquetado numérico con hash de su ejemplo |
| **Qlik Sense** — [estados alternativos](https://help.qlik.com/en-US/sense/May2026/Subsystems/Hub/Content/Sense_Hub/Visualizations/alternate-states-comparative-analysis.htm), [selecciones asociativas](https://help.qlik.com/en-US/cloud-services/Subsystems/Hub/Content/Sense_Hub/Selections/associative-selection-model.htm) | Comparar selecciones independientes y mostrar estados de los valores al filtrar | Un conjunto de bloques de consulta no equivale a contextos de exploración nombrados ni a una API de valores posibles | Incorporar comparación de contextos y facetas autorizadas. Adaptarlo a SEF; no sustituirlo por un motor asociativo residente en memoria. Distinguir exclusión analítica y falta de autorización |
| **Sigma** — [métricas reutilizables](https://help.sigmacomputing.com/docs/create-and-manage-metrics), [modelos y linaje](https://help.sigmacomputing.com/docs/navigate-data-models) | Acercar la creación de métricas y elementos reutilizables a una experiencia de autor, con contexto de uso y dependencias | Hay modelos versionados y diff semántico; faltan el ciclo visual de creación/publicación y sus vistas de impacto | Tomar accesibilidad de autoría, reutilización y visibilidad de dependencias. Una definición validada para visual, archivos e IA; fórmulas de negocio evaluadas en el servidor |
| **ThoughtSpot** — [modelo para Spotter](https://docs.thoughtspot.com/cloud/26.10.0.cl/spotter-model), [preguntas de referencia](https://docs.thoughtspot.com/cloud/26.10.0.cl/spotter-reference-questions), [permisos de memoria](https://docs.thoughtspot.com/cloud/26.9.0.cl/spotter-coaching-permissions) | Vocabulario del modelo, ejemplos de intención y separación de conocimiento personal y global para preguntas en lenguaje natural | Hay metadatos/contexto de IA; no hay un intérprete conversacional gobernado entregado | Rescatar preguntas verificadas y vocabulario con permisos como material de evaluación/autoría. Su documentación marca las preguntas de referencia como memoria heredada y recomienda memoria desde conversaciones: no copiar esa API particular ni suponer que un ejemplo garantiza exactitud |
| **Cube** — [medidas multietapa](https://docs.cube.dev/docs/data-modeling/measures), [matching de preagregaciones](https://docs.cube.dev/docs/pre-aggregations/matching-pre-aggregations), código fijado en §9.5 | Componer etapas con grano/filtros propios y sustituir trabajo por agregados compatibles | Tenemos DAG de ejecución y prueba de obligaciones; faltan operadores generales de contexto y un servicio de aceleración | Tomar organización de etapas, pruebas adversariales y selección de agregados. Mantener tipos, contratos y ejecución propios, sin Cube como dependencia |

Las capas de seguridad y el ciclo de autoría de cada producto tienen configuraciones y límites propios. Esta tabla no afirma que una función se ofrezca en todas sus ediciones ni que soporte cualquier combinación de métrica, fuente y permiso.

### 9.3 Matriz de brechas: capacidades concretas y evidencia nuestra

**Base** significa que encontramos contratos/código y evidencias anteriores del dominio acotado; no certifica toda la plataforma. **Parcial** significa piezas separadas sin recorrido completo demostrado. **Propuesta** significa que no encontramos un contrato/implementación general de la capacidad en el alcance revisado. No confundir estos estados con «terminado».

| Capacidad | Referentes aplicables | Estado de lo nuestro | Brecha que cambia la experiencia |
|---|---|---|---|
| Métricas reutilizables y gobernadas | Looker, Power BI, Sigma | **Base:** CSMIR/Ossie, gobierno y versiones | Edición/publicación accesible, catálogo permitido y consumo completo |
| Filtros de fila y posteriores a agregación | Power BI, Tableau, Cube | **Base:** AST y reglas separadas | Composición HTTP/UI y distinción entre origen del filtro y selección temporal |
| Cambiar contexto dentro de una expresión | Power BI, Cube | **Propuesta general** | Denominadores y subcálculos con ámbitos propios, sin retirar RLS |
| Total seleccionado frente a total autorizado | Power BI, Qlik | **Propuesta general** | Semántica de selección exterior, agrupación y límites explícitos |
| LOD y agregaciones anidadas | Tableau, Cube | **Parcial:** granos/bloques y recomposición | Fijar/ampliar/reducir grano de subexpresiones; evitar promedios de promedios accidentales |
| Ratios, distintos y semiaditividad | Power BI, Looker, Cube | **Base acotada:** reglas/corpus | Mantener exactitud al añadir operadores, contexto y aceleración; no anunciar cobertura universal |
| Relaciones, varios hechos y fanout | Tableau, Looker, Cube | **Base:** grano/caminos/alineación | Autoría comprensible y aplicación íntegra en resultados ejecutados |
| Períodos comparables y calendarios de negocio | Power BI, Cube | **Parcial:** tiempo tipado; operaciones avanzadas diferidas | Alineación de períodos, corte comparable, calendario versionado y datos faltantes |
| Grupos de cálculo | Power BI, composición de Cube | **Propuesta general** | Elementos reutilizables, compatibilidad, precedencia y formato |
| Comparación de contextos A/B | Qlik, Power BI | **Parcial:** bloques independientes | Estados nombrados y comparación de selecciones del mismo usuario; distinto de la demo de seguridad A/B |
| Valores posibles al seleccionar | Qlik, exploración de Tableau | **Propuesta** | Facetas eficientes sin revelar valores ni conteos no autorizados |
| Drill del modelo | Looker, Tableau | **Parcial:** permisos/relaciones y obligación de G2 | Destinos autorizados, navegación reversible y contexto conservado |
| Preagregaciones y frescura coordinada | Looker, Cube, Power BI | **Parcial:** equivalencia diseñada | Construcción, invalidación, selección y observación reales; separado de #100 |
| Autoría y publicación de medidas | Sigma, Looker, Power BI | **Parcial:** definiciones en Git y diff | Borrador, validación, vista previa y publicación de una definición común |
| Impacto de cambiar una definición | Sigma y nuestra dirección de Studio | **Base estructural:** diff semántico y dependencias | Mostrar qué cifras cambian sobre las mismas versiones, permisos y consultas; el diff de código no basta |
| Explicar y contrastar una cifra | Tableau, Looker y nuestras leyes | **Parcial:** explain/certificado y procedencia del adaptador | Recibo HTTP real, explicación legible, detalle autorizado y reproducción con límites |
| Preguntas naturales verificables | ThoughtSpot, Power BI | **Propuesta en experiencia IA** | Resolver intención a consultas canónicas, preguntar ante ambigüedad y evaluar contra ejemplos autorizados |
| Conformidad entre motores | Cube como referencia de adaptadores; contrato propio | **Base contractual; un adaptador de ejecución** | Segundo adaptador real más oráculo independiente antes de afirmar portabilidad probada |

La fuente propia para esta matriz son los contratos de [modelo/consulta](../../semantica/03_MODEL_AND_QUERY_IR.md), [corrección](../../semantica/04_CORRECTNESS_AND_PLANNING.md), [ejecución](../../semantica/05_EXECUTION_AND_ADAPTERS.md) y [conformidad](../../semantica/07_TESTING_AND_CONFORMANCE.md); código y límites de comprobación se detallan en §9.5. No se usa la cantidad de casillas como porcentaje de avance del producto.

### 9.4 Alternativas que propongo dejar fuera de esta tanda

| Candidata | Evaluación y decisión propuesta |
|---|---|
| Compatibilidad completa con DAX, LookML y expresiones Qlik | Coste sostenido de varios lenguajes/semánticas. Tomar capacidades y conceptos reconocibles, preservando formatos y contratos propios |
| Clonar VertiPaq, Hyper o un motor asociativo completo | Cambia el proyecto y duplica infraestructura. Medir primero los adaptadores, ejecución columnar y materializaciones de nuestra dirección vigente |
| Fórmulas particulares de negocio en cada gráfico | Favorecen discrepancias. Las operaciones de negocio tienen definición y alcance explícitos en el motor; la presentación no recalcula la verdad |
| Elegir joins ambiguos o aproximar para «hacer que funcione» | Mantener rechazo explicable y precisión declarada. El éxito aparente con cifras equivocadas no satisface ninguna capacidad |
| IA que ejecuta SQL libre o crea significados silenciosamente | La futura IA consume contratos, catálogo permitido y punto único; pide aclaración o propone una definición revisable |
| Inferir causalidad mediante correlaciones o una narrativa | Separar asociación estadística, reconciliación aritmética y causalidad. No seleccionar un «detector de causas» genérico sin diseño y evidencia |
| Cohortes, simulaciones y jerarquías avanzadas como módulos obligatorios ahora | Son casos potentes para evaluar la expresividad, pero no necesitan una solución vertical. Retenerlos como candidatos hasta contrastar demanda, operadores y coste. El drill básico sigue siendo obligatorio |
| Writeback y planificación transaccional por parecer una hoja de cálculo | Abren contratos de escritura, concurrencia y autorización distintos. Fuera de esta selección del motor analítico; no se incorporan implícitamente al tomar ideas de Sigma |

La exclusión de esta tanda es una decisión de foco propuesta, no una afirmación de que esas capacidades nunca tengan valor.

### 9.5 Fuentes, versiones y trazabilidad

**Comprobación propia:** `main` consultado `99f711c1c648ca2ed454bc7ff43cffb7492863c1`. Se comparó el hash Git de cinco archivos locales leídos con sus blobs remotos: todos coincidieron. Esto identifica el código revisado, no acredita el resto del clon ni un resultado de pruebas nuevas:

- [Expresiones y tipos de cálculo](../../../backend/crates/sef-model-ir/src/expressions.rs).
- [Entrada de consultas y filtros](../../../backend/crates/sef-query-ir/src/unbound.rs).
- [Diff semántico y dependencias](../../../backend/crates/sef-model-ir/src/diff.rs).
- [Alcance del planificador base](../../../backend/crates/sef-planner/src/lib.rs).
- [Procedencia del adaptador](../../../backend/crates/sef-adapter-duckdb/src/provenance.rs).

También se inspeccionaron los módulos locales de explain/composición y las rutas registradas. No se ejecutaron pruebas del núcleo, integración, navegador ni benchmarks para elaborar esta comparación. Las evidencias previas citadas siguen teniendo su commit y alcance originales.

**Cube [CÓDIGO]:** commit `e15efc589083e85b0912056bea6d1ae2eb68c720`, archivos inspeccionados:

- [PlanningScope](https://github.com/cube-js/cube/blob/e15efc589083e85b0912056bea6d1ae2eb68c720/rust/cube/cubesqlplanner/cubesqlplanner/src/planner/planners/multi_stage/planning_scope.rs): alcance de planificación, estado de etapas y contexto de evaluación.
- [Planificador de etapas](https://github.com/cube-js/cube/blob/e15efc589083e85b0912056bea6d1ae2eb68c720/rust/cube/cubesqlplanner/cubesqlplanner/src/planner/planners/multi_stage/member_query_planner.rs): propagación del contexto/grano a consultas, series y ventanas.
- [Pruebas Rust de filtros multietapa](https://github.com/cube-js/cube/blob/e15efc589083e85b0912056bea6d1ae2eb68c720/rust/cube/cubesqlplanner/cubesqlplanner/src/tests/integration/multi_stage/filter_directive.rs): se inspeccionaron casos que distinguen población de la medida y filas visibles, claves NULL y dimensión filtrada fuera del grano. Pruebas leídas, no ejecutadas.

**Fuentes primarias [DOCS], consultadas el 2026-10-17:**

- [Power BI: contexto de filtros](https://learn.microsoft.com/en-us/dax/calculate-function-dax) y [grupos de cálculo](https://learn.microsoft.com/en-us/analysis-services/tabular-models/calculation-groups).
- [Tableau: LOD](https://help.tableau.com/current/pro/desktop/en-us/calculations_calculatedfields_lod.htm) y [orden de operaciones](https://help.tableau.com/current/pro/desktop/en-us/order_of_operations.htm).
- [Looker: drill](https://docs.cloud.google.com/looker/docs/reference/param-field-drill-fields), [symmetric aggregates](https://docs.cloud.google.com/looker/docs/best-practices/understanding-symmetric-aggregates) y [aggregate awareness](https://docs.cloud.google.com/looker/docs/aggregate_awareness).
- [Cube: medidas multietapa](https://docs.cube.dev/docs/data-modeling/measures) y [matching de preagregaciones](https://docs.cube.dev/docs/pre-aggregations/matching-pre-aggregations).

- [Power BI: ALLSELECTED y total seleccionado](https://learn.microsoft.com/en-us/dax/allselected-function-dax).
- [Power BI: inteligencia temporal y estado de preview](https://learn.microsoft.com/en-us/power-bi/transform-model/desktop-time-intelligence).
- [Power BI: Direct Lake y su arquitectura](https://learn.microsoft.com/en-us/fabric/fundamentals/direct-lake-overview).
- [Power BI: respuestas verificadas para Copilot](https://learn.microsoft.com/en-us/power-bi/create-reports/copilot-prepare-data-ai-verified-answers). Se toma la idea de respuestas aprobadas asociadas al modelo; no se confunde con una prueba de todas las respuestas generadas.
- [Tableau: relaciones de varios hechos](https://help.tableau.com/current/pro/desktop/en-us/datasource_mfr_multiple_base_tables.htm).
- [Tableau: alcance de Explain Data](https://help.tableau.com/current/pro/desktop/en-us/explain_data_explained.htm) y [tipos de explicación](https://help.tableau.com/current/pro/desktop/en-us/explain_data_explanation_types.htm).
- [Looker: coordinación de caché y persistencia con datagroups](https://docs.cloud.google.com/looker/docs/reference/param-model-datagroup).
- [Qlik Sense May 2026: estados alternativos](https://help.qlik.com/en-US/sense/May2026/Subsystems/Hub/Content/Sense_Hub/Visualizations/alternate-states-comparative-analysis.htm).
- [Qlik Cloud: estados de selección asociativa](https://help.qlik.com/en-US/cloud-services/Subsystems/Hub/Content/Sense_Hub/Selections/associative-selection-model.htm).
- [Sigma: creación/gobierno de métricas](https://help.sigmacomputing.com/docs/create-and-manage-metrics) y [modelos, uso y linaje](https://help.sigmacomputing.com/docs/navigate-data-models).
- [ThoughtSpot Cloud 26.10: vocabulario del modelo](https://docs.thoughtspot.com/cloud/26.10.0.cl/spotter-model), [preguntas de referencia y su condición de memoria heredada](https://docs.thoughtspot.com/cloud/26.10.0.cl/spotter-reference-questions) y [Cloud 26.9: niveles/permisos de memoria](https://docs.thoughtspot.com/cloud/26.9.0.cl/spotter-coaching-permissions).

Los enlaces a documentación viva conservan fecha de consulta; al convertir una capacidad en especificación se fijarán de nuevo sus versiones y límites aplicables. Las comparaciones no transfieren código, licencias, promesas comerciales ni dependencias de esos productos a nuestra plataforma.
