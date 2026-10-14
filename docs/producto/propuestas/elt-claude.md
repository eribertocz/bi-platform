# Propuesta de ELT: "di qué necesitas, no cómo hacerlo"

*Propuesta de Claude, 2026-10-11. **Cerrada:** la versión conjunta con Codex, con las 10 decisiones aprobadas por el propietario el 2026-10-12, está en [`version-comun.md`](version-comun.md). Este documento queda como la propuesta original, para seguir la discusión.*

## 1. El problema humano
Hoy, quien prepara datos tiene que decidir **cómo** se guardan antes de saber **qué** necesita:
- vista o tabla;
- completa o incremental;
- cada cuánto se recalcula (escribir un `cron`);
- qué macros de fechas usar (`@start_ds`, `@end_ds`).

Equivocarse sale caro:
- en vivo, el dashboard tarda 9 segundos;
- materializado, los datos llegan con un día de retraso;
- cambiar de uno a otro obliga a rehacer modelos y dashboards, y nadie garantiza que el número siga siendo el mismo.

Lo que la persona sabe de verdad es otra cosa:
- "este dashboard lo mira el director a las 8:00 y tiene que estar al día";
- "este informe puede tener un día de retraso";
- "no quiero pagar más de X al mes al proveedor de la nube".

## 2. La idea central
**Cada modelo se escribe una sola vez y su almacenamiento es una perilla, no una decisión de arquitectura. La persona declara su intención (frescura, velocidad, costo) donde le importa, el sistema elige cómo cumplirla y demuestra que el número no cambia.**

Tres piezas lo hacen posible, y ningún producto las tiene juntas:
1. **El motor semántico demuestra la equivalencia.** Sabe si un cálculo se puede sumar por partes, si hay que recalcularlo o si no se puede reagregar. Por eso decide con seguridad qué se puede guardar y actualizar de forma incremental, y qué no. El caso 18 del corpus ya exige que materializar no cambie ningún resultado.
2. **DuckLake versiona cada tabla.** Cada publicación es una instantánea:
   - volver atrás es inmediato;
   - una rama de desarrollo no copia datos;
   - el registro de cambios entre versiones dice exactamente qué filas son nuevas.
3. **Las definiciones son archivos** (ADR 0003): todo queda en Git, revisable y reproducible.

## 3. Cómo lo vive el usuario

### 3.1 Escribir el modelo, sin decidir nada más
```sql
MODEL (name marts.ventas_diarias, grain [fecha, region]);
SELECT fecha, region, sum(importe) AS ventas FROM staging.ventas GROUP BY ALL;
```
No declara `kind`, ni `cron`, ni macros. Si quiere control, lo fija él (§3.4).

### 3.2 Declarar la intención donde le importa
En el dashboard, la métrica o el modelo, con palabras humanas:
```yaml
frescura: 15 minutos      # como mucho, datos de hace 15 minutos
velocidad: 1 segundo      # que abra en menos de 1 segundo
costo: bajo               # bajo | normal | sin límite
```
La intención **se propaga hacia atrás por el linaje**, como el `TARGET_LAG = DOWNSTREAM` de Snowflake: si el dashboard pide 15 minutos, los modelos de los que depende heredan ese requisito, sin escribir un solo `cron`.

### 3.3 El sistema elige, explica y demuestra
Para cada modelo elige uno de cinco **modos de servicio** y lo explica en una frase:

| Modo | Qué hace | Cuándo lo elige |
|---|---|---|
| **En vivo** | Consulta el origen en cada petición | El origen es rápido o la frescura exigida es "al instante" |
| **Caché** | Guarda el resultado con fecha de caducidad | Se repiten las mismas consultas y la frescura lo permite |
| **Materializado** | Tabla en DuckLake, recalculada entera | Cálculo caro con poca frecuencia de cambio |
| **Incremental automático** | Tabla en DuckLake que solo procesa lo nuevo | El motor **demuestra** que el cálculo se puede actualizar por partes |
| **Híbrido** | Historia materializada más el tramo reciente en vivo | Mucha historia y frescura casi en tiempo real (tablas híbridas de Power BI, *lambda* de Cube) |

> "Materializo `ventas_diarias` de forma incremental cada 15 minutos porque el dashboard *Comercial* lo pide. En vivo tardaría 9,2 s; así, 0,3 s. Costo estimado: 4 minutos de cálculo al día."

**Cambio verificado:** cuando el sistema pasa de un modo a otro, calcula en paralelo con el modo anterior y compara los resultados exactos. Solo cambia si coinciden; si no, avisa y no cambia.

### 3.4 Libertad total cuando la persona quiere decidir
Cualquier modelo se puede fijar a mano: `modo: en_vivo`, `materializado` o `incremental`, con `cron` incluido. El sistema respeta la elección y solo **sugiere**, por ejemplo: "fijado en vivo, pero el dashboard tarda 9 s; materializarlo lo dejaría en 0,3 s".

Un proyecto pequeño puede quedarse todo en vivo, sin pipeline (ADR 0009); uno grande puede llevarlo todo en pipeline. Es la misma herramienta y el mismo modelo.

### 3.5 El flujo se ve y se construye en un lienzo (idea del propietario, ampliada)
Un **grafo de principio a fin**, de las fuentes a los modelos de *staging*, intermedios y *marts* en el lakehouse, y de ahí a métricas y dashboards. Cada nodo es interactivo:
- **Lienzo y código son la misma cosa.**
  - Al arrastrar un nodo o una conexión se escribe el `models/*.sql`, con su cabecera `MODEL (...)`.
  - Al editar el SQL, el lienzo se actualiza.
  - Los archivos en Git siguen siendo la única fuente de verdad. La posición de los nodos no ensucia los modelos: se calcula sola o se guarda aparte.
- **Nodos vivos.** Al abrir un nodo se ve:
  - su SQL, con el resto de modelos como referencias navegables;
  - una muestra de filas y el perfil de cada columna (nulos, distintos, mínimo y máximo);
  - número de filas, última actualización, modo de servicio (§3.3) y costo;
  - el resultado de sus pruebas de calidad.
- **Superposiciones de un clic:**
  - **linaje de una columna** desde la fuente hasta el dashboard que la muestra;
  - **plan:** qué nodos cambian y cuánto cambian los números (§4.2);
  - **frescura:** qué nodos van retrasados respecto a lo pedido;
  - **máquina del tiempo:** una barra para ver el flujo y sus datos en cualquier versión de DuckLake.
- **Construir arrastrando, con ayuda del motor semántico:**
  - soltar una tabla de origen crea su modelo de *staging* con tipos y nombres limpios;
  - conectar dos nodos propone la unión correcta, porque el modelo semántico conoce relaciones y cardinalidades, y avisa si la unión multiplicaría filas;
  - soltar un nodo de métrica conecta el modelo con la capa semántica (Ossie).
- **Referentes que superamos:**
  - el grafo de dbt y el de la UI de SQLMesh, que solo enseñan;
  - Dagster, cuyo grafo de *assets* no tiene datos dentro;
  - KNIME, Alteryx y Keboola, que permiten arrastrar pero guardan en formatos cerrados y sin código revisable.

  Aquí se arrastra **y** queda SQL limpio en Git.

## 4. Lo que va más allá de todos los referentes

### 4.1 Incremental automático demostrado
SQLMesh y dbt obligan a escribir la lógica incremental (rango de fechas, clave única), y si se escribe mal, el número sale mal en silencio. Nosotros:
1. leemos del registro de cambios de DuckLake qué filas son nuevas desde la última versión;
2. el motor semántico clasifica cada agregado, y con eso decide:
   - `SUM`, `COUNT`, `MIN` y `MAX` sobre datos que solo crecen se actualizan por partes;
   - `AVG` se actualiza a partir de su suma y su conteo;
   - `COUNT DISTINCT` exacto se recalcula entero;
3. si no puede demostrarlo (por ejemplo, si hubo borrados o actualizaciones), **lo dice** y recalcula entero; nunca adivina.

### 4.2 Plan con impacto en números reales
SQLMesh clasifica un cambio como "rompe" o "no rompe" mirando solo el modelo: en su código, solo añadir columnas cuenta como seguro, y el resto lo pregunta. Nosotros conocemos el linaje hasta las métricas y los dashboards, así que el plan dice:

> "Este cambio altera 2 métricas en 3 dashboards. *Ventas por región* de octubre pasa de 1.204.330,00 a 1.198.120,50 (−0,52 %). *Margen* no cambia."

Para eso calcula los dashboards afectados con las dos versiones, aprovechando las ramas sin copia de DuckLake. Un cambio que no altera ningún número publicado se aplica sin preguntar.

### 4.3 Máquina del tiempo y ramas gratis
- Cada publicación es una instantánea de DuckLake: "ver el dashboard como estaba el martes" y "volver a la versión anterior" son inmediatos.
- Una rama de desarrollo apunta a las mismas versiones y solo calcula lo que cambió. Es la misma idea que los entornos virtuales de SQLMesh, pero sobre las versiones nativas de DuckLake y sin vistas intermedias.

### 4.4 El sistema aprende del uso
Observa las consultas reales: "agregar `ventas` por `mes × región` ahorraría el 92 % del tiempo de 4 dashboards". Lo propone y se acepta con un clic. Así hacen las agregaciones automáticas de Power BI y los *rollups* de Rill, solo que aquí lo valida el motor semántico.

### 4.5 Calidad dentro de la publicación
Las pruebas de datos y las auditorías se ejecutan **dentro de la transacción** que publica la nueva versión. Si una falla, no se publica nada y el dashboard sigue mostrando la última versión buena, con un aviso.

## 5. Qué tomamos y qué descartamos, mirando el código

| Referente | Tomamos | Descartamos y por qué |
|---|---|---|
| **SQLMesh** v0.236.3 | Cabecera `MODEL (...)` (ADR 0005); plan antes de aplicar; huella por modelo con sus padres (`snapshot/definition.py`); intervalos y señales de "datos listos" (`signal.py`); entornos sin copia | La categorización local (`categorizer.py`: solo añadir proyecciones es seguro), sustituida por el impacto en números reales (§4.2). Las macros de fechas obligatorias, sustituidas por el incremental automático (§4.1). El runtime de Python |
| **Rill** v0.90.5 | Recursos reconciliados con recarga en caliente; `output` separado de quien calcula; particiones; pruebas en el modelo | El interruptor manual `materialize: true/false` con un valor por defecto del proyecto (`parse_model.go`, `model_manager.go`), que aquí es una opción más dentro de la perilla con intención (§3.4) |
| **Snowflake**, *dynamic tables* | Frescura objetivo que se propaga hacia los consumidores (`TARGET_LAG`, `DOWNSTREAM`) | Que solo exista para tablas de Snowflake: aquí vale para dashboards, métricas y modelos |
| **Power BI** | Modo de almacenamiento por tabla (*Import*, *DirectQuery*, *Dual*); tablas híbridas; agregaciones automáticas | Que el modo lo elija a mano un experto, sin prueba de equivalencia |
| **Cube** | Preagregaciones *lambda* (historia más tramo en vivo) | Su YAML propio y su runtime de Node |
| **Dagster** | Automatización declarativa basada en frescura | Orquestar *assets* de Python |
| **Materialize, Feldera** | El ideal de vistas siempre al día por mantenimiento incremental | Un motor de *streaming* aparte: usamos DuckDB y DuckLake (ADR 0001) |

## 6. Cómo llegar sin sobreingeniería (por fases)

| Fase | Qué entrega | Valor para el usuario |
|---|---|---|
| **G3a** (mínimo de la ADR 0005) | Modelos con cabecera, DAG, plan, modos **En vivo** y **Materializado** fijados a mano, auditorías en la transacción, instantáneas de DuckLake | ELT funcionando, con vuelta atrás inmediata |
| **G3b** | Intención de **frescura** con propagación hacia atrás; **Caché**; explicación de cada elección; cambio verificado | Ya no se escriben `cron` y nada cambia de número en silencio |
| **G3c** | **Incremental automático** demostrado (DuckLake más la clasificación del motor) y plan con impacto en números | Lo diferencial: rápido, al día y demostrado |
| **G4+** | **Híbrido**, sugerencias por uso, presupuesto de costo | Optimización continua sin expertos |

## 7. Riesgos
- **Elegir mal el modo.** Se mitiga con la explicación visible, la anulación manual y el cambio verificado: el error sería de velocidad o de costo, nunca de número.
- **Costo del cambio verificado** (calcular dos veces): se limita a una ventana de muestra y a los dashboards afectados.
- **Orígenes que no indican qué cambió** (por ejemplo, una tabla CSV que se reescribe): incremental automático solo cuando DuckLake o el origen lo demuestran; si no, completo.
- **El registro de cambios de DuckLake aún tiene errores con borrados tras compactar** (duckdb/ducklake #1572, #1603 y #1573, abiertos en octubre de 2026). Por eso:
  - la primera versión solo usa las **inserciones** en tablas que solo crecen;
  - ante borrados o actualizaciones, recalcula entero;
  - cada cierto tiempo hace una verificación completa que compara el incremental con un recálculo desde cero.
- **Complejidad:** por eso las fases. G3a es el mínimo ya aprobado; cada fase siguiente se aprueba por separado.

## 8. Decisiones para el propietario
1. ¿Se adopta como dirección del ELT "di qué necesitas, no cómo hacerlo": intención, modos elegidos por el sistema y equivalencia demostrada?
2. ¿Se mantiene G3a como primera entrega, sin las fases de intención, para tener ELT funcionando antes?
3. ¿Cómo se expresa la intención: palabras simples (`frescura: 15 minutos`), números con unidades o ambas?
