// Portapapeles vía evento `copy`: permite poner text/html y text/plain a la vez
// sin plugins (WebKitGTK entrega ambos al portapapeles CLIPBOARD de X11/Wayland).
import { quotePaths } from './files';

/** Las imágenes de tus mensajes se ven como su marca: al copiar, vuelve a ser su ruta. */
function pathsBack(clone: HTMLElement) {
  clone.querySelectorAll<HTMLElement>('.att-pill').forEach((p) => p.replaceWith(quotePaths([p.dataset.path ?? p.textContent ?? ''])));
  clone.querySelectorAll('.media-strip').forEach((n) => n.remove());
}

function writeViaCopyEvent(data: Record<string, string>): boolean {
  const handler = (e: ClipboardEvent) => {
    for (const [type, value] of Object.entries(data)) e.clipboardData?.setData(type, value);
    e.preventDefault();
  };
  // execCommand('copy') necesita una selección activa en WebKit: se usa un textarea efímero.
  const ta = document.createElement('textarea');
  ta.value = data['text/plain'] ?? ' ';
  ta.setAttribute('readonly', '');
  ta.style.position = 'fixed';
  ta.style.opacity = '0';
  ta.style.left = '-9999px';
  const prevFocus = document.activeElement as HTMLElement | null;
  document.body.appendChild(ta);
  ta.select();
  document.addEventListener('copy', handler, true);
  let ok = false;
  try {
    ok = document.execCommand('copy');
  } finally {
    document.removeEventListener('copy', handler, true);
    ta.remove();
    prevFocus?.focus?.({ preventScroll: true });
  }
  return ok;
}

export function copyPlain(text: string): boolean {
  return writeViaCopyEvent({ 'text/plain': text });
}

export function copyRich(html: string, plain: string): boolean {
  return writeViaCopyEvent({ 'text/html': html, 'text/plain': plain });
}

// Estilos en línea para que el formato sobreviva al pegar en Teams, Outlook, Confluence…
const INLINE: Record<string, string> = {
  pre: 'background:#f6f8fa;border:1px solid #d0d7de;border-radius:6px;padding:8px 10px;font-family:Consolas,"DejaVu Sans Mono",monospace;font-size:12px;white-space:pre;overflow:auto',
  code: 'font-family:Consolas,"DejaVu Sans Mono",monospace;font-size:12px;background:#f2f2f2;padding:0 3px;border-radius:3px',
  table: 'border-collapse:collapse',
  th: 'border:1px solid #c8c8c8;padding:4px 8px;background:#f2f2f2;text-align:left',
  td: 'border:1px solid #c8c8c8;padding:4px 8px',
  blockquote: 'border-left:3px solid #c8c8c8;margin:0;padding-left:10px;color:#555',
};

/** HTML apto para pegar: clona, quita botones/etiquetas de la UI y aplica estilos en línea. */
export function portableHtml(el: Element): string {
  const clone = el.cloneNode(true) as HTMLElement;
  clone.querySelectorAll('.copy-code, .code-lang, .msg-tools, .open-file').forEach((n) => n.remove());
  pathsBack(clone);
  clone.querySelectorAll('.table-wrap').forEach((w) => w.replaceWith(...Array.from(w.childNodes)));
  clone.querySelectorAll('.code-block').forEach((w) => w.replaceWith(...Array.from(w.childNodes)));
  for (const [tag, style] of Object.entries(INLINE)) {
    clone.querySelectorAll(tag).forEach((n) => {
      if (tag === 'code' && n.parentElement?.tagName === 'PRE') {
        (n as HTMLElement).removeAttribute('class');
        return;
      }
      (n as HTMLElement).setAttribute('style', style);
    });
  }
  clone.querySelectorAll('span[class^="hljs"]').forEach((s) => s.replaceWith(s.textContent ?? ''));
  return clone.innerHTML;
}

/** Texto sin formato tal como se ve (sin marcas markdown). */
export function visibleText(el: Element): string {
  const clone = el.cloneNode(true) as HTMLElement;
  clone.querySelectorAll('.copy-code, .code-lang, .msg-tools, .open-file').forEach((n) => n.remove());
  pathsBack(clone);
  // Las viñetas y números de lista no forman parte de innerText: se añaden a mano.
  clone.querySelectorAll('li').forEach((li) => {
    const parent = li.parentElement;
    let mark = '• ';
    if (parent?.tagName === 'OL') {
      const start = Number(parent.getAttribute('start') ?? 1);
      mark = `${start + Array.from(parent.children).indexOf(li)}. `;
    }
    li.insertBefore(document.createTextNode(mark), li.firstChild);
  });
  // innerText necesita que el nodo esté en el DOM para respetar saltos de bloque.
  clone.style.position = 'fixed';
  clone.style.left = '-99999px';
  clone.style.whiteSpace = 'normal';
  document.body.appendChild(clone);
  const t = clone.innerText;
  clone.remove();
  return t.replace(/\n{3,}/g, '\n\n').trim();
}
