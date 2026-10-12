# 0012 — Motor de permisos: Cedar dentro del motor semántico

**Estado:** aprobada el 2026-10-12 (Claude y Codex, decisión técnica), con la revisión de seguridad de Codex incorporada. **Concreta:** [0010](0010-seguridad-del-blueprint-en-g2.md) §A (punto único y motor de permisos con denegación por defecto), que pedía elegir el motor en su propio ADR tras leer el código de los candidatos. **Se apoya en:** ADR 0011 (permisos dentro del motor), las leyes 3 y 11 (`docs/producto/leyes-bi.md`) y el modelo de seguridad del propietario (documento 6, PR #85).

## Contexto
El motor de permisos (Issue #69) decide **quién puede hacer qué sobre qué objeto**: métrica, dimensión, dataset, dashboard, modelo, fuente o usuario. Debe:
- funcionar con roles y atributos (RBAC + ABAC);
- denegar por defecto, también ante cualquier error;
- explicar cada decisión (para la auditoría, #71);
- vivir **dentro del núcleo**, sin tokio ni red (ADR 0008 y 0011);
- tener políticas versionadas como archivos (ADR 0003).

La seguridad por filas (RLS) ya existe en CSMIR: políticas tipadas con parámetros `ContextSlot`, exigidas por el motor de corrección (#83) y aplicadas por el planificador (`PolicyGuard`, #86).

## Candidatos, leídos en su código (2026-10-11)
| Candidato | Código | Encaje |
|---|---|---|
| **Cedar** (`cedar-policy` 4.13.0, Apache-2.0) | `cedar-policy/cedar@56c9706` | Rust nativo, **sin tokio ni red**, evaluación síncrona. Lenguaje de políticas con esquema y validación estricta. Su **semántica está modelada formalmente en Lean** y la implementación Rust se contrasta con ese modelo mediante pruebas diferenciales: eso no demuestra la implementación Rust ni nuestra integración. `cedar-policy-symcc` demuestra propiedades **del conjunto de políticas** (`check_implies`, `check_equivalent`, `check_always_denies`) con contraejemplos. Evaluación parcial con tipos (`tpe`), experimental |
| **Casbin** (`casbin` 2.18.1, Apache-2.0) | `casbin/casbin-rs@ab8b1e9` | `enforce` es síncrono (`CoreApi`) y tokio es opcional, pero la construcción y la carga de políticas son asíncronas (`async-trait`). Los *matchers* son *scripts* `rhai` evaluados como texto. Sin validación de esquema ni análisis formal |
| **OpenFGA** (Go, Apache-2.0) | `openfga/openfga@526995e` | Servicio aparte (modelo Zanzibar, permisos por relación). Exigiría una llamada de red en el camino de cada consulta |
| **OPA** (Go, Apache-2.0) | `open-policy-agent/opa@b86655f` (`Rego.Partial`, evaluación parcial para filtrar datos) | Se usa como servicio, como biblioteca Go o compilado a Wasm. Desde Rust, en proceso, la vía es Wasm. Rego es un segundo lenguaje para los autores. Su evaluación parcial para filtrar datos es madura |

**Prueba real con Cedar** (`cedar-policy =4.13.0`, sin funciones experimentales, Windows, perfil de depuración):
- el esquema `User in [Role]`, `Metric { certified, sensitive }` y la acción `view` se validan en modo estricto;
- un `viewer` ve la métrica certificada y no sensible, y se le **deniega** la no certificada y la sensible;
- cada decisión devuelve la política que la permitió;
- **≈30 µs** por decisión (dato de esa prueba, no un límite);
- **90 crates**, ninguno de ellos tokio, todos con licencias de la lista de `backend/deny.toml`.

## Decisión
1. **Cedar** decide los permisos: RBAC + ABAC con denegación por defecto.
   - Vive en un crate de núcleo nuevo, **`sef-authz`**, que depende de `cedar-policy` con versión fijada y **sin funciones experimentales**.
   - Expone `authorize(snapshot, principal, acción, recurso, contexto) → Decision`.
   - El motor semántico la consulta **antes** del motor de corrección para cada objeto que la consulta usa (punto 3).

2. **Denegación ante cualquier error.** El autorizador de Cedar omite una política cuya evaluación falla (`ErrorHandling::Skip`, `authorizer.rs`): un `permit` general junto a un `forbid` que falla por un atributo ausente **permitiría**. Por eso `sef-authz`:
   - deniega si la respuesta trae cualquier error de evaluación;
   - deniega si una entidad o el contexto de la solicitud no validan contra el esquema, o si el principal, la acción o el recurso son desconocidos. La validación estricta de las políticas no sustituye validar las entidades de cada solicitud;
   - rechaza cargar un conjunto de políticas que no valide: el anterior sigue vigente y el error va al panel de Problemas.

   Pruebas negativas obligatorias: `permit` + `forbid` con error, atributo requerido ausente, y principal, acción o recurso desconocidos.

3. **Contrato de la decisión.** `Decision` contiene:
   - `allowed`;
   - un **motivo estable**: `permitted`, `no_permit` (ningún `permit` aplica, aunque no haya política determinante), `forbidden`, `evaluation_error`, `invalid_request`;
   - las **políticas determinantes**;
   - las **restricciones** que el motor debe aplicar: máscaras de columna (punto 5) y políticas de filas (punto 6);
   - una **huella** que cubre la versión del conjunto de políticas, la del esquema, la acción, el recurso y los atributos del principal y del contexto que intervinieron.

   Instantánea y caché:
   - cada consulta se evalúa contra una **instantánea inmutable** del conjunto de políticas y del esquema, con su versión;
   - recargar las políticas publica una instantánea nueva;
   - la clave de caché de resultados incluye la huella de la decisión. Cambiar una política o un atributo del usuario (revocación) cambia la huella e invalida lo cacheado.

   Toda decisión se audita (#71), también las denegaciones sin política determinante.

4. **Acciones y roles iniciales**, los del documento 6 del propietario:
   - acciones: `view`, `view_masked` y `use_in_dashboard` (métrica y dimensión); `view`, `edit`, `share` y `delete` (dashboard); `view`, `propose_change`, `approve` y `publish` (modelo semántico); `connect` y `configure` (fuente); `manage` (usuario y rol);
   - roles base `viewer`, `analyst`, `modeler` y `admin`, como políticas por defecto **editables** del proyecto.

5. **Seguridad por columnas (CLS).** La decide el motor antes de ejecutar. Nunca se resuelve quitando columnas en silencio ni en el frontend.
   - **`view`:** el objeto se usa sin límites: selección, filtro, orden, agrupación y linaje de otras métricas.
   - **`view_masked` sin `view`:** el objeto solo puede **proyectarse** y sale enmascarado.
     - La máscara es tipada: el valor es `NULL` del tipo declarado y la columna lleva `masked: true` en los metadatos del resultado. El contrato de tipos no cambia.
     - Cualquier otro uso **se deniega** antes de ejecutar: filtrar por el valor original, ordenar, agrupar (revelaría su distribución), unir por él o usarlo en una métrica derivada.
   - **Sin ninguna de las dos:** cualquier uso deniega la consulta entera con motivo estable y el objeto afectado. No se elimina la selección ni se altera el grano.
   - **`forbid`** prevalece sobre `view` y `view_masked`.
   - **Linaje:** una métrica derivada exige `view` sobre **todas** las métricas y dimensiones de su linaje. Así no se puede deducir una cifra restringida a partir de un cociente o una diferencia.

   Pruebas negativas obligatorias: filtrar por el valor original, ordenar por él, agruparlo, derivarlo en una métrica y pedirlo sin permiso. Ninguna debe ejecutar SQL.

6. **La seguridad por filas sigue en CSMIR**: políticas tipadas con `ContextSlot`.
   - Los valores de sus parámetros salen de los **atributos verificados** del usuario (OIDC, #70), nunca de texto de la consulta.
   - Derivar filtros por fila de los residuos de Cedar (`tpe`) queda aplazado mientras sea experimental.

7. **Esquema y recursos neutrales** (ADR 0008).
   - `sef-authz` recibe **contratos de recurso neutrales**: tipo, identificador y atributos tipados. No depende de módulos de producto.
   - El esquema de Cedar, versionado, se compone de dos partes:
     - la **semántica** (métricas, dimensiones, datasets y modelo), generada desde CSMIR;
     - la **de plataforma** (dashboards, fuentes, usuarios y roles), declarada por el borde (`bi-server`) como fragmento versionado.
   - Los bordes aportan las entidades de cada solicitud. `sef-authz` las valida contra el esquema compuesto (punto 2).

8. **Políticas como archivos:**
   - `policies/*.cedar` y el fragmento de plataforma del esquema, en Git;
   - validación estricta al cargar, con errores al panel de Problemas.

9. **Verificación de cambios** (siguiente paso, no en la primera entrega): usar SymCC para demostrar en el plan de un cambio de políticas que **no amplía** el acceso (`check_implies`), o enseñar el contraejemplo.
   - Necesita el resolvedor `cvc5` como herramienta de desarrollo y de CI, nunca en el servidor.
   - SymCC razona sobre las políticas, no sobre el SQL, la caché ni la CLS: esas fugas las cubren las pruebas del punto único (ADR 0010).

## Alternativas descartadas
- **Casbin:** su carga y sus adaptadores son asíncronos y sus reglas son *scripts* `rhai` como texto, sin validación de esquema ni análisis formal. Cedar ofrece un contrato síncrono y validado.
- **OpenFGA:** una llamada de red en el camino de cada consulta. Queda como **adaptador futuro** si se necesita compartir por relaciones a gran escala.
- **OPA:** en proceso exige Wasm y Rego, un segundo lenguaje para los autores; como servicio, una llamada de red. Su evaluación parcial para filtrar datos es la referencia conceptual del punto 6.
- **Motor propio:** reinventar un lenguaje de políticas sin modelo formal ni herramientas de análisis.

## Consecuencias
- Dependencia nueva: `cedar-policy` fijada en `=4.13.0`. Una actualización se revisa como un cambio de seguridad.
- La parte semántica del esquema se genera desde CSMIR, así que los objetos del modelo y los de los permisos no pueden divergir. La de plataforma la versiona el borde que la declara.
- La primera entrega de #69 incluye:
  - `sef-authz` con denegación ante errores y validación de entidades;
  - `Decision` con motivo, restricciones y huella;
  - instantánea inmutable de políticas;
  - los roles base y CLS en tres niveles;
  - la integración antes del motor de corrección;
  - pruebas negativas (puntos 2 y 5) y de propiedades. La de "ninguna combinación de roles ve una métrica sensible sin `view`" usa `proptest`.
