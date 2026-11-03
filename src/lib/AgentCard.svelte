<script lang="ts">
  import {
    api,
    fmtDuration,
    fmtTokens,
    shortModel,
    type AgentActivity,
    type Item,
  } from './api';
  import { renderMarkdown } from './markdown';
  import Spinner from './Spinner.svelte';

  type AgentItem = Extract<Item, { kind: 'agent' }>;
  type AgentResult = Extract<Item, { kind: 'agentResult' }>;
  type TaskNotice = Extract<Item, { kind: 'taskNotice' }>;

  interface Props {
    /** Índice del item (para los botones de copiar del resultado). */
    i: number;
    item: AgentItem;
    result?: AgentResult;
    notice?: TaskNotice;
    /** Informe final ya resuelto (hand-back, aviso de fin o resultado síncrono). */
    report: string;
    /** La sesión sigue viva (si no, lo que no terminó ya no va a terminar). */
    live: boolean;
    onOpen: (toolUseId: string, title: string) => void;
  }
  let { i, item, result, notice, report, live, onOpen }: Props = $props();

  const FAILED = ['failed', 'error', 'killed', 'cancelled', 'stopped'];
  const finished = $derived(!!notice || (!!result && result.status !== 'async_launched'));
  const failed = $derived(
    (notice && FAILED.includes(notice.status)) ||
      !!result?.isError ||
      (!!result && FAILED.includes(result.status)),
  );
  const running = $derived(!finished && live);
  const model = $derived(shortModel(result?.model) || item.model);
  const stats = $derived(
    [
      fmtDuration(notice?.durationMs ?? result?.durationMs),
      (notice?.toolUses ?? result?.toolUses) != null
        ? `${notice?.toolUses ?? result?.toolUses} herramientas`
        : '',
      (notice?.tokens ?? result?.tokens) != null
        ? `${fmtTokens(notice?.tokens ?? result?.tokens)} tokens`
        : '',
    ]
      .filter(Boolean)
      .join(' · '),
  );
  const preview = $derived(
    report
      .replace(/[#*_`>|]/g, '')
      .split('\n')
      .find((l) => l.trim()) ?? '',
  );

  // Progreso en vivo, leído del fichero del subagente mientras trabaja.
  let activity = $state<AgentActivity | null>(null);
  $effect(() => {
    if (!running) return;
    const id = item.id;
    let stop = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const a = await api.agentActivity(id);
        if (!stop) activity = a;
      } catch {
        /* aún sin fichero */
      }
      if (!stop) timer = setTimeout(poll, 2500);
    };
    poll();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
  });
</script>

<div class="agent-card" class:running class:failed>
  <div class="agent-head">
    <span class="agent-ic" class:ok={finished && !failed} class:bad={failed}
      >{#if running}<Spinner size={13} />{:else if failed}✗{:else if finished}✓{:else}⏸{/if}</span
    >
    <span class="agent-title" title={item.description}>{item.description || 'Subagente'}</span>
    <span class="agent-meta"
      >{item.agentType || 'agente'}{model ? ' · ' + model : ''}{item.background
        ? ' · 2º plano'
        : ''}</span
    >
    <span class="agent-stats">
      {#if running}
        {activity ? `${activity.toolUses} herramientas…` : 'trabajando…'}
      {:else if !finished}
        sin terminar
      {:else if failed}
        {notice?.status || result?.status || 'error'}{stats ? ' · ' + stats : ''}
      {:else}
        {stats}
      {/if}
    </span>
    <button class="small" onclick={() => onOpen(item.id, item.description)}>Ver transcript</button>
  </div>

  {#if running && activity?.recent.length}
    <div class="agent-steps">
      {#each activity.recent as r, k (k)}
        <div class="agent-step" class:last={k === activity.recent.length - 1}>⚙ {r}</div>
      {/each}
    </div>
  {/if}

  <details class="agent-sec">
    <summary>Encargo</summary>
    <div class="md agent-prompt">{@html renderMarkdown(item.prompt)}</div>
  </details>

  {#if report}
    <details class="agent-sec">
      <summary>Resultado <span class="agent-preview">{preview}</span></summary>
      <div class="msg assistant" data-i={i}>
        <div class="msg-tools">
          <button data-act="rich" title="Copiar con formato (HTML + markdown)">Copiar</button>
          <button data-act="plain" title="Copiar sólo texto, sin formato">Texto</button>
          <button data-act="md" title="Copiar el markdown original">MD</button>
        </div>
        <div class="body md">{@html renderMarkdown(report)}</div>
      </div>
    </details>
  {/if}
</div>
