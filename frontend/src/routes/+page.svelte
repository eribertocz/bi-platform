<script lang="ts">
  import { resolve } from '$app/paths';
</script>

<svelte:head
  ><title>Inicio · bi-platform</title><meta
    name="description"
    content="Inicio de la plataforma BI."
  /></svelte:head
>
<h1>Inicio</h1>
<p class="mb-6 text-slate-700">Bienvenido a bi-platform.</p>
<section class="panel" aria-labelledby="operacion">
  <h2 id="operacion" class="mb-3">Conexión con la plataforma</h2>
  <p class="mb-4 text-slate-700">
    Consulta si la API responde y si sus dependencias están listas para servir.
  </p>
  <a class="font-semibold" href={resolve('/estado')}>Ver estado del sistema →</a>
</section>
