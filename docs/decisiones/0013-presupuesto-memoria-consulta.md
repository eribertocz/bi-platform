# 0013 — Presupuesto de memoria de consulta con instancias DuckDB independientes

**Estado:** decisión técnica de Codex, pendiente de revisión de Claude, 2026-10-12 (#101). **Concreta:** M13 §13 y §19; cambia el pool G1 de conexiones de una instancia a una instancia por trabajador en el servidor. No cambia los motores elegidos, los permisos ni el almacenamiento (ADR 0001/0008/0010).

## Contexto y alcance

`memory_limit` es global por instancia DuckDB. Las conexiones clonadas comparten un buffer manager: un SET por conexión reduce también el presupuesto de las demás. DuckDB fijado en `duckdb-rs =1.10506.0` no expone `operator_memory_limit` (comprobado en los fuentes empaquetados). La propuesta upstream [20219](https://github.com/duckdb/duckdb/pull/20219) distingue sesión, pero declara un límite parcial para operadores; no protege todas las asignaciones.

**El presupuesto cubre memoria gestionada por DuckDB, no todo el RSS.** Bibliotecas, resultados Rust y ciertas asignaciones del motor viven fuera del buffer manager. El resultado HTTP conserva los límites de filas, bytes y celda de G1. Esto no es un sandbox de SQL arbitrario ni una garantía frente a cualquier asignación nativa: la API solo ejecuta los planes acotados que ya admite. Un aislamiento duro del proceso requeriría procesos trabajadores con límite del SO e intercambio Arrow; no se promete con un SET.

## Decisión

- El servidor configura `BI_QUERY_MEMORY_LIMIT`, por defecto `256MiB`, validado con aritmética exacta y rechazo de cero, desbordamiento, tamaños ilegibles y SQL incrustado.
- Cada trabajador tiene una instancia DuckDB independiente, un buffer manager y una sola consulta activa. El límite está aplicado y bloqueado antes de admitir trabajo. No hay SET por petición ni cuotas elegidas por el cliente.
- Todos adjuntan el mismo DuckLake (catálogo PostgreSQL y Parquet). No se copian tablas ni archivos. La fase administrativa carga/publica antes del arranque del pool; tablas y vistas temporales son locales al trabajador.
- El presupuesto efectivo es `min(BI_QUERY_MEMORY_LIMIT, BI_DUCKDB_MEMORY_LIMIT / workers)` cuando existe un presupuesto total explícito. El conjunto de trabajadores no multiplica ese total. El número de hilos también se reparte, con mínimo uno por instancia.
- Spill deshabilitado para los lectores limitados: superar el presupuesto falla con `RuntimeError::MemoryLimit`, HTTP 422 `query_memory_limit` y un motivo seguro. El trabajador sigue disponible; las otras instancias conservan su presupuesto. Cola, cancelación y recuperación de conexiones siguen acotadas.
- `RuntimeConfig::in_memory` mantiene explícitamente el modo compartido para pruebas/embedders que necesitan un catálogo temporal común. `query_memory_limit: Some(...)` activa el modo independiente. La configuración de entorno del servidor siempre configura un límite y no permite desactivarlo.
- `bi_query_rejected{reason="query_memory_limit"}` registra cada rechazo; ejecución registra `result="error"`. Request ID permanece en el problema HTTP y el span de petición.

## Referentes y alternativas

- [Rill v0.90.5, config.go](https://github.com/rilldata/rill/blob/v0.90.5/runtime/drivers/duckdb/config.go) **[CÓDIGO]**: divide CPU/memoria entre instancias de lectura/escritura y acota el pool. Se toma la división explícita, se mejora separando cada consulta activa; no se copian sus bases internas.
- [ClickHouse v25.8.1.5101-lts, MemoryTracker.cpp](https://github.com/ClickHouse/ClickHouse/blob/v25.8.1.5101-lts/src/Common/MemoryTracker.cpp) **[CÓDIGO]**: contabilidad por contexto y excepción `MEMORY_LIMIT_EXCEEDED`. Se toma el error tipado y el rechazo de una consulta; no se intenta interponer un contador Rust en las asignaciones C++ de DuckDB.
- [Power BI Query Memory Limit](https://learn.microsoft.com/en-us/power-bi/enterprise/service-admin-premium-workloads) **[DOCS]**: presupuesto de consultas separado de la capacidad. Se toma el límite configurable con valor por defecto; no se replica su control de capacidades.
- Atlas BI, docs 01 y 19: DuckDB embebido, configuración validada y salud real. Se conserva ese nivel operativo con pruebas DuckLake/PostgreSQL y observabilidad, sin un servicio DuckDB nuevo.
- Descartado: cambiar un límite global por petición (interfiere con consultas concurrentes), serializar todo el pool (pierde concurrencia), usar un contador de RSS del proceso para atribuir una consulta (cancela consultas inocentes), actualizar/parchear el motor solo por un límite parcial no disponible en el pin.

## Consecuencias

Se duplican cachés y conexiones de catálogo por trabajador, acotadas por presupuesto; no se duplica el almacenamiento. El arranque abre hasta cuatro instancias por defecto. El coste se mide en el PR con el mismo workload antes/después y con DuckLake real. Se verifica rechazo real, trabajador concurrente independiente, reutilización tras OOM, configuración bloqueada y contrato/métrica HTTP. El aislamiento absoluto de RSS queda fuera de la garantía de este presupuesto y se declara como riesgo abierto del Issue.
