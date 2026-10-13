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
| [0009](0009-modos-de-datos-y-persistencia-opcional.md) | Modos de datos: consultar en vivo, guardar resultados o pipeline; SOURCE, RAW, SILVER y GOLD lógicos y opcionales; lo operativo separado de la lógica | Aprobada 2026-10-11 | Matiza 0002, 0003, 0004 §6 y 0005 |
| [0010](0010-seguridad-del-blueprint-en-g2.md) | Punto único de aplicación de permisos; motor de permisos, auditoría mínima y OIDC en G2; WCAG 2.2 AA | Aprobada 2026-10-11 | Amplía el gate G2 del roadmap y concreta el blueprint del propietario |
| [0011](0011-motor-semantico-como-producto.md) | El motor semántico como producto: dentro de bi-platform, como biblioteca y como servicio independiente; Apache-2.0 para toda bi-platform | Aprobada 2026-10-11 | Amplía 0006 y 0008 |
| [0012](0012-motor-de-permisos-cedar.md) | Motor de permisos: Cedar dentro del motor semántico (RBAC + ABAC, CLS en tres niveles; RLS sigue en CSMIR) | Aprobada 2026-10-12 | Concreta 0010 §A |

| [0013](0013-presupuesto-memoria-consulta.md) | Presupuesto independiente de memoria por consulta activa | Técnica, pendiente de revisión 2026-10-12 | Pool de conexiones G1; concreta M13 §13 y §19 |

Plantilla: copiar un registro existente y numerar con el siguiente número libre.
