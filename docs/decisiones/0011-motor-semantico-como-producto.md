# 0011 — El motor semántico como producto: un motor, tres formas de entrega, Apache-2.0

**Estado:** aprobada por el propietario, 2026-10-11. **Amplía:** [0006](0006-motor-semantico-sef.md) (SEF como motor) y [0008](0008-modulos-desacoplados-por-contratos.md) (núcleos reutilizables). **Se apoya en:** [0010](0010-seguridad-del-blueprint-en-g2.md) (punto único de aplicación de permisos).

## Contexto
El propietario quiere que el motor semántico pueda usarse o venderse fuera de bi-platform. Se contemplan dos vías:
- **como pieza de otra herramienta:** recibe un modelo Apache Ossie y genera SQL, primero en DuckDB y en otros dialectos a medida que se integren motores;
- **como un servicio independiente, al estilo de Cube**, al que se conectan pantallas de BI, asistentes de IA u otras capas para consumir métricas.

La ADR 0008 ya separa el núcleo (`sef-*`) del servidor y de los motores, pero no fija cómo se entrega fuera de bi-platform ni con qué licencia.

## Decisión
1. **Un solo motor, tres formas de entrega, en este orden:**

   | Orden | Forma | Cuándo | Condición para empezar |
   |---|---|---|---|
   | 1 | Dentro de bi-platform | G2 (ahora) | Los 21 casos del corpus, con resultado exacto |
   | 2 | Biblioteca: crates Rust y, después, enlaces para Python | Al cerrar G2 | API pública estable y versionada (semver), con documentación y ejemplos |
   | 3 | Servicio independiente | Cuando haya demanda real | Motor de permisos dentro del motor, autenticación (OIDC), caché, límites por cliente y observabilidad |

   No son tres productos: el núcleo es el mismo código, y cada forma es un adaptador en el borde (ADR 0008).
2. **Desde hoy, dos reglas de diseño** que mantienen abiertas las tres formas:
   - **Contrato neutral.** El motor recibe un modelo Ossie (o CSMIR) y una consulta (SQIR), y entrega un plan, SQL del dialecto pedido o un resultado. Ningún tipo de `bi-*` aparece en su interfaz pública.
   - **Permisos dentro del motor.** El contexto de seguridad de quien pregunta (identidad, grupos, atributos y valores de contexto) es una entrada del plan semántico, como exige la ADR 0010. Así, el servicio independiente aplica los mismos permisos que bi-platform, sin depender de sus pantallas.
3. **Interfaces del servicio, cuando llegue: estándares, no inventos propios.** Se usan las capas de consulta que Apache Ossie está definiendo (`docs/referentes/ossie-especificacion.md` §2 y §9):
   - SQL con `MEASURE()` (apache/ossie#354), para que herramientas como Tableau, Power BI o Excel lo vean como una base de datos;
   - consulta estructurada por HTTP con resultados Arrow (#568);
   - MCP para asistentes de IA (#529);
   - nuestra API OpenAPI con JSON tipado (ADR 0008).
4. **Licencia: Apache-2.0 para toda bi-platform**, motor incluido (`sef-*` y `sef-adapter-*`; SEF ya la declaraba). Cada crate la declara en `Cargo.toml`, y al publicar se añaden `LICENSE` y `NOTICE`.

## Alternativas descartadas
- **Solo dentro de bi-platform.** Cierra la vía de producto que el propietario quiere, y la separación ya está pagada (ADR 0008).
- **Servicio independiente desde el principio.** Obligaría a mantener tres superficies antes de demostrar que el motor es exacto, y a hacerlo sin motor de permisos.
- **Licencia comercial o doble.** Frena la adopción de un componente cuya ventaja es ser estándar (Ossie). El negocio se basa en el producto, el servicio gestionado, el soporte y las funciones de empresa fuera del motor.

## Consecuencias
- El motor de permisos (Issue #69) se diseña como parte del motor semántico, no de `bi-server`.
- Al cerrar G2 se abre un Issue para la API pública de la biblioteca: superficie mínima, semver, documentación y ejemplos.
- La licencia permite a cualquiera usar el código, pero no el nombre del producto: conviene registrar la marca (decisión del propietario, fuera del repositorio).
