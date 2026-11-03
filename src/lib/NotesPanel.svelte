<script lang="ts">
  // Panel derecho: to-dos apuntados para lanzarlos a la sesión cuando convenga.
  import { api, ago, errText, ATTENTION, type Note, type SessionView } from './api';
  import { notes } from './notes.svelte';
  import { outbox } from './outbox.svelte';
  import { insertInto, pastedImage, quotePaths } from './files';

  interface Props {
    session: SessionView;
    now: number;
    onClose: () => void;
    onToast: (m: string) => void;
    ask: (msg: string, ok?: string) => Promise<boolean>;
  }
  let { session, now, onClose, onToast, ask }: Props = $props();

  $effect(() => {
    notes.load(session.key);
  });

  const canSend = $derived(
    session.managed &&
      !['stopped', 'starting'].includes(session.status) &&
      !ATTENTION.includes(session.status),
  );
  const why = $derived(
    !session.managed
      ? 'Sesión externa: adóptala para poder enviarle to-dos'
      : session.status === 'stopped'
        ? 'Sesión detenida: reanúdala para enviar'
        : ATTENTION.includes(session.status)
          ? 'Claude espera una respuesta en la terminal'
          : '',
  );

  // Tarjetas que están volando a la conversación (se ocultan; si el envío falla, vuelven).
  let leaving = $state<string[]>([]);

  async function send(n: Note, card: HTMLElement | null) {
    const text = n.text.trim();
    if (!text || !canSend || leaving.includes(n.id)) return;
    if (
      session.status === 'busy' &&
      !(await ask(
        'Claude está trabajando: el to-do entrará en cola y lo leerá a mitad de lo que está haciendo. ¿Enviarlo igualmente?',
        'Enviar',
      ))
    )
      return;
    // Igual que desde la caja: la burbuja provisional sale volando desde la tarjeta.
    const out = outbox.add(session.key, text, card?.getBoundingClientRect() ?? null);
    leaving = [...leaving, n.id];
    try {
      await api.sendInput(session.key, text);
      if (card) await collapse(card);
      notes.remove(n.id);
    } catch (e) {
      outbox.remove(out);
      onToast(errText(e));
    } finally {
      leaving = leaving.filter((x) => x !== n.id);
    }
  }

  /** El hueco de la tarjeta enviada se cierra poco a poco (incluido el espacio entre tarjetas). */
  function collapse(el: HTMLElement): Promise<void> {
    const h = el.getBoundingClientRect().height;
    const gap = parseFloat(getComputedStyle(el.parentElement!).rowGap) || 0;
    el.style.overflow = 'hidden';
    const a = el.animate(
      [
        { height: `${h}px`, marginBottom: '0px' },
        { height: '0px', paddingTop: '0px', paddingBottom: '0px', borderWidth: '0px', marginBottom: `${-gap}px` },
      ],
      { duration: 200, easing: 'cubic-bezier(0.33, 1, 0.68, 1)', fill: 'forwards' },
    );
    return a.finished.then(
      () => {},
      () => {},
    );
  }

  async function del(n: Note) {
    if (n.text.trim() && !(await ask('¿Borrar este to-do?', 'Borrar'))) return;
    notes.remove(n.id);
  }

  // Imagen pegada en una tarjeta: se guarda en disco y su ruta entra en el to-do.
  function onPaste(e: ClipboardEvent, n: Note) {
    const el = e.currentTarget as HTMLTextAreaElement;
    const p = pastedImage(e);
    p?.then((path) => {
      const r = insertInto(el.value, el.selectionStart, el.selectionEnd, quotePaths([path]));
      notes.update(n.id, r.value);
      el.value = r.value;
      el.setSelectionRange(r.caret, r.caret);
      grow(el);
    }).catch((err) => onToast(errText(err)));
  }

  function grow(el: HTMLTextAreaElement) {
    el.style.height = 'auto';
    el.style.height = Math.min(el.scrollHeight + 2, 420) + 'px';
  }

  function setup(el: HTMLTextAreaElement, id: string) {
    queueMicrotask(() => grow(el));
    if (notes.fresh === id) {
      el.focus();
      notes.fresh = null;
    }
  }
</script>

<aside class="notes-panel">
  <header>
    <span class="np-title">To-do</span>
    {#if notes.list.length}<span class="np-count">{notes.list.length}</span>{/if}
    <button class="icon" title="Ocultar (Ctrl+Mayús+O)" onclick={onClose}>✕</button>
  </header>
  <div class="np-list">
    {#if !notes.list.length}
      <p class="np-empty">
        Apunta aquí lo que se te ocurra para esta sesión sin interrumpir lo que está haciendo, y
        envíalo cuando te venga bien. También puedes mandar aquí lo que tengas escrito en la caja
        con «To-do» (Alt+Enter).
      </p>
    {/if}
    {#each notes.list as n (n.id)}
      <div class="note-card" class:leaving={leaving.includes(n.id)} data-note={n.id}>
        <textarea
          value={n.text}
          placeholder="Escribe el to-do…"
          spellcheck="false"
          use:setup={n.id}
          oninput={(e) => {
            notes.update(n.id, e.currentTarget.value);
            grow(e.currentTarget);
          }}
          onpaste={(e) => onPaste(e, n)}
          onkeydown={(e) => {
            if (e.key === 'Enter' && e.ctrlKey) {
              e.preventDefault();
              send(n, e.currentTarget.closest('.note-card'));
            }
          }}
        ></textarea>
        <div class="note-actions">
          <span class="note-age">{ago(n.created, now)}</span>
          <button class="small" onclick={() => del(n)}>Borrar</button>
          <button
            class="small primary"
            disabled={!canSend || !n.text.trim()}
            title={why || 'Enviar a la sesión (Ctrl+Enter)'}
            onclick={(e) => send(n, e.currentTarget.closest('.note-card'))}>Enviar ▶</button
          >
        </div>
      </div>
    {/each}
    <button class="note-new" onclick={() => notes.add()}>+ Nuevo to-do</button>
  </div>
</aside>
