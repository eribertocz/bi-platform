# 0012 — Motor de permisos: Cedar dentro del motor semántico

**Estado:** propuesta técnica de Claude (2026-10-11), pendiente de la revisión de Codex. **Concreta:** [0010](0010-seguridad-del-blueprint-en-g2.md) §A (punto único y motor de permisos con denegación por defecto), que pedía elegir el motor en su propio ADR tras leer el código de los candidatos. **Se apoya en:** ADR 0011 (permisos dentro del motor), las leyes 3 y 11 (`docs/producto/leyes-bi.md`) y el modelo de seguridad del propietario (documento 6, PR #85).

## Contexto
El motor de permisos (Issue #69) decide **quién puede hacer qué sobre qué objeto semántico**: métrica, dimensión, dataset, dashboard, modelo, fuente o usuario. Debe:
- funcionar con roles y atributos (RBAC + ABAC);
- denegar por defecto;
- explicar cada decisión (para la auditoría, #71);
- vivir **dentro del núcleo**, sin tokio ni red (ADR 0008 y 0011);
- tener políticas versionadas como archivos (ADR 0003).

La seguridad por filas (RLS) ya existe en CSMIR: políticas tipadas con parámetros `ContextSlot`, exigidas por el motor de corrección (#83) y aplicadas por el planificador (`PolicyGuard`, #86).

## Candidatos, leídos en su código (2026-10-11)
| Candidato | Código | Encaje |
|---|---|---|
| **Cedar** (`cedar-policy` 4.13.0, Apache-2.0) | `cedar-policy/cedar@56c9706` | Rust nativo, **sin tokio ni red**. Lenguaje de políticas con esquema y validación estricta. Semántica **verificada formalmente en Lean**. `cedar-policy-symcc` demuestra propiedades de conjuntos de políticas (`check_implies`, `check_equivalent`, `check_always_denies`) con contraejemplos. Evaluación parcial con tipos (`tpe`), experimental, que deja residuos sobre los atributos del recurso |
| **Casbin** (`casbin` 2.18.1, Apache-2.0) | `casbin/casbin-rs@ab8b1e9` | Por defecto exige `runtime-tokio`. API asíncrona (`async-trait`). Los *matchers* son *scripts* `rhai` evaluados como texto. Sin validación ni análisis formal |
| **OpenFGA** (Go, Apache-2.0) | `openfga/openfga@526995e` | Servicio aparte (modelo Zanzibar, permisos por relación). Exigiría una dependencia de red en el núcleo |
| **OPA** (Go, Apache-2.0) | `open-policy-agent/opa` | Rego y evaluación parcial madura para filtrar datos. En Rust exigiría un runtime de Wasm y otro lenguaje para los autores |

**Prueba real con Cedar** (`cedar-policy =4.13.0`, sin funciones experimentales, Windows, perfil de depuración):
- el esquema `User in [Role]`, `Metric { certified, sensitive }` y la acción `view` se validan en modo estricto;
- un `viewer` ve la métrica certificada y no sensible, y se le **deniega** la no certificada y la sensible;
- cada decisión devuelve la política que la permitió;
- **≈30 µs** por decisión;
- **90 crates**, ninguno de ellos tokio, todos con licencias de la lista de `backend/deny.toml`.

## Decisión
1. **Cedar** decide los permisos: RBAC + ABAC con denegación por defecto.
   - Vive en un crate de núcleo nuevo, **`sef-authz`**, que depende de `cedar-policy` con versión fijada y **sin funciones experimentales**.
   - Expone `authorize(principal, acción, recurso, contexto) → decisión + políticas que la determinan`.
   - El motor semántico la consulta **antes** del motor de corrección para cada métrica, dimensión y dataset de la consulta.
2. **Acciones y roles iniciales**, los del documento 6 del propietario:
   - acciones: `view` y `use_in_dashboard` (métrica); `view`, `edit`, `share` y `delete` (dashboard); `view`, `propose_change`, `approve` y `publish` (modelo semántico); `connect` y `configure` (fuente); `manage` (usuario y rol);
   - roles base `viewer`, `analyst`, `modeler` y `admin`, como políticas por defecto **editables** del proyecto.
3. **Seguridad por columnas (CLS), en tres niveles** (mostrar, enmascarar, eliminar):
   - un campo o métrica se muestra si `view` está permitido;
   - se enmascara si solo lo está `view_masked`;
   - si no, se elimina del resultado o la consulta se deniega.

   Lo aplica el motor, nunca el frontend.
4. **La seguridad por filas sigue en CSMIR**: políticas tipadas con `ContextSlot`.
   - Los valores de sus parámetros salen de los **atributos verificados** del usuario (OIDC, #70), nunca de texto de la consulta.
   - Derivar filtros por fila de los residuos de Cedar (`tpe`) se estudiará cuando deje de ser experimental.
5. **Políticas como archivos:**
   - `policies/*.cedar` y un esquema `policies/schema.cedarschema` **generado desde el modelo semántico**, todo en Git;
   - validación estricta al cargar, con errores al panel de Problemas.
6. **Verificación de cambios** (siguiente paso, no en la primera entrega): usar SymCC para demostrar en el plan de un cambio de políticas que **no amplía** el acceso (`check_implies`), o enseñar el contraejemplo. Necesita el resolvedor `cvc5` como herramienta de desarrollo y de CI, nunca en el servidor.
7. **Auditoría:** cada decisión registra las políticas que la determinaron (#71).

## Alternativas descartadas
- **Casbin:** tokio y API asíncrona en el núcleo (rompe la ADR 0008); reglas como *scripts* de texto; sin validación ni análisis formal.
- **OpenFGA:** un servicio de red dentro del camino crítico de cada consulta. Queda como **adaptador futuro** si se necesita compartir por relaciones a gran escala.
- **OPA:** runtime de Wasm y Rego, un segundo lenguaje para los autores. Su evaluación parcial para filtrar datos es la referencia conceptual de nuestro punto 4.
- **Motor propio:** reinventar un lenguaje de políticas sin su verificación formal ni sus herramientas.

## Consecuencias
- Dependencia nueva: `cedar-policy` fijada en `=4.13.0`. Una actualización se revisa como un cambio de seguridad.
- El esquema de Cedar se genera desde CSMIR, así que los objetos del modelo y los de los permisos no pueden divergir.
- La primera entrega de #69 incluye `sef-authz`, los roles base, CLS en tres niveles, la integración antes del motor de corrección y pruebas negativas y de propiedades. La de "ninguna combinación de roles ve una métrica sensible sin `view`" usa `proptest`.
