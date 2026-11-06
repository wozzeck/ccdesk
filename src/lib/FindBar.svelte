<script module lang="ts">
  /** Lo último buscado (en cualquier conversación): con eso se vuelve a abrir la barra. */
  let last = '';
</script>

<script lang="ts">
  // Barra de Ctrl+F: busca en la conversación abierta, de la coincidencia más reciente a la más
  // antigua (Intro, ↑) y al revés (Mayús+Intro, ↓). Abre los bloques plegados donde cae la actual
  // y, al pasar de la más antigua cargada, carga lo anterior hasta la siguiente: el servidor dice en
  // qué items de lo no cargado sale.
  // Resalta con la API de resaltado de CSS, pero sólo lo que está a la vista (y una pantalla por
  // arriba y por abajo): en WebKitGTK cada rango resaltado encarece todo el pintado, y más los que
  // caen en bloques plegados; con unos miles («w» en una sesión larga), la ventana se quedaba
  // congelada minutos.
  import { onMount } from 'svelte';
  import { api, type FindHit } from './api';
  import Icon from './Icon.svelte';
  import { findAll, indexText, matchAt, place, type Match } from './find';

  interface Props {
    /** La lista de mensajes, donde se busca. */
    root: HTMLElement;
    /** Lo que se desplaza. */
    scroller: HTMLElement;
    sessionKey: string;
    /** Primer item cargado: lo anterior sólo lo tiene el servidor. */
    startIndex: number;
    /** Lo seleccionado en la conversación (si no hay nada, se abre con lo último buscado). */
    initial: string;
    /** Abierta desde un resultado de la búsqueda de sesiones: el item de su mensaje. Se carga hasta
     *  él si hace falta y se empieza en su primera coincidencia (si no tiene, como siempre). */
    startAt: number | null;
    /** Panel del subagente abierto: la barra va a su izquierda. */
    beside: boolean;
    /** Carga lo anterior hasta el item `index`. */
    loadOlder: (index: number) => Promise<void>;
    /** Antes de mover la vista hasta una coincidencia. */
    onNav: () => void;
    onClose: () => void;
  }
  let { root, scroller, sessionKey, startIndex, initial, startAt, beside, loadOlder, onNav, onClose }: Props =
    $props();

  const CAP = 10000;
  /** Items de más que se cargan por encima de una coincidencia que estaba sin cargar. */
  const CONTEXT = 20;
  /** Resaltadas a la vez, como mucho. */
  const PAINT_MAX = 300;
  const HL = 'ccdesk-find';
  const HL_CUR = 'ccdesk-find-cur';

  let bar: HTMLDivElement;
  let input: HTMLInputElement;
  // svelte-ignore state_referenced_locally
  let query = $state(initial || last); // sólo al abrir
  /** La consulta de lo que hay en `matches`. */
  let searched = $state('');
  /** En orden del documento: de la más antigua a la más reciente. */
  let matches: Match[] = [];
  let count = $state(0);
  let capped = $state(false);
  /** La actual (posición en `matches`). */
  let cur = $state(-1);
  /** La actual como Range vivo: sigue en su sitio aunque cambie el DOM, y de ahí se parte al
   *  volver a buscar. */
  let curRange: Range | null = null;
  /** Dónde sale en lo no cargado, del item más reciente al más antiguo. */
  let older = $state<FindHit[]>([]);
  let loading = $state(false);
  /** Bloques plegados que se abrieron para enseñar la actual. */
  const opened = new Set<HTMLDetailsElement>();
  /** Para medir y comparar sin crear un Range por coincidencia. */
  const probe = document.createRange();
  const mark = document.createRange();

  // Con más de CAP en lo cargado sólo se recorren las CAP más recientes: lo anterior, ni contado.
  const pending = $derived(capped ? [] : older.filter((h) => h.index < startIndex));
  const total = $derived(count + pending.reduce((a, h) => a + h.count, 0));
  // Numeradas de la más reciente (1) a la más antigua, como se recorren.
  const label = $derived(
    loading
      ? 'Cargando…'
      : !searched.trim()
        ? ''
        : total === 0
          ? 'Sin resultados'
          : `${cur < 0 ? '?' : count - cur} de ${total}${capped ? '+' : ''}`,
  );

  // Se busca al dejar de escribir un momento (lo primero, al abrir, sin esperar).
  let first = true;
  $effect(() => {
    const q = query;
    if (q === searched) return;
    const t = window.setTimeout(run, first ? 0 : 120);
    first = false;
    return () => clearTimeout(t);
  });

  // svelte-ignore state_referenced_locally
  let aim = startAt; // sólo en la primera búsqueda

  /** Busca lo escrito; la actual pasa a la más próxima a la que había (o a lo que se ve). */
  function run() {
    if (aim !== null) {
      const at = aim;
      aim = null;
      jump(at);
      return;
    }
    const anchor = searched.trim() ? curRange : null;
    research(query, anchor ? near(anchor) : nearView);
    searched = last = query;
    show(true);
    fetchOlder(query);
  }

  /** A la primera coincidencia del mensaje del item `at`, cargando antes lo anterior hasta él si
   *  no lo estaba (si no tiene ninguna, como al abrir). */
  async function jump(at: number) {
    loading = true;
    searched = last = query;
    onNav();
    try {
      if (at < startIndex) await loadOlder(Math.max(0, at - CONTEXT));
      const row = rowOf(at);
      research(searched, row ? within(row) : nearView);
      show(true);
    } finally {
      loading = false;
    }
    fetchOlder(searched);
  }

  /** La fila del item `index`: la suya o, si va junto con los anteriores (textos seguidos de
   *  Claude), la del primero. */
  function rowOf(index: number): Element | null {
    let row: Element | null = null;
    for (const el of root.querySelectorAll<HTMLElement>('[data-i]')) {
      if (Number(el.dataset.i) > index) break;
      row = el;
    }
    return row;
  }

  /** Busca `q` en lo cargado; `choose` elige la actual. No pinta (eso es `show`). */
  function research(q: string, choose: (list: Match[]) => number) {
    const ix = indexText(root);
    const found = findAll(ix, q, CAP);
    const list = found.hits.map(([s, e]) => matchAt(ix, s, e));
    capped = found.capped;
    cur = list.length ? choose(list) : -1;
    matches = list;
    count = list.length;
    others = new Highlight();
    painted = new Map();
    CSS.highlights.set(HL, others);
  }

  /** Última de `list` que empieza antes que `a` (o donde `a`, con `same`); −1 si ninguna. */
  function before(list: Match[], a: Range, same: boolean): number {
    let k = -1;
    try {
      mark.setStart(a.startContainer, a.startOffset);
      mark.collapse(true);
      for (let lo = 0, hi = list.length - 1; lo <= hi; ) {
        const m = (lo + hi) >> 1;
        const c = mark.comparePoint(list[m].sn, list[m].so); // −1: antes; 0: en el mismo sitio
        if (c < 0 || (same && c === 0)) {
          k = m;
          lo = m + 1;
        } else hi = m - 1;
      }
    } catch {
      return -1;
    }
    return k;
  }

  /** A partir de `k`, mejor una que se vea: abrir bloques plegados sólo para enseñar la primera
   *  haría saltar la vista a cada letra que se escribe. Las de dentro se ven al ir con Intro. */
  function preferShown(list: Match[], k: number): number {
    for (let j = k; j >= 0; j--) if (!hiddenIn(list[j])) return j;
    for (let j = k + 1; j < list.length; j++) if (!hiddenIn(list[j])) return j;
    return k;
  }
  /** La que empieza donde `a` o la anterior más próxima (o la primera, si no hay anterior). */
  const near = (a: Range) => (list: Match[]) => preferShown(list, Math.max(0, before(list, a, true)));

  /** La primera dentro de `el` (si no hay ninguna, `nearView`). */
  const within = (el: Element) => (list: Match[]) => {
    const k = list.findIndex((m) => el.contains(m.sn));
    return k >= 0 ? k : nearView(list);
  };

  /** La más reciente de las que quedan por encima del final de la vista (o la primera de debajo). */
  function nearView(list: Match[]): number {
    const bottom = scroller.getBoundingClientRect().bottom - 40;
    let k = -1;
    for (let lo = 0, hi = list.length - 1; lo <= hi; ) {
      const m = (lo + hi) >> 1;
      if (topOf(list[m]) <= bottom) {
        k = m;
        lo = m + 1;
      } else hi = m - 1;
    }
    return preferShown(list, Math.max(0, k));
  }

  function rectOf(m: Match): DOMRect | null {
    if (!place(probe, m)) return null;
    const b = probe.getBoundingClientRect();
    return b.width || b.height ? b : null;
  }
  /** Altura de `m` en pantalla (la de su bloque, si está plegado). */
  function topOf(m: Match): number {
    const d = hiddenIn(m);
    if (d) return d.getBoundingClientRect().top;
    return rectOf(m)?.top ?? m.sn.parentElement?.getBoundingClientRect().top ?? 0;
  }
  const summaryOf = (d: HTMLDetailsElement) => d.querySelector(':scope > summary');
  /** El bloque plegado más externo que esconde `m` (null si se ve). */
  function hiddenIn(m: Match): HTMLDetailsElement | null {
    let out: HTMLDetailsElement | null = null;
    for (let el = m.sn.parentElement; el && el !== root; el = el.parentElement) {
      if (el instanceof HTMLDetailsElement && !el.open && !summaryOf(el)?.contains(m.sn)) out = el;
    }
    return out;
  }

  // ── pintado: la actual y, aparte, las demás que están a la vista ──
  let others: Highlight | null = null;
  /** Coincidencia (posición en `matches`) → su Range en `others`. */
  let painted = new Map<number, Range>();

  /** Enseña la actual (desplazando hasta ella si `scroll`) y pinta lo que se ve. */
  function show(scroll: boolean) {
    if (cur >= 0 && scroll) reveal(matches[cur]);
    paintCur();
    paintVisible();
  }

  function paintCur() {
    curRange = null;
    if (cur >= 0) {
      const r = document.createRange();
      if (place(r, matches[cur])) curRange = r;
    }
    if (curRange && !hiddenIn(matches[cur])) CSS.highlights.set(HL_CUR, new Highlight(curRange));
    else CSS.highlights.delete(HL_CUR);
  }

  /** Las demás que están a la vista, o a una pantalla de distancia. Las de los bloques plegados no
   *  se pintan: no se ven y son las que más le cuestan a WebKitGTK. */
  function paintVisible() {
    if (!others) return;
    const sc = scroller.getBoundingClientRect();
    const top = sc.top - sc.height;
    const bottom = sc.bottom + sc.height;
    let lo = 0;
    for (let hi = matches.length; lo < hi; ) {
      const m = (lo + hi) >> 1;
      if (topOf(matches[m]) < top) lo = m + 1;
      else hi = m;
    }
    const want = new Set<number>();
    for (let k = lo; k < matches.length && want.size < PAINT_MAX; k++) {
      const m = matches[k];
      const d = hiddenIn(m);
      if (d) {
        if (d.getBoundingClientRect().top > bottom) break;
        continue;
      }
      const b = rectOf(m);
      if (b && b.top > bottom) break;
      if (b && k !== cur) want.add(k);
    }
    for (const [k, r] of painted) {
      if (!want.has(k)) {
        others.delete(r);
        painted.delete(k);
      }
    }
    for (const k of want) {
      if (painted.has(k)) continue;
      const r = document.createRange();
      if (!place(r, matches[k])) continue;
      painted.set(k, r);
      others.add(r);
    }
  }

  let seq = 0;
  async function fetchOlder(q: string) {
    const n = ++seq;
    let r: FindHit[] = [];
    if (q.trim() && startIndex > 0) {
      try {
        r = await api.transcriptFind(sessionKey, q, startIndex);
      } catch {
        /* sin respuesta: sólo cuenta lo cargado */
      }
    }
    if (n === seq) older = r;
  }

  /** −1: a la anterior (más antigua); 1: a la siguiente (más reciente). Dan la vuelta. */
  async function go(dir: -1 | 1) {
    if (loading) return;
    if (query !== searched) return run(); // lo último escrito aún no se ha buscado
    if (dir < 0) {
      if (cur > 0) select(cur - 1);
      else if (pending.length) await more();
      else if (count) select(count - 1);
    } else if (count) select(cur < count - 1 ? cur + 1 : 0);
  }

  function select(k: number) {
    cur = k;
    show(true);
  }

  /** Carga lo anterior hasta la coincidencia siguiente a la más antigua de lo cargado. */
  async function more() {
    loading = true;
    onNav();
    try {
      let oldest: Range | null = null;
      if (matches.length) {
        oldest = document.createRange();
        if (!place(oldest, matches[0])) oldest = null;
      }
      while (pending.length) {
        const from = startIndex;
        // Con unos cuantos mensajes más por encima: contexto, y sitio para centrarla.
        await loadOlder(Math.max(0, pending[0].index - CONTEXT));
        if (startIndex >= from) break; // no se cargó nada (otra sesión, error…)
        const anchor = oldest;
        research(searched, (list) => (anchor ? before(list, anchor, false) : list.length - 1));
        if (cur >= 0) {
          show(true);
          return;
        }
        // Lo que contó el servidor no se ve (la dirección de un enlace, p. ej.): a la siguiente.
      }
      if (count) select(count - 1); // nada más atrás: vuelta a la más reciente
      else show(false);
    } finally {
      loading = false;
    }
  }

  /** Enseña `m`: abre los bloques plegados que la esconden (y cierra los que se abrieron para
   *  otra) y desplaza lo justo para que se vea, centrada si estaba fuera. */
  function reveal(m: Match) {
    onNav();
    for (const d of opened) {
      if (!d.contains(m.sn)) {
        d.open = false;
        opened.delete(d);
      }
    }
    for (let el = m.sn.parentElement; el && el !== root; el = el.parentElement) {
      if (el instanceof HTMLDetailsElement && !el.open && !summaryOf(el)?.contains(m.sn)) {
        el.open = true;
        opened.add(el);
      }
    }
    // De dentro afuera: bloques con barra propia (código, salidas largas) y luego la conversación,
    // cuya parte de arriba tapa la barra y la de abajo, el aviso de «trabajando».
    for (let el = m.sn.parentElement; el; el = el.parentElement) {
      if (el === scroller) {
        bring(el, m, bar.offsetHeight + 16, 44);
        break;
      }
      if (scrolls(el)) bring(el, m, 6, 6);
    }
  }

  function scrolls(el: HTMLElement): boolean {
    if (el.scrollHeight <= el.clientHeight + 1 && el.scrollWidth <= el.clientWidth + 1) return false;
    const s = getComputedStyle(el);
    return /auto|scroll/.test(s.overflowY + s.overflowX);
  }

  function bring(el: HTMLElement, m: Match, mt: number, mb: number) {
    const b = rectOf(m);
    if (!b) return;
    const box = el.getBoundingClientRect();
    const top = box.top + el.clientTop + mt;
    const bottom = box.top + el.clientTop + el.clientHeight - mb;
    if (el.scrollHeight > el.clientHeight + 1 && (b.top < top || b.bottom > bottom)) {
      el.scrollTop += b.top + b.height / 2 - (top + bottom) / 2;
    }
    const left = box.left + el.clientLeft + 6;
    const right = box.left + el.clientLeft + el.clientWidth - 6;
    if (el.scrollWidth > el.clientWidth + 1 && (b.left < left || b.right > right)) {
      el.scrollLeft += b.left + b.width / 2 - (left + right) / 2;
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === 'Enter' || e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault();
      go(e.key === 'ArrowDown' || (e.key === 'Enter' && e.shiftKey) ? 1 : -1);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
  }

  /** Ctrl+F con la barra ya abierta: al cuadro (con el texto seleccionado, si lo hay). */
  export function focus(text?: string) {
    if (text) query = text;
    input.focus();
    input.select();
  }

  /** La conversación se ha vuelto a cargar entera (la vista queda al final). */
  export function reloaded() {
    if (!searched.trim()) return;
    research(searched, nearView);
    show(false);
    fetchOlder(searched);
  }

  onMount(() => {
    // Al cerrar, las propiedades ya no se pueden leer (la barra se quita porque ya no hay búsqueda).
    const sc = scroller;
    input.focus();
    input.select();
    // La conversación cambia (llega texto, se despliega algo…): se vuelve a buscar, sin mover la vista.
    let t = 0;
    const mo = new MutationObserver(() => {
      if (t) return;
      t = window.setTimeout(() => {
        t = 0;
        if (loading || !searched.trim()) return;
        research(searched, curRange ? near(curRange) : nearView);
        show(false);
      }, 250);
    });
    mo.observe(root, { childList: true, subtree: true, characterData: true });
    // Al desplazar (la conversación o un bloque con barra propia), lo que entra en la vista.
    let frame = 0;
    const onScroll = () => {
      if (!frame) {
        frame = requestAnimationFrame(() => {
          frame = 0;
          paintVisible();
        });
      }
    };
    sc.addEventListener('scroll', onScroll, { capture: true, passive: true });
    window.addEventListener('resize', onScroll);
    return () => {
      mo.disconnect();
      clearTimeout(t);
      cancelAnimationFrame(frame);
      sc.removeEventListener('scroll', onScroll, { capture: true });
      window.removeEventListener('resize', onScroll);
      CSS.highlights.delete(HL);
      CSS.highlights.delete(HL_CUR);
    };
  });
</script>

<!-- Los botones no se quedan con el foco: el cursor sigue en el cuadro. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="findbar"
  class:beside
  class:none={!!searched.trim() && total === 0 && !loading}
  bind:this={bar}
  onmousedown={(e) => (e.target as Element).closest('button') && e.preventDefault()}
>
  <input
    bind:this={input}
    bind:value={query}
    placeholder="Buscar en la conversación"
    aria-label="Buscar en la conversación"
    spellcheck="false"
    onkeydown={onKey}
  />
  <span class="fcount">{label}</span>
  <button class="icon" title="Más antigua (Intro o ↑)" aria-label="Más antigua" disabled={!total} onclick={() => go(-1)}
    ><Icon name="up" size={16} /></button
  >
  <button class="icon" title="Más reciente (Mayús+Intro o ↓)" aria-label="Más reciente" disabled={!count} onclick={() => go(1)}
    ><Icon name="down" size={16} /></button
  >
  <button class="icon" title="Cerrar (Esc)" aria-label="Cerrar" onclick={onClose}>✕</button>
</div>
