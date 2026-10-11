# 0007 — Construcción propia por capa, al nivel de Atlas BI

**Estado:** aprobada por el propietario, 2026-10-11. **Relacionada:** `docs/referentes/atlasbi.md`, AGENTS.md ("Nivel mínimo: Atlas BI").

## Contexto
Atlas BI, el proyecto hermano del propietario, cubre las 12 capas de una plataforma BI **integrando** herramientas open source: Cube, SQLMesh, dlt, Airflow, OpenMetadata, ElectricSQL, better-auth y tRPC. El propietario quiere que bi-platform ofrezca un alcance y un nivel similares, pero **con construcción propia**.

## Decisión
Cada capa de producto se construye en bi-platform (Rust en el servidor, Svelte en la UI). Atlas BI y los referentes marcan el nivel, no las dependencias.

| Capa (Atlas BI) | Atlas BI integra | bi-platform construye |
|---|---|---|
| Almacenamiento | DuckDB + DuckLake | Lo mismo (motor y formato, no plataforma) |
| Transformación | SQLMesh | ELT propio estilo SQLMesh (ADR 0005) |
| Ingesta | dlt + ADBC | Conectores propios sobre DuckDB y ADBC (ADR 0002) |
| Semántica | Cube | SEF (ADR 0006) |
| IA conversacional | Claude API + MCP propio | Asistente sobre la capa semántica + MCP (M16) |
| API | tRPC + OpenAPI | Axum + OpenAPI generado (G0–G1) |
| Orquestación | Airflow | Programador y registro de trabajos propios (M14) |
| Frontend y visualización | SvelteKit + ECharts | SvelteKit + motor de visualización propio con ECharts tras un puerto (M09) |
| Autenticación y gobernanza | better-auth / Keycloak | Sesiones, permisos, RLS y auditoría propios (M11) |
| Colaboración en tiempo real | ElectricSQL | Propia, si se aprueba (sin módulo aún) |
| Infraestructura | Podman + OpenTofu | Podman y un ejecutable único (G0) |
| Metadata y linaje | OpenMetadata | Catálogo y linaje propios (M10), exportables a OpenLineage |

## Qué no se construye
"Propio" significa **las capas de producto**, no reinventar lo que ya es un estándar o un motor:
- **Motores y formatos:** DuckDB, DuckLake, Parquet, Arrow y PostgreSQL.
- **Librerías:** Axum, Tokio, sqlx, SvelteKit, ECharts…
- **Protocolos y estándares:** OIDC (con un proveedor de identidad externo como Entra ID, Google o Keycloak), OTLP, OpenAPI, Apache Ossie, OpenLineage y MCP.

No construimos un proveedor de identidad, ni un motor SQL, ni un formato de archivo.

Si en una capa construir no tiene sentido (coste desproporcionado o un estándar maduro que ya lo resuelve), la excepción se justifica en un ADR con la aprobación del propietario.

## Consecuencias
- Más trabajo por capa, a cambio de un producto coherente: un solo lenguaje y despliegue, contratos propios, sin depender de la gobernanza de terceros y con un ejecutable único.
- Cada capa empieza por sus referentes (AGENTS.md): el documento equivalente de Atlas BI, más 2–4 herramientas líderes, leyendo su código cuando es abierto.
