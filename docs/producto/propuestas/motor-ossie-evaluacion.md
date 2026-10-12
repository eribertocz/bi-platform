# Evaluación: «Motor semántico de primer nivel nativo de Apache Ossie»

*Evaluación de Claude (2026-10-12) del documento del propietario `motor-ossie-propietario.md`. El propietario pidió rescatar lo bueno e indicar qué ya está contemplado. Lo nuevo que se adopta queda en Issues; nada cambia una ADR vigente.*

## Veredicto
Construir el motor alrededor de Apache Ossie es lo correcto y ya está decidido:
- Ossie es el formato de entrada y salida (ADR 0006).
- El modelo interno es propio (CSMIR), así que un cambio de Ossie 0.x solo toca el adaptador `sef-ossie`.
- Lo no estándar va en `custom_extensions`, bajo el espacio de nombres `bi-platform`.

Unas tres cuartas partes del documento **ya están contempladas o construidas**, en algún punto con una garantía más fuerte. Se adoptan **cinco ideas nuevas**. Se descartan, con motivo, las que contradicen decisiones vigentes o añaden complejidad sin necesidad.

## Una aclaración de arquitectura: los motores de datos son conectores
El motor semántico **no contiene ningún motor de datos**.
- El núcleo (`sef-model-ir`, `sef-ossie`, `sef-query-ir`, `sef-correctness`, `sef-exec-ir`, `sef-planner`, `sef-authz`) produce un plan independiente del motor (SEIR).
- Cada motor es un **conector aparte** (`sef-adapter-duckdb`; mañana `sef-adapter-clickhouse`, `sef-adapter-postgres`…) que traduce ese plan a su SQL y lo ejecuta.
- CI lo impide de forma automática (ADR 0008, paso *Layering* de `backend.yml`): un núcleo que dependa de `duckdb`, `clickhouse`, HTTP, almacenamiento o de un conector no se integra.

La decisión «un solo motor» (ADR 0001, #25) se refiere a otra cosa: el motor **interno** de la plataforma (lago DuckLake, ELT, caché, materializado) es DuckDB. El motor semántico puede consultar en vivo otros motores, como ClickHouse, a través de sus conectores (ADR 0002 y 0009).

## Ya contemplado o construido

| Propuesta del documento | Dónde está |
|---|---|
| Semántica primero; SQL como un *backend* más | SEF (ADR 0006); plan SEIR independiente del motor; conectores (ADR 0008) |
| Modelo inmutable y versionado | CSMIR con huella canónica de cada objeto y del modelo; definiciones en Git (ADR 0003) |
| Arrow como formato columnar | Intercambio Arrow para volúmenes y JSON exacto para pantallas (ADR 0008); DuckDB ya es columnar |
| Protección formal contra *fan-out*, *chasm traps*, grano y agregaciones inválidas | Motor de corrección (#45): obligaciones demostradas, corpus normativo y propiedades |
| Crates independientes con contratos claros | Puertos y adaptadores, vigilados en CI (ADR 0008) |
| Observabilidad en cada capa | `tracing` con `request_id` y métricas OpenTelemetry (M15) |
| Solo el subconjunto estable de Ossie; lo demás por extensiones | Extensión `bi-platform` v1, versionada y con esquema cerrado (`sef-ossie`) |
| Validación estricta contra el esquema oficial | Prueba de conformidad contra el esquema oficial y las reglas de `validate.py` |
| Resolución de entidades, grano, cardinalidades y dependencias | Conversión a CSMIR y motor de corrección |
| Plan lógico independiente del dialecto | Planificador SEIR con certificado verificado de forma independiente (#46) |
| Autenticación y autorización nativas | OIDC (#70), Cedar con denegación ante errores, columnas enmascaradas y filas por política (ADR 0012) |
| Catálogo y descubrimiento de métricas | Pantalla y API Modelos (#65, #80), con metadatos de gobierno (#89) |
| Interfaz SQL acotada | Editor SQL bloqueado hasta el punto único (ADR 0010) |
| Plazos, aislamiento de fallos y admisión | Grupo de trabajadores con plazos, cancelación y admisión (`bi-runtime`, G1) |
| Trazabilidad completa y *query replay* | Recibo de cada cifra (ley 2) y reproducción desde el log (ley 9) |
| Validación contra resultados conocidos | Oráculo exacto y corpus de 21 casos; adaptador DuckDB contra el oráculo (#47) |
| *Pre-aggregations*, *aggregate awareness* y vistas materializadas | ELT con «acelerar» y reutilización de agregados solo si son válidos (decisiones 1 y 2, aprobadas) |
| Caché de resultados semánticos | Ley 4: clave con la huella de la decisión de permisos, tras el punto único (ADR 0010) |
| Explicación semántica | Explicación del plan con tipos y unidades (#86); vía semántica única (#98) |
| Calidad de datos (*consistency checks*) | Auditorías dentro de la publicación del ELT (G3) |
| Seguir la evolución de Ossie | Versión del esquema y de la extensión; los huecos de la especificación están documentados en `docs/referentes/ossie-especificacion.md` |

## Se adopta (nuevo)

| Idea | Por qué vale | Issue |
|---|---|---|
| **Diferencia semántica entre versiones del modelo** | Las huellas por objeto de CSMIR permiten decir «este cambio altera las métricas A y B» sin comparar texto. Alimenta el plan de cambios y el «¿qué rompo?» del Estudio | #99 |
| **Caché de modelos compilados por versión** | Hoy cada petición recompila el proyecto; con la huella del proyecto como clave se compila una vez por versión (principio de velocidad) | #100 |
| **Límite de memoria por consulta** | Una consulta enorme no debe tumbar a las demás; complementa plazos y admisión | #101 |
| **Interfaces MCP (agentes de IA) y Arrow Flight** | Encajan con el motor como biblioteca y servicio (ADR 0011) y con G5; siempre a través del punto único de permisos | #102 |
| **Aportes a la comunidad Ossie** | Somos de las primeras implementaciones en Rust y ya encontramos huecos en la especificación. Como los agentes usan la cuenta del propietario, preparan borradores y **él** decide publicarlos | #103 |

## Se descarta (con motivo)

| Propuesta | Motivo |
|---|---|
| Motor de ejecución propio o extender DataFusion | El motor interno es DuckDB (ADR 0001, #25), ya vectorial y columnar; otros motores entran como conectores. Un motor de ejecución propio son años de trabajo sin mejorar la exactitud |
| Optimizador por costes desde ya | Primero lo correcto: toda optimización tendrá que demostrar que da lo mismo que el plan de referencia certificado (§23 de la parte II de `docs/semantica/04_CORRECTNESS_AND_PLANNING.md`) |
| Clúster distribuido (estilo Ballista) | Innecesario a la escala actual; el motor como servicio (ADR 0011) cubre el crecimiento previsible |
| Tokio para ejecutar consultas | Las llamadas al motor bloquean y no pueden ir dentro de Tokio (AGENTS.md); por eso existe `bi-runtime` |
| Arenas de memoria e *interning* desde el día uno | Solo cuando una medición lo justifique; SEIR ya comparte nodos por contenido |
| Catálogos externos (Polaris, Unity) como base | El catálogo es el de DuckLake, elegido al crear el proyecto (ADR 0004); un conector a otros catálogos puede llegar cuando un cliente lo pida |
| *Multi-tenancy* nativa ahora | Las organizaciones quedan aplazadas (ADR 0010) |
