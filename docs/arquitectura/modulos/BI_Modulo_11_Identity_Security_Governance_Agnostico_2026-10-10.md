# Plataforma BI abierta — Módulo 11: Identity, Authorization, Data Security & Governance

**Edición:** 1.0 — 10 de octubre de 2026  
**Estado:** investigación de código abierto + arquitectura propuesta; no implementada.  
**Producto:** plataforma BI nueva, aún sin nombre; **no depende de SQLVIZ**, de `.sqlviz` ni de un fabricante de gráficos, un almacén o un motor SQL.  
**Alcance exclusivo:** identidad, permisos de recursos, seguridad semántica de filas/campos, límites de ejecución, secretos, compartición, auditoría y gobernanza técnica.  
**No contiene:** implementación completa de autenticación, ETL, optimización SQL, Semantic Engine, UI o materialización. Se especifican sus fronteras y las pruebas que deben respetar.  
**Bases documentales:** Rill `v0.90.4` (tag fijo); Lightdash `main`, Cube `master`, Grafana `main` y Superset `master` consultados a 10/10/2026 (referencias mutables). Se inspeccionaron rutas concretas en GitHub; no se ejecutaron sus suites de pruebas ni se auditó una instalación.

---

## 00. Dictamen ejecutivo

**Diseñar una capa de autorización y gobernanza independiente, transversal y obligatoria en cada ruta de acceso a datos.** No tratar la seguridad como un `WHERE` opcional dentro del dashboard ni como permisos exclusivos de su interfaz.

Decisiones:

1. **Identidad federada** con OIDC/OAuth2 para entornos conectados, y configuración local adecuada para uso individual. Evitar implementar un proveedor de identidad propio.
2. **Authorization Core en Rust** con interfaz `authorize(principal, action, resource, context)` y resolución de decisiones `ALLOW`, `DENY`, `NOT_APPLICABLE`, `INDETERMINATE` (las dos últimas nunca dan acceso).
3. **Dos motores lógicos diferentes:** Resource Authorization (qué puede hacer alguien con cada recurso) y Data Policy Enforcement (qué filas, medidas, dimensiones y valores puede obtener). Comparten actor y trazabilidad, pero **no** la misma abstracción SQL.
4. **Punto de aplicación central en el backend:** metadatos, ejecución semántica, previews, búsqueda, Pivot, Canvas, APIs, exportaciones, agentes y tareas programadas.
5. **RLS y autorización no son filtros de usuario:** un modificador como `REMOVEFILTERS`, la navegación o la exportación jamás elimina la política de seguridad.
6. **Políticas declarativas y versionadas, no nuevo DSL obligatorio:** contrato tipado de plataforma; evaluar Cedar (Rust) para decisiones de acceso; OpenFGA/OPA opcionales para escenarios que los justifiquen.
7. **Fail closed y privilegios mínimos:** claves de cache con contexto de seguridad; revocación; secretos referenciados; consultas SQL de autoría aisladas del servicio de lectura de dashboards.
8. **Portabilidad:** recurso Policy y contratos neutrales respecto a Git, SQLite, PostgreSQL, ECharts, DuckDB, ClickHouse y proveedor de identidad.
9. **MVP progresivo:** identidad, roles por recurso, un RLS tipado y CLS/members, permisos de exportación, aislamiento y auditoría. Delegación federada, ABAC avanzada y relaciones organizativas complejas posteriormente.
10. **La seguridad real debe verificarse mediante pruebas adversariales**, no mediante vistas preliminares o diagramas.

**Lección rectora:** tener permiso para **ver un dashboard** no implica tener permiso para **leer todos los datasets que aparecen en él**. El segundo permiso no se infiere del primero.

---

## 01. Qué pertenece a este módulo

| Concepto | Propiedad de M11 | Ejemplo |
|---|---|---|
| Autenticación | Valida identidad y sesión; consume IdP | OIDC/JWT, usuario local autorizado |
| Identidad y principal | Actor estable + pertenencias verificadas | usuario, equipo, servicio, agente |
| Autorización recursos | Acciones específicas sobre IDs/versiones | `dashboard:publish`, `model:edit` |
| Relaciones | Herencia explícita equipo/proyecto/carpeta | miembro del equipo analítico |
| Seguridad de datos | Filtros RLS/CLS y visibilidad semántica | municipio autorizado; ocultar importe individual |
| Policies | Definiciones, revisión, evaluación y auditoría | `policy_revision_id` |
| Credentials | Referencias a secretos, acceso delegado | `secret_ref`; nunca contraseñas exportadas |
| Share/embed | Tokens restringidos por recurso/expiración | dashboard embebido de solo lectura |
| Eventos de auditoría | Evidencia de acceso/publicación/exportación | identidad, recurso, decisión, tiempo |
| Clasificación | Sensibilidad, finalidad y propietarios | `confidential`, `restricted` |

**No** le corresponde ejecutar SQL analítico, inferir relaciones semánticas, administrar datos Parquet, dibujar paneles ni decidir el layout. Entrega restricciones autorizadas a esos motores.

---

## 02. Hallazgos reproducibles: Rill (Go, tag v0.90.4)

### 02.1 Autorización integrada en el AST de Metrics View

Archivo fijo: [`runtime/metricsview/ast.go`](https://github.com/rilldata/rill/blob/v0.90.4/runtime/metricsview/ast.go).

- `SecurityFilterSQL` compila el filtro de consulta y de filas de una política de Metrics View a expresión SQL y argumentos aplicables al `WHERE`.
- `LookupMeasure(..., visible=true)` comprueba `CanAccessField` y rechaza medidas no autorizadas.
- La seguridad se contempla **antes** de terminar la construcción de SQL, no como un postprocesado JavaScript de los resultados.

**Rescatar:** compilar las restricciones autorizadas en la ruta semántica; permitir visibilidad individual de dimensiones y medidas; mantener argumentos separados de SQL.

**Mejorar:** que el motor no dependa de SQL escrito como política arbitraria: definir un `PolicyPredicate` tipado, compilarlo por dialecto y rechazar predicados no traducibles; diferenciar restricciones del usuario y restricciones inviolables. Esta mejora es propuesta propia, no una descripción de Rill.

### 02.2 Seguridad en operaciones del ejecutor

Archivo fijo: [`runtime/metricsview/executor/executor.go`](https://github.com/rilldata/rill/blob/v0.90.4/runtime/metricsview/executor/executor.go).

- `Query`, `Export`, `Search` y `Schema` verifican acceso mediante `e.security.CanAccess()`.
- El ejecutor distingue rutas de datos y de metadatos, detalle esencial para cerrar canales secundarios.
- **Excepción explícita:** `Timestamps` no aplica filtros RLS, intencionadamente, para compartir límites temporales/caché y mantener intervalos comunes. En entornos sensibles podría revelar que existen datos fuera del intervalo autorizado.

**Rescatar:** asegurar operaciones más allá del dashboard, especialmente exportación y búsquedas.

**Mejorar:** metadatos temporales y facetas por defecto **scoped** al usuario; cualquier rango global solo si existe permiso específico de ver metadatos globales. Comprobar capacidades en todos los métodos, incluso `schema`, `search`, `distinct`, `min/max`, `cardinality`, `export` y preview.

### 02.3 Declaración de políticas

Referencia: [Metrics View YAML — `security`](https://docs.rilldata.com/reference/project-files/metrics-views).

Rill documenta `access`, `row_filter` y lista de medidas/dimensiones incluidas. Las expresiones pueden depender de atributos de usuario. El lenguaje de las condiciones y sus efectos dependen de la versión y el recurso: **no** reproducir literalmente esa sintaxis sin pruebas de compatibilidad.

**Rescatar:** políticas cercanas al modelo semántico y participación de atributos del actor.

**Mejorar:** propiedad inequívoca de política, herencia, prioridad, combinación de varias políticas y simulación explicativa. Evitar la ambigüedad entre «no hay regla» y «acceso total».

### 02.4 Limitación conocida del análisis

Esta auditoría mira rutas de fuente concretas, no audita todo el sistema de autenticación multi-tenant de Rill. No debemos extrapolar el comportamiento de estas funciones a cada endpoint de Rill ni atribuirle fallas de seguridad sin una prueba explotable.

---

## 03. Hallazgos reproducibles: Lightdash (TypeScript)

Archivos concretos:

- [`packages/common/src/authorization/index.ts`](https://github.com/lightdash/lightdash/blob/main/packages/common/src/authorization/index.ts)
- [`packages/common/src/authorization/scopeAbilityBuilder.ts`](https://github.com/lightdash/lightdash/blob/main/packages/common/src/authorization/scopeAbilityBuilder.ts)
- [`.context/PERMISSIONS.md`](https://github.com/lightdash/lightdash/blob/main/.context/PERMISSIONS.md)
- [`packages/backend/src/services/CLAUDE.md`](https://github.com/lightdash/lightdash/blob/main/packages/backend/src/services/CLAUDE.md)

### 03.1 Qué hace

Lightdash utiliza **CASL** para construir habilidades a partir de funciones de organización, roles de proyecto, roles personalizados y scopes. La lógica de control de acceso se evalúa en los servicios backend. Sus guías internas insisten en la evaluación **auditada**, no solamente en `ability.can()` de la UI. El código `buildAbilityFromScopes` separa scopes válidos/ inválidos antes de aplicar reglas.

### 03.2 Lo que rescataríamos

- Vocabulario explícito de `action` + `resource` + condiciones.
- Reutilizar una misma resolución de permisos para usuarios y cuentas de servicio.
- Auditoría de decisiones dentro del servicio.
- Scopes versionados y comprobación de su validez.
- Identidad de recurso estable: UUID de organización/proyecto en lugar de confiar en slugs ambiguos.

### 03.3 Mejoras posibles

- **Denegaciones y límites no acumulativos:** la propia guía de Lightdash advierte que ciertos permisos CASL son aditivos; una regla permisiva de nivel superior puede dejar sin efecto la intención de restringir abajo. Nuestro motor definirá jerarquías de decisión y denegación explícita, con pruebas.
- Política persistente versionada; cambios de vocabulario de scopes deben migrarse, no quedar como strings desconocidos.
- Aislamiento de actores de servicio: los mismos `PolicyDecision` y `PolicyContext` para human/service, evitando conjuntos paralelos de roles que derivan con el tiempo.
- El frontend oculta operaciones no permitidas por UX, pero el backend siempre reevalúa.

### 03.4 Precaución de evidencia

Las referencias `main` son mutables y contienen documentación interna de desarrollo que puede no coincidir exactamente con despliegues anteriores. Fijar SHA antes de derivar tests o contratos obligatorios. La existencia de una política recomendada **no prueba** que cada endpoint de Lightdash la cumpla.

---

## 04. Hallazgos reproducibles: Cube (Core y Cloud)

Código de servidor: [`cube/packages/cubejs-server-core/src/core/server.ts`](https://github.com/cube-js/cube/blob/master/packages/cubejs-server-core/src/core/server.ts).

Se observan opciones `checkAuth`, `queryRewrite`/`queryTransformer`, `extendContext`, `contextRejectionMiddleware` y aceptación de contexto WebSocket en la construcción del API Gateway. Esto demuestra separación de identidad, transformación de consulta y control del contexto.

Fuentes de política:

- [Cube Core 1.2 — Access Policies](https://cube.dev/blog/cube-core-v1-2-data-access-policies-hierarchies-and-folders-updates-to)
- [Políticas declarativas y casos RLS/CLS](https://cube.dev/blog/introducing-data-access-policies-in-cube-cloud)

**Rescatar:** el `securityContext` como entrada tipada y verificada; política declarativa cercana a cubos/vistas; control por medida/dimensión y filtro de filas; compatibilidad entre APIs y semantic planning.

**Mejorar:** que la autorización sea un paso imposible de omitir por construcción del plan; evitar callbacks que puedan ampliar privilegios; trazas que indiquen exactamente qué políticas restringieron el plan sin revelar su contenido a quien no tenga permiso.

**Aclaración de edición:** Cube Core documentó Data Access Policies en v1.2; el editor gráfico/visualizador de políticas mencionado en otros artículos es capacidad de producto Cloud o de versiones concretas. No asumir paridad automática Core–Cloud.

---

## 05. Hallazgos reproducibles: Grafana (Go)

Código: [`pkg/services/accesscontrol/accesscontrol.go`](https://github.com/grafana/grafana/blob/main/pkg/services/accesscontrol/accesscontrol.go); [`pkg/api/api.go`](https://github.com/grafana/grafana/blob/main/pkg/api/api.go).

Grafana expone `AccessControl.Evaluate` y evaluadores parametrizables; las rutas API muestran autorización diferenciada para leer/editar dashboards, consultar fuentes y administrar plugins.

[La documentación RBAC](https://grafana.com/docs/grafana/latest/administration/roles-and-permissions/access-control/) modela privilegios como `action` y `scope`. Las capacidades granulares y roles personalizados **no tienen la misma disponibilidad en OSS que en Enterprise/Cloud**.

**Rescatar:** capacidades distintas de leer metadatos de una fuente, ejecutar la consulta y editar el dashboard; acciones y scopes estables; auditoría y validación de permisos por ruta.

**Mejorar:** separar «ver panel» de «ejecutar su SQL» y «exportar datos» desde el contrato, con restricciones por tenant y dataset que ni el plugin gráfico ni el compartido puedan ampliar.

---

## 06. Hallazgos reproducibles: Apache Superset (Python)

Fuentes: [`superset/security/manager.py`](https://github.com/apache/superset/blob/master/superset/security/manager.py), [configuración de RLS](https://github.com/apache/superset/blob/master/docs/admin_docs/security/security.mdx), [`superset/row_level_security/api.py`](https://github.com/apache/superset/blob/master/superset/row_level_security/api.py).

Superset permite políticas RLS por dataset y sujetos (roles/grupos/usuarios), combinadas en la generación de SQL; distingue reglas base y regulares, y ofrece API para administrar reglas.

**Rescatar:** administración explícita de RLS, grupos como sujetos para conjuntos de recursos, inspección de reglas y auditoría.

**Mejorar:** en nuestro sistema las reglas nuevas se expresarán mediante campos/operadores tipados, no por fragmentos SQL interpolables y plantillas Jinja de usuario. Las expresiones avanzadas requieren validación exhaustiva, revisión y pruebas por dialecto.

**Nota:** no se afirma que Superset tenga una vulnerabilidad concreta: solo comparamos alternativas de diseño de la superficie de configuración.

---

## 07. Comparativa y decisiones para la plataforma nueva

| Capacidad | Rill | Lightdash | Cube | Grafana | Superset | Plataforma propuesta |
|---|---|---|---|---|---|---|
| Seguridad en consulta semántica | Sí | Sí | Sí | Depende del datasource | Sí | Invariante en pipeline |
| Atributos usuario para filas | Sí | Configurable según función | Sí | Plugin/datasource | Sí | Atributos verificados y tipados |
| Medidas/dimensiones visibles | Sí | Por permisos/modelado | Sí | Varía con datasource | Según dataset | IDs de miembro y CLS explícita |
| Acciones por recurso | Parcialmente | CASL scopes | Políticas de API | Acciones y scopes | FAB/RBAC | RBAC/ABAC propio de contrato |
| Separación AuthN/AuthZ | Sí | Sí | Sí | Sí | Sí | Obligatoria |
| RLS sin campo SQL arbitrario | No universal | Depende de ruta | Sí en políticas declarativas | No universal | No universal | Predicado tipado por defecto |
| Contexto inmutable de seguridad | Variable por ruta | Backend abilities | Security context | Evaluator | Request principal | Obligatorio |
| Explain de autorizaciones | Varía | Logs auditados | Existen herramientas según edición | RBAC UI | APIs | `PolicyDecisionTrace` redaccionada |

**No convertir esta tabla en una clasificación de seguridad o en un ranking de vulnerabilidades:** es una comparativa de funcionalidades documentadas y áreas arquitectónicas. Las opciones por edición/versiones difieren.

---

## 08. Modelo de amenazas explícito

Actores: usuario autenticado normal, desarrollador de Models, editor de dashboards, administrador de proyecto, cuenta de servicio, visitante embebido, agente con herramientas y atacante sin autenticar.

Activos: datos, definiciones semánticas, secretos, rutas de fuentes, tokens, cache, artefactos exportados, dashboards, jobs, logs y snapshots.

Fronteras de confianza:

```
Browser / Embedded Viewer / Agent
        │ (untrusted request)
        ▼
API Gateway — authentication, rate limiting, request validation
        │ verified ActorContext
        ▼
Authorization Core — resource & action decision
        │ AuthorizedRequest
        ▼
Semantic Security Compiler — data policy + authorized members
        │ AuthorizedSemanticPlan
        ▼
Query Coordinator — quotas, cache scope, cancellation
        │ governed EngineRequest
        ▼
SQL Engine Adapter — bounded credentials and permissions
        │
        ▼
Dataset / Warehouse / Lakehouse
```

Amenazas prioritarias:

| Amenaza | Control | Condición comprobable |
|---|---|---|
| IDOR de dashboard/modelo | Comprobación por ID estable | No se puede pedir otro proyecto modificando URL |
| SQL arbitrario bajo token de visualizador | Roles de servicio/aislamiento | `ATTACH/COPY/INSTALL` no accesibles por lector |
| Saltarse RLS con filtro visual | Separación de contextos | El cálculo nunca altera `SecurityPredicate` |
| Caché entre usuarios | Key con policy scope/revisión | A y B no comparten resultados restringidos |
| Fuga por facetas/búsqueda | Policy antes de distinct/search | Valores exclusivos de B no aparecen a A |
| Fuga por rangos temporales | Metadata con seguridad | Intervalos prohibidos no se infieren por defecto |
| Enlace público permisivo | Token capability restringido | No permite abrir dataset base ni ampliar scope |
| Panel con plugin malicioso | Renderer sin acceso a tokens | Plugin nunca obtiene credenciales ni SQL privilegiado |
| Export masivo | Autorización/limites/auditoría | XLSX/CSV/Parquet sujetos a mismas políticas |
| Tablas materializadas de alto privilegio | Serving dataset controlado | Agregado no desanonimiza/omite RLS |
| Revocación tardía | Versionado policy/ACL | Revalidación en acciones sensibles |
| Token filtrado en logs | Secret redaction | Logs no contienen claves/cadenas JWT |
| Carga de config maliciosa | Validación y sandbox | YAML/SQL importado no ejecuta acciones automáticamente |
| AI/agent prompt injection | Tools autorizadas y delimitadas | Texto fuente no concede nuevas capacidades |
| Confused deputy | Delegación verificable | Acceso backend no excede privilegios aprobados |

---

## 09. Diseño modular, responsabilidades y contratos

**M11 no es un microservicio obligatorio.** Es un módulo lógico implementable como crates Rust y adaptadores en un mismo proceso.

```text
security-governance/
  contracts/         # ActorContext, PolicyDecision, AuthorizedQuery
  authentication/    # OIDC integration, sessions, token verification ports
  identity/          # user, group, service principal, tenant
  authorization/     # roles, resource grants, policy evaluator
  data-policy/       # typed predicates, member ACLs, masking rules
  delegation/        # embed, share, service account, agent context
  secrets/           # references, vault/keychain adapter
  audit/             # events, writer/reader, retention policies
  governance/        # classifications, owners, certifications
  adapters/          # cedar (optional), openfga (optional), oidc, stores
  tests/             # fixtures, model checks, adversarial scenarios
```

Contratos principales:

| Contrato | Dueño | Contenido mínimo |
|---|---|---|
| `ActorContext` | AuthN | principal_id, kind, tenant_id, trusted claims, expiry, token-id |
| `ResourceAction` | AuthZ | versionada: list/read/edit/publish/query/export/share/admin |
| `ResourceRef` | Metadata M10 | ID global, tipo, workspace, revision |
| `AuthzRequest` | AuthZ | Actor + acción + recurso + environment |
| `PolicyDecision` | AuthZ | allow/deny, policy_revision, audit_reason redaccionada |
| `DataPolicy` | Semantics/M11 | dataset, allowed members, typed row predicate, masks |
| `AuthorizedSemanticQuery` | M11 → Semantic | Immutable security constraints, query intent, actor scope |
| `ExecutionPrincipal` | M11 → Engine adapter | CredentialRef, privileges, tenant/job identity |
| `SecurityCacheScope` | M11 → Serving | actor/role/tenant/policy hash + dataset/semantic revision |
| `AuditEvent` | M11 | timestamp, actor, action, resource, decision, trace, context |

### Semántica de decisiones

- `ALLOW` requiere concesión válida y ausencia de prohibición aplicable.
- `DENY` explícito gana cuando el modelo aprobado lo prevé.
- `NOT_APPLICABLE` implica **denegado por defecto**.
- `INDETERMINATE` por error de carga, parser, expiración o timeout implica **denegado** y alarma operativa.
- Las políticas son revisiones inmutables con puntero publicado, como los recursos del M10.
- Las restricciones del motor SQL no sustituyen el control de la aplicación; en sistemas con RLS nativo, sirven como **defensa en profundidad**.

---

## 10. Identidad y autenticación

### 10.1 Tipo de actores

`User`, `ServiceAccount`, `EmbeddedViewer`, `AgentDelegate`, `SchedulerJob` y `SystemOperator` comparten interfaz de principal; no comparten necesariamente permisos.

**Identidad verificable:** `issuer`, `subject`, `audience`, `tenant`, `issued_at`, `expires_at`, `session_id`, `authn_strength` y membresías obtenidas de fuente confiable. No derivar tenants ni permisos desde query params manipulables.

### 10.2 Integración OIDC

Verificar firma contra JWKS, `iss`, `aud`, `exp`, `nbf`, nonce/estado donde aplique, política de algoritmo, revocación/sesión y rotación de claves. No confiar en los campos sin verificar del JWT.

Para servicio-a-servicio: tokens acotados al recurso, identidad laboral separada, caducidad breve, scopes explícitos, rotación y atribución.

### 10.3 Autoría frente a lectura

Diseñadores de modelos pueden requerir SQL de preparación más amplio; lectores de dashboards no deben disponer de ese privilegio. No ejecutar un script de Model bajo el token de un espectador, ni usar tokens de administrador para consultar dashboards.

### 10.4 Modo local/offline

Puede existir proveedor local de identidad/permiso para instancias personales. No implementar multiusuario seguro simplemente añadiendo contraseñas a una base de datos: exige políticas, cifrado, recuperación y pruebas adicionales.

---

## 11. Resource Authorization: roles, atributos y relaciones

### 11.1 Acciones mínimas

`workspace:list`, `source:discover`, `source:manage`, `model:view`, `model:edit`, `model:run`, `model:publish`, `metrics:view`, `metrics:edit`, `explore:query`, `dashboard:view`, `dashboard:edit`, `dashboard:publish`, `dataset:query`, `dataset:export`, `user:share`, `policy:admin`, `secret:use`, `audit:read`.

Estas cadenas son **ejemplos de vocabulario**, no un nuevo lenguaje de consultas. Versionar IDs de permisos; no reutilizar un identificador antiguo con nueva semántica.

### 11.2 Roles

Roles iniciales sugeridos: `viewer`, `analyst`, `modeler`, `publisher`, `workspace_admin`, `service_principal`. Roles son plantillas de concesiones, no «superpoderes» por definición; cada recurso se comprueba.

### 11.3 Herencia

Un permiso sobre workspace puede propagar una capacidad a proyectos y dashboards, salvo denegación explícita declarada. Registrar el árbol y cómo se resuelven precedencias; no asumir que `editor` implica `export` ni que leer un dashboard implica editar el Model.

### 11.4 Multi-tenant

El `tenant_id` es condición obligatoria del contexto para un backend compartido. Todas las consultas de metadatos y datos lo aplican; las rutas de almacenamiento/cache incluyen ámbito de tenant. Si hay recursos compartidos entre tenants, exigir política específica y auditoría del vínculo.

### 11.5 Ownership y delegación

No confundir `owner` de un dashboard con administrador del dataset. Permitir grants temporales con expiración, delegación explícita y protección contra escalación (`can_share` no implica `can_grant_admin`).

---

## 12. Data Security: RLS, CLS y visibilidad semántica

### 12.1 Regla invariable

```
EffectiveQuery = UserAnalyticContext ∩ AuthorizedDataDomain
```

El símbolo denota intersección lógica, no concatenación textual. La aplicación convierte el contexto a un plan tipado. No se permite que filtros analíticos, comparaciones temporales, porcentajes del total o medidas derivadas borren `AuthorizedDataDomain`.

### 12.2 Dos niveles de autorización

1. **Miembro semántico**: determinar si una medida o dimensión es visible/consultable.
2. **Fila/columna física**: aplicar RLS y CLS/masking en datasets base y relaciones; el permiso para usar una medida no revela sus columnas subyacentes.

### 12.3 Política tipada ilustrativa

```yaml
api_version: bi.open/v1
kind: DataPolicy
id: sales-by-region
subject_selector:
  roles: [regional_analyst]
target:
  semantic_model: sales
permissions:
  measures: [revenue, quantity, margin_pct]
  dimensions: [region, product, date]
row_filter:
  op: eq
  field_id: sales.region_id
  value_from_claim: allowed_region_id
on_missing_claim: deny
```

No es sintaxis de Rill ni Cube, ni una norma aprobada. Es una representación portable propuesta; los *claims* deben validarse desde el proveedor de identidad o atributo autorizado.

### 12.4 Operadores del predicado

`eq`, `neq`, `in`, `lt`, `lte`, `gt`, `gte`, `is_null`, `not_null`, `and`, `or`, `not`, `between` con tipos comprobados; admitir transformaciones más complejas solo mediante extensiones validadas y registradas.

### 12.5 Combinación de políticas

Regla de referencia: permisos de recursos por unión de grants admisibles menos prohibiciones explícitas; **condiciones RLS obligatorias en intersección** salvo políticas alternativas declaradas para la misma categoría. No mezclar OR y AND entre grupos de forma implícita.

Ejemplo: equipo `Norte` obtiene regiones `1,2`; rol de auditor permitido en Región `2`. Si ambos son grants alternativos de acceso a filas, su unión puede ser `1,2`; si hay restricción regulatoria «solo región 1», intersectar la restricción regulatoria. Estas clases deben distinguirse en metadatos.

### 12.6 Totales y ratios

`REMOVEFILTERS(region)` puede quitar el filtro elegido del dashboard, pero no la región obligatoria del actor. Un porcentaje del total debe aclarar si total = universo autorizado, total visible o total público; la definición semántica indica el tipo.

### 12.7 JOIN entre hechos

RLS del consumidor y RLS de pagos aplican antes de combinar hechos. El compilador valida propagación, cardinalidad y versiones. Una política de un hecho no debe dejar accesibles dimensiones de otro sin comprobar autorización.

### 12.8 Campos ocultos y transformaciones

CLS impide seleccionar, ordenar, filtrar, agrupar, buscar o inferir valores prohibidos cuando esas operaciones permitan filtrar información. Una columna física restringida puede participar en una medida agregada solo si la política autoriza expresamente el indicador derivado y se satisfacen reglas de divulgación.

### 12.9 Mascaramiento

`mask` es una transformación diferente de `deny`: email parcialmente oculto, identificador pseudonimizado, redondeo o supresión. Definir propósitos, consumidores y estabilidad por actor; un agregado reversible por demasiados filtros puede violar privacidad aunque use máscara.

---

## 13. Planner seguro y SQL generado

```text
ValidatedIdentity
  ↓ authorize(resource action)
AuthorizedModelRef + PolicyRevision
  ↓ resolve allowed dimensions/measures + security RLS
SecuredSemanticIR (immutable)
  ↓ compile dialect (DuckDB, ClickHouse, future)
SQL + bound parameters + security provenance
  ↓ policy-aware query coordinator
Results under bounded execution principal
```

**Reglas técnicas:**

- Utilizar AST SQL, escapado de identificadores y parámetros vinculados; nunca formar `WHERE` con interpolación de valores no confiables.
- Solo fuentes semánticas y campos registrados; no permitir `SELECT *` ad hoc sobre tablas protegidas desde APIs de visualizador.
- Política aplicada en etapa correcta: antes de agregación y antes de rollup elegible; comprobación de herencia después de joins.
- Cada compiled plan incorpora `security_policy_revision`, actor_scope y `resolved_dataset_revision` en la procedencia.
- Prohibir compilaciones con campos restringidos incluso cuando el resultado aparente no devolverlos (filtros y ORDER BY también pueden filtrar información).
- Si el adaptador SQL no ofrece una operación segura o no puede validar RLS, fallar en lugar de degradar a una consulta no protegida.
- Filtros de seguridad preparados para extensiones de lenguaje sin cambiar el contrato de negocio.

### Ejemplo conceptual, DuckDB

```sql
SELECT municipio_id, SUM(importe) AS importe
FROM facturacion
WHERE municipio_id = ?
  AND fecha_emision >= ?
  AND fecha_emision < ?
GROUP BY municipio_id;
```

El primer parámetro proviene **exclusivamente del contexto de seguridad validado**; los otros son filtros temporales analíticos autorizados. La consulta es ilustrativa, no evidencia de que un `?` tenga el mismo uso en todas las interfaces remotas o dialectos.

---

## 14. Ejecución SQL arbitraria: frontera de alto riesgo

### 14.1 Tres tipos de workload

| Trabajo | Capacidad | Principal |
|---|---|---|
| Viewer semantic query | `SELECT` gobernado sobre modelos permitidos | viewer con scope de datos |
| Model preview/build | SQL de autoría, DDL selectivo, staging | modeler con entorno restringido |
| Administración del motor | extensiones/conexiones/configuración | operador o servicio privilegiado |

**No se pueden asegurar consultas SQL arbitrarias únicamente anexando un `WHERE`.** El lenguaje puede admitir `ATTACH`, `COPY`, funciones de ficheros, extensiones, subconsultas no previstas o expresiones con efectos secundarios. La separación de privilegios debe existir *por debajo* del compilador.

### 14.2 DuckDB como un adaptador, no la arquitectura universal

DuckDB documenta opciones `enable_external_access=false`, `disabled_filesystems`, `allowed_directories`, `allowed_paths`, `allow_community_extensions=false`, y bloqueo de configuración. Estas restricciones limitan acceso a archivos/extensiones, pero **no sustituyen** aislación de procesos/identidad ni controles de la API. Referencia: https://duckdb.org/docs/current/operations_manual/securing_duckdb/overview.

Una instancia remota con Quack requiere comprobar cómo se aplican estas configuraciones en el **servidor** y si el protocolo ofrece separación efectiva de roles. La existencia de un token Quack no equivale a RLS o RBAC semántico.

### 14.3 Adaptadores futuros

ClickHouse/PostgreSQL/Snowflake deben informar capacidades `supports_native_rls`, `supports_readonly`, `supports_query_budget`, `supports_statement_timeout`, `supports_credential_scopes`, `supports_sandbox` y su modo real. Un conector **no anuncia una capacidad por analogía** con otro.

---

## 15. Materializaciones, preagregados, caché y snapshots

### 15.1 Publicar no significa autorizar

M03 puede publicar un agregado materializado; M11 decide quién puede usarlo. La revisión física del dataset no hereda automáticamente todos los permisos de su tabla de origen.

### 15.2 Tres estrategias

1. **Materialización compartida de datos no sensibles**: aplicar RLS al consultar el resultado si conserva dimensiones necesarias y demuestra equivalencia.
2. **Materialización particionada por ámbito autorizado**: exigir identificadores de tenant/espacio y lifecycle específico.
3. **Materialización personalizada de actor**: usar solo cuando realmente se necesita y existe política clara de expiración/limpieza; coste mayor.

Si un rollup elimina la dimensión necesaria para RLS, **no es elegible** para esa consulta. La protección no se recupera sumando celdas después.

### 15.3 Cache key segura

```text
hash(
  tenant_id,
  authorized_scope_fingerprint,
  policy_revision,
  semantic_model_revision,
  dataset_snapshot,
  normalized_semantic_query,
  engine_version,
  relevant_timezone_and_locale
)
```

Los atributos sensibles no se guardan en texto en claves de caché; usar fingerprint estable y autorización comprobada al leer. TTL es complemento, no revocación suficiente por sí solo.

### 15.4 Revocación

Al cambiar membresía/política: invalidar referencias y objetos afectados, mantener `revocation_generation`, revalidar requests nuevos y flujos de exportación largos. Una consulta iniciada antes de revocación requiere política explícita de cancelación o finalización sin revelar resultados.

---

## 16. Exportaciones, APIs y agentes

### 16.1 Exportaciones

`ExportRequest` enlaza `SemanticQuery`, actor, permisos y revisión. CSV/Excel/Parquet deben utilizar el mismo plan gobernado; distinto formato no supone una ruta alternativa al almacenamiento.

### 16.2 API pública

Verificación por endpoint **y por objeto**. `GET /dashboard/{id}` comprueba capacidad de verlo, pero `POST /query` comprueba también métricas/datasets del dashboard. Evitar que el ID del panel o un token de widget se use como permiso general.

### 16.3 AI y agentes

El agente es un delegado de una persona o cuenta de servicio. Sus herramientas reciben `DelegationContext`, no claves de admin. Texto de tablas, comentarios SQL y descripciones de modelos son datos no confiables: nunca instrucciones con autoridad para `grant`, `export`, `share` o `alter`.

### 16.4 Compartir y embebido

Un enlace compartido debe ser una capability restringida: recurso/revisión, acciones admitidas, caducidad, audiencia, restricciones de dominio cuando se pueda, revocación. Se deben evitar URLs con credenciales o atributos de seguridad serializados. Compartición pública deshabilitada por defecto.

### 16.5 Query Service multi-API

REST, GraphQL, SQL API, embedded SDK, webhook o plugin visual deben pasar por el mismo `AuthorizationContext`. Si una nueva API no integra M11 se declara experimental o bloqueada; no publicarla sin seguridad.

---

## 17. Catálogo, descubrimiento y metadatos sensibles

RLS debe considerarse también para:

- Campos enumerados en autocompletado de SQL.
- Lista de medidas/dimensiones disponibles.
- Facetas, sugerencias, top-N y cardinalidades.
- Rangos de fechas, watermark y extremos de valores.
- Listados de dashboards y títulos/descripciones confidenciales.
- Linaje; un gráfico de dependencias puede revelar datasets prohibidos.
- Mensajes de error; evitar rutas físicas, tokens, SQL con valores secretos y stack traces.

Comportamiento preferido: no revelar existencia de recursos inaccesibles a usuarios ordinarios (404 o respuesta mínima), sin impedir registro interno del 403 para auditoría.

La exposición del período global encontrada en Rill refuerza la necesidad de distinguir `global_metadata_access` de `dataset_query_access`.

---

## 18. Política como recurso versionado del M10

```yaml
api_version: bi.open/v1
kind: ResourcePolicy
id: revenue-analyst-access
scope:
  workspace_ref: analytics
rules:
  - effect: allow
    principals:
      groups: [regional-analysts]
    actions: [semantic.query, dashboard.view]
    resources:
      semantic_models: [sales]
      dashboards: [sales-overview]
  - effect: deny
    principals:
      groups: [regional-analysts]
    actions: [dataset.export]
    resources:
      datasets: [customer-identifiable-details]
```

**Solo ejemplo conceptual:** la sintaxis real se fijará con schema formal antes de implementarla. El parser nunca ejecuta expresiones arbitrarias como JavaScript/Python. Se guarda ID, revisión, creador, fecha, validación, estado (`draft/approved/published/retired`) y dependencias.

La fuente de verdad puede ser archivo GitOps, repositorio SQL transaccional u otro adaptador del M10; no hay requisito de `.sqlviz` ni de una base central concreta.

### Publicación

1. Crear nueva revisión de política.
2. Validar sintaxis/esquema/IDs.
3. Ejecutar **simulaciones sobre actores representativos** y suite de regresión.
4. Comprobar impacto en recursos y snapshots dependientes.
5. Requerir revisión para cambios de alto privilegio.
6. Publicar puntero versionado atómicamente y activar invalidaciones.
7. Auditar actor que aprobó y actor que publicó.

---

## 19. Comparación de motores de políticas existentes (no reinventar DSL)

| Motor | Qué aporta | Ventaja | Coste/inconveniente | Adopción propuesta |
|---|---|---|---|---|
| **Cedar** | Autorización RBAC/ABAC con schema, Rust crates | Embebible, tipado/validación, Apache 2.0 | Modelar entidades y revisiones; no genera RLS SQL | Evaluar primera opción AuthZ Rust |
| **OpenFGA** | Relaciones usuario-recurso tipo Zanzibar | Excelentes permisos por jerarquías y equipos | Servicio/tuplas y consistencia, operacional adicional | Opcional al escalar relaciones |
| **OPA/Rego** | Motor general de políticas y bundles | Muy expresivo y ampliamente integrado | Otra semántica, posible runtime/sidecar, gobernanza | Adaptador opcional enterprise |
| **Propio** | Motor de reglas mínimo | Arranque simple, integración directa | Riesgo de errores, lenguaje propio, deuda de seguridad | **No** construir evaluador genérico sin justificación |

Repositorios: [Cedar Rust](https://github.com/cedar-policy/cedar), [OpenFGA](https://github.com/openfga/openfga), [OPA](https://www.openpolicyagent.org/docs/http-api-authorization).

**Recomendación técnica:** M11 define un *port* de decisión; Rust `cedar-policy` es candidato para el adaptador local. El compilador de `DataPolicy` a SQL tipado continúa siendo competencia compartida de M11 y del Semantic Planner: **Cedar no traduce por sí mismo filas SQL ni comprueba agregados**.

---

## 20. Gobernanza más allá de permisos

**Clasificación de datos:** etiquetas `public`, `internal`, `confidential`, `restricted`, más finalidad y retención definida por organización. Etiquetas heredables con revisión, no confiar exclusivamente en nombres de columnas.

**Propiedad:** `data_owner`, `technical_owner`, `steward`; la certificación de métricas tiene revisión y explicita quién aprobó su definición.

**Consentimiento/finalidad:** una persona autorizada para un indicador no recibe autorización universal para reutilizar datos fuera de su finalidad. Implementar por políticas de actividad cuando la organización lo necesite.

**Auditoría:** registrar `access_granted/denied`, `policy_changed`, `model_published`, `dashboard_shared`, `export_completed`, `secret_rotated`, `service_account_used` y operación sensible. No registrar valores de filas o consultas con PII en logs generales.

**Retención y borrado:** políticas y evidencia operativa distintas; RLS no sustituye retención legal, privacidad o eliminación de datos. El módulo se integra con Storage/Metadata sin asumir su motor físico.

---

## 21. Observabilidad y auditoría técnica

`AuditEvent`:

```json
{
  "event_type": "authorization.decision",
  "event_id": "evt-opaque",
  "timestamp": "2026-10-10T12:00:00Z",
  "actor_ref": "principal-opaque",
  "tenant_ref": "tenant-opaque",
  "action": "semantic.query",
  "resource_ref": "semantic-model:sales@revision-12",
  "decision": "ALLOW",
  "policy_revision": "policy-revision-7",
  "trace_id": "trace-opaque",
  "reason_code": "GRANTED_BY_SCOPE"
}
```

IDs ilustrativos. Nunca registrar JWT/credenciales, columnas sensibles, cuerpos de respuesta o filtros personales en claro cuando no son necesarios.

**Métricas operativas:** número de decisiones/denegaciones/errores, latencia AuthZ P50/P95/P99, fallos de carga de políticas, revocaciones, consultas canceladas y éxitos de exportación. No usar cardinalidad de actores como etiqueta de métricas de alta frecuencia.

**Explaining policies:** interfaz administrativa `explain_access(actor, action, resource, as_of_revision)` devuelve política ganadora, razones de denegación, herencia y datos necesarios redaccionados. El usuario final solo ve qué acción no está permitida, sin descubrir restricciones internas.

---

## 22. Casos prácticos de BI

### Caso A — Analista regional

Puede explorar energía facturada únicamente para municipios autorizados. Medidas aprobadas: `importe_facturado`, `kwh`, `clientes`; detalle de identidad de consumidor restringido. Un KPI total y una exportación deben producir la misma cifra bajo la misma revisión y filtro.

### Caso B — Gerencia

Puede acceder a consolidado total sin detalle individual. Dar acceso al total **no** significa permitir drilldown al cliente ni `SELECT *` de la tabla física. Configurar medidas agregadas/limitaciones de drill y umbral mínimo de grupos si hay riesgo de inferencia.

### Caso C — Cobranza vs facturación

No se permite que RLS en `Facturacion` quede olvidado al combinar pagos en `Cobranza`; ambas fuentes usan políticas compatibles. Si una relación semántica carece de propagación de seguridad bien definida, bloquear el plan de cruce o exigir dataset gobernado separado.

### Caso D — Agencia externa embebida

Token de lectura de dashboard expira; las credenciales de fuente quedan siempre en backend; el token no autoriza exportar ni listar modelos; la URL no contiene secreto persistente.

### Caso E — Agente BI

El agente puede consultar métricas publicadas en nombre del actor; no puede crear políticas, visualizar columnas privadas ni ejecutar código arbitrario. Cualquier cambio de grants necesita aprobación humana o rol autorizado distinto.

---

## 23. API de Rust: contratos propuestos (pseudocódigo de diseño)

```rust
pub struct ActorContext {
    pub principal_id: PrincipalId,
    pub tenant_id: TenantId,
    pub principal_kind: PrincipalKind,
    pub trusted_attributes: TrustedClaims,
    pub session_ref: SessionRef,
}

pub enum DecisionEffect { Allow, Deny, NotApplicable, Indeterminate }

pub struct AuthorizationDecision {
    pub effect: DecisionEffect,
    pub policy_revision: RevisionId,
    pub audit_reason: ReasonCode,
}

pub trait AuthorizationPort {
    fn decide(&self, request: &AuthzRequest) -> Result<AuthorizationDecision, AuthzError>;
}

pub struct SecurityConstraints {
    pub allowed_measures: Vec<SemanticMemberId>,
    pub allowed_dimensions: Vec<SemanticMemberId>,
    pub required_row_predicate: TypedPredicate,
    pub masking: Vec<MaskRule>,
    pub policy_revision: RevisionId,
}

pub struct AuthorizedSemanticRequest {
    pub actor: ActorContext,
    pub query: SemanticQuery,
    pub security: SecurityConstraints,
}
```

No es código compilable ni firma final; ilustra el contrato. Definir ownership/lifetimes/async/error types tras escoger runtime, no inventar dependencias prematuremente. Si se usa Cedar, el adaptador traduce principal/action/resource/context a entidades, y retorna decisión; el `DataPolicyCompiler` sigue separado.

---

## 24. Fronteras con módulos 01–10

| Módulo precedente | Lo que entrega a M11 | Lo que recibe de M11 |
|---|---|---|
| M01 Sources | SourceRef, credential_ref, schema | `source:discover`, autorización de extract, permiso secret:use |
| M02 Models | ModelRef, run plan, dependencias | permisos edit/run/publish, principal de ejecución |
| M03 Materialization | DatasetRef/snapshot | clasificación/políticas de serving, ACL, seguridad de rollback |
| M04 Semantic | medidas/dimensiones + SemanticPlan | filtros RLS inmutables y miembros visibles |
| M05 Query Serving | QueryRequest/cache executor | scope de caché, quotas y credenciales delimitadas |
| M06 Explore | ExploreDefinition/State | miembros/filtros permitidos, límites de metadata |
| M07 Pivot | MatrixRequest | mismos permisos para totales, filas, columnas y export |
| M08 Canvas | Dashboard/Panel/Share | autorización sobre documento y cada binding |
| M09 Visualization | VizSpec y eventos | sin acceso a secretos ni consultas SQL privilegiadas |
| M10 Metadata | recursos/revisiones/policies | decisiones por recurso, protección de drafts/exports |

**Invariantes de dependencia:** M11 no importa paquetes UI ni renderers; el motor semántico no consulta al proveedor de identidad directamente; el adaptador DuckDB no decide roles de usuario; M10 no revela recursos sin filtrar el permiso de enumeración.

---

## 25. Orden de implementación y entregables verificables

### P0 — Contratos y amenaza mínima

- `ActorContext`, `AuthzRequest`, `PolicyDecision`, `SecurityConstraints` tipados.
- Fixture dos tenants, dos roles, dos datasets con datos sensibles.
- Política `deny by default`, capability matrix y trazas de denegación.
- Tests negativos de spoofing de tenant y recurso ajeno.

**Salida:** integración unitaria con recurso de M10, sin motor SQL.

### P1 — AuthN y RBAC mínimo real

- OIDC o proveedor local adecuado al entorno.
- Roles de workspace/proyecto/dashboard, recursos y acciones separados.
- Gateway verifica sesión; servicios vuelven a verificar IDs y scopes.
- ServiceAccounts con tokens cortos y secreto externo al repositorio.

**Salida:** dos usuarios no pueden intercambiar acceso por URL.

### P2 — RLS/CLS semánticos reales

- Atributos verificados y `TypedPredicate`.
- Prohibir miembros no autorizados en SELECT/GROUP BY/ORDER/FILTER.
- SQL parametrizado y plan de seguridad inmutable.
- Casos numéricos con dos tenants, facturación/cobranza, ratios y totales.

**Salida:** misma política se cumple en Explore, Pivot y Canvas.

### P3 — Cierre de rutas auxiliares

- Search, Facets, Schema, Timestamps, Exports, API, agente y bookmarks.
- Cache scope + invalidación por política.
- Sandboxing para modelers / permisos read-only para viewers.

**Salida:** cero ruta sin `AuthorizedRequest` en tests de integración.

### P4 — Colaboración enterprise y análisis

- Políticas revisadas, explain/simulate, public share acotado, SCIM/SSO complejo según demanda.
- Cedar incorporado si aporta valor demostrado; OpenFGA para relaciones a gran escala si escala operativa lo justifica.
- Herramientas de certificación y auditoría/datos clasificatorios.

**Salida:** suite completa adversarial, reportes de compliance y revocación verificable.

---

## 26. Pruebas de aceptación mínimas (70 casos)

### A. AuthN e identidad

1. Solicitud anónima a API protegida denegada.
2. JWT expirado no autoriza.
3. JWT con audiencia diferente no autoriza.
4. JWT con firma inválida no autoriza.
5. Emisor OIDC no confiable no autoriza.
6. `tenant_id` enviado por query param no reemplaza tenant verificado.
7. Actor sin membresías no hereda un rol amplio.
8. Servicio con token expirado no ejecuta job.
9. Rotación de credenciales no modifica definiciones publicadas.
10. Atributos no verificables se tratan como ausentes.

### B. Recursos y roles

11. Viewer abre dashboard permitido.
12. Viewer no edita/publíca dashboard.
13. Publisher puede publicar donde tenga grant explícito.
14. Permiso de dashboard no concede consulta de dataset prohibido.
15. Conocer UUID de recurso ajeno no concede acceso.
16. Equipo hereda grants según política explícita.
17. Denegación explícita prevalece con precedencia declarada.
18. Permisos de proyectos distintos no se mezclan.
19. Renombrar recurso conserva identidad y grants.
20. Revisión vieja de recurso no omite política actual.
21. Error del Policy Engine devuelve deny y alarma.
22. Role/permission desconocido bloquea despliegue o se marca inválido.

### C. RLS/CLS y semántica

23. Usuario Norte no recibe filas Sur.
24. Dos medidas conservan el mismo RLS.
25. `COUNT DISTINCT` con RLS se calcula sobre universo autorizado.
26. Ratio mantiene numerador y denominador sujetos a RLS.
27. `REMOVEFILTERS` no quita RLS obligatorio.
28. Filtro por campo oculto se rechaza cuando permite inferencia.
29. `ORDER BY` de campo prohibido no expone ranking.
30. Medida no autorizada no aparece en catálogo ni puede invocarse por ID.
31. Dimensión restringida no permite drill.
32. JOIN entre dos hechos mantiene seguridad por ambos lados.
33. Política con claim ausente deniega.
34. Claim con tipo inesperado deniega.
35. Filtro tipado genera SQL con parámetros, no interpolación directa.
36. `NULL` se trata según semántica de la política.
37. Cambio de grupo del usuario recalcula autorización.
38. Materialización sin dimensión RLS no se selecciona para usuario restringido.
39. Totales Pivot coinciden con consulta autorizada a grano correcto.
40. Preagregado compartido no expone datos prohibidos de origen.

### D. Rutas auxiliares y visuales

41. Facetas no muestran valores exclusivos de otro tenant.
42. Autocompletado omite columna prohibida.
43. Rango temporal no revela actividad fuera del ámbito autorizado.
44. Search usa misma política que Query.
45. Export CSV reproduce filas autorizadas.
46. Export XLSX reproduce las mismas restricciones.
47. Export Parquet no permite consultar tablas base arbitrarias.
48. Canvas mezcla dos métricas, cada una con su política vigente.
49. Cambio ECharts→Highcharts no afecta autorización.
50. URL de dashboard compartido no permite ejecutar SQL libre.
51. Panel con error no imprime tokens ni rutas físicas.
52. Agente no recibe policy-admin por prompt injection.

### E. Ejecución, cache y sistema

53. Viewer no ejecuta `ATTACH` / `INSTALL` arbitrario mediante UI.
54. Modelo SQL corre bajo principal de autoría controlado, no viewer.
55. Cache tiene policy revision y ámbito tenant.
56. Revocar permiso impide nuevo cache hit autorizado por revisión vieja.
57. Dos actores con RLS diferentes no comparten resultado restringido.
58. Un request cancelado no conserva export completo accesible.
59. Timeout del IdP/Policy Store no concede permiso.
60. Intentar acceder a carpeta padre de rutas permitidas no funciona.
61. Importar YAML no ejecuta código en loader.
62. Plugin de gráfico no accede a credenciales de origen.

### F. Versiones, logs y gobernanza

63. Cambio de policy fallido conserva versión publicada anterior.
64. Publicar policy incompatible requiere confirmación/validación.
65. Auditoría registra quién aprobó y publicó.
66. Logs no incluyen tokens ni contraseñas.
67. Usuario sin permiso de auditor no ve decisiones ajenas.
68. Simulador administrativo explica una denegación sin revelar filas.
69. Export/import portable conserva recursos policy válidos y marca capacidades no convertidas.
70. Nueva API de consultas no pasa CI si omite verificación de políticas.

---

## 27. Riesgos y decisiones pendientes

| Riesgo | Mitigación/decisión |
|---|---|
| Reimplementar un lenguaje AuthZ completo | Evaluar Cedar; contrato propio, engine intercambiable |
| Confiar en filtros del frontend | Seguridad obligatoria en el backend y motor SQL |
| Legibilidad frente a seguridad en YAML | Políticas declarativas auditables, no SQL interpolado |
| Mezclar membresías con data RLS | Roles de recursos y filtros de datos independientes |
| Materializaciones cross-tenant | Evaluación de elegibilidad y seguridad sobre revisiones |
| SQL arbitrary en DuckDB | Entorno separado, privilegios/Filesystem lock y políticas de servidor |
| Política compleja OR/AND | Esquema con categorías y precedencia comprobada |
| Feature enterprise dependiente de terceros | Compatibilidad basada en OSS y adaptadores opcionales |
| API/Agente como bypass | Identidad delegada y mismas políticas semánticas |
| Sobreingeniería de control de acceso | Empezar RBAC+RLS+CLS, medir antes de ReBAC distribuido |

**Decisiones que deben quedar abiertas:** proveedor OIDC concreto; necesidad de multi-tenant compartido; licencias/ediciones de plugins de terceros; nivel de aislamiento SQL por despliegue; política concreta de consultas en marcha tras revocación; elección de Cedar/OpenFGA/OPA; formatos canónicos del M10.

---

## 28. Bibliografía de código fuente y documentación inspeccionada

### Rill (`v0.90.4`, inmutable)

1. [`runtime/metricsview/ast.go` — SecurityFilterSQL y LookupMeasure](https://github.com/rilldata/rill/blob/v0.90.4/runtime/metricsview/ast.go)
2. [`runtime/metricsview/executor/executor.go` — Query, Search, Export, Schema, Timestamps](https://github.com/rilldata/rill/blob/v0.90.4/runtime/metricsview/executor/executor.go)
3. [Metrics View reference — `security`](https://docs.rilldata.com/reference/project-files/metrics-views)

### Lightdash (`main`, fijar SHA)

4. [`authorization/index.ts` — abilities](https://github.com/lightdash/lightdash/blob/main/packages/common/src/authorization/index.ts)
5. [`scopeAbilityBuilder.ts` — scopes](https://github.com/lightdash/lightdash/blob/main/packages/common/src/authorization/scopeAbilityBuilder.ts)
6. [Permissions architecture notes](https://github.com/lightdash/lightdash/blob/main/.context/PERMISSIONS.md)
7. [Service layer and audited abilities](https://github.com/lightdash/lightdash/blob/main/packages/backend/src/services/CLAUDE.md)

### Cube (`master` / docs según versión)

8. [`server.ts` — gateway checkAuth/queryRewrite/context](https://github.com/cube-js/cube/blob/master/packages/cubejs-server-core/src/core/server.ts)
9. [Cube Core v1.2 — data access policies](https://cube.dev/blog/cube-core-v1-2-data-access-policies-hierarchies-and-folders-updates-to)
10. [Access Policies declarativas en Cube](https://cube.dev/blog/introducing-data-access-policies-in-cube-cloud)

### Grafana (`main`, fijar SHA)

11. [`accesscontrol.go`](https://github.com/grafana/grafana/blob/main/pkg/services/accesscontrol/accesscontrol.go)
12. [`api.go` — checks por API](https://github.com/grafana/grafana/blob/main/pkg/api/api.go)
13. [Grafana RBAC y límites por edición](https://grafana.com/docs/grafana/latest/administration/roles-and-permissions/access-control/)

### Apache Superset (`master`, fijar SHA)

14. [`superset/security/manager.py`](https://github.com/apache/superset/blob/master/superset/security/manager.py)
15. [`superset/row_level_security/api.py`](https://github.com/apache/superset/blob/master/superset/row_level_security/api.py)
16. [Políticas RLS y autenticación](https://github.com/apache/superset/blob/master/docs/admin_docs/security/security.mdx)

### Tecnologías y restricciones opcionales

17. [Cedar Rust y licencia Apache 2.0](https://github.com/cedar-policy/cedar)
18. [OpenFGA](https://github.com/openfga/openfga)
19. [OPA / Rego](https://www.openpolicyagent.org/docs/http-api-authorization)
20. [DuckDB Security](https://duckdb.org/docs/current/operations_manual/securing_duckdb/overview)

**Regla de reproducibilidad:** al implementar, fijar SHA para repositorios mutables y versión/crate para Cedar u otra dependencia. En esta revisión se han inspeccionado archivos públicos y documentación, **no** ejecutado su código ni realizado auditoría de seguridad ofensiva.

---

## 29. Criterio de cierre y enlace a la serie

El Módulo 11 estará conceptualmente listo cuando:

1. Todas las APIs y contratos conozcan a un principal validado y un recurso versionado.
2. La decisión de recurso y las restricciones de filas/campos sean separadas pero inseparables durante ejecución.
3. Los mismos permisos rijan Explore, Pivot, Canvas, exports, API y agentes.
4. RLS resista `REMOVEFILTERS`, joins, rollups y cachés, sin cambiar resultados semánticos correctos.
5. Las políticas puedan editarse/versionarse y auditarse sin formato propietario obligatorio.
6. Exista un primer recorrido de prueba real con dos tenants, roles y datos restringidos.
7. Ningún motor físico, renderer, repositorio de metadatos o proveedor de identidad esté incrustado en las interfaces centrales.

**Siguiente módulo sugerido, sin iniciarlo:** M12 — APIs, integraciones y ecosistema (REST/GraphQL/SQL API/MCP/SDK), o alternativamente M12 — Operations & Observability si se decide dar prioridad al despliegue.

**Fin del Módulo 11.** Mantenerlo independiente para futura compilación en ZIP junto con los informes 01–10; M01–M09 aún requieren revisión de referencias a SQLVIZ para alinearlos con la nueva plataforma agnóstica.
