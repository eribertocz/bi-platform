# Plataforma BI abierta — Módulo 13: Runtime analítico, motores y conectividad Rust
## Auditoría del código fuente de DuckDB/duckdb-rs, DuckLake, Polars y Apache Arrow ADBC; decisiones de ejecución local, almacenamiento, concurrencia y Quack

**Edición:** 1.0 — 10 de octubre de 2026  
**Estado:** arquitectura recomendada, no implementación ejecutada ni benchmark.  
**Producto:** plataforma BI nueva y sin nombre; no depende de SQLVIZ, `.sqlviz` ni repositorios heredados.  
**Decisión asumida:** Rust como lenguaje del backend; DuckDB embebido inicialmente; DuckLake con catálogo PostgreSQL (sin catálogo SQLite); Parquet como almacenamiento físico; Arrow como transporte columnar; Polars opcional para transformaciones especializadas; ADBC en conectores si aporta compatibilidad; Quack opcional, no requerido.  
**Límites del módulo:** elección y ciclo de vida de runtimes, cliente DuckDB/Rust, transacciones DuckLake, interoperabilidad, ejecución/transformación y despliegue. No redefine la semántica BI (M04), la lógica de modelos/DAG (M02), la seguridad de autorización (M11) ni la API externa (M12): especifica los puertos y capacidades requeridos para integrarlos.  
**Método:** se inspeccionaron APIs y archivos fuente concretos accesibles de `duckdb-rs`, DuckLake, Polars y Arrow ADBC; se contrastaron con la documentación oficial. Ramas `main`/`master` son mutables: **fijar SHA y versiones exactas antes de implementar**. No se compiló un prototipo ni se midió rendimiento. Hipótesis y evidencias se distinguen en cada sección.

---

## 0. Dictamen ejecutivo — decisión adoptada

**Seleccionar un runtime embebido Rust + DuckDB (`duckdb-rs`) que consulte y escriba a un DuckLake cuyo catálogo de metadatos transaccionales se aloja en PostgreSQL.** DuckLake conserva filas en Parquet bajo un `DATA_PATH` explícito, inicialmente local para una instalación de nodo único, y potencialmente en almacenamiento de objetos para despliegue multiproceso/multihost. DuckDB puede conectarse en modo `:memory:` y adjuntar DuckLake: **no exige persistir un archivo `.duckdb`**.

### ¿Qué queda en cada pieza?

| Pieza | Naturaleza real | Papel aprobado | ¿Obligatoria al principio? |
|---|---|---|---|
| Rust (backend) | Lenguaje / runtime | API, planificación, coordinación, políticas, servicios y adaptadores | Sí |
| `duckdb-rs` | Cliente Rust, wrapper de la API C de DuckDB | Ejecutar SQL analítico **dentro del proceso**, retornar Arrow, administrar conexiones | Sí |
| DuckDB | Motor analítico implementado primordialmente en C++ | SQL, joins, agregaciones, scans, ejecución vectorizada | Sí |
| DuckLake | Extensión y formato de lago transaccional | Tablas y snapshots sobre archivos, catálogo externo | Sí en la vía de persistencia elegida |
| PostgreSQL | SGBD transaccional | **Catálogo DuckLake** y, separadamente, metadata operativa/BI | Sí para despliegue persistente |
| Parquet | Formato columnar de archivos | Filas físicas del lago | Sí |
| Apache Arrow | Formato / interfaces de intercambio | Resultados y transferencia tipada in-process | Sí, como contrato |
| Polars Rust | Motor DataFrame / LazyFrame de Rust | Transformaciones procedurales o especializadas | Opcional; no reemplaza SQL ni transacción DuckLake |
| ADBC Rust | API estándar + driver manager para acceso a bases | Conectores externos que dispongan de driver probado | Opcional por conector |
| Quack | Protocolo remoto DuckDB | Servidor de ejecución alternativo, aislamiento de procesos o clientes remotos | **No** |
| `.duckdb` | Formato nativo de almacenamiento de DuckDB | Debug/test y almacenamiento excepcional por caso | **No** como data lake principal |
| SQLite | SGBD embebido | Ninguna función obligatoria | **No** para catálogo DuckLake ni metadata operativa objetivo |

**Matiz esencial:** escoger Rust como backend **no convierte DuckDB en software 100 % Rust**; `duckdb-rs` une Rust y el motor nativo mediante FFI/C API. La solución es Rust-first e integra código C++ de DuckDB. Polars, en cambio, sí está implementado principalmente en Rust.

### Justificación cuantificable, no estética

- DuckDB embebido elimina una llamada de red por consulta y permite pruebas de la semántica directamente contra el mismo ejecutor.
- PostgreSQL coordina el catálogo transaccional cuando existen varias instancias, pero **no almacena las filas Parquet ni garantiza por sí solo acceso a los mismos archivos entre máquinas**.
- DuckLake permite transacciones y versiones, pero los conflictos y escenarios de concurrencia requieren pruebas; no confundir capacidad documentada con funcionamiento ilimitado bajo carga.
- Polars se incorpora cuando una transformación concreta supera en claridad, rendimiento o expresividad a SQL, con pruebas reproducibles.
- ADBC conecta fuentes externas si hay drivers maduros; no sustituye al motor SQL local ni obliga a instalar Quack.
- Quack queda como adaptador opt-in cuando el problema real es compartir una instancia DuckDB remota o separar cargas.

**No se aceptará una implementación que obligue a escribir datos analíticos en `.duckdb` para luego volver a exportarlos a DuckLake sin justificación.**

---

## 1. Anatomía correcta de los tres catálogos que suelen confundirse

### 1.1 Catálogo BI de producto

Guarda definiciones de modelos, métricas, roles, visualizaciones, dashboards, revisiones, trabajos y rutas de publicación. Pertenece a **Metadata Core** (M10). Nuestra instalación multiusuario elegirá PostgreSQL, con un esquema/base y roles diferentes a los del catálogo físico de DuckLake.

### 1.2 Catálogo DuckLake

Registra tablas físicas, manifiestos de archivos, transacciones y snapshots del lago. Pertenece a DuckLake; su implementación PostgreSQL exige la extensión de conectividad correspondiente y permisos apropiados. **No guardar definiciones de dashboards dentro de las tablas internas DuckLake**.

### 1.3 Catálogo temporal de la conexión DuckDB

Contiene la sesión, vistas temporales, extensiones cargadas, aliases de `ATTACH`, temporales del plan y funciones registradas. Puede residir completamente en memoria (`Connection::open_in_memory`). Su destrucción tras reiniciar no borra el catálogo PostgreSQL ni los Parquet ya comprometidos.

### 1.4 Separación recomendada en PostgreSQL

```text
Servidor PostgreSQL
├── db: bi_metadata         # M10: recursos, revisiones, trabajos, permisos
│   └── roles: bi_app_rw, bi_app_ro
└── db: bi_lake_catalog     # DuckLake: catálogo físico transaccional
    └── roles: lake_writer, lake_reader

DATA_PATH del DuckLake
└── /var/lib/bi/lake/     # single-host MVP, respaldado de forma consistente
    ├── .../*.parquet
    └── ... archivos administrados por DuckLake
```

También se pueden separar por servidores, pero no es necesario. Separar credenciales y ciclo de vida sí es obligatorio. PostgreSQL no equivale a una capa OLAP sustitutiva de DuckDB.

---

## 2. Cuatro arquitecturas candidatas, con elección

| Topología | Ejecutor | Persistencia | Ventaja | Restricción | Decisión |
|---|---|---|---|---|---|
| A. Rust + DuckDB in-process + DuckLake/Postgres | Local dentro del backend | DuckLake/Parquet | Una arquitectura, bajo overhead y control de tipos | Dependencia de proceso, aislamiento físico reducido | **Elegida MVP** |
| B. Rust + Quack remoto + `.duckdb` | Quack server | Archivo nativo administrado por servidor | Sesión SQL compartida y servicio remoto | Componente de red, protocolo aún con advertencias de estabilidad, locks nativos | Alternativa futura |
| C. Rust + múltiples DuckDB embebidos + DuckLake/Postgres | Varios workers/procesos | DuckLake/objetos compartidos | Escala de lectura y coordinación sobre un catálogo | Backoff, conflictos, paths/credenciales/consistencia del almacén | Fase posterior |
| D. Rust + solo Polars | Motor DataFrame | Parquet plano, sin transacción integrada de DuckLake | ETL vectorizado expresivo | No cubre el lenguaje SQL ni commits DuckLake por sí solo | **Descartada como runtime BI central** |

**Importante:** DuckLake no es un motor SQL ni un daemon al que conectarse por JDBC. DuckDB ejecuta SQL y la extensión DuckLake gestiona transacciones/catálogo/archivos. Quack tampoco es sinónimo de DuckLake: es un protocolo para DuckDB remoto. Estas dimensiones se pueden combinar, pero no deben aparecer como casillas equivalentes en la configuración.

### Reglas de elección automática

1. Solicitud semántica agregada → DuckDB SQL embebido.
2. Materialización SQL de modelo → DuckDB SQL + DuckLake writer.
3. Transformación declarada explícitamente como `PolarsPipeline` → Polars Rust, con salida Arrow/Parquet, y commit/publish por DuckLake.
4. Fuente externa con ADBC validado → driver ADBC como `SourceConnector` (M01), devolviendo lotes Arrow; sin declarar automáticamente DuckLake como destino.
5. Necesidad real de servidor DuckDB remoto/aislamiento → posible Quack, tras superar pruebas de protocolo, autenticación y manejo de tipos.

---

## 3. Evidencia: `duckdb-rs` — API Rust real

**Código inspeccionado:** [`crates/duckdb/src/statement.rs`](https://github.com/duckdb/duckdb-rs/blob/main/crates/duckdb/src/statement.rs) y [`appender/arrow.rs`](https://github.com/duckdb/duckdb-rs/blob/main/crates/duckdb/src/appender/arrow.rs). Documentación: [cliente Rust](https://duckdb.org/docs/current/clients/rust/overview), [conexiones](https://duckdb.org/docs/current/clients/rust/connecting), [manejo de resultados](https://duckdb.org/docs/current/clients/rust/result_handling).

### 3.1 Hechos observados en el código

- `Statement::query_arrow()` devuelve Arrow, pero su ejecución/materialización puede consumir memoria proporcional al resultado. La documentación del fuente apunta a `stream_arrow()` para consumir lotes perezosamente.
- `Statement::stream_arrow()` no promete memoria constante para **todos** los operadores DuckDB: ciertas operaciones materializan internamente, y la documentación del código advierte que su iterador puede causar pánico ante fallos de fetch o de conversión Arrow **después** del comienzo de la ejecución.
- Bajo feature `polars`, `Statement::query_polars()` devuelve iterador de `DataFrame` de Polars. Esto **no** significa que toda ejecución deba pasar por Polars, ni garantiza compatibilidad binaria entre versiones de Arrow y Polars.
- Prepared statements permiten enlazar parámetros tipados: defender el SQL semántico contra interpolación de literales. Los identificadores de tablas/columnas requieren validación aparte.
- `Appender::append_record_batch()` permite la ingesta de `RecordBatch` a tablas DuckDB; el contrato requiere esquema/columnas compatibles.
- `Connection` es `Send`, pero no `Sync`: puede moverse a un hilo, no compartirse simultáneamente. Un `Statement` no es `Send` ni `Sync`.
- Existe `Connection::try_clone()` para disponer de otra conexión hacia la misma instancia subyacente y `interrupt_handle()` para cancelar una consulta desde otro hilo.

### 3.2 Decisiones de ingeniería que siguen de lo anterior

- **No compartir `Connection` bajo un `Mutex` global para todas las consultas**: diseñar pools acotados de conexiones por worker y controlar los límites de CPU/memoria.
- **No hacer `collect()` de todos los lotes sin límite** en resultados BI o exportaciones.
- Para streaming seguro con errores recuperables, validar la ruta de la API específica y encapsular la iteración; un iterador que pueda entrar en pánico no puede exponerse directamente en endpoints empresariales sin control.
- Cancelación por identificador de consulta con ownership de conexión; no reutilizar una conexión antes de confirmar que la consulta anterior terminó o fue interrumpida.
- Versionar conjuntamente binarios DuckDB, crate Rust y extensiones `ducklake`/`postgres`; no asumir que `cargo update` actualiza todos con compatibilidad garantizada.

### 3.3 Ejemplo mínimo, orientativo

```rust
// Ejemplo conceptual; fijar versiones del crate y extensiones antes de compilar.
use duckdb::{Connection, Result};

fn run() -> Result<()> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("INSTALL postgres; LOAD postgres; INSTALL ducklake; LOAD ducklake;")?;
    // ATTACH debe construirse desde configuración validada y secretos administrados.
    // Nunca concatenar host/clave arbitrarios proporcionados por un viewer.
    Ok(())
}
```

**Advertencia de seguridad:** permitir `INSTALL` y cargar extensiones bajo cualquier usuario de dashboard es inseguro. Su instalación debe corresponder al bootstrap administrado por el sistema; viewers nunca reciben SQL administrativo sobre la conexión.

---

## 4. Evidencia: DuckLake + PostgreSQL

**Fuentes:** [cómo conectar](https://ducklake.select/docs/stable/duckdb/usage/connecting), [elegir catálogo PostgreSQL](https://ducklake.select/docs/stable/duckdb/usage/choosing_a_catalog_database), [implementación DuckLake](https://github.com/duckdb/ducklake), [concurrencia DuckDB](https://duckdb.org/docs/current/connect/concurrency).

### 4.1 Forma real de conexión

```sql
-- Ejemplo ilustrativo de SQL de administración; sin contraseñas en el archivo.
INSTALL postgres;
LOAD postgres;
INSTALL ducklake;
LOAD ducklake;

-- Se asume que la identidad/credenciales PostgreSQL ya fueron aprovisionadas.
ATTACH 'ducklake:postgres:dbname=bi_lake_catalog host=localhost' AS analytics
  (DATA_PATH '/var/lib/bi/lake/');

-- Operaciones de ejemplo sobre el catálogo adjunto.
CREATE SCHEMA IF NOT EXISTS analytics.gold;
CREATE TABLE analytics.gold.demo AS
SELECT 1::BIGINT AS id, 100.25::DECIMAL(18, 2) AS amount;
SELECT * FROM analytics.gold.demo;
```

El `DATA_PATH` es **ubicación de archivos de datos**, no una ruta `.duckdb`. Si el catálogo se reabre desde otra máquina, esa segunda máquina debe poder resolver/acceder al mismo objeto o directorio con semántica compatible. Una ruta local idéntica en distintas máquinas **no garantiza** compartir archivos.

### 4.2 Qué ocurre cuando termina el proceso Rust

- Desaparecen conexión, recursos en memoria y vistas temporales.
- Permanecen los metadatos comprometidos en PostgreSQL y archivos Parquet gestionados por DuckLake.
- Una nueva conexión Rust puede reanexar el mismo catálogo y leer las tablas publicadas.
- Un fallo entre creación de archivos y confirmación de catálogo puede dejar temporales/archivos huérfanos: debe existir procedimiento de recuperación/limpieza, sin borrar por intuición archivos aún referenciados.

### 4.3 Concurrencia: promesa y límite real

DuckDB documenta que múltiples procesos DuckDB pueden coordinar lecturas y escrituras mediante catálogo DuckLake PostgreSQL. Pero conflictos optimistas siguen siendo posibles. Hubo problemas públicos sobre reintentos de escrituras concurrentes y sobre escenarios de conflicto: revisar estado/resolución para la versión fijada. En este informe **no damos por probado el éxito de N escritores simultáneos ni que toda clase de actualización sea libre de conflictos**.

Fuentes de ingeniería: [`ducklake_transaction.cpp`](https://github.com/duckdb/ducklake/blob/main/src/storage/ducklake_transaction.cpp), [issue 233 de conflictos/reintentos](https://github.com/duckdb/ducklake/issues/233), [issue 1215 de coherencia de borrados](https://github.com/duckdb/ducklake/issues/1215). Los issues son evidencia de escenarios a probar, **no prueba de que permanezcan abiertos** en la versión elegida.

### 4.4 Política de escritura inicial

- **Un solo coordinador lógico de publicaciones/materializaciones** por dataset o conjunto de particiones afectadas.
- Lecturas simultáneas permitidas, con snapshots consistentes y revisiones declaradas.
- Ingesta incremental idempotente; en caso de conflicto, reintento acotado con backoff+jitter y comprobación de que el lote no se aplicó.
- Evitar que dos materializadores sobrescriban la misma tabla/partición sin protocolo explícito.
- Monitorizar conflictos, número de reintentos, latencia commit, archivos huérfanos y versiones servidas.

---

## 5. Almacenamiento local: PostgreSQL no obliga a S3

### Perfil A: una máquina, desarrollo/primera producción controlada

- Un PostgreSQL local o de red como catálogo.
- `DATA_PATH` en filesystem local accesible a **un solo host de ejecución**.
- Backend Rust con DuckDB embebido en el mismo host, worker de escritura coordinado y pools limitados de lectura.
- Backups consistentes: PostgreSQL **y** árbol de Parquet (no basta un `pg_dump`).

### Perfil B: más de un host o despliegue elástico

- PostgreSQL de catálogo compartido.
- `DATA_PATH` en object storage S3-compatible/GCS/Azure compatible con la versión probada de DuckLake.
- Credenciales y permisos controlados por rol, y workers capaces de acceder a los mismos objetos.
- Pruebas explícitas de concurrencia, snapshots, latencias, borrados y consistencia.

**No recomendar un directorio NFS arbitrario como base multiproceso sin ensayos específicos de semántica de locking, cachés y fallos.** Los archivos `.duckdb` nativos tampoco se deberían compartir para escrituras multihost directamente.

---

## 6. ¿ADBC es más moderno que `duckdb-rs`? Comparación correcta

**No son productos sustitutos.** `duckdb-rs` es un cliente especializado del motor DuckDB mediante API C; ADBC es **una API genérica de acceso a bases y un modelo de drivers** orientado a Arrow.

| Eje | `duckdb-rs` | ADBC Rust |
|---|---|---|
| DuckDB embebido | API directa y completa del cliente | Posible vía driver nativo, si existe/empaqueta correctamente |
| Gestión explícita de extensiones DuckDB | Sí, via SQL y API | Depende del driver |
| Consultas `query_arrow` / `stream_arrow` | Sí; matices según API | Modelo Arrow común, sujeto a driver |
| Soporte genérico múltiples bases | No, salvo extensiones DuckDB / código adicional | Objetivo principal |
| ABI/dependencias | DuckDB C/C++ y crate Rust | FFI/driver manager y bibliotecas dinámicas por driver |
| Cancelación, tipos, transacciones | Semántica DuckDB y APIs disponibles | **Se prueban driver por driver**, sin asumir universalidad |
| Usarlo para la ejecución BI central | **Sí** | No en MVP |
| Usarlo para conectores externos | Si extensión/postgres_scanner lo justifica | **Sí, cuando driver está probado** |

**Fuente primaria:** [Arrow ADBC Rust](https://arrow.apache.org/adbc/main/rust/quickstart.html), [`rust/core/src/lib.rs`](https://github.com/apache/arrow-adbc/blob/main/rust/core/src/lib.rs), [ADBC Rust en repositorio](https://github.com/apache/arrow-adbc/tree/main/rust), [ADBC DuckDB driver](https://arrow.apache.org/adbc/current/driver/duckdb.html). La página de driver DuckDB puede enumerar clientes de otros lenguajes y no garantizar que exista un paquete Rust ya operativo; probar carga del driver C, símbolos, versión y plataforma es imprescindible.

### 6.1 Conectores iniciales de datos

- Parquet y CSV locales/remotos: preferir scanners nativos DuckDB y/o Polars cuando la semántica sea adecuada.
- PostgreSQL operacional: priorizar el escáner/extensión `postgres` DuckDB para lecturas federadas autorizadas, con límites y validación de pushdown; ADBC PostgreSQL cuando un pipeline Arrow específico lo justifique.
- Otras fuentes: adaptador `SourceConnector`, con ADBC si hay driver confiable, y fallback especializado cuando no.
- **No hacer que la disponibilidad de una conexión remota determine automáticamente el motor de todas las transformaciones BI.**

### 6.2 Riesgos ADBC

Carga dinámica de drivers arbitrarios, incompatibilidades ABI/Arrow, empaquetado Windows/Linux, credenciales, semántica de transacciones y ausencia de paridad entre drivers. Registro allowlist de drivers; manifiestos firmados o verificados y compatibilidad testeada en CI.

---

## 7. Polars Rust: dónde sí y dónde no

**Fuentes:** [código Polars LazyFrame](https://github.com/pola-rs/polars/blob/main/crates/polars-lazy/src/frame/mod.rs), [scans Parquet Rust](https://docs.pola.rs/user-guide/io/parquet/), [integración Arrow](https://docs.pola.rs/user-guide/misc/arrow/).

### 7.1 Casos correctos

- Transformación de columnas compleja o especializada con Expressions/LazyFrames.
- Construcción de reglas analíticas reutilizables expresadas en una API de DataFrames Rust.
- Lectura selectiva de Parquet y pipelines que aprovechan lazy optimization, pushdown o streaming según el caso.
- Ingestión/conversión de datos que proceden de conectores ADBC a lotes Arrow, sin pasar por Pandas ni Python.

### 7.2 Casos que NO justifican introducir Polars

- Un `GROUP BY` o JOIN ya resuelto por DuckDB en un modelo SQL.
- Las consultas de métricas/dashboards para las que ya existe SQL DuckDB; duplicar la semántica produce divergencias.
- Usarlo solamente porque es Rust: comparar rendimiento y coste de serializar/convertir buffers.
- Escribir Parquet suelto directamente en la ruta de DuckLake **pretendiendo que eso crea una transacción DuckLake**. No la crea.

### 7.3 Ruta correcta de publicación de Polars

```text
InputDataset / Arrow / Parquet
   ↓
Polars LazyFrame (transformación tipada)
   ↓
RecordBatches Arrow / staging controlado
   ↓
DuckDB import / staging con tipos validados
   ↓
DuckLake writer + commit transaccional
   ↓
PublishedDatasetRef / snapshot
```

Si se escribe Parquet de staging, **ese archivo no es todavía una tabla DuckLake publicada**. El módulo Materialization debe leerlo/ingestarlo mediante un camino soportado y registrar commit. El pipeline no debe modificar por su cuenta tablas internas del catálogo DuckLake.

### 7.4 La interoperabilidad no es siempre zero-copy

El contrato público será Arrow `Schema + RecordBatch stream`, pero los detalles Arrow2/Arrow-Rust, tipos anidados, diccionarios, zonas horarias, decimal, UTF-8 y offsets pueden requerir conversiones o copias. Medir bytes copiados y vigilar el árbol de dependencias; la existencia de Arrow no garantiza coste cero universal.

### 7.5 ¿Polars como motor de modelos de primera clase?

**Sí, potencialmente**, mediante una capacidad `PipelineDefinition::Polars` que produce un dataset con el mismo contrato de salida que SQL; no como lenguaje inventado ni ejecutor obligatorio para todas las consultas. En el MVP mantener una sola ruta de producción DuckDB SQL y preparar interfaces estables; después activar Polars mediante pruebas funcionales y comparativas.

---

## 8. Apache Arrow: contrato de datos, no base ni motor

Propuesta `ArrowBatchStream` con:

- `schema`: nombres, tipos lógicos, nullability, metadata mínima; prohibir truncar precisión decimal.
- `batches`: lotes acotados, lectura incremental y backpressure.
- `lineage`: identificador de consulta/modelo, versión física y snapshot.
- `context`: principal y scope de política (solo referencias/fingerprints; no credenciales).
- `limits`: tamaño máximo por lote, filas/bytes máximos y deadline.
- `lifetime`: ownership explícito de buffers, liberación en cancelación/error y tipos compatibles.

**Versionado Arrow:** no permitir upgrades arbitrarios de crates Arrow/Polars/ADBC sin CI que pruebe ABI e intercambio de tipos. Las estructuras de SDK hacia redes (REST/Flight SQL) se versionan aparte.

---

## 9. Quack: cuándo ayuda y por qué no forma parte del núcleo

**Fuentes:** [extensión Quack](https://duckdb.org/docs/current/core_extensions/quack), [concurrencia DuckDB](https://duckdb.org/docs/current/connect/concurrency).

Quack transforma DuckDB en un servicio remoto accesible mediante un protocolo propio. La documentación consultada aún advierte estado beta/experimental en la línea 1.5 y posibles cambios en protocolo y funciones. Su versión futura puede mejorar; no basar hoy la arquitectura de producto en una promesa.

**Casos justificables:**

1. Interfaz BI y servidor SQL en procesos/nodos distintos por aislamiento.
2. Clientes externos que requieren protocolo DuckDB remoto.
3. Reutilización de un servidor central especializado para muchas solicitudes.
4. Migración temporal desde una topología ya existente basada en Quack.

**Casos que NO requieren Quack:**

- Una sola API Rust y DuckDB en el mismo proceso.
- Consultar DuckLake con catálogo PostgreSQL.
- Leer archivos Parquet en servidor.
- Ejecutar Polars Rust.
- Utilizar Apache Arrow entre módulos del mismo proceso.
- Concurrencia de escritores DuckLake gestionada mediante catálogo PostgreSQL y diferentes instancias DuckDB.

**Nota:** Quack y DuckLake pueden coexistir en una solución híbrida; no es una prohibición tecnológica. El adaptador Quack solo se habilitará con tests equivalentes al embebido para seguridad, cancelación, tipos y resultados.

---

## 10. Arquitectura final — componentes lógicos

```text
Svelte UI / SDK / API client
           │
       HTTP API (Rust)
           │
┌─────────────────────────────────────────────────┐
│                 BI Backend Rust                 │
│                                                 │
│  Security / Semantic Core / Model Planner       │
│                     │                           │
│              Query Coordinator                 │
│                     │                           │
│              Runtime Router                    │
│               ┌─────┴──────┐                    │
│               │            │                    │
│       DuckDB InProcess  Polars Runtime          │
│        (duckdb-rs)    (capacidad opcional)       │
│               │            │                    │
│               └──Arrow/Staging──┘               │
│                     │                           │
│               Materializer                      │
│                                                 │
│   Source Adapters: Parquet, Postgres, ADBC...    │
└─────────────────────┬───────────────────────────┘
                      │
           DuckLake extension
                 ┌────┴─────┐
                 │          │
     PostgreSQL catalog   Parquet objects
                 │          │
                 └────┬─────┘
                      │
        Published dataset / snapshot

Quack (opt-in): alternativa de ejecución remota detrás de RuntimeRouter,
no dependencia de las definiciones, seguridad ni metadatos.
```

**Aclaración de diagrama:** materialización de un `PolarsPipeline` necesita pasar por el API de escritura DuckLake del motor apropiado. **Polars no realiza commits DuckLake por sí mismo**.

---

## 11. Contratos propuestos en Rust (pseudocódigo de arquitectura)

Estos tipos ilustran los límites de responsabilidad; **no son firmas confirmadas de crate ni fragmentos comprobados compilando**.

```rust
pub enum RuntimeKind {
    DuckDbEmbedded,
    Polars,
    QuackRemote, // feature opcional
}

pub enum StorageTarget {
    Live,
    View,
    DuckLake { catalog_ref: CatalogRef, schema: String, table: String },
    ExternalParquet { location_ref: LocationRef },
}

pub struct QueryExecutionRequest {
    pub request_id: QueryId,
    pub principal: PrincipalContext,
    pub compiled_sql: Option<PreparedSql>,
    pub model_pipeline: Option<PipelineRef>,
    pub runtime_hint: RuntimeHint,
    pub version: Option<DatasetVersionRef>,
    pub deadline: Deadline,
    pub limits: ResourceBudget,
}

pub trait AnalyticalRuntime {
    fn capabilities(&self) -> RuntimeCapabilities;
    fn execute(&self, request: QueryExecutionRequest) -> Result<ArrowBatchStream>;
    fn cancel(&self, request_id: QueryId) -> Result<()>;
}

pub trait DatasetPublisher {
    fn prepare_candidate(&self, request: MaterializationRequest) -> Result<CandidateRef>;
    fn validate_candidate(&self, candidate: &CandidateRef) -> Result<QualityReport>;
    fn publish(&self, candidate: CandidateRef, expected: PublishedVersionRef)
        -> Result<PublishedVersionRef>;
}
```

En implementación real preferir `async` solo donde el backend I/O lo requiera; las consultas DuckDB embebidas pueden ser bloqueantes. Un pool de workers CPU ejecuta consultas y comunica resultados a la capa async mediante colas acotadas. No dejar que un servidor `tokio` procese scans pesados en los hilos de I/O.

### 11.1 Versiones y capacidades que cada adaptador debe declarar

`RuntimeCapabilities`: `supports_sql`, `supports_arrow_stream`, `supports_cancel`, `supports_parameters`, `supports_transaction`, `supports_external_table`, `supports_ducklake_write`, `supports_snapshot_pin`, `supports_polars_lazy`, `max_concurrency`, `native_decimal_range`, `timezone_semantics`, `extension_policy`.

El planner nunca asigna `PolarsRuntime` para `CREATE TABLE DuckLake` solo porque el modelo produzca un `LazyFrame`; debe crear una etapa de publicación ejecutada por un writer compatible.

---

## 12. Modo local — qué instalar realmente

**Configuración seleccionada para desarrollo/pequeña empresa:**

```text
1. Rust backend BI (un proceso)
2. PostgreSQL (servidor local; dos bases lógicas o esquemas separados)
3. Extensión DuckLake y extensión postgres en DuckDB
4. Directorio administrado de Parquet
5. Frontend estático Svelte 5
```

**No requiere:** Quack, Redis, Kafka, servicio Flight SQL, servidor Polars, SQLite, archivo `.duckdb`, clúster Kubernetes ni objeto S3 desde el primer prototipo.

**Consecuencia aceptada:** con PostgreSQL la instalación no es un único ejecutable totalmente autónomo. Es la decisión consciente a favor de una evolución multiusuario y catálogo común. Desarrollo local puede usar un PostgreSQL administrado por Podman/Docker Compose; no confundir contenedor con requisito de producción.

### 12.1 Dos catálogos diferentes no implican dos clústeres PostgreSQL

Un mismo servidor PostgreSQL puede manejar `bi_metadata` y `bi_lake_catalog` con permisos, usuarios y backups diferenciados. Si el uso crece, se pueden separar físicamente.

### 12.2 Un solo nodo vs varios

En un solo nodo los archivos locales son válidos para DuckLake. Para múltiples hosts usar un `DATA_PATH` realmente compartido y soportado, preferiblemente object storage según despliegue; los catálogos PostgreSQL y permisos de lectura no distribuyen físicamente Parquet.

---

## 13. Modelo de conexiones y concurrencia

### 13.1 Conexiones

- Inicializar DuckDB con extensiones y configuración controladas.
- Pool pequeño de conexiones de consulta: no usar una conexión simultáneamente en hilos múltiples.
- `Connection::try_clone()` cuando corresponda para compartir instancia embebida; probar alcance de `ATTACH`, secretos, settings y transacciones por conexión.
- Sesiones con aliases estables y policy context reconstruible; configuración consistente por worker.
- Separar rol/credencial lectora y escritora; no permitir SQL arbitrario de viewer contra conexión de escritura.

### 13.2 Aislamiento de tareas

- BI interactivo: cola corta, prioridades, límites de filas/bytes/CPU/memoria y cancelación.
- Model refresh: cola de mayor duración; cuota independiente y límite de escrituras.
- Exportación: streaming, con su propio límite y TTL de artefactos.
- Sincronización del catálogo: snapshot/versionado; jamás leer una publicación a medio commit.

### 13.3 Cancelación

La API de Rust ofrece `interrupt_handle()` en la versión inspeccionada. Al cancelar:

1. Marcar `request_id` como `cancelling`.
2. Interrumpir la consulta asociada a la conexión.
3. Esperar confirmación o fallo final, registrarlo.
4. Descartar lotes atrasados y liberar buffers.
5. Restaurar configuración segura de la conexión o descartarla del pool cuando su estado sea incierto.

---

## 14. Transacciones y publicación: no mezclar tres atomicidades

1. **Transacción PostgreSQL del catálogo DuckLake:** administra la coherencia del estado del lago.
2. **Publicación lógica BI (M10):** cambia qué versión/snapshot está visible para un dashboard.
3. **Transferencia de resultados Polars/Arrow a staging:** puede no ser transaccional.

La arquitectura debe coordinar estos tres estados mediante un flujo idempotente:

```text
RUN_CREATED
  → INPUT_SNAPSHOT_RESOLVED
  → CANDIDATE_BUILDING
  → CANDIDATE_READY
  → VALIDATING
  → CANDIDATE_VALID
  → LAKE_COMMITTED (identificador snapshot físico)
  → BI_POINTER_PUBLISHED
  → CACHE_INVALIDATED
  → RUN_COMPLETED
```

Si falla después del commit físico pero antes de la publicación BI, **reintentar publicación de forma idempotente**, no volver a cargar filas sin control. Si falla la publicación por conflicto de revisión, no forzar el puntero. Registrar artefactos huérfanos para GC seguro.

### 14.1 No prometer una transacción única entre dos PostgreSQL distintos

Si `bi_metadata` y `bi_lake_catalog` son bases de datos separadas, **una transacción SQL simple no abarca ambas**. Utilizar state machine, outbox o saga ligera con compensación/idempotencia. No asumir ACID distribuido mágico.

### 14.2 Watermarks e incrementales

Un watermark por remesa/mes no basta si hay correcciones retroactivas. Conservar `source_revision`, `ingest_run_id`, `partition_key`, `last_successful_commit`, ventanas de relectura, claves de deduplicación y reglas de recomputación. La lógica de dependencia/incremental completo corresponde al Model Engine M02; aquí definimos garantías del runtime y publicación.

---

## 15. Prueba de corrección con facturación y cobranza

Dataset ilustrativo:

```text
facturacion: factura F1 = 100.00, factura F2 = 200.00
cobranza:    F1 pago A = 40.00, F1 pago B = 60.00, F2 pago C = 50.00
```

Un JOIN directo `facturacion × cobranza` repite F1 y puede inflar facturación a 400.00 en lugar de 300.00. El runtime no corrige el fanout por sí mismo: el motor semántico debe planificar a la granularidad adecuada. El runtime **sí** debe preservar `DECIMAL` y producir los mismos resultados independientemente de si recibe datos Arrow, Parquet o una tabla DuckLake.

Validar en distintos caminos:

- DuckDB SQL directo → total facturación 300.00; cobranza 150.00.
- ArrowBatch → DuckDB → mismo resultado sin redondear.
- Polars transformation (si se habilita) → materialización DuckLake → mismo resultado.
- Repetir ingreso de lote idéntico → sin duplicados por idempotencia.
- Dos lectores concurrentes durante un refresh → cada uno consume un snapshot completo, nunca mitad anterior/mitad nuevo.

---

## 16. Tipos críticos: condiciones de aceptación

| Tipo | Riesgo | Regla de implementación |
|---|---|---|
| `DECIMAL(18,2)`/mayor | Convertir a `f64`, pérdida de centavos | Tipo decimal Arrow/DuckDB y serialización exacta |
| `BIGINT/UBIGINT` | JS pierde precisión por >2^53 | JSON como string o serialización tipada, schema junto a valor |
| `TIMESTAMP/TIMESTAMPTZ` | Diferencia zonas locales | Roles temporales y zona declarada |
| `DATE` | Cambio de día al UTC | No transformar Date a midnight-UTC implícito |
| Texto de ID `"001"` | Ceros iniciales perdidos | Texto end-to-end |
| `NULL` | Interpretar cadena `"null"` o 0 | Null bitmap separado |
| `LIST`/`STRUCT`/`MAP` | Conversión entre Arrow y Polars | Capability + test específico; rechazo explícito si no soportado |
| `UUID` | Conversión incorrecta / colisión | Tipo/literal verificable |
| `INTERVAL` | Unidades ambiguas | Canonicalizar tipos de duración e intervalos |

La UI de gráficos consume formatos numéricos controlados sin convertir las medidas monetarias originales en floats cuando esa conversión afecte exactitud.

---

## 17. Seguridad de runtime y extensiones

- Permitir únicamente extensiones necesarias y aprobadas: `ducklake`, `postgres`, `httpfs` u otras si el caso las justifica.
- No conceder a usuarios de dashboards la posibilidad de `INSTALL`, `LOAD`, `ATTACH`, leer archivos de servidor, crear secrets o ejecutar comandos administrativos.
- Las conexiones para consultas semánticas tienen permisos de lectura de catálogo y objetos de datos apropiados.
- RLS/CLS y ABAC se aplican antes de compilar SQL, en el motor semántico M04/M11; las consultas directas de autores requieren otro rol y auditoría diferenciada.
- Nunca interpolar cadenas arbitrarias para host, path, tabla o esquema. Validar identificadores, parámetros y allowlists.
- ADBC: bibliotecas externas son código ejecutable; una conexión de ADBC no es simplemente una URL inocua.
- Las credenciales no entran en metadatos Git, trazas, dashboards, Arrow schema metadata ni respuestas de errores.
- Configurar ubicaciones de temporary spill, permisos de archivo, límites de memoria y limpieza posterror.

---

## 18. Observabilidad y gestión de recursos

Métricas mínimas:

- Tiempo de compilación semántica (M04).
- Tiempo en cola del runtime.
- Tiempo de ejecución DuckDB/Polars.
- Tiempo de primer lote Arrow y throughput (rows/s, bytes/s).
- Memoria máxima del proceso y memory/spill por workload.
- Filas/bytes devueltos y descartados.
- Conexiones DuckDB activas / disponibles.
- Intentos/latencia de conexión PostgreSQL catálogo.
- Conflictos/retries del commit DuckLake, latencia de publicación.
- Refresh, cardinalidad, tamaño Parquet, snapshots retenidos.
- Versiones concretas: DuckDB, `duckdb-rs`, `ducklake`, `postgres` extension, Arrow y Polars.

No introducir un proxy/servicio nuevo por un problema de latencia hasta separar tiempos de scan, planificación, materialización, transporte, UI y scheduling.

---

## 19. Ajustes de rendimiento recomendados

### 19.1 DuckDB

- Controlar `threads`, `memory_limit`, `temp_directory` y presupuesto de concurrencia; no asumir que cada conexión puede usar todos los cores sin interferencia.
- Evaluar `EXPLAIN ANALYZE` sobre SQL real; usar Parquet predicate pushdown y estadísticas cuando apliquen.
- Evitar que exportaciones completas compitan sin prioridad con consultas de dashboard.
- Streaming Arrow en lugar de `collect()` para resultados grandes; medir retención de buffers.

### 19.2 DuckLake / Parquet

- Elegir particiones con carga real, no sobreparticionar por todos los filtros posibles.
- Controlar tamaño de archivos y compactaciones; archivos diminutos multiplican requests/metadata.
- Mantener rutinas de GC/retención de snapshots coordinadas con dashboards publicados.
- Ubicar almacenamiento de objetos cercano al ejecutor si se escala fuera de una máquina.

### 19.3 Polars

- Usar `LazyFrame`/Expressions y scans con pushdown; no hacer `collect()` temprano por comodidad.
- Documentar si un plan dispone realmente de ejecución streaming; no darlo por garantizado para cualquier operación.
- Comparar costo total SQL vs Polars incluyendo Arrow ↔ Polars ↔ DuckDB y confirmación DuckLake.

---

## 20. Benchmarks que decidirán si Polars merece ser primera clase

Mismo host, misma CPU, mismos archivos, mismo input/snapshot y límites equivalentes:

| Caso | Ruta A | Ruta B | Criterio decisor |
|---|---|---|---|
| Parse CSV/Parquet y casting | DuckDB SQL | Polars Lazy | Tiempo + exactitud + memoria + fallos |
| Regla compleja de deduplicación | DuckDB Window SQL | Polars Expressions | Velocidad y legibilidad mantenible |
| Agrupaciones y joins facturación | DuckDB SQL | Polars Lazy | Plan y costo total incluyendo publicación |
| UDF/transformación especializada | DuckDB SQL/UDF | Polars Rust | Expresividad y coste de ejecución |
| Query interactiva + filtros | DuckDB | Polars (si aplica) | P95 y coste de servir Arrow |
| Escritura a DuckLake | DuckDB nativo | Polars→DuckDB→DuckLake | Commit seguro y throughput |

Medir datasets de tamaño 1M, 12M, 60M y 120M filas y añadir un escalón mayor si el hardware lo permite. Registrar tipos, particiones, repeticiones, número de workers, caché fría/caliente y planes de ejecución. No anunciar ganadores sin medidas.

---

## 21. Estructura de módulos físicos razonable

Monolito modular Rust, no quince microservicios:

```text
bi-platform/
  apps/
    server/                # API Rust + arranque y composición
    studio/                # Svelte 5
  crates/
    bi-contracts/          # tipos de datos y recursos públicos
    bi-metadata/           # interfaces de repositorio y publicación (M10)
    bi-runtime/            # contratos de ejecución y capabilities (M13)
    bi-duckdb/            # embebido duckdb-rs, conexiones y cancelación
    bi-ducklake/          # attachment, catalog refs, writer, snapshots
    bi-polars/            # feature opcional, sólo si se valida
    bi-sources/           # lectores nativos/ADBC bajo M01
    bi-models/            # planner/DAG M02
    bi-semantic/          # medidas/AST/seguridad M04
    bi-serving/           # queries/colas/caché M05
    bi-security/          # actor/políticas M11
```

**No obligatorio crear todos los crates el primer día.** Iniciar con `bi-runtime`, adaptador `bi-duckdb` y `bi-ducklake`; separar módulos físicos cuando los límites estén testeados. Los contratos compartidos no pueden introducir dependencia circular del runtime a visualización.

---

## 22. CI/CD y versionado de dependencias

- Pin versiones específicas `duckdb-rs`, DuckDB engine, extensión DuckLake, PostgreSQL extension, Polars, Arrow y ADBC; generar lockfile reproducible.
- Matriz Linux/Windows si se distribuye para ambos; probar cargas de extensiones, símbolos FFI, rutas de archivos y formato de tipos.
- CI con PostgreSQL real (contenedor, no mock) y DuckLake + Parquet real.
- Fijar fixtures de actualización de esquema y regresión de catálogo.
- Validar restauración desde backup de PostgreSQL **más** Parquet y reanexado desde proceso nuevo.
- Compatibilidad entre versión antigua y nueva: abrir catálogo en modo seguro, migraciones explícitas, nunca upgrade destructivo silencioso.

**Prohibición:** tests que solo comprueben que el método Rust retorna `Ok(())` no sustituyen comprobar igualdad de filas/medidas tras reabrir el lago.

---

## 23. Plan de entrega incremental del Módulo 13

### Etapa A — Spike técnico mínimo

1. Instalar PostgreSQL; crear catálogo DuckLake.
2. Compilar Rust usando `duckdb-rs` fijado; instalar/load `ducklake` y `postgres`.
3. Abrir `Connection::open_in_memory()`.
4. Hacer `ATTACH` a DuckLake PostgreSQL con `DATA_PATH` controlado.
5. Crear tabla, insertar registros `DECIMAL`, consultar por `query_arrow`, cerrar el proceso.
6. Relanzar proceso y comprobar persistencia y valores.

**Criterio de salida:** ningún archivo `.duckdb` obligatorio, PostgreSQL conserva catálogo, Parquet existe, tipos exactos.

### Etapa B — Concurrencia y seguridad

1. Pool acotado de lectores Rust; escritor coordinado.
2. SQL preparado, cancelación, streaming Arrow, memory/temp limits.
3. Dos lectores y un escritor; reintentos y consistencia de snapshot.
4. Roles PostgreSQL reader/writer y credenciales de secret manager.

**Criterio de salida:** nunca hay lectura parcial y los límites no se evaden.

### Etapa C — Model + Semantic vertical slice

1. Dos modelos SQL dependientes con política de storage DuckLake.
2. Quality gate y revisión publicada.
3. Una SemanticQuery filtrada con tipos exactos.
4. Cache key con snapshot+policy+semantic revision.

**Criterio de salida:** mismo número semántico en dos consultas y después de restart.

### Etapa D — Polars/ADBC como capacidades optativas

1. Transformación Polars representativa y equivalencia con DuckDB SQL.
2. Transferencia Arrow a staging/ducklake y commit explícito.
3. Driver ADBC PostgreSQL/otro en CI y compatibilidad Arrow.
4. Benchmarks y decisión go/no-go por característica.

**Criterio de salida:** no se añade costo de mantenimiento sin mejora demostrada.

### Etapa E — Quack/ejecutor remoto (si hay necesidad)

1. Implementar `QuackRemoteRuntime` detrás del mismo trait.
2. Probar equivalencia de tipos, cancelación, RLS, límites y manejo de errores.
3. Probar reconexión, credenciales, failover y estado beta/madurez de la versión.
4. Publicar solo si demuestra valor de aislamiento o escalabilidad.

---

## 24. Pruebas de aceptación de integración (65 casos)

### A. Instalación y reanudación

1. Conectar a PostgreSQL y ATTACH DuckLake desde Rust in-memory.
2. Reiniciar Rust sin `.duckdb` y recuperar registros Parquet.
3. Verificar que PostgreSQL no contiene filas analíticas como filas de tabla OLAP por defecto.
4. Verificar que `DATA_PATH` realmente guarda archivos Parquet.
5. Al eliminar caché/session Rust no desaparece tabla DuckLake comprometida.
6. Directorio de Parquet inaccesible genera error claro, no catálogo aparentemente vacío.
7. Credenciales equivocadas se rechazan sin filtrarse en logs.
8. Bootstrap no permite `INSTALL` por viewer.
9. Versión incompatible de extensión se detecta antes del refresh.
10. Recuperación de backup restaura catálogo y Parquet coherentes.

### B. Concurrencia y transacciones

11. Dos consultas simultáneas sobre snapshot estable retornan resultados consistentes.
12. Escribir nueva versión no expone filas parcialmente construidas.
13. Dos escritores del mismo dataset no publican revisiones incompatibles.
14. Reintento tras conflicto no duplica lote.
15. Escrituras concurrentes en datasets distintos respetan límite de workers.
16. Fallo a mitad de upload no altera publicación anterior.
17. Fallo después de commit y antes de puntero BI se recupera idempotentemente.
18. Fallo del servidor PostgreSQL devuelve error recuperable, no dato inventado.
19. El algoritmo de limpieza no borra un Parquet aún referenciado por un snapshot válido.
20. Lectores largos conservan la revisión esperada durante una actualización.

### C. Runtime `duckdb-rs`

21. Conexión se usa con seguridad según `Send`/`!Sync`.
22. Pool agotado encola/rechaza conforme política en vez de crear conexiones ilimitadas.
23. `interrupt_handle` cancela consulta larga correctamente.
24. Después de cancelar, la conexión se recicla solo si está en estado limpio.
25. Query de usuario no ejecuta SQL administrativo de extensión.
26. `query_arrow` vs `stream_arrow` producen mismos valores.
27. Resultado grande se procesa por lotes sin `collect()` total descontrolado.
28. Error tardío de conversión Arrow no derriba servidor por un pánico sin captura.
29. `EXPLAIN ANALYZE` observacional no expone secretos o datos del cliente.
30. OOM controlado produce error y no detiene todos los workers sin recuperación.

### D. Tipos e interoperabilidad

31. `DECIMAL(18,2)` conserva centavos exactos entre DuckDB y Arrow.
32. `DECIMAL` grande fuera de `rust_decimal` se conserva/rechaza explícitamente.
33. `UBIGINT` mayor que precisión de JavaScript se serializa sin pérdida.
34. UUID y código `"001"` mantienen semántica textual.
35. Zona horaria conserva interpretación de TIMESTAMPTZ.
36. DATE no cambia por conversiones UTC.
37. NULL real no se transforma en string `"null"`.
38. Diccionarios Arrow son admitidos o rechazados con diagnóstico.
39. Tipos List/Struct se convierten sin corromper valores.
40. Buffer Arrow se libera al interrumpir consumidor.

### E. Polars

41. Polars LazyFrame ejecuta una transformación real sin Python.
42. Plan Polars lee Parquet con pushdown cuando es aplicable.
43. Resultado Polars equivale a DuckDB SQL en dataset fixture.
44. Importar Arrow desde Polars no degrada DECIMAL.
45. Parquet creado por Polars fuera de catálogo NO se considera tabla DuckLake válida.
46. Publicación Polars→DuckDB→DuckLake produce snapshot legible.
47. Pipeline Polars falla con limpieza de staging controlada.
48. La desactivación de la feature Polars no impide consultas DuckDB.
49. Se mide la copia real de memoria Arrow↔Polars y se registra.
50. Versiones incompatibles de crates fallan en CI, no en producción silenciosamente.

### F. ADBC

51. Driver ADBC elegido carga en Windows/Linux objetivo.
52. Driver inexistente devuelve error de capabilities.
53. Consulta PostgreSQL vía ADBC devuelve batches tipados equivalentes.
54. Cancelación/transacción no soportadas se declaran como tales.
55. Driver arbitrario no aparece instalado/ejecutable desde texto de usuario.
56. Driver se descarga/carga solo desde allowlist aprobada.

### G. Quack y escalamiento

57. Sin Quack instalado, todas las rutas core del MVP funcionan.
58. Adaptador remoto optativo pasa igualdad de resultados del test semántico.
59. Fallo de servidor remoto no deja queries eternamente pendientes.
60. La no equivalencia en cancelación remota se reporta y bloquea release.
61. Escalamiento multiproceso sobre mismo DuckLake prueba dos nodos y almacenamiento realmente compartido.
62. `DATA_PATH` local diferente en dos hosts detecta error en bootstrap.
63. Backend Rust y catálogo PostgreSQL se pueden actualizar/reiniciar sin perder filas físicas comprometidas.
64. Medidas facturación/cobranza no se inflan por cambios de runtime.
65. Materialización y uso de caché reflejan la versión física/semántica publicada.

---

## 25. Riesgos que podrían cambiar la arquitectura elegida

| Riesgo | Indicador de alerta | Mitigación / posible cambio |
|---|---|---|
| Un solo proceso Rust sufre por consultas pesadas | P95 y memoria bajo refresh simultáneos | Separar worker proceso o Quack remoto |
| Inestabilidad FFI o extensiones en plataforma objetivo | Crash/repro en CI | Fijar stack, aislar runtime en proceso separado |
| Conflictos de commit en DuckLake | Reintentos agotados y altas colisiones | Serializar escritores por tabla o partición |
| Demasiados archivos Parquet | Latencia metadata y requests | Política de compactación y partición |
| PostgreSQL catálogo como cuello de botella | Latencia/locks/cpu catalog | Optimizar writes, pooling, catálogo dedicado |
| Filesystem local restringe multi-host | Necesidad de segundo nodo | Object storage soportado + pruebas |
| Polars y DuckDB convierten demasiado | Más memoria/tiempo que SQL | Desactivar camino Polars para ese modelo |
| ADBC no tiene driver confiable | ABI/feature gaps | Adaptador nativo o DuckDB extension |
| Quack cambia protocolo | Fallo compatibilidad | Mantener adaptador fuera de core |
| SQL avanzado fuerza extensiones inseguras | Escalada privilegios | Sandboxing, capability policy, permitir solo plugins aprobados |

**La solución correcta no se elige por ser “más Rust” o “más moderna” como etiqueta; se elige porque sus contratos, seguridad y rendimiento están comprobados.**

---

## 26. Decisiones cerradas, abiertas y qué NO afirmar

### Cerradas para el diseño inicial

- Motor de consultas BI: **DuckDB embebido** dentro del backend Rust.
- Almacén persistente: **DuckLake**, metadatos físicos en **PostgreSQL**, datos en **Parquet**.
- Catálogo local DuckLake SQLite: **no**.
- Archivo persistente `.duckdb` central: **no obligatorio ni elegido**.
- Quack: **opcional** y habilitado solamente por una razón demostrada.
- Arrow: contrato de resultados/transferencias columnar.
- ADBC: adaptador opcional de conectividad; no reemplaza `duckdb-rs`.
- Polars Rust: capacidad opcional de transformación de modelos, no motor semántico universal.
- Metadata BI de aplicación: PostgreSQL como backend inicial multiusuario **separado lógicamente** del catálogo DuckLake; import/export declarativo de M10.

### Abiertas para spike con código

- Versión fija de DuckDB / duckdb-rs / extensiones / Polars según plataforma.
- Modo exacto de distribución de extensiones y allowlist de descargas.
- Control de permisos del directorio Parquet y object storage.
- Semántica real de instantáneas leídas bajo refresh, para la versión fijada.
- Ruta estable de Arrow streaming con recuperación de errores tardíos.
- Número óptimo de conexiones, hilos DuckDB, límite RAM y CPU bajo concurrencia real.
- Umbral que justifica añadir Polars y ADBC en producción.
- Escenario que justifica aislar DuckDB en otro proceso.

### No afirmamos

- Que toda la solución esté escrita nativamente en Rust: DuckDB es un motor nativo por debajo de FFI.
- Que Polars escriba transacciones DuckLake sin el adaptador correspondiente.
- Que ADBC ofrezca paridad funcional total entre drivers.
- Que DuckLake con PostgreSQL elimine conflictos de escritura o garantice performance ilimitada.
- Que usar PostgreSQL permita compartir automáticamente directorios Parquet entre hosts.
- Que DuckDB `:memory:` implique persistencia física de las tablas temporales.
- Que Quack esté suficientemente maduro para ser requisito inicial sin benchmark y tests.
- Que se haya probado la implementación o auditado un repositorio privado de producto.

---

## 27. Índice de fuentes primarias inspeccionadas / contrastadas

### DuckDB / cliente Rust

1. [`duckdb-rs`: módulo `statement.rs` — `query_arrow`, `stream_arrow`, `query_polars`, restricciones de thread](https://github.com/duckdb/duckdb-rs/blob/main/crates/duckdb/src/statement.rs)
2. [`duckdb-rs`: `appender/arrow.rs` — ingesta de RecordBatch](https://github.com/duckdb/duckdb-rs/blob/main/crates/duckdb/src/appender/arrow.rs)
3. [Documentación oficial Rust — instalación/features](https://duckdb.org/docs/current/clients/rust/overview)
4. [Documentación oficial Rust — conexión, clonación, pooling, Send/Sync](https://duckdb.org/docs/current/clients/rust/connecting)
5. [Documentación oficial Rust — resultados Arrow y Polars](https://duckdb.org/docs/current/clients/rust/result_handling)
6. [Documentación oficial Rust — profiling e interrupción](https://duckdb.org/docs/current/clients/rust/profiling)
7. [Concurrencia DuckDB: in-process, `.duckdb`, DuckLake y Quack](https://duckdb.org/docs/current/connect/concurrency)
8. [Extensión Quack — estado y limitaciones](https://duckdb.org/docs/current/core_extensions/quack)
9. [Extensión PostgreSQL DuckDB](https://duckdb.org/docs/current/core_extensions/postgres/overview)
10. [Extensiones INSTALL/LOAD](https://duckdb.org/docs/current/sql/statements/load_and_install)

### DuckLake

11. [Conectar DuckLake con PostgreSQL y DATA_PATH](https://ducklake.select/docs/stable/duckdb/usage/connecting)
12. [Elegir catálogo — PostgreSQL](https://ducklake.select/docs/stable/duckdb/usage/choosing_a_catalog_database)
13. [Código DuckLake — repositorio](https://github.com/duckdb/ducklake)
14. [Transacciones DuckLake — `ducklake_transaction.cpp`](https://github.com/duckdb/ducklake/blob/main/src/storage/ducklake_transaction.cpp)
15. [Issue DuckLake 233 — conflictos concurrentes y reintentos](https://github.com/duckdb/ducklake/issues/233)
16. [Issue DuckLake 1215 — comprobar validación de conflictos en borrados](https://github.com/duckdb/ducklake/issues/1215)

### Polars / Arrow / ADBC

17. [Polars: implementación `LazyFrame`](https://github.com/pola-rs/polars/blob/main/crates/polars-lazy/src/frame/mod.rs)
18. [Polars Rust: Parquet Scan y Writer](https://docs.pola.rs/user-guide/io/parquet/)
19. [Polars Arrow producer/consumer](https://docs.pola.rs/user-guide/misc/arrow/)
20. [ADBC Rust: quickstart API y driver manager](https://arrow.apache.org/adbc/main/rust/quickstart.html)
21. [ADBC Rust core — código](https://github.com/apache/arrow-adbc/blob/main/rust/core/src/lib.rs)
22. [ADBC DuckDB driver — soporte por lenguaje](https://arrow.apache.org/adbc/current/driver/duckdb.html)
23. [ADBC changelog y cambios ABI](https://github.com/apache/arrow-adbc/blob/main/CHANGELOG.md)
24. [DuckDB Rust `docs.rs`](https://docs.rs/duckdb/latest/duckdb/)

**Criterio de reproducibilidad:** los enlaces a `main` se usan para descubrir firmas/decisiones. Antes de transcribir código o escribir tests de compatibilidad hay que anotar los SHAs, versiones Cargo y extensión DuckLake realmente instalados. Los fragmentos Rust de este informe son interfaces propuestas salvo indicación explícita.

---

## 28. Conclusión y puerta para siguiente capa

**Arquitectura ganadora:** **Rust + `duckdb-rs` embebido + DuckLake/PostgreSQL/Parquet + Arrow**, con **Polars Rust para transformaciones donde se justifique**, **ADBC para conectores compatibles** y **Quack solo si una necesidad real aconseja un servidor remoto**.

La prueba decisiva no es que `ATTACH` funcione una vez: es que, tras reiniciar Rust, dos consultas simultáneas lean el mismo snapshot comprometido, un refresh pueda publicarse sin resultados parciales, un DECIMAL preserve precisión, un fallo se recupere y no exista dependencia de un `.duckdb` persistente.

**Este es exclusivamente el Módulo 13.** La siguiente investigación, cuando el usuario indique continuar, vuelve al **Módulo 14: Automatización, scheduling, alertas y reporting**. El ZIP final integrará los módulos 01–13, revisando primero las decisiones heredadas de SQLVIZ en M01–M09 para que el conjunto sea coherente con la plataforma nueva.

---

**Fin del Módulo 13.**
