# bi-platform

Plataforma BI nueva: monolito modular Rust (Axum, DuckDB, PostgreSQL) y frontend SvelteKit. `bi-platform` es un nombre provisional de ingeniería.

- Arquitectura de referencia: [LEER_PRIMERO.md](LEER_PRIMERO.md) y [docs/arquitectura/](docs/arquitectura/INDICE_17_MODULOS.md).
- Plan por gates: [ROADMAP_GLOBAL.md](docs/arquitectura/gobierno/ROADMAP_GLOBAL.md).
- Contrato HTTP vigente: [docs/api/openapi.json](docs/api/openapi.json).
- Reglas de colaboración: [AGENTS.md](AGENTS.md).

Estado: en desarrollo activo (gate G2, semántica y seguridad). El estado real, por módulo, vive en GitHub (Issues y Pull Requests) para no quedar desactualizado aquí; ver también [ROADMAP_GLOBAL.md](docs/arquitectura/gobierno/ROADMAP_GLOBAL.md) para la secuencia de gates.

## Inicio local con un comando

Con Podman activo y un proveedor Compose instalado (o Docker con Compose):

```sh
podman compose up --build    # o: docker compose up --build
```

Abre `http://127.0.0.1:8080/estado`. El build multi-stage compila Rust y Svelte;
Axum sirve la API y los archivos estáticos en el mismo origen. No requiere Rust
ni Node en el host. PostgreSQL persiste en el volumen `pgdata` y se expone en
`127.0.0.1:54320`; los puertos 8080 y 54320 deben estar libres. Para parar:
`podman compose down` (conserva los datos). El primer build descarga y compila
las dependencias.

## Desarrollo

```sh
podman compose up -d postgres # o: docker compose up -d postgres
cd backend
# Desarrollo sintético local: migra al arrancar y avisa que el rol `bi` es privilegiado.
BI_METADATA_MODE=development BI_DEV_USERS='[{"id":"synthetic-analyst","display_name":"Analista sintético"}]' \
  cargo run -p bi-server --features dev-auth
```

Sin `BI_METADATA_MODE=development`, el servidor arranca en el modo restringido
por defecto: no migra y no admite operaciones con un rol privilegiado como el
`bi` de desarrollo (ver «Metadatos: migraciones y rol de la aplicación»).

En otra terminal: `cd frontend`, `npm ci` y `npm run dev`. Vite sirve
`http://127.0.0.1:5173` y envía `/health` y `/api` a Rust en el puerto 8080.
Si la pila completa estaba levantada, detén antes `app` para liberar ese puerto.

Pruebas: en `backend/`, `cargo fmt --all --check`,
`cargo clippy --locked --all-targets --all-features -- -D warnings` y
`cargo test --locked`; con PostgreSQL real, configura
`DATABASE_URL=postgres://bi:bi@127.0.0.1:54320/bi` y ejecuta
`cargo test --locked --all-features` (PowerShell: `$env:DATABASE_URL='postgres://bi:bi@127.0.0.1:54320/bi'`).
Usa `127.0.0.1`, no `localhost`: en Windows la resolución de `localhost` por
conexión vuelve muy lentas estas pruebas. Ese usuario debe poder crear bases y
roles: las pruebas del modo restringido crean y borran roles temporales propios.
Estas pruebas también requieren el proveedor real descrito en «Login corporativo».
Variables: `BI_BIND_ADDR`, `BI_DATABASE_URL` (por defecto la base local de
`compose.yaml`), `BI_LOG_FORMAT=text|json`, `RUST_LOG` y `BI_STATIC_DIR`
(directorio del build; sin esta variable, Rust sirve solo la API).

`BI_PROJECT_DIR` selecciona la carpeta del proyecto (ADR 0003). En **Modelos**,
tras iniciar sesión, «Actualizar modelos» recarga `metrics/*.yaml`, `*.yml` y
`*.json` sin reiniciar: cada archivo muestra sus métricas y dimensiones o sus
errores en español. Sin esta variable, la pantalla indica que no hay proyecto.
Se rechazan enlaces simbólicos y archivos mayores que 8 MiB; el máximo es
100 modelos, 32 MiB en conjunto y 4096 entradas en `metrics/`.
La pila de desarrollo monta `examples/models-project/` como proyecto de ejemplo.

Métricas de consulta M15: `OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318`
habilita exportación OTLP/HTTP protobuf a `/v1/metrics`. Sin la variable, la
instrumentación es no-op y no inicia el SDK ni hilos de exportación.
`OTEL_METRIC_EXPORT_INTERVAL` controla el intervalo en milisegundos (por defecto
60 000). El exportador tiene un plazo de 2 segundos y sus fallos no cambian
la respuesta de consulta ni readiness. Los tiempos OTel usan unidad `s`;
Prometheus agrega `_seconds` y los contadores `_total` según M15 §10.
Solo se exportan atributos de conjuntos cerrados; nunca SQL, valores ni IDs.
El tiempo de ejecución llega hasta la finalización o el deadline de la petición;
el gauge `bi_query_inflight` sigue contando trabajo nativo y recuperación hasta
liberar el worker. `bi_query_queued` incluye slots cancelados aún sin retirar;
`bi_query_queue_capacity` y `bi_query_capacity` muestran cola máxima y workers vivos.

## Metadatos: migraciones y rol de la aplicación

Por defecto (`BI_METADATA_MODE=restricted`) el servidor **nunca migra ni ejecuta
DDL**: comprueba, solo leyendo, que el esquema de `bi_meta` sea exactamente el
de esta versión y que el rol con el que se conecta tenga los permisos mínimos
y nada más. Hasta comprobarlo, y de nuevo tras perder la conexión, `/health/ready`
responde 503 (checks `migrations` y `metadata_security`) y toda operación de
`/api/v1` responde 503 `metadata_not_ready`; `/health/live` sigue respondiendo.
Cada conexión nueva que abre el servidor se comprueba también antes de usarse:
si una falla, se detiene la admisión hasta volver a comprobar.

Secuencia administrativa (en cada instalación y antes de desplegar una versión nueva):

1. Un administrador del clúster crea el rol de la aplicación, `LOGIN` y sin
   `SUPERUSER`, `CREATEROLE`, `CREATEDB`, `REPLICATION` ni `BYPASSRLS`, y sin ser
   miembro del rol dueño de los metadatos. Su contraseña nunca va al repositorio.
2. Con la credencial del dueño: `BI_DATABASE_URL=<url del dueño> bi-server migrate`.
   Termina con código 0, 1 si falla la base o una migración y 2 si falta o es
   inválida la configuración; nunca imprime la URL. Dura como mucho 5 minutos,
   también si espera un bloqueo: al agotarlos, código 1 con
   `metadata_migration_timed_out`, sin dejar nada esperando en PostgreSQL.
3. También como dueño, aplica los permisos documentados (idempotente; vuelve a
   aplicarlos tras una versión con tablas nuevas):
   `psql "<url del dueño>" -v ON_ERROR_STOP=1 -f backend/crates/bi-server/admin/runtime-grants.sql`
   (reemplaza el nombre de ejemplo `bi_runtime` por el tuyo).
4. Arranca el servidor con la credencial del rol de la aplicación; el proceso
   HTTP nunca recibe la credencial del dueño.

El servidor rechaza un rol que sea superusuario, tenga esas capacidades, sea o
pueda convertirse (por herencia o `SET ROLE`) en el dueño de la base, de
`bi_meta`, del almacenamiento de auditoría o de la tabla de migraciones, pueda
leer o escribir `audit_events`/`audit_head` fuera de `audit_append` (también por
columna o mediante `PUBLIC`) o modificar el historial de migraciones; y también
rechaza un rol al que le falte un permiso necesario. Los motivos aparecen en el
log con códigos estables (`metadata_not_ready`, `reason=...`), sin roles, URL ni
SQL, y en la métrica `bi.metadata.validation.attempts{result}`.

`BI_METADATA_MODE=development` solo se acepta con el build `dev-auth`,
`BI_DEV_USERS` configurado, un `BI_PUBLIC_ORIGIN` local y sin OIDC: migra al
arrancar y avisa con `metadata_privileged_development` si el rol es privilegiado.
`compose.yaml` lo declara de forma explícita. Nunca se deduce del entorno.

## Lago de datos (DuckLake)

DuckDB embebido con un DuckLake cuyo catálogo vive en la base `bi_lake_catalog` del mismo PostgreSQL (se crea con volúmenes nuevos; en uno existente: `podman compose exec postgres createdb -U bi bi_lake_catalog`) y cuyos datos son Parquet en `BI_LAKE_DATA_PATH` (por defecto `data/lake`).
Cargar y verificar el fixture sintético contra su oráculo, desde `backend/`: `cargo run -p bi-runtime -- load-sales` (idempotente).
Variables: `BI_LAKE_CATALOG`, `BI_LAKE_DATA_PATH`, `BI_LAKE_METADATA_SCHEMA`, `BI_DUCKDB_WORKERS`, `BI_DUCKDB_QUEUE`, `BI_DUCKDB_THREADS`, `BI_DUCKDB_MEMORY_LIMIT`, `BI_DUCKDB_EXTENSION_DIR`.

Memoria de consultas: `BI_QUERY_MEMORY_LIMIT` (por defecto `256MiB`) reserva un presupuesto
independiente de memoria gestionada por DuckDB para cada consulta activa. Si se configura
`BI_DUCKDB_MEMORY_LIMIT`, el presupuesto total se divide entre trabajadores. El límite no
abarca todo el RSS del proceso. El rechazo HTTP 422 `query_memory_limit` recomienda reducir
la consulta y se cuenta en `bi_query_rejected{reason="query_memory_limit"}`. Véase
[ADR 0013](docs/decisiones/0013-presupuesto-memoria-consulta.md) para alcance y costes.

### Consulta de demostración (Explorar)

`POST /api/v1/queries`, la consulta de Explorar, compila consultas del catálogo G1 y ejecuta
DuckDB sin el motor de permisos ni un plan semántico certificado. Por eso solo responde en
desarrollo sintético local, cuando se cumple todo lo siguiente:

- build con `dev-auth`;
- `BI_METADATA_MODE=development` y `BI_DEV_USERS` configurados;
- `BI_PUBLIC_ORIGIN` loopback y sin OIDC;
- sesión del login de desarrollo;
- el propio servidor cargó al arrancar el fixture sintético de ventas (`BI_DEV_FIXTURE_ROWS`).

El servidor solo escribe la tabla `sales` de un lago que no la tiene, o que tiene
exactamente el fixture que nombra su marca: al reutilizar un lago, o al cambiar
`BI_DEV_FIXTURE_ROWS`, la compara fila por fila con el fixture generado antes de darla por
buena o reemplazarla. Una tabla con otras filas, o sin la marca del fixture, nunca se
modifica: el servidor registra `fixture_unverified` y la consulta de demostración queda
cerrada. Para recuperar la demo, usa un lago nuevo (otros `BI_LAKE_METADATA_SCHEMA` y
`BI_LAKE_DATA_PATH`) o, en la pila Compose, recrea sus volúmenes.

En cualquier otro caso responde 403 `legacy_query_disabled` sin leer el cuerpo, también con
OIDC o para administradores, y registra la denegación en la auditoría. Si no puede
registrarla dentro de `BI_QUERY_TIMEOUT_MS` (por defecto 10 s), responde 503 `audit_unavailable` y tampoco
ejecuta. Las consultas fuera de esta demo llegarán por el flujo semántico, que aún no está
disponible.

## Frontend

Requiere Node >=22.17 (CI usa 24.21.0). En `frontend/`: `npm ci` y
`npm run dev`. Abre `http://127.0.0.1:5173`; el proxy envía `/health` y `/api`
a la API Rust en `http://127.0.0.1:8080`. En `/estado` se consulta la API real;
si no está disponible, se muestra el error y se puede reintentar.

La pila Compose habilita el login de desarrollo y configura dos identidades
sintéticas. Abre `/login` para entrar; `/estado` sigue siendo público. El resto
de la UI requiere sesión y todo `/api/v1/**` la valida en PostgreSQL, salvo los
endpoints de login de desarrollo y OIDC. Logout revoca la sesión en el servidor.

Para desarrollar con Vite, configura en el proceso Rust
`BI_PUBLIC_ORIGIN=http://127.0.0.1:5173` y
`BI_DEV_USERS=[{"id":"synthetic-analyst","display_name":"Analista sintético"}]`,
y `BI_METADATA_MODE=development`, y ejecuta
`cargo run -p bi-server --features dev-auth`. En PowerShell, asigna estas
variables con `$env:BI_PUBLIC_ORIGIN='http://127.0.0.1:5173'`,
`$env:BI_DEV_USERS='[{"id":"synthetic-analyst","display_name":"Analista sintético"}]'`
y `$env:BI_METADATA_MODE='development'`.
La cookie es HttpOnly y SameSite=Strict; con origen HTTPS añade Secure. Solo
se permite HTTP en loopback. `BI_SESSION_TTL_SECONDS` admite 60–86400 segundos
(por defecto 3600). Las operaciones mutadoras exigen el Origin configurado.
El build por defecto no admite login de desarrollo aunque exista `BI_DEV_USERS`; no añadas
`dev-auth` al build de producción. Estas sesiones solo autorizan datos sintéticos.

### Login corporativo

Configura `BI_OIDC_ISSUER` (URL exacta del emisor) y `BI_OIDC_CLIENT_ID` en el
servidor. Para un cliente confidencial, añade `BI_OIDC_CLIENT_SECRET` mediante
el gestor de secretos o el entorno del proceso. Registra en el proveedor el
callback exacto `BI_PUBLIC_ORIGIN/api/v1/oidc/callback`; exige Authorization Code
y PKCE S256. HTTPS es obligatorio salvo en loopback. No hay destinos de retorno
elegidos por el cliente. En `/login` aparece «Entrar con mi empresa».

Se conserva la sesión de servidor HttpOnly, SameSite=Strict, Secure con HTTPS
y validación de Origin. Solo la cookie de enlace OIDC, de cinco minutos,
usa SameSite=Lax para recibir el callback. El intento se consume una sola vez
antes del intercambio. Cada callback vuelve a leer JWKS para aceptar la
rotación; una caída del proveedor impide el login. No se guardan tokens OAuth.
El callback registra éxitos y rechazos en la cadena de auditoría; la creación
de sesión y su evento se confirman en la misma transacción. Si falla la
auditoría, no se crea la sesión.
La identidad se identifica por emisor y `sub`, nunca por email; registra
`groups` y únicamente los atributos adicionales enumerados en
`BI_OIDC_ATTRIBUTE_CLAIMS` (nombres separados por comas). Los grupos no conceden
permisos por sí mismos y el acceso sigue limitado a datos sintéticos hasta que
existan políticas semánticas. Cerrar sesión revoca el acceso a BI; la sesión
SSO corporativa la controla el proveedor.

Inicio y callback comparten admisión por proceso: cubo global de 120 fichas
(recarga de 2/s) y por IP del socket de 20 fichas (una cada 3 s), con un máximo
de 4096 clientes activos. No se confía en `Forwarded` ni `X-Forwarded-For`:
tras un proxy los usuarios comparten el límite de su IP. El rechazo devuelve
429 antes de contactar al proveedor o escribir en PostgreSQL; se cuenta en
`bi_oidc_login{code="rate_limited"}` y en logs agregados cada minuto, sin IP.
Los callbacks admitidos conservan la auditoría completa. PostgreSQL limita
a 1024 intentos vivos entre todas las instancias: al llenarse devuelve 503
sin insertar ni emitir cookie, con comprobación e inserción serializadas.

Prueba reproducible (Keycloak y PostgreSQL exclusivos para esta batería):
desde la raíz, `docker compose -f compose.oidc.yaml up -d` (o `podman compose`).
Espera a que responda `http://127.0.0.1:18071/realms/bi-test/.well-known/openid-configuration`.
En `backend/`, configura `DATABASE_URL=postgres://bi:bi@127.0.0.1:18072/bi`,
`BI_TEST_OIDC_ISSUER=http://127.0.0.1:18071/realms/bi-test` y
`BI_TEST_LAKE_CATALOG=dbname=bi_lake_catalog host=127.0.0.1 port=18072 user=bi password=bi`;
ejecuta `cargo test --locked --all-features -j 4`.
La fixture contiene credenciales públicas **solo de prueba**, sin valor para despliegue.

Para probar en navegador, compila `frontend/` con `npm ci` y `npm run build`.
Como OIDC solo admite el modo restringido, sigue la secuencia administrativa con
la base de la fixture (las mismas órdenes que CI): `bi-server migrate` con
`BI_DATABASE_URL=postgres://bi:bi@127.0.0.1:18072/bi`; luego, con `psql` sobre esa
URL, `CREATE ROLE bi_runtime LOGIN PASSWORD 'runtime-test-only'` y el archivo
`backend/crates/bi-server/admin/runtime-grants.sql`.
En el servidor configura `BI_BIND_ADDR=127.0.0.1:18070`,
`BI_PUBLIC_ORIGIN=http://127.0.0.1:18070`,
`BI_DATABASE_URL=postgres://bi_runtime:runtime-test-only@127.0.0.1:18072/bi`,
`BI_OIDC_ISSUER=http://127.0.0.1:18071/realms/bi-test`,
`BI_OIDC_CLIENT_ID=bi-platform`, `BI_ANALYTICS=off` y el directorio absoluto
de `frontend/build` como `BI_STATIC_DIR`; ejecuta `cargo run --locked -j 4 -p bi-server`
sin `dev-auth`. En `frontend/`, configura `BI_E2E_BASE_URL=http://127.0.0.1:18070`
y ejecuta `npm run test:e2e -- e2e/oidc.spec.ts`.
Validación manual: abre `/login`, pulsa «Entrar con mi empresa», entra en Keycloak
con `oidc-user` / `oidc-test-only`, comprueba «Sesión: Corporate Analyst», recarga
y cierra sesión. Debes volver a `/login` y no poder acceder a Inicio sin entrar.

Verificación local (los mismos pasos de CI): `npm run check:api`,
`npm run format:check`, `npm run check`, `npm run lint`, `npm test`,
`npm run build`. Tras cambiar `docs/api/openapi.json`, ejecuta
`npm run generate:api` y versiona los tipos generados.

El build queda en `frontend/build/`, con fallback SPA `200.html`, sin servidor
Node en producción. Axum usa el fallback para GET/HEAD de navegación que aceptan
HTML; archivos ausentes, fetches, `/health` y `/api` no reciben el shell.

Con la pila completa levantada, en `frontend/`: `npx playwright install chromium`
y `npm run test:e2e`. Para Podman, configura `BI_COMPOSE_COMMAND=podman`
(PowerShell: `$env:BI_COMPOSE_COMMAND='podman'`). El e2e detiene PostgreSQL,
verifica «No listo» y lo arranca en un bloque `finally`; también comprueba la
recuperación. Ejecuta estas pruebas sin otros procesos que dependan de esa base.
CI ejecuta el mismo e2e con Docker Compose en Linux y conserva trazas al fallar.

## Licencia

bi-platform se distribuye bajo la [licencia Apache 2.0](LICENSE) (ADR 0011). Los componentes de terceros conservan sus licencias, que CI vigila: solo licencias permisivas.
