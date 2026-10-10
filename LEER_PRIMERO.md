# DOCUMENTACIÓN TÉCNICA — PLATAFORMA BI NUEVA DESDE CERO

**Fuente técnica recuperada del diseño arquitectónico original.** Este paquete no presupone un repositorio, código fuente, Issues, ramas ni contratos implementados. Se parte de una carpeta local vacía y un repositorio GitHub nuevo con historia nueva.

## Contenido existente, no inventado

- `docs/arquitectura/modulos/`: **17 investigaciones técnicas M01–M17**, conservadas sin modificar respecto del archivo anterior.
- `docs/arquitectura/DECISIONES_TECNICAS_RC2.md`: decisiones de referencia del diseño original. Las menciones a un contrato 'actual' o a tareas previas son históricas, **no constatan que dichos archivos o tareas existan**.
- `docs/arquitectura/ESTANDARES_Y_PORTABILIDAD.md` y `PERFORMANCE_DESDE_G0.md`: criterios técnicos originales.
- `docs/arquitectura/gobierno/`: baseline técnico y roadmap por capacidades, no un sistema de coordinación.
- `docs/arquitectura/contratos_referencia/`: muestras de API originales, **no contratos implementados ni automáticamente vigentes**.

## Cómo comenzar desde cero

1. Crear una carpeta local **nueva y vacía** para el proyecto. Extraer este ZIP en ella: debe aparecer `docs/arquitectura/`.
2. Abrir esa carpeta en VS Code. Claude Code lee este archivo, el índice, las decisiones técnicas y el roadmap, crea un repositorio GitHub **nuevo** y configura únicamente Git/GitHub estándar, sin reutilizar proyecto anterior.
3. Claude Code prepara un clon/worktree independiente para Codex; ambos podrán trabajar en ventanas separadas de VS Code con el mismo GitHub.
4. **Antes de programar en paralelo, definir y versionar el contrato HTTP mínimo en el repositorio nuevo** a partir de las referencias técnicas, con una decisión compartida. No afirmar que existe `docs/api/openapi.yaml` hasta haberlo creado.
5. Crear los Issues de la primera entrega **en el repositorio nuevo**: backend Rust + DuckDB y frontend SvelteKit + ECharts. Los números de Issues los asigna GitHub, no están preestablecidos.
6. Ambos agentes consultan el índice y los módulos pertinentes para su tarea. No necesitan releer los 17 en cada PR. No inventar una arquitectura alternativa; las desviaciones se justifican en el Issue/PR.
7. Coordinarse únicamente con Git, GitHub Issues, ramas o worktrees, Pull Requests, comentarios y GitHub Actions. **No construir scripts ni protocolos propios de coordinación de agentes.**

## Qué NO contiene

No incluye código ejecutable del BI, repositorio Git, `AGENTS.md` ni `CLAUDE.md`, scripts propios de coordinación, ni contratos/API ya implementados. Ninguna afirmación de pruebas pasadas del proyecto borrado tiene vigencia en el proyecto nuevo.

**Prioridad:** una primera capacidad real extremo a extremo, código y pruebas; no burocracia de arranque. La documentación sirve como diseño de referencia, no como evidencia de que el software exista.
