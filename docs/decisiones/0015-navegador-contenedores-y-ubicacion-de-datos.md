# 0015 — Experiencia web, contenedores oficiales y ubicación independiente de los datos

**Estado:** aprobada por el propietario el 2026-10-13 ([conformidad](https://github.com/eribertocz/bi-platform/issues/117#issuecomment-6159822059)); se integra mediante el PR documental de [#117](https://github.com/eribertocz/bi-platform/issues/117). Las capacidades que describe siguen pendientes: ninguna está implementada y cada una se especificará y probará en su gate. **Referencias:** ADR 0003, 0004, 0008, 0009, 0010 y 0012; M03, M08, M10, M11, M12 y M17; roadmap G3/G4/G6. No declara capacidades implementadas ni autoriza comenzar gates posteriores a G2.

## Contexto

El propietario quiere crear y consumir dashboards desde el navegador, compartirlos dentro de una empresa y elegir dónde se guardan los datos. La plataforma puede estar instalada en un equipo de la empresa o en infraestructura de nube contratada por ella. Ninguna de esas opciones implica un servicio alojado y operado por nosotros.

Las ADR 0004/0009 ya permiten conexiones y destinos elegidos explícitamente, sin importar todo a DuckLake. Falta expresar la relación entre instalación, publicación de dashboards, disponibilidad y acceso de los viewers.

## Decisión

### 1. Una experiencia web y una distribución principal

Autores y viewers usan el navegador. El administrador instala el servidor mediante contenedores oficiales versionados, con almacenamiento persistente y secretos externos a la imagen. El mismo producto debe poder desplegarse en infraestructura de la empresa o de nube; los perfiles y sistemas operativos efectivamente soportados se validarán y anunciarán en G6.

Instaladores nativos pueden evaluarse como complemento si hay una necesidad demostrada. No son requisito de la experiencia principal. No se requiere pip, una aplicación de escritorio ni Kubernetes para consumir dashboards. Esta decisión tampoco promete un servicio comercial gestionado por nosotros.

### 2. Tres elecciones independientes

| Elección | Opciones |
|---|---|
| Dónde corre el servidor BI | Equipo/servidor de la empresa o infraestructura de nube elegida por ella |
| Quién puede entrar | Usuarios y grupos autorizados, por red interna/VPN o por acceso HTTPS configurado por la empresa |
| Dónde se consultan o guardan los datos | Conexión local, remota o híbrida, dentro de las capacidades verificadas del conector |

Una URL accesible desde Internet no convierte el dashboard en público. La red determina desde dónde se llega; la identidad y las políticas de la plataforma determinan qué puede ver cada persona. El acceso por red interna/VPN no sustituye la autorización.

La conexión a datos se establece desde el servidor BI, no desde el navegador del autor o del viewer. Tener Internet no demuestra acceso: hacen falta conectividad, credenciales y capacidades del conector verificadas. Una ruta local es local al servidor y debe ser accesible por su proceso/contenedor.

Un servidor BI en nube puede consultar datos dentro de la empresa mediante conectividad privada configurada por ella. Esta decisión no construye un gateway propio, no abre bases de datos a Internet ni garantiza conectividad automáticamente.

### 3. Libertad de motor y destino, sin copias implícitas

Se conserva ADR 0009: consulta directa, resultados transformados o pipeline de actualización. RAW/SILVER/GOLD son conceptos lógicos y opcionales; no se fuerza una copia por capa. No se cambia la elección explícita del conector de ADR 0004.

| Ejemplo | Comportamiento |
|---|---|
| Archivos Excel/Access dentro de la empresa | Cuando su conector esté especificado y verificado, se puede extraer y transformar al destino elegido; DuckLake es una recomendación posible, no una obligación |
| ClickHouse local con modelos analíticos preparados | Consultar en ClickHouse; materializar allí si el destino y operación están soportados; trasladar a DuckLake solo por elección explícita |
| ClickHouse en nube | Consultar en su motor sin un DuckLake intermedio obligatorio |
| DuckLake remoto: catálogo PostgreSQL y archivos en S3 compatible | El servidor accede a ambos componentes; no basta conectar solo el catálogo |
| Datos remotos que se desean acercar al servidor | Traslado o materialización explícitos, con coste, actualización y consistencia visibles; no copia silenciosa |
| DuckLake en disco de un servidor en nube | Válido con almacenamiento persistente y catálogo configurado; no exige un servicio externo de objetos en todos los casos |
| Servidor BI en nube y datos en la empresa | Válido si el servidor alcanza el origen por conexión privada autorizada y el conector lo admite |

Mover datos cerca del motor puede reducir la latencia de red. No garantiza latencia cero ni una mejora total: deben medirse consulta, transferencia, cálculo, disco, concurrencia y frescura. La optimización se decide con evidencia; una copia tiene coste de almacenamiento y actualización.

Para servidores de equipo se recomienda un catálogo DuckLake PostgreSQL. El catálogo en archivo mantiene la limitación de un proceso de ADR 0004; no se comparte el archivo entre procesos/replicas para simular un catálogo multiusuario.

### 4. Dónde viven el dashboard y los datos

Las definiciones canónicas siguen en los archivos versionados del proyecto, incluidos `dashboards/*.yaml`, según ADR 0003. La instancia compartida mantiene las revisiones publicadas disponibles en almacenamiento persistente; Git es compatible con este flujo, no un requisito de uso manual para autores o viewers.

PostgreSQL de la plataforma guarda usuarios, sesiones, permisos, auditoría y referencias operativas a revisiones publicadas. No es el almacén de los hechos analíticos. La alternativa opcional de edición multiusuario en PostgreSQL sigue pendiente de diseño en G4, sin cambiar el formato portable.

DuckLake distingue catálogo y almacenamiento de los datos, configurables por separado. El catálogo del lago y la base de metadatos de la plataforma conservan funciones, permisos, migraciones y recuperación independientes. Los datos analíticos también pueden permanecer en ClickHouse u otro origen soportado.

Los discos que deban sobrevivir a reinicios/actualizaciones no se guardan únicamente en la capa efímera del contenedor. Las copias de seguridad deben cubrir las definiciones, los metadatos operativos y, cuando se administra un lago, su catálogo y archivos de manera consistente.

### 5. Publicar y compartir dentro de una instancia

El autor prepara un borrador, valida sus dependencias, publica una revisión y concede acceso a usuarios/grupos autorizados. Comparte una URL de esa instancia; no despliega un servidor o un sitio independiente por dashboard.

Una revisión publicada es inmutable. Un borrador inválido o un fallo al publicar no sustituye la última revisión válida. La actualización del puntero publicado debe ser atómica y detectar conflictos de edición. Los detalles de contrato/concurrencia y sus pruebas se especificarán en G4.

Publicar una definición no copia ni congela automáticamente los 100 millones de filas. La política de datos actualizados o de un informe con corte fijo debe acordarse por separado: versión del dashboard y versión de los datos son conceptos distintos. Los viewers reciben los resultados autorizados necesarios para la visualización, no el dataset completo para resolver permisos en el navegador.

Compartir el enlace no concede los permisos del autor. Cada consulta, caché y exportación aplica la identidad y políticas del viewer por el punto único del motor semántico. No se exponen credenciales, acceso directo al lago ni rutas de archivos como mecanismo de compartir. Una revocación se respeta en las solicitudes posteriores; rollback de una revisión no restaura permisos antiguos.

### 6. Comprobación antes de compartir y traslado entre instalaciones

La experiencia propuesta es: crear → comprobar → elegir audiencia → publicar → copiar enlace. La comprobación explica problemas de acceso, dependencias, configuración y disponibilidad sin exigir al autor manejar contenedores, SQL o rutas físicas.

Si datos y plataforma solo existen en una laptop, el enlace depende de que esa instalación siga disponible. No se afirma independencia de la laptop por comprobar una conexión una vez; deben considerarse todas las dependencias y las cargas futuras.

Trasladar a otra instancia es una operación diferente de publicar. Primero se elige destino y se muestra qué se trasladará, su tamaño conocido/estimado y qué seguirá dependiendo del origen. Después se valida el destino antes de cambiar referencias; nunca se suben datos automáticamente al compartir.

El traslado de DuckLake requiere conservar una versión consistente de catálogo y archivos, resolver rutas y secretos en destino y validar equivalencia. Una copia incompleta no se publica como válida; el origen se conserva hasta que el destino esté validado. Protocolo de transferencia, reanudación, retención y prueba de recuperación tendrán su propia especificación y validación en G3/G6.

### 7. Ejemplos y rechazos

Casos de revisión de esta decisión. Describen el comportamiento acordado, no funcionalidades entregadas: cada uno tendrá especificación y pruebas reales en su gate.

| Caso | Comportamiento acordado |
|---|---|
| BI en la empresa + ClickHouse local | Consulta sin migración forzosa |
| BI en la empresa + DuckLake remoto (catálogo PostgreSQL, archivos S3) | Ambos componentes accesibles desde el servidor, sin copia implícita |
| BI en nube + ClickHouse Cloud | Consulta directa si el conector está verificado |
| BI en nube + DuckLake en un volumen persistente de esa infraestructura | Válido; no se exige S3 |
| BI en nube + base interna por conexión privada | Permitido si hay alcance y autorización; no se promete un gateway propio |
| BI en nube + archivo que solo existe en la laptop del autor | **Rechazo:** no se afirma que el servidor lo alcanza |
| Enlace interno abierto por un usuario sin permisos | **Rechazo:** pertenecer a la red no concede acceso |
| Viewer con políticas distintas de las del autor | Sus datos siguen limitados a su propia identidad |
| Publicar un dashboard sobre 100 millones de filas | No obliga a copiar filas por publicación ni por viewer |
| Publicar una definición inválida o un traslado incompleto | **Rechazo:** no sustituye la revisión ni el destino válidos |
| Cambiar el destino de los datos | No exige rediseñar el dashboard si conserva el contrato semántico y pasa la validación |
| Trasladar datos remotos | Coste y frescura explícitos, sin promesa de latencia cero |

## Alternativas

- Solo nube operada por nosotros: no cubre el control y alojamiento propios solicitados y añade operación comercial de un servicio.
- Datos obligatoriamente junto al servidor: simplifica algunas instalaciones, pero impide conexiones remotas/híbridas ya aprobadas.
- Importar siempre a DuckLake: contradice ADR 0009 y duplica almacenes sin necesidad.
- Exportar cada dashboard como sitio estático: no resuelve consultas actualizadas, permisos por viewer ni disponibilidad del origen.
- Aplicación de escritorio como experiencia principal: no responde a la prioridad de navegador y complica la distribución antes de validar su necesidad.

## Referentes revisados y criterio propio

- **Rill v0.90.5:** [deploy.go](https://github.com/rilldata/rill/blob/v0.90.5/cli/cmd/project/deploy.go) y [canvas.go](https://github.com/rilldata/rill/blob/v0.90.5/runtime/reconcilers/canvas.go). Tomar recursos versionables, validación de dependencias y URL de acceso. No copiar dependencia de su nube, subida automática de variables de entorno ni publicación parcial de un borrador inválido.
- **Lightdash:** [compose leído](https://github.com/lightdash/lightdash/blob/168c3ffd53d7e7d1081b55b5c6a8de42e217a11c/docker-compose.yml). Tomar servidor web distribuido en contenedor y separación de servicios operativos. No copiar credenciales/permisos o imágenes latest de un ejemplo como configuración de producción.
- **Power BI:** [conexión de servicio cloud con datos internos](https://learn.microsoft.com/en-us/power-bi/connect-data/service-gateway-onprem). Tomar separación entre ubicación del servicio y de los datos. No construir un gateway propio en esta entrega.
- **Atlas BI:** documentos locales 12 (catálogo separado de autorización) y 22 (VisualSpec/contratos independientes de render y secretos). Mantener ese nivel de separación con implementación propia, sin adoptar su stack.
- **DuckLake:** [catálogo y almacenamiento separados](https://ducklake.select/docs/stable/duckdb/introduction). Fuente técnica para la composición del lago; no cambia versiones de dependencias del repositorio.

## Consecuencias y precedencia

Concreta ADR 0003/0004/0009 y la experiencia web de M17. Donde M08/M10 históricos presentan PostgreSQL BI como formato canónico exclusivo de dashboards, sigue prevaleciendo ADR 0003; las menciones concretas están en la tabla «Menciones obsoletas» de `docs/decisiones/README.md`. Ninguna parte de la arquitectura histórica se edita ni se cambia su inventario de huellas.

Esta es una decisión de dirección y límites. G3 especifica conectores, modos, publicación de datos y traslados; G4, contratos de dashboard, borrador/publicación y compartir; G6, distribución y operación verificadas. No adelanta su implementación ni altera el orden de los gates. Las metas de rendimiento, recuperación y experiencia se acordarán con datos de pruebas antes de habilitar cada capacidad.
