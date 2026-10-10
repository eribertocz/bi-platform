<script lang="ts">
  import type { Snippet } from 'svelte';
  import { page } from '$app/state';
  import { resolve } from '$app/paths';
  import SessionGate from '#lib/SessionGate.svelte';
  import '../app.css';

  let { children }: { children: Snippet } = $props();
</script>

<a class="skip-link" href="#contenido">Saltar al contenido</a>
<header class="border-b border-slate-200 bg-white">
  <div class="shell flex flex-wrap items-center justify-between gap-4 py-5">
    <a class="brand" href={resolve('/')}>bi-platform</a>
    <span class="text-sm text-slate-600">Plataforma de inteligencia de negocios</span>
  </div>
</header>
<div class="shell app-layout">
  <nav aria-label="Navegación principal">
    <ul class="nav-list">
      <li>
        <a href={resolve('/')} aria-current={page.url.pathname === '/' ? 'page' : undefined}
          >Inicio</a
        >
      </li>
      <li>
        <a
          href={resolve('/estado')}
          aria-current={page.url.pathname === '/estado' ? 'page' : undefined}>Estado del sistema</a
        >
      </li>
    </ul>
  </nav>
  <main id="contenido" tabindex="-1">
    <SessionGate pathname={page.url.pathname}>{@render children()}</SessionGate>
  </main>
</div>
<footer class="shell py-6 text-sm text-slate-600">bi-platform · Fundaciones</footer>
