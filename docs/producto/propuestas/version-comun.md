# Versión común: ELT, Estudio, bibliotecas y fundamentos

*Borrador de Claude, 2026-10-12, que une las propuestas independientes de Claude (archivos `*-claude.md`) y de Codex (comentarios del PR #85) tras la discusión en el PR. **Codex aún debe confirmarlo.** Después se presenta al propietario, que aprueba o rechaza cada decisión del final. Nada de esto está aprobado.*

## Cómo leer este documento
- Cada apartado dice **en qué coincidimos**, **qué corregimos al discutir** y **qué queda para después**.
- Al final hay **nueve decisiones** para el propietario, cada una con nuestra recomendación.
- Usamos tres estados distintos: **decidido** (hay una ADR aprobada), **en una rama** (código en un PR abierto) e **integrado** (en `main`, probado). Una decisión no es una función terminada.

---

## 1. ELT y lienzo: una definición, tres decisiones

### La idea
Una persona empieza con un archivo y una pregunta. Con el tiempo la pregunta se vuelve lenta, otro equipo quiere datos más recientes y finanzas necesita defender el cierre del mes. Hoy eso acaba en tres proyectos, copias y discusiones sobre qué cifra vale.

Proponemos que **cada modelo se escriba una sola vez**, con un nombre lógico que no cambia, y que sobre él la persona tome tres decisiones reversibles:
- **consultar** en vivo, el valor inicial (ADR 0009);
- **acelerar**, con caché o materializado en DuckLake;
- **conservar**, para poder reproducir un resultado, por ejemplo un cierre.

El dashboard siempre consulta el mismo modelo semántico, se guarde como se guarde. Nunca se copia nada por sorpresa, y las capas físicas (RAW, SILVER, GOLD) son opcionales.

### En qué coincidimos
- Los modelos son archivos `models/*.sql` con su cabecera `MODEL (...)`, en Git (ADR 0003 y 0005).
- **Código y lienzo son dos vistas de lo mismo.** Editar uno actualiza el otro. Las posiciones de los nodos se guardan aparte.
- **Plan antes de publicar**, con vista previa, problemas junto al editor y pruebas que deciden la publicación.
- La frescura se ve en cada nodo y en cada panel.
- La elección manual **siempre** está disponible: se puede fijar cualquier modelo en vivo, en caché o materializado.
- Los horarios, reintentos y retención viven en `pipelines/`, separados del cálculo: nunca un `cron` dentro de `MODEL` (ADR 0009).

### Qué corregimos al discutir
Codex encontró siete exageraciones en las primeras propuestas. Las aceptamos todas:
1. **Una muestra no demuestra que dos cálculos sean iguales.** Ejemplo: un `COUNT DISTINCT` puede coincidir hoy y fallar mañana. Nada se aplica "porque no cambió ninguna cifra visible". El plan enseña además los cambios de estructura, de contrato y de seguridad.
2. **El incremental no sale gratis de la matemática del motor.** Hay que garantizar también que las filas son nuevas y no repetidas, y que no cambiaron uniones ni políticas. Los borrados necesitan otro trato. La primera versión incremental declara su clave, sus intervalos y que se puede repetir sin efecto doble. La automatización llega después, como sugerencia demostrada.
3. **"Frescura de 15 minutos" no significa "recalcular cada 15".** También cuentan el retraso de la fuente, las dependencias y la duración del cálculo. Si dos pedidos chocan, o chocan con el presupuesto, se explica el conflicto y se respetan los límites.
4. **Volver atrás y viajar en el tiempo tienen límites.** Una versión de DuckLake puede expirar, una fuente en vivo no tiene historia y una rama de Git no fija los datos. Por eso existe "conservar este cierre", que fija las versiones necesarias. Volver a una versión anterior mantiene los permisos actuales: nunca devuelve accesos revocados.
5. **Dibujar una flecha no escribe "la unión correcta".** Conectar dos nodos propone una referencia y enseña el cambio. Se confirma al compilar y nunca se inventan una unión, una clave o un grano.
6. **Rill ya dibuja el grafo** de fuente a dashboard con Svelte Flow. Nuestra novedad no es el dibujo.
7. **Las cifras de negocio no se recalculan en el navegador.** Romperían la verdad única y los totales no aditivos.

### Lo diferencial: "¿por qué cambiaría esta cifra?"
Une la propuesta de Codex (ensayo contrafactual) con la de Claude (intención de servicio).
- La persona **arrastra una cifra concreta** de un dashboard sobre una ruta alternativa, por ejemplo "servir este panel desde lo materializado".
- El sistema responde por separado si cambiarían:
  - el **valor**;
  - la **frescura**;
  - los **permisos**;
  - la **capacidad de reproducirlo**.
- Si algo falla, señala **el primer nodo responsable** y, cuando puede, un contraejemplo mínimo.
- La persona puede fijar **compromisos** ("este cierre no cambia", "este panel admite 5 minutos de retraso"). Ninguna optimización posterior puede romperlos.
- La **intención** ("como mucho 15 minutos", "que abra en menos de 1 segundo", "costo bajo") hace que el sistema **proponga** rutas. Cada propuesta es visible y anulable, y nunca se aplica sin plan.

No conocemos otra herramienta que una resultado, seguridad, tiempo y almacenamiento en una sola interacción. Es una apuesta por validar, no una afirmación de que nadie lo haya hecho.

### Qué tomamos de los referentes (leído en su código)
| Referente | Tomamos | Descartamos |
|---|---|---|
| SQLMesh v0.236.3 | Plan con intervalos por el grafo (`plan/builder.py`); huella del modelo y de sus padres (`snapshot/definition.py`); la cautela de `categorizer.py`, que pregunta cuando no sabe | El runtime de Python, Jinja y el `cron` dentro del modelo |
| Rill v0.90.5 | Reconciliación con recarga en caliente (`reconcilers/model.go`); vista o tabla y estrategias incrementales (`drivers/duckdb/crud.go`); navegación del grafo (`resource-graph`) | Que un refresco automático publique en producción sin plan ni pruebas |
| Power BI, Tableau, Looker, Cube | Modo de almacenamiento por tabla; lógica separada de lo físico; reutilizar agregados solo cuando son válidos | El modo elegido a mano por un experto, sin prueba |
| Snowflake, Databricks, Dagster | Frescura declarada que se propaga; comprobaciones por activo | Atarlo a un motor o a Python |
| Fivetran, Airbyte | Puntos de control e historia de cambios y borrados | — |

### Por fases
| Fase | Qué entrega |
|---|---|
| **G3, primera entrega** | Estudio con archivos, lienzo y lista; `MODEL` explícito; modos vista y tabla completa elegidos a mano; vista previa con los permisos del usuario; plan con diferencias; auditorías; publicación versionada de DuckDB a DuckLake |
| **Después, cada una con su Issue** | Intención de frescura, velocidad y costo como propuesta visible; caché segura (cuando el punto único esté integrado, ADR 0010); incremental para los casos demostrados; ensayo "¿por qué cambiaría esta cifra?" sobre una instantánea |

---

## 2. Estudio: "el proyecto vivo"

### La idea
Hoy el código, los datos, el linaje, los errores y las versiones viven en sitios que no se hablan, y preguntas simples ("¿dónde se usa esta métrica?", "¿qué rompo si cambio este modelo?") cuestan minutos.

Proponemos **un único espacio de trabajo donde cada archivo está vivo**: sabe en qué estado está, qué datos produce, de dónde vienen y a quién afecta.

### Las piezas
- **Explorador con dos vistas enlazadas:** **Archivos**, al estilo VS Code y Rill, y **Flujo**, el lienzo del ELT. Seleccionar en una selecciona en la otra.
- **Estado en cada elemento**, sin abrirlo: válido, con errores, ejecutándose, desactualizado o con cambios sin guardar.
- **Un editor para cada tipo**, y cualquier archivo se puede abrir siempre como texto:
  - modelo SQL con vista previa;
  - métrica Ossie con formulario y YAML sincronizados;
  - dashboard;
  - conexión, que nunca muestra secretos.
- **Panel compartido** con Código, Datos, Problemas y Cambios. Seleccionar una columna sigue sus dependencias.
- **Paleta de comandos** (Ctrl+K), en lenguaje humano.
- **Git sin saber Git:**
  - tres botones: Guardar versión, Probar en una rama y Publicar;
  - las diferencias enseñan también qué cifras cambian.
- **Accesible por diseño:** lista y teclado equivalentes al arrastre; zoom y subgrafos por objetivo para no ver una pared de cientos de nodos.

Rill ya ofrece archivos y grafo. Lo que añadimos es el estado vivo, las cifras en cada cambio y la seguridad en la vista previa.

### Por fases
1. **Con la pantalla Modelos (G2):** explorador de archivos con estado, panel de Problemas y apertura como texto.
2. **G3:** vista Flujo, editor de modelos con vista previa, "Probar en una rama" y "Guardar versión".
3. **G4:** editores de dashboard y de métrica en vivo, y la paleta completa.

---

## 3. Bibliotecas de la interfaz
El frontend actual se mantiene: SvelteKit 3, Svelte 5, Tailwind 4, Vite 8, TypeScript 6 y ECharts 6. Nada de React.

| Función | Elección | Versión y licencia | Notas |
|---|---|---|---|
| Lienzo con nodos y conexiones | **Svelte Flow** (`@xyflow/svelte`) | 1.7.0, MIT | Teclado y ARIA; conectar también por formulario y lista |
| Orden automático del grafo | **Dagre** (`@dagrejs/dagre`) | 3.1.1, MIT | Fuera del hilo principal si el grafo crece |
| Editor de código | **CodeMirror 6**, por módulos (`view`, `state`, `lang-sql`, `autocomplete`, `lint`, `merge`) | 6.x, MIT | Las sugerencias vienen del catálogo autorizado. La validación real la hace Rust |
| Tablas de vista previa | **TanStack Table** | 9.2.8, MIT | Tabla HTML semántica; paginación, filtro y orden en el servidor; decimales exactos |
| Filas virtuales, solo si hace falta | **TanStack Virtual** (`@tanstack/virtual-core`) | 3.19.0, MIT | Con paginación accesible como alternativa |
| Paneles, menús y diálogos | **Bits UI** | 2.19.5, MIT | Da foco, teclado y ARIA, pero la accesibilidad se verifica en cada pantalla |
| Arrastre externo, opcional | **Pragmatic drag and drop** | 4.0.0, Apache-2.0 | Con comandos equivalentes de teclado |
| Paneles divididos | **svelte-splitpanes** | 8.0.12, MIT | — |
| Iconos | **Lucide** (`@lucide/svelte`) | 1.55.0, ISC | — |
| Lienzo de dashboards (G4) | **GridStack** | 14.0.0, MIT | Ya está en la hoja de ruta |
| Estilos | **Tailwind**, el que ya tenemos | 4.3.3, MIT | Sin una segunda biblioteca de CSS |

**Aplazadas** hasta tener un caso medido: Perspective, DuckDB-WASM y Mosaic. Si llegan, será solo sobre datos ya autorizados y para presentación. **Descartadas:** ELK.js (licencia EPL o GPL), Monaco (pesado) y un segundo motor de arrastre o de edición.

**Puerta de adopción.** Antes de adoptarlas, una prueba con nuestras versiones exactas debe:
- compilar con renderizado en servidor;
- cargar el editor y el lienzo bajo demanda;
- medir el tamaño de cada pantalla con un grafo de 200 nodos;
- pasar axe, teclado y lector de pantalla (NVDA y Narrador).

Los pesos de npm no son el tamaño que descarga el navegador: se mide en esa prueba.

---

## 4. Fundamentos del propietario (8 documentos)

### Veredicto común
Los ocho documentos son un marco muy bueno y casi todo encaja con las ADR vigentes. Recomendamos adoptarlos **como objetivos de producto**, con las ADR como autoridad técnica y el estado de cada punto a la vista.

### Precisiones en las que coincidimos
- **Autor y consumo son públicos distintos.** El estudio de autor necesita SQL, tablas y lienzo (ADR 0005). Los dashboards, la exploración, la API y la IA solo conocen el modelo semántico. No hace falta prohibir el SQL en todo el frontend.
- **Ossie sigue siendo el formato de métricas** (ADR 0006), con `owner`, `tags`, estado y sensibilidad en la extensión `bi-platform` (en curso). No usamos un YAML propio.
- **Se mantiene la estructura actual** del repositorio. Los árboles de carpetas de los documentos son ejemplos.
- **Datos hacia el navegador:** JSON exacto para pantallas y Arrow para volúmenes grandes. Los decimales siempre son exactos.
- **Seguridad:**
  - políticas con parámetros tipados, nunca plantillas de texto como `${user.regions}`;
  - roles editables `viewer`, `analyst`, `modeler` y `admin`;
  - columnas enmascaradas con las reglas de la ADR 0012;
  - el modo avanzado de gráficos admite solo JSON, sin funciones ni HTML.
- **Caché:** se consulta **después** de autorizar y de comprobar la frescura, y antes de ejecutar. Su clave incluye la huella de los permisos, así que una revocación la invalida.
- **"Mismo resultado"** exige la misma versión de datos, contexto y políticas, no solo los mismos filtros.
- **Una fuente en vivo** se puede explicar, pero no siempre reproducir.
- **Orden de los gates:** G2 seguridad y exactitud; G3 datos; G4 consumo; G5 automatización. Caché, exportación e IA llegan cuando el punto único esté integrado.
- **Los totales los calcula el motor**, también en los gráficos. Un NULL legítimo o la falta de filas no son errores, y nunca se inventa un cero.
- **Tres niveles de gráfico** sobre el mismo `VisualSpec`. Al bajar de nivel se conserva todo y se señala lo que el nivel básico no edita. `VisualSpec` no es el objeto de ECharts: ECharts es un renderizador reemplazable.

### Orden de prioridad para resolver conflictos
leyes → anti-patrones → seguridad → **ADR vigentes** → principios → resto de documentos.
- La accesibilidad y la exactitud cuentan como leyes.
- Si un documento choca con una ADR, lo decide el propietario y queda en una ADR nueva.

---

## 5. Decisiones para el propietario
Cada una se aprueba o se rechaza por separado.

1. **Dirección del ELT.** Una definición; consultar, acelerar y conservar; el sistema propone y el usuario siempre puede elegir a mano; nada se aplica sin plan. *Recomendado: sí.*
2. **Primera entrega de G3.** Estudio con archivos, lienzo y lista; modos vista y tabla completa elegidos a mano; vista previa autorizada; plan con diferencias; auditorías; publicación versionada en DuckLake. Intención, caché, incremental y ensayo de cifras, después. *Recomendado: sí.*
3. **Reproducción con plazo declarado.** Cada resultado dice si es reproducible y hasta cuándo. "Conservar este cierre" fija sus versiones. Lo consultado en vivo se explica, pero no se promete reproducirlo. *Recomendado: sí.*
4. **Límites de la automatización y de la seguridad.** Nada se aplica por una muestra. Las columnas enmascaradas salen vacías con su tipo y un aviso, y no se pueden usar para filtrar, ordenar ni agrupar. *Recomendado: sí.*
5. **Estudio "el proyecto vivo"**, con dos vistas enlazadas, estado en todo y Git sin saber Git. *Recomendado: sí.*
6. **Bibliotecas de la tabla del apartado 3**, adoptadas tras la prueba medida. *Recomendado: sí.*
7. **Solo licencias permisivas en el frontend** (MIT, Apache-2.0, BSD o ISC), vigiladas por CI como ya se hace en Rust. *Recomendado: sí.*
8. **Cómo se escribe la intención:** en palabras simples ("como mucho 15 minutos") que se guardan como valores con unidades. *Recomendado: sí.*
9. **Fundamentos:** adoptar los ocho documentos como objetivos de producto, con las precisiones del apartado 4 y su orden de prioridad. *Recomendado: sí.*
