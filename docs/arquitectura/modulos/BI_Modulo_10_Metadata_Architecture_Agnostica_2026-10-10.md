# Plataforma BI abierta — Módulo 10: Arquitectura de metadatos, catálogo y portabilidad

**Edición:** 1.0, 10 de octubre de 2026  
**Estado:** arquitectura propuesta para una plataforma BI nueva, todavía sin nombre ni implementación.  
**Alcance:** definición de recursos, metadatos de desarrollo y operación, versionado, repositorios, interoperabilidad, linaje, colaboración, seguridad y publicación.  
**No depende de:** un producto previo, una extensión de archivo propietaria, SQLite, PostgreSQL, Git, DuckDB, ECharts, un frontend o un motor semántico particular.  
**Metodología:** revisión localizada de código y contratos públicos; distinguir hechos comprobables, lecciones rescatables y propuestas nuevas. Los módulos M01–M09 sirven de contexto conceptual; este informe no pretende renombrarlos ni afirmar que ya estén implementados.

---

## 0. Decisión arquitectónica

**No elegir primero una base de datos ni un formato de proyecto.** Diseñar primero un modelo lógico versionado de recursos BI y sus APIs. Después ofrecer **repositorios de metadatos intercambiables** para diferentes escenarios.

1. **Definiciones declarativas y transportables:** Model, Source, SemanticModel, Metric, Explore, Pivot, Dashboard, VizSpec, Policy, Schedule y otros recursos definidos mediante contratos tipados y validables.
2. **Repositorio local:** directorio de archivos legibles y Git como flujo principal para desarrollo individual y revisiones por pares. Se permiten SQL, YAML y JSON según el recurso; no hay extensión única obligatoria.
3. **Repositorio multiusuario:** PostgreSQL como opción inicial razonable para transacciones, concurrencia, índices, auditoría y revisiones; nunca como dependencia del núcleo.
4. **Proyección opcional:** SQLite como índice/cache local o backend transaccional ligero, NO como requisito universal ni empaquetado único del proyecto.
5. **Intercambio de estándares:** exportación/importación con adaptadores a TMDL, Apache Ossie, dbt y OpenLineage, preservando explícitamente la información que no pueda convertirse.
6. **Estado de ejecución separado:** los trabajos, logs, credenciales, caché, sesiones y resultados físicos no son parte de las definiciones portables.
7. **Publicación segura:** las revisiones publicadas son inmutables, con punteros que se actualizan atómicamente; un borrador inválido nunca sustituye una versión válida.

**Límite importante:** «agnóstico» no significa admitir de inmediato todos los almacenes, estándares y formatos. Significa que el contrato central no obliga a elegirlos y permite implementarlos por adaptadores conforme existan pruebas.

---

## 1. Qué es un metadato en BI: seis dominios distintos

| Dominio | Ejemplos | Duración / autoridad | Lugar lógico |
|---|---|---|---|
| **Definición de producto** | Models, medidas, dimensiones, relaciones, dashboards, temas, VizSpec | Versionable; sujeto a aprobación | `DefinitionRepository` |
| **Catálogo y descubrimiento** | Esquemas detectados, tipos, estadísticas, descripciones, owners, certificaciones | Puede cambiar fuera del proyecto | `CatalogIndex` + proveedores de origen |
| **Estado operacional** | Ejecución, job, checkpoints, errores, calidad, último refresco | Mutable y temporal | `OperationalStore` |
| **Publicación / releases** | RevisionId, manifiesto publicado, compatibilidad, enlaces a snapshots | Histórico inmutable + puntero actual | `PublicationRegistry` |
| **Identidad, permisos y secretos** | Roles, grupos, políticas, referencias de claves | Fuertemente protegido | `PolicyStore` y `SecretProvider` |
| **Estado de usuario y sesiones** | Selecciones, filtros temporales, panel expandido, bookmarks | Efímero o preferencia personal | `SessionStore` |

**El almacenamiento analítico no figura aquí:** una tabla en `.duckdb`, Parquet en DuckLake o ClickHouse pertenece al plano de datos. Su *referencia y versión* sí puede aparecer en metadatos.

No juntar los seis dominios en un archivo monolítico ni pretender que una transacción SQLite abarque DuckLake, Git y PostgreSQL: son dominios de consistencia distintos.

### 1.1 Autoridad y confianza

- `Source of truth` significa el lugar que acepta cambios de definición; depende del modo de instalación, no del motor semántico.
- Un esquema detectado es **observación**, no contrato permanente; el usuario puede fijar un esquema esperado.
- Una definición SQL/YAML es **intención**, no prueba de que el objeto físico publicado exista.
- Una política de seguridad no puede sustituirse por valores recibidos desde la sesión del navegador.
- Un grafo de dependencias compilado es una **proyección derivada**: se regenera a partir de definiciones y resolver de referencias.

---

## 2. Auditoría A — Rill: archivos, RepoStore, Parser y recursos tipados

### 2.1 Evidencia concreta (tag fijo v0.90.4)

**Código:** https://github.com/rilldata/rill/blob/v0.90.4/runtime/parser/parser.go  
**Código:** https://github.com/rilldata/rill/blob/v0.90.4/runtime/drivers/repo.go  
**Proyecto declarativo:** https://github.com/rilldata/rill/blob/main/docs/docs/reference/project-files/rill-yaml.md

En `parser.go`, la estructura `Resource` tiene `Name`, `Paths`, `Refs`, `Tags`, `Metadata` y un campo de spec tipado según `ResourceKind` (Source, Model, MetricsView, Explore, Canvas, Component, etc.). **Un recurso puede provenir de varios archivos, y un archivo puede contribuir a varios recursos.** El parser ofrece `Parse`, `Reparse`, un `Diff` con added/modified/deleted, y registra errores con origen. El parser documenta que no es seguro para uso concurrente, por lo que sus llamadas deben coordinarse.

En `drivers/repo.go`, `RepoStore` es una interfaz para un repositorio de archivos con operaciones como `ListGlob`, `Get`, `Hash`, `Put`, `Rename`, `Delete` y otras; no expone una implementación física única al parser. El contrato del repositorio incluso contiene operaciones de control de conflictos en su conjunto de capacidades.

En `parser.go` se hallan límites defensivos de 10.000 archivos y 128 KiB por archivo analizado: **son límites propios de esa implementación**, NO valores que debamos copiar automáticamente para otro proyecto.

### 2.2 Rescatar

- Recursos **tipados y referenciables** separados del archivo que los contiene.
- Parser incremental: saber qué cambió evita recompilar todo el proyecto.
- Mapa inverso archivo → recursos para recargar únicamente dependencias.
- Interfaz de repositorio independiente del filesystem.
- Distinción parse error vs recurso compilado exitoso.
- YAML y SQL legibles, favorables a colaboración y revisión en Git.

### 2.3 Mejorar para la plataforma nueva

- Separar todavía mejor **Document/Authoring AST**, **Normalized Resource Definition**, **Compiled Resource** y **Published Resource**: no confundir texto, definición, plan ni revisión efectiva.
- Permitir backend de repositorio transaccional además de archivo/Git, sin acoplar parser a rutas físicas.
- Implementar `write_if_revision_matches` en lugar de sobrescrituras ciegas: protección ante edición concurrente.
- Distinguir `resource_id` estable de `display_name`, `slug` y `path` (el renombrado no debe romper referencias).
- Mantener diagnósticos estructurados `file/line/column/resource/field`, no únicamente mensaje de error.
- Hacer que la compilación de un recurso cambie solo el índice derivado; publicación separada con validaciones.

### 2.4 Lo que NO copiamos

El formato `rill.yaml`, sus nombres de objetos y su gramática particular. Son contratos de Rill, no estándares transversales de BI. Tampoco se asume que «archivos» sea la única manera de almacenar proyectos.

---

## 3. Auditoría B — Lightdash: metadatos transaccionales y versiones

**Código:** https://github.com/lightdash/lightdash/blob/main/packages/backend/src/models/DashboardModel/DashboardModel.ts  
**Código:** https://github.com/lightdash/lightdash/blob/main/packages/backend/src/models/ProjectModel/ProjectModel.ts  
**Repositorio:** https://github.com/lightdash/lightdash

### 3.1 Observaciones de código

En `DashboardModel.ts`, `createVersion(trx, dashboardId, version, ...)` crea una versión y almacena, dentro de una transacción, vistas, tabs, tiles y referencias. La rutina depura `tileTargets` que apuntan a tiles inexistentes y evita conservar `tabUuid` inválidos. Son **restricciones de integridad referencial en recursos visuales**, no simples opciones cosméticas.

El backend de Lightdash se describe como TypeScript + Node + Knex + PostgreSQL, además de sus conectores de warehouses. En `ProjectModel.ts`, existe un mecanismo de bloqueo de compilación mediante `pg_try_advisory_xact_lock`: ejemplo de control de concurrencia de operaciones de proyecto. No significa que PostgreSQL deba ejecutarlo todo ni que ese lock funcione en cualquier backend.

### 3.2 Rescatar

- Cambios agrupados transaccionalmente en una revisión coherente.
- Versiones de dashboard y objetos relacionados con referencias verificadas.
- IDs persistentes de componentes, no dependientes de su posición.
- Lock/serialización para tareas de compilación que no deben pisarse.
- Separación entre contenido declarativo y metadatos operativos de la aplicación.

### 3.3 Mejorar

- No propagar el esquema SQL interno de la app al contrato público: exponer tipos/API de recurso estables.
- El export de un proyecto debe ser **determinista** y sin secretos; Git no debe recibir datos de sesión o históricos voluminosos.
- Añadir validación previa al commit de cambios multi-recurso y revisiones publicadas atómicas.
- Convertir referencias `dashboard→viz→metric→model` en grafo común, no verificarlas con lógica ad hoc distinta en cada pantalla.

### 3.4 No copiar

Su diseño físico de tablas ni asumir que Knex/Node/PostgreSQL son arquitectura obligatoria. El patrón de versiones y referencias es lo valioso.

---

## 4. Auditoría C — Apache Superset: metadatos operativos e import/export

**Código:** https://github.com/apache/superset/blob/master/superset/models/core.py  
**Código:** https://github.com/apache/superset/blob/master/superset/models/dashboard.py  
**Documentación:** https://superset.apache.org/docs/configuration/importing-exporting-datasources/

### 4.1 Evidencia

El modelo `Database` de `models/core.py` representa conexiones y capacidades, incluyendo propiedades almacenadas de forma cifrada (`password`, `encrypted_extra`). `Dashboard` de `models/dashboard.py` utiliza ORM con relaciones, registros de auditoría y configuración/versionado. Superset puede exportar/importar fuentes y activos mediante YAML/ZIP, pero sus datos operativos siguen estando en su base de metadatos.

### 4.2 Rescatar

- Base OLTP de metadatos distinta de warehouses de análisis.
- Importación/exportación de activos relacionados.
- Modelos con relaciones y auditoría; configuración cifrada separada.
- Etiquetas, propiedad del activo y certificación cuando aporten valor de gobierno.

### 4.3 Mejorar

- No mezclar configuraciones sensibles en bundles compartibles. Incluso el import/export oficial de Superset documenta precauciones sobre valores cifrados: debe existir un modo **seguro por defecto**, sin bytes de credenciales ni tokens.
- Evitar que `position_json`, opciones de proveedor o IDs internos de una base de datos sean la definición universal del dashboard.
- La importación debe resolver IDs y referencias, mostrar un plan/diff de conflictos y permitir modo dry-run.

### 4.4 No copiar

Ni la jerarquía completa del ORM ni la necesidad de una base central incluso para un proyecto local offline.

---

## 5. Auditoría D — Grafana: dashboard como recurso versionado y APIs

**Código:** https://github.com/grafana/grafana/blob/main/pkg/api/dashboard.go  
**Migraciones:** https://github.com/grafana/grafana/blob/main/pkg/services/sqlstore/migrations/dashboard_mig.go

### Evidencia

El API de dashboards incluye recuperación por UID, consulta de versiones y restauración. Su código actual contempla recursos estilo Kubernetes, con `apiVersion`, `kind`, `metadata`, `spec`, y distingue APIs heredadas. Las migraciones históricas evidencian evolución del esquema persistido y conservación de compatibilidad.

### Rescatar

- Identidad estable por UID y versiones explícitas.
- Documento declarativo + envoltura de metadatos administrativos.
- Endpoints de lectura, edición, versiones y restauración.
- Evolución de esquema documentada mediante migraciones.

### Mejorar

- Modelo de recurso propio mínimo, sin requerir Kubernetes como dependencia del producto.
- Diferenciar `apiVersion` del formato de recurso frente a `revision_id` de un cambio concreto.
- No exponer todas las capacidades internas de cada visualización en el tipo estable de dashboard.
- Protección optimista ante actualizaciones concurrentes (`expected_revision`), errores útiles y conflictos visibles.

---

## 6. Estándares que conviene integrar, pero no confundir con el núcleo

### 6.1 Microsoft TMDL

Fuente: https://learn.microsoft.com/en-us/analysis-services/tmdl/tmdl-overview

TMDL serializa modelos tabulares del Tabular Object Model, optimizado para lectura, edición y control de versiones. Su semántica y gramática están orientadas a modelos Microsoft/Analysis Services. **No es un formato genérico de dashboards ni reemplazo universal de Models SQL**.

**Decisión:** adaptador de importación/exportación de un subconjunto compatible (tablas, columnas, medidas, relaciones, descripciones) con reporte de pérdida de compatibilidad. No intentar asumir compatibilidad total con DAX.

### 6.2 Apache Ossie (antes Open Semantic Interchange)

Fuentes: https://github.com/apache/ossie y https://github.com/apache/ossie/blob/main/core-spec/spec.md

Ossie define intercambio de **metadatos semánticos** con JSON/YAML para datasets, fields, metrics y relationships. El repositorio distingue especificaciones publicadas y desarrollo; en octubre de 2026 su especificación en `spec.md` marca `0.2.0.dev0` como inestable/no publicada y registra `0.1.1` como versión inicial.

**Decisión:** **primera apuesta de interoperabilidad semántica**, mediante adaptador versionado. Para evitar pérdida de significado, registrar expresiones de dialecto nativo, restricciones temporales, seguridad, aditividad y extensiones no portables. No declarar que Ossie define por sí solo la totalidad de una plataforma BI o de la persistencia del proyecto.

### 6.3 dbt y artefactos de compilación

Referencia: https://docs.getdbt.com/reference/artifacts/manifest-json

Distinguir proyecto fuente/dbt YAML de `manifest.json` compilado y `run_results.json` operacional. Los artefactos muestran por qué **definiciones** y **resultados observados** tienen ciclos de vida distintos.

**Decisión:** importar el DAG y catálogo conocido cuando haya integración dbt; no imponer dbt como motor de Models ni almacenar su manifest como formato canónico propio.

### 6.4 OpenLineage

Fuente: https://openlineage.io/docs/spec/ y https://github.com/OpenLineage/OpenLineage/blob/main/spec/OpenLineage.md

OpenLineage diferencia Job, Run y Dataset con facets extensibles, eventos START/COMPLETE/FAIL/ABORT y metadata de esquemas/linaje. Su especificación permite linaje por columnas y versiones de datasets según los facets usados.

**Decisión:** exportar eventos reales de ejecución y lineage desde nuestros módulos, no usar OpenLineage como formato de edición de dashboards ni inventar eventos de ejecuciones que no ocurrieron.

### 6.5 Estándares que no elegimos por anticipado

No se declara que exista un estándar universal, maduro y suficiente para modelado, seguridad, layouts, ETL, semántica, queries y visualización en un solo documento. La interoperabilidad se consigue con contratos tipados propios **más adaptadores**, no afirmando equivalencias inexistentes.

---

## 7. Modelo lógico de recursos: contrato estable, no formato de archivo

### 7.1 Envoltorio común propuesto

```json
{
  "api_version": "bi.platform/v1",
  "kind": "SemanticModel",
  "id": "018f0000-0000-7000-8000-000000000021",
  "namespace": "comercial",
  "name": "facturacion",
  "labels": {"domain": "sales"},
  "annotations": {"description": "Modelo de facturación"},
  "spec": {"model_ref": "model:018f0000-0000-7000-8000-000000000020"}
}
```

**Esquema totalmente propuesto e ilustrativo**: `bi.platform/v1` no es un estándar existente ni nombre de producto. Su `id` es una muestra de UUID estable. El contrato real deberá definir cada `kind` y pasar validación de JSON Schema/Serde.

| Campo | Semántica |
|---|---|
| `api_version` | Versión del esquema de este recurso; no del ejecutor ni del proyecto |
| `kind` | Tipo de recurso con registro de tipos permitido |
| `id` | Identidad estable inmutable, preferiblemente UUID/ULID compatible |
| `namespace` | Espacio lógico; separar de organización física y carpetas |
| `name` | Nombre editable; no usarlo como FK estable |
| `labels`, `annotations` | Búsqueda, documentación y extensiones validadas |
| `spec` | Contrato tipado específico del recurso |

`created_at`, `updated_at`, `owner`, `revision`, `etag`, `status`, `published_revision` pueden aparecer en la envoltura servida por la API, pero no todos deben guardarse en el archivo de definición; se separan metadatos editoriales de estado servidor.

### 7.2 Tipos mínimos (propuestos)

- `ConnectionRef` — conecta con un SecretProvider, sin incrustar credenciales.
- `SourceDataset` — origen observable con esquema y capacidades.
- `Model` — definición SQL, dependencias, configuración de ejecución.
- `SemanticModel` — datasets, dimensiones, medidas, relaciones, calendarios.
- `Explore` — catálogo de análisis/medidas disponibles.
- `PivotDefinition` — disposición semántica de matriz.
- `Dashboard` — páginas, layouts, interacciones, componentes.
- `VizSpec` — contrato neutral de representación.
- `Policy` — permisos, RLS/CLS, referencias de identidad.
- `Schedule` — refrescos/alertas sin contener secretos.
- `Theme` — propiedades visuales neutrales.
- `MaterializationPolicy` — política deseada, no objeto físico.

Estas clases son entidades **lógicas**. No obligan a crear 11 tablas, archivos, servicios o repositorios independientes.

### 7.3 Referencias

- `ResourceRef = {kind, id, optional revision_constraint}`.
- Resolver referencias por ID y por alcance permitido; un nombre duplicado no puede resolver arbitrariamente otro modelo.
- Prohibir referencias circulares donde el grafo sea acíclico; validar conflictos y claves ausentes antes de publicar.
- Los dashboards no guardan copias de SQL de las medidas. Guardan referencias semánticas estables y políticas explícitas de versiones.
- Una referencia a snapshot físico debe incluir dataset ID, storage ID, snapshot/revision y política de frescura; no solo un path.

### 7.4 Tres representaciones diferentes de un mismo recurso

1. **Source document**: texto que el usuario edita o crea mediante UI; preservar comentarios donde sea viable.
2. **Normalized IR**: estructura validada independiente de YAML/JSON/Rust; IDs y referencias resueltas.
3. **Compiled/Published artifact**: definición compilada, fingerprint, diagnósticos, dependencias y revisión efectiva.

No serializar objetos internos del parser o un AST SQL específico como el contrato de intercambio a largo plazo.

---

## 8. Repositorios de metadatos: comparativa de alternativas

| Alternativa | Mejor uso | Ventajas | Límites |
|---|---|---|---|
| **Directorio + Git** | Autores, desarrollo local, CI/CD | Diff, revisión, historial, portabilidad, herramientas estándar | Transacciones entre archivos, colaboración en tiempo real y consultas ad hoc requieren proyección/control |
| **SQLite** | Local offline, index/cache, proyecto monousuario | Embebido, transaccional, sencillo | Fusión Git de archivos binarios pobre; concurrencia distribuida limitada |
| **PostgreSQL** | Multiusuario, servidor, auditoría | ACID, MVCC, índices, búsquedas, permisos de servicio | Requiere operación, backup y sincronización con Git |
| **Git + PostgreSQL** | Empresa que quiere GitOps y editor web | Code review + colaboración + API | Si ambos son escribibles sin protocolo, aparecen dos verdades |
| **Object storage** | Backups, bundles, artefactos inmutables | Escala, distribuye snapshots | No sustituye una base transaccional para edición concurrente |
| **DuckDB / DuckLake** | Datos y snapshots analíticos | Alta capacidad analítica | **No elegirlo como metastore OLTP principal** por defecto |

### 8.1 Recomendación condicionada por modo

**Modo desarrollo local:** archivos declarativos en una carpeta y un índice SQLite **derivado/descartable**; Git controla los cambios. Si se usa una interfaz visual, escribe los archivos usando un adaptador de repositorio con bloqueo y revisión esperada.

**Modo servidor multiusuario:** PostgreSQL como **autoridad transaccional de recursos publicados y borradores compartidos**; Git puede ser origen de importación o destino de exportación mediante sincronización controlada. Guardar trazabilidad del commit y hash de cada recurso.

**Modo GitOps estricto (alternativo):** Git es la autoridad de las *definiciones*; el servidor ingiere commits validados y sirve una proyección de solo lectura. La UI que edita crea ramas/PRs, no muta de forma independiente una definición publicada. El estado operativo sigue fuera de Git.

**Regla de oro:** no permitir dos autores concurrentes no coordinados (Git y SQL) sobre la misma definición. Definir explícitamente quién es la autoridad *por workspace*. Una misma API lógica puede usar diferentes backends y procesos de publicación.

---

## 9. Contratos de almacenamiento en Rust (diseño, no implementación)

```rust
// Pseudocódigo: firmas ilustrativas, no crate compilado.
trait DefinitionRepository {
    async fn get(&self, id: ResourceId, revision: Option<RevisionId>) -> Result<Resource>;
    async fn list(&self, filter: ResourceFilter, page: Page) -> Result<ResourcePage>;
    async fn put(&self, draft: Resource, expected: Option<RevisionId>) -> Result<RevisionId>;
    async fn delete(&self, id: ResourceId, expected: RevisionId) -> Result<()>;
}

trait PublicationRegistry {
    async fn stage(&self, release: ReleaseManifest) -> Result<ReleaseId>;
    async fn publish(&self, id: ReleaseId, expected: RevisionId) -> Result<()>;
    async fn current(&self, workspace: WorkspaceId) -> Result<ReleaseManifest>;
}

trait CatalogIndex {
    async fn resolve(&self, reference: ResourceRef) -> Result<ResourceDescriptor>;
    async fn dependents(&self, id: ResourceId) -> Result<Vec<ResourceId>>;
    async fn search(&self, query: CatalogQuery) -> Result<CatalogPage>;
}

trait SecretProvider {
    async fn resolve(&self, reference: SecretRef, actor: ServicePrincipal) -> Result<SecretValue>;
}
```

El contrato real debe incluir transacciones o `UnitOfWork`, errores tipados (`Conflict`, `NotFound`, `Forbidden`, `ValidationFailed`, `Unavailable`), cancelación, streaming cuando corresponda, paginación y trazas. El consumidor nunca debe saber si se ejecuta SQLite, PostgreSQL o filesystem.

**Pragmatismo:** no crear un trait separado por cada tabla. Agrupar operaciones que comparten invariantes y transacciones; implementar puertos solo para variaciones reales.

### 9.1 Atomicidad distinta en filesystem y SQL

- PostgreSQL: cambio multi-recurso y puntero de publicación en una transacción, con validación anticipada.
- Filesystem: escritura temporal + fsync + rename por archivo; el conjunto multiarchivo no se vuelve atómico por arte de magia. Publicar **manifiesto de revisión** que apunte al conjunto completo y cambiar su referencia final en un único paso controlado.
- Git: commits inmutables ya agrupan el árbol; actualizar la rama con control de conflictos. La publicación apunta a commit SHA + manifest hash.
- DuckLake: snapshot de datos físico independiente del release de metadata; coordinar mediante referencias explícitas y reconciliación tras fallos, no simular una transacción distribuida inexistente.

---

## 10. Versionado correcto: cuatro números diferentes

**No utilizar un único `version` ambiguo.**

| Identificador | Ejemplo | Qué versiona |
|---|---|---|
| `schema_version` | `bi.platform/v1` | Estructura del documento de metadatos |
| `resource_revision` | hash contenido `sha256:...` | Un cambio concreto a un recurso |
| `release_id` | UUID / hash manifiesto | Conjunto coherente de recursos publicado |
| `data_snapshot_id` | DuckLake snapshot / tabla física | Datos consultados por un release o dashboard |
| `platform_release` | `0.2.0` | Versión del software; independiente de los anteriores |

Agregar `semantic_revision` (o modelo efectivo) y `engine_version` al manifest de una consulta para auditar exactamente qué lógica produjo un resultado.

### 10.1 Algoritmo de fingerprint

- Normalizar estructuras tipadas: orden determinista de mapas, strings Unicode consistentes, números representados canónicamente.
- No hashear datos volátiles (hora de guardar, autor del último clic) como parte del contenido semántico.
- Distinguir `content_hash` y `effective_hash`: el segundo puede incluir IDs/versiones transitivas de dependencias y capacidades de un dialecto.
- Conservar campo específico de `representation_hash` para detectar modificaciones no semánticas relevantes para el editor (comentarios, estilo).
- No declarar equivalencia semántica de SQL solo por cambios de formato si el parser no la puede demostrar.

### 10.2 Draft → Validate → Plan → Publish

```text
Editar definiciones (draft / branch)
    ↓
Parsear + normalizar (diagnósticos con ubicación)
    ↓
Resolver referencias + DAG + esquemas + seguridad
    ↓
Plan de impacto (recursos alterados + afectados)
    ↓
Validar permisos y calidad; compilar artefactos
    ↓
Crear ReleaseManifest inmutable
    ↓
Publicar puntero release activo mediante CAS
    ↓
Indexar y notificar caches/consumidores
```

Si algo falla, la versión publicada anterior permanece activa. `CAS` = compare-and-swap respecto a la revisión que el editor creía estar modificando.

---

## 11. Manifiesto de publicación: consistencia sin locking global

```json
{
  "release_id": "release_2026_10_10_001",
  "workspace_id": "workspace_analytics",
  "schema_version": "bi.platform/v1",
  "resources": [
    {"kind": "Model", "id": "model-id", "revision": "sha256:model..."},
    {"kind": "SemanticModel", "id": "semantic-id", "revision": "sha256:semantic..."},
    {"kind": "Dashboard", "id": "dashboard-id", "revision": "sha256:dashboard..."}
  ],
  "compiled_against": {
    "engine": "duckdb",
    "capabilities_version": "validated-capabilities-id"
  }
}
```

**Ejemplo ilustrativo, no esquema definitivo.**

Invariantes:

- No publicar un dashboard si refiere una métrica ausente o incompatible.
- No publicar una Metrics View que refiera modelo físico inservible si no existe fallback válido.
- Resolver todos los recursos desde la **misma revisión lógica** durante la ejecución de una sesión.
- Fijar estrategia de datos (`latest-compatible`, `snapshot-pinned`, etc.) explícita por modelo/dashboard.
- Una restauración crea nuevo evento de publicación que apunta a una revisión conocida: no editar en sitio el pasado.
- Mantener bitácora de cambios de seguridad; revocación de privilegios debe surtir efecto inmediatamente aunque el release histórico exista.

---

## 12. Diseño de proyecto portable sin extension propietaria

**Ejemplo orientativo (no estructura obligatoria):**

```text
analytics-project/
├── project.yaml                # workspace, dependencias, entorno mínimo, schemas
├── sources/
│   └── billing.yaml            # connection_ref, source datasets; sin secretos
├── models/
│   ├── billing.sql             # SQL nativo del engine declarado
│   └── billing.model.yaml      # metadata de build, pruebas, materialización
├── semantic/
│   └── commercial.yaml        # dimensiones, métricas, relaciones
├── explores/
│   └── billing.yaml
├── dashboards/
│   └── executive.dashboard.yaml
├── visuals/
│   └── billing_trend.viz.yaml  # especificación visual neutral
├── policies/
│   └── roles.yaml              # definición pública, sin secretos
└── tests/
    └── checks.yaml
```

El editor puede generar estos archivos sin pedir al usuario escribir YAML a mano. **No convertir el árbol en un DSL obligatorio si una instalación desea almacenar documentos de recursos en PostgreSQL**. El bundle exportado usa la misma representación pública aunque el backend de origen sea SQL.

### 12.1 Separación de secretos

Archivo público:

```yaml
kind: ConnectionRef
name: billing_pg
spec:
  provider: postgres
  endpoint_ref: billing-primary
  credential_ref: secret://workspaces/analytics/billing-reader
```

`secret://...` es **una URI simbólica de referencia propuesta**, no una contraseña, ni un protocolo de red, ni un estándar universal. El SecretProvider de producción resolverá credenciales con IAM y auditoría.

### 12.2 Modelos y medidas: no repetir SQL

`models/billing.sql` define el dataset. `semantic/commercial.yaml` define `facturado = SUM(importe)` una sola vez. Explore, Matrix, Dashboard y APIs referencian `metric_id`, no guardan una copia de SQL agregada. Las medidas derivadas y políticas de filtros pertenecen al motor semántico, no al repositorio.

---

## 13. APIs de metadatos necesarias

| Operación | Objetivo | Restricción |
|---|---|---|
| `create/update/delete_resource` | Autoría tipada | Revisión esperada + permisos |
| `validate_resource` | Diagnósticos estructurados | Sin efectos secundarios por defecto |
| `plan_changes` | Impacto, dependencias, compatibilidad | No publica ni materializa inadvertidamente |
| `publish_release` | Cambiar revisión efectiva | Atomicidad del puntero y tests |
| `rollback_release` | Restaurar definición efectiva | Nuevo evento auditado |
| `search_catalog` | Encontrar activos | No revelar existencia prohibida |
| `describe_lineage` | Origen e impacto por recurso | Respeta visibilidad y versión |
| `export_bundle/import_bundle` | GitOps y portabilidad | Dry run, reporte de conflictos, secretos excluidos |
| `list_revisions/diff_revisions` | Revisión humana/CI | No exponer datos sensibles |
| `resolve_snapshot` | Conectar definición a artefacto físico | Contrato con materialización |

Los endpoints REST, GraphQL y RPC son superficies opcionales sobre estos comandos. **El núcleo no debe depender de rutas HTTP**.

---

## 14. Catálogo: activos, entidades, búsqueda y dependencias

Distinguir dos grafos:

1. **Dependencias de definición:** `Dashboard → VizSpec → Metric → Model → SourceDataset` y modelos entre sí. Puede compilarse de forma determinista con las definiciones.
2. **Linaje observado:** ejecuciones reales y datasets físicos que se leyeron/escribieron, con IDs de job/run, tiempo, esquema y snapshot. Es incompleto si faltan eventos o fuentes externas.

No inferir que una dependencia de configuración prueba que una ejecución real utilizó el dataset. Y no confundir linaje de columnas con relaciones semánticas del modelo estrella.

### 14.1 Índices recomendados

- Por `(tenant, kind, id)` y por nombres/aliases dentro de namespace.
- Índice inverso de referencias para análisis de impacto y bloqueo de borrados.
- Índice texto: nombre, descripción, tags, owner, medidas; sujeto a permisos.
- `effective_revision`, `schema_fingerprint`, `last_observed_at`, `published_release`.
- Tags y certificaciones pueden ser gestionados por el catálogo sin reescribir SQL de Model.

### 14.2 Descubrimiento de fuentes

La metadata detectada contra PostgreSQL/DuckDB/ClickHouse puede volverse obsoleta. Guardar `observed_at`, `source_version` y `schema_fingerprint`. Mostrar discrepancias entre definición esperada y realidad sin reescribir silenciosamente modelos.

---

## 15. Interoperabilidad: contratos de importación/exportación

### 15.1 Principio

**Nunca un formato externo debe convertirse en el AST canónico del motor.** Convertir con adaptadores explícitos:

`ExternalFormat → ImportedResource + Diagnostics → NormalizedResource → Validation → Release`.

### 15.2 Matriz de cobertura propuesta

| Entrada/salida | Datos que puede representar | No asumir |
|---|---|---|
| Apache Ossie | Métricas, datasets, campos, relaciones | Dashboards, jobs, seguridad completa, madurez de especificaciones dev |
| TMDL | Tabular model, tablas, columnas, relaciones y medidas | DAX ejecutable automáticamente en DuckDB |
| dbt | SQL models, referencias y metadatos/manifest | Que dbt reemplace al Model Engine propio |
| Rill YAML | Model/metrics/explore según versión Rill | Que sus props sean las nuestras |
| Grafana dashboard JSON | Paneles, vistas y ciertas interacciones | VizSpec portable 1:1, semántica uniforme |
| Superset ZIP/YAML | Fuentes, charts, dashboards, métricas soportadas | Exportación de secretos ni compatibilidad sin pérdidas |
| OpenLineage | Jobs/runs/datasets/linaje | Definición de UI ni motor semántico |

Todo adaptador retorna `converted`, `lossy`, `unsupported` y `diagnostics`; en conversiones con pérdida requiere confirmación. Jamás eliminar silenciosamente RLS, granularidad, unidades o roles temporales.

### 15.3 Versionar adaptadores

Cada export importa también la versión de origen, la versión del adaptador y el reporte de mapeo. Los tests de regresión fijan fixtures provenientes de versiones precisas. Para repositorios `main`, registrar commit SHA al iniciar implementación.

---

## 16. Colaboración y concurrencia: tres escenarios

### 16.1 Único autor offline

Directorio + Git opcional. El editor guarda en archivos y un índice SQLite reconstruible. Confirmación de cambios por comando. No necesita CRDT ni servidor de sincronización.

### 16.2 Equipo pequeño

PostgreSQL transaccional, revisiones optimistas y edición de recursos individual. Conflicto al editar misma revisión: mostrar diff y exigir reconciliación. `Draft` aislado por autor/sesión. No confundir historial con autoría simultánea en una misma línea de código.

### 16.3 Empresa/GitOps

Envío de propuestas desde UI a PR, CI valida y servidor despliega solo commits aprobados **o** UI edita PostgreSQL como autoridad y un exporter produce commits unidireccionales. Elegir uno por workspace; no habilitar bidireccionalidad sin reconciliador robusto y política de conflictos.

### 16.4 Evolución posterior

Edición colaborativa en tiempo real por CRDT/OT no es prioridad mientras contratos de versiones, refs y publicación no estén probados. Si se incorpora, el CRDT gestiona borradores de editor; nunca sustituye las invariantes del PublicationRegistry.

---

## 17. Seguridad y gobierno de metadatos

- Todas las consultas de catálogo aplican permisos. Un listado de nombres puede revelar información sensible.
- Seguridad operativa y semántica se evalúa en backend, no solo en UI ni en YAML.
- Guardar `credential_ref`; nunca serializar tokens o contraseñas en bundles, Git o logs.
- ACL por workspace, recurso y operación; separar `can_edit`, `can_publish`, `can_execute`, `can_administer_secrets`.
- Registro de autor, evento, timestamp, tipo de modificación, revisión anterior y nueva; no almacenar secretos dentro de diffs.
- Revisiones antiguas no deben permitir bypass de políticas actuales: el acceso se decide en cada solicitud.
- Borrados: implementar soft-delete/tombstone para referencias, retención configurable y restauración auditada.
- Firmas/checksums de artefactos publicados para evitar carga de contenido alterado.
- YAML/JSON importados: límites de tamaño, profundidad, aliases, campos desconocidos y archivos externos; evitar inyección mediante SQL de definiciones no autorizadas.
- SSRF, traversal (`../`) y symlinks al importar recursos: validar rutas y destinos.

**Regla fail-closed:** si no puede comprobarse autorización o integridad de una definición, no publicarla ni consultar el recurso protegido.

---

## 18. Migraciones de esquema y compatibilidad hacia atrás

**Nunca confundir:** versión del software, versión de documento, migración física de la base de metadatos y revisión de contenido.

Secuencia:

1. Un nuevo binario identifica capacidades y versiones soportadas.
2. Lee un recurso legado a través de `Reader(version=n)` sin cambiarlo todavía.
3. Ejecuta conversión pura a versión normalizada nueva; conserva original y diagnóstico.
4. Ensaya `dry-run`: referencias, hashes, semántica, permisos y diferencias.
5. Al confirmar, publica nueva revisión y mantiene backup/rollback.
6. Si usa PostgreSQL, migra índices/tablas con migración transaccional cuando sea posible; desaconsejar migraciones irreversibles no comprobadas.
7. Si usa Git, commit de migración reproducible y revisión de PR.

Para campos desconocidos, distinguir entre `reject`, `preserve_extension` y `warn`: seguridad rechaza, propiedades de visualización pueden conservarse en extensiones versionadas, valores triviales pueden avisar.

---

## 19. Relación con los nueve módulos anteriores

| Módulo | Qué entrega al catálogo/metastore | Qué recibe del metastore |
|---|---|---|
| M01 Sources & Ingestion | Sources observadas, batch manifests, schema fingerprints | ConnectionRef, SourceDefinition, secretos por referencia |
| M02 Models & Transformation | ModelDefinition, dependencias, builds, tests | Revisión del Model y plan/publicación |
| M03 Storage & Materialization | PublishedDatasetRef, snapshot/freshness | MaterializationPolicy y versiones activas |
| M04 Semantic Engine | SemanticModel, medidas y relaciones compiladas | Definiciones aprobadas y policy refs |
| M05 Query Serving | Historial operativo y estadísticas agregadas | Release, semántica, snapshot, actor |
| M06 Explore | ExploreDefinition, bookmarks opcionales | MetricRefs, configuración publicada |
| M07 Pivot & Matrix | PivotDefinition, layout neutro | Semantic refs y revisión |
| M08 Canvas | DashboardDefinition, paneles, navegación | VizSpec y MetricRefs |
| M09 Visualization | VizSpec, registry/capabilities por proveedor | RenderSpec y preferencias neutrales |

No trasladar trabajo de otros módulos al catálogo. Este módulo **almacena, indexa, valida referencias, versiona y publica**; no ejecuta SQL ni renderiza gráficos.

---

## 20. Interfaces entre Rust y frontend

### 20.1 Núcleo

Los tipos de recursos y comandos de metadatos se implementan en Rust con serialización validada y schemas para clientes. SQL/DuckDB, store y APIs son adaptadores.

### 20.2 Frontend

El frontend consume DTOs/versiones generadas desde el mismo contrato. No mantener interfaces TypeScript duplicadas a mano cuando cambie un schema.

### 20.3 Compilación y tipos

- JSON Schema / OpenAPI como contrato público transportable.
- Rust `serde` para serialización, validación separada para invariantes semánticas.
- SQL estándar de compatibilidad no reemplaza dialectos específicos de ejecución.
- UUID, Decimal, timestamps y zonas horarias se codifican sin truncamiento.
- El programa puede añadir protobuf/Arrow solo si existen necesidades específicas de RPC/columnar: no forzarlos como formato de metadatos.

---

## 21. Plan de implementación concreto (sin crear infraestructura innecesaria)

### Etapa 1 — Contratos y parser

Entregar `ResourceId`, `ResourceKind`, `ResourceRef`, `Definition`, `Revision`, `ReleaseManifest`, `Diagnostics`, tipos de error y schemas versionados; un parser para recursos SQL/YAML. Probar importación/extracción de nombres/dependencias con SQL real.

**Criterio:** dos modelos y una medida se compilan con IDs estables, y un cambio de nombre no rompe referencias.

### Etapa 2 — Repository local / Git-ready

Desarrollar `FilesystemRepository` (directorio declarativo), escritura segura, hash, listing, reverse deps; indexar en memoria o SQLite de proyección. Exportación deterministic sin secretos.

**Criterio:** save→close→open produce idénticas definiciones y referencias; Git diff es legible.

### Etapa 3 — Draft/Publish

Añadir revisiones inmutables, manifiesto y CAS para publicación; validar permisos y dependencias; rollback sin reescribir historial.

**Criterio:** draft inválido no altera release servido.

### Etapa 4 — Servidor PostgreSQL (cuando haya colaboración real)

Implementar backend transaccional de recursos y estados de publicación, migraciones, auditoría, permisos; mismas APIs y fixtures que filesystem.

**Criterio:** dos editores en conflicto nunca pierden silenciosamente cambios; CI y UI trabajan con revisión explícita.

### Etapa 5 — Interoperabilidad

Primero Ossie (versión publicada/estable) y OpenLineage; después TMDL, dbt y exporters de dashboards específicos, basándose en pruebas de compatibilidad.

**Criterio:** conversiones con pérdida se anuncian; export→import de casos compatibles conserva medida, tipos, granularidad y relaciones.

### Etapa 6 — Catálogo empresarial

Índice semántico y búsqueda, impacto, propiedad, certificación, datasets observados, reconciliador GitOps, gobernanza multiworkspace. No añadir Elasticsearch o motores de grafos antes de medir PostgreSQL/índices existentes.

**Criterio:** búsqueda autorizada y `impact(ModelId)` correctos a través de todas las capas.

---

## 22. Pruebas de aceptación (50 casos mínimos)

### A. Identidad e integridad

1. Crear recurso ID estable; renombrar conserva referencias.
2. Dos recursos con mismo nombre en distintos namespaces no colisionan.
3. Recurso sin `kind` o `api_version` es rechazado.
4. Referencia inexistente bloquea publicación.
5. Dependencia circular entre modelos se reporta con traza.
6. Recurso que depende de versión incompatible es rechazado.
7. Borrar recurso referenciado exige estrategia de migración o bloqueo.
8. Cambiar una descripción no invalida materialización si la semántica no cambió.
9. Cambiar medida sí invalida versiones semánticas afectadas.
10. Borrar un alias no borra la identidad estable.

### B. Persistencia local

11. Archivo SQL y YAML guardados reaparecen idénticos tras reinicio.
12. Cambiar un archivo solo recompila recursos afectados.
13. Cambio de archivo/rename mantiene IDs a través de referencias.
14. Edición interrumpida no deja archivo publicado truncado.
15. Exportación usa orden determinista y no crea diffs espurios.
16. Git checkout a commit anterior reproduce definición revisada.
17. Índice SQLite borrado se reconstruye desde fuentes.
18. Un archivo duplicado produce diagnóstico de conflicto.
19. YAML con alias recursivo/expansión excesiva es rechazado.
20. Archivos en `../` o symlink externo no se importan sin permiso.

### C. Revisión y publicación

21. Publicación con `expected_revision` desactualizada falla por conflicto.
22. Draft inválido conserva versión publicada anterior.
23. Dos editores sobre el mismo recurso no pisan cambios.
24. Commit de release reúne exactamente el conjunto validado.
25. Se puede restaurar una revisión anterior sin mutarla.
26. Cache de metadata cambia al publicar revision nueva.
27. Revisión antigua no sirve para eludir RLS actual.
28. Publicación fallida mantiene manifest anterior.
29. Si cae el proceso entre staging y publish, se reconcilia sin recursos huérfanos publicados.
30. Un dashboard recibe metrics y model de la misma revisión lógica.

### D. Seguridad y portabilidad

31. Bundle nunca contiene contraseña ni token real.
32. `secret_ref` no se resuelve para actor no autorizado.
33. Usuario sin acceso no ve nombres protegidos en búsqueda.
34. Import reporta cambios potenciales de permisos.
35. Import de medida DAX no soportada indica `unsupported`.
36. Export de medida con dialecto DuckDB conserva marca de no-portabilidad si aplica.
37. Ossie adapter versión dev no se declara estable.
38. OpenLineage refleja un run real, no una inferencia de un SQL guardado.
39. Import con mismo ID y contenido distinto exige política de conflicto.
40. Export y reimport de un dashboard compatible conserva refs, layout y filtros.

### E. Multiusuario y datos físicos

41. Servidor PG y local FS validan con mismos esquemas y golden fixtures.
42. Release metadata apunta al snapshot físico elegido sin copiar sus filas.
43. Borrado de objeto físico no elimina automáticamente su historia metadata.
44. Schema detectado viejo muestra alerta de drift.
45. `last_observed_at` no cambia fingerprint del SQL fuente.
46. Concurrencia de compilación evita dos writes sobre mismo release.
47. El cierre de sesión no modifica definiciones ni release.
48. Dashboards enlazados a metric ID reciben actualización de versión sin copiar fórmulas.
49. Un caso de pago duplicado muestra lineage real y no transforma semántica por deducción.
50. Se ejecuta un rollback consistente después de error de cache invalidation, con alerta y reconciliación.

---

## 23. Errores que la plataforma debe evitar

1. Elegir `.sqlite`, `.db`, `.yaml` o `.json` como **la arquitectura** en vez de como un adaptador.
2. Forzar un único archivo que combine proyectos, usuarios, sesiones, credenciales, datos y dashboards.
3. Declarar Git-friendly a un SQLite binario solo porque puede guardarse en Git.
4. Duplicar fuentes de verdad autoritativas sin resolver conflictos.
5. Confiar en que `DROP/RENAME` sea atómico entre SQLite/PostgreSQL, DuckLake y filesystem.
6. Usar nombres de medidas en lugar de IDs como referencias durables.
7. Publicar un YAML parseable sin comprobar correspondencia física y permisos.
8. Tratar a estándares parciales como formato universal obligatorio.
9. Adoptar CRDT, Kafka o un grafo distribuido sin casos probados.
10. Mezclar el runtime del motor semántico con el almacenamiento de metadatos.

---

## 24. Decisiones tomadas y pendientes

### Recomendaciones firmes

- Contrato de recursos independiente del backend físico.
- Directorio/Git como opción local inicial; PostgreSQL como backend de equipo posterior.
- Separación de definiciones, estado operativo, snapshots físicos, sesiones y secretos.
- Recursos tipados con ID estable, revisiones inmutables, validación y publicación.
- Modelo semántico reutilizable sin SQL repetido en dashboards.
- Adaptadores hacia estándares; Apache Ossie es especialmente relevante para interoperabilidad semántica.
- Ninguna dependencia de una plataforma o archivo heredado.

### Decisiones por validar con prototipos

- Sintaxis autoral preferida para definiciones complejas: YAML, JSON, SQL sidecar o combinación.
- Límite entre JSON Schema público y los tipos Rust internos.
- Implementación concreta de CAS sobre directorio local/Windows.
- Modelo de búsqueda (índices PG/SQLite vs servicio dedicado) según volumen.
- Qué subset de Ossie se importa/exporta sin pérdida.
- GitOps estricto vs PostgreSQL autoritativo en instalaciones empresariales.
- Política de retención de releases y `data_snapshot_id`.

---

## 25. Índice de referencias reproducibles

### Código fuente analizado

1. Rill parser, resource kinds, reparse y diff (v0.90.4): https://github.com/rilldata/rill/blob/v0.90.4/runtime/parser/parser.go
2. Rill RepoStore (v0.90.4): https://github.com/rilldata/rill/blob/v0.90.4/runtime/drivers/repo.go
3. Lightdash DashboardModel versiones/transacciones (rama main): https://github.com/lightdash/lightdash/blob/main/packages/backend/src/models/DashboardModel/DashboardModel.ts
4. Lightdash ProjectModel / lock (main): https://github.com/lightdash/lightdash/blob/main/packages/backend/src/models/ProjectModel/ProjectModel.ts
5. Superset Database ORM (master): https://github.com/apache/superset/blob/master/superset/models/core.py
6. Superset Dashboard ORM/versionado (master): https://github.com/apache/superset/blob/master/superset/models/dashboard.py
7. Grafana Dashboard HTTP APIs (main): https://github.com/grafana/grafana/blob/main/pkg/api/dashboard.go
8. Grafana dashboard database migrations (main): https://github.com/grafana/grafana/blob/main/pkg/services/sqlstore/migrations/dashboard_mig.go
9. Apache Ossie core spec (main): https://github.com/apache/ossie/blob/main/core-spec/spec.md
10. OpenLineage OpenLineage.md (main): https://github.com/OpenLineage/OpenLineage/blob/main/spec/OpenLineage.md

### Estándares y documentación

11. Microsoft TMDL: https://learn.microsoft.com/en-us/analysis-services/tmdl/tmdl-overview
12. Apache Ossie repository: https://github.com/apache/ossie
13. OpenLineage specification: https://openlineage.io/docs/spec/
14. dbt manifest artifact: https://docs.getdbt.com/reference/artifacts/manifest-json
15. Superset import/export: https://superset.apache.org/docs/configuration/importing-exporting-datasources/
16. Grafana dashboards API: https://grafana.com/docs/grafana/latest/developers/http_api/dashboard/

**Reproducibilidad:** Rill está fijado al tag `v0.90.4`; la evidencia de Lightdash, Superset, Grafana, Ossie y OpenLineage proviene de ramas `main`/`master` consultadas el 10/10/2026. Antes de implementar o basarse en líneas exactas, fijar commit SHA específico y almacenar `source_sha` en las notas de investigación. Este documento distingue comportamiento constatado en archivos concretos, recomendaciones y pseudocódigo nuevo. No se afirma que ya exista un repositorio del nuevo producto.

---

## 26. Conclusión

**La plataforma no tendrá «un archivo de proyecto que gobierna toda su arquitectura».** Tendrá un **modelo lógico de recursos BI** independiente de la persistencia, y uno o más adaptadores coherentes con la forma de trabajo: archivos/Git para desarrollo, PostgreSQL para colaboración, SQLite como soporte local opcional.

Los estándares abiertos serán interfaces de intercambio, especialmente Apache Ossie para semántica y OpenLineage para ejecuciones/linaje, pero ninguno se impondrá como sustituto de todo el modelo de producto. Primero deben funcionar identidades, versiones, referencias, validación, publicación y rollback; las integraciones llegarán después.

**Fin exclusivo del Módulo 10.** No se inicia otro módulo hasta la siguiente instrucción. El objetivo de la serie es una plataforma BI nueva, todavía sin nombre, no una migración de software heredado.
