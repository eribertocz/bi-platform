# bi-platform

Plataforma BI nueva: monolito modular Rust (Axum, DuckDB, PostgreSQL) y frontend SvelteKit. `bi-platform` es un nombre provisional de ingeniería.

- Arquitectura de referencia: [LEER_PRIMERO.md](LEER_PRIMERO.md) y [docs/arquitectura/](docs/arquitectura/INDICE_17_MODULOS.md).
- Plan por gates: [ROADMAP_GLOBAL.md](docs/arquitectura/gobierno/ROADMAP_GLOBAL.md).
- Contrato HTTP vigente: [docs/api/openapi.json](docs/api/openapi.json).
- Reglas de colaboración: [AGENTS.md](AGENTS.md).

Estado: G0 — Fundaciones, en construcción.

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
cargo run -p bi-server        # API sin login de desarrollo; health sigue público
```

En otra terminal: `cd frontend`, `npm ci` y `npm run dev`. Vite sirve
`http://127.0.0.1:5173` y envía `/health` y `/api` a Rust en el puerto 8080.
Si la pila completa estaba levantada, detén antes `app` para liberar ese puerto.

Pruebas: en `backend/`, `cargo fmt --all --check`,
`cargo clippy --locked --all-targets --all-features -- -D warnings` y
`cargo test --locked`; con PostgreSQL real, configura
`DATABASE_URL=postgres://bi:bi@127.0.0.1:54320/bi` y ejecuta
`cargo test --locked --all-features` (PowerShell: `$env:DATABASE_URL='postgres://bi:bi@127.0.0.1:54320/bi'`).
Variables: `BI_BIND_ADDR`, `BI_DATABASE_URL` (por defecto la base local de
`compose.yaml`), `BI_LOG_FORMAT=text|json`, `RUST_LOG` y `BI_STATIC_DIR`
(directorio del build; sin esta variable, Rust sirve solo la API).

## Lago de datos (DuckLake)

DuckDB embebido con un DuckLake cuyo catálogo vive en la base `bi_lake_catalog` del mismo PostgreSQL (se crea con volúmenes nuevos; en uno existente: `podman compose exec postgres createdb -U bi bi_lake_catalog`) y cuyos datos son Parquet en `BI_LAKE_DATA_PATH` (por defecto `data/lake`).
Cargar y verificar el fixture sintético contra su oráculo, desde `backend/`: `cargo run -p bi-runtime -- load-sales` (idempotente).
Comparar DuckDB SQL y Polars sobre el mismo fixture (exactitud + tiempos, informe JSON): `cargo run --release -p bi-engines -- --out engines.json`.
Variables: `BI_LAKE_CATALOG`, `BI_LAKE_DATA_PATH`, `BI_LAKE_METADATA_SCHEMA`, `BI_DUCKDB_WORKERS`, `BI_DUCKDB_QUEUE`, `BI_DUCKDB_THREADS`, `BI_DUCKDB_MEMORY_LIMIT`, `BI_DUCKDB_EXTENSION_DIR`.

## Frontend

Requiere Node >=22.17 (CI usa 24.21.0). En `frontend/`: `npm ci` y
`npm run dev`. Abre `http://127.0.0.1:5173`; el proxy envía `/health` y `/api`
a la API Rust en `http://127.0.0.1:8080`. En `/estado` se consulta la API real;
si no está disponible, se muestra el error y se puede reintentar.

La pila Compose habilita el login de desarrollo y configura dos identidades
sintéticas. Abre `/login` para entrar; `/estado` sigue siendo público. El resto
de la UI requiere sesión y todo `/api/v1/**` la valida en PostgreSQL, salvo los
dos endpoints de login de desarrollo. Logout revoca la sesión en el servidor.

Para desarrollar con Vite, configura en el proceso Rust
`BI_PUBLIC_ORIGIN=http://127.0.0.1:5173` y
`BI_DEV_USERS=[{"id":"synthetic-analyst","display_name":"Analista sintético"}]`,
y ejecuta `cargo run -p bi-server --features dev-auth`. En PowerShell, asigna
estas variables con `$env:BI_PUBLIC_ORIGIN='http://127.0.0.1:5173'` y
`$env:BI_DEV_USERS='[{"id":"synthetic-analyst","display_name":"Analista sintético"}]'`.
La cookie es HttpOnly y SameSite=Strict; con origen HTTPS añade Secure. Solo
se permite HTTP en loopback. `BI_SESSION_TTL_SECONDS` admite 60–86400 segundos
(por defecto 3600). Las operaciones mutadoras exigen el Origin configurado.
El build por defecto no admite login aunque exista `BI_DEV_USERS`; no añadas
`dev-auth` al build de producción. Estas sesiones solo autorizan datos sintéticos.

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
