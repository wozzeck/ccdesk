<script lang="ts">
  // Botones de ventana propios: la app va sin decoración del sistema para que la barra siga el tema.
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  const win = getCurrentWindow();
  let maximized = $state(false);

  onMount(() => {
    const update = () =>
      win
        .isMaximized()
        .then((m) => (maximized = m))
        .catch(() => {});
    update();
    const un = win.onResized(update);
    return () => {
      un.then((f) => f());
    };
  });
</script>

<div class="winctl">
  <button title="Minimizar" onclick={() => win.minimize()} aria-label="Minimizar">
    <svg viewBox="0 0 10 10"><path d="M1.5 5.5h7" /></svg>
  </button>
  <button
    title={maximized ? 'Restaurar' : 'Maximizar'}
    aria-label={maximized ? 'Restaurar' : 'Maximizar'}
    onclick={() => win.toggleMaximize()}
  >
    {#if maximized}
      <svg viewBox="0 0 10 10"><path d="M3 1.5h5.5V7M1.5 3h5.5v5.5H1.5z" /></svg>
    {:else}
      <svg viewBox="0 0 10 10"><path d="M1.5 1.5h7v7h-7z" /></svg>
    {/if}
  </button>
  <button
    class="close"
    title="Cerrar ccdesk (las sesiones siguen en marcha)"
    aria-label="Cerrar"
    onclick={() => win.close()}
  >
    <svg viewBox="0 0 10 10"><path d="M2 2l6 6M8 2l-6 6" /></svg>
  </button>
</div>
