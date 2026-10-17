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
| [0013](0013-presupuesto-memoria-consulta.md) | Presupuesto independiente de memoria por consulta activa | Aprobada 2026-10-13 | Pool de conexiones G1; concreta M13 §13 y §19 |
| [0014](0014-versiones-por-gate.md) | Versiones tentativas por gate: alpha en G2/G3, beta del alcance BI inicial desde G4 y objetivo de candidato a producción tras G6, siempre condicionados a evidencia | Aprobada 2026-10-13 | Correspondencia de versiones de «Milestones de release propuestos» de `ROADMAP_GLOBAL.md` |
| [0015](0015-navegador-contenedores-y-ubicacion-de-datos.md) | Navegador como experiencia y contenedores oficiales como distribución principal; servidor, acceso y ubicación de los datos son elecciones independientes; publicar y compartir sin copiar datos ni ceder los permisos del autor | Aprobada 2026-10-13 (funciones pendientes en G3/G4/G6) | Concreta 0003, 0004 y 0009; prevalece sobre PostgreSQL BI como formato exclusivo de dashboards en M08 |

Plantilla: copiar un registro existente y numerar con el siguiente número libre.

## Menciones obsoletas en documentos históricos

`docs/arquitectura/` no se modifica (huella en `INVENTARIO.md`): las menciones de abajo siguen escritas en esos archivos aunque una ADR ya las sustituyó. No seguirlas; son historia, no diseño vigente.

| Dónde | Qué dice | Qué manda en realidad |
|---|---|---|
| `docs/arquitectura/modulos/BI_Modulo_13_..._Polars_ADBC_2026-10-10.md` (título y cuerpo) | Polars como motor de transformación de primera clase, junto a DuckDB | ADR 0001: un solo motor, DuckDB. Polars fuera del núcleo |
| `docs/arquitectura/INDICE_17_MODULOS.md` (título de M13) | "Runtime analítico Rust, DuckDB y Polars" | Igual: ADR 0001 |
| `docs/arquitectura/DECISIONES_TECNICAS_RC2.md` (enmienda I.2) | "Polars 2.0 de primera clase" | Igual: ADR 0001 (la sustituye explícitamente) |
| `docs/semantica/06_TECHNOLOGY.md` (tabla de tecnología) | Polars para "DataFrame/ETL tooling" | ADR 0001. Este documento no está en `INVENTARIO.md`, pero se conserva igual (ver `docs/semantica/README.md`) |
| `docs/arquitectura/modulos/M08_Canvas_Dashboard_Composition_RC1.md` (§0 «Fuentes de verdad», §2 punto 2, §3 «Fuente de verdad» y §6 «Persistencia») | `PostgreSQL BI` como fuente de verdad de las definiciones y revisiones de dashboards (`DashboardRevision`) | ADR 0003 y 0015: la definición canónica está en los archivos del proyecto (`dashboards/*.yaml`); PostgreSQL de la plataforma guarda usuarios, sesiones, permisos, auditoría y referencias operativas a revisiones publicadas. La edición multiusuario opcional en PostgreSQL sigue pendiente de diseño en G4 |

**ADBC no está en esta lista:** sigue vigente para conectores a otras bases (ADR 0002, ADR 0007). Solo Polars se descartó del núcleo.
