# Leyes fundamentales de bi-platform

**Origen:** documento del propietario "Leyes Fundamentales de una Herramienta BI" (2026-10-11). Aquí se adopta, se concreta para este proyecto y se mejora.

Son **innegociables**: antes de implementar cualquier funcionalidad (frontend, backend, motor semántico o ELT) se comprueba que no viola ninguna ley. Si dos chocan, priman las leyes **1, 2, 3 y 5**.

**Son retroactivas:** lo ya construido se audita contra ellas. Cada incumplimiento se resuelve en su Issue, con prioridad para las leyes 1, 2, 3 y 5. Seguimiento: Issue #84.

Para cada ley se indica cómo la cumple el proyecto (✅ hecho; 🔄 en curso; ⏳ planificado, con su gate) y qué mejora añadimos sobre el texto original.

## 1. Verdad única
**Todo número mostrado se rastrea hasta una definición semántica única, versionada y auditada.**

| Requisito | Cómo se cumple |
|---|---|
| El motor semántico es la única fuente de métricas, dimensiones y relaciones | ✅ SEF (ADR 0006); 🔄 punto único de aplicación (ADR 0010) |
| Ningún cálculo de negocio en el frontend | ✅ Las pantallas consumen contratos (ADR 0008 §5) |
| Cada métrica con nombre canónico, descripción, fórmula, responsable e historial | ✅ Nombre, descripción, fórmula e historial (Ossie en Git, ADR 0003); ⏳ **responsable** (`owner`) en la extensión `bi-platform` (G2) |
| Los usuarios de negocio no escriben SQL crudo para métricas | ✅ El editor SQL está bloqueado hasta el punto único (ADR 0010) |

**Mejora: los totales y subtotales también los calcula el motor.**
- Sumar en pantalla las filas de una métrica no aditiva da un total falso. Por ejemplo, con "clientes distintos" por región un cliente cuenta en dos regiones.
- Todo total se pide al motor como un bloque más de la consulta (SQIR admite varios granos por petición), y el motor decide si se suma o se recalcula.
- El frontend solo hace transformaciones de presentación: formato, orden visual y escala.

## 2. Transparencia y auditabilidad
**Cualquiera con permiso puede responder "¿de dónde sale este número y por qué?".**

| Requisito | Cómo se cumple |
|---|---|
| Ver la definición, los filtros (incluidos los de seguridad) y la fecha de los datos | 🔄 *Explain* del planificador (`04_CORRECTNESS_AND_PLANNING.md` §26), con hechos y obligaciones del motor de corrección (#83) |
| Ver la consulta generada | ⏳ Disponible, pero **en segundo plano**: la explicación primero se da en términos semánticos (§26: "no debe obligar a leer SQL") |
| Linaje fuente → modelo → métrica → visualización | ⏳ G3 (ELT) y G4 (visualización), hasta la columna |
| Cambios del modelo versionados y reversibles | ✅ Git (ADR 0003); ⏳ versiones de datos en DuckLake |
| Registro de **quién consultó qué y cuándo** | 🔄 Auditoría con cadena de hashes (#71, acción `query`) |

**Mejora: el "recibo del número".**
- Cada resultado lleva un recibo con:
  - la consulta canónica (SQIR), no solo su huella, porque una huella no reconstruye la consulta;
  - la versión del modelo (CSMIR) y la huella del plan (SEIR);
  - el origen de los datos, según el modo (ADR 0009):
    - **materializado**: la versión de DuckLake que se leyó;
    - **en vivo**: la fuente, el instante de la lectura y sus límites. No hay versión que prometer.
- El recibo declara su **capacidad de reproducción**:
  - **exacta** solo mientras se conserven todas las versiones necesarias, dentro de la retención declarada del lago;
  - **solo explicable** si la versión de datos expiró o la consulta fue en vivo: se demuestra qué definición se usó y qué se observó, sin prometer el mismo resultado.

## 3. Seguridad por diseño
**La seguridad se aplica antes de que los datos salgan del motor; nunca solo en el frontend.**

| Requisito | Cómo se cumple |
|---|---|
| Seguridad por filas (RLS) en el motor | 🔄 Políticas obligatorias con contexto tipado: el motor de corrección exige `ApplyMandatoryPolicy` (#83), y una propiedad probada garantiza que ningún filtro del usuario la quita |
| Seguridad por columnas (CLS) | ⏳ **Pendiente.** Se añade al motor de permisos (#69): campos y métricas visibles por rol, con enmascaramiento |
| Roles, permisos granulares y mínimo privilegio | ⏳ Motor de permisos con denegación por defecto (ADR 0010, #69) |
| SSO: OIDC | 🔄 #70 |
| SSO: SAML y SCIM | ⏳ Después de G2, para entornos empresariales |
| Datos sensibles nunca sin filtrar hacia el frontend | 🔄 Punto único decidido (ADR 0010) pero **aún no integrado**: hoy una sesión consulta todas las filas sintéticas. Las piezas (#69, #70, #83 y #86) todavía no forman una vía de consulta |
| El modo avanzado de gráficos no puede saltarse la seguridad | ⏳ Cuando el punto único esté integrado, los datos llegarán ya filtrados; ver la mejora |

**Mejora: el modo avanzado de ECharts es un riesgo de inyección (XSS).**
- Sus opciones admiten **funciones JavaScript** (por ejemplo `formatter`) y HTML en los *tooltips*.
- Solo se aceptan opciones **JSON sin funciones**, validadas contra una lista de propiedades permitidas, y el texto enriquecido se limpia.
- Formatos y plantillas se expresan de forma declarativa.

## 4. Consistencia
**El mismo número es el mismo número en todas partes, dentro de la frescura declarada.**

| Requisito | Cómo se cumple |
|---|---|
| Misma métrica y mismos filtros, mismo resultado | ✅ Motor determinista con huellas canónicas (CSMIR, SQIR) |
| Frescura visible | ⏳ "Datos de hace X min" en cada panel; frescura declarada y propagada (propuesta ELT) |
| Modelos de ELT deterministas y reproducibles | ⏳ G3, con versiones de DuckLake |
| Caché semántica, no de SQL | ⏳ Bloqueada hasta el punto único (ADR 0010) |
| *Time travel* | ⏳ Versiones de DuckLake ("ver como estaba el martes") |

**Mejora: la clave de la caché.**
- Incluye la consulta canónica, la versión del modelo y de los datos, **y la huella de la decisión de permisos** de quien pregunta: versión de las políticas y atributos que intervinieron (ADR 0010 §1 y ADR 0012).
- Sin ese componente, un usuario podría recibir el resultado guardado de otro con más permisos.
- El contexto del usuario no basta. Al cambiar una política o un atributo (una revocación), la huella cambia y lo cacheado deja de servir.

## 5. Separación de capas
**El frontend nunca conoce la estructura física de las tablas.**

| Capa | En bi-platform |
|---|---|
| Ingesta / ELT | Modelos con cabecera `MODEL (...)` propia, en Rust (ADR 0005) |
| Almacenamiento | DuckLake, PostgreSQL o ClickHouse, o consulta en vivo (ADR 0002 y 0009) |
| Motor semántico | SEF en Rust, motores detrás de adaptadores, Arrow como intercambio (ADR 0006 y 0008) |
| API de consulta | OpenAPI tipada con valores exactos (ADR 0008) |
| Frontend | SvelteKit 3, Svelte 5, ECharts y bibliotecas de primer nivel |

✅ CI vigila las capas (ADR 0008: `deny.toml` y `backend.yml`).

**Mejora: distinguir dos públicos.**
- El **estudio de autor**, para quien construye modelos, muestra tablas y archivos de forma deliberada.
- Las vistas de **consumo** (dashboards, exploración, API, IA) solo conocen el modelo semántico.

## 6. Experiencia por niveles
**Sirve al usuario de negocio y al analista avanzado sin sacrificar a ninguno.**
- **Básico:** métricas, dimensiones y filtros del modelo; no se puede romper la visualización.
- **Intermedio:** ejes, series, tipos de gráfico, colores y *tooltips*, dentro de un subconjunto validado.
- **Avanzado:** casi todo el objeto de ECharts, solo JSON y validado (ley 3); los datos llegan ya filtrados.

**Mejora:**
- Los tres niveles son vistas del **mismo** contrato visual (`VisualSpec`, G4).
- Al bajar de nivel, el contrato se conserva completo: el nivel básico **señala** las opciones que no sabe editar y las deja intactas. Nunca las reescribe ni las pierde en silencio.
- No toda opción avanzada tiene representación básica, ni se promete.
- La misma idea vale para todo el producto: lienzo o código en el ELT, formulario o YAML en las métricas.

## 7. Rendimiento predecible
**El usuario sabe qué esperar.**

| Requisito | Cómo se cumple |
|---|---|
| Plazos, cancelación y admisión por recurso | ✅ G1 (`bi-runtime`) |
| Límites por usuario o rol | ⏳ Con el motor de permisos |
| Estimación de costo antes de ejecutar | ⏳ Optimizador semántico, después del planificador base, con estadísticas de DuckDB |
| Caché semántica | ⏳ Ley 4 |
| Consultas asíncronas y materializaciones | ⏳ ELT: modos en vivo, caché, materializado e incremental |
| Estados de carga, progreso y error claros | ✅ e2e de G1; ⏳ en todas las pantallas |

**Mejora: el aviso antes de esperar.** Si la estimación supera el objetivo de velocidad declarado, el sistema avisa antes de ejecutar y propone una alternativa ("materializar este agregado lo dejaría en 0,3 s").

## 8. Gobierno y ciclo de vida del modelo
**El modelo semántico es código.**
- ✅ Versionado en Git (ADR 0003).
- ⏳ Entornos de desarrollo, staging y producción, como ramas con datos sin copia en DuckLake.
- ✅ Revisión por PR.
- ⏳ Pruebas de métricas: valores esperados en el corpus, auditorías en el ELT.
- ⏳ Documentación viva generada desde Ossie.

**Mejora: deprecación controlada.**
- Una métrica se marca `deprecated` con fecha de retirada y reemplazo.
- El motor avisa a quien la consulta y el panel de Problemas lista sus usos.
- No se puede borrar mientras alguien la use.

## 9. Observabilidad
**Si no se mide, no se opera.**
- ✅ `tracing` con `request_id` y métricas OTel (M15).
- ⏳ Uso por dashboard, métrica y usuario.
- ⏳ Latencia p50, p95 y p99.
- ⏳ Tasa de aciertos de la caché.
- ⏳ Alertas de degradación.

**Mejora: reproducir desde el log.**
- El log de cada consulta guarda su recibo (ley 2), no el SQL.
- Como el motor es determinista, la consulta problemática se reproduce exactamente con el mismo modelo y los mismos datos, **si el recibo declara reproducción exacta** y esos datos siguen retenidos.

## 10. Extensibilidad controlada
**Se extiende sin romper el núcleo.**

| Requisito | Cómo se cumple |
|---|---|
| Conectores aislados | ✅ Adaptadores (ADR 0008) |
| Visualizaciones nuevas sin tocar el motor | ⏳ Registro de renderizadores sobre `VisualSpec` (G4) |
| API pública estable | ✅ OpenAPI versionada. ⏳ El motor como biblioteca al cerrar G2 y como servicio más adelante (ADR 0011) |
| Modo avanzado aislado | Ver la ley 3 |

## Tres leyes que añadimos
- **11. Exactitud demostrable.**
  - Decimales exactos de punta a punta, sin redondeos de JavaScript ni `0.30000000000000004`.
  - Nulos según la definición.
  - Si el motor no puede demostrar que un número es correcto, lo **rechaza** con un motivo (corpus normativo, motor de corrección).
- **12. Apertura.**
  - Sin encierro: el modelo entra y sale como Apache Ossie, los datos como Arrow y Parquet, las definiciones como archivos en Git.
  - Toda exportación declara lo que pierde.
- **13. Accesibilidad.**
  - WCAG 2.2 AA en todas las pantallas (ADR 0010): teclado, lector de pantalla y contraste, verificado con axe.

## Regla para los agentes
Todo PR de producto indica qué leyes toca y cómo las cumple. Un PR que viola una ley no se integra, aunque CI esté en verde.
