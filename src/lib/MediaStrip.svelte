<script module lang="ts">
  // Imágenes ya cargadas (URL `data:`), compartidas entre tiras: al volver a pintar la
  // conversación no se piden otra vez. Con tope, que cada una pesa cientos de KB.
  const loaded = new Map<string, string>();
  const MAX = 40;
  function remember(k: string, url: string) {
    loaded.set(k, url);
    if (loaded.size > MAX) loaded.delete(loaded.keys().next().value!);
  }
</script>

<script lang="ts">
  // Miniaturas de las imágenes de un mensaje o de un grupo de herramientas (fotos que leyó
  // Claude, capturas pegadas…). Se cargan al entrar en pantalla; al pulsar, se ven en grande.
  // `images`: guardadas en el transcript; `files`: ficheros del disco (las imágenes que adjuntas
  // en tus mensajes), con su nombre debajo. `small` y `onRemove`: en la caja de texto.
  import { onDestroy } from 'svelte';
  import { api, type ImgRef } from './api';

  interface FileImg {
    path: string;
    label: string;
  }
  let {
    images = [],
    files = [],
    small = false,
    onRemove,
  }: { images?: ImgRef[]; files?: FileImg[]; small?: boolean; onRemove?: (path: string) => void } = $props();

  interface Entry {
    key: string;
    label?: string;
    path?: string;
    fetch: () => Promise<string>;
  }
  const keyOf = (im: ImgRef) => `${im.path}|${im.offset}|${im.n}`;
  const MIME: Record<string, string> = { jpg: 'image/jpeg', jpeg: 'image/jpeg', svg: 'image/svg+xml' };
  async function fileUrl(path: string): Promise<string> {
    const buf = await api.imageFile(path);
    const ext = path.split('.').pop()!.toLowerCase();
    const blob = new Blob([buf], { type: MIME[ext] ?? `image/${ext}` });
    return new Promise((ok, ko) => {
      const r = new FileReader();
      r.onload = () => ok(String(r.result));
      r.onerror = () => ko(r.error);
      r.readAsDataURL(blob);
    });
  }
  const entries = $derived<Entry[]>([
    ...images.map((im) => ({ key: keyOf(im), fetch: () => api.transcriptImage(im.path, im.offset, im.n) })),
    ...files.map((f) => ({ key: `file|${f.path}`, label: f.label, path: f.path, fetch: () => fileUrl(f.path) })),
  ]);

  let urls = $state<Record<string, string>>({});
  let failed = $state<Record<string, boolean>>({});

  // Tamaño de cada miniatura con la proporción de la foto (sin recortar): 110 px de alto y el
  // ancho que le toque; si es muy panorámica, 260 px de ancho y menos alto. Se fija en cuanto
  // carga: si se deja al tamaño intrínseco, WebKit conserva el provisional (el del «…») y
  // deforma la miniatura hasta que algo le obliga a recalcular (p. ej. pasar el ratón).
  // En la caja de texto, más pequeñas.
  const H = $derived(small ? 56 : 110);
  const W = $derived(small ? 150 : 260);
  let ratio = $state<Record<string, number>>({});
  function measured(k: string, e: Event) {
    const img = e.currentTarget as HTMLImageElement;
    if (img.naturalWidth && img.naturalHeight) ratio[k] = img.naturalWidth / img.naturalHeight;
  }
  function sizeOf(k: string): string {
    const r = ratio[k];
    if (!r) return `width:${H}px;height:${H}px`;
    const [w, h] = H * r <= W ? [H * r, H] : [W, W / r];
    return `width:${Math.max(16, Math.round(w))}px;height:${Math.max(16, Math.round(h))}px`;
  }

  async function load(en: Entry) {
    const k = en.key;
    if (urls[k] || failed[k]) return;
    const hit = loaded.get(k);
    if (hit) {
      urls[k] = hit;
      return;
    }
    try {
      const url = await en.fetch();
      remember(k, url);
      urls[k] = url;
    } catch {
      failed[k] = true;
    }
  }

  function lazy(node: HTMLElement, en: Entry) {
    const io = new IntersectionObserver(
      (es) => {
        if (es.some((e) => e.isIntersecting)) {
          io.disconnect();
          load(en);
        }
      },
      { rootMargin: '300px' },
    );
    io.observe(node);
    return { destroy: () => io.disconnect() };
  }

  // ── vista ampliada ──
  let open = $state<number | null>(null);
  function show(k: number) {
    open = k;
    entries.forEach(load);
  }
  function onKey(e: KeyboardEvent) {
    if (open === null) return;
    const n = entries.length;
    if (e.key === 'Escape') open = null;
    else if (e.key === 'ArrowRight') open = (open + 1) % n;
    else if (e.key === 'ArrowLeft') open = (open - 1 + n) % n;
    else return;
    e.preventDefault();
    e.stopPropagation();
  }
  $effect(() => {
    if (open === null) return;
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });
  // Si se quita la que se estaba viendo (o la lista encoge), se cierra.
  $effect(() => {
    if (open !== null && open >= entries.length) open = null;
  });
  onDestroy(() => window.removeEventListener('keydown', onKey, true));
</script>

<div class="media-strip" class:small>
  {#each entries as en, k (en.key)}
    {#if en.label !== undefined}
      <div class="att-tile">
        <button class="thumb" style={sizeOf(en.key)} use:lazy={en} onclick={() => show(k)} title="Ver en grande · {en.path}">
          {#if urls[en.key]}
            <img src={urls[en.key]} alt="" draggable="false" onload={(e) => measured(en.key, e)} />
          {:else}
            <span class="thumb-ph">{failed[en.key] ? 'no disponible' : '…'}</span>
          {/if}
        </button>
        <span class="att-name" title={en.path}>{en.label}</span>
        {#if onRemove}
          <button class="att-rm" type="button" title="Quitar" aria-label="Quitar {en.label}" onclick={() => onRemove(en.path!)}>×</button>
        {/if}
      </div>
    {:else}
      <button class="thumb" style={sizeOf(en.key)} use:lazy={en} onclick={() => show(k)} title="Ver en grande">
        {#if urls[en.key]}
          <img src={urls[en.key]} alt="" draggable="false" onload={(e) => measured(en.key, e)} />
        {:else}
          <span class="thumb-ph">{failed[en.key] ? 'no disponible' : '…'}</span>
        {/if}
      </button>
    {/if}
  {/each}
</div>

{#if open !== null && entries[open]}
  {@const en = entries[open]}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="lightbox" onclick={() => (open = null)}>
    {#if urls[en.key]}
      <img src={urls[en.key]} alt="" />
    {:else}
      <span class="thumb-ph">…</span>
    {/if}
    {#if en.label}<div class="lb-name">{en.label}</div>{/if}
    {#if entries.length > 1}
      <button
        class="lb-nav prev"
        aria-label="Anterior"
        onclick={(e) => {
          e.stopPropagation();
          open = (open! - 1 + entries.length) % entries.length;
        }}>‹</button
      >
      <button
        class="lb-nav next"
        aria-label="Siguiente"
        onclick={(e) => {
          e.stopPropagation();
          open = (open! + 1) % entries.length;
        }}>›</button
      >
      <div class="lb-count">{open + 1} / {entries.length}</div>
    {/if}
  </div>
{/if}
