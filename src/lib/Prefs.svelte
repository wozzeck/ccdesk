<script lang="ts">
  import { errText, type Settings } from './api';
  import { MODE_NAMES } from './modes';
  import { applyTheme } from './theme';

  const THEMES = [
    { v: 'system', l: 'Sistema' },
    { v: 'light', l: 'Claro' },
    { v: 'dark', l: 'Oscuro' },
    { v: 'sepia', l: 'Sepia' },
  ] as const;

  interface Props {
    settings: Settings;
    onSave: (s: Settings) => Promise<void>;
    onCancel: () => void;
  }
  let { settings, onSave, onCancel }: Props = $props();

  // El tema se prueba en vivo al elegirlo; al cancelar vuelve el guardado.
  function cancel() {
    applyTheme(settings.theme || 'system');
    onCancel();
  }

  // svelte-ignore state_referenced_locally
  let draft = $state<Settings>({
    ...settings,
    theme: settings.theme || 'system',
    sessionOrder: settings.sessionOrder || 'manual',
    newSessionMode: settings.newSessionMode || '',
  });
  let busy = $state(false);
  let err = $state('');

  // YOLO sólo se ofrece si está permitido; si se deja de permitir, el modo vuelve al de Claude Code.
  const modes = $derived(Object.entries(MODE_NAMES).filter(([v]) => v !== 'bypassPermissions' || draft.allowYolo));
  function yoloChanged() {
    if (!draft.allowYolo && draft.newSessionMode === 'bypassPermissions') draft.newSessionMode = '';
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    err = '';
    try {
      await onSave({
        ...draft,
        tmuxSession: draft.tmuxSession.trim() || 'claude',
        autostopMinutes: Math.max(5, Number(draft.autostopMinutes) || 60),
      });
    } catch (x) {
      err = errText(x);
    } finally {
      busy = false;
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="modal-bg" onclick={cancel}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <form
    class="modal prefs"
    onclick={(e) => e.stopPropagation()}
    onsubmit={submit}
    onkeydown={(e) => e.key === 'Escape' && cancel()}
  >
    <h3>Preferencias</h3>
    <!-- Sólo esto se desplaza si no cabe: el título y los botones quedan siempre a la vista.
         Tres columnas (interfaz, sesiones, sistema) que pasan a dos o a una si no caben. -->
    <div class="prefs-body">
      <div class="pcol">
        <section class="pgroup" aria-labelledby="pg1">
          <div class="pgroup-title" id="pg1">Apariencia</div>
          <div class="themes">
            {#each THEMES as t (t.v)}
              <label class="theme-opt" class:sel={draft.theme === t.v}>
                <input type="radio" name="theme" value={t.v} bind:group={draft.theme} onchange={() => applyTheme(draft.theme)} />
                <span class="sw sw-{t.v}"></span>
                {t.l}
              </label>
            {/each}
          </div>
          <p class="hint">
            Zoom: Ctrl + rueda del ratón (Ctrl+0 lo devuelve al 100 %). Ahora: {Math.round((settings.zoom || 1) * 100)} %.
          </p>
        </section>

        <section class="pgroup" aria-labelledby="pg7">
          <div class="pgroup-title" id="pg7">Lista de sesiones</div>
          <label class="row">
            <input type="radio" name="sessionOrder" value="manual" bind:group={draft.sessionOrder} />
            Orden manual (arrastrando)
          </label>
          <label class="row">
            <input type="radio" name="sessionOrder" value="recent" bind:group={draft.sessionOrder} />
            Por actividad reciente
          </label>
          <p class="hint">
            Por actividad reciente, arriba va la que ha tenido actividad hace menos. Las fijadas siguen
            arriba en su orden y son las únicas que se pueden arrastrar (entre ellas). El orden manual se
            guarda para cuando vuelvas a él.
          </p>
        </section>

        <section class="pgroup" aria-labelledby="pg6">
          <div class="pgroup-title" id="pg6">Conversación</div>
          <label class="row">
            <input type="checkbox" bind:checked={draft.showMedia} />
            Mostrar las imágenes que Claude mira o que le pegas
          </label>
          <p class="hint">
            Fotos, capturas o fotogramas de vídeo que lee, como miniaturas bajo sus herramientas, sin
            desplegarlas. Al pulsar una se ve en grande (← → para pasar, Esc para cerrar).
          </p>
        </section>
      </div>

      <div class="pcol">
        <section class="pgroup" aria-labelledby="pg3">
          <div class="pgroup-title" id="pg3">Permisos</div>
          <label>
            Modo de las sesiones nuevas
            <select bind:value={draft.newSessionMode}>
              <option value="">El de Claude Code</option>
              {#each modes as [v, l] (v)}<option value={v}>{l}</option>{/each}
            </select>
          </label>
          <p class="hint">
            Las que creas en ccdesk arrancan en este modo; luego se cambia en la barra inferior. «El de
            Claude Code» es el de su configuración (<code>permissions.defaultMode</code>).
          </p>
          <label class="row">
            <input type="checkbox" bind:checked={draft.allowYolo} onchange={yoloChanged} />
            Permitir modo YOLO (bypass permissions)
          </label>
          <p class="hint">
            Las sesiones que crea o reanuda ccdesk arrancan con
            <code>--allow-dangerously-skip-permissions</code>: el modo sin permisos queda disponible en el
            desplegable de modo de la barra inferior, sin activarse salvo que lo elijas arriba. Las que
            ya están abiertas lo tendrán al reanudarlas.
          </p>
        </section>

        <section class="pgroup" aria-labelledby="pg5">
          <div class="pgroup-title" id="pg5">Memoria</div>
          <label class="row">
            <input type="checkbox" bind:checked={draft.autostop} />
            Detener las sesiones inactivas tras
            <input class="mins" type="number" min="5" step="5" bind:value={draft.autostopMinutes} />
            min
          </label>
          <p class="hint">
            Nunca se detiene la sesión abierta ni las marcadas «∞». Una detenida se reanuda con ▶.
          </p>
        </section>
      </div>

      <div class="pcol">
        <section class="pgroup" aria-labelledby="pg4">
          <div class="pgroup-title" id="pg4">Notificaciones de escritorio</div>
          <label class="row">
            <input type="checkbox" bind:checked={draft.notifyDone} />
            Cuando Claude termina y tiene una respuesta nueva
          </label>
          <label class="row">
            <input type="checkbox" bind:checked={draft.notifyAttention} />
            Cuando Claude te pregunta algo o pide permiso
          </label>
          <p class="hint">
            No avisa de la sesión que tienes abierta mientras miras ccdesk. Al pulsar el aviso se abre
            esa sesión, y al verla se retira el aviso.
          </p>
        </section>

        <section class="pgroup" aria-labelledby="pg2">
          <div class="pgroup-title" id="pg2">tmux</div>
          <label>
            Nombre de la sesión
            <input bind:value={draft.tmuxSession} spellcheck="false" placeholder="claude" />
          </label>
          <p class="hint">
            Todas las sesiones de Claude viven como ventanas de esta sesión del tmux normal. Desde una
            terminal o por SSH: <code>tmux a -t {draft.tmuxSession.trim() || 'claude'}</code>. Si la
            cambias con la sesión ya creada, se renombra en tmux sin cerrar nada.
          </p>
        </section>
      </div>
    </div>
    {#if err}<div class="err">{err}</div>{/if}
    <div class="buttons">
      <button type="button" onclick={cancel}>Cancelar</button>
      <button class="primary" type="submit" disabled={busy}>{busy ? 'Guardando…' : 'Guardar'}</button>
    </div>
  </form>
</div>
