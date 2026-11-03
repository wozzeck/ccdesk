<script lang="ts">
  // Spinner SVG cuadrado y de tamaño fijo: no se deforma al girar (también sin compositing GPU).
  let { size = 12 }: { size?: number } = $props();
</script>

<svg class="spinner" width={size} height={size} viewBox="0 0 16 16" aria-hidden="true">
  <circle class="track" cx="8" cy="8" r="6" />
  <path class="arc" d="M8 2a6 6 0 0 1 6 6" />
</svg>
