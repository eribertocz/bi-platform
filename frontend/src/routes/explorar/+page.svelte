<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { listCatalog, type SemanticCatalog } from '#lib/api/semantic.ts';
  import {
    answer,
    askable,
    consistency,
    emptyTotalAllowed,
    failure,
    groupable,
    initialChoice,
    jointProblems,
    MANY_METRICS,
    MAX_DIMENSIONS,
    MAX_METRICS,
    questionProblems,
    reconcile,
    requests,
    withFilters,
    type Answer,
    type Choice,
    type Failure,
  } from '#lib/features/semantic/explore.ts';
  import { metricsDomain, rowsDomain } from '#lib/features/semantic/condition.ts';
  import {
    back,
    drill,
    literal,
    origin,
    returnTo,
    startPath,
    targets,
    type DrillPath,
    type Drilled,
    type Step,
  } from '#lib/features/semantic/drill.ts';
  import { shown, shownText, type SemanticBlock } from '#lib/features/semantic/exact.ts';
  import {
    dimensionId,
    NO_FILTERS,
    type SemanticValue,
    type UserFilters,
  } from '#lib/features/semantic/filters.ts';
  import FilterTreeEditor from '#lib/features/semantic/FilterTreeEditor.svelte';
  import { summarize, type TreeChange } from '#lib/features/semantic/filterTree.ts';
  import KpiStrip from '#lib/features/semantic/KpiStrip.svelte';
  import ReceiptPanel from '#lib/features/semantic/ReceiptPanel.svelte';
  import ResultChart from '#lib/features/semantic/ResultChart.svelte';
  import ResultTable from '#lib/features/semantic/ResultTable.svelte';

  let catalog = $state<
    | { kind: 'loading' }
    | { kind: 'ready'; value: SemanticCatalog }
    | { kind: 'error'; failure: Failure }
  >({ kind: 'loading' });
  let catalogRevision = $state(0);
  let choice = $state<Choice | null>(null);
  // The person's filters being edited, before and after aggregating, apart
  // from the applied ones: no edit asks anything; only «Aplicar filtros»
  // changes the question (#265, #149). `null`: that editor is untouched.
  let drafts = $state<{ where: TreeChange | null; having: TreeChange | null }>({
    where: null,
    having: null,
  });
  let editorRevision = $state(0);
  let filterNotice = $state<string | null>(null);
  let attempt = $state(0);
  let view = $state<
    | { kind: 'idle' }
    | { kind: 'loading' }
    | {
        kind: 'ready';
        /** The question this answer is of: a drill starts only from its cells. */
        asked: Choice;
        answer: Answer;
        labels: Record<string, string>;
        emptyAllowed: boolean;
      }
    | { kind: 'error'; failure: Failure }
  >({ kind: 'idle' });

  // The drill path lives in this page only: leaving it, as a sign-out or an
  // expired session does, drops it. A new path starts on another model, a
  // refreshed catalog or a question no longer available (#267).
  const instance = crypto.randomUUID();
  const pathFor = (model: string) => startPath({ principal: instance, model });
  let path = $state<DrillPath>(pathFor(''));
  /** The cell a drill is being chosen from, and where it may go. */
  let drilling = $state<{
    row: number;
    selection: string;
    from: string;
    shownValue: string;
    targets: string[];
    trigger: HTMLElement;
  } | null>(null);
  let drillTarget = $state('');
  let drillNotice = $state<string | null>(null);
  let drillAnnouncement = $state('');

  const model = $derived(
    catalog.kind === 'ready'
      ? catalog.value.models.find((m) => m.model === choice?.model)
      : undefined,
  );
  // The question as data: a change of selection or model is a new question.
  const question = $derived(askable(choice, model) ? JSON.stringify(choice) : null);
  const applied = $derived<UserFilters>(choice?.filters ?? NO_FILTERS);
  const appliedProblems = $derived(choice && model ? questionProblems(choice, model) : []);
  // Conditions on the rows' fields, and on the metrics this question selects.
  const rows = $derived(model ? rowsDomain(model) : null);
  const aggregates = $derived(model && choice ? metricsDomain(model, choice.metrics) : null);
  /** The filters `drafts` would apply: an untouched editor keeps what is applied. */
  const drafted = $derived<UserFilters>({
    where: drafts.where ? drafts.where.filter : applied.where,
    having: drafts.having ? drafts.having.filter : applied.having,
  });
  const pending = $derived(JSON.stringify(drafted) !== JSON.stringify(applied));
  const appliedSummary = $derived.by(() => {
    const parts = [
      applied.where && `antes de agregar, ${summarize(applied.where, rows ?? undefined)}`,
      applied.having && `después de agregar, ${summarize(applied.having, aggregates ?? undefined)}`,
    ].filter(Boolean);
    return parts.length > 0 ? `Filtros aplicados: ${parts.join('; ')}.` : 'Sin filtros aplicados.';
  });

  /**
   * Applies both drafts as one question when it can be sent, checked whole
   * against the metrics it selects now; otherwise says why, where.
   */
  async function applyFilters() {
    if (!choice || !model) return;
    const next = withFilters(choice, $state.snapshot(drafted) as UserFilters);
    if (questionProblems(next, model).length > 0) {
      // What no editor marks, such as the budget both share, is said here.
      const unmarked = jointProblems(next, model).map((p) => p.message);
      filterNotice = [
        'Corrige los errores marcados en los filtros antes de aplicarlos.',
        ...unmarked,
      ].join(' ');
      await tick();
      const invalid = document.querySelector<HTMLElement>('#filter-section [aria-invalid="true"]');
      (invalid ?? document.getElementById('filters-notice'))?.focus();
      return;
    }
    filterNotice = null;
    pick(next);
    drafts = { where: null, having: null };
    await tick();
    document.getElementById('filters-applied')?.focus();
  }

  /** Withdraws the person's filters only: the model's and the policies stay. */
  async function clearFilters() {
    if (!choice) return;
    filterNotice = null;
    drafts = { where: null, having: null };
    editorRevision++;
    pick(withFilters(choice, NO_FILTERS));
    await tick();
    document.getElementById('filters-add')?.focus();
  }

  // The catalog first, and again only on an explicit refresh: while it
  // loads, no question runs and the previous answer is withdrawn. The editor
  // starts again from the applied filters, so an unapplied draft goes too:
  // only the tree on screen, checked against this catalog, can be applied.
  $effect(() => {
    void catalogRevision;
    const controller = new AbortController();
    catalog = { kind: 'loading' };
    view = { kind: 'idle' };
    drafts = { where: null, having: null };
    filterNotice = null;
    drilling = null;
    drillNotice = null;
    void listCatalog(controller.signal)
      .then((value) => {
        if (controller.signal.aborted) return;
        catalog = { kind: 'ready', value };
        choice = reconcile($state.snapshot(choice), value);
        path = pathFor(choice?.model ?? '');
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted) catalog = { kind: 'error', failure: failure(error) };
      });
    return () => controller.abort();
  });

  // One attempt per question: replacing or leaving it aborts what is
  // pending, withdraws its answer and discards anything that arrives late.
  $effect(() => {
    void attempt;
    const asked = question;
    if (catalog.kind !== 'ready' || asked === null) {
      // A failure stays on screen until the person changes the question.
      if (untrack(() => view.kind) !== 'error') view = { kind: 'idle' };
      return;
    }
    const current: Choice = JSON.parse(asked);
    const controller = new AbortController();
    view = { kind: 'loading' };
    void answer(current, controller.signal)
      .then((value) => {
        if (!controller.signal.aborted)
          view = {
            kind: 'ready',
            asked: current,
            answer: value,
            labels: requests(current).labels,
            emptyAllowed: emptyTotalAllowed(current),
          };
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        const failed = failure(error);
        // No longer usable: the selection goes with its answer. The person's
        // filters stay, as the editors show them: none is dropped (#142).
        if (failed.gone) {
          choice = withFilters(
            { ...current, metrics: [], dimensions: [] },
            current.filters ?? NO_FILTERS,
          );
          // Its path may name what the session no longer sees.
          path = pathFor(current.model);
        }
        view = { kind: 'error', failure: failed };
      });
    return () => controller.abort();
  });

  function pick(next: Choice) {
    if (view.kind === 'error') view = { kind: 'idle' };
    drilling = null;
    choice = next;
  }

  /** A literal as the path names it: never a figure of an answer. */
  function valueText(value: SemanticValue | null) {
    if (value === null || value.type === 'null') return 'sin valor (nulo)';
    if (value.type === 'boolean') return value.value ? 'verdadero' : 'falso';
    return value.type === 'text' ? `«${value.value}»` : value.value;
  }
  const stepText = (step: Step) => `${step.from} = ${valueText(step.value)}, por ${step.to}`;

  /** Whether the cell may start a drill: a dimension the session sees and filters, a literal for its value. */
  function drillable(block: SemanticBlock, asked: Choice, row: number, column: number) {
    const declared = block.columns[column]!;
    const from = asked.dimensions.find((name) => dimensionId(name) === declared.selection_id);
    if (!model || from === undefined || declared.masked || !origin(asked, model, from))
      return false;
    if (!model.fields.find((f) => f.name === from)?.capabilities.filter) return false;
    const cell = block.rows[row]?.[column] ?? null;
    return (
      targets(asked, model, from).length > 0 && (cell === null || !!literal(declared.type, cell))
    );
  }

  /** Before navigating, the person's unapplied filters are theirs to resolve. */
  async function blockedByDraft() {
    if (!pending) return false;
    drillNotice =
      'Tienes cambios de filtros sin aplicar: aplícalos o límpialos antes de explorar o volver.';
    await tick();
    document.getElementById('drill-notice')?.focus();
    return true;
  }

  async function openDrill(row: number, selection: string, trigger: HTMLElement) {
    if (view.kind !== 'ready' || !model || (await blockedByDraft())) return;
    const block = view.answer.table?.result.blocks[0];
    const index = block?.columns.findIndex((c) => c.selection_id === selection) ?? -1;
    const from = view.asked.dimensions.find((name) => dimensionId(name) === selection);
    if (!block || index < 0 || from === undefined) return;
    const options = targets(view.asked, model, from);
    drillNotice = null;
    drilling = {
      row,
      selection,
      from,
      shownValue: shownText(shown(block.columns[index]!, block.rows[row]?.[index] ?? null)),
      targets: options,
      trigger,
    };
    drillTarget = options[0] ?? '';
    await tick();
    document.getElementById('drill-target')?.focus();
  }

  function cancelDrill() {
    const trigger = drilling?.trigger;
    drilling = null;
    trigger?.focus();
  }

  const REFUSED: Record<string, string> = {
    target: 'Ese campo no está disponible para desglosar.',
    no_frame: 'No hay un paso anterior.',
  };

  /** Takes a step or a return: the coordinator asks again with current permissions. */
  async function navigate(result: Drilled, said: string) {
    if (!result.ok) {
      drillNotice =
        result.reason === 'question'
          ? `Ese paso no se puede consultar: ${result.problems.map((p) => p.message).join(' ')}`
          : (REFUSED[result.reason] ??
            'La celda ya no corresponde a la pregunta en pantalla. Elige otra.');
      await tick();
      document.getElementById('drill-notice')?.focus();
      return;
    }
    drillNotice = null;
    path = result.path;
    // The editors start again from the filters the step or return applied,
    // and so do the drafts: nothing was pending, but an untouched-looking
    // draft equal to the former question would differ from this one.
    drafts = { where: null, having: null };
    filterNotice = null;
    editorRevision++;
    pick(result.question);
    drillAnnouncement = said;
    await tick();
    document.getElementById(path.frames.length > 0 ? 'drill-current' : 'explore-answer')?.focus();
  }

  async function goDrill() {
    if (!drilling || view.kind !== 'ready' || !model || !choice || (await blockedByDraft())) return;
    const block = view.answer.table?.result.blocks[0];
    if (!block) return;
    const { row, selection } = drilling;
    const result = drill(
      $state.snapshot(choice) as Choice,
      $state.snapshot(path) as DrillPath,
      { principal: instance, model: choice.model },
      model,
      { asked: view.asked, block, row, selection },
      drillTarget,
    );
    const step = result.ok ? result.path.frames.at(-1)!.step : null;
    await navigate(result, step ? `Explorando ${stepText(step)}.` : '');
  }

  async function goTo(index: number) {
    if (!choice || (await blockedByDraft())) return;
    const context = { principal: instance, model: choice.model };
    const snapshot = $state.snapshot(path) as DrillPath;
    const result = index < 0 ? back(snapshot, context) : returnTo(snapshot, context, index);
    await navigate(result, 'Volviste a un paso anterior del camino.');
  }
  /** Whether the change was taken; a refused one leaves the box as it was. */
  function toggle(list: 'metrics' | 'dimensions', name: string, on: boolean): boolean {
    if (!choice) return false;
    const names = choice[list].filter((n) => n !== name);
    // The same limits as the controls, held by the state itself.
    const limit = list === 'metrics' ? MAX_METRICS : MAX_DIMENSIONS;
    if (on && names.length >= limit) return false;
    pick({ ...choice, [list]: on ? [...names, name] : names });
    return true;
  }
</script>

<svelte:head><title>Exploración · bi-platform</title></svelte:head>
<h1>Exploración</h1>
<p class="mb-6 text-slate-700">
  Elige métricas y, si quieres, hasta {MAX_DIMENSIONS} campos para desglosarlas. Solo aparece lo que tu
  sesión puede consultar.
</p>

{#snippet problem(failed: Failure, title: string)}
  <div class="panel mb-6" role="alert">
    <h2>{title}</h2>
    <p>{failed.message}</p>
    {#if failed.status}<p>
        {failed.code ? `HTTP ${failed.status} · Código: ${failed.code}` : `HTTP ${failed.status}`}
      </p>{/if}
    {#if failed.requestId}<p>Solicitud: {failed.requestId}</p>{/if}
  </div>
{/snippet}

{#if catalog.kind === 'loading'}<p role="status">Cargando el catálogo…</p>
{:else if catalog.kind === 'error'}
  {@render problem(catalog.failure, 'No se pudo cargar el catálogo')}
  {#if catalog.failure.retry}<button onclick={() => catalogRevision++}>Reintentar</button>{/if}
{:else if catalog.value.status === 'not_configured'}
  <p role="status">Esta instancia todavía no tiene un proyecto configurado.</p>
{:else if catalog.value.models.length === 0}
  <p role="status">Tu sesión no tiene métricas disponibles para consultar.</p>
  <button onclick={() => catalogRevision++}>Actualizar catálogo</button>
{:else}
  <form class="panel mb-6 grid gap-4" onsubmit={(event) => event.preventDefault()}>
    <div class="flex flex-wrap items-center gap-3">
      <label for="model" class="font-semibold">Modelo</label>
      <select
        id="model"
        class="rounded border border-slate-400 bg-white p-2"
        value={choice?.model}
        onchange={(event) => {
          if (catalog.kind !== 'ready') return;
          const next = catalog.value.models.find((m) => m.model === event.currentTarget.value);
          if (next) {
            // Another model is another question: its filters start empty.
            drafts = { where: null, having: null };
            filterNotice = null;
            drillNotice = null;
            path = pathFor(next.model);
            pick(initialChoice({ status: 'ready', models: [next] })!);
          }
        }}
      >
        {#each catalog.value.models as option (option.model)}
          <option value={option.model}>{option.name} · {option.model}</option>
        {/each}
      </select>
      <button type="button" onclick={() => catalogRevision++}>Actualizar catálogo</button>
    </div>
    {#if choice && !model}
      <p class="text-sm text-red-800" role="alert">
        El modelo de esta pregunta ya no está en tu catálogo; sus filtros no se aplican a otro.
        Elige un modelo para empezar otra pregunta.
      </p>
    {/if}
    {#if model && choice}
      <fieldset aria-describedby="metrics-limit">
        <legend class="font-semibold">Métricas (1 a {MAX_METRICS})</legend>
        {#each model.metrics as metric (metric.name)}
          {@const on = choice.metrics.includes(metric.name)}
          <label class="mr-4 inline-flex items-center gap-2">
            <input
              type="checkbox"
              checked={on}
              disabled={metric.capabilities.select === 'masked' ||
                (!on && choice.metrics.length >= MAX_METRICS)}
              onchange={(event) => {
                const box = event.currentTarget;
                if (!toggle('metrics', metric.name, box.checked)) box.checked = !box.checked;
              }}
            />
            {metric.name}
            {#if metric.capabilities.select === 'masked'}<span class="text-sm text-slate-600"
                >(solo valores ocultos: todavía no se puede consultar)</span
              >{/if}
            {#if metric.lifecycle.state === 'deprecated'}<span class="text-sm text-slate-600"
                >(obsoleta)</span
              >{/if}
          </label>
        {/each}
      </fieldset>
      <div id="metrics-limit">
        {#if choice.metrics.length >= MAX_METRICS}
          <p class="text-sm text-slate-700">
            Máximo {MAX_METRICS} métricas: quita una para agregar otra.
          </p>
        {/if}
        <!-- Announced once when crossing the threshold: the region stays mounted. -->
        <div role="status">
          {#if choice.metrics.length >= MANY_METRICS}
            <p class="text-sm text-slate-700">
              Muchas métricas pueden dificultar la lectura. Puedes quitar algunas para comparar
              mejor.
            </p>
          {/if}
        </div>
      </div>
      <fieldset>
        <legend class="font-semibold">Desglosar por (0 a {MAX_DIMENSIONS})</legend>
        {#each groupable(model) as field (field.name)}
          {@const on = choice.dimensions.includes(field.name)}
          <label class="mr-4 inline-flex items-center gap-2">
            <input
              type="checkbox"
              checked={on}
              disabled={!on && choice.dimensions.length >= MAX_DIMENSIONS}
              onchange={(event) => {
                const box = event.currentTarget;
                if (!toggle('dimensions', field.name, box.checked)) box.checked = !box.checked;
              }}
            />
            {field.name}
          </label>
        {:else}<p class="text-sm text-slate-600">Este modelo no tiene campos para desglosar.</p>
        {/each}
      </fieldset>
      <section id="filter-section" class="grid gap-3" aria-labelledby="filters-title">
        <h2 id="filters-title" class="font-semibold">Filtros</h2>
        <p class="text-sm text-slate-700">
          Se suman a tus permisos y a los filtros del modelo, que no se cambian aquí.
        </p>
        {#key `${choice.model}:${editorRevision}`}
          <section class="grid gap-3" aria-labelledby="filters-rows-title">
            <h3 id="filters-rows-title" class="font-semibold">Filtros de filas</h3>
            <p class="text-sm text-slate-700">
              Se aplican antes de agregar: deciden qué filas entran en cada cálculo.
            </p>
            <FilterTreeEditor
              domain={rows!}
              initial={applied.where}
              id="filters"
              onchange={(change) => {
                drafts.where = change;
                filterNotice = null;
              }}
            />
          </section>
          <section class="grid gap-3" aria-labelledby="filters-metrics-title">
            <h3 id="filters-metrics-title" class="font-semibold">Filtros de métricas</h3>
            <p class="text-sm text-slate-700">
              Se aplican después de agregar, sobre las métricas seleccionadas, al total y a cada
              fila del desglose por separado: el total puede quedar excluido aunque haya filas.
            </p>
            <FilterTreeEditor
              domain={aggregates!}
              initial={applied.having}
              id="having"
              onchange={(change) => {
                drafts.having = change;
                filterNotice = null;
              }}
            />
          </section>
        {/key}
        <div class="flex flex-wrap gap-2">
          <button type="button" onclick={applyFilters} disabled={!pending}>Aplicar filtros</button>
          <button
            type="button"
            onclick={clearFilters}
            disabled={!applied.where && !applied.having && !drafts.where && !drafts.having}
          >
            Limpiar filtros
          </button>
        </div>
        {#if filterNotice}
          <p id="filters-notice" class="text-sm text-red-800" role="alert" tabindex="-1">
            {filterNotice}
          </p>
        {/if}
        {#if pending}<p class="text-sm text-slate-700">Hay cambios sin aplicar.</p>{/if}
        <p id="filters-applied" class="text-sm" tabindex="-1">
          {appliedSummary}
        </p>
        {#if appliedProblems.length > 0}
          <p class="text-sm text-red-800" role="alert">
            Los filtros aplicados ya no se pueden consultar: {appliedProblems
              .map((p) => p.message)
              .join(' ')} Corrígelos o límpialos.
          </p>
        {/if}
      </section>
    {/if}
  </form>

  {#if path.frames.length > 0}
    <nav class="panel mb-6 grid gap-2" aria-label="Camino de exploración">
      <ol class="flex flex-wrap items-center gap-2">
        {#each path.frames as frame, i (i)}
          <li>
            <button type="button" onclick={() => void goTo(i)}>
              {i === 0
                ? `Inicio: por ${frame.question.dimensions.join(' y ')}`
                : stepText(path.frames[i - 1]!.step)}
            </button>
            <span aria-hidden="true">›</span>
          </li>
        {/each}
        <li>
          <span id="drill-current" aria-current="step" tabindex="-1"
            >{stepText(path.frames.at(-1)!.step)}</span
          >
        </li>
      </ol>
      <button type="button" class="justify-self-start" onclick={() => void goTo(-1)}>Atrás</button>
    </nav>
  {/if}
  {#if drillNotice}
    <p id="drill-notice" class="mb-6 text-sm text-red-800" role="alert" tabindex="-1">
      {drillNotice}
    </p>
  {/if}
  <p class="sr-only" role="status">{drillAnnouncement}</p>

  <div id="explore-answer" tabindex="-1" aria-busy={view.kind === 'loading'} aria-live="polite">
    {#if view.kind === 'idle'}
      {#if !choice || choice.metrics.length === 0}<p role="status">
          Elige al menos una métrica.
        </p>{/if}
    {:else if view.kind === 'loading'}<p role="status">Consultando…</p>
    {:else if view.kind === 'error'}
      {@render problem(view.failure, 'No se pudo responder la consulta')}
      {#if view.failure.retry}<button onclick={() => attempt++}>Reintentar</button>{/if}
      {#if view.failure.gone}<button onclick={() => catalogRevision++}>Actualizar catálogo</button
        >{/if}
    {:else}
      {@const result = view.answer}
      <section class="mb-6" aria-labelledby="totals-title">
        <h2 id="totals-title">Totales</h2>
        <p class="mb-3 text-sm text-slate-700">
          Calculados por el servidor sobre todos los datos que puedes ver, sin desglose; no son la
          suma de las filas de la tabla.
        </p>
        <KpiStrip run={result.kpi} labels={view.labels} emptyAllowed={view.emptyAllowed} />
      </section>
      {#if result.table}
        {@const block = result.table.result.blocks[0]!}
        {@const asked = view.asked}
        {#if consistency(result) === 'diverged'}
          <p class="panel mb-6" role="note">
            Los totales y el desglose leyeron versiones distintas de los datos, del modelo o de las
            políticas: pueden no coincidir.
          </p>
        {:else}
          <p class="mb-6 text-sm text-slate-700" role="note">
            Los totales y el desglose son dos lecturas, cada una con su recibo.
          </p>
        {/if}
        <section class="panel mb-6" aria-labelledby="chart-title">
          <h2 id="chart-title">Gráfico</h2>
          <ResultChart {block} labels={view.labels} />
        </section>
        <section class="panel mb-6" aria-label="Desglose">
          {#if drilling}
            <section class="mb-4 grid gap-2" aria-labelledby="drill-title">
              <h3 id="drill-title" class="font-semibold">
                Explorar {drilling.from} = {drilling.shownValue}
              </h3>
              <div class="flex flex-wrap items-center gap-2">
                <label for="drill-target">Desglosar por</label>
                <select
                  id="drill-target"
                  class="rounded border border-slate-400 bg-white p-2"
                  bind:value={drillTarget}
                >
                  {#each drilling.targets as name (name)}<option value={name}>{name}</option>{/each}
                </select>
                <button type="button" onclick={() => void goDrill()}>Explorar</button>
                <button type="button" onclick={cancelDrill}>Cancelar</button>
              </div>
            </section>
          {/if}
          <ResultTable
            {block}
            caption="Desglose"
            labels={view.labels}
            drillable={(row, column) => drillable(block, asked, row, column)}
            ondrill={(row, selection, trigger) => void openDrill(row, selection, trigger)}
          />
        </section>
        <ReceiptPanel
          receipt={result.table.receipt}
          notices={result.table.notices}
          title="Recibo del desglose"
        />
      {/if}
      <div class="mt-6">
        <ReceiptPanel
          receipt={result.kpi.receipt}
          notices={result.kpi.notices}
          title="Recibo de los totales"
        />
      </div>
    {/if}
  </div>
{/if}
