# bi-platform

Plataforma BI nueva: monolito modular Rust (Axum, DuckDB, PostgreSQL) y frontend SvelteKit. `bi-platform` es un nombre provisional de ingeniería.

- Arquitectura de referencia: [LEER_PRIMERO.md](LEER_PRIMERO.md) y [docs/arquitectura/](docs/arquitectura/INDICE_17_MODULOS.md).
- Plan por gates: [ROADMAP_GLOBAL.md](docs/arquitectura/gobierno/ROADMAP_GLOBAL.md).
- Contrato HTTP vigente: [docs/api/openapi.json](docs/api/openapi.json).
- Reglas de colaboración: [AGENTS.md](AGENTS.md).

Estado: G0 — Fundaciones, en construcción.

## Backend en desarrollo

```sh
podman compose up -d          # o: docker compose up -d  (PostgreSQL en 127.0.0.1:54320)
cd backend
cargo run -p bi-server        # API en http://127.0.0.1:8080 (/health/live, /health/ready)
```

Pruebas: `cargo test`; con PostgreSQL real: `DATABASE_URL=postgres://bi:bi@127.0.0.1:54320/bi cargo test --all-features`.
Variables: `BI_BIND_ADDR`, `BI_DATABASE_URL` (por defecto la base local de `compose.yaml`), `BI_LOG_FORMAT=text|json`, `RUST_LOG`.

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
Node en producción. El servidor de archivos debe usar el fallback en rutas de
navegación; `/health` y `/api` deben resolverse en Rust y nunca devolver el shell.
La integración del servidor estático Rust se entrega en el gate G0.
