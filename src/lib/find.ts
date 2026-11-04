// Ctrl+F: buscar en el texto que se ve de la conversación. Se pliega como en la búsqueda de
// sesiones (sin mayúsculas ni tildes), pero carácter a carácter: así cada coincidencia vuelve a su
// sitio exacto del DOM y se resalta con un Range, sin tocar lo que pinta Svelte.

/** Lo que no es conversación: botones (copiar, abrir…), horas, etiquetas de la cola, iconos. */
const SKIP_TAGS = new Set(['BUTTON', 'svg', 'STYLE', 'SCRIPT', 'TEMPLATE', 'TEXTAREA', 'INPUT']);
const SKIP_CLASSES = ['msg-tools', 'msg-time', 'qtag'];
/** No parten el texto: una búsqueda puede atravesarlos («**negrita** y más»). Los demás sí. */
const INLINE = new Set([
  'A', 'ABBR', 'B', 'BDI', 'BDO', 'CITE', 'CODE', 'DATA', 'DEL', 'DFN', 'EM', 'I', 'INS', 'KBD',
  'MARK', 'Q', 'S', 'SAMP', 'SMALL', 'SPAN', 'STRONG', 'SUB', 'SUP', 'TIME', 'U', 'VAR',
]);

export interface TextIndex {
  /** Texto de `root` plegado; entre bloques distintos, un `\0` que nada encuentra. */
  folded: string;
  nodes: Text[];
  /** Dónde empieza cada nodo en `folded`. */
  starts: number[];
}

const table = new Uint16Array(65536); // carácter → plegado (0: sin calcular)
function foldCode(c: number): number {
  if (c < 128) return c >= 65 && c <= 90 ? c + 32 : c;
  if (c >= 0xd800 && c <= 0xdfff) return c; // mitades de emoji y demás: tal cual
  if (table[c]) return table[c];
  const ch = String.fromCharCode(c);
  let l = ch.toLowerCase();
  if (l.length !== 1) l = ch;
  // Sin tilde: la letra base, si lo demás son sólo marcas (á, ñ, ç, ő…; no el hangul, p. ej.).
  const d = l.normalize('NFD');
  let f = l.charCodeAt(0);
  if (d.length > 1 && /^\p{M}+$/u.test(d.slice(1))) {
    const b = d[0].toLowerCase();
    f = (b.length === 1 ? b : d[0]).charCodeAt(0);
  }
  table[c] = f;
  return f;
}

const chars: string[] = [];
const foldOne = (ch: string) => {
  const c = ch.charCodeAt(0);
  return (chars[c] ??= String.fromCharCode(foldCode(c)));
};
/** Minúsculas y sin tildes, con la misma longitud (en unidades UTF-16) que `s`. Sólo se tocan las
 *  mayúsculas y lo que no es ASCII: el resto ya está plegado. */
export function fold(s: string): string {
  return s.replace(/[A-Z\u0080-￿]/g, foldOne);
}

/** Lo plegado de cada nodo de texto, mientras no cambie: al llegar texto nuevo sólo se pliega eso. */
const foldedNodes = new WeakMap<Text, { data: string; folded: string }>();
function foldNode(n: Text): string {
  const data = n.data;
  const c = foldedNodes.get(n);
  if (c?.data === data) return c.folded;
  const folded = fold(data);
  foldedNodes.set(n, { data, folded });
  return folded;
}

function skipped(e: Element): boolean {
  if (SKIP_TAGS.has(e.tagName) || e.getAttribute('aria-hidden') === 'true') return true;
  const c = e.classList;
  return c.length > 0 && SKIP_CLASSES.some((k) => c.contains(k));
}

/** El texto de `root` (lo plegado, sus nodos y dónde empieza cada uno). */
export function indexText(root: Element): TextIndex {
  const parts: string[] = [];
  const nodes: Text[] = [];
  const starts: number[] = [];
  let pos = 0;
  let last: Element | null = null;
  const walk = (el: Element, block: Element) => {
    for (let n = el.firstChild; n; n = n.nextSibling) {
      if (n.nodeType === Node.TEXT_NODE) {
        const t = (n as Text).data;
        if (!t) continue;
        if (last && block !== last) {
          parts.push('\0');
          pos++;
        }
        last = block;
        nodes.push(n as Text);
        starts.push(pos);
        parts.push(foldNode(n as Text));
        pos += t.length;
      } else if (n.nodeType === Node.ELEMENT_NODE) {
        const e = n as Element;
        if (skipped(e)) continue;
        if (e.tagName === 'BR') {
          parts.push('\n'); // un salto de línea cuenta como espacio
          pos++;
        } else walk(e, INLINE.has(e.tagName) ? block : e);
      }
    }
  };
  walk(root, root);
  return { folded: parts.join(''), nodes, starts };
}

/** Dónde está `query` en el texto (inicio y fin), en orden; si son más de `cap`, las `cap` últimas
 *  (las más recientes, las que quedan cerca de lo que se ve). Un espacio vale por cualquier racha de
 *  blancos (en el HTML, un salto de línea dentro de un párrafo se ve como un espacio). */
export function findAll(ix: TextIndex, query: string, cap: number): { hits: [number, number][]; capped: boolean } {
  if (!query.trim()) return { hits: [], capped: false };
  const pat = fold(query)
    .replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    .replace(/\s+/g, '\\s+');
  const re = new RegExp(pat, 'g');
  // En anillo: se van pisando las más antiguas.
  const starts = new Int32Array(cap);
  const ends = new Int32Array(cap);
  let n = 0;
  for (let m = re.exec(ix.folded); m; m = re.exec(ix.folded)) {
    starts[n % cap] = m.index;
    ends[n % cap] = m.index + m[0].length;
    n++;
  }
  const first = n > cap ? n % cap : 0;
  const hits: [number, number][] = [];
  for (let i = 0; i < Math.min(n, cap); i++) {
    const j = (first + i) % cap;
    hits.push([starts[j], ends[j]]);
  }
  return { hits, capped: n > cap };
}

/** Una coincidencia: nodo y posición de su principio y de su final. No es un Range: con miles de
 *  Range vivos cada cambio del DOM sale caro (el navegador los pone al día uno a uno). */
export interface Match {
  sn: Text;
  so: number;
  en: Text;
  eo: number;
}

/** Nodo y posición del carácter `off` (el final de una coincidencia, con `end`). */
function locate(ix: TextIndex, off: number, end: boolean): [Text, number] {
  let lo = 0;
  let hi = ix.nodes.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (end ? ix.starts[mid] < off : ix.starts[mid] <= off) lo = mid;
    else hi = mid - 1;
  }
  const n = ix.nodes[lo];
  const k = off - ix.starts[lo];
  if (k <= n.length) return [n, Math.max(0, k)];
  // Cae entre dos nodos (el salto de un <br>): al principio del siguiente o al final de éste.
  const next = ix.nodes[lo + 1];
  return end || !next ? [n, n.length] : [next, 0];
}

export function matchAt(ix: TextIndex, start: number, end: number): Match {
  const [sn, so] = locate(ix, start, false);
  const [en, eo] = locate(ix, end, true);
  return { sn, so, en, eo };
}

/** Pone `r` sobre `m`; false si el DOM ya no cuadra (cambió después de buscar). */
export function place(r: Range, m: Match): boolean {
  try {
    r.setStart(m.sn, m.so);
    r.setEnd(m.en, m.eo);
    return true;
  } catch {
    return false;
  }
}
