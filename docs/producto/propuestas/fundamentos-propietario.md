# Documentación Fundamental – Herramienta BI Open Source

*Documento aportado por el propietario el 2026-10-11. Se conserva tal cual. Su evaluación está en [`fundamentos-claude.md`](fundamentos-claude.md); la versión conjunta con las 10 decisiones aprobadas por el propietario, en [`version-comun.md`](version-comun.md). **No es el reglamento vigente**: las reglas de trabajo son `AGENTS.md`, en la raíz del repositorio.*

**Stack:** ELT propio (estilo SQLMesh) · Motor Semántico (Rust + Apache Arrow) · Frontend (Svelte 5 + ECharts)

Este documento contiene los **8 documentos base** que el propietario propuso como contexto de sistema / project rules para Claude Code, Codex u otros asistentes de código; lo que se adoptó de ellos está en los documentos enlazados arriba.

---

# Documento 1 – Leyes Fundamentales de una Herramienta BI

Estas son las reglas **innegociables**. Si se viola alguna, el sistema deja de ser una herramienta BI confiable.

## 1. Ley de la Verdad Única (Single Source of Truth)
Todo número que se muestre al usuario debe poder rastrearse hasta una definición semántica única, versionada y auditada.

- El motor semántico es la **única** fuente de métricas, dimensiones y relaciones.
- No se permiten cálculos ad-hoc en el frontend que no pasen por el motor semántico.
- Toda métrica tiene: nombre canónico, descripción de negocio, fórmula, owner y historial de cambios.
- Los usuarios no técnicos nunca escriben SQL crudo para métricas de negocio.

## 2. Ley de la Transparencia y Auditabilidad (Explainability)
Cualquier usuario con permisos suficientes debe poder responder: *“¿De dónde sale este número y por qué es este valor?”*

- Cada visualización debe poder mostrar: query generada, definición de la métrica, filtros aplicados (incluyendo seguridad) y timestamp de los datos.
- Debe existir lineage mínimo: Tabla origen → Modelo semántico → Métrica → Visualización.
- Los cambios en el modelo semántico deben ser versionables y reversibles.
- Debe registrarse quién consultó qué y cuándo.

## 3. Ley de la Seguridad por Diseño (Security by Design)
La seguridad no es un módulo opcional. Es una capa transversal que se aplica **antes** de que los datos lleguen al motor de consulta.

- Row-Level Security (RLS) y Column-Level Security (CLS) se aplican en el motor semántico o en la capa de acceso a datos, nunca solo en el frontend.
- Autenticación y autorización basadas en roles + permisos granulares.
- Principio de mínimo privilegio por defecto.
- Soporte nativo para SSO (OIDC/SAML).
- El modo avanzado de ECharts no puede bypassear los filtros de seguridad.

## 4. Ley de la Consistencia de Datos (Data Consistency)
El mismo número debe ser el mismo número en todos los lugares donde se muestre.

- El motor semántico garantiza que la misma métrica con los mismos filtros produce el mismo resultado.
- Debe existir un concepto claro de freshness visible para el usuario.
- Los modelos deben ser deterministas y reproducibles.
- Cache semántico que respete la definición de la métrica.

## 5. Ley de la Separación de Capas (Layer Separation)
Las responsabilidades deben estar claramente separadas.

| Capa              | Responsabilidad                          |
|-------------------|------------------------------------------|
| Ingesta / ELT     | Extraer, transformar, cargar             |
| Almacenamiento    | Persistencia (directo o importado)       |
| Motor Semántico   | Métricas, dimensiones, seguridad, lineage|
| API de Consulta   | Exponer el modelo semántico de forma segura |
| Frontend          | Visualización y experiencia de usuario   |

El frontend **nunca** debe conocer la estructura física de las tablas.

## 6. Ley de la Experiencia por Niveles de Usuario (Progressive Disclosure)
- **Modo Básico:** solo métricas, dimensiones y filtros predefinidos.
- **Modo Intermedio:** configuración avanzada de gráficos dentro de un subconjunto seguro.
- **Modo Avanzado:** acceso amplio a options de ECharts, pero siempre sobre datos ya filtrados por el motor semántico.

## 7. Ley del Rendimiento Predecible (Performance Predictability)
- El motor debe poder estimar costo/complejidad de una consulta.
- Query timeout y resource governance por usuario/rol.
- Cache inteligente a nivel de resultado semántico.
- Consultas asíncronas / materializaciones cuando el volumen lo requiera.

## 8. Ley de la Gobernanza y el Ciclo de Vida del Modelo
El modelo semántico es un artefacto de software.

- Versionado (Git-based).
- Entornos: desarrollo → staging → producción.
- Pull requests / revisiones de cambios en métricas críticas.
- Tests de métricas.
- Deprecación controlada.

## 9. Ley de la Observabilidad
- Métricas de uso, rendimiento y errores.
- Logs estructurados de queries.
- Capacidad de reproducir una consulta problemática.

## 10. Ley de la Extensibilidad Controlada
Extensible, pero nunca a costa de la integridad del modelo semántico.

---

# Documento 2 – Principios de Diseño + Anti-Patrones

## 1. Principios de Diseño (Strongly Recommended)

### 1.1 Progressive Disclosure
Mostrar la complejidad solo cuando el usuario la solicita.

### 1.2 Fail Fast & Fail Loud
Si algo está mal, el sistema debe fallar de forma clara y visible. Nunca devolver números incorrectos en silencio.

### 1.3 Semantic First, Visualization Second
Primero se resuelve el significado del dato. Después se decide cómo se visualiza.

### 1.4 Least Privilege by Default
Todo usuario comienza con el mínimo acceso posible.

### 1.5 Determinism
Misma definición + mismos filtros + mismos datos = mismo resultado.

### 1.6 Human-Readable Semantics
Las métricas deben ser entendibles por personas de negocio.

### 1.7 Separation of Concerns
Lógica de negocio ≠ lógica de presentación ≠ lógica de almacenamiento.

### 1.8 Immutability of Historical Data
Los datos históricos no se reescriben sin dejar rastro.

### 1.9 Observability by Default
Si no se puede medir, no se puede operar.

### 1.10 Extensibility without Corruption
Extender nunca debe romper las Leyes ni el modelo semántico.

## 2. Anti-Patrones (Prohibiciones de diseño)

- **SQL en el Frontend:** prohibido escribir SQL crudo para métricas de negocio desde el cliente.
- **Métricas calculadas solo en el gráfico:** genera inconsistencia entre dashboards.
- **Seguridad solo en el Frontend:** fácil de bypassear.
- **God Dashboard:** dashboards monolíticos sin estructura.
- **Magic Configuration:** métricas solo en UI sin versionado.
- **Silent Failures:** devolver 0 o null sin explicación.
- **Over-fetching:** traer todos los datos al cliente para filtrar.
- **Tight Coupling Frontend ↔ Tablas Físicas:** el frontend no debe conocer tablas.
- **Permisos Binarios:** solo Admin vs Viewer es insuficiente.
- **Ignorar Freshness:** nunca mostrar datos sin indicar cuándo se actualizaron.
- **Bypass del Motor Semántico en Modo Avanzado:** el modo avanzado no puede saltarse el motor ni la seguridad.
- **Cambios Destructivos sin Control:** no eliminar métricas en uso sin deprecación.

## Regla de Prioridad
1. Leyes Fundamentales  
2. Anti-Patrones (no introducirlos)  
3. Principios de Diseño  

---

# Documento 3 – Capacidades Core + Patrones de Arquitectura

## 1. Capacidades Core (Obligatorias)

- Modelo Semántico Versionado
- Resolución Semántica Única
- Seguridad de Datos (RLS + CLS)
- Lineage y Explicabilidad
- Roles y Permisos Granulares
- Freshness Visible
- Cache Semántico
- Audit Log
- Multi-Modo de Acceso a Datos (Direct Query + Importado)
- Exportación que respeta seguridad

## 2. Capacidades Avanzadas (Diferenciadoras)

- Alertas basadas en métricas
- What-if / escenarios
- Embedded analytics
- Time intelligence avanzado
- Collaborative editing
- Versionado de dashboards
- Metric tests / asserts
- Self-service modeling con gobernanza
- API pública estable (Headless BI real)

## 3. Patrones de Arquitectura

- **Semantic Layer Central** – todo pasa por el motor semántico
- **Metrics as Code** – métricas versionadas como código
- **Headless BI** – el motor se expone por API; el frontend es solo un cliente
- **Separación Clara de Capas**
- **Compiler + Runtime** – compilar petición semántica → plan → ejecutar
- **Pushdown Computation** – empujar cálculo al storage cuando sea posible
- **Dual Storage Strategy** – Direct Query + Importado
- **Progressive UI Modes** – Básico / Intermedio / Avanzado
- **Environment Promotion** – dev → staging → prod
- **Observability First**

## Criterios de Aceptación Rápidos
Una feature está bien implementada solo si:
- No viola ninguna Ley
- No introduce Anti-Patrones
- Pasa por el Motor Semántico
- Aplica seguridad correctamente
- Es observable
- Tiene comportamiento claro ante errores

---

# Documento 4 – Decisiones Técnicas Concretas por Capa

## 1. Capa de Ingesta / ELT
- Modelos como código (archivos), no configuración en UI.
- Entornos: dev → staging → prod.
- Versionado Git completo.
- Declarar dependencias, materialización y tests.
- Soporte de `plan` + `apply`.
- Incremental models desde el día 1.
- Separar staging y marts.
- **Prohibido:** que el frontend dispare transformaciones ad-hoc o que la lógica de métricas viva en el ELT.

## 2. Capa de Almacenamiento
- Soporte nativo de Direct Query e Importado.
- El modo se decide por fuente o por modelo.
- El Motor Semántico abstrae el modo al Frontend.
- Escrituras de datos importados auditables.
- Preferir formatos columnares (Parquet + Arrow).

## 3. Motor Semántico (Rust + Apache Arrow)
**Responsabilidades exclusivas:**
- Resolución de métricas y dimensiones
- RLS + CLS
- Generación y optimización del plan de ejecución
- Cache semántico
- Lineage y explicación
- Validación de permisos

**Decisiones de diseño:**
- Recibe peticiones semánticas, nunca SQL crudo del frontend.
- Entrada típica: `{ metrics, dimensions, filters, time_range, user_context }`
- Salida: Arrow + metadatos de explicación + freshness.
- Arrow como formato interno y de intercambio.
- Compiler + Runtime (plan lógico → optimización → ejecución).

**Seguridad y Cache:**
- RLS/CLS se aplican dentro del motor con `user_context`.
- Cache seguro por contexto de usuario/rol.
- Invalidación por cambio de métrica, datos o políticas.

## 4. API de Consulta
- Único punto de entrada al Motor Semántico.
- Tipada y versionada (`/v1/...`).
- Autentica y autoriza antes de llamar al motor.
- Devuelve datos + metadatos + freshness + explicación (según permisos).
- Soporte síncrono y asíncrono.
- Rate limiting y resource governance.
- **Prohibido:** endpoints que acepten SQL libre del usuario final.

## 5. Frontend (Svelte 5 + ECharts)
- Solo conoce el modelo semántico, nunca tablas físicas.
- Todo dato pasa por la API de consulta.
- Los 3 modos son de **configuración visual**, no de acceso a datos.

**Modo Básico:** métricas, dimensiones y filtros predefinidos. Sin opciones ECharts.  
**Modo Intermedio:** configuración avanzada dentro de subconjunto seguro.  
**Modo Avanzado:** acceso amplio a `option` de ECharts, pero los datos ya vienen resueltos y filtrados.

- Separar estado de datos, configuración visual y estado de UI.
- Nunca construir la query desde el `option` de ECharts.
- Guardar por separado: definición semántica + configuración visual.
- Mostrar siempre freshness, empty states explicativos y errores legibles.

## 6. Seguridad Transversal
- Autenticación preferente: OIDC.
- Autorización evaluada en Motor + API.
- Frontend solo oculta UI; nunca es la capa de seguridad real.
- Audit log de acceso y de cambios de modelo.

## 7. Observabilidad Transversal
- Latencia (p50/p95/p99), cache hit ratio, errores, uso por métrica/dashboard.
- Trazas reproducibles.
- Recomendación: OpenTelemetry desde el inicio.

## 8. Guardado de Artefactos
| Artefacto              | Cómo se guarda                     | Versionado |
|------------------------|------------------------------------|----------|
| Modelos ELT            | Código                             | Git      |
| Semantic Model         | Código (Metrics as Code)           | Git      |
| Dashboards             | Definición semántica + config visual | Sí     |
| Permisos / Políticas   | Código o config versionada         | Sí       |
| Cache                  | Efímero                            | No       |

---

# Documento 5 – Estructura de Repositorios + Contratos entre Capas

## 1. Estrategia de Repositorios (Recomendada)

**Monorepo** (recomendado para las primeras etapas):

```
bi-tool/
├── crates/                          # Rust
│   ├── semantic-engine/             # Motor semántico core
│   ├── arrow-runtime/               # Ejecución y optimización
│   ├── security/                    # RLS / CLS / políticas
│   └── api-server/                  # API de consulta
├── elt/                             # Proyecto estilo SQLMesh
│   ├── models/
│   ├── tests/
│   └── sematic/                     # (opcional) puentes
├── packages/
│   ├── semantic-client/             # Cliente TS tipado del motor
│   ├── ui/                          # Design system + componentes Svelte
│   └── echarts-safe/                # Wrapper seguro de ECharts
├── apps/
│   └── web/                         # Frontend Svelte 5
├── docs/
│   └── architecture/                # Estos 8 documentos
└── scripts/
```

Más adelante se puede separar en polirepo si el equipo crece.

## 2. Contratos entre Capas (Interfaces)

### 2.1 Contrato Frontend → API (Semantic Query)

```typescript
interface SemanticQuery {
  metrics: string[];                 // nombres canónicos
  dimensions?: string[];
  filters?: Filter[];
  timeRange?: TimeRange;
  orderBy?: OrderBy[];
  limit?: number;
  offset?: number;
}

interface SemanticQueryResult {
  data: ArrowTable | RecordBatch;    // preferir Arrow
  schema: Field[];
  meta: {
    freshness: string;               // ISO timestamp
    metrics: MetricMeta[];
    queryId: string;
    cacheHit: boolean;
    explanation?: Explanation;       // según permisos
  };
}
```

### 2.2 Contrato API → Motor Semántico (Rust)

```rust
pub struct SemanticRequest {
    pub metrics: Vec<MetricRef>,
    pub dimensions: Vec<DimensionRef>,
    pub filters: Vec<Filter>,
    pub time_range: Option<TimeRange>,
    pub user_context: UserContext,   // roles, atributos RLS
    pub options: QueryOptions,
}

pub struct SemanticResponse {
    pub batches: Vec<RecordBatch>,
    pub meta: ResponseMeta,
}
```

### 2.3 Contrato Motor → Storage

El motor genera un **Execution Plan** que puede ser:
- Pushdown SQL (Direct Query)
- Lectura de datasets Arrow/Parquet ya materializados
- Híbrido

El storage nunca ve el modelo semántico completo; solo recibe planes de ejecución concretos.

## 3. Versionado de Contratos
- API pública versionada (`/v1`, `/v2`).
- Cambios breaking en el contrato semántico requieren versión nueva.
- El cliente TypeScript se genera o se mantiene alineado con el schema del motor.

## 4. Reglas de Dependencias
- `apps/web` solo depende de `packages/semantic-client` y `packages/ui`.
- `semantic-client` solo habla con la API, nunca directamente con el motor.
- El motor no conoce Svelte ni ECharts.
- ELT no conoce el frontend.

---

# Documento 6 – Modelo de Seguridad (Roles, RLS, CLS, Auth)

## 1. Principios
- Seguridad se evalúa en el **Motor Semántico** y se refuerza en la API.
- El frontend solo controla visibilidad de UI, nunca acceso real a datos.
- Mínimo privilegio por defecto.
- Todo acceso debe ser auditable.

## 2. Autenticación
- Método prioritario: **OIDC** (SSO).
- Soporte adicional: API keys para service accounts / embeds (con scopes limitados).
- Sesiones con expiración y refresh rotativo.

## 3. Autorización – Modelo de Roles y Permisos

### Roles base (ejemplo)
- `viewer` – solo lectura de dashboards y métricas permitidas
- `analyst` – puede crear dashboards y usar modo intermedio/avanzado
- `modeler` – puede proponer cambios al semantic model (con review)
- `admin` – gestión de usuarios, permisos y entornos

Los roles se combinan con **permisos granulares**:

| Recurso        | Acciones posibles                     |
|----------------|---------------------------------------|
| Métrica        | view, use_in_dashboard                |
| Dashboard      | view, edit, share, delete             |
| Semantic Model | view, propose_change, approve, publish|
| Fuente de datos| connect, configure                    |
| Usuario / Rol  | manage                                |

## 4. Row-Level Security (RLS)
- Se define como políticas asociadas a entidades o métricas.
- Se evalúa con el `user_context` (atributos del usuario: organización, región, equipo, etc.).
- Ejemplo conceptual:

```yaml
policy:
  entity: orders
  filter: "orders.region IN ${user.regions}"
```

- El motor inyecta estos filtros de forma obligatoria. El usuario no puede eliminarlos.

## 5. Column-Level Security (CLS)
- Columnas marcadas como `sensitive` o `restricted`.
- Según el rol se pueden: mostrar, enmascarar o eliminar del resultado.
- Nunca se envían al frontend si el usuario no tiene permiso.

## 6. Seguridad en Modo Avanzado de ECharts
- El `option` de ECharts solo afecta la visualización.
- No puede alterar la Semantic Query ni los filtros de seguridad.
- Se debe sanitizar cualquier expresión evaluable si se permite.

## 7. Audit Log Obligatorio
Registrar al menos:
- Login / logout
- Ejecución de consultas (user, metrics, dashboard, timestamp, duración, cache)
- Cambios al semantic model
- Cambios de permisos
- Intentos de acceso denegados

## 8. Embeds y Service Accounts
- Tokens de corta duración o API keys con scopes muy limitados.
- RLS/CLS se siguen aplicando con el contexto del token.

---

# Documento 7 – Diseño del Motor Semántico (DSL + Arrow)

## 1. Objetivo del Motor
Ser la **única fuente de verdad** para:
- Definición de métricas y dimensiones
- Resolución de consultas semánticas
- Aplicación de seguridad
- Generación de planes de ejecución
- Cache semántico y explicación

## 2. Metrics as Code – Estructura recomendada

```yaml
# metrics/revenue.yml
metric:
  name: revenue
  label: "Ingresos"
  description: "Suma de amount de órdenes completadas"
  type: sum
  sql: "orders.amount"
  filters:
    - "orders.status = 'completed'"
  dimensions:
    - date
    - region
    - product_category
  owner: finance-team
  tags: [core, finance]
  status: active
```

Alternativa: DSL propio o macros en Rust más potentes. Lo importante es que sea **código versionado**.

## 3. Flujo de una Consulta

1. API recibe `SemanticQuery` + `user_context`
2. Motor valida permisos y existencia de métricas
3. Aplica RLS / CLS
4. Compila a **Plan Lógico**
5. Optimiza (projection pruning, filter pushdown, constant folding…)
6. Decide ejecución:
   - Pushdown SQL (Direct Query)
   - Lectura Arrow/Parquet materializado
   - Híbrido
7. Ejecuta y materializa resultado en **Arrow RecordBatch**
8. Aplica post-agregaciones si son necesarias
9. Consulta/actualiza cache semántico
10. Devuelve datos + meta (freshness, explanation, cache hit)

## 4. Arrow como formato central
- Todo el camino caliente trabaja con Arrow.
- Evitar JSON en el hot path.
- Usar Arrow IPC o Arrow Flight para comunicación de alto rendimiento si es necesario.
- El frontend puede recibir Arrow y convertirlo, o recibir un formato tabular ya serializado según el caso.

## 5. Time Intelligence
El motor debe soportar de forma nativa (o vía macros):
- `time_range` relativo y absoluto
- Comparaciones (previous period, YoY, MoM)
- Grain (day, week, month, quarter, year)
- YTD / MTD / QTD

## 6. Testing del Motor
- Tests unitarios de compilación de métricas
- Tests de políticas RLS/CLS
- Tests de consistencia (misma métrica + mismos filtros = mismo resultado)
- Golden files de planes de ejecución
- Tests de rendimiento básicos (latencia y memoria)

## 7. Extensibilidad
- Nuevas funciones de agregación y time intelligence vía plugins internos.
- Conectores de storage detrás de una interfaz común (`DataSource` trait).
- Nunca permitir que un plugin salte el sistema de seguridad.

---

# Documento 8 – UX y los 3 Modos de Visualización

## 1. Principio rector
La complejidad se revela progresivamente.  
El usuario no técnico nunca debe sentirse abrumado.  
El usuario avanzado nunca debe sentirse limitado (dentro de los límites de seguridad y semántica).

## 2. Modo Básico (Usuarios no técnicos)

**Objetivo:** responder preguntas de negocio en el menor número de clics posible.

Características:
- Selector de métricas (con búsqueda y descripciones).
- Selector de dimensiones.
- Filtros predefinidos y time range claro.
- Tipos de gráfico sugeridos automáticamente según la combinación métrica + dimensión.
- Sin acceso a opciones de ECharts.
- Freshness siempre visible.
- Empty states con explicación y sugerencias.
- Un solo clic para exportar (CSV / imagen).

## 3. Modo Intermedio (Analistas)

**Objetivo:** dar control real sobre la visualización sin exponer la complejidad total de ECharts.

Características:
- Todo lo del modo Básico +
- Elección explícita de tipo de gráfico.
- Configuración de ejes, series, apilado, leyenda, tooltip, colores, orden, límites, formato de números.
- Dual axis cuando aplique.
- Referencias / líneas de meta.
- Guardado de la configuración visual de forma estructurada (no el `option` crudo completo).

Restricción: solo se exponen opciones validadas y seguras.

## 4. Modo Avanzado (Power Users)

**Objetivo:** control casi total de ECharts.

Características:
- Editor del objeto `option` (JSON o UI asistida).
- Posibilidad de usar features avanzadas de ECharts (graphic, visualMap, dataZoom complejo, etc.).
- **Restricción crítica:** el `option` solo define la representación visual.  
  Los datos ya vienen del motor semántico y ya tienen aplicados RLS/CLS.
- Se guarda por separado:
  1. Semantic Query
  2. ECharts option (sanitizado)

## 5. Reglas UX transversales a los 3 modos

- Freshness siempre visible.
- Loading states claros (y skeleton cuando sea posible).
- Errores legibles y accionables (“No tienes permiso sobre la métrica X”, “Datos desactualizados desde…”, “Timeout”).
- Empty states útiles (nunca solo “No data”).
- Los filtros deben ser obvios, reversibles y visibles.
- Drill-down predecible.
- Exportación de primera clase.
- Responsive real (consumo en móvil usable).
- Modo claro / oscuro.

## 6. Guardado de un gráfico / widget

Todo widget debe persistir:

```json
{
  "id": "...",
  "semanticQuery": { ... },
  "visualMode": "basic" | "intermediate" | "advanced",
  "visualConfig": { ... },          // estructurado o option
  "title": "...",
  "description": "..."
}
```

Nunca persistir solo el `option` de ECharts sin la Semantic Query.

## 7. Instrucción para implementación
Al crear cualquier componente de visualización:
1. Separar datos (vienen de la API) de configuración visual.
2. Respetar el modo actual del usuario.
3. Nunca permitir que el modo avanzado modifique la Semantic Query.
4. Sanitizar cualquier `option` guardado.
5. Mostrar siempre freshness y manejar errores de forma explícita.

---

# Cierre – Cómo usar estos 8 documentos

**Orden de prioridad cuando haya conflicto:**

1. Documento 1 – Leyes Fundamentales  
2. Documento 2 – Anti-Patrones  
3. Documento 6 – Seguridad  
4. Documento 4 y 7 – Decisiones del Motor y capas  
5. Documento 3 – Capacidades y patrones  
6. Documento 5 – Estructura y contratos  
7. Documento 8 – UX  

**Instrucción recomendada para el asistente de código:**

> Antes de implementar cualquier feature, verifica que no viola las Leyes (Doc 1), no introduce Anti-Patrones (Doc 2) y respeta el modelo de seguridad (Doc 6) y las decisiones técnicas por capa (Doc 4). El frontend nunca calcula métricas ni aplica seguridad real. Todo dato pasa por el Motor Semántico.

---

*Fin de la documentación fundamental (8 documentos).*
