# Interoperabilidad, Polars de primera clase y sistemas operativos

**Principio:** la plataforma no crea un lenguaje de consulta propietario obligatorio. Usuarios expertos pueden utilizar DuckDB SQL y, dentro de un entorno autorizado, Polars; usuarios de negocio disponen de un constructor visual que emite los mismos contratos semánticos.

## Estándares por interfaz

- SQL: DuckDB SQL documentado, sin DSL propio para consultas; especificar extensiones no portables cuando se usan.
- Archivos/columnar: Parquet y Apache Arrow (no asumir siempre zero-copy), CSV import/export; contratos con precisión decimal, timestamps, tipos y nulos.
- Modelo semántico: evaluar interoperabilidad de intercambio abierto, incluida la especificación OSS *Apache Ossie* / Open Semantic Interchange, con una matriz de características import/export y pérdidas explícitas; no suponer equivalencia con un motor DAX.
- Web/API: HTTP, OpenAPI 3.1, JSON tipado, SSE, OIDC y convenciones HTTP; MCP externo bajo la misma autorización, nunca superusuario.
- Metadatos: esquemas con versión, JSON/YAML abiertos cuando corresponda; evitar `.sqlviz` o dialectos cerrados como requisito.

## Polars y DuckDB ambos como primera clase

Polars Rust 2.0 y DuckDB SQL deben tener caminos operativos completos y verificables, no solamente referencias en documentos:

1. **DuckDB:** definir modelo SQL → plan autorizado → ejecución embebida → Arrow/TypedResult → publicar en DuckLake cuando corresponda.
2. **Polars:** definir modelo de expresiones/LazyFrame en Rust → plan autorizado → ejecución lazy/streaming si corresponde → Arrow/TypedResult → publicar en DuckLake mediante frontera transaccional segura; no asumir que Polars hace commits DuckLake directamente.
3. Si se ofrece Polars Python: ejecución aislada en proceso/container con límites y modelo de seguridad propio; no cargar código Python arbitrario en proceso de API ni bypass del motor semántico.
4. Mismo `run_id`, políticas, logs, lineage, coste y gates de calidad para ambos; benchmarks que separan coste de cada motor y transferencias.
5. Guardar en un formato portable de metadatos/versiones; no exigir aprender expresiones Rust a usuarios de negocio.

## Windows, Linux y macOS

- Objetivos CI: Windows x86_64, Linux x86_64, macOS arm64 (agregar macOS x86_64 si mercado/hardware lo justifica); toolchains y dependencias fijadas por plataforma, sin rutas Linux-only en código común.
- Normalización de rutas, separators, permisos, TLS/certificados, timezone, archivos temporales, signal handling y proceso de shutdown. Evitar shell scripts POSIX como único bootstrap; scripts Python portables o comandos multiplataforma.
- La matriz debe verificar Rust/Svelte build, integración DuckDB/Polars/Arrow, tests e instalación local; PostgreSQL/DuckLake en runners pertinentes o contenedores compatibles.
- No declarar soporte completo de un OS solo porque compila; distribución con prueba limpia, upgrade/rollback y E2E en ese OS.

## Versionado de motores: stable vs experimental

Polars 2.0 estable es la base candidata, fijando la versión concreta tras un spike de compatibilidad. DuckDB 2.0-dev o alfa puede evaluarse **en carril experimental** (nightly/allow-failure) sin bloquear CI principal; el carril estable usa DuckDB release compatible con `duckdb-rs` y extensión DuckLake disponibles.

La fecha anunciada de DuckDB 2.0 no equivale a release garantizada. Cuando haya release estable, abrir microtarea de actualización, fijar toolchain/ABI/extensión, probar lectura/escritura DuckLake, tipos Arrow, concurrencia, cancelación, transacciones y downgrade/rollback en tres OS. Sólo migrar principal cuando la matriz esté verde y ADR aprobado.
