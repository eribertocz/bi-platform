<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { resolve } from '$app/paths';
  import { ApiError, getDevUsers, login, type User } from '#lib/api/session.ts';
  let users = $state<User[]>([]);
  let selected = $state('');
  let loading = $state(true);
  let busy = $state(false);
  let error = $state('');
  let disabled = $state(false);

  async function load(signal?: AbortSignal) {
    loading = true;
    error = '';
    try {
      const values = await getDevUsers(signal);
      if (signal?.aborted) return;
      users = values;
      selected = values[0]?.id ?? '';
    } catch (cause) {
      if (signal?.aborted) return;
      disabled = cause instanceof ApiError && cause.status === 404;
      if (!disabled) error = 'No se pudieron cargar los usuarios. Intenta de nuevo.';
    } finally {
      if (!signal?.aborted) loading = false;
    }
  }

  onMount(() => {
    const controller = new AbortController();
    void load(controller.signal);
    return () => controller.abort();
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    error = '';
    try {
      await login(selected);
      await goto(resolve('/'));
    } catch {
      error = 'No se pudo iniciar sesión. Intenta de nuevo.';
    } finally {
      busy = false;
    }
  }
</script>

<svelte:head><title>Iniciar sesión · bi-platform</title></svelte:head>
<h1>Iniciar sesión</h1>
<p class="mb-6">Este acceso de desarrollo permite explorar únicamente datos sintéticos.</p>
{#if disabled}
  <p role="status">El acceso de desarrollo está deshabilitado en este servidor.</p>
{:else if loading}
  <p role="status">Cargando usuarios…</p>
{:else if users.length > 0}
  <form onsubmit={(event) => void submit(event)} class="panel grid gap-4">
    <label for="usuario">Usuario de desarrollo</label>
    <select id="usuario" bind:value={selected} disabled={busy}>
      {#each users as user (user.id)}<option value={user.id}>{user.display_name}</option>{/each}
    </select>
    <button type="submit" disabled={busy || !selected}
      >{busy ? 'Iniciando sesión…' : 'Entrar'}</button
    >
  </form>
{/if}
{#if error}
  <p role="alert">{error}</p>
  <button onclick={() => void load()}>Reintentar</button>
{/if}
