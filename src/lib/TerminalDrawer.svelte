<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import '@xterm/xterm/css/xterm.css';
  import { api, Channel, errText, type SessionView } from './api';
  import { copyPlain } from './clipboard';

  interface Props {
    session: SessionView | null;
    open: boolean;
    onClose: () => void;
  }
  let { session, open, onClose }: Props = $props();

  let host: HTMLDivElement;
  let term: Terminal | null = null;
  let fit: FitAddon;
  let attached: string | null = null;
  let chan: Channel<string> | null = null;
  let seq = 0;

  function cssVar(name: string, fallback: string): string {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
  }

  function safeFit() {
    try {
      if (term && open && host.clientHeight > 20) fit.fit();
    } catch {
      /* contenedor oculto */
    }
  }

  const termTheme = () => ({
    background: cssVar('--term-bg', '#16161a'),
    foreground: cssVar('--term-fg', '#e6e3de'),
  });

  // xterm.js se crea la primera vez que se abre la terminal (ahorra memoria si no se usa).
  function ensureTerm(): Terminal {
    if (term) return term;
    term = new Terminal({
      fontFamily: '"JetBrains Mono", "DejaVu Sans Mono", "Liberation Mono", monospace',
      fontSize: 13,
      scrollback: 1000,
      cursorBlink: false,
      theme: termTheme(),
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    term.onData((d) => {
      if (attached) api.termWrite(d).catch(() => {});
    });
    term.onResize(({ cols, rows }) => {
      if (attached) api.termResize(cols, rows).catch(() => {});
    });
    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== 'keydown' || !e.ctrlKey || !e.shiftKey) return true;
      if (e.code === 'KeyC') {
        const s = term!.getSelection();
        if (s) copyPlain(s);
        return false;
      }
      if (e.code === 'KeyV') {
        navigator.clipboard
          ?.readText?.()
          .then((t) => t && term!.paste(t))
          .catch(() => {});
        return false;
      }
      return true;
    });
    return term;
  }

  onMount(() => {
    const ro = new ResizeObserver(() => {
      if (term) safeFit();
    });
    ro.observe(host);
    // Al cambiar de tema, la terminal toma sus colores.
    const mo = new MutationObserver(() => {
      if (term) term.options.theme = termTheme();
    });
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    const un = listen<{ key: string; gen: number }>('term-exit', (e) => {
      // Sólo el pty vigente: el cierre del anterior puede llegar después de abrir el siguiente.
      if (e.payload.key === attached && e.payload.gen === seq) {
        attached = null;
        term?.write('\r\n\x1b[2m[desconectado]\x1b[0m\r\n');
      }
    });
    return () => {
      ro.disconnect();
      mo.disconnect();
      un.then((f) => f());
      api.termClose().catch(() => {});
      term?.dispose();
    };
  });

  const want = $derived(
    open && session && session.managed && session.status !== 'stopped' ? session.key : null,
  );

  $effect(() => {
    const w = want;
    if (w !== attached) attach(w);
  });

  async function attach(key: string | null) {
    const my = ++seq;
    if (chan) chan.onmessage = () => {};
    chan = null;
    if (attached) {
      attached = null;
      await api.termClose().catch(() => {});
    }
    if (my !== seq) return;
    term?.reset();
    if (!key) return;
    const t = ensureTerm();
    await tick();
    safeFit();
    if (my !== seq) return;
    const c = new Channel<string>();
    c.onmessage = (d) => {
      if (chan === c) t.write(d);
    };
    chan = c;
    attached = key;
    try {
      await api.termOpen(key, t.cols, t.rows, c, my);
      t.focus();
    } catch (e) {
      if (chan === c) {
        attached = null;
        t.write(`\x1b[31m${errText(e)}\x1b[0m\r\n`);
      }
    }
  }

  export function focus() {
    term?.focus();
  }
</script>

<div class="drawer" class:open>
  <div class="drawer-head">
    <span class="dtitle">Terminal{session ? ' · ' + session.title : ''}</span>
    <span class="dhint">Ctrl+º alterna · Ctrl+Mayús+C copia</span>
    <button class="icon" title="Cerrar (Ctrl+º)" onclick={onClose}>✕</button>
  </div>
  <div class="term-host" bind:this={host}></div>
</div>
