<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { api, errText, type Item, type SessionView, type TranscriptAppend } from './api';
  import Spinner from './Spinner.svelte';
  import Transcript from './Transcript.svelte';
  import AgentViewer from './AgentViewer.svelte';
  import FindBar from './FindBar.svelte';
  import { outbox } from './outbox.svelte';

  interface Props {
    session: SessionView;
    onToast: (m: string) => void;
    /** Abrir el transcript de un subagente pedido desde fuera (árbol del panel izquierdo). */
    openAgent?: { key: string; id: string; title: string; n: number } | null;
    /** Ir a la coincidencia de un resultado de la búsqueda de sesiones (panel izquierdo). */
    findReq?: { key: string; text: string; n: number } | null;
  }
  let { session, onToast, openAgent = null, findReq = null }: Props = $props();

  let items = $state<Item[]>([]);
  let startIndex = $state(0);
  let loading = $state(false);
  let empty = $state(false);
  // Con algo ya enviado, el aviso de conversación vacía (que va bajado, a media pantalla) se quita
  // al momento: si no, la burbuja provisional se pondría debajo de él, volaría hasta allí y subiría
  // de golpe al llegar el mensaje real.
  const sending = $derived(outbox.items.some((o) => o.key === session.key));
  let path = '';
  let loadedId = '';
  /** Sesión cuya conversación está pintada (la de `items`). */
  let shownKey = $state('');
  let scroller: HTMLDivElement;
  let inner: HTMLDivElement;

  $effect(() => {
    const id = session.key + '|' + session.sid;
    if (id !== loadedId) {
      loadedId = id;
      load(session.key);
    }
  });

  async function load(key: string) {
    loading = true;
    try {
      const r = await api.transcriptOpen(key);
      if (key !== session.key) return;
      items = r?.items ?? [];
      startIndex = r?.startIndex ?? 0;
      path = r?.path ?? '';
      empty = !r || r.total === 0;
      await tick();
      stopGlide();
      toBottom();
      stuck = true;
      shownKey = key;
      finder?.reloaded();
    } catch (e) {
      onToast(errText(e));
    } finally {
      loading = false;
    }
  }

  function atBottom(): boolean {
    return !scroller || scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 60;
  }
  function toBottom() {
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }

  // Si estaba abajo del todo, seguir abajo cuando cambie el alto (p. ej. al abrirse la terminal).
  let stuck = true;
  function onScroll() {
    if (!gliding) stuck = atBottom();
  }

  // Lo nuevo entra deslizándose: si la vista estaba abajo, se acerca al final con una curva
  // suave en lugar de saltar. Cada fotograma apunta al final de ese momento, así que lo que llegue
  // a mitad (más texto, una burbuja que crece) se suma al mismo movimiento.
  let gliding = false;
  let glideFrame = 0;
  function glideToBottom() {
    if (!scroller || gliding) return;
    gliding = true;
    let pos = scroller.scrollTop;
    let last = performance.now();
    const step = (now: number) => {
      if (!gliding || !scroller) return;
      if (scroller.dataset.flying) {
        gliding = false; // la animación de envío lleva el desplazamiento
        return;
      }
      const target = scroller.scrollHeight - scroller.clientHeight;
      const d = target - pos;
      if (d <= 0.5) {
        scroller.scrollTop = target;
        gliding = false;
        stuck = true;
        return;
      }
      const dt = Math.min(64, now - last);
      last = now;
      pos += Math.max(0.5, d * (1 - Math.exp(-dt / 80)));
      scroller.scrollTop = pos;
      glideFrame = requestAnimationFrame(step);
    };
    glideFrame = requestAnimationFrame(step);
  }
  function stopGlide() {
    if (!gliding) return;
    gliding = false;
    cancelAnimationFrame(glideFrame);
  }
  // El usuario manda: si sube (rueda, barra, teclado) mientras se desliza, se para ahí.
  function userScroll(e: Event) {
    if (!gliding) return;
    if (e instanceof WheelEvent && e.deltaY >= 0) return;
    if (e instanceof KeyboardEvent && !['PageUp', 'ArrowUp', 'Home'].includes(e.key)) return;
    stopGlide();
    stuck = false;
  }

  onMount(() => {
    // Vigila el contenedor y también el contenido: cualquier crecimiento (mensajes nuevos,
    // herramientas, bloques que se despliegan) mantiene la vista pegada abajo si lo estaba.
    // El callback corre tras el layout y antes de pintar, así que no se ve el salto.
    let lastH = scroller.clientHeight;
    const ro = new ResizeObserver((entries) => {
      const h = scroller.clientHeight;
      const shrank = h < lastH;
      lastH = h;
      // Durante la animación de envío el desplazamiento lo lleva ella (sin saltos).
      if (!stuck || scroller.dataset.flying) return;
      // Si el hueco crece (ventana, terminal que se cierra) se ajusta al momento: no hay nada que
      // deslizar. Si encoge (la caja de texto gana líneas, se abre la terminal) o crece el
      // contenido, el final de la conversación sube deslizándose.
      if (entries.some((en) => en.target === scroller) && !shrank) {
        stopGlide();
        toBottom();
      } else glideToBottom();
    });
    ro.observe(scroller);
    ro.observe(inner);
    const uns = [
      listen<TranscriptAppend>('transcript-append', async (e) => {
        const p = e.payload;
        if (p.key !== session.key || p.path !== path) return;
        if (p.startIndex !== startIndex + items.length) {
          load(session.key);
          return;
        }
        const stick = stuck || atBottom();
        items.push(...p.items);
        empty = false;
        if (stick && !scroller.dataset.flying) {
          stuck = true;
          await tick();
          glideToBottom();
        }
      }),
      listen<string>('transcript-available', (e) => {
        if (e.payload === session.key) load(session.key);
      }),
    ];
    return () => {
      ro.disconnect();
      uns.forEach((u) => u.then((f) => f()));
    };
  });

  /** Añade delante los `count` items anteriores a lo cargado, sin mover lo que se ve. */
  async function prepend(count: number) {
    const key = session.key;
    const older = await api.transcriptBefore(key, startIndex, count);
    if (key !== session.key || !older.length) return;
    const h0 = scroller.scrollHeight;
    items = [...older, ...items];
    startIndex -= older.length;
    await tick();
    scroller.scrollTop += scroller.scrollHeight - h0;
  }
  const loadMore = () => prepend(300);

  // ── Ctrl+F: barra de búsqueda en la conversación (FindBar) ──
  let finding = $state<{ root: HTMLElement; scroller: HTMLElement; initial: string; startAt?: number | null } | null>(
    null,
  );
  let finder = $state<ReturnType<typeof FindBar>>();
  /** Lo seleccionado en la conversación, si es una línea corta (para buscarlo). */
  function selected(): string {
    const s = window.getSelection();
    const t = s?.toString().trim() ?? '';
    return t && !t.includes('\n') && t.length <= 200 && s?.anchorNode && inner.contains(s.anchorNode) ? t : '';
  }
  /** Abre la barra (o vuelve a ella) con lo seleccionado o, si no, con lo último buscado. */
  export function find() {
    const sel = selected();
    if (finding) finder?.focus(sel);
    else {
      const root = inner.querySelector<HTMLElement>(':scope > .transcript');
      if (root) finding = { root, scroller, initial: sel };
    }
  }
  // Resultado de la búsqueda de sesiones: la barra con lo buscado, empezando en el mensaje del
  // fragmento (el más reciente de lo escrito o contestado que lo contiene; la barra carga hasta él
  // si no lo estaba). Se espera a que esté pintada la conversación de esa sesión.
  let handledFind = 0;
  $effect(() => {
    const r = findReq;
    if (r && r.n !== handledFind && r.key === sessionKey && shownKey === sessionKey) {
      handledFind = r.n;
      goToHit(r.key, r.text);
    }
  });
  async function goToHit(key: string, text: string) {
    const index = await api.transcriptLocate(key, text).catch(() => null);
    if (key !== session.key) return;
    finding = null; // la barra se abre de nuevo: con lo buscado y desde ese mensaje
    await tick();
    const root = inner.querySelector<HTMLElement>(':scope > .transcript');
    if (root) finding = { root, scroller, initial: text, startAt: index };
  }

  function closeFind() {
    finding = null;
    stuck = atBottom();
    document.querySelector<HTMLTextAreaElement>('.composer textarea')?.focus();
  }

  // Panel lateral con el transcript de un subagente. Se cierra al cambiar de sesión, y sólo
  // entonces: cada instantánea del sondeo trae un objeto `session` nuevo (para la misma sesión), y
  // vigilando el objeto se cerraba solo en cuanto la sesión cambiaba en algo (subagentes en marcha).
  let viewer = $state<{ id: string; title: string } | null>(null);
  /** El panel del subagente, contraído en su pestaña (se contrae al pulsar fuera). */
  let viewerCollapsed = $state(false);
  const sessionKey = $derived(session.key);
  $effect(() => {
    sessionKey;
    viewer = null;
    finding = null; // la búsqueda es de cada conversación
  });
  /** Abrir un subagente, o el mismo otra vez estando contraído, lo enseña desplegado. */
  function showAgent(id: string, title: string) {
    viewer = { id, title };
    viewerCollapsed = false;
  }
  // Pestañas en el borde derecho: una por subagente que trabaja y la del abierto, aunque haya
  // terminado. Pulsar una lo despliega. Con un panel desplegado no se pintan: quedarían encima.
  type AgentTab = { id: string; title: string; running: boolean };
  const tabs = $derived.by((): AgentTab[] => {
    const v = viewer;
    if (v && !viewerCollapsed) return [];
    const list: AgentTab[] = session.agents
      .filter((a) => a.toolUseId)
      .map((a) => ({ id: a.toolUseId, title: a.description || a.agentType, running: true }));
    if (v && !list.some((t) => t.id === v.id)) list.unshift({ id: v.id, title: v.title, running: false });
    return list;
  });
  function openTab(t: AgentTab) {
    if (viewer?.id === t.id) viewerCollapsed = false;
    else showAgent(t.id, t.title);
  }
  let handledReq = 0;
  $effect(() => {
    const r = openAgent;
    if (r && r.n !== handledReq && r.key === sessionKey) {
      handledReq = r.n;
      showAgent(r.id, r.title);
    }
  });

</script>

<div class="conv-wrap">
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="conv"
    bind:this={scroller}
    onscroll={onScroll}
    onwheel={userScroll}
    onmousedown={userScroll}
    onkeydown={userScroll}
    ontouchstart={userScroll}
  >
    <div class="conv-inner" bind:this={inner}>
      {#if startIndex > 0}
        <button class="more" onclick={loadMore}>Cargar anteriores ({startIndex})</button>
      {/if}
      {#if empty && !loading && !sending}
        <div class="empty">
          {#if session.status === 'starting'}Arrancando Claude…{:else}Sin conversación todavía.{/if}
        </div>
      {/if}
      <Transcript
        {items}
        {startIndex}
        busy={session.status === 'busy'}
        live={session.status !== 'stopped'}
        idle={session.status === 'idle'}
        showPending
        sessionKey={session.key}
        cwd={session.cwd}
        {onToast}
        onOpenAgent={showAgent}
      />
    </div>
  </div>
  {#if session.status === 'busy'}
    <div class="thinking-bar"><Spinner size={12} /> Claude está trabajando…</div>
  {/if}
  {#if finding}
    <FindBar
      bind:this={finder}
      root={finding.root}
      scroller={finding.scroller}
      sessionKey={session.key}
      {startIndex}
      initial={finding.initial}
      startAt={finding.startAt ?? null}
      beside={!!viewer && !viewerCollapsed}
      loadOlder={(index) => prepend(startIndex - index)}
      onNav={() => {
        stopGlide();
        stuck = false;
      }}
      onClose={closeFind}
    />
  {/if}
  {#if viewer}
    <AgentViewer
      toolUseId={viewer.id}
      title={viewer.title}
      live={session.status !== 'stopped'}
      bind:collapsed={viewerCollapsed}
      onClose={() => (viewer = null)}
      {onToast}
      onOpenAgent={showAgent}
    />
  {/if}
  {#if tabs.length}
    <div class="av-tabs">
      {#each tabs as t (t.id)}
        <button class="av-tab" title="Ver el subagente: {t.title || 'Subagente'}" onclick={() => openTab(t)}>
          <span class="av-tab-ic">◂</span>
          <span class="av-tab-title">{t.title || 'Subagente'}</span>
          {#if t.running}<Spinner size={12} />{/if}
        </button>
      {/each}
    </div>
  {/if}
</div>
