# 0008 — Módulos desacoplados por contratos (puertos y adaptadores)

**Estado:** aprobada por el propietario, 2026-10-11. **Refuerza:** SEF `02_ARCHITECTURE.md` ("engines and protocols live behind adapters") y las leyes de dependencia de `08_IMPLEMENTATION_PLAN.md`; M12/M13 (contratos). **Concreta:** qué crates del mapa de SEF (`02_ARCHITECTURE.md` §8) son núcleo y cuáles adaptador (§2).

## Contexto
El propietario quiere que el motor semántico pueda **reutilizarse en otras herramientas BI**, y que **todos** los módulos y capas estén desacoplados: un módulo no sabe quién le entrega los datos ni a quién se los entrega, pero define con precisión el formato que recibe y el que entrega.

## Decisión
1. **Cada módulo expone contratos y nada más:**
   - tipos de entrada y salida públicos, versionados y documentados;
   - errores estructurados con código estable;
   - ninguna suposición sobre quién llama ni de dónde vienen los datos.
2. **Las dependencias apuntan hacia el núcleo:**
   - los **núcleos** (motor semántico `sef-*`; más adelante ELT, especificación visual…) no dependen de transporte (HTTP), almacenamiento (PostgreSQL), motores (DuckDB, ClickHouse), runtime asíncrono ni de otros módulos de producto;
   - los **adaptadores** de los bordes (`bi-server` para HTTP, `bi-runtime` para DuckDB, conectores…) dependen de los núcleos, nunca al revés;
   - el servidor es el único lugar donde se componen.

   **En el motor semántico, la frontera se decide por el nombre:**
   - **Núcleo:** todo `sef-*` salvo `sef-adapter-*`.
     - `sef-runtime` orquesta de forma síncrona y es núcleo; el envoltorio asíncrono vive en `bi-server`.
     - `sef-ossie` es el `sef-adapter-ossie` del mapa: un adaptador de formato sin infraestructura que cumple la regla de núcleo.
   - **Adaptadores de ejecución:** se llaman `sef-adapter-<motor>` (por ejemplo `sef-adapter-duckdb`, #47).
     - Pueden depender de su motor, que se añade como *wrapper* en `deny.toml`.
     - Nunca dependen del servidor HTTP ni de crates `bi-*`.
     - Ningún núcleo depende de ellos.
3. **Formatos de intercambio estándar entre módulos y hacia fuera:**

   | Qué | Formato |
   |---|---|
   | Modelos semánticos | Apache Ossie (YAML/JSON) |
   | Consultas y resultados por HTTP | OpenAPI + JSON tipado (valores exactos como texto) |
   | Datos tabulares voluminosos | Apache Arrow |
   | Hacia motores | SQL del dialecto de cada uno, generado desde un plan, nunca concatenado |
   | Telemetría | OTLP |
   | Linaje | OpenLineage |

4. **Se vigila automáticamente en CI:**
   - `backend/deny.toml` (`[bans]` con *wrappers*): solo `bi-server` puede depender de `axum`, `utoipa`, `sqlx` y `opentelemetry-otlp`; solo `bi-runtime`, de `duckdb`; solo `bi-server` compone `bi-runtime`. Para `bi-fixtures`, solo ellos dos.
   - `backend.yml`, con todas las features y todos los targets:
     - **núcleos:** su grafo no contiene `tokio`, `axum`, `tower`, `hyper`, `reqwest`, `sqlx`, `duckdb`, `clickhouse` ni `opentelemetry` (ni sus familias: `tokio-util`, `tower-http`, `opentelemetry_sdk`…), ni adaptadores `sef-adapter-*`, ni crates `bi-*`;
     - **adaptadores:** su grafo no contiene `axum`, `utoipa` ni crates `bi-*`;
     - si `cargo` falla o no encuentra crates, el paso falla: nunca pasa en silencio.
   - **Comprobado provocando cada violación:**
     - `duckdb` en `sef-model-ir`: fallan las dos reglas;
     - `tokio` opcional tras una feature no predeterminada: lo detecta;
     - nombres como `bi-query-core`, `opentelemetry_sdk` o `tower-http`: los detecta;
     - `bitflags`, `towers` o `hyperlocal`: no dan falsos positivos.
5. **El frontend sigue la misma regla:** las pantallas consumen contratos (`QueryResult`, y más adelante `VisualSpec`) a través del cliente generado desde OpenAPI, sin conocer cómo se calculan.

## Consecuencias
- El motor semántico se podrá ofrecer como **librería** (Apache-2.0) para otras herramientas, y más adelante con CLI, MCP o bindings de Python, sin arrastrar bi-platform.
- Un módulo nuevo debe declarar su contrato antes de integrarse. Añadir una dependencia prohibida exige un ADR que cambie estas reglas.
- Coste: algo más de diseño en las fronteras (tipos de contrato y adaptadores), a cambio de módulos que se prueban, sustituyen y reutilizan por separado.
