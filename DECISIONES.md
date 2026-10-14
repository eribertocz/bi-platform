# Decisiones

*Índice de todo lo decidido: qué manda sobre qué. No duplica el contenido de cada documento, solo dice dónde está y en qué orden se aplica. Escrito por Claude el 2026-10-13, para el corte a roles.*

## Orden de prioridad cuando dos documentos chocan

**Decidido por el propietario, 2026-10-12** (`docs/producto/propuestas/version-comun.md`, decisión 10):

```
1. Leyes fundamentales      docs/producto/leyes-bi.md (13 leyes; prioridad 1, 2, 3 y 5)
2. Anti-patrones            fundamentos-propietario.md, «Documento 2»
3. Seguridad                ADR 0010 y 0012; AGENTS.md
4. ADR vigentes             docs/decisiones/ (0001-0013)
5. Principios de producto    AGENTS.md («facilidad, velocidad, modernidad...»)
6. Resto de documentos       docs/arquitectura/, docs/semantica/, docs/referentes/...
```

- La accesibilidad y la exactitud cuentan como leyes (11 y 13 de `leyes-bi.md`).
- Si una ley choca con una ADR, decide el propietario y queda en una ADR **nueva**; ninguna ADR se anula en silencio.
- Las leyes y los fundamentos son **retroactivos**: lo ya construido se audita contra ellos (Issue #84).

## Las piezas, cada una en su sitio

| Qué | Dónde | Quién lo aprueba |
|---|---|---|
| **13 leyes fundamentales** | [`docs/producto/leyes-bi.md`](docs/producto/leyes-bi.md) | Propietario |
| **Anti-patrones y principios de diseño** | [`docs/producto/propuestas/fundamentos-propietario.md`](docs/producto/propuestas/fundamentos-propietario.md), «Documento 2» | Propietario (adoptado como objetivo, decisión 9 abajo) |
| **13 ADR técnicas** (0001–0013) | [`docs/decisiones/README.md`](docs/decisiones/README.md) | Propietario (producto) o rutinaria (técnica) |
| **10 decisiones de producto aprobadas el 2026-10-12** (ELT, Estudio, bibliotecas, fundamentos, orden de prioridad...) | [`docs/producto/propuestas/version-comun.md`](docs/producto/propuestas/version-comun.md), apartado 5 | Propietario |
| **Blueprint del propietario** (visión original) | [`docs/producto/BLUEPRINT_2026-10-11.md`](docs/producto/BLUEPRINT_2026-10-11.md) | — (ADR manda donde contradiga) |
| **Base histórica de arquitectura** (no se modifica) | [`docs/arquitectura/`](docs/arquitectura/INDICE_17_MODULOS.md), huellas en `INVENTARIO.md` | — (ADR la sustituye donde aplica) |
| **Diseño normativo del motor semántico** | [`docs/semantica/`](docs/semantica/README.md) | — (ADR manda donde contradiga) |
| **Reglas de trabajo y calidad** | [`AGENTS.md`](AGENTS.md) | Propietario |
| **Estado actual del proyecto** | [`ESTADO.md`](ESTADO.md) | Se actualiza solo, al cerrar cada Issue |

## Decisión: roles estrictos (2026-10-13)

**Aprobada por el propietario.** Hasta el 2026-10-12, Claude y Codex trabajaban con revisión cruzada, sin roles fijos. Desde el 2026-10-13:

- **Codex** es arquitecto, orquestador y verificador: crea y prioriza los Issues con sus criterios de aceptación, puede escribir pruebas de aceptación (nunca código de producto), verifica cada PR ejecutando sus pruebas, y **integra** (el verificador cierra, el autor nunca fusiona su propio trabajo).
- **Claude** es el implementador: solo trabaja issues que Codex especificó, escribe el código y ejecuta todo lo operativo de Git.
- Cada uno en su propia ventana de VS Code; ya no hay lanzamiento de uno por el otro.
- El trabajo en vuelo antes del corte se cerró con las reglas anteriores (PR #104–#111); este corte empieza con `main` en ese punto.
- **No sustituye ninguna ley, ADR ni decisión de producto anterior:** es una decisión de proceso, sobre cómo se trabaja, no sobre qué se construye.

## Decisión: documentación limpia antes del corte (2026-10-13)

**Auditoría de `docs/`** (propietario): estados desactualizados corregidos (ADR 0013, README), propuestas cerradas señaladas como tales, índice de referentes reordenado por módulo, y un listado único de menciones obsoletas de Polars en documentos congelados. Detalle: `docs/decisiones/README.md`, sección «Menciones obsoletas».
