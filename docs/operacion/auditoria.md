# Auditoría v1: almacenamiento y verificación (#71, primera entrega)

Esta entrega es la base de persistencia, **no la auditoría operativa completa**:
todavía no conecta los productores de sesiones, consultas y denegaciones, ni
expone el endpoint del auditor. #71 permanece abierto. No hay retención,
muestreo ni eliminación automática.

## Separación de privilegios

Ejecutar las migraciones con un propietario dedicado. El rol de ejecución no
puede ser propietario de las tablas/funciones, superusuario, ni miembro del rol
propietario. Estas condiciones son necesarias: un propietario puede eludir ACLs
y desactivar triggers. La separación de conexión de migraciones y ejecución del
servidor se completa al conectar los productores en la siguiente entrega.

El administrador concede únicamente estos permisos de auditoría a los roles ya
creados por su despliegue (los nombres son ejemplos):

```sql
GRANT USAGE ON SCHEMA bi_meta TO bi_app, bi_auditor;
GRANT EXECUTE ON FUNCTION bi_meta.audit_append(TEXT) TO bi_app;
GRANT SELECT ON bi_meta.audit_events, bi_meta.audit_head TO bi_auditor;
```

No conceder INSERT, UPDATE, DELETE, TRUNCATE, privilegios de propietario ni
acceso a `audit_head` al rol de escritura. La función `SECURITY DEFINER` tiene
`search_path` fijo y tablas calificadas; el rol de escritura sólo puede añadir
mediante esa función. El trigger también rechaza mutaciones accidentales del
propietario. Nunca publicar credenciales en comandos copiados a Issues/PRs.

## Escritura y contrato

`bi-audit::Event` es el contrato independiente de infraestructura. Los IDs son
referencias opacas del servidor; no emails, nombres ni tokens. `session_id` es
nulo cuando todavía no existe sesión; `policy_decision_id` puede ser nulo hasta
que exista el motor de permisos. Ambos campos siempre aparecen en el JSON.
`metadata_redacted` sólo admite conteos numéricos de la forma de consulta.

`bi_server::audit::append` exige una `Transaction<Postgres>` del llamante.
El evento y la acción de seguridad deben confirmarse juntos; ante cualquier
error el llamante debe revertir toda la transacción y devolver un fallo.
No hay reintentos implícitos: `event_id` es único y una respuesta ambigua requiere
reconciliar el commit antes de volver a intentar. `audit_append_pending` indica
escritura dentro de la transacción, **no confirmación** de ésta. Se registra
latencia y correlación por `request_id`/`trace_id`, sin volcar errores SQL.
`append_observed` admite un `AuditMetrics` construido con un `Meter` explícito
para OpenTelemetry: duración en segundos e intentos, con resultado acotado
(`pending`, `invalid`, `unavailable`), sin IDs ni payloads como etiquetas.

## Verificar

Configurar `BI_DATABASE_URL` con un rol lector y ejecutar:

```text
bi-server audit-verify
bi-server audit-verify SEQUENCE HASH
```

El comando no inicia HTTP, migraciones, DuckDB ni exportadores. Lee una instantánea
`REPEATABLE READ READ ONLY`, en páginas de 256 filas. Salida JSON v1: `sequence`
como texto decimal y `hash` como 64 dígitos hexadecimales minúsculos. Código de
salida 0: válido; 1: fallo de integridad/almacenamiento; 2: configuración/argumentos
inválidos. Los errores son códigos estables sin contenido de eventos.

La segunda forma exige que el checkpoint suministrado coincida con el prefijo
verificado; acepta eventos posteriores. Conservar checkpoints en un medio
externo de confianza y volver a pasarlos al verificar. Si un propietario hostil
reescribe **historia y cabecera juntas**, un hash sin firma/anclaje externo no
demuestra autenticidad. Un checkpoint sólo protege su prefijo; no protege filas
posteriores aún no ancladas. Tampoco demuestra que un productor emitió todos los
eventos que debía emitir ni autentica las afirmaciones de un escritor autorizado
comprometido, que puede inventar eventos nuevos aunque no pueda editar los previos.

Hash v1: SHA-256 de `bi-platform.audit.v1` + byte NUL + secuencia positiva
(8 bytes, big endian) + hash previo (32 bytes) + bytes UTF-8 del JSON compacto
del contrato v1. Génesis: 32 bytes cero. PostgreSQL conserva el texto original,
sin normalizarlo como JSONB; Rust verifica los mismos bytes. La fila de cabecera
serializa escritores hasta commit y permite detectar borrados del final.

## Pruebas y medida reproducible

Con PostgreSQL real y `DATABASE_URL` de pruebas (permiso para crear bases y roles):

```text
cargo test --locked -j 4 -p bi-audit
cargo test --locked -j 4 -p bi-server --all-features --test audit
cargo test --locked -j 4 -p bi-server --all-features --test audit audit_write_cost_baseline -- --nocapture
```

La medida compara una transacción PostgreSQL mínima con y sin evento, incluyendo
commit/WAL; 10 calentamientos y 100 muestras. Reporta p50/p95 sin umbrales de CI.
El coste sobre la consulta analítica real se medirá al conectar su productor.
