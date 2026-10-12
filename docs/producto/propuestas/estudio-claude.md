# Propuesta del Estudio: "el proyecto vivo"

*Propuesta de Claude, 2026-10-11. En discusión con Codex en el PR; el propietario la aprueba o la rechaza al final.*

## 1. El problema humano
En las herramientas de BI, el trabajo está repartido en sitios que no se hablan:
- el código vive en un editor;
- los datos, en otra pestaña;
- el linaje, en una página de documentación;
- los errores, en un log;
- las versiones, en Git, que la mayoría no sabe usar.

Preguntas simples cuestan minutos: "¿dónde se usa esta métrica?", "¿por qué este dashboard marca otra cifra que ayer?", "¿qué rompo si cambio este modelo?".

## 2. La idea central
**Un único espacio de trabajo donde cada archivo del proyecto está vivo**: sabe en qué estado está, qué datos produce, de dónde vienen y a quién afecta. Las mismas definiciones (archivos en Git, ADR 0003) se ven de dos formas enlazadas: como **archivos**, al estilo VS Code y Rill, y como **flujo**, el lienzo del ELT.

```
┌──────────────┬──────────────────────────────────────┬──────────────────────┐
│ EXPLORADOR   │ EDITOR (pestañas)                    │ CONTEXTO             │
│ ◉ Archivos   │ models/marts/ventas_diarias.sql  ● ✓ │ Vista previa (datos) │
│ ○ Flujo      │ MODEL (name marts.ventas_diarias…)   │ Perfil de columnas   │
│ ▸ connectors │ SELECT fecha, region, sum(importe)…  │ Linaje ↑ / Impacto ↓ │
│ ▸ sources  ✓ │                                      │ Usos: 3 métricas,    │
│ ▾ models   ⚠ │                                      │ 2 dashboards         │
│   ▸ staging  │                                      │ Frescura: 12 min ✓   │
│   ▸ marts  ● │                                      │ Modo: incremental    │
│ ▸ metrics  ✓ ├──────────────────────────────────────┴──────────────────────┤
│ ▸ dashboards │ PROBLEMAS (2) · PLAN · HISTORIAL · TERMINAL SQL              │
│ ▸ alerts     │ ⚠ metrics/ventas.yaml:14  ambiguous_reference "region"…      │
│ ▸ apis       │                                                              │
│ ◆ DuckLake   │ Rama: probar-descuentos · 3 cambios sin guardar · [Guardar]  │
└──────────────┴──────────────────────────────────────────────────────────────┘
```

## 3. Qué tomamos de los referentes y qué hacemos mejor

| Referente | Tomamos | Lo llevamos más allá |
|---|---|---|
| **VS Code** | Árbol, pestañas, paneles divididos, panel de Problemas, paleta de comandos, atajos | Cada nodo del árbol conoce su significado (métrica, modelo, dashboard), no solo su nombre de archivo |
| **Rill** | Carpetas por tipo de recurso, recarga en caliente, vista previa junto al código, error anclado a la línea | Dos vistas enlazadas (archivos y flujo), estado visible y versiones de datos |
| **dbt Cloud IDE y Explorer** | Linaje y documentación generada | Linaje hasta el dashboard y por columna, dentro del mismo espacio |
| **Hex y Count** | Datos visibles junto al cálculo | Además, el motor semántico demuestra que el número es correcto |
| **GitHub Desktop** | Git con botones | Git **sin saber Git**: ramas = entornos de datos sin copia y diferencias con impacto en números |

## 4. Las piezas

### 4.1 Explorador con dos vistas
- **Archivos.** Las carpetas del proyecto, más un nodo especial **DuckLake** con tablas, tamaños y versiones navegables:

  | Carpeta | Contiene |
  |---|---|
  | `connectors/` | Conexiones; los secretos solo como referencias |
  | `sources/` | Fuentes |
  | `models/` | `staging/`, `intermediate/`, `marts/` |
  | `metrics/` | Ossie |
  | `dashboards/` | Dashboards |
  | `alerts/` | Alertas |
  | `apis/` | APIs |
  | `tests/` | Pruebas |
  | `project.yaml` | Configuración del proyecto |

  El laboratorio Python, futuro (`pyproject.toml`, `notebooks/`), aparece cuando se active, sin entrar en el núcleo (ADR 0001).
- **Flujo.** El grafo de principio a fin (propuesta ELT §3.5). Seleccionar en una vista selecciona en la otra.
- **Estado en cada elemento, sin abrirlo:**
  - ✓ válido;
  - ⚠ con errores;
  - ⟳ ejecutándose;
  - ◷ desactualizado respecto a la frescura pedida;
  - ● cambios sin guardar.
- **Acciones de contexto según el tipo:**
  - en un modelo: "vista previa", "materializar", "ver linaje", "¿qué rompe si lo cambio?";
  - en una métrica: "explorar", "¿dónde se usa?".

### 4.2 Un editor a la medida de cada tipo
| Tipo | Editor |
|---|---|
| Modelo SQL | Código con autocompletado de tablas, columnas y modelos, y vista previa de datos al lado |
| Métrica (Ossie) | Formulario y YAML sincronizados; exploración en vivo con el motor semántico |
| Dashboard | Lienzo de paneles; el YAML se escribe solo |
| Conexión | Formulario con "probar conexión"; los secretos nunca se muestran |
| Cualquier archivo | Siempre se puede abrir como texto: nada queda escondido tras un formulario |

### 4.3 Panel de Problemas
Reúne todos los diagnósticos del proyecto (Ossie, CSMIR, motor de corrección, auditorías, ELT) con su código, archivo y línea. Un clic lleva al punto exacto. Ya existe la base: los errores llegan estructurados con ruta JSON.

### 4.4 Paleta de comandos (Ctrl+K)
Abre cualquier cosa por su nombre y ejecuta acciones en lenguaje humano:
- "materializar ventas_diarias";
- "¿dónde se usa margen?";
- "ver el dashboard Comercial como estaba el martes".

### 4.5 Git sin saber Git
- Tres botones: **Guardar versión** (commit), **Probar en una rama** (una rama de Git más un entorno de datos sin copia en DuckLake) y **Publicar** (PR y merge, o publicación directa si el proyecto es personal).
- **Las diferencias enseñan también los números**: "este cambio altera *Ventas* de octubre en −0,52 %" (propuesta ELT §4.2).
- **Historial** como línea de tiempo, con "volver a esta versión" en un clic.

### 4.6 Contexto a la derecha
Para lo que esté seleccionado:
- vista previa de datos;
- perfil de columnas;
- linaje hacia arriba e impacto hacia abajo;
- usos, frescura, modo de servicio y costo.

## 5. Bibliotecas de UI de primer nivel
Comprobadas en el registro de npm el 2026-10-11. Todas encajan con SvelteKit 3, Svelte 5, Tailwind 4, Vite 8 y TypeScript 6. **Regla propuesta:** solo licencias permisivas (MIT, Apache-2.0, BSD o ISC), verificadas en CI como ya se hace en Rust con `cargo-deny`. El peso se mide por pantalla al integrar cada una, como exige AGENTS.md.

| Módulo | Elegida | Versión y licencia | Por qué | Alternativa descartada |
|---|---|---|---|---|
| Lienzo del flujo | **Svelte Flow** (`@xyflow/svelte`) | 1.7.0, MIT; publicada el 2026-10-11, con la 2.0 en `next` | El estándar de grafos con nodos interactivos, nativo de Svelte 5, accesible por teclado | Dibujar el grafo a mano: meses de trabajo |
| Diseño automático del grafo | **dagre** (`@dagrejs/dagre`) | 3.1.1, MIT | Grafo por capas izquierda→derecha, ideal para un DAG | **ELK.js** 0.12 (más potente, pero EPL-2.0 o GPL-3.0) |
| Editor de código | **CodeMirror 6** (`codemirror`, `@codemirror/lang-sql`, `@codemirror/merge`) | 6.x, MIT | Ligero, modular y accesible; SQL y diferencias lado a lado | **Monaco** 0.57 (el de VS Code): varios MB y peor accesibilidad |
| Arrastrar y soltar | **Pragmatic drag and drop** (`@atlaskit/pragmatic-drag-and-drop`) | 4.0.0, Apache-2.0 | Independiente del framework, pequeño, con guía de accesibilidad; el de Jira y Trello | Bibliotecas solo para React (dnd-kit) |
| Tablas grandes | **TanStack Table + TanStack Virtual** | 9.2.8 y 3.13.41, MIT | Sin estilo propio y con millones de filas virtualizadas | Rejillas cerradas o de pago (AG Grid Enterprise) |
| Tabla dinámica (pivot) | **Perspective** (`@finos/perspective`) | 3.8.0, Apache-2.0 | WebAssembly sobre Arrow, rapidísimo; de la FINOS | S2 (la de la hoja de ruta), a comparar midiendo |
| Gráficos | **ECharts 6** (ya está) | 6.1.0, Apache-2.0 | Completo, Canvas o SVG, temas | — |
| Exploración enlazada masiva | **Mosaic** (`@uwdata/vgplot`) | 0.32.1, BSD-3; publicada el 2026-10-09 | Vistas enlazadas sobre millones de filas que se filtran en la base de datos | Hacer el *crossfilter* a mano |
| Datos en el navegador | **Apache Arrow JS** y **DuckDB-WASM** | 21.2.0, Apache-2.0; 1.33.1-dev, MIT | Arrow ya es nuestro formato (ADR 0008); DuckDB-WASM filtra al instante sin volver al servidor | — |
| Componentes accesibles | **Bits UI** (base de shadcn-svelte) | 2.19.5, MIT | Sin estilo propio, WCAG resuelto; diálogos, menús, árbol y *command* | Hacerlos a mano |
| Paneles divididos | **svelte-splitpanes** | 8.0.12, MIT | Activa | **PaneForge** 1.0.2 (sin cambios desde 2025-08) |
| Iconos | **Lucide** (`@lucide/svelte`) | 1.55.0, ISC | Coherentes y ligeros | — |
| Lienzo de dashboards | **GridStack** | 14.0.0, MIT | Arrastrar y cambiar el tamaño de los paneles; ya está en la hoja de ruta (G4) | — |

**Riesgos de bibliotecas:**
- **DuckDB-WASM** publica versiones `-dev` como estables: se fija una versión exacta y se prueba con nuestro oráculo de exactitud.
- **Svelte Flow 2.0** está en camino: se empieza con la 1.x estable.

## 6. Por fases
1. **Con la pantalla Modelos (G2):** explorador de archivos con estado, panel de Problemas y apertura como texto.
2. **G3 (ELT):** vista Flujo, editor de modelos con vista previa, "Probar en una rama" y "Guardar versión".
3. **G4 (BI):** editores de dashboard y métrica en vivo, paleta de comandos completa, Mosaic y Perspective.
4. **Más adelante:** diferencias con impacto en números y máquina del tiempo en todo el estudio.

## 7. Decisiones para el propietario
1. ¿Se adopta "el proyecto vivo", con dos vistas enlazadas, estado en todo y Git sin saber Git?
2. ¿Se adopta la regla de solo licencias permisivas en el frontend?
3. ¿Hay alguna carpeta o tipo de archivo más que quieras ver en el explorador?
