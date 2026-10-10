# M01 — Sources & Ingestion

**Plataforma BI nueva · edición normativa RC1 · 2026-10-10**

**Reemplaza las decisiones de producto del informe histórico:** `../investigaciones_historicas/HISTORICO_NO_NORMATIVO_SQLVIZ_Modulo_01_Sources_e_Ingestion_Auditoria_Rill_Lightdash_2026-10-10.md`. El análisis de proyectos externos se mantiene como evidencia histórica.

## 0. Estatus, límites y decisiones transversales obligatorias

**Estatus:** edición normativa de arquitectura `RC1` para una plataforma BI NUEVA, aún sin nombre comercial; **no es código implementado ni verificado**. La investigación original de Rill/Lightdash/Cube/Grafana/SQLMesh está preservada aparte en `investigaciones_historicas/` y sirve como evidencia comparativa, **jamás como instrucción de implementación**. Esta edición sustituye la recomendación de producto de la antigua investigación, pero no borra ni falsifica sus hallazgos externos.

**Precedencia:** `DECISIONES_TRANSVERSALES.md` → contratos y ADR aprobados → estos M01–M09 → M10–M17, sujetos a la misma precedencia → investigación histórica. Las contradicciones se registran y resuelven con ADR; un agente no escogerá a escondidas la versión que le convenga.

**Topología única:** Svelte 5/SvelteKit 3/TypeScript (UI) → HTTP API Rust/Axum → autorización y contratos → motor semántico/serving → `duckdb-rs` embebido y Arrow → DuckLake con **catálogo PostgreSQL** + ficheros **Parquet** bajo `DATA_PATH`. `Polars Rust` es auxiliar cuando hay valor medido. `ADBC` sólo si existe driver interoperable validado. Quack es **adaptador remoto optativo**, nunca condición de arranque.

**Fuentes de verdad:** `PostgreSQL BI` gestiona usuarios, proyectos, definiciones/revisiones, ejecuciones y políticas; `PostgreSQL DuckLake catalog` gestiona snapshots/archivos/transacciones del lago; Parquet conserva bytes de los datos; el estado UI es efímero hasta que un comando explícito lo guarda. Pueden coexistir en un clúster PostgreSQL, pero deben ser esquemas/DB, migraciones, credenciales y backups lógicamente independientes. **No usar `.sqlviz` ni SQLite como fuente canónica de producto** y no agregar el legado como dependencia/migración del MVP.

**Confianza y calidad:** toda ruta (SQL, Explore, Pivot, Canvas, API, exportación, IA) aplica identidad y políticas M11 del lado servidor. Consultas SQL arbitrarias no comparten automáticamente los permisos seguros del motor semántico: requieren perfil, aislamiento, validación, límites y tests negativos propios. Preservar `DECIMAL`, `INT64`, `TIMESTAMPTZ` y `NULL`; distinguir datos y metadatos; no inventar optimizaciones sin benchmarks.

**Normas de ingeniería:** componentes modulares sin microservicios innecesarios, interfaces pequeñas, TypeScript estricto, Rust `fmt/clippy`, SQL parametrizado donde corresponda, pruebas unitarias/contrato/integración/E2E; errores tipados; migraciones reversibles o forward-only justificadas; observabilidad sin PII; SemVer sólo al liberar. Cada microentrega sigue Plan → Implementación → Test → Informe conciso → Git.

## 1. Responsabilidad y auditoría de evidencias

**Dueño:** Source Discovery / Data Connectivity.

**Hallazgo de investigación conservado:** Rill distingue conectores/model execution y reconciliación; Lightdash presupone un warehouse externo en lugar de ser un sistema ETL general. Conservar descubrimiento, capacidad por conector y reconciliación, no replicar su topología.

**Fuera de alcance:** interfaces de usuario o APIs ajenas a esta capa, aunque la capa proporcione contratos para ellas; no sustituye a M10–M17.

## 2. Decisiones normativas y opciones no seleccionadas

1. Una `SourceDefinition` referencia credenciales por `SecretRef`, nunca guarda contraseñas ni SQL interpolado en metadata pública.
2. Dos modalidades `ExternalReference` (sin copiar datos, lectura bajo permiso) y `ManagedSnapshot` (ingesta física con manifiesto/estado). No obligar a cargar todo en RAW.
3. La ingesta física usa manifiestos, checkpoints, versión de esquema y reintento idempotente; no genera tablas publicadas semánticas por su cuenta.
4. `M01` es dueño del descubrimiento y movimiento a staging; `M02` transforma y `M03` decide publicación y retención.
5. Prioridad MVP: Parquet/CSV y una fuente PostgreSQL read-only; otros conectores mediante pruebas de capacidades/seguridad; ADBC sólo cuando aporte un driver operativo.

**No elegir por defecto:** SQLVIZ como nombre/producto/código, `.sqlviz`, SQLite canónico, Quack como servidor obligatorio, microservicio por submódulo, reimplementación de lógica semántica o APIs de otras capas.

## 3. Contratos tipados independientes del transporte

`SourceDefinition { id, tenant_id, kind, config_ref, secret_ref, revision, capabilities, owner }`; `SourceSchema { fields, logical_types, observed_at, fingerprint }`; `IngestionPlan { source_ref, mode, target_candidate, projection, checkpoint_policy }`; `IngestRun { run_id, source_revision, phase, manifest_ref, rows, bytes, schema_fingerprint, retry_key }`.

`SourceRef` no es `DatasetRef`: origen y dataset publicado tienen propiedad y ciclo de vida diferentes. La selección del motor ocurre por capacidades (`ConnectorCapabilities`), no por si existe un servicio Quack.

Los campos anteriores son **esquemas conceptuales**; los DTO definitivos deben establecerse en OpenAPI/JSON Schema con semántica `required/optional`, estados, `schema_version`, referencias opacas, errores tipados y golden fixtures. No son declaraciones Rust compiladas.

## 4. Flujo end-to-end y responsabilidades

1. Autorizar `SourceRef` y `SecretRef` (M11).
2. Probar conectividad con tiempo límite y límites de descubrimiento; retornar campos tipados.
3. Para lectura directa, crear una referencia autorizada resoluble por M13, no copiar por defecto.
4. Para carga administrada, escribir archivos temporales y un manifiesto con checksum, schema y cursor/watermark.
5. Verificar calidad técnica; entregar `CandidateRef` a M02/M03; registrar checkpoint sólo después de commit aceptado.
6. Exponer estado/errores/reintentos por M12/M14; nunca declarar publicación desde ingesta.

## 5. Seguridad, fiabilidad y fallos

- Credenciales caducadas o revocadas: ejecución fallida sin exponer secretos.
- CSV parcial, encoding, nulos y columnas cambiantes: cuarentena del candidato; no sobrescribir versión estable.
- Watermark y cargas fuera de orden: idempotencia basada en identificador de origen, lote y hash de contenido.
- Política explícita de redacción en logs, URLs, errores y trazas.

**Invariantes comunes:** actor autorizado, `tenant_id` no confiable desde browser, secretos sólo en secret store, trazabilidad sin PII, presupuestos finitos, reintentos con idempotencia, no degradar silenciosamente una política de seguridad.

## 6. Librerías, dependencias y límites tecnológicos

- Lenguaje de lógica central: **Rust**, con módulos de dominio + traits pequeños (donde aporten sustitución real), `Result` y errores categorizados.
- Runtime analítico: **duckdb-rs** con DuckDB embebido M13. Polars Rust y ADBC como extensiones evaluadas, no rutas obligatorias.
- Persistencia: PostgreSQL BI para revisiones/configuración/runs; DuckLake catalog PostgreSQL para snapshots; Parquet en DATA_PATH para datos materiales.
- Transporte web: M12 — API HTTP/OpenAPI, JSON tipado, Arrow IPC según cardinalidad y SSE si hay progreso.
- SvelteKit/Svelte 5 + TypeScript: UI M17. Librerías visuales adicionales sólo si este módulo las requiere realmente y pasan spike/licencia/a11y.

## 7. Contratos de integración y ownership

**Módulos relacionados:** M02/M03/M10/M11/M12/M13/M14/M15.

La integración atraviesa el API M12 (nunca llamando DuckDB directamente desde navegador); M11 autoriza y M15 observa. Los recursos tienen un solo dueño de escritura y pueden leerse por referencias versionadas/servicios tipados. Una implementación no puede copiar la lógica de otra capa para mejorar artificialmente un benchmark.

## 8. Pruebas de aceptación propuestas (aún no ejecutadas)

1. Parquet externo accesible se descubre y se consulta sin copia obligatoria.
2. CSV con fila corrupta reporta línea/causa y no publica datos incompletos.
3. Reejecutar mismo lote con igual clave no duplica registros publicados.
4. Credenciales nunca salen por API ni trazas.
5. Schema drift incompatible falla con reporte.
6. PostgreSQL fuente sin permiso de escritura no intenta DDL.
7. Cancelación interrumpe cuando el driver lo soporta o informa estado real.
8. Checkpoint no avanza tras fallo de commit.

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
