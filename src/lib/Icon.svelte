<script lang="ts">
  // Iconos de la interfaz (trazos al estilo Lucide, licencia ISC), en el color del texto.
  interface Props {
    name: 'terminal' | 'todo' | 'settings' | 'send';
    size?: number;
  }
  let { name, size = 17 }: Props = $props();
</script>

<svg
  class="ico"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.8"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#if name === 'terminal'}
    <rect x="3" y="4" width="18" height="16" rx="2.5" />
    <path d="m7.5 10 2.5 2.5-2.5 2.5" />
    <path d="M12.5 15.5h4" />
  {:else if name === 'todo'}
    <rect x="3" y="4.5" width="6" height="6" rx="1.2" />
    <path d="m3.5 17 2 2 3.5-3.5" />
    <path d="M13 7.5h8" />
    <path d="M13 12.5h8" />
    <path d="M13 17.5h8" />
  {:else if name === 'settings'}
    <path
      d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"
    />
    <circle cx="12" cy="12" r="3" />
  {:else if name === 'send'}
    <!-- Triángulo de «enviar» relleno, como en las apps de mensajería. -->
    <path
      d="M4.4 3.6 21 11.1a1 1 0 0 1 0 1.8L4.4 20.4a1 1 0 0 1-1.38-1.2L5.4 12 3.02 4.8A1 1 0 0 1 4.4 3.6Z"
      fill="currentColor"
      stroke="none"
    />
  {/if}
</svg>
