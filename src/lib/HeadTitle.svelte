<script lang="ts">
  /** Nombre de la sesión en la cabecera: un clic lo vuelve editable para renombrarla.
   *  Es un botón para que Tauri no lo trate como zona de arrastre de la ventana. */
  let {
    sessionKey,
    title,
    editable,
    onRename,
    onDone,
  }: {
    sessionKey: string;
    title: string;
    /** Sólo las sesiones gestionadas se pueden renombrar. */
    editable: boolean;
    onRename: (key: string, name: string) => void;
    /** Al acabar con Enter o Esc, para devolver el foco a donde se escribe. */
    onDone?: () => void;
  } = $props();

  let editing = $state(false);
  let value = $state('');
  // Se fijan al empezar: si mientras tanto llega un snapshot o cambia la sesión abierta, se
  // renombra la que se estaba editando y se compara con el nombre que tenía entonces.
  let key = '';
  let before = '';

  function start() {
    key = sessionKey;
    before = title;
    value = title;
    editing = true;
  }
  function finish(save: boolean, byKey: boolean) {
    if (!editing) return;
    editing = false;
    const v = value.trim();
    // Vacío = quitar el nombre propio (vuelve el automático), igual que en la lista.
    if (save && v !== before) onRename(key, v);
    if (byKey) onDone?.();
  }
  function focusSelect(el: HTMLInputElement) {
    el.focus();
    el.select();
  }
</script>

{#if editing}
  <input
    class="h-rename"
    bind:value
    use:focusSelect
    spellcheck="false"
    aria-label="Nombre de la sesión"
    onkeydown={(e) => {
      if (e.key === 'Enter' || e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        finish(e.key === 'Enter', true);
      }
    }}
    onblur={() => finish(true, false)}
  />
{:else if editable}
  <button class="h-name" title="Clic para renombrar" onclick={start}><span class="name">{title}</span></button>
{:else}
  <span class="name">{title}</span>
{/if}
