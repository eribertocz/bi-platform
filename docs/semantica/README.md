# Motor semántico: SEF (Semantic Execution Fabric)

Diseño normativo del motor semántico de bi-platform (ADR 0006). Procede del proyecto SEF del propietario: blueprint v1.4 auditado el 2026-09-27 y fase 1A implementada el 2026-10-04. Los documentos se conservan tal como estaban; [README_SEF_ORIGINAL.md](README_SEF_ORIGINAL.md) es el README original.

**Qué prevalece:** para el motor semántico, estos documentos prevalecen sobre M04/M05. Las decisiones de `docs/decisiones/` prevalecen sobre ellos. El alcance de producto ajeno al núcleo semántico (consola, plano de control, despliegue) lo define bi-platform, y el protocolo de trabajo es AGENTS.md (no `CODEX_START.md`).

| Documento | Contenido |
|---|---|
| [01_PRODUCT.md](01_PRODUCT.md) | Producto, alcance, ventaja diferencial y lo que no es |
| [02_ARCHITECTURE.md](02_ARCHITECTURE.md) | Capas, propiedad, IRs y leyes de dependencia |
| [03_MODEL_AND_QUERY_IR.md](03_MODEL_AND_QUERY_IR.md) | Contratos de CSMIR y SQIR |
| [04_CORRECTNESS_AND_PLANNING.md](04_CORRECTNESS_AND_PLANNING.md) | Reglas de corrección, obligaciones, planificador y SEIR |
| [05_EXECUTION_AND_ADAPTERS.md](05_EXECUTION_AND_ADAPTERS.md) | Traducción física, adaptadores de motor y capacidades |
| [06_TECHNOLOGY.md](06_TECHNOLOGY.md) | Estándares y política tecnológica |
| [07_TESTING_AND_CONFORMANCE.md](07_TESTING_AND_CONFORMANCE.md) | Corpus, conformidad y aceptación |
| [08_IMPLEMENTATION_PLAN.md](08_IMPLEMENTATION_PLAN.md) | Fases (en bi-platform: fases 1–2 = G2) |
| [adr/0001-sef-canonical-value-v1.md](adr/0001-sef-canonical-value-v1.md) | Formato canónico para huellas (CBOR + SHA-256) |
| [PHASE_1A_REPORT.md](PHASE_1A_REPORT.md), [AUDITORIA_V1_4.md](AUDITORIA_V1_4.md) | Informe de la fase 1A y auditoría del blueprint |
| [REFERENCES.md](REFERENCES.md) | Fuentes |

**Código:** `backend/crates/sef-model-ir` (CSMIR). **Corpus:** `backend/corpus/` (21 casos, conformidad obligatoria).
