# AGENTS.md — reglas de trabajo (Claude Code y Codex)

Dos ingenieros full-stack, cada uno en su propio clon y rama. Coordinación únicamente con Git, GitHub Issues, Pull Requests, comentarios y GitHub Actions. No crear scripts ni protocolos propios de coordinación.

## Referencia técnica

- Arquitectura: `LEER_PRIMERO.md`, `docs/arquitectura/INDICE_17_MODULOS.md`, `docs/arquitectura/DECISIONES_TECNICAS_RC2.md`, `docs/arquitectura/gobierno/ROADMAP_GLOBAL.md`; consultar el módulo pertinente a cada tarea.
- No inventar una arquitectura alternativa. Toda desviación se justifica en el Issue o PR.
- `docs/arquitectura/` es la documentación original: no se modifica (hashes en `INVENTARIO.md`).
- Contrato HTTP vigente: `docs/api/openapi.json`. Los archivos de `contratos_referencia/` son solo muestras.

## Flujo

1. Se avanza por gates del roadmap en orden (G0, G1, …). No se inicia un gate sin cumplir los criterios funcionales del anterior.
2. Cada tarea es un Issue con criterios de aceptación y dependencias, dentro del milestone del gate. Para tomarlo: etiqueta `agent:claude` o `agent:codex` y comentario breve.
3. Rama desde `main` actualizado: `claude/<n>-descripcion` o `codex/<n>-descripcion`. Nunca push directo ni force-push a `main`.
4. Un PR pequeño por Issue, con `Closes #n`, qué cambia, cómo se probó y riesgos abiertos.
5. Revisión cruzada obligatoria: el otro agente revisa el código y lo prueba. Ambos usamos la misma cuenta GitHub, por eso no existe "Approve": se aprueba con `gh pr review --comment` escribiendo **Aprobado**, o se piden cambios concretos.
6. Merge (squash) por el autor solo con CI verde y aprobación del otro agente; después se borra la rama.
7. Todo comentario en GitHub empieza con **[Claude]** o **[Codex]**.
8. Commits con la identidad del agente configurada en su clon, nunca la del propietario.
9. Decisiones técnicas rutinarias: las toman los agentes. Decisiones fundamentales de producto: se consultan al propietario (@eribertocz) en el Issue.

## Calidad

- Software funcionando y pruebas reales; nada simulado que aparente funcionalidad.
- Antes de abrir o actualizar un PR deben pasar localmente formato, lint, tipos, pruebas y build del área tocada (los mismos comandos que ejecuta CI).
- Rust: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`. Sin llamadas bloqueantes (DuckDB/Polars/FFI) dentro de executores async.
- Tipos exactos: `DECIMAL` e `INT64` no se redondean en JS; se preservan `TIMESTAMPTZ` y `NULL`.
- Seguridad: autorización en el servidor; nunca secretos en Git, logs ni comentarios.
- Dependencia nueva: una línea en el PR con problema, paquete y licencia, alternativa y motivo.
- Rendimiento: en rutas críticas, medir antes y después (`docs/arquitectura/PERFORMANCE_DESDE_G0.md`).
- Documentación nueva solo cuando algo no se pueda expresar en código, contrato o pruebas.
