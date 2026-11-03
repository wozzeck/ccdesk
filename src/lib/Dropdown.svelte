<script lang="ts">
  // Desplegable propio (el <select> de WebKitGTK usa el estilo nativo de GTK y no se deja
  // ajustar a una barra de 22 px). El menú se abre hacia arriba: vive en la barra inferior.
  import { onMount } from 'svelte';

  interface Option {
    v: string;
    l: string;
    cls?: string;
  }
  interface Props {
    value: string;
    options: Option[];
    disabled?: boolean;
    title?: string;
    placeholder?: string;
    cls?: string;
    /** Etiqueta del botón cuando el valor no está (aún) entre las opciones. */
    label?: string;
    onChange: (v: string) => void;
    /** Se llama al abrir (p. ej. para cargar las opciones reales). */
    onOpen?: () => Promise<void> | void;
  }
  let {
    value,
    options,
    disabled = false,
    title = '',
    placeholder = '—',
    cls = '',
    label = '',
    onChange,
    onOpen,
  }: Props = $props();

  let open = $state(false);
  let loading = $state(false);

  async function toggle() {
    open = !open;
    if (open && onOpen) {
      loading = true;
      try {
        await onOpen();
      } finally {
        loading = false;
      }
    }
  }
  let root: HTMLSpanElement;
  const current = $derived(options.find((o) => o.v === value));

  function pick(v: string) {
    open = false;
    if (v !== value) onChange(v);
  }

  onMount(() => {
    const close = (e: MouseEvent) => {
      if (open && !root.contains(e.target as Node)) open = false;
    };
    const esc = (e: KeyboardEvent) => {
      if (open && e.key === 'Escape') {
        e.stopPropagation();
        open = false;
      }
    };
    window.addEventListener('mousedown', close, true);
    window.addEventListener('keydown', esc, true);
    return () => {
      window.removeEventListener('mousedown', close, true);
      window.removeEventListener('keydown', esc, true);
    };
  });
</script>

<span class="dd" bind:this={root}>
  <button
    type="button"
    class="dd-btn {cls} {current?.cls ?? ''}"
    class:open
    {disabled}
    {title}
    onclick={toggle}
  >
    {current?.l ?? (label || placeholder)}{#if !disabled}<span class="dd-caret">▾</span>{/if}
  </button>
  {#if open}
    <div class="dd-menu" role="listbox">
      {#if loading && !options.length}<span class="dd-loading">Leyendo opciones de Claude…</span>{/if}
      {#each options as o (o.v)}
        <button
          type="button"
          role="option"
          aria-selected={o.v === value}
          class="dd-opt {o.cls ?? ''}"
          class:sel={o.v === value}
          onclick={() => pick(o.v)}>{o.l}</button
        >
      {/each}
    </div>
  {/if}
</span>
