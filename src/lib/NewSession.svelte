<script lang="ts">
  import { errText } from './api';

  interface Props {
    recent: string[];
    home: string;
    onCreate: (cwd: string, name: string, model: string, effort: string) => Promise<void>;
    onCancel: () => void;
  }
  let { recent, home, onCreate, onCancel }: Props = $props();

  // svelte-ignore state_referenced_locally
  let cwd = $state(recent[0] ?? home);
  let name = $state('');
  let model = $state('');
  let effort = $state('');
  let busy = $state(false);
  let err = $state('');

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    err = '';
    try {
      await onCreate(cwd, name, model, effort);
    } catch (x) {
      err = errText(x);
    } finally {
      busy = false;
    }
  }
  function focusSelect(el: HTMLInputElement) {
    el.focus();
    el.select();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="modal-bg" onclick={onCancel}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <form
    class="modal"
    onclick={(e) => e.stopPropagation()}
    onsubmit={submit}
    onkeydown={(e) => e.key === 'Escape' && onCancel()}
  >
    <h3>Nueva sesión</h3>
    <label>
      Directorio de trabajo
      <input list="ccdesk-cwds" bind:value={cwd} use:focusSelect spellcheck="false" />
    </label>
    <datalist id="ccdesk-cwds">
      {#each recent as c (c)}<option value={c}></option>{/each}
    </datalist>
    <label>
      Nombre (opcional)
      <input bind:value={name} placeholder="p. ej. Revisión tren 26.11" />
    </label>
    <div class="two">
      <label>
        Modelo
        <select bind:value={model}>
          <option value="">Por defecto</option>
          <option value="opus">Opus</option>
          <option value="sonnet">Sonnet</option>
          <option value="haiku">Haiku</option>
          <option value="claude-fable-5-1">Fable 5.1</option>
        </select>
      </label>
      <label>
        Effort
        <select bind:value={effort}>
          <option value="">Por defecto</option>
          {#each ['low', 'medium', 'high', 'xhigh', 'max'] as e (e)}<option value={e}>{e}</option>{/each}
        </select>
      </label>
    </div>
    {#if err}<div class="err">{err}</div>{/if}
    <div class="buttons">
      <button type="button" onclick={onCancel}>Cancelar</button>
      <button class="primary" type="submit" disabled={busy}>{busy ? 'Creando…' : 'Crear'}</button>
    </div>
  </form>
</div>
