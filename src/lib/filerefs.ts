// Rutas de fichero o carpeta en la conversación: tras cada una, un icono para abrirla con su
// aplicación predeterminada (la carpeta, en el gestor de archivos). Al pintar se marcan las
// candidatas (`.fref` con `data-ref`) y luego el backend dice cuáles existen: sólo esas enseñan el
// icono. Una relativa se busca entre las rutas que han
// usado las herramientas (Read, Edit…), en la carpeta en la que estaba Claude al escribirla y en
// las que la contienen (suele escribirlas desde la raíz del proyecto aunque un `cd` lo haya dejado
// en una subcarpeta), en las demás carpetas por las que ha pasado la conversación y en la de la
// sesión.
import { api } from './api';

/** Icono «abrir» (Lucide external-link), en el color del texto. */
export const OPEN_ICON =
  '<svg class="ico" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/></svg>';

/** Imágenes: el icono las enseña en grande dentro de ccdesk, como las miniaturas. */
export const IMAGE_RE = /\.(?:png|jpe?g|gif|webp|bmp|svg)$/i;

/** Herramientas cuyo resumen es la ruta del fichero. */
export const FILE_TOOLS = new Set(['Read', 'Write', 'Edit', 'MultiEdit', 'NotebookEdit']);

const SEG = String.raw`[\p{L}\p{N}_.+@-]+`;
/** Nombre suelto con extensión que lleva alguna letra (app.rs, README.md; no «v1.2» ni «etc.»). */
const BARE = /^[\p{L}\p{N}_.+@-]*[\p{L}\p{N}_+@-]\.[\p{L}\p{N}]*\p{L}[\p{L}\p{N}]*$/u;
/** En texto: ruta (absoluta, ~/, ./ o relativa; con la barra final si es una carpeta) o nombre
 *  suelto, con `:línea[:col]` opcional; no a media palabra ni dentro de una URL. */
const TEXT_RE = new RegExp(String.raw`(?<![\p{L}\p{N}_./~:@\\-])((?:~/|\.{1,2}/|/)?(?:${SEG}/)*${SEG}/?)((?::\d+){0,2})`, 'gu');

function plausible(p: string): boolean {
  if (p.length < 3 || p.length > 400) return false;
  if (p.includes('/')) return !p.startsWith('//') && !p.endsWith('//');
  return BARE.test(p);
}

/** Código en línea que es una ruta entera (`src/a.ts`, `~/x/y.md:12`, `/tmp/a b.png`, `app.rs`). */
export function codeRef(code: string): string | null {
  let t = code.trim();
  if (!t || t.includes('://') || /[*?<>|"'`$(){}[\]\n\\]/.test(t) || t.startsWith('-')) return null;
  t = t.replace(/(?::\d+){1,2}$/, '');
  // Con espacios, sólo las absolutas (`/tmp/a b.png`); si no, es un comando o una frase.
  if (/\s/.test(t) && !/^(?:~\/|\/)/.test(t)) return null;
  return plausible(t) ? t : null;
}

/** Texto troceado en lo que es ruta (`ref`) y lo que no. */
export function splitRefs(s: string): { text: string; ref?: string }[] {
  const out: { text: string; ref?: string }[] = [];
  let last = 0;
  for (const m of s.matchAll(TEXT_RE)) {
    const start = m.index ?? 0;
    // El punto o la coma que cierran la frase no son de la ruta.
    const path = m[2] ? m[1] : m[1].replace(/[.,]+$/, '');
    if (!plausible(path)) continue;
    const end = start + path.length + (m[2] ?? '').length;
    if (start > last) out.push({ text: s.slice(last, start) });
    out.push({ text: s.slice(start, end), ref: path });
    last = end;
  }
  if (last < s.length) out.push({ text: s.slice(last) });
  return out;
}

/** Lo que linkify toma por un dominio sin serlo: un nombre de fichero (README.md, app.rs…). */
export function linkifiedFile(text: string): boolean {
  return /^[\p{L}\p{N}_.+-]+\.(?:md|rs|py|sh|pl|cc|tf|mk|ml|so|in)$/iu.test(text);
}

function join(dir: string, rel: string): string {
  const out: string[] = [];
  for (const p of `${dir}/${rel}`.split('/')) {
    if (p === '' || p === '.') continue;
    if (p === '..') out.pop();
    else out.push(p);
  }
  return '/' + out.join('/');
}

/** La carpeta y las que la contienen, de la más cercana a la más lejana (hasta cuatro niveles). */
function upwards(dir: string): string[] {
  const out: string[] = [];
  for (let d = dir.replace(/\/+$/, ''), i = 0; d && i < 5; d = d.slice(0, d.lastIndexOf('/')), i++) out.push(d);
  return out;
}

/** Dónde buscar las rutas relativas: lo que la conversación sabe de sus carpetas. */
export interface RefCtx {
  /** Carpeta de la sesión. */
  cwd: string;
  /** Rutas absolutas que han usado las herramientas, en orden. */
  known: string[];
  /** Carpetas en las que ha estado Claude, de la más reciente a la más antigua. */
  cwds: string[];
}

/** Rutas absolutas en las que puede estar `ref`, de la más probable a la menos. */
function candidates(ref: string, msgCwd: string, ctx: RefCtx): string[] {
  if (ref.startsWith('/') || ref.startsWith('~/')) return [ref];
  const rel = ref.replace(/^\.\//, '');
  const out: string[] = [];
  // La más reciente primero: si hay dos `index.ts`, la que se acaba de tocar.
  const { known } = ctx;
  if (!rel.startsWith('../')) for (let i = known.length - 1; i >= 0; i--) if (known[i].endsWith('/' + rel)) out.push(known[i]);
  for (const d of [...upwards(msgCwd), ...ctx.cwds, ctx.cwd]) if (d.startsWith('/')) out.push(join(d, rel));
  return [...new Set(out)];
}

// ruta absoluta → qué es (1 fichero, 2 carpeta, 0 no existe); lo que no existía se vuelve a mirar
// pasado un rato (puede crearse luego).
const seen = new Map<string, { kind: number; at: number }>();
const running = new WeakMap<HTMLElement, { again: boolean }>();

/** Resuelve las rutas marcadas de `root` que aún no lo están: `data-st` ok (con `data-abs`) o no. */
export async function resolveFileRefs(root: HTMLElement, ctx: RefCtx): Promise<void> {
  const r = running.get(root);
  if (r) {
    r.again = true;
    return;
  }
  const state = { again: false };
  running.set(root, state);
  try {
    do {
      state.again = false;
      const plan = [...root.querySelectorAll<HTMLElement>('.fref:not([data-st])')].map((el) => ({
        el,
        cands: candidates(el.dataset.ref ?? '', el.closest<HTMLElement>('[data-cwd]')?.dataset.cwd ?? '', ctx),
      }));
      if (!plan.length) break;
      const now = Date.now();
      const need = [...new Set(plan.flatMap((p) => p.cands))].filter((c) => {
        const h = seen.get(c);
        return !h || (!h.kind && now - h.at > 15_000);
      });
      if (need.length) {
        const res = await api.filesExist(need);
        need.forEach((c, i) => seen.set(c, { kind: Number(res[i]) || 0, at: now }));
      }
      for (const { el, cands } of plan) {
        if (!el.isConnected || el.dataset.st) continue;
        const abs = cands.find((c) => (seen.get(c)?.kind ?? 0) > 0);
        el.dataset.st = abs ? 'ok' : 'no';
        if (abs) {
          el.dataset.abs = abs;
          const dir = seen.get(abs)?.kind === 2;
          if (dir) el.dataset.kind = 'dir';
          const title = dir ? `Abrir la carpeta ${abs}` : `${IMAGE_RE.test(abs) ? 'Ver' : 'Abrir'} ${abs}`;
          el.querySelector('.open-file')?.setAttribute('title', title);
        }
      }
    } while (state.again);
  } catch {
    // Sin respuesta del backend: se reintenta con el siguiente cambio de la conversación.
  } finally {
    running.delete(root);
  }
}
