<script lang="ts">
  import type { Snippet } from 'svelte';
  import { goto } from '$app/navigation';
  import { resolve } from '$app/paths';
  import {
    ApiError,
    getSession,
    logout,
    setUnauthorizedHandler,
    type Session,
  } from '#lib/api/session.ts';

  let { pathname, children }: { pathname: string; children: Snippet } = $props();
  let session = $state<Session | null>(null);
  let checking = $state(true);
  let error = $state('');
  let busy = $state(false);
  let revision = $state(0);
  const publicPage = $derived(pathname === '/login' || pathname === '/estado');

  function unauthorized() {
    session = null;
    if (!publicPage) void goto(resolve('/login'));
  }

  $effect(() => {
    setUnauthorizedHandler(unauthorized);
    return () => setUnauthorizedHandler();
  });

  $effect(() => {
    // Revalidate on every navigation; a UI cache never grants access.
    void pathname;
    void revision;
    const controller = new AbortController();
    checking = true;
    error = '';
    void getSession(controller.signal)
      .then((value) => {
        if (!controller.signal.aborted) session = value;
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        session = null;
        if (cause instanceof ApiError && cause.status === 401) unauthorized();
        else error = 'No se pudo verificar la sesión. Intenta de nuevo.';
      })
      .finally(() => {
        if (!controller.signal.aborted) checking = false;
      });
    return () => controller.abort();
  });

  async function signOut() {
    busy = true;
    error = '';
    try {
      await logout();
      session = null;
      await goto(resolve('/login'));
    } catch (cause) {
      if (!(cause instanceof ApiError && cause.status === 401))
        error = 'No se pudo cerrar la sesión. Intenta de nuevo.';
    } finally {
      busy = false;
    }
  }
</script>

<div class="mb-6 flex flex-wrap items-center justify-between gap-3" aria-label="Sesión">
  {#if session}
    <span>Sesión: {session.user.display_name} · Datos sintéticos</span>
    <button onclick={() => void signOut()} disabled={busy}>Cerrar sesión</button>
  {:else}
    <span>{checking ? 'Verificando sesión…' : 'Sin sesión'}</span>
    {#if pathname !== '/login'}<a href={resolve('/login')}>Iniciar sesión</a>{/if}
  {/if}
</div>
{#if error}
  <p role="alert">{error}</p>
  <button onclick={() => revision++}>Reintentar sesión</button>
{/if}
{#if publicPage || (!checking && session)}
  {@render children()}
{:else if checking}
  <p role="status">Verificando acceso…</p>
{/if}
