<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import {
    api,
    errText,
    shortPath,
    ATTENTION,
    type Snapshot,
    type SessionView,
    type Settings,
    type AgentRow,
  } from './lib/api';
  import Sidebar from './lib/Sidebar.svelte';
  import Icon from './lib/Icon.svelte';
  import { ui } from './lib/ui.svelte';
  import Conversation from './lib/Conversation.svelte';
  import Composer from './lib/Composer.svelte';
  import HeadTitle from './lib/HeadTitle.svelte';
  import TerminalDrawer from './lib/TerminalDrawer.svelte';
  import StatusBar from './lib/StatusBar.svelte';
  import NewSession from './lib/NewSession.svelte';
  import NotesPanel from './lib/NotesPanel.svelte';
  import { applyTheme } from './lib/theme';
  import WindowControls from './lib/WindowControls.svelte';
  import Prefs from './lib/Prefs.svelte';
  import { notes } from './lib/notes.svelte';
  import { copyPlain } from './lib/clipboard';
  import { quotePaths } from './lib/files';
  import { getCurrentWebview } from '@tauri-apps/api/webview';

  let snap = $state<Snapshot | null>(null);
  let activeKey = $state<string | null>(null);
  let unread = $state<Record<string, boolean>>({});
  let sidebar = $state<ReturnType<typeof Sidebar>>();
  let drawerOpen = $state(false);
  let drawerAuto = false;
  let sidebarWidth = $state(290);
  let showNew = $state(false);
  let showPrefs = $state(false);
  /** Petición de abrir el transcript de un subagente (desde el árbol del panel izquierdo). */
  let agentReq = $state<{ key: string; id: string; title: string; n: number } | null>(null);
  let notesOpen = $state(true);
  let toastMsg = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let confirmBox = $state<{ msg: string; ok: string; resolve: (v: boolean) => void } | null>(null);
  let now = $state(Date.now());
  let drawer = $state<ReturnType<typeof TerminalDrawer> | null>(null);
  let composer = $state<ReturnType<typeof Composer> | null>(null);
  let conversation = $state<ReturnType<typeof Conversation> | null>(null);
  const prevStatus: Record<string, string> = {};
  let layoutLoaded = false;

  const active = $derived<SessionView | null>(
    snap?.sessions.find((s) => s.key === activeKey) ?? null,
  );
  const attentionCount = $derived(
    snap?.sessions.filter((s) => s.managed && ATTENTION.includes(s.status)).length ?? 0,
  );

  $effect(() => {
    document.title = attentionCount ? `(${attentionCount}) ccdesk` : 'ccdesk';
  });

  // Tema guardado (el de Preferencias lo prueba en vivo y, al cancelar, vuelve a éste).
  let themeSaved = $state<string | null>(null);
  $effect(() => {
    const t = snap?.settings.theme;
    if (t && t !== themeSaved) {
      themeSaved = t;
      applyTheme(t);
    }
  });

  // Zoom con Ctrl+rueda (y Ctrl+0 para volver al 100 %): se aplica al momento y se guarda al parar.
  let zoom = 1;
  let zoomAcc = 0;
  let zoomSave: ReturnType<typeof setTimeout> | undefined;
  function setZoom(z: number) {
    z = Math.round(Math.min(2.5, Math.max(0.5, z)) * 10) / 10;
    if (z === zoom) return;
    zoom = z;
    api.setZoom(z).catch((e) => toast(errText(e)));
    toast(`Zoom ${Math.round(z * 100)} %`);
    clearTimeout(zoomSave);
    zoomSave = setTimeout(() => saveSettings({ zoom }), 500);
  }
  function onWheel(e: WheelEvent) {
    if (!e.ctrlKey) return;
    e.preventDefault();
    e.stopPropagation();
    // Una muesca de rueda = un paso; el touchpad (deltas pequeños) se acumula.
    zoomAcc += e.deltaMode === 1 ? e.deltaY * 40 : e.deltaY;
    if (Math.abs(zoomAcc) < 40) return;
    const step = zoomAcc < 0 ? 0.1 : -0.1;
    zoomAcc = 0;
    setZoom(zoom + step);
  }

  function toast(m: string) {
    toastMsg = m;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toastMsg = null), 4000);
  }

  function ask(msg: string, ok = 'Aceptar'): Promise<boolean> {
    (document.activeElement as HTMLElement | null)?.blur?.();
    return new Promise((resolve) => (confirmBox = { msg, ok, resolve }));
  }
  function focusEl(el: HTMLElement) {
    el.focus();
  }
  function answer(v: boolean) {
    confirmBox?.resolve(v);
    confirmBox = null;
  }

  function applySnap(s: Snapshot) {
    ui.showMedia = s.settings.showMedia ?? true;
    for (const v of s.sessions) {
      const p = prevStatus[v.key];
      if (
        p &&
        p !== v.status &&
        v.key !== activeKey &&
        v.status === 'idle' &&
        (p === 'busy' || ATTENTION.includes(p as any))
      ) {
        unread[v.key] = true;
      }
      prevStatus[v.key] = v.status;
    }
    snap = s;
    if (!layoutLoaded) {
      layoutLoaded = true;
      sidebarWidth = s.settings.sidebarWidth || 290;
      notesOpen = s.settings.notesOpen ?? true;
      zoom = s.settings.zoom || 1;
    }
    const a = s.sessions.find((x) => x.key === activeKey);
    if (activeKey && !a) select(null);
    // La terminal se abre sola cuando Claude pide algo (permiso, pregunta) y se cierra al terminar.
    if (a && a.managed && ATTENTION.includes(a.status)) {
      if (!drawerOpen) {
        drawerOpen = true;
        drawerAuto = true;
      }
    } else if (drawerAuto && drawerOpen) {
      drawerOpen = false;
      drawerAuto = false;
    }
  }

  function select(key: string | null) {
    activeKey = key;
    if (key) unread[key] = false;
    drawerAuto = false;
    try {
      if (key) localStorage.setItem('ccdesk.active', key);
    } catch {
      /* sin almacenamiento */
    }
    if (!key) api.transcriptOpen(null).catch(() => {});
    const a = snap?.sessions.find((x) => x.key === key);
    if (a && a.managed && ATTENTION.includes(a.status)) {
      drawerOpen = true;
      drawerAuto = true;
    }
  }

  function cycle(dir: number) {
    const list = snap?.sessions ?? [];
    if (!list.length) return;
    const i = list.findIndex((s) => s.key === activeKey);
    const n = list[(i + dir + list.length) % list.length];
    select(n.key);
  }

  /** Como `cycle`, pero saltando las detenidas (Ctrl+N / Ctrl+P), en el orden de la lista. */
  function cycleLive(dir: number) {
    const list = snap?.sessions ?? [];
    const n = list.length;
    const i = list.findIndex((s) => s.key === activeKey);
    const base = i >= 0 ? i : dir > 0 ? -1 : n;
    for (let step = 1; step <= n; step++) {
      const s = list[(((base + dir * step) % n) + n) % n];
      if (s.status !== 'stopped' && s.key !== activeKey) {
        select(s.key);
        return;
      }
    }
  }

  function saveSettings(patch: Partial<Settings>) {
    if (!snap) return;
    api
      .settingsUpdate({ ...snap.settings, ...patch })
      .then(applySnap)
      .catch((e) => toast(errText(e)));
  }

  const BG_WARN =
    'tiene trabajo en segundo plano (comandos, Monitor o subagentes) y al detenerla se corta.';

  async function stopSession(s: SessionView, force = false) {
    try {
      await api.sessionStop(s.key, force);
    } catch (e) {
      if (errText(e) === 'background') {
        if (await ask(`«${s.title}» ${BG_WARN} ¿Detenerla igualmente?`, 'Detener'))
          await api.sessionStop(s.key, false, true);
        return;
      }
      if (errText(e) !== 'busy') throw e;
      const what = s.status === 'busy' ? 'está trabajando' : 'espera una respuesta';
      if (await ask(`«${s.title}» ${what}. ¿Forzar la detención?`, 'Forzar'))
        await api.sessionStop(s.key, true);
    }
  }

  async function onAction(action: string, s: SessionView, arg?: string) {
    try {
      switch (action) {
        case 'stop':
          await stopSession(s);
          break;
        case 'kill':
          if (await ask(`¿Forzar la detención de «${s.title}»?${s.background ? ` ${BG_WARN}` : ''}`, 'Forzar'))
            await api.sessionStop(s.key, true);
          break;
        case 'resume':
          await api.sessionResume(s.key);
          select(s.key);
          break;
        case 'adopt': {
          const ok = await ask(
            `Su ventana de tmux (${s.tmuxPane}) pasará a la sesión de ccdesk. El proceso sigue igual: ` +
              `no se reinicia ni pierde lo que tenga en segundo plano. ¿Adoptar «${s.title}»?`,
            'Adoptar',
          );
          if (!ok) break;
          toast('Adoptando…');
          const k = await api.sessionAdopt(s.key);
          select(k);
          toast('Sesión adoptada');
          break;
        }
        case 'adopt-all': {
          const ext = snap?.sessions.filter((x) => !x.managed) ?? [];
          const ready = ext.filter((x) => x.tmuxPane);
          if (!ready.length) {
            toast('No hay externas en tmux que se puedan adoptar');
            break;
          }
          const ok = await ask(
            `Las ventanas de tmux de ${ready.length} de ${ext.length} sesiones externas pasarán a la sesión de ccdesk. ` +
              `Los procesos siguen igual: no se reinician ni pierden lo que tengan en segundo plano, ` +
              `aunque estén trabajando. Las que no corren en tmux se quedan como están.`,
            'Adoptar',
          );
          if (!ok) break;
          toast(`Adoptando ${ready.length} sesiones…`);
          const r = await api.sessionAdoptAll();
          toast(
            `Adoptadas ${r.adopted.length}` +
              (r.skipped.length ? ` · sin adoptar ${r.skipped.length}: ${r.skipped.join('; ')}` : ''),
          );
          break;
        }
        case 'pin':
          applySnap(await api.setPinned(s.key, !s.pinned));
          break;
        case 'exempt':
          applySnap(await api.setExempt(s.key, !s.exempt));
          break;
        case 'rename':
          applySnap(await api.sessionRename(s.key, arg ?? ''));
          break;
        case 'forget':
          if (await ask(`¿Quitar «${s.title}» de la lista? El transcript no se borra.`, 'Quitar'))
            applySnap(await api.sessionForget(s.key));
          break;
        case 'copy-id':
          if (copyPlain(s.sid)) toast('ID copiado');
          break;
        case 'copy-resume':
          if (copyPlain(`cd '${s.cwd.replace(/'/g, "'\\''")}' && claude --resume ${s.sid}`))
            toast('Comando copiado');
          break;
        case 'open-dir':
          await api.openPath(s.cwd);
          break;
        case 'open-scratch':
          await api.openScratchpad(s.sid);
          break;
      }
    } catch (e) {
      toast(errText(e));
    }
  }

  /** Renombrar desde el título de la cabecera (la clave es la de la sesión que se editaba). */
  function renameFromHead(key: string, name: string) {
    const s = snap?.sessions.find((x) => x.key === key);
    if (s) onAction('rename', s, name);
  }

  async function onCreate(cwd: string, name: string, model: string, effort: string) {
    const key = await api.sessionNew(cwd, name || undefined, model || undefined, effort || undefined);
    showNew = false;
    applySnap(await api.snapshot());
    select(key);
  }

  function onKey(e: KeyboardEvent) {
    if (confirmBox) {
      if (e.key === 'Escape' || e.key === 'Enter') {
        e.preventDefault();
        e.stopPropagation();
        answer(e.key === 'Enter');
      }
      return;
    }
    const k = e.key.toLowerCase();
    if (e.ctrlKey && !e.shiftKey && k === 't') {
      e.preventDefault();
      showNew = true;
    } else if (e.ctrlKey && e.code === 'Backquote') {
      e.preventDefault();
      drawerOpen = !drawerOpen;
      drawerAuto = false;
      if (drawerOpen) setTimeout(() => drawer?.focus(), 50);
    } else if (e.ctrlKey && e.shiftKey && e.code === 'KeyO') {
      e.preventDefault();
      toggleNotes();
    } else if (e.ctrlKey && !e.shiftKey && e.key === '0') {
      e.preventDefault();
      setZoom(1);
    } else if (e.ctrlKey && e.key === ',') {
      e.preventDefault();
      showPrefs = true;
    } else if (
      e.ctrlKey && !e.shiftKey && !e.altKey && (k === 'n' || k === 'p') &&
      // En la terminal integrada son el historial de la shell y de Claude Code.
      !(e.target as Element | null)?.closest?.('.xterm')
    ) {
      e.preventDefault();
      cycleLive(k === 'n' ? 1 : -1);
    } else if (
      e.ctrlKey && !e.shiftKey && !e.altKey && k === 'r' &&
      // En la terminal integrada es la búsqueda en el historial.
      !(e.target as Element | null)?.closest?.('.xterm')
    ) {
      e.preventDefault();
      const a = snap?.sessions.find((x) => x.key === activeKey);
      if (a && !a.managed) toast('Las sesiones externas no se pueden renombrar: adóptala primero');
      else if (a) sidebar?.rename(a.key);
    } else if (
      e.ctrlKey && !e.shiftKey && !e.altKey && k === 'f' &&
      // En la terminal integrada es de la shell o de Claude Code.
      !(e.target as Element | null)?.closest?.('.xterm')
    ) {
      e.preventDefault();
      // Con la terminal a pantalla completa la conversación no se ve.
      if (active && !drawerOpen && !showNew && !showPrefs) conversation?.find();
    } else if (e.ctrlKey && e.key === 'PageDown') {
      e.preventDefault();
      cycle(1);
    } else if (e.ctrlKey && e.key === 'PageUp') {
      e.preventDefault();
      cycle(-1);
    } else if (e.ctrlKey && e.shiftKey && e.code === 'KeyC') {
      const sel = window.getSelection()?.toString();
      if (sel) {
        e.preventDefault();
        if (copyPlain(sel)) toast('Copiado como texto sin formato');
      }
    }
  }

  // ── soltar ficheros (desde el gestor de ficheros): sus rutas van a donde se suelten ──
  // Tarjeta de to-do → se añaden a ella; panel de to-dos → to-do nuevo; terminal abierta → a la
  // TUI; resto de la sesión → a la caja de texto, en el cursor.
  type DropZone = { kind: 'card'; id: string; el: Element } | { kind: 'notes' | 'terminal' | 'composer'; el: Element };
  function dropZone(target: EventTarget | null): DropZone | null {
    const el = target as Element | null;
    if (!el?.closest || !active) return null;
    const card = el.closest<HTMLElement>('[data-note]');
    if (card?.dataset.note) return { kind: 'card', id: card.dataset.note, el: card };
    const np = el.closest('.notes-panel');
    if (np) return { kind: 'notes', el: np };
    const term = el.closest('.drawer.open');
    if (term) return { kind: 'terminal', el: term };
    const box = document.querySelector('.composer:not(.term-hidden)');
    if (el.closest('main') && box && active.managed && active.status !== 'stopped') return { kind: 'composer', el: box };
    return null;
  }
  let lit: Element | null = null;
  function highlight(el: Element | null) {
    if (lit === el) return;
    lit?.classList.remove('drop-hover');
    el?.classList.add('drop-hover');
    lit = el;
  }
  // Tauri da la posición en píxeles de pantalla; en WebKitGTK devicePixelRatio ya incluye el zoom.
  function zoneAt(pos: { x: number; y: number }): DropZone | null {
    const r = window.devicePixelRatio || 1;
    return dropZone(document.elementFromPoint(pos.x / r, pos.y / r));
  }
  async function dropPaths(z: DropZone | null, raw: string[]) {
    if (!z) return;
    if (!raw.length) {
      toast('No me llega la ruta de lo soltado');
      return;
    }
    const paths = await api.resolvePaths(raw).catch(() => raw);
    const text = quotePaths(paths);
    if (z.kind === 'card') {
      const n = notes.list.find((x) => x.id === z.id);
      if (n) notes.update(n.id, n.text.trim() ? `${n.text.replace(/\s+$/, '')} ${text}` : text);
    } else if (z.kind === 'notes') {
      notes.add(text, false);
    } else if (z.kind === 'terminal') {
      api.termWrite(text + ' ').catch((err) => toast(errText(err)));
      drawer?.focus();
    } else if (!composer?.attach(paths)) {
      toast('La sesión no admite texto ahora mismo');
    }
  }

  function openAgent(key: string, a: AgentRow) {
    if (!a.toolUseId) {
      toast('Este subagente no indica su llamada; ábrelo desde la conversación');
      return;
    }
    if (activeKey !== key) select(key);
    agentReq = { key, id: a.toolUseId, title: a.description, n: Date.now() };
  }

  function toggleNotes(open = !notesOpen) {
    notesOpen = open;
    saveSettings({ notesOpen });
  }

  /** Lo escrito en la caja pasa a un to-do de la sesión abierta. */
  async function stash(text: string) {
    if (!active) return;
    // El panel carga la lista al abrirse; cerrado, la del almacén puede ser de otra sesión (o de
    // ninguna) y el to-do iría a parar allí.
    await notes.load(active.key);
    notes.add(text, false);
    if (!notesOpen) toggleNotes(true);
    toast('Guardado en «To-do»');
  }

  async function savePrefs(st: Settings) {
    applySnap(await api.settingsUpdate(st));
    showPrefs = false;
    toast('Preferencias guardadas');
  }

  function startSidebarResize(e: PointerEvent) {
    const x0 = e.clientX;
    const w0 = sidebarWidth;
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const clamp = (w: number) => Math.round(Math.max(200, Math.min(560, w)));
    const move = (ev: PointerEvent) => (sidebarWidth = clamp(w0 + ev.clientX - x0));
    const up = () => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      saveSettings({ sidebarWidth });
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
  }

  onMount(() => {
    let alive = true;
    const un = listen<Snapshot>('snapshot', (e) => applySnap(e.payload));
    // Pulsada una notificación de escritorio: el backend ya ha traído la ventana; se abre su sesión.
    const unFocus = listen<string>('focus-session', (e) => {
      if (snap?.sessions.some((x) => x.key === e.payload)) select(e.payload);
    });
    api
      .snapshot()
      .then((s) => {
        if (!alive) return;
        applySnap(s);
        let last: string | null = null;
        try {
          last = localStorage.getItem('ccdesk.active');
        } catch {
          /* sin almacenamiento */
        }
        // La última abierta: por clave, por session id (la clave cambia al adoptarla), por lo
        // guardado en el navegador y, si no, la primera gestionada.
        const pick =
          s.sessions.find((x) => x.key === s.lastActive) ??
          s.sessions.find((x) => !!s.lastActiveSid && x.sid === s.lastActiveSid) ??
          s.sessions.find((x) => x.key === last) ??
          s.sessions.find((x) => x.managed) ??
          null;
        if (pick) select(pick.key);
      })
      .catch((e) => toast(errText(e)));
    const timer = setInterval(() => (now = Date.now()), 30_000);
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('wheel', onWheel, { capture: true, passive: false });
    const unDrop = getCurrentWebview().onDragDropEvent((ev) => {
      const p = ev.payload;
      if (p.type === 'enter' || p.type === 'over') highlight(zoneAt(p.position)?.el ?? null);
      else if (p.type === 'leave') highlight(null);
      else if (p.type === 'drop') {
        highlight(null);
        dropPaths(zoneAt(p.position), p.paths);
      }
    });
    return () => {
      alive = false;
      un.then((f) => f());
      unFocus.then((f) => f());
      clearInterval(timer);
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('wheel', onWheel, true);
      unDrop.then((f) => f());
    };
  });
</script>

<div class="app" style="--sidebar:{sidebarWidth}px">
  <aside>
    {#if snap}
      <Sidebar
        bind:this={sidebar}
        {snap}
        {activeKey}
        {unread}
        {now}
        onSelect={select}
        onNew={() => (showNew = true)}
        {onAction}
        onLayout={(p, o) =>
          api
            .setLayout(p, o)
            .then(applySnap)
            .catch((e) => toast(errText(e)))}
        onOpenAgent={openAgent}
      />
    {/if}
  </aside>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="vresizer" onpointerdown={startSidebarResize}></div>

  <main>
    <header class="session-head" data-tauri-drag-region="deep">
    {#if active && snap}
        <div class="h-title">
          {#key active.key}
            <HeadTitle
              sessionKey={active.key}
              title={active.title}
              editable={active.managed}
              onRename={renameFromHead}
              onDone={() => (drawerOpen ? drawer?.focus() : composer?.focus())}
            />
          {/key}
          <span class="path" title={active.cwd}>{shortPath(active.cwd, snap.home)}</span>
        </div>
        {#if active.managed && active.status !== 'stopped'}
          <button
            class="hbtn"
            class:on={drawerOpen}
            title="Terminal (Ctrl+º)"
            aria-label="Terminal"
            onclick={() => {
              drawerOpen = !drawerOpen;
              drawerAuto = false;
            }}><Icon name="terminal" /></button
          >
        {/if}
        <button
          class="hbtn"
          class:on={notesOpen}
          title="To-do{active.notes ? ` (${active.notes})` : ''} (Ctrl+Mayús+O)"
          aria-label="To-do"
          onclick={() => toggleNotes()}
          ><Icon name="todo" />{#if active.notes}<span class="hbadge">{active.notes}</span>{/if}</button
        >
    {:else}
        <div class="h-title"><span class="name">ccdesk</span></div>
    {/if}
      <button class="hbtn" title="Preferencias (Ctrl+,)" aria-label="Preferencias" onclick={() => (showPrefs = true)}
        ><Icon name="settings" /></button
      >
      <WindowControls />
    </header>
    <div class="main-body">
      <div class="main-col">
    <!-- La terminal ocupa todo el hueco de la conversación; ésta sigue debajo (oculta), así al
         cerrarla vuelve donde estaba. -->
    <div class="conv-area" class:term-full={drawerOpen && !!active}>
    {#if active && snap}
      <Conversation bind:this={conversation} session={active} onToast={toast} openAgent={agentReq} />
    {:else}
      <div class="welcome">
        <h2>ccdesk</h2>
        <p>Elige una sesión a la izquierda o crea una nueva.</p>
        <button class="primary" onclick={() => (showNew = true)}>+ Nueva sesión</button>
        <p class="muted">
          Ctrl+T nueva · Ctrl+Av/Re Pág cambia de sesión · Ctrl+N/P siguiente/anterior en marcha ·
          Ctrl+R renombra · Ctrl+F busca en la conversación · Ctrl+º terminal · Ctrl+Mayús+C copia sin formato
        </p>
      </div>
    {/if}
    <TerminalDrawer
      bind:this={drawer}
      session={active}
      open={drawerOpen && !!active}
      onClose={() => {
        drawerOpen = false;
        drawerAuto = false;
      }}
    />
    </div>
    {#if active}
      <Composer
        bind:this={composer}
        session={active}
        suggestion={snap?.activeSuggestion?.key === active.key ? snap.activeSuggestion.text : null}
        hidden={drawerOpen}
        onToast={toast}
        {onAction}
        onStash={stash}
        onShowTerminal={() => {
          drawerOpen = true;
          setTimeout(() => drawer?.focus(), 50);
        }}
      />
    {/if}
      </div>
      {#if active && notesOpen}
        <NotesPanel session={active} {now} onClose={() => toggleNotes(false)} onToast={toast} {ask} />
      {/if}
    </div>
  </main>

  <footer>
    <StatusBar
      status={snap?.activeStatus ?? null}
      quota={snap?.quota ?? null}
      usage={snap?.usage ?? null}
      session={active}
      mode={snap?.activeMode ?? null}
      allowYolo={snap?.settings.allowYolo ?? true}
      {now}
      onToast={toast}
      {ask}
    />
  </footer>
</div>

{#if showNew && snap}
  <NewSession
    recent={snap.recentCwds}
    home={snap.home}
    {onCreate}
    onCancel={() => (showNew = false)}
  />
{/if}

{#if showPrefs && snap}
  <Prefs settings={snap.settings} onSave={savePrefs} onCancel={() => (showPrefs = false)} />
{/if}

{#if confirmBox}
  <div class="modal-bg">
    <div class="modal small-modal" role="dialog">
      <p>{confirmBox.msg}</p>
      <div class="buttons">
        <button onclick={() => answer(false)}>Cancelar</button>
        <button class="primary" use:focusEl onclick={() => answer(true)}>{confirmBox.ok}</button>
      </div>
    </div>
  </div>
{/if}

{#if toastMsg}
  <div class="toast" role="status">{toastMsg}</div>
{/if}
