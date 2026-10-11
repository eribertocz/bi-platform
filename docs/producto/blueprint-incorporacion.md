# Incorporación del blueprint empresarial

**Fuente:** [`BLUEPRINT_2026-10-11.md`](BLUEPRINT_2026-10-11.md), documento del propietario. **Fecha del análisis:** 2026-10-11, sobre `main` en `7d5f53f`. **Responde a:** §17 del blueprint (primer entregable).

Este documento no sustituye a los ADR. Las decisiones de producto que propone (§7) necesitan la aprobación del propietario y, una vez aprobadas, se registran como ADR.

## 1. Conclusión

- **Coincidencias de fondo.** El blueprint coincide con lo ya decidido en los puntos que más pesan:
  - SQL estándar y una representación intermedia tipada en lugar de un lenguaje propietario: Ossie con `OSSIE_SQL_2026` y CSMIR (ADR 0006).
  - Módulos desacoplados y frontera control/data plane (ADR 0008).
  - Consultar sin copiar datos por defecto (ADR 0009).
  - Pruebas de oro: el corpus de 21 casos de SEF y el oráculo e2e.
- **Lo que más aporta:** lo que aún no tenemos.
  - Un **punto único de decisión y aplicación de autorización**, con denegación por defecto.
  - **Eventos de auditoría** con un esquema estable.
  - **Organizaciones y espacios de trabajo.**
  - La exigencia de demostrar, vía por vía, que **ninguna consulta evade las políticas**.
- **Varias "innovaciones" de su §12 salen casi gratis del diseño actual.** El motor ya calcula la huella de cada definición, las dependencias tipadas de cada métrica y, con #46, un certificado de corrección por consulta. DuckLake versiona los datos con instantáneas (§6).

## 2. Matriz de capacidades (§17.1)

**Estados:**
- **Implementado:** funciona con pruebas.
- **Parcial:** existe una parte con pruebas, o está decidido y empezado.
- **Ausente:** no existe.
- **Bloqueado:** no debe construirse hasta que exista otra pieza.

| Capacidad | Estado | Evidencia |
|---|---|---|
| Inicio de sesión y sesiones | **Parcial** | `bi-server/src/auth.rs`: cookie `HttpOnly; SameSite=Strict`, validación de `Origin` y sesiones en PostgreSQL (`migrations/0002_sessions.sql`). Pruebas en `tests/auth.rs` y `e2e/auth.spec.ts`. El acceso de desarrollo (`BI_DEV_USERS`) solo trae identidades sintéticas, y el build de producción no tiene selector. Falta la federación (OIDC), prevista para G2 en el propio `auth.rs`. |
| Organizaciones, espacios, grupos y roles | **Ausente** | No hay tablas ni rutas. |
| Autorización centralizada (sujeto + acción + recurso + contexto) | **Ausente** | Hoy solo se comprueba que exista una sesión. |
| RLS y políticas de filas | **Parcial** | Se declaran y compilan a CSMIR (`Policy`, `ContextSlot` enlazado como parámetro; caso 015 del corpus y `tests/csmir_governance.rs`), pero aún no se aplican al ejecutar (#45, #47). |
| CLS y enmascaramiento | **Ausente** | — |
| Auditoría de eventos | **Ausente** | Hay logs estructurados con `request_id` (`telemetry.rs`), que no son un registro de auditoría. |
| Linaje | **Parcial** | CSMIR guarda las dependencias tipadas métrica → medida → campo → origen (`Metric.dependencies`, relaciones y puentes). Falta el grafo navegable y la exportación OpenLineage (ADR 0008 §3). |
| Versionado de definiciones | **Parcial** | El proyecto son archivos en Git (ADR 0003) y CSMIR da una huella por objeto y por modelo. Falta publicar o revertir desde la plataforma. |
| Conectores | **Parcial** | DuckDB, Parquet y DuckLake con catálogo PostgreSQL (`bi-runtime/tests/lake.rs`). ClickHouse está decidido (ADR 0002) pero no implementado. |
| Modos en vivo y materializado | **Parcial** | El modo en vivo funciona sobre el dataset sintético; los demás están decididos (ADR 0009). |
| Ingesta, transformación y calidad | **Ausente** | G3 (ADR 0005). |
| Catálogo técnico (tipos exactos y nulabilidad) | **Parcial** | `sef-adapter-duckdb` con `DESCRIBE`, en revisión (#65). |
| Sandbox del motor | **Implementado** | `bi-runtime/src/bootstrap.rs`: `enable_external_access = false`, `allowed_directories` y `lock_configuration = true`. |
| Consulta tipada (valores exactos, límite de tiempo, cancelación) | **Implementado** | `bi-server/src/query/`. Los valores nunca llegan al texto SQL (proptest en `query/properties.rs`). Pruebas en `tests/query.rs` (`a_query_past_its_deadline_is_cancelled`). |
| Motor semántico | **Parcial** | Ossie → CSMIR: los 21 casos del corpus dan una huella idéntica (#63 y 3b de #48). Faltan SQIR (#44), corrección (#45), planificador (#46) y ejecución (#47). |
| Caché de resultados | **Ausente** | No existe; tampoco hay riesgo de filtración por caché. |
| Editor SQL | **Bloqueado** | Por el punto único de autorización (§3). |
| Explorador sin código | **Parcial** | `routes/explorar`: KPI, gráfico y filtro sobre el dataset sintético, con oráculo (`e2e/sales.oracle.json`) y axe (`e2e/explore.spec.ts`). Falta elegir métricas y dimensiones del modelo. |
| Visualización | **Parcial** | ECharts básico. Falta el contrato `VisualSpec` (ADR 0008 §5). |
| Dashboards, pivot, colaboración, exportación, alertas, *embedding* e IA | **Ausente** | G4 y G5. La exportación está **bloqueada** hasta el punto único de autorización. |
| Observabilidad | **Parcial** | Logs JSON con `request_id`, métricas OTLP por HTTPS (#60) y `/health/live` y `/health/ready`. Faltan las trazas distribuidas OTel. |
| Accesibilidad | **Parcial** | axe en e2e; falta declarar el objetivo WCAG 2.2 AA (§7). |
| Copias de seguridad y restauración | **Ausente** | G6. |

## 3. Fronteras de seguridad: vías por las que una consulta podría evadir políticas (§17.2)

**Regla que se propone (§7, punto A):** todo acceso a datos pasa por **un único punto de aplicación**, el plan del motor semántico con su contexto de seguridad (identidad, grupos, atributos y valores de contexto). Ninguna vía ejecuta SQL que no haya generado ese plan, o que el plan no haya envuelto con las políticas del usuario.

| Vía | Hoy | Riesgo cuando haya datos reales | Control previsto |
|---|---|---|---|
| `POST /api/v1/queries` (consulta tipada sobre el catálogo) | Basta una sesión; solo hay datos sintéticos | Cualquier usuario vería todas las filas | Migrar a la consulta semántica con políticas (#44–#47). Retirar la consulta directa o restringirla a administradores. |
| `GET /api/v1/models` (#65) | Basta una sesión | Nombres de orígenes, columnas y errores visibles para cualquiera | Permiso `view` sobre el proyecto o el modelo |
| Editor SQL (futuro) | No existe | DuckDB podría leer archivos, conectarse a otras bases (`ATTACH`) o saltarse la RLS consultando las tablas directamente | Además del sandbox: SQL sobre métricas (`MEASURE()`) compilado por el mismo plan, y solo relaciones envueltas por política por usuario. Nunca SQL directo sobre tablas base. |
| Caché (futuro) | No existe | Un resultado de A servido a B | Clave = plan canónico + huella CSMIR + **huella de la decisión de política** + instantánea de datos (§6, idea 4) |
| Exportaciones, alertas, *embedding* e IA (futuro) | No existen | Cada canal nuevo es una puerta | Todos invocan el mismo plan. La IA genera **consultas semánticas (SQIR)**, no SQL libre. |
| Conexión compartida de DuckDB | Una para todo el servidor | Estado de sesión (`SET`) que pasa de un usuario a otro | Los valores de contexto van siempre como parámetros enlazados, nunca como variables de sesión. Ya es así en CSMIR (`ContextSlot`). |

**Prueba obligatoria** (blueprint §4.2): una batería de aislamiento "A no obtiene datos de B" que crece con cada vía. Se añade a la batería de conformidad.

## 4. Qué adoptamos

| Idea del blueprint | Cómo encaja aquí | Cuándo |
|---|---|---|
| Autorización `sujeto + acción + recurso + contexto → decisión + restricciones + motivo`; denegación por defecto; RBAC + ABAC (+ ReBAC si hace falta) | Punto de decisión en el servidor y punto de aplicación en el plan semántico. Las restricciones (filtros de fila, columnas ocultas) viajan al plan como políticas CSMIR. | G2: ADR de autorización, §8 |
| Acciones (`view`, `query`, `export`, `manage_credentials`…) y recursos | Vocabulario cerrado del ADR; cada ruta declara su acción | G2 |
| Esquema de evento de auditoría (§5.1) | Tabla de solo adición en PostgreSQL con encadenamiento por hash (integridad verificable). Los eventos de seguridad nunca se muestrean. | G2 (mínimo: login, denegaciones, consultas) |
| Separar "usar conexión" de "ver credenciales" | Permisos distintos; las credenciales ya son referencias a secretos (ADR 0004) | Con las conexiones (G3) |
| Linaje con declarado, observado e inferido | CSMIR da el declarado; DuckDB, al analizar los modelos SQL, da el observado (ADR 0005 §5); se exporta a OpenLineage | G3–G4 |
| Explicar cualquier número (§1, §12.1) | Certificado del plan (#46): reglas aplicadas, grano, políticas, redondeo, SQL efectivo e instantánea de datos | G2 (certificado), G4 (UI) |
| Pruebas de oro entre canales (§7) | El mismo KPI con el mismo contexto en API, explorador, dashboard e IA, comparado con el oráculo del corpus | Cada canal al llegar |
| Contrato de visualización abstracto (§9) | `VisualSpec` (ADR 0008 §5) con adaptador ECharts; sin licencias comerciales | G4 |
| Objetivo WCAG 2.2 AA (§14) | Se añade a AGENTS.md | Ahora |
| DoD (§16): evento de auditoría y pruebas negativas de autorización | Se añade a AGENTS.md cuando exista el punto de autorización | G2 |
| No lanzar SQL libre, cachés ni exportaciones antes de sus controles (§15) | Ya es así; queda escrito en §3 | Ahora |

## 5. Qué adaptamos o no adoptamos, y por qué

- **Python/Polars en transformaciones (§6, §8):** ADR 0001, decisión del propietario: un solo motor en el núcleo y Polars solo en un futuro laboratorio Python. El blueprint lo deja como "decisión abierta", y la mantenemos cerrada: dos motores son dos semánticas que probar.
- **SQLMesh (§6):** ADR 0007: construcción propia al estilo SQLMesh (ADR 0005), sin depender de él.
- **Orden de fases (§15):** su F0 trae organizaciones, permisos y auditoría al principio. Nuestro G0–G1 los dejó para G2 porque solo había datos sintéticos, y G2 es justo esa fase. Se adapta adelantando lo mínimo a G2 (§7, punto B), no reordenando lo ya hecho.
- **"No reutilizar proyectos anteriores" (§0.1):** SEF y las referencias de Atlas BI se integraron con aprobación expresa del propietario (ADR 0006 y 0007).
- **API Gateway/BFF y microservicios (§13):** de acuerdo con el propio blueprint, un solo servidor modular (ADR 0008); se separan procesos solo con un motivo medido.

## 6. Mejoras propias que complementan el blueprint

1. **Certificado de corrección como base de "Explica este KPI" y del depurador semántico** (§12.1–2). El planificador de SEF (#46) emite, por consulta, qué reglas aplicó: grano, *fanout*, alineación de varios hechos, políticas, selección de periodo y redondeo. La explicación sale del motor, no de un texto escrito a mano, así que no puede contradecir al número.
2. **Análisis reproducible sin copiar datos** (§12.6). Consulta = SQIR canónica + huella CSMIR + huella de la decisión de política + **instantánea de DuckLake** (viaje en el tiempo). Reejecutarla da el mismo número; los datos no se duplican.
3. **Grafo de impacto con dependencias tipadas** (§12.4). CSMIR ya relaciona métricas, medidas, campos, dimensiones, relaciones y políticas. Al sumar el DAG de modelos SQL (G3) y los dashboards (G4) se obtiene el impacto de cambiar una columna antes de aplicarlo, dentro del "plan" de cambios de ADR 0005.
4. **Caché con clave semántica e invalidación exacta.** La clave incluye la instantánea de DuckLake: un commit nuevo invalida exactamente lo que cambió, sin TTL heurísticos, y la huella de política impide servir a B lo calculado para A.
5. **Simulador de políticas con análisis formal** (§12.3). Si el ADR de autorización elige un motor de políticas analizable (candidato: Cedar, §8), se puede probar antes de publicar que una política no amplía accesos, y calcular el acceso efectivo de un usuario sin consultar datos.
6. **IA que produce consultas semánticas, no SQL.** El asistente genera SQIR; el motor valida grano, *fanout* y políticas y devuelve el certificado. Una inyección de *prompt* no puede saltarse políticas, porque la IA no ejecuta nada fuera del plan.
7. **Portabilidad por estándar.** Ossie como formato de entrada y salida (parte 4 de #48), con sus conversores hacia Cube, dbt, Power BI, Tableau y otros.
8. **Identificadores que sobreviven al renombrado** (#67). El nombre visible queda separado del identificador estable, como el `lineageTag` de Power BI, para que renombrar nunca rompa dashboards, permisos ni cachés.

## 7. Decisiones que se proponen al propietario

- **A. Punto único de aplicación de políticas** (§3), como regla de arquitectura en AGENTS.md y en un ADR.
- **B. Adelantar a G2 lo mínimo de seguridad del blueprint (F0):**
  - organización y espacios de trabajo mínimos;
  - grupos y roles;
  - motor de autorización con denegación por defecto;
  - auditoría mínima;
  - federación OIDC, ya prevista en `auth.rs`.

  Todo esto es lo que hace verificable el criterio de G2: "usuarios con permisos distintos no cruzan filas, caché ni exportaciones".
- **C. Objetivo WCAG 2.2 AA** en los flujos principales.

## 8. Siguientes PR verticales (§17.3) y ADR pendientes (§17.4)

| # | Entrega demostrable | Depende de |
|---|---|---|
| 1 | Pantalla **Modelos** (#65) y cierre de Ossie → CSMIR (3b de #48) | En curso |
| 2 | **Consultar un modelo:** SQIR y endpoint de consulta semántica, con resultado exacto contra el oráculo del corpus. En Explorar, elegir métricas y dimensiones. | #44, #45 (núcleo) y #47 (DuckDB) |
| 3 | **Autorización:** ADR, punto único, grupos y roles mínimos, denegación por defecto y RLS del modelo aplicada. Demo: dos usuarios ven filas distintas del mismo KPI; batería de aislamiento. | Punto 2 y decisión B |
| 4 | **Auditoría mínima:** eventos de login, denegaciones y consultas con encadenamiento por hash, y vista de solo lectura para el auditor | Punto 3 |
| 5 | **Explica este KPI v1:** definición, certificado, SQL efectivo e instantánea de datos de cualquier cifra del explorador | #46 y punto 2 |

**ADR pendientes:**
- **Autorización.** Candidatos con código abierto que se leerá antes de decidir (AGENTS.md):
  - Cedar (`cedar-policy`, Rust, Apache-2.0, embebible y analizable);
  - OpenFGA (Zanzibar, servicio en Go);
  - Casbin (`casbin-rs`);
  - OPA/Rego (servicio en Go).
- **Organizaciones y espacios frente al proyecto como archivos** (ADR 0003): un espacio de trabajo agrupa uno o varios proyectos Git.
- **Contrato de consulta semántica:** SQIR (#44), su API versionada y el SQL sobre métricas.

**Recorrido funcional de referencia** (§17.5): iniciar sesión → elegir espacio → registrar fuente (ADR 0009, "consultar sin copiar") → consulta semántica autorizada → guardar gráfico → publicar dashboard → ver su evento de auditoría. Es la prueba e2e que cerrará G2 y abrirá G4.
