# Registros de decisión (ADR)

`docs/arquitectura/` es la **base histórica** del diseño: no se modifica (hashes en `INVENTARIO.md`). Las mejoras y correcciones se registran aquí, una decisión por archivo. **Donde un registro contradice a la base, prevalece el registro.**

Cada registro contiene: contexto, decisión, alternativas descartadas, consecuencias y la parte de la base a la que sustituye. Las decisiones de producto requieren la aprobación del propietario (@eribertocz). Las técnicas rutinarias, no.

| Nº | Decisión | Estado | Sustituye |
|---|---|---|---|
| [0001](0001-un-solo-motor-duckdb.md) | Núcleo con un solo motor analítico: DuckDB | Aprobada 2026-10-11 | Enmienda I.2 de `DECISIONES_TECNICAS_RC2.md` (Polars de primera clase) |
| [0002](0002-fuentes-y-conexiones-en-vivo.md) | Fuentes modernas, conexiones en vivo y ClickHouse como segunda fuente | Aprobada 2026-10-11 | Amplía M01 §2.5 y M13 §6.1 |
| [0003](0003-proyecto-como-archivos.md) | Definiciones como archivos de un proyecto en Git (estilo Rill) | Aprobada 2026-10-11 | Concreta M10 §0.1–0.3 |

Plantilla: copiar un registro existente y numerar con el siguiente número libre.
