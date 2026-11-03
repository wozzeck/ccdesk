<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import type { AgentRow, SearchHit, SessionView, Snapshot } from './api';
  import { api, shortPath, ago, BUSY, ATTENTION } from './api';
  import { fullDate } from './clock.svelte';
  import Spinner from './Spinner.svelte';

  interface Props {
    snap: Snapshot;
    activeKey: string | null;
    unread: Record<string, boolean>;
    now: number;
    onSelect: (key: string) => void;
    onNew: () => void;
    onAction: (action: string, s: SessionView, arg?: string) => void;
    /** `order` null: orden por recientes, sólo se han movido las fijadas. */
    onLayout: (pinned: string[], order: string[] | null) => void;
    onOpenAgent: (key: string, a: AgentRow) => void;
  }
  let {
    snap,
    activeKey,
    unread,
    now,
    onSelect,
    onNew,
    onAction,
    onLayout,
    onOpenAgent,
  }: Props = $props();

  // ── buscador ──
  let query = $state('');
  let searchEl: HTMLInputElement;
  // Sin distinguir mayúsculas ni tildes («configuracion» encuentra «Configuración»).
  const fold = (s: string) => s.toLowerCase().normalize('NFD').replace(/\p{M}/gu, '');
  const q = $derived(fold(query.trim()));
  const matches = (s: SessionView) =>
    !q || !!hits[s.key] || [s.title, s.cwd, s.location ?? '', s.sid].some((x) => fold(x).includes(q));

  // Búsqueda también en las conversaciones (lo escrito y lo contestado; el backend lo indexa).
  let hits = $state<Record<string, SearchHit>>({});
  let searchSeq = 0;
  $effect(() => {
    const text = query.trim();
    const n = ++searchSeq;
    if (fold(text).length < 2) {
      hits = {};
      return;
    }
    const t = setTimeout(() => {
      api
        .searchSessions(text)
        .then((r) => {
          if (n === searchSeq) hits = Object.fromEntries(r.map((h) => [h.key, h]));
        })
        .catch(() => {});
    }, 150);
    return () => clearTimeout(t);
  });

  // Orden recién soltado, hasta que llega el snapshot con él (así la fila no vuelve a su sitio
  // un instante mientras el backend lo guarda).
  let localLayout = $state<{ pinned: string[]; order: string[]; until: number } | null>(null);
  const managedAll = $derived.by(() => {
    const list = snap.sessions.filter((s) => s.managed);
    const L = localLayout;
    if (!L) return list;
    const idx = new Map([...L.pinned, ...L.order].map((k, i) => [k, i]));
    const pin = new Set(L.pinned);
    return list
      .map((s) => (pin.has(s.key) === s.pinned ? s : { ...s, pinned: pin.has(s.key) }))
      .sort((a, b) => (idx.get(a.key) ?? 1e9) - (idx.get(b.key) ?? 1e9));
  });
  $effect(() => {
    const L = localLayout;
    if (!L) return;
    const fromSnap = snap.sessions.filter((s) => s.managed).map((s) => s.key).join('\0');
    const shown = untrack(() => managedAll.map((s) => s.key).join('\0'));
    if (fromSnap === shown || now > L.until) localLayout = null;
  });
  const managed = $derived(managedAll.filter(matches));
  const pinnedCount = $derived(managed.filter((s) => s.pinned).length);
  const externalsAll = $derived(snap.sessions.filter((s) => !s.managed));
  const externals = $derived(externalsAll.filter(matches));
  const extAttention = $derived(externals.filter((s) => ATTENTION.includes(s.status)).length);

  function searchKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      query = '';
      searchEl.blur();
    } else if (e.key === 'Enter') {
      const first = managed[0] ?? externals[0];
      if (first) {
        // Sin esto el Intro acaba en la caja de texto, que se enfoca antes de que el navegador
        // escriba la tecla (y en una detenida, la reanudaría).
        e.preventDefault();
        onSelect(first.key);
        query = '';
        searchEl.blur();
      }
    }
  }
  let showExt = $state(false);

  // La sesión seleccionada siempre visible: si es externa, se despliega su grupo, y la fila
  // se desplaza a la vista (sólo cuando cambia la selección, no en cada refresco).
  let shownKey: string | null = null;
  $effect(() => {
    const k = activeKey;
    if (k === shownKey) return;
    shownKey = k;
    untrack(() => {
      if (k && snap.sessions.some((s) => s.key === k && !s.managed)) showExt = true;
    });
    tick().then(() =>
      listEl?.querySelector('.row.active')?.scrollIntoView({ block: 'nearest' }),
    );
  });

  // ── renombrar en línea ──
  let editing = $state<string | null>(null);
  let editValue = $state('');
  function startRename(s: SessionView) {
    editing = s.key;
    editValue = s.title;
  }
  function commitRename(s: SessionView) {
    if (editing !== s.key) return;
    editing = null;
    if (editValue.trim() !== s.title) onAction('rename', s, editValue.trim());
  }
  /** Ctrl+R: renombrar en línea la sesión abierta. Si se acaba con Enter o Esc, el cursor vuelve
   *  a la caja de texto (si se acaba con un clic en otro sitio, el foco se queda donde se pulsó). */
  let backToComposer = false;
  export function rename(key: string) {
    const s = managedAll.find((x) => x.key === key);
    if (!s) return;
    if (!matches(s)) query = '';
    backToComposer = true;
    startRename(s);
  }
  function endByKey() {
    if (!backToComposer) return;
    backToComposer = false;
    focusComposer();
  }
  function focusComposer() {
    tick().then(() => document.querySelector<HTMLTextAreaElement>('.composer textarea')?.focus());
  }
  function focusSelect(el: HTMLInputElement) {
    el.focus();
    el.select();
  }

  // ── menú contextual ──
  let menu = $state<{ x: number; y: number; s: SessionView } | null>(null);
  function openMenu(e: MouseEvent, s: SessionView) {
    e.preventDefault();
    const x = Math.min(e.clientX, window.innerWidth - 250);
    const y = Math.min(e.clientY, window.innerHeight - 330);
    menu = { x, y, s };
  }
  function act(action: string) {
    const s = menu!.s;
    menu = null;
    if (action === 'rename') startRename(s);
    else onAction(action, s);
  }
  onMount(() => {
    const focusSearch = (e: KeyboardEvent) => {
      if (e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        searchEl?.focus();
        searchEl?.select();
      }
    };
    window.addEventListener('keydown', focusSearch, true);
    const close = () => (menu = null);
    window.addEventListener('click', close);
    window.addEventListener('blur', close);
    return () => {
      window.removeEventListener('keydown', focusSearch, true);
      window.removeEventListener('click', close);
      window.removeEventListener('blur', close);
    };
  });

  // ── arrastrar y soltar (pointer events; el DnD HTML5 es poco fiable en WebKitGTK) ──
  // La fila arrastrada sigue al puntero y las demás se apartan (transform con transición) para
  // dejarle hueco donde caería. Las posiciones se calculan con la geometría del inicio, no con la
  // que van tomando las filas al apartarse (si no, el hueco «huiría» del puntero).
  // Con el orden por recientes (Preferencias) sólo se arrastran las fijadas, y sin salir de su
  // grupo: el sitio de las demás lo da su actividad.
  let listEl: HTMLDivElement;
  const recent = $derived(snap.settings.sessionOrder === 'recent');
  let drag = $state<{ key: string; y0: number; active: boolean; over: number; pinned: boolean } | null>(null);
  let geo: {
    items: HTMLElement[];
    mids: number[];
    top: number; // de la arrastrada
    min: number;
    max: number;
    h: number; // alto de la arrastrada (con sus subagentes)
    from: number;
    listTop: number;
    scroll0: number;
  } | null = null;

  const itemEls = () => Array.from(listEl.querySelectorAll<HTMLElement>('[data-item]'));
  const dividerEl = () => listEl.querySelector<HTMLElement>('.list > .divider');
  const contentY = (clientY: number) => clientY - (geo?.listTop ?? 0) + listEl.scrollTop;

  /** Dónde quedaría la arrastrada y si queda fijada (la línea entre grupos lo decide). */
  function plan(from: number, over: number) {
    const to = over > from ? over - 1 : over;
    const wasPinned = managed[from].pinned;
    const pc = pinnedCount - (wasPinned ? 1 : 0);
    const nowPinned = to < pc ? true : to > pc ? false : wasPinned;
    return { to, wasPinned, nowPinned };
  }

  function onPointerDown(e: PointerEvent, s: SessionView) {
    if (e.button !== 0 || editing) return;
    if ((e.target as HTMLElement).closest('button, input')) return;
    drag = { key: s.key, y0: e.clientY, active: false, over: -1, pinned: s.pinned };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function startGeo(d: NonNullable<typeof drag>) {
    const items = itemEls();
    const from = managed.findIndex((m) => m.key === d.key);
    if (from < 0 || items.length !== managed.length) return null;
    const lr = listEl.getBoundingClientRect();
    const sc = listEl.scrollTop;
    const rects = items.map((el) => el.getBoundingClientRect());
    const tops = rects.map((r) => r.top - lr.top + sc);
    const last = recent ? pinnedCount - 1 : rects.length - 1;
    const h = rects[from].height;
    return {
      items,
      mids: rects.map((r, i) => tops[i] + r.height / 2),
      top: tops[from],
      min: tops[0],
      max: tops[last] + rects[last].height - h,
      h,
      from,
      listTop: lr.top,
      scroll0: sc,
    };
  }

  function onPointerMove(e: PointerEvent) {
    if (!drag || q || (recent && !drag.pinned)) return; // con el buscador activo no se reordena
    if (!drag.active) {
      if (Math.abs(e.clientY - drag.y0) < 5) return;
      geo = startGeo(drag);
      if (!geo) return;
      drag.active = true;
    }
    if (!geo) return;
    const g = geo;
    const y = contentY(e.clientY);
    const y0 = drag.y0 - g.listTop + g.scroll0;
    const ty = Math.max(g.min, Math.min(g.max, g.top + (y - y0))) - g.top;
    g.items[g.from].style.transform = `translateY(${ty}px)`;
    // Hueco: la arrastrada pasa a otra fila cuando su borde delantero (el de abajo al bajar, el
    // de arriba al subir) cruza el centro original de esa fila. Con el centro, una sesión alta
    // (con subagentes) no llegaba nunca al final porque topa antes con el borde de la lista.
    const top = g.top + ty;
    const bottom = top + g.h;
    let over = g.from;
    for (let i = g.from + 1; i < g.mids.length && bottom > g.mids[i]; i++) over = i + 1;
    if (over === g.from) for (let i = g.from - 1; i >= 0 && top < g.mids[i]; i--) over = i;
    if (recent) over = Math.min(over, pinnedCount);
    if (over === drag.over) return;
    drag.over = over;
    const { wasPinned, nowPinned } = plan(g.from, over);
    g.items.forEach((el, i) => {
      if (i === g.from) return;
      let s = 0;
      if (over > g.from && i > g.from && i < over) s = -g.h;
      else if (over < g.from && i >= over && i < g.from) s = g.h;
      el.style.transform = s ? `translateY(${s}px)` : '';
    });
    const div = dividerEl();
    if (div)
      div.style.transform =
        wasPinned && !nowPinned ? `translateY(${-g.h}px)` : !wasPinned && nowPinned ? `translateY(${g.h}px)` : '';
  }

  function onPointerCancel() {
    drag = null;
    geo = null;
    for (const el of [...itemEls(), dividerEl()]) if (el) el.style.transform = '';
  }

  async function onPointerUp(s: SessionView) {
    const d = drag;
    const g = geo;
    drag = null;
    geo = null;
    if (!d) return;
    if (!d.active) {
      // Al cambiar de sesión, la caja se enfoca sola; una detenida que ya estaba abierta también
      // lleva el cursor a la caja al pulsarla.
      if (s.key === activeKey && s.status === 'stopped') focusComposer();
      onSelect(s.key);
      return;
    }
    if (!g || d.over < 0) {
      onPointerCancel();
      return;
    }
    const keys = managed.map((m) => m.key);
    const { to, nowPinned, wasPinned } = plan(g.from, d.over);
    keys.splice(g.from, 1);
    keys.splice(to, 0, d.key);
    let pc = pinnedCount - (wasPinned ? 1 : 0);
    if (nowPinned) pc += 1;
    const pinned = keys.slice(0, pc);
    const order = keys.slice(pc);

    // FLIP: se anota dónde se ve cada fila, se aplica el orden nuevo sin transición y cada
    // una se desliza desde donde estaba hasta su sitio (la soltada, desde el puntero).
    const before = new Map(itemEls().map((el) => [el.dataset.key!, el.getBoundingClientRect().top]));
    const divBefore = dividerEl()?.getBoundingClientRect().top;
    listEl.classList.add('settling');
    for (const el of [...itemEls(), dividerEl()]) if (el) el.style.transform = '';
    localLayout = { pinned, order, until: Date.now() + 3000 };
    onLayout(pinned, recent ? null : order);
    await tick();
    const moved: HTMLElement[] = [];
    for (const el of itemEls()) {
      const b = before.get(el.dataset.key!);
      if (b === undefined) continue;
      const dy = b - el.getBoundingClientRect().top;
      if (Math.abs(dy) > 0.5) {
        el.style.transform = `translateY(${dy}px)`;
        moved.push(el);
      }
    }
    const div = dividerEl();
    if (div && divBefore !== undefined) {
      const dy = divBefore - div.getBoundingClientRect().top;
      if (Math.abs(dy) > 0.5) {
        div.style.transform = `translateY(${dy}px)`;
        moved.push(div);
      }
    }
    void listEl.offsetHeight;
    listEl.classList.remove('settling');
    requestAnimationFrame(() => moved.forEach((el) => (el.style.transform = '')));
  }
</script>

{#snippet icon(s: SessionView)}
  {#if BUSY.includes(s.status)}
    <span class="st" title={s.status === 'starting' ? 'Arrancando' : 'Procesando'}><Spinner /></span>
  {:else if ATTENTION.includes(s.status)}
    <span class="st attn" title="Requiere tu atención">!</span>
  {:else if s.status === 'stopped'}
    <span class="st stopped" title="Detenida"></span>
  {:else if s.background}
    <span
      class="st bg"
      class:has-unread={unread[s.key]}
      title="Claude espera, pero tiene trabajo en segundo plano (comandos, Monitor o subagentes): detenerla lo cortaría{unread[s.key] ? '\nRespuesta nueva sin leer' : ''}"
    ></span>
  {:else if unread[s.key]}
    <span class="st unread" title="Respuesta nueva sin leer"></span>
  {:else}
    <span class="st idle" title="En marcha, esperando"></span>
  {/if}
{/snippet}

{#snippet children(s: SessionView)}
  {#if q && hits[s.key]}
    {@const h = hits[s.key]}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="hit" onclick={() => onSelect(s.key)}>
      <span class="hit-text">{h.before}<mark>{h.hit}</mark>{h.after}</span>
      {#if h.count > 1}<span class="hit-n" title="{h.count} coincidencias en la conversación">{h.count}</span>{/if}
    </div>
  {/if}
  {#each s.agents as a (a.toolUseId || a.agentId)}
    <div
      class="row sub"
      style="padding-left:{14 + 12 * a.depth}px"
      role="button"
      tabindex="0"
      title="Subagente: {a.description}{a.agentType ? ` (${a.agentType})` : ''}&#10;Clic para ver su transcript"
      onclick={() => onOpenAgent(s.key, a)}
      onkeydown={(e) => e.key === 'Enter' && onOpenAgent(s.key, a)}
    >
      <span class="st"><Spinner size={11} /></span>
      <span class="title">{a.description}</span>
      <span class="sub-type">{a.agentType}</span>
    </div>
  {/each}
{/snippet}

<div class="sidebar">
  <div class="side-head" data-tauri-drag-region="deep">
    <input
      class="search"
      type="search"
      placeholder="Buscar sesión…  (Ctrl+K)"
      spellcheck="false"
      bind:value={query}
      bind:this={searchEl}
      onkeydown={searchKey}
    />
    <button class="primary small" onclick={onNew} title="Nueva sesión (Ctrl+T)">+ Nueva</button>
  </div>

  <div class="list" bind:this={listEl}>
    {#each managed as s, i (s.key)}
      {#if i === pinnedCount && pinnedCount > 0}<div class="divider"></div>{/if}
      <div class="item" data-item data-key={s.key} class:lifted={drag?.active && drag.key === s.key}>
      <div
        class="row"
        data-row
        class:active={s.key === activeKey}
        class:stopped={s.status === 'stopped'}
        role="button"
        tabindex="0"
        onpointerdown={(e) => onPointerDown(e, s)}
        onpointermove={onPointerMove}
        onpointerup={() => onPointerUp(s)}
        onpointercancel={onPointerCancel}
        oncontextmenu={(e) => openMenu(e, s)}
        onkeydown={(e) => {
          if (e.key !== 'Enter') return;
          e.preventDefault(); // que el Intro no acabe en la caja de texto (como en el buscador)
          onSelect(s.key);
        }}
      >
        {@render icon(s)}
        {#if editing === s.key}
          <input
            class="rename"
            bind:value={editValue}
            use:focusSelect
            onkeydown={(e) => {
              if (e.key === 'Enter') {
                // Sin esto, el Intro acaba también en la caja de texto: el cursor pasa allí antes
                // de que el navegador escriba la tecla.
                e.preventDefault();
                commitRename(s);
                endByKey();
              }
              if (e.key === 'Escape') {
                editing = null;
                endByKey();
              }
            }}
            onblur={() => {
              backToComposer = false;
              commitRename(s);
            }}
          />
        {:else}
          <span
            class="title"
            title="{s.title}&#10;{shortPath(s.cwd, snap.home)}{s.location ? `\ntmux: ${s.location}` : ''}"
            >{s.title}</span
          >
        {/if}
        {#if s.notes}<span class="nbadge" title="{s.notes} to-do{s.notes === 1 ? '' : 's'} pendiente{s.notes === 1 ? '' : 's'}">✎{s.notes}</span>{/if}
        {#if s.dupes.length}
          <span class="dupe" title="Abierta también en otro proceso (escriben en el mismo transcript):&#10;{s.dupes.join('\n')}">⚠</span>
        {/if}
        <span class="when" title={s.stoppedAt ? `Detenida el ${fullDate(s.stoppedAt)}` : undefined}
          >{ago(s.lastActivity, now)}{s.exempt ? ' ∞' : ''}</span
        >
        <div class="actions">
          <button
            class="icon pin"
            class:on={s.pinned}
            title={s.pinned ? 'Desfijar' : 'Fijar arriba'}
            onclick={() => onAction('pin', s)}
          >
            <svg viewBox="0 0 16 16" width="13" height="13"
              ><path
                d="M9.8 1.2 14.8 6.2 13.4 6.9 10.9 9.4 10.6 13 9.2 14.4 6.2 11.4 2.4 15.2 1.6 15.2 1.6 14.4 5.4 10.6 2.4 7.6 3.8 6.2 7.4 5.9 9.9 3.4 10.6 2z"
                fill="currentColor"
              /></svg
            >
          </button>
          {#if s.status === 'stopped'}
            <button class="icon go" title="Reanudar" onclick={() => onAction('resume', s)}>▶</button>
          {:else}
            <button class="icon" title="Detener (libera memoria)" onclick={() => onAction('stop', s)}
              >■</button
            >
          {/if}
        </div>
      </div>
      {@render children(s)}
      </div>
    {/each}
    {#if q && !managed.length && !externals.length}
      <div class="hint">Ninguna sesión coincide con «{query.trim()}».</div>
    {:else if !q && managedAll.length === 0}
      <div class="hint">Aún no hay sesiones de ccdesk. Crea una con «+ Nueva» o adopta una externa.</div>
    {/if}

    {#if externals.length}
      <div class="group-row">
        <button class="group" onclick={() => (showExt = !showExt)}>
          <span>{showExt || q ? '▾' : '▸'} Externas ({externals.length})</span>
          {#if extAttention}<span class="st attn tiny">!</span>{/if}
        </button>
        <button
          class="small adopt-all"
          title="Pasar a ccdesk todas las inactivas que corren en tmux"
          onclick={() => onAction('adopt-all', externals[0])}>Adoptar todas</button
        >
      </div>
      {#if showExt || q}
        {#each externals as s (s.key)}
          <div
            class="row ext"
            class:active={s.key === activeKey}
            role="button"
            tabindex="0"
            onclick={() => onSelect(s.key)}
            onkeydown={(e) => e.key === 'Enter' && onSelect(s.key)}
            oncontextmenu={(e) => openMenu(e, s)}
          >
            {@render icon(s)}
            <span
              class="title"
              title="{s.title}&#10;{shortPath(s.cwd, snap.home)} · {s.tmuxPane
                ? 'tmux ' + s.tmuxPane.split(':')[0]
                : (s.externalKind ?? 'externa')}">{s.title}</span
            >
            <span class="when">{ago(s.lastActivity, now)}</span>
            <div class="actions">
              {#if s.tmuxPane}
                <button
                  class="icon"
                  title="Adoptar: mover su ventana de tmux a ccdesk (el proceso sigue igual)"
                  onclick={(e) => {
                    e.stopPropagation();
                    onAction('adopt', s);
                  }}>⇱</button
                >
              {/if}
            </div>
          </div>
          {@render children(s)}
        {/each}
      {/if}
    {/if}
  </div>
</div>

{#if menu}
  {@const s = menu.s}
  <div
    class="menu"
    style="left:{menu.x}px;top:{menu.y}px"
    role="menu"
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.key === 'Escape' && (menu = null)}
  >
    {#if s.managed}
      <button onclick={() => act('rename')}>Renombrar</button>
      <button onclick={() => act('pin')}>{s.pinned ? 'Desfijar' : 'Fijar arriba'}</button>
      <button onclick={() => act('exempt')}
        >{s.exempt ? 'Permitir auto-detener' : 'No auto-detener nunca (∞)'}</button
      >
      <hr />
      {#if s.status === 'stopped'}
        <button onclick={() => act('resume')}>Reanudar</button>
        <button onclick={() => act('forget')}>Quitar de la lista</button>
      {:else}
        <button onclick={() => act('stop')}>Detener</button>
        <button onclick={() => act('kill')}>Forzar detención</button>
      {/if}
    {:else if s.tmuxPane}
      <button onclick={() => act('adopt')}>Adoptar en ccdesk</button>
    {/if}
    <hr />
    <button onclick={() => act('copy-id')}>Copiar ID de sesión</button>
    <button onclick={() => act('copy-resume')}>Copiar comando «claude --resume»</button>
    <button onclick={() => act('open-dir')}>Abrir carpeta</button>
    <button onclick={() => act('open-scratch')}>Abrir scratchpad</button>
  </div>
{/if}
