// Tooltips instantáneos en toda la app. El nativo (atributo `title`) tarda ~0,5 s en WebKitGTK y
// no se puede acelerar, así que al entrar en un elemento con `title` se le quita (no sale el
// nativo) y se pinta uno propio al momento; al salir se le devuelve. Así el DOM sigue como lo
// dejó Svelte salvo mientras el ratón está encima, y los `title` siguen sirviendo de siempre.

let tip: HTMLDivElement | null = null;
let cur: Element | null = null; // elemento cuyo tooltip se enseña
let held: string | null = null; // su `title` original, para devolvérselo
let muted: Element | null = null; // tras un clic no vuelve a salir hasta cambiar de elemento
let px = 0;
let py = 0;

function ensureTip(): HTMLDivElement {
  if (!tip) {
    tip = document.createElement('div');
    tip.className = 'tip';
    tip.setAttribute('role', 'tooltip');
    document.body.appendChild(tip);
  }
  return tip;
}

/** Quita el `title` del elemento (para que no salga el nativo) y lo guarda. */
function take(el: Element) {
  const t = el.getAttribute('title');
  if (t === null) return;
  held = t;
  el.removeAttribute('title');
}

function restore() {
  if (cur && held !== null && cur.isConnected && !cur.hasAttribute('title')) {
    cur.setAttribute('title', held);
  }
  cur = null;
  held = null;
}

function place(el: Element) {
  const t = ensureTip();
  const r = el.getBoundingClientRect();
  const w = t.offsetWidth;
  const h = t.offsetHeight;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const gap = 6;
  let left: number;
  let top: number;
  if (r.height > 64 || r.width > 480) {
    // Elementos grandes (filas, bloques): junto al puntero, no bajo el elemento entero.
    left = px - w / 2;
    top = py + 18;
    if (top + h > vh - 4) top = py - 10 - h;
  } else {
    left = r.left + r.width / 2 - w / 2;
    top = r.bottom + gap;
    if (top + h > vh - 4) top = r.top - gap - h;
  }
  left = Math.max(4, Math.min(left, vw - w - 4));
  top = Math.max(4, top);
  t.style.transform = `translate(${Math.round(left)}px, ${Math.round(top)}px)`;
}

function show(el: Element) {
  if (cur !== el) restore();
  cur = el;
  take(el);
  const text = held ?? '';
  if (!text.trim()) {
    ensureTip().classList.remove('on');
    return;
  }
  const t = ensureTip();
  t.textContent = text;
  t.classList.add('on');
  place(el);
}

function hide() {
  tip?.classList.remove('on');
  restore();
}

export function installTooltips() {
  document.addEventListener(
    'pointerover',
    (e) => {
      px = e.clientX;
      py = e.clientY;
      const target = e.target as Element | null;
      let el: Element | null = target?.closest?.('[title]') ?? null;
      // Dentro del que ya se enseña (su `title` está quitado): se queda, salvo que haya otro
      // elemento con `title` más adentro.
      if (cur && target && cur.contains(target) && (!el || el.contains(cur))) el = cur;
      if (el === cur) return;
      if (muted && el !== muted) muted = null;
      if (!el || el === muted) {
        hide();
        return;
      }
      show(el);
    },
    true,
  );
  // Fuera de la ventana.
  document.addEventListener('pointerout', (e) => {
    if (!e.relatedTarget) hide();
  });
  // Un clic, la rueda, el teclado o perder el foco lo esconden (como el nativo).
  const dismiss = () => {
    if (cur) muted = cur;
    hide();
  };
  document.addEventListener('pointerdown', dismiss, true);
  document.addEventListener('wheel', dismiss, { capture: true, passive: true });
  document.addEventListener('keydown', dismiss, true);
  window.addEventListener('blur', hide);
  window.addEventListener('scroll', hide, true);
  // Si Svelte cambia el `title` del elemento que se está enseñando, se actualiza al momento.
  new MutationObserver((list) => {
    for (const m of list) {
      if (m.type !== 'attributes') continue;
      const el = m.target as Element;
      if (el === cur && el.hasAttribute('title')) show(el);
      else if (!cur && el.hasAttribute('title') && el.matches(':hover') && el !== muted) {
        const inner = document.querySelectorAll(':hover');
        const deepest = inner[inner.length - 1];
        if (deepest && deepest.closest('[title]') === el) show(el);
      }
    }
    if (cur && !cur.isConnected) hide();
  }).observe(document.body, { attributes: true, attributeFilter: ['title'], subtree: true, childList: true });
}
