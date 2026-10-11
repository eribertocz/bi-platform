# Registros de decisión (ADR)

`docs/arquitectura/` es la **base histórica** del diseño: no se modifica (hashes en `INVENTARIO.md`). Las mejoras y correcciones se registran aquí, una decisión por archivo. **Donde un registro contradice a la base, prevalece el registro.**

Cada registro contiene: contexto, decisión, alternativas descartadas, consecuencias y la parte de la base a la que sustituye. Las decisiones de producto requieren la aprobación del propietario (@eribertocz). Las técnicas rutinarias, no.

| Nº | Decisión | Estado | Sustituye |
|---|---|---|---|
| [0001](0001-un-solo-motor-duckdb.md) | Núcleo con un solo motor analítico: DuckDB | Aprobada 2026-10-11 | Enmienda I.2 de `DECISIONES_TECNICAS_RC2.md` (Polars de primera clase) |
| [0002](0002-fuentes-y-conexiones-en-vivo.md) | Fuentes modernas, conexiones en vivo y ClickHouse como segunda fuente | Aprobada 2026-10-11 | Amplía M01 §2.5 y M13 §6.1 |
| [0003](0003-proyecto-como-archivos.md) | Definiciones como archivos de un proyecto en Git (estilo Rill) | Aprobada 2026-10-11 | Concreta M10 §0.1–0.3 |
| [0004](0004-conexion-elegida-al-crear-el-proyecto.md) | Conexión elegida al crear el proyecto; DuckLake con catálogo y datos locales o remotos | Aprobada 2026-10-11 | Lago único por variables de entorno (G1); matiza el catálogo solo PostgreSQL |
| [0005](0005-modelos-sql-estilo-sqlmesh.md) | Modelos SQL estilo SQLMesh (staging → intermediate → mart) con motor de cálculo (`engine`) y destino (`output`) separados | Aprobada 2026-10-11 | Amplía 0001 y el formato de modelos de M02 |
| [0006](0006-motor-semantico-sef.md) | El motor semántico es SEF (CSMIR/SQIR/SEIR, corpus de 21 casos), integrado en bi-platform | Aprobada 2026-10-11 | Diseño semántico de M04 y la parte semántica de M05 |
| [0007](0007-construccion-propia-por-capa.md) | Construcción propia por capa al nivel de Atlas BI (sin integrar Cube, SQLMesh, Airflow…); sí motores, librerías y estándares | Aprobada 2026-10-11 | Estrategia de producto |
| [0008](0008-modulos-desacoplados-por-contratos.md) | Módulos desacoplados por contratos (puertos y adaptadores), vigilado en CI | Aprobada 2026-10-11 | Refuerza SEF 02/08 y M12/M13 |

Plantilla: copiar un registro existente y numerar con el siguiente número libre.
