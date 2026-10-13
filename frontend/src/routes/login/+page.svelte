<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { resolve } from '$app/paths';
  import {
    ApiError,
    getDevUsers,
    getOidcConfig,
    startCorporateLogin,
    login,
    type User,
  } from '#lib/api/session.ts';
  let users = $state<User[]>([]);
  let selected = $state('');
  let loading = $state(true);
  let busy = $state(false);
  let error = $state('');
  let disabled = $state(false);
  let corporate = $state(false);
  let corporateBusy = $state(false);
  let corporateError = $state('');

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
    void getOidcConfig(controller.signal)
      .then((enabled) => {
        if (!controller.signal.aborted) corporate = enabled;
      })
      .catch(() => {
        if (!controller.signal.aborted)
          corporateError =
            'No se pudo comprobar el acceso corporativo. Recarga la página para intentar de nuevo.';
      });
    return () => controller.abort();
  });

  async function corporateLogin() {
    corporateBusy = true;
    corporateError = '';
    try {
      await startCorporateLogin();
    } catch {
      corporateBusy = false;
      corporateError = 'No se pudo iniciar el acceso corporativo. Intenta de nuevo.';
    }
  }

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
{#if corporate}
  <p class="mb-6">Entra con tu cuenta de la empresa.</p>
  <button onclick={() => void corporateLogin()} disabled={corporateBusy}>
    {corporateBusy ? 'Abriendo acceso corporativo…' : 'Entrar con mi empresa'}
  </button>
{/if}
{#if corporateError}<p role="alert">{corporateError}</p>{/if}
{#if disabled}
  {#if !corporate}<p role="status">
      El acceso de desarrollo está deshabilitado en este servidor.
    </p>{/if}
{:else if loading}
  <p role="status">Cargando usuarios…</p>
{:else if users.length > 0}
  <p class="my-6">El acceso de desarrollo permite explorar únicamente datos sintéticos.</p>
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
