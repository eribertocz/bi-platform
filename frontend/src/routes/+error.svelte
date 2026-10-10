<script lang="ts">
  import { page } from '$app/state';
  import { resolve } from '$app/paths';
</script>

<svelte:head><title>{page.status} · bi-platform</title></svelte:head>
<h1>{page.status === 404 ? 'Página no encontrada' : 'No se pudo abrir la página'}</h1>
<p class="mb-6 text-slate-700">
  {page.status === 404
    ? 'La dirección solicitada no existe.'
    : 'Intenta volver al inicio y abrir la página de nuevo.'}
</p>
<a href={resolve('/')}>Volver al inicio</a>
