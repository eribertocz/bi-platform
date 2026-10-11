# 0010 — Punto único de aplicación de permisos; seguridad mínima en G2; WCAG 2.2 AA

**Estado:** aprobada por el propietario, 2026-10-11, sobre las propuestas A, B y C de [`docs/producto/blueprint-incorporacion.md`](../producto/blueprint-incorporacion.md) §7.
- **Amplía:** el gate G2 de `ROADMAP_GLOBAL.md` ("usuarios con permisos distintos no cruzan filas, caché ni exportaciones").
- **Concreta:** el blueprint del propietario, §4, §5.1, §14 y §15.

## Contexto
Hoy cualquier sesión puede consultar todas las filas del dataset sintético (`POST /api/v1/queries`) y ver los modelos (`GET /api/v1/models`). Es aceptable con datos de prueba. Con datos reales, cada vía nueva sería una puerta para saltarse los permisos: editor SQL, caché, exportación, alertas, *embedding* o IA. El blueprint pide demostrar que el usuario A no obtiene datos de B por ninguna vía.

## Decisión
1. **Punto único de aplicación (A).**
   - **Qué pasa por él:** todo acceso a datos pasa por el plan del motor semántico, con el contexto de seguridad de quien pregunta: identidad, grupos, atributos y valores de contexto. Ninguna vía ejecuta SQL que no haya generado ese plan, o que el plan no haya envuelto con las políticas del usuario.
   - **Restricciones por vía:**
     - el editor SQL compila al mismo plan (SQL sobre métricas);
     - la caché incluye en su clave la huella de la decisión de política;
     - la IA genera consultas semánticas, nunca SQL libre;
     - los valores de contexto se enlazan como parámetros, nunca como estado de sesión del motor.
   - **Qué pasa con una vía que no pueda cumplirlo:** se bloquea.
2. **Seguridad mínima adelantada a G2 (B).**
   - **Motor de permisos:** decisión `sujeto + acción + recurso + contexto → decisión + restricciones + motivo`, denegación por defecto, RBAC con atributos (ABAC). El motor concreto se elige en su propio ADR, tras leer el código de los candidatos (Cedar, OpenFGA, Casbin, OPA).
   - **Auditoría mínima:** eventos de inicio de sesión, denegaciones y consultas, con el esquema del blueprint §5.1, en una tabla de solo adición con encadenamiento por hash. Los eventos de seguridad nunca se muestrean.
   - **Login corporativo:** federación OIDC, ya prevista en `bi-server/src/auth.rs`.
   - **Para más adelante:** organizaciones y espacios de trabajo, que el propietario no adelanta a G2.
3. **Accesibilidad (C):** objetivo **WCAG 2.2 AA** en los flujos principales, verificado en cada pantalla además de axe.

## Consecuencias
- **Batería de aislamiento:** G2 se cierra con una batería "A no obtiene datos de B" que crece con cada vía (API, explorador, caché, exportación…) y forma parte de la conformidad.
- **Bloqueos hasta que exista el punto único:** el editor SQL, la caché de resultados y la exportación. `POST /api/v1/queries` se migra a la consulta semántica con políticas o se restringe a administradores.
- **Definición de terminado:** toda ruta nueva declara su acción y su recurso, y lleva pruebas negativas de autorización y, cuando corresponda, un evento de auditoría.
- **Coste:** G2 crece en tres piezas, a cambio de que su criterio de aprobación sea verificable y de no rehacer las vías de acceso después.
