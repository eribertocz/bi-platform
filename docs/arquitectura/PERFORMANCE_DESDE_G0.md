# Rendimiento y control de recursos desde el primer código, verificables

**No aplazar el rendimiento a G6.** Desde la fundación deben existir fixture sintético, oráculo, harness reproducible y medición de latencia/cold-warm, CPU, memoria, I/O, serialización y frontend. Los presupuestos definitivos se calibran con evidencia.

## Gates iniciales

- G0.10: fixtures tipados y oráculo numérico; G0.13: harness benchmark mínimo con machine profile/commit/versiones, smoke de serialización JSON, métrica RSS y temporización; G0.14: compilación/smoke por OS; G0.15: matriz de versiones stable/experimental.
- G1: contrastar DuckDB SQL, Polars LazyFrame y puentes Arrow en fixture de dimensión realista; medir `plan/build/execute/serialize/render`, cold/warm y distintas cardinalidades. **Exactitud primero**.
- PR sobre hot path: comparación mismo hardware/dataset/herramientas, p50/p95 (p99 si muestra suficiente), throughput, pico de memoria, bytes leídos y número de conversiones; baseline y margen de regresión acordado en revisión. Si no es estadísticamente concluyente, reportarlo.
- Consultas: admisión, cuota, cancelación, deadlines, spill, streaming/backpressure; no bloquear Tokio con motor nativo. La memoria RSS de proceso es diferente de `memory_limit` DuckDB.
- UI: build chunks, JS descargado, main-thread long tasks, hidratar solo interactivos, virtualización, tamaño de objetos de datos y `dispose` de ECharts/GridStack. No enviar millones de filas por JSON solo para dibujar tres barras.
- CI: mediciones deterministas ligeras por PR; nightly / manual cargas más representativas, perfiles detallados y tres OS. Evitar gates de microbench ruidosos en runners compartidos; instrumentar tendencia y comparar entorno controlado.

## Formato breve de benchmark por microtarea

```text
Task / SHA / OS / CPU / RAM / versiones / seed / fixtures:
Medida previa y nueva (p50/p95 si aplica):
Filas, bytes, memoria pico, tiempo de serialización, errores:
Correctness oracle / seguridad:
Regresión o mejora, confianza y limitaciones:
Próximo experimento o N/A razonado:
```
