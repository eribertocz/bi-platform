# 0009 — Modos de datos: consultar en vivo, guardar resultados o pipeline; SOURCE, RAW y GOLD son lógicos

**Estado:** aprobada por el propietario, 2026-10-11 (propuesta suya en la conversación de trabajo). **Matiza:** 0002 (importar a DuckLake deja de ser la vía para todo lo que se transforma), 0003 (`sources/` y nueva carpeta `pipelines/`), 0004 §6 (un proyecto solo con ClickHouse puede materializar en ClickHouse) y 0005 (`cron` sale de la cabecera `MODEL`; las capas staging → intermediate → mart son opcionales).

## Contexto
No todos los usuarios necesitan ETL/ELT para crear dashboards. Hay quien tiene un `ventas.parquet` y quiere graficarlo ya, quien tiene los modelos listos en su base de datos y quien necesita cargas periódicas con históricos y auditoría. Obligar a `Parquet original → RAW → SILVER → GOLD` en DuckLake multiplica copias de la misma información (con 30 GB, varias veces 30 GB) sin aportar nada. DuckDB ya consulta Parquet directamente con lectura selectiva de columnas y filtros.

## Decisión
1. **Cuatro conceptos lógicos, no ubicaciones físicas.** Rutas, tablas y motores se resuelven por configuración (`connectors/`).

   | Concepto | Qué representa | ¿Obligatorio? |
   |---|---|---|
   | **SOURCE** | Origen: CSV, Parquet, PostgreSQL, `.duckdb`, DuckLake, ClickHouse… | Sí |
   | **RAW** | Copia persistente de los datos originales o extraídos | No |
   | **SILVER** | Resultados intermedios | No: solo si son reutilizables o aportan valor |
   | **GOLD** | Datos transformados y listos para analizar | No siempre |

2. **Tres modos, con la misma experiencia visual.** El usuario puede empezar en vivo y convertirlo después en pipeline **sin rehacer sus dashboards**.

   | Modo | Qué hace | Ejemplo |
   |---|---|---|
   | **Consultar directamente, sin copiar** (en vivo) | El modelo semántico apunta al origen; no se crea RAW ni GOLD. También sirve para quien ya tiene sus modelos listos en su base de datos | `ventas.parquet` → DuckDB en memoria (un `models/ventas.sql` de tipo `VIEW` con `read_parquet`) → dashboard |
   | **Guardar resultados transformados** | Transformaciones SQL cuyo resultado se guarda en el destino elegido | CSV → SQL → GOLD en DuckLake |
   | **Crear pipeline de actualización automática** | Extrae, conserva los originales cuando corresponde, transforma, audita y publica de forma recurrente | PostgreSQL → RAW → incremental → GOLD en DuckLake |

3. **RAW y GOLD se ubican por separado.** GOLD va donde convenga ejecutar las consultas analíticas; RAW, donde convenga conservar y recuperar los originales.

   | Situación | RAW recomendado | GOLD recomendado |
   |---|---|---|
   | CSV o Parquet local | el archivo original, sin duplicarlo | DuckLake, si hace falta materializar |
   | PostgreSQL remoto | sin RAW, o instantánea Parquet opcional | DuckLake |
   | DuckLake en la nube | DuckLake o archivos en S3 | DuckLake en S3 |
   | ClickHouse local | sin RAW, o RAW en ClickHouse | ClickHouse |
   | ClickHouse Cloud | S3 opcional o tablas RAW en ClickHouse | ClickHouse Cloud |
   | Solo consulta en vivo | no hace falta | no hace falta |

4. **La persistencia es opcional por etapa.** Cada modelo es virtual (`VIEW`) o materializado (`FULL`, `INCREMENTAL_*`) según su uso y lo que admita el motor (0005 §3).
5. **La ejecución depende del destino, sin pasos de más.** Cada modelo sigue eligiendo quién calcula (`engine`) y dónde se guarda (`output`); se mantienen todas las combinaciones de 0005 §2, incluidas DuckDB → ClickHouse y, más adelante, ClickHouse → DuckLake mediante Parquet intermedio registrado por DuckDB. Lo que se evita son los pasos innecesarios:
   - un proyecto solo con ClickHouse calcula y guarda en ClickHouse, sin pasar por DuckDB ni por un DuckLake intermedio;
   - ClickHouse también lee S3 directamente.
6. **Los dashboards no dependen de rutas físicas.** Consumen datasets y modelos semánticos estables (`metrics/`, 0002). Cambiar de modo o de destino exige volver a validar la ejecución, pero no rediseñar los gráficos.
7. **Lo operativo, separado de la lógica.** Los archivos de transformación (`models/*.sql`) solo describen qué se calcula:
   - **credenciales:** en `connectors/`, como referencia a secretos (0004);
   - **programación, retención y reintentos:** en `pipelines/*.yaml`, nueva carpeta del proyecto (0003). `cron` sale de la cabecera `MODEL` de 0005; al importar un proyecto SQLMesh, su `cron` pasa al pipeline;
   - **registro de ejecuciones:** en la base PostgreSQL de la plataforma (0003), nunca en los archivos del proyecto.

   `sources/` declara qué se extrae a RAW, pero no cuándo.
8. **Nunca registrar originales con `ducklake_add_data_files`.** La función no copia los Parquet: DuckLake pasa a ser su dueño, y `merge_adjacent_files` o `expire_snapshots` + `cleanup_old_files` pueden borrarlos ([documentación de DuckLake, "Adding Files"](https://ducklake.select/docs/stable/duckdb/metadata/adding_files)). Solo se usa con archivos que la plataforma generó para el lago (la zona intermedia de 0005 §2). Un original que deba conservarse se lee en su sitio o se copia.

## Interfaz
Al añadir datos no se muestran RAW, SILVER, GOLD, instantánea ni política de materialización. Se pregunta "¿Cómo quieres trabajar con estos datos?" con tres opciones:
- "Consultar directamente, sin copiar" (**opción por defecto**: no se copian datos sin necesidad).
- "Guardar resultados transformados".
- "Crear pipeline de actualización automática".

Cada opción tiene una vista previa en lenguaje llano; por ejemplo, para la primera: "El dashboard leerá el archivo original. No se creará ninguna tabla RAW ni GOLD." El detalle técnico queda en "Examinar diseño técnico".

## Alternativas descartadas
- **Medallón obligatorio (RAW → SILVER → GOLD físicos):** copias innecesarias, más almacenamiento y más latencia, sin ganancia para quien solo quiere analizar.
- **Importar siempre a DuckLake:** obliga a un almacén que el usuario quizá no quiere; ClickHouse no necesita un DuckLake intermedio.
- **`cron` dentro del modelo (SQLMesh, Rill):** mezcla la operación con la lógica; cambiar un horario parecería un cambio del modelo en el plan.

## Consecuencias
- **G2 (motor semántico):** sin cambios. SEF no sabe de dónde vienen los datos (0008); un dataset de Ossie apunta a un nombre lógico que resuelve su conexión.
- **G3 (ELT):** la primera entrega de 0005 cubre los modos 1 y 2 (`VIEW` y `FULL` con DuckDB → DuckLake). El modo 3 añade `sources/`, `pipelines/` e `INCREMENTAL_BY_TIME_RANGE`. Los incrementales desde bases operacionales deben tratar duplicados, modificaciones y borrados, no solo filas nuevas: se aborda en su propio Issue, con casos negativos.
- **Pruebas:** el paso de modo 1 a modo 2 sobre el mismo dataset debe devolver resultados idénticos en el mismo dashboard. Es la prueba de que los dashboards no dependen de rutas físicas.
- **Referentes:** SQLMesh admite DuckDB y ClickHouse, pero el SQL no es portable entre motores. Se mantiene 0005 §6: sin traducción automática; cada modelo se escribe en el dialecto de su motor.
