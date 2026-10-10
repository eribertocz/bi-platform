# M09 — Visualization Core & Renderer Adapters

**Plataforma BI nueva · edición normativa RC1 · 2026-10-10**

**Reemplaza las decisiones de producto del informe histórico:** `../investigaciones_historicas/HISTORICO_NO_NORMATIVO_SQLVIZ_Modulo_09_Visualization_Core_Renderer_Adapters_Rill_Lightdash_Grafana_2026-10-10.md`. El análisis de proyectos externos se mantiene como evidencia histórica.

## 0. Estatus, límites y decisiones transversales obligatorias

**Estatus:** edición normativa de arquitectura `RC1` para una plataforma BI NUEVA, aún sin nombre comercial; **no es código implementado ni verificado**. La investigación original de Rill/Lightdash/Cube/Grafana/SQLMesh está preservada aparte en `investigaciones_historicas/` y sirve como evidencia comparativa, **jamás como instrucción de implementación**. Esta edición sustituye la recomendación de producto de la antigua investigación, pero no borra ni falsifica sus hallazgos externos.

**Precedencia:** `DECISIONES_TRANSVERSALES.md` → contratos y ADR aprobados → estos M01–M09 → M10–M17, sujetos a la misma precedencia → investigación histórica. Las contradicciones se registran y resuelven con ADR; un agente no escogerá a escondidas la versión que le convenga.

**Topología única:** Svelte 5/SvelteKit 3/TypeScript (UI) → HTTP API Rust/Axum → autorización y contratos → motor semántico/serving → `duckdb-rs` embebido y Arrow → DuckLake con **catálogo PostgreSQL** + ficheros **Parquet** bajo `DATA_PATH`. `Polars Rust` es auxiliar cuando hay valor medido. `ADBC` sólo si existe driver interoperable validado. Quack es **adaptador remoto optativo**, nunca condición de arranque.

**Fuentes de verdad:** `PostgreSQL BI` gestiona usuarios, proyectos, definiciones/revisiones, ejecuciones y políticas; `PostgreSQL DuckLake catalog` gestiona snapshots/archivos/transacciones del lago; Parquet conserva bytes de los datos; el estado UI es efímero hasta que un comando explícito lo guarda. Pueden coexistir en un clúster PostgreSQL, pero deben ser esquemas/DB, migraciones, credenciales y backups lógicamente independientes. **No usar `.sqlviz` ni SQLite como fuente canónica de producto** y no agregar el legado como dependencia/migración del MVP.

**Confianza y calidad:** toda ruta (SQL, Explore, Pivot, Canvas, API, exportación, IA) aplica identidad y políticas M11 del lado servidor. Consultas SQL arbitrarias no comparten automáticamente los permisos seguros del motor semántico: requieren perfil, aislamiento, validación, límites y tests negativos propios. Preservar `DECIMAL`, `INT64`, `TIMESTAMPTZ` y `NULL`; distinguir datos y metadatos; no inventar optimizaciones sin benchmarks.

**Normas de ingeniería:** componentes modulares sin microservicios innecesarios, interfaces pequeñas, TypeScript estricto, Rust `fmt/clippy`, SQL parametrizado donde corresponda, pruebas unitarias/contrato/integración/E2E; errores tipados; migraciones reversibles o forward-only justificadas; observabilidad sin PII; SemVer sólo al liberar. Cada microentrega sigue Plan → Implementación → Test → Informe conciso → Git.

## 1. Responsabilidad y auditoría de evidencias

**Dueño:** Viz Contracts / ECharts Adapter.

**Hallazgo de investigación conservado:** Rill ofrece configuración de componentes; Lightdash formatos/tipos; Grafana registro de plugins. ECharts aporta renderer gráfico general; ninguno reemplaza un `VizSpec` propio que preserve tipos, capacidades y revisiones.

**Fuera de alcance:** interfaces de usuario o APIs ajenas a esta capa, aunque la capa proporcione contratos para ellas; no sustituye a M10–M17.

## 2. Decisiones normativas y opciones no seleccionadas

1. `VizSpec` neutral describe intención, encodings, formato, estilo y eventos; no persiste `echarts.Option` como contrato de producto.
2. ECharts principal, activado perezosamente en navegador; su SSR SVG es optativo para imágenes/reportes y no promete interactividad completa.
3. Highcharts es adaptador opcional condicionado a licencia y demanda real; no depender de él en el producto base.
4. Un `CapabilityRegistry` valida chart data/escala/eventos/renderer y fallback antes de montar componentes.
5. El frontend calcula sólo presentación (no SUM, DISTINCT ni métricas gobernadas), aplica transformaciones ligeras trazables y gestiona `mount/update/resize/destroy`.

**No elegir por defecto:** SQLVIZ como nombre/producto/código, `.sqlviz`, SQLite canónico, Quack como servidor obligatorio, microservicio por submódulo, reimplementación de lógica semántica o APIs de otras capas.

## 3. Contratos tipados independientes del transporte

`VizSpec { kind, encodings, axes, series, field_options, interactions, theme_ref, spec_version }`; `ChartData { schema, columnar_batches|small_rows, provenance }`; `RendererCapabilities { kinds, supports_svg, supports_interaction, limits }`; `VizEvent { type, semantic_field_ref, typed_value, origin_panel }`.

`VizSpec` se almacena en metadatos BI junto a revisión del panel o vista. No es un formato de intercambio de base de datos ni una ruta a Quack.

Los campos anteriores son **esquemas conceptuales**; los DTO definitivos deben establecerse en OpenAPI/JSON Schema con semántica `required/optional`, estados, `schema_version`, referencias opacas, errores tipados y golden fixtures. No son declaraciones Rust compiladas.

## 4. Flujo end-to-end y responsabilidades

1. Elegir visualización por intención y esquema de datos, generar propuesta de `VizSpec`.
2. Validar field refs, escalas, cardinalidad y adaptador requerido.
3. Solicitar datos analíticos reducidos a M05; preservar tipos y procedencia.
4. Cargar adaptador ECharts dinámicamente en client mount con destroy/cleanup correctos.
5. Emitir eventos semánticos tipados a Explore/Pivot/Canvas.
6. Exportar imagen accesible por vía autorizada con restricciones explícitas de SSR.

## 5. Seguridad, fiabilidad y fallos

- ECharts SVG SSR carece de algunas interacciones, no declarar SSR = hidratación completa.
- No insertar HTML en tooltips sin saneamiento; reducir XSS.
- Gran volumen no debe serializarse en opciones JS gigantes; priorizar agregados, ventanas y muestreo documentado.
- Adaptadores no deben filtrar opciones internas al contrato persistido.

**Invariantes comunes:** actor autorizado, `tenant_id` no confiable desde browser, secretos sólo en secret store, trazabilidad sin PII, presupuestos finitos, reintentos con idempotencia, no degradar silenciosamente una política de seguridad.

## 6. Librerías, dependencias y límites tecnológicos

- Lenguaje de lógica central: **Rust**, con módulos de dominio + traits pequeños (donde aporten sustitución real), `Result` y errores categorizados.
- Runtime analítico: **duckdb-rs** con DuckDB embebido M13. Polars Rust y ADBC como extensiones evaluadas, no rutas obligatorias.
- Persistencia: PostgreSQL BI para revisiones/configuración/runs; DuckLake catalog PostgreSQL para snapshots; Parquet en DATA_PATH para datos materiales.
- Transporte web: M12 — API HTTP/OpenAPI, JSON tipado, Arrow IPC según cardinalidad y SSE si hay progreso.
- SvelteKit/Svelte 5 + TypeScript: UI M17. Librerías visuales adicionales sólo si este módulo las requiere realmente y pasan spike/licencia/a11y.

## 7. Contratos de integración y ownership

**Módulos relacionados:** M04/M05/M06/M07/M08/M10/M11/M12/M17.

La integración atraviesa el API M12 (nunca llamando DuckDB directamente desde navegador); M11 autoriza y M15 observa. Los recursos tienen un solo dueño de escritura y pueden leerse por referencias versionadas/servicios tipados. Una implementación no puede copiar la lógica de otra capa para mejorar artificialmente un benchmark.

## 8. Pruebas de aceptación propuestas (aún no ejecutadas)

1. Un mismo VizSpec produce resultado coherente con diferentes themes.
2. Adaptador desmontado libera listeners/ResizeObserver.
3. DECIMAL formateado no cambia cifra exacta.
4. Tipo de gráfico incompatible devuelve error/fallback explicable.
5. Eventos de filtro cruzado transmiten valor tipado, no fragmentos de SQL.
6. SSR estático y gráfico interactivo declaran capacidades distintas.
7. Alternar panels no provoca fugas acumulativas.
8. Licencias de adaptadores se verifican en CI/release.

**Gate de implementación:** PR pequeño con contract tests y suite negativa cuando afecte seguridad, formateo/lint y evidencia de un slice real. Documentar mediciones sólo si se ejecutaron, nunca prometer latencia por intuición.

## 9. Orden mínimo de implementación

1. Acordar y revisar contratos DTO mínimos; golden fixture positiva y negativa.
2. Probar un caso feliz in-process con dependencia real (o test fake estrictamente diferenciado).
3. Cubrir un fallo real, concurrencia/retry si aplica, y permisos.
4. Conectar la UI sólo cuando la API sea tipada y probada; medir antes de optimizar.
5. Publicar una microtarea por PR con registro de tests y Git.

## 10. Compatibilidad histórica y decisión de cierre

El informe anterior se conserva íntegro como **investigación histórica NO NORMATIVA**, incluyendo las comparaciones de Rill, Lightdash, Cube, SQLMesh o Grafana. Las menciones a formatos SQLVIZ, Quack obligatorio, migración del repo SQLVIZ y SQLite como repositorio del producto NO se trasladan al nuevo proyecto. Si un algoritmo del informe anterior resulta útil se reespecifica y prueba bajo el contrato vigente.

**Estado final del documento:** `normalizado-documentalmente` para la baseline RC1. **No verificado en implementación.** Requiere ADR y CI para pasar de arquitectura a código; no afirmar rendimiento/HA ni seguridad por documento.
