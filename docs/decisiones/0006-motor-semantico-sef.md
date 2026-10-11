# 0006 — El motor semántico es SEF (Semantic Execution Fabric)

**Estado:** aprobada por el propietario, 2026-10-11. **Sustituye:** el diseño del motor semántico de M04 y la parte semántica de M05, donde contradigan a `docs/semantica/`.

## Contexto
El propietario diseñó antes **SEF**, en `E:\semantic-execute-fabric`: un runtime de ejecución semántica neutral respecto al motor. El blueprint v1.4 se auditó el 2026-09-27 y la fase 1A se implementó y aprobó el 2026-10-04. Sus piezas:
- Ossie como frontera.
- Representaciones intermedias propias: CSMIR (modelo), SQIR (consulta) y SEIR (ejecución).
- Un núcleo de inteligencia semántica: álgebra de granos, detección de *fanout*, caminos de relaciones, alineación de varios hechos, recálculo seguro de métricas, semántica temporal y semiaditiva, y evidencia determinista de cada plan.
- Un corpus normativo de 21 casos.

Es el nivel que el propietario exige para G2.

## Decisión
1. **SEF se integra en bi-platform** como su motor semántico:
   - **`docs/semantica/`:** documentos de SEF, normativos para el motor semántico.
   - **`backend/corpus/`:** los 21 casos, como **conformidad obligatoria** en CI.
   - **`backend/crates/sef-model-ir`:** CSMIR, fase 1A, importado **sin cambios**: 57 pruebas en verde y huellas canónicas fijas.
2. **Encaje con las decisiones vigentes:**
   - **Frontera Ossie (ADR 0003 y ficha Ossie):** se entra por `metrics/*.yaml`, escrito en `OSSIE_SQL_2026`. Sigue en pie la regla de SEF: Ossie nunca es el modelo interno; el modelo interno es CSMIR.
   - **Motores:** DuckDB primero, sobre el pool de `bi-runtime` y con el contrato exacto de G1; ClickHouse después, mediante la capa de dialectos (ADR 0002). Encaja con las capacidades de SEF: `NativeExact`, `EmulatedExact` y `Unsupported`.
   - **Alcance de producto:** lo que SEF planteaba más allá del núcleo semántico (Semantic Console, plano de control, trabajos, persistencia, identidad, despliegue) lo cubren los módulos y el roadmap de bi-platform.
   - **Forma de trabajo:** el protocolo de trabajo de SEF (`CODEX_START.md`) se sustituye por AGENTS.md.
3. **Plan de G2:**
   - Continuar la **fase 1 de SEF**: SQIR, motor de corrección, planificador determinista, SEIR, y los 21 casos con resultado exacto.
   - Seguir con la **fase 2**: adaptador DuckDB sobre `bi-runtime`, resultados Arrow o tipados, admisión, plazos y cancelación, reutilizando G1.
   - Por último: el adaptador Ossie, la API de consultas semánticas y la UI.

## Consecuencias
- Cambiar la semántica exige actualizar el corpus mediante un ADR. Ninguna optimización puede cambiar un resultado que ya pasa.
- `sef-model-ir` conserva su política de lints original (prohíbe `unwrap`, `expect` y `panic`). La alineación con el conjunto *pedantic* del workspace es un cambio aparte, protegido por sus pruebas y sus huellas.
- Licencia del crate: Apache-2.0, la que tenía SEF.
