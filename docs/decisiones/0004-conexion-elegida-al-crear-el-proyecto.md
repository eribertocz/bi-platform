# 0004 — La conexión de datos se elige al crear el proyecto; DuckLake local o remoto

**Estado:** aprobada por el propietario, 2026-10-11. **Sustituye:** el supuesto de un DuckLake único de la plataforma configurado por variables de entorno (G1) y la frase "por defecto nuestro DuckLake" de 0002. **Matiza:** `DECISIONES_TECNICAS_RC2.md` (catálogo DuckLake en PostgreSQL, sin SQLite). **Matizada por:** [0009](0009-modos-de-datos-y-persistencia-opcional.md) (modos de datos, persistencia opcional).

## Contexto
El propietario no quiere un motor por defecto implícito. Al crear un proyecto, el usuario elige con qué conectarse, como en Rill: DuckLake, DuckDB, ClickHouse u otros. DuckLake separa **catálogo** y **datos**, y cada uno puede estar en local o en remoto. Ejemplo: catálogo en Supabase (PostgreSQL gestionado) y datos en S3.

## Decisión
1. **Asistente al crear el proyecto:** se elige la conexión principal y se escribe de forma explícita en `connectors/<nombre>.yaml` y `project.yaml` (`connector: <nombre>`). No hay valor implícito. Cada modelo semántico puede usar otra conexión (0002).
2. **Conexiones del asistente:**

   | Conexión | Qué se configura | Uso |
   |---|---|---|
   | **DuckLake** | **Catálogo** + **datos**, cada uno local o remoto (tablas siguientes) | Lago propio: importar, transformar y publicar con versiones |
   | **DuckDB** (archivo `.duckdb`) | ruta del archivo, solo lectura | Analizar una base DuckDB existente |
   | **ClickHouse** | host, base de datos y usuario; la credencial como secreto | Análisis en vivo, cálculo en ClickHouse |
   | **PostgreSQL** | conexión de solo lectura | Análisis en vivo de una base operacional |

3. **Catálogo de DuckLake:**

   | Dónde | Ejemplos | Varios usuarios a la vez | Recomendado para |
   |---|---|---|---|
   | PostgreSQL remoto | Supabase, Neon, RDS, Cloud SQL, servidor propio | sí | servidor y equipos |
   | PostgreSQL local | contenedor o instalación local | sí | desarrollo y servidor propio |
   | Archivo local (catálogo DuckDB) | `./lake/catalog.ducklake` | **no: un solo proceso** | uso personal o de escritorio |

4. **Datos de DuckLake:** carpeta local, **S3** o compatible (MinIO, Cloudflare R2), **Google Cloud Storage** o **Azure Blob**. Con catálogo y datos locales, todo queda en la máquina del usuario. Con catálogo y datos remotos, la plataforma solo se conecta y no guarda copias.
5. **Credenciales** (contraseña del catálogo, claves de S3 y de ClickHouse): solo como referencia a un secreto (variable de entorno o gestor de secretos), nunca en el archivo del proyecto ni en logs. Ya está garantizado en G1 por `secret::scrub` y sus pruebas de propiedades.
6. **Un proyecto que solo usa ClickHouse** sirve para análisis en vivo. Para importar o transformar datos, necesita además una conexión DuckLake, porque las transformaciones se ejecutan en DuckDB y se publican en DuckLake (0001).

## Alternativas descartadas
- **DuckLake de la plataforma como valor implícito:** esconde dónde están los datos y obliga a un almacén que el cliente quizá no quiere.
- **Solo catálogo PostgreSQL:** impide el uso personal sin servidor. El catálogo en archivo se admite, con su limitación de un solo proceso explicada en el asistente.

## Consecuencias
- **G2/G3:** la configuración del lago pasa de variables de entorno del servidor a conexiones por proyecto. El arranque de G1 (`Bootstrap`, `ATTACH`, bloqueo de DuckDB) se generaliza por conexión.
- **Spike obligatorio antes de G3:** comprobar que el bloqueo de seguridad de DuckDB (`enable_external_access=false` + `allowed_directories`) admite rutas `s3://`, `gs://` y `az://`, con la extensión `httpfs` y los secretos cargados antes de bloquear. También medir la latencia con un catálogo remoto (Supabase) y datos en S3.
- **Desarrollo y la pila de un comando:** ofrecen una plantilla "DuckLake local" (PostgreSQL del compose + carpeta local), elegida de forma explícita, no implícita.
- La base PostgreSQL **de la plataforma** (usuarios, sesiones, ejecuciones) es independiente del catálogo del lago del proyecto.
