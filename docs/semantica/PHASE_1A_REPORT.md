# Informe de ingeniería — Phase 1A

Fecha: 2026-10-04 (America/La_Paz). Estado: **PASS para el alcance de Phase 1A**.
Phase 1B no se ha iniciado y requiere nueva autorización explícita.

## Alcance y recuperación

Se retomó la implementación presente en el workspace tras perderse el contexto
de la conversación anterior. Había un crate CSMIR, validación, ADR y fixtures,
pero la compilación fallaba: faltaban las proyecciones canónicas de identidades,
tipos, unidades, expresiones, granos y tiempo, y el cast de métricas usado por
CASE-006. El formato también incumplía `cargo fmt`.

Se completaron las proyecciones explícitas, el cast numérico de métricas con
preservación de unidades y redondeo declarado, los controles y el informe.
`CompiledSemanticModel` solo se construye mediante compilación válida y conserva
un snapshot inmutable con fingerprints de modelo y objetos. Las verificaciones
son locales a las definiciones: no ejecutan datos ni certifican resultados.

El texto completo del plan anterior y su lista original de criterios no estaban
disponibles. La siguiente matriz evalúa el alcance autorizado en el mensaje
recuperado y los contratos aplicables del blueprint; no inventa criterios del
plan anterior ni declara terminada toda Phase 1.

## Criterios de aceptación

| Criterio | Estado | Evidencia |
|---|---|---|
| Fundamentos canónicos y CSMIR compilado e inmutable | PASS | Compilación y hashes de los 21 modelos; validación de referencias, tipos, dependencias y contratos |
| `DeclaredUnverified` no impide compilar; `Invalid` se rechaza donde aplica | PASS | Regresiones de cardinalidad, unicidad, temporal match y allocation; CASE-003 |
| `INCOMPATIBLE_UNITS` se limita a definiciones inválidas en este bloque | PASS | USD/BOB separados compilan; suma en definición falla; CASE-017 válido como modelo |
| ADR canónico con tags, opcionales, integers, decimal, Unicode, maps, sets, secuencias, enums/unions, separación de dominios, versión y metadata | PASS | ADR 0001 y vectores CBOR/SHA-256 |
| Serde/JSON/layout Rust no definen el formato canónico | PASS | Árbol tipado y proyecciones explícitas; ninguna derive Serde en IR; goldens de regresión |
| Frontera: sin SQIR, query correctness, fanout, planner, SEIR, motores, Ossie, API/UI | PASS | Workspace contiene únicamente `sef-model-ir`; inspección y control de dependencias |
| Sin dependencias importantes adicionales | PASS respecto al workspace recuperado | Manifiestos y lockfile conservados; cuatro dependencias directas fijadas |
| Quality gates disponibles ejecutados | PASS | Matriz siguiente y logs |
| 21 fixtures representados sin modificar su verdad esperada | PASS | 21 pruebas de modelos y 81 archivos originales verificados por SHA-256 |
| Detención antes de Phase 1B | PASS | Ningún crate ni implementación de Phase 1B |

Los identificadores SQIR/SEIR en el encoder y sus vectores prueban únicamente
separación de dominios reservados; no implementan esos IRs.

## Quality gates

Verificación en Ubuntu/WSL, Linux x86_64, Rust/Cargo **1.98.1**.

| Comando | Estado | Resultado |
|---|---|---|
| `cargo fmt --all --check` | PASS | Sin diferencias |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | PASS | Sin warnings |
| `cargo test --workspace --all-features --locked` | PASS | 57 tests; 0 fallos, 0 ignorados |
| `python3 scripts/check_phase_1a.py` | PASS | Crate/dependencias/proyección explícita; 81 archivos originales intactos; 21 casos y hashes |
| `cargo deny --all-features check` | PASS | Advisories, bans, licenses, sources: ok |

Se empleó `cargo test`; `cargo-nextest` no está instalado. No hay frontend,
benchmarks ni runtime aplicables a este bloque. No se hicieron afirmaciones de
performance. Los logs están en `.openai/phase-1a/{fmt,clippy,tests,architecture,deny}.log`.

`cargo-deny 0.20.2` devuelve advertencias no bloqueantes por `syn 2.0.119` y
`syn 3.0.6` transitivos, y por tres licencias permitidas que no aparecen en el
grafo (BSD-3-Clause, ISC, Zlib). No reportó vulnerabilidades ni licencias rechazadas.
La doble versión de `syn` proviene de `zerocopy-derive` y `serde_derive`.

## Dependencias utilizadas

| Dependencia directa | Versión exacta | Uso |
|---|---|---|
| `ciborium` | 0.2.2 | Escritura CBOR del árbol ya construido explícitamente |
| `semver` | 1.0.27 | SemVer de objetos gobernados |
| `sha2` | 0.10.9 | SHA-256 de valores y envelopes |
| `proptest` (dev) | 1.9.0 | Propiedades de orden, duplicados, tipos y dominios |

Serde **1.0.229** es transitivo de ciborium y no se usa para derivar el contrato
del IR. Las versiones transitivas exactas permanecen en `Cargo.lock`; el árbol
completo quedó en `.openai/phase-1a/dependencies.txt`.
No se cambiaron los manifiestos ni `Cargo.lock` durante esta reanudación.
Rust, cargo-deny y Python son herramientas de verificación, no dependencias del
producto. Rust se instaló en el usuario de WSL con `--no-modify-path`.

## Pruebas y goldens

- `canonical_vectors.rs`: **15 tests**, incluyendo cuatro propiedades generativas.
  Comprueba 20 valores canónicos con bytes y hashes fijos, tres envelopes de
  dominio, un golden independiente de identidad, anchos CBOR, Unicode exacto,
  decimal de hasta 76 dígitos, metadata/evidencia excluida y límites de recursos.
- `corpus_models.rs`: **24 tests**: los 21 modelos y tres rechazos locales
  (evidencia temporal inválida, allocation inválida, definición USD + BOB).
  Cada fixture comprueba fuentes/columnas, hashes de objetos, compilación,
  invariancia por reordenamiento y hash golden del modelo.
- `model_validation.rs`: **18 tests** de errores estructurados, evidencia,
  referencias, ciclos, localidad, políticas, rounding, tipos, claves compuestas
  y roles. Dos regresiones nuevas comprueban casts de métricas: redondeo/unidades
  y detección de ciclos atravesando un cast.
- `csmir-golden-hashes.txt`: **21 hashes** de artefactos CSMIR congelados.

Los bytes primitivos/identidad se derivaron independientemente de la proyección;
los digests fijos se contrastaron con SHA-256 de .NET. Los 21 hashes completos de
modelos son baselines generados de sus definiciones compiladas y sirven como
regresión de perfil, sin representar validación independiente de resultados.

Cobertura: **21/21 representaciones CSMIR**, detalladas en
`corpus/PHASE_1A_COVERAGE.md`. Las consultas de CASE-016, 017 y 020 siguen siendo
rechazos normativos para una etapa posterior; sus modelos válidos compilan aquí.
No se ejecutaron filas CSV, outputs, query correctness, planes ni certificados.

## Archivos

No hay repositorio Git en este directorio; la atribución de esta reanudación usa
un snapshot SHA-256 local previo a los cambios. No se puede atribuir con certeza
qué archivos creó la sesión anterior.

Creado en esta reanudación:

- `crates/sef-model-ir/src/canonical_projection.rs`.
- `crates/sef-model-ir/tests/csmir-golden-hashes.txt`.
- `scripts/check_phase_1a.py`.
- `docs/PHASE_1A_REPORT.md`.
- Evidencias y snapshot local en `.openai/phase-1a/`.

Modificado funcionalmente:

- `src/lib.rs`: registro de proyecciones y excepción documentada de Clippy.
- `src/canonical.rs`: proyección de Vec como secuencia y ajuste recomendado por Clippy.
- `src/expressions.rs`: cast explícito de métricas y recorrido de dependencias.
- `src/validation.rs`: validación de casts y simplificaciones de Clippy.
- Los tres tests de integración: golden de identidad, hashes de 21 modelos y
  regresiones de cast.
- `README.md`, `docs/adr/0001-sef-canonical-value-v1.md`,
  `corpus/PHASE_1A_COVERAGE.md`: estado real, proyecciones y evidencia de cobertura.

Formateado también, sin cambios funcionales: `src/diagnostic.rs`, `grain.rs`,
`identity.rs`, `model.rs`, `policy.rs`, `relationships.rs`, `time.rs`, `types.rs`,
`units.rs`. Todas las rutas `src/` y tests anteriores pertenecen a `crates/sef-model-ir`.

Ya presentes al retomar: workspace y manifiestos Rust, `Cargo.lock`,
`rust-toolchain.toml`, `deny.toml`, crate CSMIR, ADR y cobertura. No se eliminaron
archivos ni se alteraron fixtures originales.

## Desviaciones, deuda y riesgos

- No se amplió el alcance ni se incorporaron nuevas dependencias de producto.
  Se completó el cast numérico que el fixture existente requería para CASE-006;
  es parte de la definición CSMIR y no una operación de consulta.
- Se permite explícitamente `clippy::result_large_err` en el crate: los errores
  estructurados se devuelven por valor en las rutas de compilación. Las demás
  reglas de Clippy, incluidos panic/unwrap/expect de producto, permanecen activas.
  Helpers de pruebas usan `expect` únicamente con fixtures fijos.
- Los límites de canonicalización se verifican sobre el árbol tipado; no
  equivalen a un presupuesto global de memoria para toda compilación. No hay
  parser externo ni servicios expuestos en este bloque.
- TZDB membership, validación de filas/evidencia fresca, ejecución decimal,
  runtime y certificación se dejan a sus etapas autorizadas. Las declaraciones
  de timezone/unidades no incluyen una base externa de ejecución.
- No se verificó una matriz de plataformas: los gates se ejecutaron en WSL/Linux.
- `CHECKSUMS.sha256` se conserva como baseline histórico del blueprint; algunos
  documentos de estado se actualizaron. El control de integridad limita su
  afirmación a los 81 archivos normativos originales del corpus.
- La falta del plan anterior impide certificar su lista literal de aceptación;
  la matriz de este informe explicita los criterios recuperables verificados.

El bloque autorizado termina aquí. No se inició Phase 1B.
