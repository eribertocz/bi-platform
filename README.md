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
cargo run -p bi-server        # API en http://127.0.0.1:8080 (/health/live, /health/ready)
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

## Frontend

Requiere Node >=22.17 (CI usa 24.21.0). En `frontend/`: `npm ci` y
`npm run dev`. Abre `http://127.0.0.1:5173`; el proxy envía `/health` y `/api`
a la API Rust en `http://127.0.0.1:8080`. En `/estado` se consulta la API real;
si no está disponible, se muestra el error y se puede reintentar.

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
