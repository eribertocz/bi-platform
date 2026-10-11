# 0003 — Definiciones como archivos de un proyecto en Git

**Estado:** aprobada por el propietario, 2026-10-11 (Issue #28). El propietario mostró el explorador de proyecto de Rill Developer como modelo. **Concreta:** M10 §0, puntos 1–3.

## Contexto
M10 pide definiciones declarativas, portables y versionables, con "archivos legibles y Git como flujo principal", y PostgreSQL como repositorio multiusuario opcional. Rill, Cube, Lightdash, dbt y Evidence guardan el proyecto como código. Power BI se ha movido en la misma dirección con PBIP/TMDL, y Looker con LookML.

## Decisión
```
mi-proyecto/
├── project.yaml     # nombre, conexión por defecto, valores por defecto, variables, usuarios de prueba
├── connectors/      # *.yaml: DuckLake, PostgreSQL, ClickHouse…; credenciales solo como referencia a secretos
├── sources/         # *.yaml: qué se importa y cuándo (M01)
├── models/          # *.sql: transformaciones en SQL de DuckDB (M02)
├── metrics/         # *.yaml: modelo semántico y métricas, Apache Ossie + extensiones con espacio de nombres (M04)
├── dashboards/      # *.yaml: exploraciones y dashboards (M06–M09)
├── apis/            # *.yaml: APIs sobre métricas (M12)
└── alerts/          # *.yaml: alertas e informes programados (M14)
```
- Cada archivo es un **recurso tipado** con esquema validable (JSON Schema publicado) y errores por archivo y línea.
- La UI incluye un **explorador de proyecto** (árbol, "Añadir" con plantillas, edición validada) y recarga en caliente (ver `docs/referentes/rill.md`, punto R1).
- **PostgreSQL** guarda solo el catálogo de DuckLake (`bi_lake_catalog`) y el estado operativo (`bi`): usuarios, sesiones, permisos, ejecuciones, auditoría y punteros a revisiones publicadas.

## Alternativas descartadas
- **Definiciones en PostgreSQL como formato principal:** no son revisables en Git, son difíciles de portar y crean dependencia de la base de datos (M10 la descarta como núcleo).
- **Una extensión de archivo propietaria** (`.pbix`, `.twbx`, `.sqlviz`): no es legible ni revisable línea a línea.

## Consecuencias
- G2 empieza por `metrics/` (Ossie) y por el cargador y validador del directorio de proyecto.
- Más adelante, opcional (G4): si muchos usuarios no técnicos editan dashboards desde la UI a la vez, un repositorio multiusuario en PostgreSQL con historial, **siempre exportable e importable a estos mismos archivos**.
