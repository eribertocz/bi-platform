# Atlas BI: qué rescatamos del proyecto hermano

**Fuente:** repositorio local del propietario `E:\bi-platform`, commit `f29c1486` (2026-10-09), versión 0.5.x, `[CÓDIGO]` y `[DOCS]`.
- Atlas BI es otro producto del propietario y **seguirá más adelante**. Aquí solo se toman ideas; no se modifica ni se copia su código.
- Su stack es distinto: TypeScript, Cube, SQLMesh vía Python y Garage S3.
- Sus documentos están en `E:\bi-platform\docs\` (numerados 00–22).

**Nivel objetivo (decisión del propietario, 2026-10-11):** visualización, ELT, motor semántico y el resto de capacidades deben llegar **casi al mismo nivel que Atlas BI, pero con implementación propia**. Cada punto de esta ficha queda **aceptado** como nivel mínimo y se planifica en su gate.

**Documento de Atlas BI que marca el nivel de cada módulo** (leerlo antes de diseñar esa capacidad):

| Atlas BI (`E:\bi-platform\docs\`) | Módulo nuestro | Notas |
|---|---|---|
| 01 motor y almacenamiento | M13, M03 | DuckDB + DuckLake: igual que nosotros |
| 02 transformación | M02 | Nuestro ELT propio estilo SQLMesh (ADR 0005) |
| 03 ingesta | M01 | Fuentes de ADR 0002 |
| 04 semántico | M04 | Nuestro motor es SEF (ADR 0006); Atlas BI usaba Cube |
| 05 cerebro IA | M16 | |
| 06 API gateway | M12 | |
| 07 orquestación | M14 | Verbos de trabajos (A7) |
| 08 frontend, 20 producto UI/UX | M17 | Perfiles de usuario (A6) |
| 09 auth y gobernanza, 12 metadata | M11, M10 | |
| 10 colaboración en tiempo real | sin módulo propio aún | Valorar en G4–G6 |
| 14 interacción y estado | M06 | Filtros AST, drill, deshacer (A1) |
| 17 cálculos de autoservicio | M06, M04 | SQL sobre métricas (A2) |
| 18 ciudadanos de primera clase | lista de comprobación G2–G4 | (A4) |
| 19 operabilidad en ejecución | M15, M13 | |
| 21 estándares de ingeniería | AGENTS.md | Contratos versionados (A10) |
| 22 motor de visualización | M09 | VisualSpec y puerto de renderizado (A5) |

| # | Qué resolvió Atlas BI | Qué tomamos | Gate / módulo |
|---|---|---|---|
| A1 | **Los filtros son un AST, no `{campo: [valores]}`** (doc 14 §1.1): entre, mayor que, no está en, contiene, fechas relativas. Un filtro guardado por el autor y un clic temporal del usuario son entidades distintas. El drill conserva el contexto del nivel anterior, y deshacer/rehacer es exacto. | Modelo de filtros tipado desde el primer filtro semántico; distinción entre filtro del autor y filtro de la sesión; pila de navegación del drill. | G2 (M04/M06) |
| A2 | **Cálculos de autoservicio sin lenguaje propio** (doc 17): el autor de un dashboard escribe SQL estándar que referencia métricas gobernadas por nombre (`MEASURE()`). Se descartaron un lenguaje propio y las fórmulas tipo Excel. | Igual sobre nuestro motor: SQL semántico (Ossie SQL + métricas por nombre), el mismo de las APIs (Rill R12). | G4 (M06/M12) |
| A3 | **Una definición, tres caminos** (doc 21 §3.6): una medida creada en el editor visual, escrita como código o pedida por un agente acaba en la misma definición validada y persistida. Un agente nunca inventa una aplicación nueva en cada conversación. | Regla normativa: UI, archivos y asistente IA producen el mismo `metrics/*.yaml` (Ossie) y usan los mismos comandos. | G2–G5 (M04/M16) |
| A4 | **Ciudadanos de primera clase** (doc 18): filtros, bookmarks, matriz o pivot… nunca son un detalle de un visual, sino subsistemas con contrato propio, por niveles (Tier 0/1/2), aunque empiecen pequeños. | Usar su tabla de niveles como lista de comprobación de G2–G4. | G2–G4 |
| A5 | **Motor de visualización propio como diferenciador** (doc 22): `VisualSpec`, un puerto de renderizado (ECharts es una implementación), un sistema de diseño de visualización, movimiento y un SDK de visuales personalizados. | Contratos `VisualSpec` y puerto de renderizado desde el primer dashboard. ECharts queda detrás del puerto. | G4 (M09) |
| A6 | **Arquitectura de experiencia por perfiles** (doc 20): usuario de BI, autor y administrador. Una sola UI principal; las herramientas técnicas nunca son el frontend. "El usuario de BI nunca toca una terminal." | Principio de producto para M17: toda operación de usuario existe en la UI; la CLI es para operación. | G4 (M17) |
| A7 | **Verbos de trabajos uniformes** (CHANGELOG, "Plan C"): cada trabajo tiene un verbo, un resumen JSON versionado con un sobre común (`status`, `exitCode`, `retryable`, inicio y fin, error) y códigos de salida sysexits (0, 64, 65, 70, **75 reintentable**, 77, 78). | Mismo contrato para los trabajos de G3/G5 (`run`, `plan`, `test`, mantenimiento), invocables igual desde la UI, CI u orquestadores. | G3/G5 (M14) |
| A8 | **Mantenimiento del lago** (doc 01 §6.8): expirar versiones, borrar archivos que solo usaban ellas y huérfanos con un margen mínimo de 1 h, compactar. `--dry-run` informa exactamente lo mismo que la ejecución real, y fuera de desarrollo se pide confirmación escrita. | Igual para nuestro DuckLake (M03/M15 ya prevé las métricas de huérfanos). | G3 (M03) |
| A9 | **Niveles de lakehouse** (doc 01 §6.6–6.7): un nivel portátil (catálogo **SQLite** y carpeta local, un solo escritor) y un nivel de equipo (catálogo PostgreSQL y S3). | Revisar en el spike de G3 si el catálogo en archivo local de ADR 0004 debe ser SQLite (admite varios procesos locales) en lugar de DuckDB. | G3 (ADR 0004) |
| A10 | **Contratos versionados por separado** (doc 21 §2): API, esquema de informe, esquema de configuración y migraciones de BD, cada uno con su versión y listados en cada release. | Norma para nuestros contratos: OpenAPI ya versionado; añadir esquemas de proyecto y migraciones. | Continuo |
| A11 | **`preflight`**: comprueba los requisitos y explica cómo resolver lo que falta. | Comando de diagnóstico del entorno para quien instala la plataforma. | G6 |

## Lo que no tomamos
- **Cube y SQLMesh como dependencias en ejecución:** nuestro motor semántico es SEF (ADR 0006) y el ELT es propio, estilo SQLMesh (ADR 0005). Atlas BI ya confinaba ambos tras puertos propios, la misma idea que aplicamos.
- **Stack TypeScript y Python en el servidor:** el nuestro es Rust (ADR 0001).
