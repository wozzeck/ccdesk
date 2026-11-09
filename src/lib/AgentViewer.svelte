<script lang="ts">
  // Panel lateral con la conversación completa de un subagente. Pulsar fuera lo contrae a su
  // pestaña (las pinta Conversation) y pulsarla lo vuelve a abrir como estaba: contraído sigue
  // montado, con lo cargado y el punto de lectura.
  import { onMount, tick } from 'svelte';
  import { api, errText, type Item } from './api';
  import Transcript from './Transcript.svelte';

  interface Props {
    toolUseId: string;
    title: string;
    live: boolean;
    /** Contraído: sólo se ve su pestaña. */
    collapsed: boolean;
    onClose: () => void;
    onToast: (m: string) => void;
    onOpenAgent: (toolUseId: string, title: string) => void;
  }
  let {
    toolUseId,
    title,
    live,
    collapsed = $bindable(false),
    onClose,
    onToast,
    onOpenAgent,
  }: Props = $props();

  let items = $state<Item[]>([]);
  let loaded = $state(false);
  let scroller: HTMLDivElement;
  let panel: HTMLElement;
  let stuck = true;

  function atBottom() {
    return scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 60;
  }

  async function load(id: string) {
    try {
      const r = await api.agentTranscript(id);
      if (id !== toolUseId) return;
      if (loaded && r.length === items.length) return;
      items = r;
      loaded = true;
      if (stuck) {
        await tick();
        scroller.scrollTop = scroller.scrollHeight;
      }
    } catch (e) {
      onToast(errText(e));
      onClose();
    }
  }

  $effect(() => {
    const id = toolUseId;
    loaded = false;
    stuck = true;
    items = [];
    load(id);
    if (!live) return;
    const t = setInterval(() => load(id), 2500);
    return () => clearInterval(t);
  });

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      // Contraído, el Esc es de la conversación (interrumpir a Claude). El de la barra de Ctrl+F la
      // cierra a ella, no al panel.
      if (collapsed || e.key !== 'Escape' || (e.target as Element | null)?.closest?.('.findbar')) return;
      e.stopPropagation();
      onClose();
    };
    // Pulsar fuera lo contrae. Al pulsar, no al soltar: así no lo contrae seleccionar texto
    // arrastrando hasta fuera. No cuentan la barra de Ctrl+F, que convive con el panel, ni un modal
    // abierto encima.
    const onDown = (e: PointerEvent) => {
      const t = e.target as Element | null;
      if (collapsed || e.button !== 0 || !t?.closest || panel.contains(t) || t.closest('.findbar, .modal-bg'))
        return;
      collapsed = true;
    };
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('pointerdown', onDown, true);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('pointerdown', onDown, true);
    };
  });

  const working = $derived(
    live && loaded && !items.some((it) => it.kind === 'assistant') ? true : false,
  );
</script>

<aside class="agent-viewer" class:collapsed bind:this={panel}>
  <header>
    <span class="av-kind">Subagente</span>
    <span class="av-title" title={title}>{title}</span>
    <button class="icon" title="Cerrar (Esc)" onclick={onClose}>✕</button>
  </header>
  <div class="av-body" bind:this={scroller} onscroll={() => (stuck = atBottom())}>
    {#if !loaded}
      <div class="empty">Cargando…</div>
    {:else if !items.length}
      <div class="empty">{working ? 'Arrancando…' : 'Sin actividad registrada.'}</div>
    {:else}
      <Transcript
        {items}
        startIndex={0}
        busy={live}
        {live}
        {onToast}
        {onOpenAgent}
      />
    {/if}
  </div>
</aside>
