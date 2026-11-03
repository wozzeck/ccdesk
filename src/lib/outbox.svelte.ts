// Mensajes recién enviados que aún no han aparecido en el transcript: se pintan como burbuja
// provisional al final de la conversación (y la animación de envío vuela hasta ella).
export interface Outgoing {
  id: number;
  key: string;
  text: string;
  /** Posición de la caja de texto al enviar (origen de la animación). */
  from: { left: number; top: number; width: number; height: number } | null;
  /** Ya se ha animado (no repetir al re-renderizar). */
  flown: boolean;
  /** En vuelo: mientras dura, el mensaje real espera para relevarla al aterrizar. */
  flying: boolean;
  /** Espera a que la sesión arranque: aún no se ha tecleado. */
  waiting: boolean;
  /** Hora del envío (ms). */
  at: number;
}

let seq = 0;

class Outbox {
  items = $state<Outgoing[]>([]);
  /** Texto de los mensajes en cola que Claude Code apuntó sin él (pasa, p. ej., durante /compact):
   *  el que se envió desde aquí, por sesión y hora del apunte. Sobrevive al cambio de sesión. */
  private queued = $state<Record<string, string>>({});
  /** Tamaño del transcript (índice global) al ver cada envío por primera vez: su mensaje real sólo
   *  puede estar después. Fuera del componente, para que un cambio de sesión no lo pierda. */
  readonly base = new Map<number, number>();

  remember(key: string, ts: string, text: string) {
    this.queued[`${key}\u0000${ts}`] = text;
  }

  recall(key: string, ts: string): string | undefined {
    return this.queued[`${key}\u0000${ts}`];
  }

  add(key: string, text: string, from: DOMRect | null, waiting = false): number {
    const id = ++seq;
    const r = from ? { left: from.left, top: from.top, width: from.width, height: from.height } : null;
    this.items.push({ id, key, text, from: r, flown: false, flying: false, waiting, at: Date.now() });
    if (!waiting) this.expire(id);
    return id;
  }

  /** La que esperaba a que arrancase la sesión ya se ha tecleado. */
  sent(id: number) {
    const o = this.items.find((x) => x.id === id);
    if (!o) return;
    o.waiting = false;
    this.expire(id);
  }

  /** Red de seguridad: si nunca aparece (p. ej. un /comando), se quita sola. */
  private expire(id: number) {
    setTimeout(() => this.remove(id), 20_000);
  }

  remove(id: number) {
    if (this.items.some((o) => o.id === id)) this.items = this.items.filter((o) => o.id !== id);
    this.base.delete(id);
  }
}

export const outbox = new Outbox();

const FLY_MS = 320;
const ease = (t: number) => 1 - Math.pow(1 - t, 3); // = cubic-bezier(0.33, 1, 0.68, 1)

/**
 * Acción: la burbuja provisional entra volando desde la caja de texto. Si la conversación estaba
 * abajo del todo, el desplazamiento para mostrarla se anima a la vez (con la misma curva), en
 * lugar de saltar de golpe; la marca `data-flying` en el contenedor evita el salto automático.
 */
export function flyIn(el: HTMLElement, o: Outgoing) {
  if (o.flown || !o.from) return;
  o.flown = true;
  o.flying = true;
  const from = o.from;
  const sc = el.closest('.conv') as HTMLElement | null;
  if (sc) sc.dataset.flying = '1';
  el.style.visibility = 'hidden';
  requestAnimationFrame(() => {
    const r = el.getBoundingClientRect();
    let start = 0;
    let delta = 0;
    if (sc) {
      start = sc.scrollTop;
      const end = sc.scrollHeight - sc.clientHeight;
      const wasBottom = end - start - (r.height + 28) < 80;
      delta = wasBottom ? Math.max(0, end - start) : 0;
    }
    const to = { left: r.left, top: r.top - delta, width: r.width, height: r.height };
    const ghost = el.cloneNode(true) as HTMLElement;
    ghost.classList.add('flying');
    Object.assign(ghost.style, {
      position: 'fixed',
      left: `${from.left}px`,
      top: `${from.top}px`,
      width: `${from.width}px`,
      height: `${from.height}px`,
      margin: '0',
      visibility: 'visible',
      zIndex: '90',
      pointerEvents: 'none',
    });
    document.body.appendChild(ghost);
    // Sin sombra: sin compositing por GPU, repintar una sombra difuminada en cada fotograma
    // hacía que la animación tartamudeara.
    const anim = ghost.animate(
      [
        {
          left: `${from.left}px`,
          top: `${from.top}px`,
          width: `${from.width}px`,
          height: `${from.height}px`,
          opacity: 0.55,
        },
        {
          left: `${to.left}px`,
          top: `${to.top}px`,
          width: `${to.width}px`,
          height: `${to.height}px`,
          opacity: 1,
        },
      ],
      { duration: FLY_MS, easing: 'cubic-bezier(0.33, 1, 0.68, 1)', fill: 'forwards' },
    );
    if (sc && delta > 0) {
      const t0 = performance.now();
      const step = (now: number) => {
        const t = Math.min(1, (now - t0) / FLY_MS);
        sc.scrollTop = start + delta * ease(t);
        if (t < 1) requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    }
    let finished = false;
    const done = () => {
      if (finished) return;
      finished = true;
      el.style.visibility = '';
      ghost.remove();
      if (sc) {
        delete sc.dataset.flying;
        if (delta > 0) sc.scrollTop = sc.scrollHeight;
      }
      o.flying = false; // ahora sí puede entrar el mensaje real, en el mismo sitio
    };
    anim.onfinish = done;
    anim.oncancel = done;
  });
}

// ── Relevo de burbujas ──
// Un mensaje tuyo pasa por varios estados (provisional → en cola → mensaje real, con o sin pie
// «enviado mientras trabajaba»), y cada uno es un elemento distinto. Al aparecer el siguiente con
// el mismo texto, crece o encoge desde el tamaño del anterior en vez de saltar.
const MORPH_MS = 240;
const gone = new Map<string, { h: number; opacity: number; foot: string; t: number }>();

export function morph(el: HTMLElement, id: string) {
  let h = 0;
  let opacity = 1;
  const measure = () => {
    if (el.isConnected) h = el.getBoundingClientRect().height;
  };
  const ro = new ResizeObserver(measure);
  ro.observe(el);
  // El anterior se destruye en el mismo ciclo de render (a veces después de montar éste).
  queueMicrotask(() => {
    if (!el.isConnected) return;
    opacity = parseFloat(getComputedStyle(el).opacity) || 1;
    measure();
    const prev = gone.get(id);
    gone.delete(id);
    if (!prev || performance.now() - prev.t > 600) return;
    const opts = { duration: MORPH_MS, easing: 'cubic-bezier(0.33, 1, 0.68, 1)' };
    if (Math.abs(h - prev.h) > 1) {
      el.style.overflow = 'hidden';
      const a = el.animate([{ height: `${prev.h}px` }, { height: `${h}px` }], opts);
      a.onfinish = a.oncancel = () => (el.style.overflow = '');
    }
    if (Math.abs(opacity - prev.opacity) > 0.05) el.animate([{ opacity: prev.opacity }, { opacity }], opts);
    const foot = el.querySelector('.qtag');
    if (foot && foot.textContent !== prev.foot)
      foot.animate([{ opacity: 0 }, { opacity: 1 }], { ...opts, duration: MORPH_MS * 1.5 });
  });
  return {
    destroy() {
      ro.disconnect();
      if (h) gone.set(id, { h, opacity, foot: el.querySelector('.qtag')?.textContent ?? '', t: performance.now() });
    },
  };
}
