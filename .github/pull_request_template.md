Closes #

## Qué cambia

<!-- En palabras simples. -->

## Cómo se verificó

<!-- Comandos exactos y su resultado (pruebas, clippy, fmt). Pega la salida relevante, no solo "pasó". -->

## Riesgos abiertos

<!-- O "ninguno". -->

## Antes de pedir verificación a Codex

- [ ] Pasan localmente los mismos comandos que CI (`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` del área tocada)
- [ ] Cero decisiones fuera de lo que pedía el Issue (si tomé alguna, está explicada arriba y comentada en el Issue)
- [ ] Deja pruebas que protegen este cambio de una regresión futura
- [ ] Sin `TODO` ni código muerto
- [ ] `ESTADO.md` actualizado, si este PR cambia qué existe o qué falta — o no aplica
- [ ] Dependencia nueva: declarada arriba con paquete, licencia y motivo — o no aplica
