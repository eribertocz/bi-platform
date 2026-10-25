<script lang="ts">
  import type { CatalogModel } from '#lib/api/semantic.ts';
  import {
    arity,
    condition,
    conditionProblems,
    filterable,
    operators,
    type ConditionChange,
    type Draft,
    type Operator,
  } from './condition';

  let {
    model,
    initial,
    id,
    onchange,
    onremove,
  }: {
    model: CatalogModel;
    initial: Draft;
    /** Unique prefix of this editor's element ids. */
    id: string;
    onchange: (change: ConditionChange) => void;
    onremove: () => void;
  } = $props();

  // The editor owns its draft; it never asks the API nor runs a query.
  // svelte-ignore state_referenced_locally
  // A plain copy: `initial` may be another component's reactive state.
  let draft = $state<Draft>($state.snapshot(initial) as Draft);

  const fields = $derived(filterable(model));
  const member = $derived(fields.find((f) => f.name === draft.field));
  const allowed = $derived(member ? operators(member) : []);
  const written = $derived(condition(draft, model));
  const problems = $derived(conditionProblems(written, model));
  const count = $derived(arity(draft.operator, draft.values));

  const LABELS: Record<Operator, string> = {
    '=': 'es igual a',
    '<>': 'es distinto de',
    '<': 'es menor que',
    '<=': 'es menor o igual que',
    '>': 'es mayor que',
    '>=': 'es mayor o igual que',
    between: 'está entre',
    in: 'es uno de',
    is_null: 'no tiene valor (nulo)',
    is_not_null: 'tiene valor (no nulo)',
  };

  /** The problems shown beside one control, by their path in the condition. */
  function at(...paths: string[]) {
    return problems.filter((p) => paths.includes(p.path)).map((p) => p.message);
  }
  const valuePaths = (i: number) =>
    draft.operator === 'between'
      ? [i === 0 ? '/filter/low' : '/filter/high']
      : draft.operator === 'in'
        ? [`/filter/values/${i}`]
        : ['/filter/value'];
  const general = $derived(at('/filter', '/filter/values', '/filter/comparison'));

  function changed() {
    const snapshot = $state.snapshot(draft);
    onchange({
      draft: snapshot,
      condition: condition(snapshot, model),
      problems: conditionProblems(condition(snapshot, model), model),
    });
  }

  function setValue(i: number, text: string) {
    const values = [...draft.values];
    while (values.length <= i) values.push('');
    values[i] = text;
    draft.values = values;
    changed();
  }

  function addValue() {
    draft.values = [...draft.values.slice(0, count), ''];
    changed();
  }

  function removeValue(i: number) {
    draft.values = draft.values.filter((_, j) => j !== i);
    changed();
  }

  const placeholder = $derived(
    member?.type === 'DATE'
      ? 'AAAA-MM-DD'
      : member?.type === 'INT64' || member?.type.startsWith('DECIMAL(')
        ? 'Número exacto, con punto decimal'
        : undefined,
  );
</script>

<fieldset class="grid gap-2 rounded border border-slate-300 p-3">
  <legend class="font-semibold">Condición</legend>
  <div class="flex flex-wrap items-center gap-3">
    <label for="{id}-field">Campo</label>
    <select
      id="{id}-field"
      class="rounded border border-slate-400 bg-white p-2"
      bind:value={draft.field}
      onchange={changed}
      aria-invalid={at('/filter/target').length > 0 ? 'true' : undefined}
      aria-describedby={at('/filter/target').length > 0 ? `${id}-field-error` : undefined}
    >
      {#if !member}
        <option value={draft.field} disabled
          >{draft.field || 'Elige un campo'} (no disponible)</option
        >
      {/if}
      {#each fields as field (field.name)}
        <option value={field.name}>{field.name}</option>
      {/each}
    </select>
    <label for="{id}-operator">Operador</label>
    <select
      id="{id}-operator"
      class="rounded border border-slate-400 bg-white p-2"
      bind:value={draft.operator}
      onchange={changed}
      aria-invalid={member && !allowed.includes(draft.operator) ? 'true' : undefined}
      aria-describedby={member && !allowed.includes(draft.operator)
        ? `${id}-operator-error`
        : undefined}
    >
      {#if !allowed.includes(draft.operator)}
        <option value={draft.operator} disabled>{LABELS[draft.operator]} (no admitido)</option>
      {/if}
      {#each allowed as operator (operator)}
        <option value={operator}>{LABELS[operator]}</option>
      {/each}
    </select>
  </div>
  {#if at('/filter/target').length > 0}
    <p id="{id}-field-error" class="text-sm text-red-800">{at('/filter/target').join(' ')}</p>
  {/if}
  {#if member && !allowed.includes(draft.operator)}
    <p id="{id}-operator-error" class="text-sm text-red-800">
      Este operador no se admite para el tipo del campo; elige otro.
    </p>
  {/if}

  {#each Array.from({ length: count }, (_, i) => i) as i (i)}
    {@const errors = at(...valuePaths(i))}
    <div class="flex flex-wrap items-center gap-2">
      <label for="{id}-value-{i}">
        {draft.operator === 'between'
          ? i === 0
            ? 'Desde'
            : 'Hasta'
          : `Valor${count > 1 ? ` ${i + 1}` : ''}`}
      </label>
      {#if member?.type === 'BOOLEAN'}
        <select
          id="{id}-value-{i}"
          class="rounded border border-slate-400 bg-white p-2"
          value={draft.values[i] ?? ''}
          onchange={(event) => setValue(i, event.currentTarget.value)}
          aria-invalid={errors.length > 0 ? 'true' : undefined}
          aria-describedby={errors.length > 0 ? `${id}-value-${i}-error` : undefined}
        >
          {#if draft.values[i] !== 'true' && draft.values[i] !== 'false'}
            <option value={draft.values[i] ?? ''} disabled>Elige</option>
          {/if}
          <option value="true">Verdadero</option>
          <option value="false">Falso</option>
        </select>
      {:else}
        <input
          id="{id}-value-{i}"
          class="rounded border border-slate-400 p-2"
          type="text"
          inputmode={placeholder === 'AAAA-MM-DD' ? 'numeric' : placeholder ? 'decimal' : 'text'}
          {placeholder}
          value={draft.values[i] ?? ''}
          oninput={(event) => setValue(i, event.currentTarget.value)}
          aria-invalid={errors.length > 0 ? 'true' : undefined}
          aria-describedby={errors.length > 0 ? `${id}-value-${i}-error` : undefined}
        />
      {/if}
      {#if draft.operator === 'in' && count > 1}
        <button type="button" onclick={() => removeValue(i)}>Quitar valor {i + 1}</button>
      {/if}
    </div>
    {#if errors.length > 0}
      <p id="{id}-value-{i}-error" class="text-sm text-red-800">{errors.join(' ')}</p>
    {/if}
  {/each}
  {#if draft.operator === 'in'}
    <button type="button" class="justify-self-start" onclick={addValue}>Añadir valor</button>
  {/if}
  {#if general.length > 0}
    <p class="text-sm text-red-800" role="status">{general.join(' ')}</p>
  {/if}
  <button type="button" class="justify-self-start" onclick={onremove}>Quitar condición</button>
</fieldset>
