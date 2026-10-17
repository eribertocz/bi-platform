# Referentes: aprender de las mejores herramientas de BI

**Regla (AGENTS.md):** antes de diseñar una capacidad nueva, se revisa cómo la resuelven las herramientas de referencia de esa capacidad, y el PR deja constancia de qué se toma, qué se descarta y por qué. Las correcciones y los cambios pequeños no lo necesitan.

## Método
1. **Elegir 2–4 referentes** de la matriz para la capacidad, no todos.
2. **Fuente primaria con versión fijada:** código en un tag o SHA para las herramientas abiertas; documentación oficial con fecha para las cerradas (Power BI, Tableau, Looker, Sigma, ThoughtSpot, Omni). Marcar `[CÓDIGO]` o `[DOCS]`; la documentación comercial no prueba qué hay en la edición abierta.
3. **Empezar por lo ya investigado** en `docs/arquitectura/` (Rill, Cube, Lightdash, Superset, Grafana, Ossie…) para no repetir trabajo ni gastar de más.
4. **Rescatar ideas, no copiar:** contratos propios, compatibles con estándares (Apache Ossie, OpenAPI, Arrow, OpenLineage). Nunca nombres, formatos ni código con licencia incompatible.
5. **Registrar:** lo transversal va en `docs/referentes/<herramienta>.md`; lo que afecta a la arquitectura, en un ADR (`docs/decisiones/`).

## Matriz: dónde mirar para cada capacidad

| Capacidad (módulo) | Referentes principales | Qué destaca, a verificar con la fuente al implementar |
|---|---|---|
| Fuentes e ingesta (M01) | Rill, Metabase, Superset, Power Query, Airbyte | Conexiones en vivo frente a importación; asistentes de conexión |
| Modelos y transformación (M02/M03) | dbt, SQLMesh, Rill models | DAG, incrementales, particiones, pruebas de datos, entornos dev/prod |
| Capa semántica y métricas (M04) | Looker (LookML), Cube, dbt Semantic Layer, Power BI (TMDL/DAX), Rill, Apache Ossie | Métricas reutilizables, joins declarados, medidas derivadas y de ventana, comparación de periodos, formatos |
| Aceleración de consultas (M05) | Cube (pre-aggregations), Power BI (aggregations, Direct Lake), Looker (aggregate awareness), Tableau (extracts/Hyper), Rill (rollups) | Enrutado automático a tablas pre-agregadas cuando la consulta encaja |
| Exploración (M06) | Tableau, Power BI, Rill Explore, Metabase (preguntas, X-rays), Omni, Sigma | Exploración sin código; leaderboards; drill; estado en la URL |
| Tabla dinámica (M07) | Power BI (matriz), Excel, Sigma, Tableau | Pivot con subtotales y totales exactos sobre grandes volúmenes |
| Dashboards (M08) | Power BI, Tableau, Grafana, Superset, Metabase, Rill Canvas, Evidence | Composición, filtros cruzados, variables, temas, dashboards como código |
| Visualización (M09) | Tableau (VizQL), Power BI, ECharts, Vega-Lite, Observable Plot | Gramática de gráficos y elección automática del gráfico adecuado |
| Seguridad por filas y campos (M11) | Power BI (RLS/OLS), Looker (access filters), Cube (security context), Rill (policies) | Atributos de usuario en las políticas; probar "ver como" otro usuario |
| APIs y embebido (M12) | Cube (REST, GraphQL, SQL API), Metabase/Superset/Power BI embedded, Rill APIs | SQL sobre métricas, embebido seguro con tokens firmados |
| Alertas e informes (M14) | Grafana (alerting), Metabase, Power BI, Rill | Condiciones sobre métricas, re-notificación, recuperación, envío programado |
| Observabilidad (M15) | Grafana, CubeStore metrics, Lightdash | Métricas por fase, colas, diagnóstico por consulta |
| IA y BI conversacional (M16) | Power BI Copilot, Tableau Pulse/Agent, Looker + Gemini, ThoughtSpot, Rill (AI, MCP), Metabase Metabot, Omni | Preguntas sobre la capa semántica (no SQL libre), instrucciones por modelo, MCP |
| Diseño y UX (M17) | Tableau, Power BI, Linear/Vercel (patrones de producto), Rill | Rapidez percibida, teclado, accesibilidad |
| BI como código y cuadernos (sin módulo numerado) | Evidence, Hex, Observable, Rill | Markdown con consultas, revisión en Git, vistas previas por rama |

## Fichas por herramienta
- [Rill](rill.md): primera ficha (v0.90.5).
- [Apache Ossie](ossie.md): estándar de intercambio semántico y lenguaje portable de expresiones (`OSSIE_SQL_2026`), base del motor semántico de G2.
- [Motor semántico](semantica.md): Cube, Malloy, MetricFlow, Rill (código) y Looker, Power BI, Tableau, Snowflake, Databricks (documentación), contrastados con el corpus de SEF.
- [Atlas BI](atlasbi.md): proyecto hermano del propietario (`E:\bi-platform`); ideas de filtros, cálculos de autoservicio, visualización, trabajos y mantenimiento del lago.

## Propuestas que parten de esta matriz
- [Propuesta de evolución analítica del motor semántico](../producto/propuestas/evolucion-analitica-motor-semantico.md): propuesta de dirección, no estado normativo ni ficha de una herramienta implementada. Compara siete referentes para contexto de filtros, nivel de detalle (LOD), grupos de cálculo, inteligencia temporal, composición multietapa y drill. Integrarla no aprueba implementar esas capacidades.
