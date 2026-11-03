<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { api, errText, ATTENTION, type SessionView, type SlashCmd } from './api';
  import { outbox } from './outbox.svelte';
  import Icon from './Icon.svelte';
  import MediaStrip from './MediaStrip.svelte';
  import {
    addAttachment,
    expand,
    insertInto,
    isImagePath,
    pastedImage,
    quotePaths,
    removeToken,
    renumber,
    tokenize,
    type Att,
  } from './files';

  interface Props {
    session: SessionView;
    /** Sugerencia de Claude Code para el siguiente mensaje (se acepta con Tab, como en la TUI). */
    suggestion?: string | null;
    /** Con la terminal abierta no se ve (la TUI tiene su propia entrada); el borrador se conserva. */
    hidden?: boolean;
    onToast: (m: string) => void;
    onAction: (action: string, s: SessionView) => void;
    onShowTerminal: () => void;
    /** Guardar el texto como to-do en vez de enviarlo. */
    onStash: (text: string) => void;
  }
  let { session, suggestion = null, hidden = false, onToast, onAction, onShowTerminal, onStash }: Props = $props();

  /** Borradores por sesión, con las rutas de sus imágenes (como se enviarían). */
  const drafts = new Map<string, string>();
  let text = $state('');
  // Imágenes adjuntas: en el texto van como marca («[imagen 1]») y se ven en miniatura encima de
  // la caja; al enviar, cada marca se cambia por su ruta. Si se borra la marca, la imagen se va.
  let atts = $state<Att[]>([]);
  const shown = $derived(atts.filter((a) => text.includes(a.token)));
  /** El texto tal como se envía: con las rutas en vez de las marcas. */
  function canonical(): string {
    return expand(text, atts);
  }
  /** Pone en la caja un texto con rutas (borrador, historial, lo que no llegó a enviarse): las de
   *  imágenes pasan a marcas con su miniatura. */
  function setCanonical(v: string) {
    const r = tokenize(v);
    text = r.text;
    atts = r.atts;
  }
  let currentKey = '';
  let sending = $state(false);
  let ta = $state<HTMLTextAreaElement | null>(null);
  let box: HTMLDivElement | undefined;
  let ghostEl = $state<HTMLDivElement | null>(null);

  $effect(() => {
    const k = session.key;
    untrack(() => {
      if (k === currentKey) return;
      if (currentKey) drafts.set(currentKey, canonical());
      currentKey = k;
      setCanonical(drafts.get(k) ?? '');
      dismissed = null;
      resetHistory();
      queueMicrotask(() => {
        autosize();
        // También en una detenida: se puede escribir ya, y lo primero que se teclea la reanuda.
        ta?.focus();
      });
    });
  });

  const mode = $derived(
    session.status === 'stopped'
      ? 'stopped'
      : !session.managed
        ? 'external'
        : ATTENTION.includes(session.status)
          ? 'attention'
          : session.status === 'starting'
            ? 'starting'
            : 'ready',
  );

  // Al reanudar la sesión abierta (desde la lista, la cabecera o escribiendo), el cursor va a la caja.
  let prevMode = '';
  let prevModeKey = '';
  $effect(() => {
    const m = mode;
    const k = session.key;
    untrack(() => {
      if (prevMode === 'stopped' && m !== 'stopped' && prevModeKey === k && !hidden) tick().then(() => ta?.focus());
      prevMode = m;
      prevModeKey = k;
    });
  });

  // Al cerrar la terminal, el cursor vuelve a la caja.
  let wasHidden = false;
  $effect(() => {
    if (wasHidden && !hidden) queueMicrotask(() => ta?.focus());
    wasHidden = hidden;
  });

  // ── sugerencia de Claude Code ──
  // Se ve en gris con la caja vacía y Tab la escribe. La que se acaba de enviar no vuelve a salir
  // mientras llega el siguiente estado (la sesión pasa a trabajar y luego trae otra nueva).
  let dismissed = $state<string | null>(null);
  $effect(() => {
    if (session.status !== 'idle') dismissed = null;
  });
  const ghost = $derived(
    mode === 'ready' && session.status === 'idle' && !text && suggestion && suggestion !== dismissed
      ? suggestion
      : null,
  );
  // Si es larga y ocupa varias líneas, la caja se ajusta a ella (y vuelve al quitarse).
  $effect(() => {
    ghost;
    queueMicrotask(autosize);
  });

  // ── autocompletar «/»: comandos de Claude Code, skills y comandos propios ──
  // Mientras la caja sólo tiene «/palabra». ↑↓ eligen, Tab completa, Enter completa y envía (como
  // en la TUI), Esc lo cierra hasta que cambie el texto. Lo que trae el historial no lo abre.
  let cmds = $state<SlashCmd[] | null>(null);
  let cmdsKey = '';
  let cmdsAt = 0;
  let menuSel = $state(0);
  let menuOff = $state(false);
  let menuEl = $state<HTMLDivElement | null>(null);
  const slashQ = $derived(/^\/[^\s]*$/.test(text) ? text.slice(1).toLowerCase() : null);
  $effect(() => {
    const q = slashQ;
    if (q === null || mode !== 'ready') return;
    untrack(() => {
      menuSel = 0;
      const k = session.key;
      // Se piden al abrir el menú (por si hay skills nuevos), no en cada tecla.
      if (cmds && cmdsKey === k && (q !== '' || Date.now() - cmdsAt < 3000)) return;
      cmdsKey = k;
      cmdsAt = Date.now();
      api
        .slashCommands(k)
        .then((r) => {
          if (session.key === k) cmds = r;
        })
        .catch((e) => {
          cmdsKey = '';
          onToast(errText(e));
        });
    });
  });
  const menu = $derived.by(() => {
    if (slashQ === null || menuOff || mode !== 'ready' || !cmds || cmdsKey !== session.key) return [];
    const q = slashQ;
    const byName = (a: SlashCmd, b: SlashCmd) => a.name.localeCompare(b.name);
    const starts = cmds.filter((c) => c.name.toLowerCase().startsWith(q)).sort(byName);
    const inside = q ? cmds.filter((c) => !c.name.toLowerCase().startsWith(q) && c.name.toLowerCase().includes(q)).sort(byName) : [];
    return [...starts, ...inside];
  });
  function moveSel(d: number) {
    menuSel = (menuSel + d + menu.length) % menu.length;
    tick().then(() => menuEl?.querySelector('.sel')?.scrollIntoView({ block: 'nearest' }));
  }
  function pickCmd(c: SlashCmd, go: boolean) {
    if (go) {
      text = '/' + c.name;
      send();
      // Los que abren un diálogo (selectores, paneles) se ven en la terminal.
      if (c.interactive) onShowTerminal();
    } else {
      showEntry('/' + c.name + ' ');
      ta?.focus();
    }
  }

  // ── historial ↑/↓ (el de la TUI: ~/.claude/history.jsonl del proyecto) ──
  let hist: string[] = [];
  let hIdx = -1;
  let hDraft = '';
  let navigating = false;
  function resetHistory() {
    hIdx = -1;
    hist = [];
  }
  function caretOnFirstLine(): boolean {
    return !!ta && !text.slice(0, ta.selectionStart).includes('\n');
  }
  function caretOnLastLine(): boolean {
    return !!ta && !text.slice(ta.selectionEnd).includes('\n');
  }
  function showEntry(v: string) {
    navigating = true;
    setCanonical(v);
    queueMicrotask(() => {
      autosize();
      ta?.setSelectionRange(text.length, text.length);
      navigating = false;
    });
  }
  async function historyUp() {
    if (hIdx === -1) {
      hDraft = canonical();
      try {
        hist = await api.promptHistory(session.cwd);
      } catch (e) {
        onToast(errText(e));
        return;
      }
    }
    if (hIdx + 1 < hist.length) {
      hIdx += 1;
      showEntry(hist[hIdx]);
      // Una entrada «/comando» no abre el menú: si no, se quedaría con ↑↓ y no se podría seguir
      // por el historial. Vuelve en cuanto se edita (onInput).
      menuOff = true;
    }
  }
  function historyDown() {
    if (hIdx < 0) return;
    hIdx -= 1;
    showEntry(hIdx >= 0 ? hist[hIdx] : hDraft);
    menuOff = true;
  }
  function onInput() {
    menuOff = false;
    autosize();
    if (!navigating) hIdx = -1; // al editar, el siguiente ↑ vuelve a empezar desde el más reciente
    wake();
  }

  // Sesión detenida: la caja sigue admitiendo texto y lo primero que se escribe la reanuda. Se
  // pide una vez (el estado tarda un momento en cambiar); si falló, la siguiente tecla pasado un
  // rato lo vuelve a intentar.
  let wokeKey = '';
  let wokeAt = 0;
  function wake() {
    if (mode !== 'stopped') return;
    const k = session.key;
    if (wokeKey === k && Date.now() - wokeAt < 8000) return;
    wokeKey = k;
    wokeAt = Date.now();
    onAction('resume', session);
  }

  async function send() {
    const t = canonical();
    if (!t.trim() || sending) return;
    if (mode === 'starting' || mode === 'stopped' || (mode === 'ready' && chains.has(session.key))) {
      queue(t);
      return;
    }
    if (mode !== 'ready') return;
    sending = true;
    // La burbuja provisional aparece ya (y vuela desde aquí); la real la sustituye al llegar.
    // `/compact` a secas no deja mensaje en la conversación (se ve como la marca de compactada):
    // sin burbuja, que se quedaría esperando.
    const quiet = /^\/compact\s*$/.test(t.trim());
    const out = quiet ? null : outbox.add(session.key, t.trim(), ta?.getBoundingClientRect() ?? null);
    dismissed = suggestion;
    text = '';
    atts = [];
    drafts.delete(session.key);
    resetHistory();
    queueMicrotask(autosize);
    try {
      await api.sendInput(session.key, t);
    } catch (e) {
      if (out !== null) outbox.remove(out);
      setCanonical(t);
      queueMicrotask(autosize);
      onToast(errText(e));
    } finally {
      sending = false;
      ta?.focus();
    }
  }

  // ── envío en espera ──
  // Enter mientras la sesión arranca (o detenida, que así se reanuda): la burbuja sale ya, con la
  // marca de que espera, y el texto se teclea en cuanto Claude está listo. Van encadenados por
  // sesión (salen en orden, y lo que se envíe detrás espera su turno); si no llega a arrancar, el
  // texto vuelve a la caja.
  const chains = new Map<string, Promise<void>>();
  function queue(t: string) {
    const key = session.key;
    wake();
    const quiet = /^\/compact\s*$/.test(t.trim());
    const out = quiet ? null : outbox.add(key, t.trim(), ta?.getBoundingClientRect() ?? null, true);
    text = '';
    atts = [];
    drafts.delete(key);
    resetHistory();
    queueMicrotask(autosize);
    const link: Promise<void> = (chains.get(key) ?? Promise.resolve())
      .then(() => api.sendWhenReady(key, t))
      .then(
        () => {
          if (out !== null) outbox.sent(out);
        },
        (e) => {
          if (out !== null) outbox.remove(out);
          restore(key, t);
          onToast(errText(e));
        },
      )
      .finally(() => {
        if (chains.get(key) === link) chains.delete(key);
      });
    chains.set(key, link);
  }
  /** Lo que no llegó a enviarse vuelve a la caja de su sesión, delante de lo que haya. */
  function restore(key: string, t: string) {
    if (key === currentKey) {
      const now = canonical();
      setCanonical(now.trim() ? `${t}\n${now}` : t);
      queueMicrotask(autosize);
    } else {
      const d = drafts.get(key);
      drafts.set(key, d?.trim() ? `${t}\n${d}` : t);
    }
  }

  function stash() {
    if (!text.trim()) return;
    onStash(canonical().trim());
    text = '';
    atts = [];
    drafts.delete(session.key);
    resetHistory();
    queueMicrotask(autosize);
    ta?.focus();
  }

  function interrupt() {
    api.sendKeys(session.key, ['Escape']).catch((e) => onToast(errText(e)));
  }

  function onKeydown(e: KeyboardEvent) {
    if (menu.length && !e.altKey && !e.ctrlKey && !e.metaKey && !e.isComposing) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        moveSel(e.key === 'ArrowDown' ? 1 : -1);
        return;
      }
      if ((e.key === 'Tab' || e.key === 'Enter') && !e.shiftKey) {
        e.preventDefault();
        pickCmd(menu[menuSel] ?? menu[0], e.key === 'Enter');
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        menuOff = true;
        return;
      }
    }
    if (e.key === 'Enter' && e.altKey) {
      e.preventDefault();
      stash();
    } else if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    } else if (e.key === 'Tab' && ghost && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey) {
      e.preventDefault();
      showEntry(ghost);
      hIdx = -1;
    } else if (e.key === 'ArrowUp' && !e.shiftKey && !e.altKey && !e.ctrlKey && caretOnFirstLine()) {
      e.preventDefault();
      historyUp();
    } else if (e.key === 'ArrowDown' && !e.shiftKey && !e.altKey && !e.ctrlKey && hIdx >= 0 && caretOnLastLine()) {
      e.preventDefault();
      historyDown();
    } else if (e.key === 'Escape' && session.status === 'busy') {
      e.preventDefault();
      interrupt();
    }
  }

  function autosize() {
    if (!ta) return;
    // Para medir, la caja pasa un instante a 'auto' (su tamaño mínimo). Si la barra encogiera
    // con ella, la conversación ganaría alto en ese layout forzado y el navegador la bajaría
    // (scrollTop recortado) sin devolverla luego: se fija la altura de la barra mientras tanto.
    if (box) box.style.minHeight = box.offsetHeight + 'px';
    ta.style.height = 'auto';
    // La capa de la sugerencia lleva además el borde de la caja en su relleno (1 px arriba y abajo).
    const need = Math.max(ta.scrollHeight, ghostEl && !text ? ghostEl.scrollHeight - 2 : 0);
    ta.style.height = Math.min(need + 2, window.innerHeight * 0.4) + 'px';
    if (box) box.style.minHeight = '';
  }

  export function focus() {
    ta?.focus();
  }

  /**
   * Ficheros soltados o imágenes pegadas: las imágenes entran como marca con su miniatura (al
   * enviar, su ruta); lo demás, como ruta escrita. En el orden en que llegan.
   */
  export function attach(paths: string[]): boolean {
    if (!ta || mode === 'external' || !paths.length) return false;
    let list = atts.filter((a) => text.includes(a.token));
    const parts = paths.map((p) => {
      if (!isImagePath(p)) return quotePaths([p]);
      const r = addAttachment(list, p);
      list = r.atts;
      return r.att.token;
    });
    const start = ta.selectionStart ?? text.length;
    const ins = insertInto(text, start, ta.selectionEnd ?? start, parts.join(' '));
    // Una pegada en medio de otras: se renumeran por orden («imagen 1» es la primera del texto).
    const fromEnd = ins.value.length - ins.caret;
    const r = renumber(ins.value, list);
    text = r.text;
    atts = r.atts;
    hIdx = -1;
    wake();
    const caret = text.length - fromEnd;
    queueMicrotask(() => {
      autosize();
      ta?.focus();
      ta?.setSelectionRange(caret, caret);
    });
    return true;
  }

  function unattach(path: string) {
    const a = atts.find((x) => x.path === path);
    if (!a) return;
    const r = renumber(removeToken(text, a.token), atts);
    text = r.text;
    atts = r.atts;
    queueMicrotask(() => {
      autosize();
      ta?.focus();
    });
  }

  // Imagen pegada: se guarda en disco y entra como marca con su miniatura (Claude recibe la ruta).
  function onPaste(e: ClipboardEvent) {
    const p = pastedImage(e);
    p?.then((path) => attach([path])).catch((err) => onToast(errText(err)));
  }
</script>

<div class="composer" class:term-hidden={hidden} bind:this={box}>
  {#if mode === 'external'}
    <div class="banner center">
      Sesión externa: sólo lectura.
      {#if session.tmuxPane}
        <button onclick={() => onAction('adopt', session)}>Adoptar en ccdesk</button>
      {/if}
    </div>
  {:else}
    {#if mode === 'attention'}
      <div class="banner warn">
        Claude espera tu respuesta en la terminal.
        <button onclick={onShowTerminal}>Ir a la terminal</button>
      </div>
    {/if}
    {#if shown.length}
      <div class="attach-row">
        <MediaStrip files={shown.map((a) => ({ path: a.path, label: a.label }))} small onRemove={unattach} />
      </div>
    {/if}
    <div class="input-row">
      <div class="ta-wrap">
        {#if menu.length}
          <div class="slash-menu" bind:this={menuEl} role="listbox">
            {#each menu as c, i (c.name)}
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <div
                class="slash-item"
                class:sel={i === menuSel}
                role="option"
                tabindex="-1"
                aria-selected={i === menuSel}
                onmousedown={(e) => {
                  e.preventDefault();
                  pickCmd(c, false);
                }}
                onmousemove={() => (menuSel = i)}
              >
                <span class="slash-name">/{c.name}</span>
                <span class="slash-desc">{c.description}</span>
                {#if c.kind !== 'builtin'}<span class="slash-tag">{c.kind === 'skill' ? 'skill' : 'comando'}</span>{/if}
                {#if c.interactive}<span class="slash-tag">terminal</span>{/if}
              </div>
            {/each}
          </div>
        {/if}
        <textarea
          bind:this={ta}
          bind:value={text}
          oninput={onInput}
          onkeydown={onKeydown}
          onpaste={onPaste}
          rows="1"
          spellcheck="false"
          class:stopped={mode === 'stopped'}
          placeholder={mode === 'stopped'
            ? 'Sesión detenida (no ocupa memoria)'
            : mode === 'starting'
            ? 'Arrancando Claude…'
            : ghost
              ? ''
              : 'Escribe a Claude…  (Enter envía · Mayús+Enter nueva línea · ↑↓ historial · Esc interrumpe)'}
          disabled={mode === 'attention'}
        ></textarea>
        <!-- La sugerencia va encima de la caja (un placeholder no admite el dibujo de la tecla). -->
        {#if ghost}
          <div class="ghost-text" bind:this={ghostEl} aria-hidden="true">
            {ghost}<kbd class="key"><span class="key-arrow">⇥</span> Tab</kbd>
          </div>
        {/if}
      </div>
      {#if session.status === 'busy'}
        <button class="stopbtn" title="Interrumpir (Esc)" onclick={interrupt}>■</button>
      {/if}
      <button
        class="stash"
        disabled={!text.trim()}
        title="Guardar como to-do, sin enviar (Alt+Enter)"
        aria-label="Guardar como to-do"
        onclick={stash}><Icon name="todo" size={18} /></button
      >
      <button
        class="primary send"
        disabled={!text.trim() || !['ready', 'starting', 'stopped'].includes(mode) || sending}
        title="Enviar (Enter)"
        aria-label="Enviar"
        onclick={send}><Icon name="send" size={18} /></button
      >
    </div>
  {/if}
</div>
