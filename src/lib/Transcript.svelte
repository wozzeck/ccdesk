<script lang="ts">
  // Pinta una lista de items de transcript (conversación principal o de un subagente).
  import { api, errText, type ImgRef, type Item } from './api';
  import { renderMarkdown } from './markdown';
  import Spinner from './Spinner.svelte';
  import MediaStrip from './MediaStrip.svelte';
  import { ui } from './ui.svelte';
  import AgentCard from './AgentCard.svelte';
  import { outbox, flyIn, morph } from './outbox.svelte';
  import { copyPlain, copyRich, portableHtml, visibleText } from './clipboard';
  import { FILE_TOOLS, IMAGE_RE, OPEN_ICON, resolveFileRefs, splitRefs } from './filerefs';
  import { splitAttachments, type AttSeg } from './files';
  import { ago, clock, fullDate } from './clock.svelte';

  /** Imágenes de un mensaje tuyo (cada una una vez), para sus miniaturas. */
  function attFiles(segs: AttSeg[]): { path: string; label: string }[] {
    const seen = new Set<string>();
    return segs.flatMap((s) =>
      'att' in s && !seen.has(s.att.path) && seen.add(s.att.path) ? [{ path: s.att.path, label: s.att.label }] : [],
    );
  }

  interface Props {
    items: Item[];
    startIndex: number;
    /** Claude está trabajando (spinner en herramientas sin resultado). */
    busy: boolean;
    /** El proceso sigue vivo. */
    live: boolean;
    /** Claude espera un mensaje (para ver si el último tuyo, sin respuesta, se interrumpió). */
    idle?: boolean;
    /** Enseñar los mensajes en cola (sólo en la conversación principal). */
    showPending?: boolean;
    /** Clave de la sesión (para las burbujas provisionales de lo recién enviado). */
    sessionKey?: string;
    /** Carpeta de la sesión: base de las rutas relativas que no se encuentran de otro modo. */
    cwd?: string;
    onToast: (m: string) => void;
    onOpenAgent: (toolUseId: string, title: string) => void;
  }
  let {
    items,
    startIndex,
    busy,
    live,
    idle = false,
    showPending = false,
    sessionKey = '',
    cwd = '',
    onToast,
    onOpenAgent,
  }: Props = $props();

  type ToolItem = Extract<Item, { kind: 'tool' }>;
  type ResultItem = Extract<Item, { kind: 'toolResult' }>;
  type AgentItem = Extract<Item, { kind: 'agent' }>;
  type AgentResult = Extract<Item, { kind: 'agentResult' }>;
  type TaskNotice = Extract<Item, { kind: 'taskNotice' }>;
  // Mensajes tuyos interrumpidos antes de que Claude respondiera: Claude Code los devuelve a su caja
  // de texto y sigue la conversación desde el anterior. El backend lo ve en el árbol del JSONL al
  // llegar lo siguiente; hasta entonces, el último sin respuesta con Claude esperando también lo
  // está (con unos segundos de margen: el estado «trabajando» llega algo después que el mensaje).
  const withdrawn = $derived(new Set(items.flatMap((it) => (it.kind === 'withdrawn' ? [it.of] : []))));
  const QUIET = new Set<Item['kind']>(['queueAdd', 'queueDequeue', 'queueRemove', 'queueClear', 'withdrawn']);
  const lastUser = $derived.by(() => {
    if (!idle) return -1;
    for (let k = items.length - 1; k >= 0; k--) {
      const it = items[k];
      if (QUIET.has(it.kind)) continue;
      return it.kind === 'user' && !it.midTurn ? k : -1;
    }
    return -1;
  });
  const GRACE = 3000;
  let tick = $state(Date.now());
  $effect(() => {
    if (lastUser < 0) return;
    const left = Date.parse(items[lastUser].ts) + GRACE - Date.now();
    if (!(left > 0)) {
      tick = Date.now();
      return;
    }
    const t = setTimeout(() => (tick = Date.now()), left + 50);
    return () => clearTimeout(t);
  });
  const cutOff = $derived(
    lastUser >= 0 && tick >= Date.parse(items[lastUser].ts) + GRACE ? startIndex + lastUser : -1,
  );

  type Row =
    | { t: 'user'; i: number; text: string; midTurn: boolean; images: ImgRef[]; withdrawn: boolean; at: number }
    | { t: 'assistant'; i: number; text: string; cwd: string; at: number }
    | { t: 'tools'; i: number; tools: { item: ToolItem; result?: ResultItem }[] }
    | {
        t: 'agent';
        i: number;
        item: AgentItem;
        result?: AgentResult;
        notice?: TaskNotice;
        report?: string;
        text: string;
      }
    | { t: 'command'; i: number; name: string; args: string }
    | { t: 'output'; i: number; text: string }
    | { t: 'notice'; i: number; text: string }
    | { t: 'summary'; i: number; text: string };

  const rows = $derived.by(() => {
    const results = new Map<string, ResultItem>();
    const agentResults = new Map<string, AgentResult>();
    const notices = new Map<string, TaskNotice>();
    const reports = new Map<string, string>();
    const toolIds = new Set<string>();
    for (const it of items) {
      if (it.kind === 'toolResult') results.set(it.toolUseId, it);
      else if (it.kind === 'agentReport') reports.set(it.agentId, it.text);
      else if (it.kind === 'agentResult') agentResults.set(it.toolUseId, it);
      else if (it.kind === 'taskNotice') notices.set(it.toolUseId, it);
      else if (it.kind === 'tool' || it.kind === 'agent') toolIds.add(it.id);
    }
    const out: Row[] = [];
    items.forEach((it, idx) => {
      const i = startIndex + idx;
      if (held.has(idx)) return;
      switch (it.kind) {
        case 'user':
          out.push({
            t: 'user',
            i,
            text: it.text,
            midTurn: it.midTurn,
            images: it.images ?? [],
            withdrawn: withdrawn.has(i) || i === cutOff,
            at: Date.parse(it.ts) || 0,
          });
          break;
        case 'assistant': {
          const last = out[out.length - 1];
          // Bloques de texto seguidos del mismo turno → un solo mensaje (con la hora del último).
          const at = Date.parse(it.ts);
          if (last && last.t === 'assistant') {
            last.text += '\n\n' + it.text;
            if (at) last.at = at;
          } else out.push({ t: 'assistant', i, text: it.text, cwd: it.cwd ?? '', at: at || 0 });
          break;
        }
        case 'tool': {
          const last = out[out.length - 1];
          const entry = { item: it, result: results.get(it.id) };
          if (last && last.t === 'tools') last.tools.push(entry);
          else out.push({ t: 'tools', i, tools: [entry] });
          break;
        }
        case 'agent': {
          const result = agentResults.get(it.id);
          const notice = notices.get(it.id);
          const report = result ? reports.get(result.agentId) : undefined;
          out.push({ t: 'agent', i, item: it, result, notice, report, text: agentReport(result, notice, report) });
          break;
        }
        case 'toolResult':
          if (!toolIds.has(it.toolUseId)) out.push({ t: 'output', i, text: it.text });
          break;
        case 'command':
          out.push({ t: 'command', i, name: it.name, args: it.args });
          break;
        case 'output': {
          // Abrir /model y cerrarlo sin elegir («Kept model as …») no cambia nada: ni se pinta.
          if (/^Kept model as /.test(it.text.trim())) {
            const last = out[out.length - 1];
            if (last?.t === 'command' && last.name === '/model' && !last.args) out.pop();
            break;
          }
          out.push({ t: 'output', i, text: it.text });
          break;
        }
        case 'notice':
          out.push({ t: 'notice', i, text: it.text });
          break;
        case 'summary': {
          // Va justo tras el aviso «Conversación compactada»: lo sustituye por el bloque plegable
          // (sólo ese aviso; otro cualquiera que hubiera delante se queda).
          const last = out[out.length - 1];
          const marker = last !== undefined && last.t === 'notice' && last.text === 'Conversación compactada';
          if (marker) out.pop();
          out.push({ t: 'summary', i: marker && last ? last.i : i, text: it.text });
          break;
        }
        // agentResult / taskNotice se pintan dentro de su tarjeta; las notificaciones de
        // Monitor o Bash en segundo plano no se enseñan.
      }
    });
    return out;
  });

  // Réplica de la cola de entrada de Claude Code (FIFO): lo enviado mientras trabaja y aún
  // no recogido. Los avisos internos (<task-notification>…) también pasan por la cola, pero
  // no se enseñan. A veces Claude Code apunta un mensaje en la cola sin su texto (durante
  // /compact, por ejemplo): se enseña con el que se envió desde aquí y, si se escribió en la
  // terminal y no se sabe qué dice, no se enseña (pero sigue contando para el orden de la cola).
  const pending = $derived.by(() => {
    if (!showPending) return [];
    const q: { raw: string; text: string; show: boolean; id: number; at: number }[] = [];
    for (const [idx, it] of items.entries()) {
      if (it.kind === 'queueAdd') {
        const text = it.text || blankText.get(idx) || outbox.recall(sessionKey, it.ts) || '';
        // Mientras vuela la burbuja provisional, ésta espera (pero ocupa su sitio en la cola).
        q.push({
          raw: it.text,
          text,
          show: !it.system && !!text.trim() && !held.has(idx),
          id: startIndex + idx,
          at: Date.parse(it.ts) || 0,
        });
      } else if (it.kind === 'queueDequeue') q.shift();
      else if (it.kind === 'queueClear') q.length = 0;
      else if (it.kind === 'queueRemove') {
        let k = q.findIndex((x) => x.raw === it.text);
        if (k < 0) k = q.findIndex((x) => x.text === it.text);
        if (k < 0) k = q.findIndex((x) => !x.raw);
        if (k >= 0) q.splice(k, 1);
      }
    }
    return q.filter((x) => x.show);
  });

  // El informe puede llegar por hand-back (mensaje del subagente), en el aviso de fin o,
  // si es síncrono, en el propio resultado de la llamada.
  function agentReport(result?: AgentResult, notice?: TaskNotice, report?: string): string {
    if (report) return report;
    const r = notice?.result ?? '';
    if (r && !/^This agent's report was delivered/i.test(r)) return r;
    return result && result.status !== 'async_launched' ? result.text : '';
  }

  // Burbujas provisionales de lo recién enviado: se relevan por el mensaje real cuando aparece en
  // el transcript (como mensaje, en cola o como comando), pero no mientras la animación de envío
  // está en vuelo: el real espera y entra al aterrizar, en el mismo sitio, sin salto. Sólo cuentan
  // los items posteriores al envío, para no confundirlo con un mensaje igual anterior.
  type QueueAdd = Extract<Item, { kind: 'queueAdd' }>;
  const isMatch = (it: Item, text: string) =>
    (it.kind === 'user' && it.text.trim() === text) ||
    (it.kind === 'queueAdd' && it.text.trim() === text) ||
    (it.kind === 'command' && (it.args ? `${it.name} ${it.args}` : it.name) === text);
  /** Apunte de la cola sin texto que aún no se ha atribuido a ningún envío. */
  const isBlankQueued = (it: Item): it is QueueAdd =>
    it.kind === 'queueAdd' && !it.system && !it.text.trim() && outbox.recall(sessionKey, it.ts) === undefined;
  /** envío provisional → índice (en items) de su mensaje real */
  const matches = $derived.by(() => {
    const m = new Map<number, number>();
    if (!showPending || !sessionKey) return m;
    const total = startIndex + items.length;
    const taken = new Set<number>();
    // En orden de envío. Primero el mismo texto; si no está, el primer apunte de la cola sin texto
    // (es FIFO: el primero enviado, el primero apuntado).
    const find = (from: number, ok: (it: Item) => boolean) => {
      for (let j = from; j < items.length; j++) if (!taken.has(j) && ok(items[j])) return j;
      return -1;
    };
    for (const o of outbox.items) {
      if (o.key !== sessionKey) continue;
      if (!outbox.base.has(o.id)) {
        // Lo que llegó a la vez que se veía el envío por primera vez (p. ej., con la vista sin
        // montar) cuenta si es posterior por la hora.
        let b = items.length;
        while (b > 0 && Date.parse(items[b - 1].ts) >= o.at - 1000) b--;
        outbox.base.set(o.id, startIndex + b);
      }
      const from = Math.max(0, outbox.base.get(o.id)! - startIndex);
      let j = find(from, (it) => isMatch(it, o.text));
      if (j < 0 && !o.waiting) j = find(from, isBlankQueued);
      if (j >= 0) {
        m.set(o.id, j);
        taken.add(j);
      }
    }
    return m;
  });
  /** índice de un apunte sin texto → texto del envío al que corresponde (mientras sigue en el buzón) */
  const blankText = $derived(
    new Map(
      outbox.items.flatMap((o) => {
        const j = matches.get(o.id);
        const it = j === undefined ? undefined : items[j];
        return j !== undefined && it?.kind === 'queueAdd' && !it.text.trim() ? [[j, o.text] as const] : [];
      }),
    ),
  );
  /** índices de mensajes reales que esperan a que aterrice su animación */
  const held = $derived(
    new Set(
      outbox.items.filter((o) => o.flying && matches.has(o.id)).map((o) => matches.get(o.id)!),
    ),
  );
  const outgoing = $derived(
    showPending && sessionKey
      ? outbox.items.filter((o) => o.key === sessionKey && (!matches.has(o.id) || o.flying))
      : [],
  );
  $effect(() => {
    // Quitar del buzón las ya relevadas por su mensaje real (si es un apunte de la cola sin texto,
    // guardando antes el texto, que el transcript no tiene).
    for (const o of outbox.items) {
      if (o.key !== sessionKey || o.flying || !matches.has(o.id)) continue;
      const it = items[matches.get(o.id)!];
      if (it?.kind === 'queueAdd' && !it.text.trim()) outbox.remember(sessionKey, it.ts, o.text);
      outbox.remove(o.id);
    }
  });

  // ── rutas de fichero: icono para abrirlas (filerefs.ts) ──
  // Las absolutas que han usado las herramientas sirven de base a las relativas del texto. Cada
  // cambio en la conversación resuelve las rutas nuevas (las del markdown llegan con el HTML).
  let root: HTMLDivElement;
  const known = $derived(
    items.flatMap((it) => (it.kind === 'tool' && FILE_TOOLS.has(it.name) && it.summary.startsWith('/') ? [it.summary] : [])),
  );
  const cwds = $derived([...new Set(items.flatMap((it) => (it.kind === 'assistant' && it.cwd ? [it.cwd] : [])).reverse())]);
  $effect(() => {
    const el = root;
    let t = 0;
    const run = () => {
      t = 0;
      resolveFileRefs(el, { cwd, known, cwds });
    };
    const kick = () => {
      if (!t) t = window.setTimeout(run, 60);
    };
    const mo = new MutationObserver(kick);
    mo.observe(el, { childList: true, subtree: true });
    kick();
    return () => {
      mo.disconnect();
      clearTimeout(t);
    };
  });

  // Imagen abierta desde su ruta: en grande, con la misma vista que las miniaturas (MediaStrip).
  let big = $state<string | null>(null);
  const MIME: Record<string, string> = { jpg: 'image/jpeg', jpeg: 'image/jpeg', svg: 'image/svg+xml' };
  async function showImage(path: string) {
    try {
      const buf = await api.imageFile(path);
      const ext = path.split('.').pop()!.toLowerCase();
      closeImage();
      big = URL.createObjectURL(new Blob([buf], { type: MIME[ext] ?? `image/${ext}` }));
    } catch (x) {
      onToast(errText(x));
    }
  }
  function closeImage() {
    if (big) URL.revokeObjectURL(big);
    big = null;
  }
  $effect(() => {
    if (!big) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopPropagation();
      closeImage();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  /** Identidad de una burbuja tuya entre sus estados (provisional, en cola, real). */
  const bubbleId = (text: string) => `${sessionKey}\u0000${text.trim()}`;

  function toolsLabel(tools: { item: ToolItem }[]): string {
    const counts = new Map<string, number>();
    for (const t of tools) counts.set(t.item.name, (counts.get(t.item.name) ?? 0) + 1);
    const parts = [...counts].map(([n, c]) => (c > 1 ? `${n} ×${c}` : n));
    return parts.join(', ');
  }

  function rowText(i: number): string {
    const r = rows.find((x) => x.i === i);
    return r && 'text' in r ? r.text : '';
  }

  // Los botones de copiar no se quedan con el foco: sin anillo azul y el cursor sigue en la caja.
  function onMouseDown(e: MouseEvent) {
    if ((e.target as HTMLElement).closest('.msg-tools button, .copy-code, .open-file')) e.preventDefault();
  }

  function onClick(e: MouseEvent) {
    const t = e.target as HTMLElement;
    const pill = t.closest<HTMLElement>('.att-pill');
    if (pill?.dataset.path) {
      showImage(pill.dataset.path);
      return;
    }
    const of = t.closest('.open-file');
    if (of) {
      // También evita que se despliegue la herramienta en cuyo resumen va.
      e.preventDefault();
      const fr = of.closest<HTMLElement>('.fref');
      const abs = fr?.dataset.abs;
      if (abs && fr?.dataset.kind !== 'dir' && IMAGE_RE.test(abs)) showImage(abs);
      else if (abs) api.openFile(abs).catch((x) => onToast(errText(x)));
      return;
    }
    const a = t.closest('a[href]') as HTMLAnchorElement | null;
    if (a) {
      e.preventDefault();
      api.openUrl(a.href).catch((x) => onToast(errText(x)));
      return;
    }
    const cc = t.closest('.copy-code');
    if (cc) {
      const pre = cc.parentElement?.querySelector('pre');
      if (pre && copyPlain((pre.textContent ?? '').replace(/\n$/, ''))) flash(cc as HTMLElement);
      return;
    }
    const btn = t.closest('[data-act]') as HTMLElement | null;
    if (!btn) return;
    const msg = btn.closest('.msg') as HTMLElement;
    const body = msg?.querySelector('.body');
    if (!body) return;
    const i = Number(msg.dataset.i);
    let ok = false;
    switch (btn.dataset.act) {
      case 'rich':
        ok = copyRich(portableHtml(body), rowText(i));
        break;
      case 'plain':
        ok = copyPlain(visibleText(body));
        break;
      case 'md':
        ok = copyPlain(rowText(i));
        break;
    }
    if (ok) flash(btn);
    else onToast('No se pudo copiar');
  }

  function flash(el: HTMLElement) {
    const old = el.textContent;
    el.textContent = '✓';
    setTimeout(() => (el.textContent = old), 900);
  }
</script>

<!-- Ruta de fichero con su icono de abrir (sale sólo si existe). `line`: en una fila de una sola
     línea, la ruta se recorta y el icono sigue a la vista. Con otra ruta, elemento nuevo (sin el
     estado resuelto de la anterior). -->
{#snippet fileRef(text: string, ref: string, line: boolean)}{#key ref}<span class="fref" class:fref-line={line} data-ref={ref}><span class="fp">{text}</span><button class="open-file" type="button" tabindex="-1" aria-label="Abrir">{@html OPEN_ICON}</button></span>{/key}{/snippet}

<!-- Pastilla con la hora del mensaje (tuyo o de Claude): sale al pasar por encima. -->
{#snippet when(at: number)}{#if at}<span class="msg-time" title={fullDate(at)}>{ago(at, clock.now)}</span>{/if}{/snippet}

<!-- Texto de un mensaje tuyo: las rutas de imágenes (pegadas, soltadas…) se ven como su marca
     («imagen 1», «foto.jpg»), que al pulsarla la enseña en grande, y su miniatura con el nombre va
     debajo; las demás rutas, con su icono de abrir. -->
{#snippet userBody(text: string, images: ImgRef[])}
  {@const segs = splitAttachments(text)}
  {@const files = attFiles(segs)}
  {#if text}<div class="body">{#each segs as s, k (k)}{#if 'att' in s}<span class="att-pill" data-path={s.att.path} title="Ver {s.att.path}">{s.att.label}</span>{:else}{#each splitRefs(s.text) as x, j (j)}{#if x.ref}{@render fileRef(x.text, x.ref, false)}{:else}{x.text}{/if}{/each}{/if}{/each}</div>{/if}
  {#if ui.showMedia && (images.length || files.length)}
    <MediaStrip {images} {files} />
  {:else if images.length && !text}<div class="body">[imagen]</div>{/if}
{/snippet}

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="transcript" bind:this={root} onclick={onClick} onmousedown={onMouseDown}>
  {#each rows as row (row.i)}
    {#if row.t === 'user'}
      <div
        class="msg user"
        class:midturn={row.midTurn}
        class:withdrawn={row.withdrawn}
        data-i={row.i}
        use:morph={bubbleId(row.text)}
      >
        <div class="msg-tools"><button data-act="plain" title="Copiar texto">Copiar</button></div>
        {@render userBody(row.text, row.images)}
        {#if row.midTurn}<div class="qtag">enviado mientras trabajaba</div>{/if}
        {#if row.withdrawn}
          <div class="qtag" title="La interrumpiste antes de que Claude respondiera: no forma parte de la conversación">
            interrumpida
          </div>
        {/if}
        {@render when(row.at)}
      </div>
    {:else if row.t === 'assistant'}
      <div class="msg assistant" data-i={row.i} data-cwd={row.cwd || undefined}>
        <div class="msg-tools">
          <button data-act="rich" title="Copiar con formato (HTML + markdown)">Copiar</button>
          <button data-act="plain" title="Copiar sólo texto, sin formato">Texto</button>
          <button data-act="md" title="Copiar el markdown original">MD</button>
        </div>
        <div class="body md">{@html renderMarkdown(row.text)}</div>
        {@render when(row.at)}
      </div>
    {:else if row.t === 'tools'}
      {@const running = row.tools.some((t) => !t.result)}
      {@const failed = row.tools.some((t) => t.result?.isError)}
      {@const lastTool = row.tools[row.tools.length - 1].item}
      <details class="tools">
        <summary>
          <span class="tool-ic"
            >{#if running && busy}<Spinner size={12} />{:else}{failed ? '✗' : '⚙'}{/if}</span
          >
          <span class="tool-names">{toolsLabel(row.tools)}</span>
          <span class="tool-last"
            >{#if FILE_TOOLS.has(lastTool.name)}{@render fileRef(lastTool.summary, lastTool.summary, true)}{:else}{lastTool.summary}{/if}</span
          >
        </summary>
        {#each row.tools as t (t.item.id)}
          <details class="tool">
            <summary>
              <span class="tname">{t.item.name}</span>
              <span class="tsum"
                >{#if FILE_TOOLS.has(t.item.name)}{@render fileRef(t.item.summary, t.item.summary, true)}{:else}{t.item.summary}{/if}</span
              >
              <span class="tstate" class:err={t.result?.isError}
                >{t.result ? (t.result.isError ? '✗' : '✓') : '…'}</span
              >
            </summary>
            <pre class="tin">{t.item.input}</pre>
            {#if t.result}<pre class="tout" class:err={t.result.isError}>{t.result.text}</pre>{/if}
          </details>
        {/each}
      </details>
      <!-- Lo que Claude ha mirado (fotos, capturas…), a la vista sin desplegar las herramientas. -->
      {@const imgs = ui.showMedia ? row.tools.flatMap((t) => t.result?.images ?? []) : []}
      {#if imgs.length}<MediaStrip images={imgs} />{/if}
    {:else if row.t === 'agent'}
      <AgentCard
        i={row.i}
        item={row.item}
        result={row.result}
        notice={row.notice}
        report={row.text}
        {live}
        onOpen={onOpenAgent}
      />
    {:else if row.t === 'command'}
      <div class="cmd"><code>{row.name === '!' ? '! ' : row.name + ' '}{row.args}</code></div>
    {:else if row.t === 'output'}
      <pre class="output">{row.text}</pre>
    {:else if row.t === 'summary'}
      <details class="compact-summary">
        <summary>Conversación compactada · ver resumen</summary>
        <div class="body md">{@html renderMarkdown(row.text)}</div>
      </details>
    {:else}
      <div class="notice">{row.text}</div>
    {/if}
  {/each}
  {#each pending as p (p.id)}
    <div class="msg user pending" use:morph={bubbleId(p.text)}>
      {@render userBody(p.text, [])}
      <div class="qtag">en cola: Claude lo leerá en cuanto pueda</div>
      {@render when(p.at)}
    </div>
  {/each}
  {#each outgoing as o (o.id)}
    <div class="msg user outgoing" use:flyIn={o} use:morph={bubbleId(o.text)}>
      {@render userBody(o.text, [])}
      {#if o.waiting}<div class="qtag">se enviará en cuanto Claude arranque</div>{/if}
      {@render when(o.at)}
    </div>
  {/each}
</div>

{#if big}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="lightbox" onclick={closeImage}><img src={big} alt="" /></div>
{/if}
