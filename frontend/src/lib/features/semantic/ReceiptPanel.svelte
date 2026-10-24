<script lang="ts">
  import { instantText, type SemanticNotice, type SemanticReceipt } from './exact';
  let { receipt, notices }: { receipt: SemanticReceipt; notices: SemanticNotice[] } = $props();
  const segment = (id: string) => id.split(/[/:]/).at(-1) ?? id;
</script>

<section class="panel" aria-labelledby="receipt-title">
  <h2 id="receipt-title">Recibo de la respuesta</h2>
  {#if notices.length > 0}
    <ul class="mb-4" aria-label="Avisos">
      {#each notices as notice (notice.metric)}
        <li role="note">
          La métrica <strong>{segment(notice.metric)}</strong>
          {notice.sunset ? `está obsoleta y se retira el ${notice.sunset}.` : 'está obsoleta.'}
          {#if notice.replacement}Usa <strong>{segment(notice.replacement)}</strong>.{/if}
        </li>
      {/each}
    </ul>
  {/if}
  <dl>
    <dt>Lectura</dt>
    <dd>
      De {instantText(receipt.read_started_at)} a {instantText(receipt.read_finished_at)}: el
      momento de esta consulta, no de la última actualización de sus fuentes.
    </dd>
    <dt>Fuentes leídas que puedes ver</dt>
    <dd>
      {#if receipt.sources.length === 0}Ninguna visible para tu sesión.{:else}
        <ul>
          {#each receipt.sources as source (source.dataset)}
            <li data-testid="receipt-source">
              <strong>{segment(source.dataset)}</strong>:
              {#if source.data_snapshot !== null}versión {source.data_snapshot} del lago{:else}sin
                versión persistente: no se puede volver a leer igual{/if};
              {#if source.source_refreshed_at !== null}actualizada el {instantText(
                  source.source_refreshed_at,
                )}{:else}frescura desconocida{/if}.
            </li>
          {/each}
        </ul>
      {/if}
    </dd>
    <dt>Definiciones usadas que puedes ver</dt>
    <dd>
      {receipt.definitions.length}
      {receipt.definitions.length === 1 ? 'definición' : 'definiciones'}; el resto de la evidencia
      queda comprometida sin mostrarse.
    </dd>
    <dt>Identificador</dt>
    <dd><code>{receipt.id}</code></dd>
  </dl>
  <details class="mt-4">
    <summary>Detalle técnico</summary>
    <dl class="break-all">
      <dt>Consulta canónica ({receipt.query.encoding})</dt>
      <dd><code>{receipt.query.sqir}</code></dd>
      <dt>Huella de la consulta</dt>
      <dd><code>{receipt.fingerprints.query}</code></dd>
      <dt>Huellas protegidas</dt>
      <dd>
        <ul>
          <li>Modelo: <code>{receipt.fingerprints.model}</code></li>
          <li>Plan: <code>{receipt.fingerprints.plan}</code></li>
          <li>Decisión: <code>{receipt.fingerprints.decision}</code></li>
          <li>Políticas: <code>{receipt.fingerprints.policies}</code></li>
          <li>Contexto: <code>{receipt.fingerprints.context}</code></li>
          <li>Evidencia: <code>{receipt.evidence}</code></li>
        </ul>
      </dd>
      <dt>Definiciones</dt>
      <dd>
        <ul>
          {#each receipt.definitions as definition (definition.id)}
            <li>{segment(definition.id)}: <code>{definition.content}</code></li>
          {/each}
        </ul>
      </dd>
      <dt>Versiones</dt>
      <dd>
        Recibo {receipt.version}; motor {receipt.versions.engine}; adaptador
        {receipt.versions.adapter}; perfil {receipt.versions.capability_profile}; zonas horarias
        {receipt.versions.tzdb}.
      </dd>
    </dl>
  </details>
</section>
