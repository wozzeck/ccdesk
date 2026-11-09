<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { api, errText, type ChangeResult, type PendingChange, type RateWindow, type SessionView, type Snapshot } from './api';
  import Dropdown from './Dropdown.svelte';
  import { MODE_NAMES } from './modes';

  interface Props {
    status: any | null;
    quota: Snapshot['quota'];
    usage: Snapshot['usage'];
    session: SessionView | null;
    /** Cambio de modelo o effort de esta sesión encolado hasta que quede inactiva. */
    queued?: PendingChange;
    mode: string | null;
    allowYolo: boolean;
    now: number;
    onToast: (m: string) => void;
    ask: (msg: string, ok?: string) => Promise<boolean>;
  }
  let { status, quota, usage, session, queued, mode, allowYolo, now, onToast, ask }: Props = $props();

  const TZ = 'Europe/Madrid';
  // MODE_NAMES sólo da los nombres; las opciones son las que ofrece la sesión.
  const modeOpt = (v: string) => ({ v, l: MODE_NAMES[v] ?? v, cls: v === 'bypassPermissions' ? 'yolo' : '' });

  // Opciones reales leídas de los selectores de Claude Code al abrir cada desplegable.
  let models = $state<{ n: number; label: string; desc: string }[]>([]);
  let efforts = $state<string[]>([]);
  let modeCycle = $state<string[]>([]);
  let optsKey = '';
  $effect(() => {
    // El ciclo de modos (y los efforts, que dependen del modelo) es de cada sesión.
    const k = session?.key ?? '';
    if (k !== optsKey) {
      optsKey = k;
      modeCycle = [];
      efforts = [];
    }
  });

  const modelName = $derived<string>(status?.model?.display_name ?? '');
  const modelCurrent = $derived(String(models.find((m) => m.label === modelName)?.n ?? ''));
  const effort = $derived<string | null>(status?.effort?.level ?? null);
  const ctxPct = $derived<number | null>(status?.context_window?.used_percentage ?? null);
  const ctxSize = $derived<number | null>(status?.context_window?.context_window_size ?? null);
  const cost = $derived<number | null>(status?.cost?.total_cost_usd ?? null);
  const fast = $derived(!!status?.fast_mode);
  const rl = $derived(quota ?? status?.rate_limits ?? null);

  // Lo elegido se enseña ya, hasta que el statusline (refresco cada ~10 s) lo confirme. Lo
  // encolado (`queued`, sesión ocupada) va por delante hasta que se aplica.
  let pending = $state<{ key: string; model?: string; effort?: string; mode?: string; until: number } | null>(null);
  const live = $derived(pending && session && pending.key === session.key && pending.until > now ? pending : null);
  const shownModel = $derived(queued?.model ? String(queued.model.n) : (live?.model ?? modelCurrent));
  const modelLabel = $derived(
    queued?.model?.label ??
      (live?.model ? (models.find((m) => String(m.n) === live.model)?.label ?? modelName) : modelName),
  );
  const shownEffort = $derived(queued?.effort ?? live?.effort ?? effort ?? '');
  const shownMode = $derived(live?.mode ?? mode ?? '');

  const running = $derived(!!session && session.managed && session.status !== 'stopped');
  const canMode = $derived(running && ['idle', 'busy'].includes(session!.status));
  // El modelo y el effort se pueden elegir con la sesión ocupada: se encolan y se aplican en cuanto
  // queda inactiva.
  const busy = $derived(running && session!.status !== 'idle');
  const why = $derived(
    !session
      ? ''
      : !session.managed
        ? 'Sesión externa: adóptala para cambiarlo'
        : session.status === 'stopped'
          ? 'Sesión detenida'
          : session.status !== 'idle'
            ? 'Espera a que Claude termine'
            : '',
  );
  const LATER = 'se aplicará en cuanto la sesión quede inactiva';
  const modelTitle = $derived(
    !running
      ? why
      : queued?.model
        ? `Pendiente de aplicar: ${queued.model.label}; ${LATER}. Elige ${modelName || 'el actual'} para anularlo.`
        : busy
          ? `Modelo de esta sesión. Claude está ocupado: lo que elijas ${LATER}.`
          : 'Modelo de esta sesión',
  );
  const effortTitle = $derived(
    !running
      ? why
      : queued?.effort
        ? `Pendiente de aplicar: effort ${queued.effort}; ${LATER}. Elige ${effort || 'el actual'} para anularlo.`
        : busy
          ? `Effort de esta sesión. Claude está ocupado: lo que elijas ${LATER}.`
          : 'Effort de esta sesión',
  );
  // YOLO aparece si la preferencia lo permite, aunque la sesión no lo tenga en su ciclo
  // (al elegirlo se ofrece reiniciarla con él); sin la preferencia, se quita.
  const modes = $derived.by(() => {
    let list = modeCycle.length ? [...modeCycle] : mode ? [mode] : [];
    if (allowYolo && !list.includes('bypassPermissions')) list.push('bypassPermissions');
    if (!allowYolo) list = list.filter((m) => m !== 'bypassPermissions' || m === mode);
    return list.map(modeOpt);
  });

  async function load(kind: 'model' | 'effort' | 'mode') {
    if (!session) return;
    const key = session.key;
    try {
      if (kind === 'model' && !models.length) models = await api.sessionModels(key);
      else if (kind === 'effort' && !efforts.length) efforts = (await api.sessionEfforts(key)).levels;
      else if (kind === 'mode' && !modeCycle.length) modeCycle = await api.sessionModes(key);
    } catch (e) {
      onToast(errText(e));
    }
  }

  async function change(kind: 'model' | 'effort' | 'mode', value: string) {
    if (!session || !value) return;
    const key = session.key;
    pending = { ...(pending?.key === key ? pending : {}), key, [kind]: value, until: Date.now() + 15000 };
    try {
      if (kind === 'model') {
        settled(key, kind, await api.setModel(key, Number(value), models.find((m) => String(m.n) === value)?.label ?? ''));
        efforts = []; // los niveles de effort dependen del modelo
      }
      else if (kind === 'effort') settled(key, kind, await api.setEffort(key, value));
      else await api.setMode(key, value);
    } catch (e) {
      pending = null;
      if (errText(e) === 'YOLO_UNAVAILABLE') await offerYoloRestart(key);
      else onToast(errText(e));
    }
  }

  function settled(key: string, kind: 'model' | 'effort', r: ChangeResult) {
    // Anulado lo encolado: vuelve a verse lo que tiene la sesión.
    if (r === 'cancelled' && pending?.key === key) pending = { ...pending, [kind]: undefined };
    else if (r === 'queued') onToast(`Claude está ocupado: el ${kind === 'model' ? 'modelo' : 'effort'} ${LATER}`);
  }

  // Lo encolado que se aplica después, cuando la sesión queda inactiva: se sigue viendo hasta que
  // el statusline lo confirme, como lo elegido con la sesión inactiva.
  onMount(() => {
    const un = listen<{ key: string } & PendingChange>('pending-applied', (e) => {
      const p = e.payload;
      if (p.model) efforts = [];
      if (session?.key !== p.key) return;
      pending = {
        ...(pending?.key === p.key ? pending : {}),
        key: p.key,
        ...(p.model ? { model: String(p.model.n) } : {}),
        ...(p.effort ? { effort: p.effort } : {}),
        until: Date.now() + 15000,
      };
    });
    // Lo encolado que no se ha podido aplicar: vuelve a verse lo que tiene la sesión.
    const unFailed = listen<{ key: string; kind: 'model' | 'effort' }>('pending-failed', (e) => {
      const p = e.payload;
      if (pending?.key === p.key) pending = { ...pending, [p.kind]: undefined };
    });
    return () => {
      un.then((f) => f());
      unFailed.then((f) => f());
    };
  });

  // El bypass sólo se puede activar si la sesión arrancó con --allow-dangerously-skip-permissions:
  // se ofrece reiniciarla (misma conversación) ya en modo YOLO.
  async function offerYoloRestart(key: string) {
    if (!session || session.key !== key) return;
    if (session.status !== 'idle' || session.background) {
      onToast(
        'Esta sesión arrancó sin el modo YOLO. Cuando Claude termine' +
          (session.background ? ' y acabe lo que tiene en segundo plano' : '') +
          ', elige YOLO otra vez para reiniciarla con él.',
      );
      return;
    }
    const ok = await ask(
      'Esta sesión arrancó sin el modo YOLO disponible y Claude Code no deja activarlo a mitad de sesión. ' +
        '¿Reiniciarla ahora en modo YOLO? Se reanuda la misma conversación; sólo se pierde lo que tenga en segundo plano.',
      'Reiniciar en YOLO',
    );
    if (!ok) return;
    onToast('Reiniciando la sesión en modo YOLO…');
    try {
      await api.sessionRestart(key, 'bypassPermissions');
      onToast('Sesión reiniciada en modo YOLO');
    } catch (e) {
      onToast(errText(e));
    }
  }

  function tok(n: number): string {
    if (n >= 1e6) return (n / 1e6).toFixed(n % 1e6 ? 1 : 0) + 'M';
    if (n >= 1e3) return Math.round(n / 1e3) + 'k';
    return String(n);
  }
  function level(p: number): string {
    return p >= 85 ? 'bad' : p >= 60 ? 'warn' : 'ok';
  }
  function reset(w: RateWindow | undefined, withDay: boolean): string {
    if (!w?.resets_at) return '';
    const d = new Date(w.resets_at * 1000);
    const hm = d.toLocaleTimeString('es-ES', { hour: '2-digit', minute: '2-digit', timeZone: TZ });
    if (!withDay) return hm;
    const day = d.toLocaleDateString('es-ES', { weekday: 'short', timeZone: TZ });
    return `${day} ${hm}`;
  }
  // Semanales por modelo (p. ej. Fable): sólo están en la caché de /usage de Claude Code.
  const scoped = $derived(usage?.scoped ?? []);
  const usageAge = $derived(usage?.fetchedAtMs ? Math.round((now - usage.fetchedAtMs) / 60000) : null);
  function ageText(m: number | null): string {
    if (m === null) return 'sin datos';
    if (m < 1) return 'ahora mismo';
    if (m < 60) return `hace ${m} min`;
    return `hace ${Math.round(m / 60)} h`;
  }
  let refreshing = $state(false);
  async function refreshUsage() {
    if (!session || refreshing) return;
    if (!session.managed || session.status !== 'idle') {
      onToast('Para actualizar las cuotas por modelo, abre una sesión inactiva (se lee de /usage)');
      return;
    }
    refreshing = true;
    try {
      await api.usageRefresh(session.key);
      onToast('Cuotas actualizadas desde /usage');
    } catch (e) {
      onToast(errText(e));
    } finally {
      refreshing = false;
    }
  }
  const five = $derived(rl?.five_hour);
  const seven = $derived(rl?.seven_day);
  // El dato puede ser de hace un rato: si el reset ya pasó, la ventana está a 0.
  const fivePct = $derived(
    five?.resets_at && five.resets_at * 1000 < now ? 0 : Math.round(five?.used_percentage ?? 0),
  );
  const sevenPct = $derived(
    seven?.resets_at && seven.resets_at * 1000 < now ? 0 : Math.round(seven?.used_percentage ?? 0),
  );
  const weekTitle = $derived(
    [
      `Semanal (todos los modelos): ${sevenPct}% · reinicio ${reset(seven, true)}`,
      ...scoped.map((sc) => `Semanal ${sc.label}: ${Math.round(sc.percent)}%`),
      scoped.length ? `(por modelo: dato de /usage, ${ageText(usageAge)}; clic para actualizar)` : '',
    ]
      .filter(Boolean)
      .join('\n'),
  );

</script>

<div class="statusbar">
  {#if session && status}
    <Dropdown
      cls={queued?.model ? 'strong pending' : 'strong'}
      title={modelTitle}
      disabled={!running}
      value={shownModel}
      label={modelLabel}
      options={models.map((m) => ({ v: String(m.n), l: m.label }))}
      onOpen={() => load('model')}
      onChange={(v) => change('model', v)}
    />
    {#if fast}<span class="seg">⚡</span>{/if}
    <Dropdown
      cls={queued?.effort ? 'pending' : ''}
      title={effortTitle}
      disabled={!running}
      value={shownEffort}
      label={shownEffort}
      options={efforts.map((e) => ({ v: e, l: e }))}
      onOpen={() => load('effort')}
      onChange={(v) => change('effort', v)}
    />
    <Dropdown
      title={canMode ? 'Modo de permisos (Mayús+Tab en la TUI)' : why}
      disabled={!canMode}
      value={shownMode}
      label={MODE_NAMES[shownMode] ?? shownMode}
      options={modes}
      onOpen={() => load('mode')}
      onChange={(v) => change('mode', v)}
    />
    {#if ctxPct !== null}
      <span class="seg" title="Contexto usado">
        ctx
        <span class="bar"><span class="fill {level(ctxPct)}" style="width:{ctxPct}%"></span></span>
        {ctxPct}%{ctxSize ? ` · ${tok((ctxPct * ctxSize) / 100)}/${tok(ctxSize)}` : ''}
      </span>
    {/if}
    {#if cost !== null}<span class="seg muted">${cost.toFixed(2)}</span>{/if}
    {#if session.status === 'stopped'}<span class="seg muted">(datos al detenerla)</span>{/if}
  {:else if session}
    <span class="seg muted">sin datos de statusline todavía</span>
  {/if}
  <span class="spacer"></span>
  {#if five}
    <span class="seg" title="Cuota de 5 horas">
      5h
      <span class="bar"><span class="fill {level(fivePct)}" style="width:{fivePct}%"></span></span>
      {fivePct}% <span class="muted">↻ {reset(five, false)}</span>
    </span>
  {/if}
  {#if five && seven}<span class="seg muted">·</span>{/if}
  {#if seven}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <span class="seg quota7" class:busy={refreshing} title={weekTitle} onclick={refreshUsage}>
      7d
      {#if scoped.length}
        <span class="bar stacked">
          <span class="track"><span class="fill {level(scoped[0].percent)}" style="width:{Math.min(100, scoped[0].percent)}%"></span></span>
          <span class="track"><span class="fill {level(sevenPct)}" style="width:{sevenPct}%"></span></span>
        </span>
      {:else}
        <span class="bar"><span class="fill {level(sevenPct)}" style="width:{sevenPct}%"></span></span>
      {/if}
      {sevenPct}% <span class="muted">↻ {reset(seven, true)}</span>
    </span>
  {/if}
</div>
