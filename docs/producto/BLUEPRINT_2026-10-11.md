# Blueprint de producto y arquitectura — Plataforma BI empresarial de nueva generación

> Documento del propietario (2026-10-11), conservado tal como lo entregó. Cómo se incorpora a este repositorio, qué se adopta, qué se adapta y qué no: [`blueprint-incorporacion.md`](blueprint-incorporacion.md). Donde una decisión vigente (`docs/decisiones/`) lo contradiga, prevalece la decisión hasta que un ADR la cambie.

**Versión:** 1.0 · **Fecha:** 2026-10-11 · **Destinatario:** Claude Code y equipo de ingeniería  
**Estado:** especificación de producto propuesta; no representa funcionalidades ya implementadas.

> **Mandato:** construir una plataforma BI de clase empresarial, abierta, extensible y usable, que adopte las mejores ideas de la industria sin copiar sus restricciones. Este documento es agnóstico al nombre, repositorio y formato de archivo del producto. No presupone que exista una implementación previa.

## 0. Instrucciones operativas para Claude Code

1. Inspeccionar primero el repositorio **actual** y sus instrucciones; no reutilizar repositorios, historia, código ni formatos de proyectos anteriores sin autorización explícita.
2. Crear un inventario `implementado / parcial / ausente / bloqueado` con evidencia (rutas, tests y demostraciones). No declarar nada terminado solo porque existe una interfaz.
3. Traducir esta especificación en épicas pequeñas, verticales y verificables. Proponer orden y dependencias antes de cambios disruptivos.
4. Preferir estándares abiertos y proyectos open source mantenidos. Decisiones tecnológicas mediante ADR breve con alternativas, coste operativo, riesgos y benchmark cuando corresponda.
5. No inventar un lenguaje de métricas o expresiones si SQL, una representación intermedia tipada o un estándar existente cubren el caso. Permitir constructor visual y acceso experto.
6. Proteger la seguridad en el servidor y en el motor de ejecución; jamás confiar en ocultar controles del frontend.
7. Todo cambio debe incluir pruebas de autorización, aislamiento, regresión y observabilidad apropiadas. Entregar funcionalidad demostrable antes que documentación extensa.
8. No ejecutar eliminaciones irreversibles, cambios de permisos productivos ni migraciones destructivas sin aprobación humana.
9. Si trabajan varios agentes, utilizar ramas/worktrees independientes y PRs pequeños, con propietario explícito por tarea; nunca editar simultáneamente el mismo árbol de trabajo.
10. No asumir que una capacidad de un competidor existe en todas sus ediciones o licencias: validar con documentación oficial antes de afirmar paridad.

## 1. Visión y principios irrenunciables

**Visión:** una experiencia que combine la exploración visual de Tableau, el modelado de Power BI, el gobierno de Looker, la búsqueda asociativa de Qlik, el SQL abierto de Superset, la observabilidad de Grafana, la sencillez de Metabase y la disciplina de métricas de Lightdash, con capacidades propias de inteligencia asistida y trazabilidad completa.

**Principios:**
- **Dos puertas de entrada:** usuarios de negocio sin SQL y especialistas con SQL avanzado; ambos trabajan sobre los mismos recursos gobernados.
- **SQL-first, no SQL-only:** consultas SQL nativas donde tengan sentido, diseñador visual y capa semántica; expresiones de datos tipadas y compilables, sin DSL propietario obligatorio.
- **Separación de control plane y data plane:** metadatos, permisos y colaboración separados de ejecución, credenciales y almacenamiento analítico.
- **Semántica única:** una métrica certificada conserva significado en dashboard, exploración, API, alerta, exportación y asistente IA.
- **Seguridad por defecto:** denegar salvo concesión explícita, mínimo privilegio, políticas consistentes en todos los canales.
- **Explicabilidad:** cualquier número debe poder explicar definición, filtros, periodo, procedencia, frescura y consulta efectiva.
- **Portabilidad:** importación/exportación de definiciones, APIs estables y formatos abiertos; evitar lock-in deliberado.
- **Rendimiento medible:** presupuestos de latencia, perfiles de consulta, caché coherente y benchmarks reproducibles.
- **Accesibilidad y colaboración:** teclado, lectores de pantalla, internacionalización y uso multiusuario real.

## 2. Referencias de la industria: qué rescatar y cómo superarlo

Estas son **inspiraciones de diseño**, no una afirmación de paridad exacta de producto, edición o fecha.

| Referente | Idea valiosa | Nuestra evolución propuesta |
|---|---|---|
| Power BI | Modelo tabular, relaciones, medidas, interacción entre visuales | Semántica reutilizable sin encerrar al usuario en un lenguaje propietario; explicabilidad del contexto de filtros |
| Tableau | Exploración visual, composición, análisis interactivo | Diseño libre con layout responsivo y especificaciones exportables |
| Looker | Métricas gobernadas y reutilización | Definiciones semánticas versionadas, probadas y accesibles vía SQL/API/visual |
| Qlik Sense | Exploración asociativa y descubrimiento | Exploración de selecciones, exclusiones y relaciones sin duplicar innecesariamente los datos |
| Apache Superset | SQL Lab, dashboards y ecosistema abierto | SQL editor de primera clase con gobierno uniforme y editor visual avanzado |
| Grafana | Paneles, alertas, observabilidad y variables | Unificar BI histórico y monitoreo operativo con límites claros entre cargas |
| Metabase | Experiencia de autoservicio amigable | Constructor guiado que escala a modelos y métricas complejas |
| Lightdash | Métricas gobernadas cercanas al modelado de datos | Métricas certificadas independientes de una sola herramienta de transformación |
| Rill | Exploración rápida sobre datos analíticos | Experiencia instantánea, filtros ricos y vistas que se puedan convertir en dashboards |
| ThoughtSpot y BI con IA | Preguntas en lenguaje natural | Respuestas verificables con SQL visible, permisos aplicados y evidencia de resultados |

**Innovaciones propias a investigar y validar:** grafo universal de dependencias (datos + métricas + dashboards + permisos + agentes), depurador del contexto semántico, simulador de políticas, pruebas de dashboards como código, trazabilidad causal de KPI y asistente que genera análisis reproducibles. No declarar estas ideas únicas del mercado sin una revisión competitiva específica.

## 3. Personas y modos de uso

- **Consumidor:** lee dashboards, filtra, comenta y recibe alertas.
- **Analista:** explora, formula hipótesis, construye consultas y comparte hallazgos.
- **Desarrollador BI:** diseña datasets, métricas, visuales, navegación y publicaciones.
- **Modelador semántico:** gobierna relaciones, cálculos, dimensiones y reglas de tiempo.
- **Ingeniero de datos:** conecta fuentes, ingesta, transforma y monitoriza pipelines.
- **Data Steward:** certifica definiciones, calidad, propiedad y clasificación.
- **Administrador de organización:** administra espacios, equipos y políticas delegadas.
- **Administrador de plataforma:** configura infraestructura, proveedores de identidad y operaciones.
- **Auditor:** investiga eventos y evidencia sin modificarla.
- **Cuenta de servicio / agente IA:** identidad no humana con alcance, caducidad y trazabilidad.

Los perfiles son **plantillas**, no permisos codificados de manera rígida.

## 4. Identidad, autorización y seguridad

### 4.1. Modelo de autorización

Implementar evaluación centralizada de `sujeto + acción + recurso + contexto -> decisión + restricciones + motivo`. Soportar RBAC de base, atributos contextuales (ABAC) y relaciones de pertenencia/propiedad (ReBAC) según necesidad. Reglas por organización, workspace, proyecto y recurso. Documentar precedencia de denegaciones y herencia. Separar administración de políticas de su evaluación.

**Recursos:** organización, workspace, conexión, secreto, fuente, pipeline, tabla/dataset, columna, modelo, métrica, consulta, dashboard, visual, alerta, reporte, exportación, API, token y evento de auditoría.

**Acciones mínimas:** `view`, `query`, `create`, `edit`, `delete`, `publish`, `share`, `export`, `execute`, `schedule`, `approve`, `manage_access`, `manage_credentials`, `audit`.

### 4.2. Seguridad de datos

- RLS por identidad/grupo/atributo; CLS, enmascaramiento, políticas de exportación y clasificación de datos.
- Políticas aplicadas a SQL libre, consultas semánticas, APIs, cachés, alertas, IA, exportaciones y resultados embebidos. Si una vía no puede hacer cumplir la política, bloquearla o aislarla.
- Separar `usar conexión` de `ver/editar credenciales`; secretos cifrados y rotables; nunca incluirlos en logs.
- Cuentas de servicio y tokens de alcance mínimo, con expiración, rotación y revocación.
- SSO/OIDC inicialmente; SAML, MFA/SCIM según clientes objetivo; sesiones y revocación auditables.
- Protección frente a SQL no autorizado: acceso de solo lectura por defecto, límites de recursos/tiempo, aislamiento, allowlists de funciones/extensiones y control de acceso a archivos/red según motor.
- Multi-tenancy: aislamiento probado de metadatos, almacenamiento, caché, ejecución, índices de búsqueda, logs y claves; evitar filtraciones entre organizaciones.
- Cifrado en tránsito y reposo, copias de seguridad, restauración probada y política de retención.

**Pruebas obligatorias:** usuario A no puede obtener datos B mediante SQL, API, caché, exportación, URL directa, alertas, IA ni recursos compartidos; permisos revocados tienen efecto definido y verificable.

## 5. Trazabilidad integral, linaje, auditoría y observabilidad

### 5.1. Modelo de eventos

Evento mínimo: `event_id`, `occurred_at` (UTC), `tenant_id`, `actor_id`, `actor_type`, `session_id`, `action`, `resource_type`, `resource_id`, `result`, `request_id`, `trace_id`, `policy_decision_id`, `source`, `metadata_redacted`, `schema_version`.

Registrar autenticación, autorización (incluidos denegados), cambios de roles/políticas, accesos a datos, consultas, exportaciones, publicaciones, cambios de métricas, ejecuciones de pipelines, acciones de agentes y restauraciones. Política explícita para volumen, muestreo, retención y privacidad; eventos críticos de seguridad no deben depender de muestreo aleatorio.

### 5.2. Linaje

- Linaje de **fuente -> ingesta -> transformación -> dataset -> modelo/métrica -> visual/dashboard -> alerta/API/IA**.
- Nivel dataset obligatorio; nivel columna incremental cuando el parser/engine pueda demostrarlo.
- Grafo navegable hacia arriba y abajo; análisis de impacto previo a borrar o modificar.
- Versionar dependencias y distinguir linaje declarado, observado e inferido (con nivel de confianza).
- Cada KPI: fórmula, versión, unidad, granularidad, calendario, filtros, procedencia, frescura, propietario y consulta de cálculo.

### 5.3. Versionado y auditoría

Versionar definiciones de modelos, métricas, consultas y dashboards; diffs legibles, autor, motivo, aprobación, publicación y rollback cuando sea compatible con el estado de datos. Registro de auditoría con integridad verificable, acceso restringido y exportación para cumplimiento.

### 5.4. Observabilidad

OpenTelemetry para trazas/métricas/logs donde aplique; correlación UI -> API -> autorización -> planificación semántica -> SQL -> motor -> caché -> respuesta. SLO de latencia, errores, frescura y disponibilidad. Perfil de consultas (tiempo de planificación/ejecución, filas, memoria, caché) sin revelar información sensible.

## 6. Datos, conectividad e ingeniería

- Conectores extensibles a SQL, archivos y objetos: PostgreSQL, DuckDB, Parquet/Arrow, ClickHouse y otros según demanda; contratos de capacidades y autenticación.
- Modos federado/live, importado/materializado e híbrido, sin prometer equivalencia de rendimiento.
- Ingesta incremental, CDC cuando exista soporte, cargas completas, deduplicación, idempotencia, validación de esquema y recuperación.
- Transformaciones SQL y, cuando aporte valor, Python/Polars; separación entre orquestación y motor de cálculo.
- Calidad de datos: tests de unicidad, nulos, integridad, rangos, frescura, anomalías y contratos de esquema.
- Catálogo técnico: tipos, estadísticas, cardinalidad, descripciones, propietario, clasificación y compatibilidad.
- **Decisión abierta:** evaluar DuckDB, DuckLake con catálogo PostgreSQL, ADBC/Arrow, Polars, SQLMesh y alternativas por responsabilidades concretas; no asumir que todos son obligatorios ni que compiten por la misma capa.
- Caché con claves que incluyan identidad/política, versión semántica, parámetros y versión/frescura de datos; invalidación explícita.

## 7. Motor semántico y métricas

**Objetivo:** definiciones consistentes y expresivas, con rendimiento y sin dependencia obligatoria de un DSL propietario.

Capacidades: entidades, relaciones y cardinalidades, medidas aditivas/semiaditivas/no aditivas, dimensiones, jerarquías, calendarios, periodos comparativos, cohortes, acumulados, porcentajes de total, filtros de contexto, métricas derivadas, métricas no aditivas, seguridad, unidades y formatos.

Diseñar representación intermedia tipada (IR/AST) compilable a SQL del motor elegido; el constructor visual y la API experta generan la misma IR. Los cálculos avanzados pueden usar SQL nativo con reglas explícitas de portabilidad. Semántica y políticas evaluadas antes de ejecutar.

**Pruebas de oro:** un mismo KPI produce el mismo resultado en dashboard, explorador, API y asistente bajo idéntico contexto; pruebas de filtros cruzados, jerarquías, totales, fechas y RLS. Definir comportamiento ante joins que multiplican filas y granularidades incompatibles.

## 8. Exploración y editor de consultas

- Editor SQL con autocompletado, catálogo, parámetros, formato, historial, planes/perfiles, resultados paginados, guardado y permisos.
- Explorador sin código: seleccionar medidas/dimensiones, agrupar, filtrar, ordenar, pivote, drill-down y drill-through.
- Vista de SQL generado y explicación de transformaciones; distinguir SQL editable del generado.
- Recomendación de visualizaciones según semántica, forma y cardinalidad de datos; siempre anulable por el usuario.
- Transformaciones con Polars o expresiones SQL solo cuando la semántica y el coste estén claros; evitar rutas de ejecución divergentes sin contrato.
- Sandbox de consultas, límites de coste, cancelación y manejo de resultados grandes.

## 9. Visualización y dashboards

- Biblioteca de gráficos: KPI, barras, líneas, áreas, dispersión, histogramas, tablas, matrices/pivots, mapas, heatmaps, treemaps, waterfall, funnel, sankey, boxplots y gráficos especializados extensibles.
- Configuración explícita de ejes, agregaciones, series, tooltips, formatos, colores, leyendas, umbrales, anotaciones y accesibilidad.
- Layout libre y por cuadrícula, responsivo, breakpoints, temas, componentes reutilizables, plantillas y navegación multipágina.
- Interacciones: cross-filter, cross-highlight, drill-down/up, drill-through, brush, parámetros, bookmarks, tooltips enriquecidos y sincronización de filtros.
- Tablas empresariales: virtualización, ordenamiento, agrupación, subtotales, congelación, búsqueda, formatos condicionales y exportación gobernada.
- Geoespacial: puntos, polígonos, capas, clustering, selección espacial y RLS geográfico.
- Rendimiento: carga diferida, consultas paralelas con límites, caché, cancelación y degradación elegante.
- Versionado, publicación a entornos, comentarios y permisos por recurso.
- **Evitar acoplamiento exclusivo a ECharts/Highcharts:** contrato abstracto de visualización y adaptadores. Revisar licencias antes de adoptar librerías comerciales.

## 10. Colaboración, distribución y ciclo de vida

Workspaces, equipos, propietarios, comentarios anclados a visual/filtro/versión, menciones, notificaciones, revisión y aprobación, historial, colecciones, favoritos y búsqueda. Enlaces compartidos con expiración y permisos explícitos. Programación de reportes, suscripciones, alertas basadas en métricas, exportación PDF/CSV/XLSX con marcas de agua y políticas. Embedding seguro mediante tokens de corto plazo y aislamiento de tenant. Entornos desarrollo/pruebas/producción y promoción con revisión.

## 11. Inteligencia artificial gobernada

- Preguntas en lenguaje natural -> plan semántico/SQL visible -> ejecución autorizada -> respuesta con evidencia.
- Asistente de construcción de dashboards, explicación de KPIs, análisis de variaciones y sugerencias de gráficos.
- Agentes con permisos delegados, límites de gasto, acciones permitidas, aprobación humana para cambios destructivos o publicaciones y registro de cada paso.
- Respuestas con definiciones, fuentes, filtros, rango temporal, frescura, confianza y enlaces a consultas reproducibles.
- Protección contra prompt injection en metadatos/documentos/resultados; nunca permitir que el modelo omita políticas de seguridad.
- Evaluaciones reproducibles de exactitud numérica, SQL generado, fuga de datos, latencia, coste y tasa de abstención.
- **No** comercializar resultados probabilísticos como hechos certificados sin verificación contra el motor.

## 12. Capacidades diferenciales a prototipar

1. **Explain this KPI:** desde cualquier cifra, abrir definición, linaje, filtros, SQL, ejecución, versión y comparativa histórica.
2. **Semantic Debugger:** inspeccionar paso a paso cómo los filtros, joins y contextos cambian una medida.
3. **Policy Simulator:** previsualizar el acceso efectivo de un usuario/grupo antes de publicar un permiso, sin exponer los datos reales.
4. **Impact Graph:** estimar qué dashboards, alertas y agentes se rompen si cambia una columna, métrica o permiso.
5. **Dashboard Tests:** pruebas automáticas de números, filtros, navegación, rendimiento, accesibilidad y capturas visuales.
6. **Reproducible Analysis:** guardar consulta + parámetros + versiones de semántica + referencia de snapshot/frescura + contexto de seguridad, sin copiar datos sensibles por defecto.
7. **Universal Metric Contract:** exponer métricas certificadas a UI, API, notebooks y agentes mediante contratos versionados.
8. **Exploración guiada por anomalías:** detectar desviaciones, explicar hipótesis comprobables y permitir navegar al dato fuente.

Cada propuesta exige prototipo, benchmark y validación con usuarios antes de elevarse a requisito general.

## 13. Arquitectura lógica de referencia

```text
Web UI / Embedded UI / API / Agents
                 |
         API Gateway / BFF
                 |
   Identity + Policy Decision/Enforcement
                 |
    Catalog / Workspace / Metadata
       |         |         |
  Semantic IR  Lineage  Audit/Event Bus
       |
  Query Planner / Cost Guard / Cache
       |
    Engine Adapters / Data Access
       |
 DuckDB / PostgreSQL / ClickHouse / files / ...

Pipelines + Scheduler + Quality -> Catalog/Lineage/Events
Telemetry -> Traces / Metrics / Logs / Alerting
```

**Fronteras:** el control plane no contiene credenciales en claro; el data plane valida políticas antes de ejecutar; el navegador no accede directamente a secretos ni conexiones privilegiadas. Diseñar contratos API versionados y eventos idempotentes. No introducir microservicios por moda: comenzar modular y separar procesos solo cuando aislamiento, escalado u operación lo justifiquen.

## 14. Requisitos no funcionales y criterios medibles

- **Seguridad:** cero accesos cruzados en suite de aislamiento; cobertura de acciones críticas y denegaciones.
- **Correctitud:** pruebas de equivalencia semántica y datasets de referencia; cifras auditables.
- **Rendimiento:** definir p50/p95 por tipo de consulta y tamaño de dataset en hardware conocido; no inventar metas universales sin benchmark.
- **Fiabilidad:** reintentos idempotentes, recuperación, backups y restore drills.
- **Accesibilidad:** objetivo WCAG 2.2 AA para flujos principales.
- **Interoperabilidad:** APIs documentadas, formatos exportables, SQL estándar cuando viable.
- **Operación:** telemetría correlacionada, runbooks mínimos, migraciones reversibles donde sea posible.
- **Privacidad:** minimización de PII en logs, trazas, resultados cacheados y prompts de IA.

## 15. Hoja de ruta propuesta (vertical slices)

| Fase | Entrega usable | Prueba de aceptación |
|---|---|---|
| F0 — Fundamentos | Login, organizaciones/workspaces, usuarios/grupos, permisos, auditoría mínima | Dos usuarios con permisos distintos; accesos negados desde UI y API |
| F1 — Primer flujo completo | Conexión -> dataset -> SQL -> gráfico -> dashboard publicado | Dashboard real con filtros, consulta reproducible y trazas |
| F2 — Semántica | Métricas, dimensiones, relaciones, RLS y explorador visual | Mismo KPI en SQL/API/dashboard con resultado idéntico |
| F3 — Producto BI | Editor de dashboards avanzado, tablas, drill, colaboración, exportación | Flujo de creación/publicación y permisos end-to-end |
| F4 — Gobierno | Linaje, catálogo, calidad, impacto, versionado y aprobación | Cambio de columna identifica dependencias afectadas |
| F5 — Enterprise | SSO, service accounts, multi-tenant robusto, alertas, observabilidad | Suite de aislamiento, recuperación y pruebas de carga |
| F6 — Diferenciación | IA gobernada, semantic debugger, policy simulator, pruebas de dashboards | Demostraciones evaluadas con datasets y métricas de precisión |

**Nota:** dependencias críticas de seguridad se adelantan aunque su interfaz completa llegue después. No lanzar SQL libre, cachés compartidas ni exportaciones antes de sus controles correspondientes.

## 16. Definición de terminado (DoD) para cada funcionalidad

- Caso de uso visible y demostrable, no solo endpoints o mocks.
- Contratos tipados, validación de entradas y manejo explícito de errores.
- Autorización aplicada en servidor y pruebas negativas.
- Evento de auditoría cuando la acción lo amerita.
- Telemetría y correlación para fallos/latencia.
- Pruebas unitarias, integración y al menos un E2E para flujo crítico.
- Documentación mínima de operación, limitaciones y migraciones.
- Accesibilidad básica y comportamiento responsivo en UI.
- Benchmark antes/después para cambios de rendimiento.
- Sin secretos en código, logs ni artefactos de prueba.

## 17. Primer entregable solicitado a Claude Code

**No intentar implementar este blueprint entero de una sola vez.** Entregar primero:

1. Matriz de capacidades contra el repositorio actual: `implementado`, `parcial`, `ausente`, `bloqueado`, con evidencia.
2. Mapa de módulos y fronteras de seguridad; identificar rutas donde una consulta podría evadir políticas.
3. Backlog priorizado de los **primeros 5 PRs verticales**, cada uno con archivos previstos, pruebas y demo.
4. ADR corto para motor de autorización, modelo de metadatos y contrato de consultas/semántica, con alternativas open source.
5. Propuesta de primer recorrido funcional: iniciar sesión -> crear workspace -> registrar fuente -> ejecutar consulta autorizada -> guardar gráfico -> publicar dashboard -> inspeccionar evento de auditoría.

**Criterio rector:** producto funcionando, seguridad demostrable, métricas correctas, experiencia sobresaliente. No confundir complejidad arquitectónica con calidad.
