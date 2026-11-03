// Ficheros arrastrados e imágenes pegadas: se convierten en rutas dentro del texto, como hace la
// terminal (Claude abre esas rutas; las imágenes las ve con su herramienta de lectura).
// Las rutas de lo soltado las da Tauri (App.svelte): WebKit no las pasa a la página, por
// privacidad sólo entrega el nombre y el contenido de cada fichero.
import { invoke } from '@tauri-apps/api/core';
import { api } from './api';

/** Como las escribe una terminal al soltarlas: separadas por espacio, entre comillas si hace falta. */
export function quotePaths(paths: string[]): string {
  return paths.map((p) => (/[\s"'\\$`]/.test(p) ? `"${p.replace(/(["\\$`])/g, '\\$1')}"` : p)).join(' ');
}

/** Inserta `ins` en `value` sustituyendo [start, end), con un espacio de separación si hace falta. */
export function insertInto(value: string, start: number, end: number, ins: string): { value: string; caret: number } {
  const before = value.slice(0, start);
  const after = value.slice(end);
  const pre = before && !/\s$/.test(before) ? ' ' : '';
  const post = after && !/^\s/.test(after) ? ' ' : after ? '' : ' ';
  const piece = pre + ins + post;
  return { value: before + piece + after, caret: before.length + piece.length };
}

// ── imágenes adjuntas ──
// En la caja de texto, una imagen pegada (o soltada) no se ve como su ruta sino como una marca
// corta («[imagen 1]», «[foto.jpg]») en el sitio donde se puso, con su miniatura encima de la caja;
// al enviar, la marca se cambia por la ruta (Claude la abre con su herramienta de lectura). En la
// conversación, las rutas de imágenes de tus mensajes se ven igual: marca y miniatura.

const IMG_EXT = /\.(?:png|jpe?g|gif|webp|bmp|svg)$/i;

/** Imagen pegada en ccdesk (guardada como `~/.cache/ccdesk/pegados/pegado-<ms>.<ext>`). */
export function isPasted(path: string): boolean {
  return /\/\.cache\/ccdesk\/pegados\/pegado-\d+\.\w+$/.test(path);
}

export function isImagePath(path: string): boolean {
  return /^(?:~\/|\/)/.test(path) && IMG_EXT.test(path);
}

export interface Att {
  /** Marca en el texto: `[etiqueta]`. */
  token: string;
  path: string;
  /** «imagen 1» (pegada) o el nombre del fichero. */
  label: string;
}
export type AttSeg = { text: string } | { att: Att };

/** Ruta entre comillas (como la escribe quotePaths, con \" \\ \$ \`) o suelta, absoluta o con ~/;
 *  la suelta, no a media palabra ni dentro de una URL. */
const PATH_RE = /"((?:~\/|\/)(?:[^"\\]|\\.)+)"|(?<![\w.~:/\\-])((?:~\/|\/)[^\s"'`<>|]+)/g;

/** Nombre libre para una imagen: «imagen N» si es pegada; si no, el del fichero (con « 2»… si se
 *  repite entre las ya usadas). */
function labelFor(path: string, used: Set<string>, pastedN: number): string {
  if (isPasted(path)) return `imagen ${pastedN}`;
  const base = path.split('/').pop() || path;
  let label = base;
  for (let k = 2; used.has(label); k++) label = `${base} ${k}`;
  return label;
}

/** Trocea un texto en las rutas de imágenes que lleva (con su marca) y lo demás. Las pegadas se
 *  numeran por orden de aparición; una misma ruta repetida es la misma imagen. */
export function splitAttachments(s: string): AttSeg[] {
  const out: AttSeg[] = [];
  const byPath = new Map<string, Att>();
  const used = new Set<string>();
  let pasted = 0;
  let last = 0;
  for (const m of s.matchAll(PATH_RE)) {
    const quoted = m[1] !== undefined;
    const path = quoted ? m[1].replace(/\\(["\\$`])/g, '$1') : m[2].replace(/[.,;:!?)\]]+$/, '');
    if (!IMG_EXT.test(path)) continue;
    const start = m.index ?? 0;
    const end = start + (quoted ? m[0].length : path.length);
    let att = byPath.get(path);
    if (!att) {
      const label = labelFor(path, used, isPasted(path) ? ++pasted : 0);
      used.add(label);
      att = { token: `[${label}]`, path, label };
      byPath.set(path, att);
    }
    if (start > last) out.push({ text: s.slice(last, start) });
    out.push({ att });
    last = end;
  }
  if (last < s.length) out.push({ text: s.slice(last) });
  return out;
}

/** Texto con rutas → texto con marcas y la lista de imágenes. */
export function tokenize(s: string): { text: string; atts: Att[] } {
  const atts: Att[] = [];
  const text = splitAttachments(s)
    .map((x) => {
      if ('text' in x) return x.text;
      if (!atts.includes(x.att)) atts.push(x.att);
      return x.att.token;
    })
    .join('');
  return { text, atts };
}

/** Texto con marcas → texto con rutas (lo que se envía). */
export function expand(text: string, atts: Att[]): string {
  let out = text;
  for (const a of atts) out = out.split(a.token).join(quotePaths([a.path]));
  return out;
}

/** Las pegadas, renumeradas por orden de aparición en el texto (tras pegar una en medio). Las que
 *  ya no están en el texto se olvidan. */
export function renumber(text: string, atts: Att[]): { text: string; atts: Att[] } {
  const live = atts.filter((a) => text.includes(a.token));
  const pasted = live.filter((a) => isPasted(a.path)).sort((a, b) => text.indexOf(a.token) - text.indexOf(b.token));
  const next = new Map<Att, Att>();
  pasted.forEach((a, k) => {
    const label = `imagen ${k + 1}`;
    next.set(a, { token: `[${label}]`, path: a.path, label });
  });
  // Por marcadores intermedios: «[imagen 2]» → «[imagen 1]» no debe pisar otra que pase a 2.
  let out = text;
  pasted.forEach((a, k) => (out = out.split(a.token).join(`\u0000${k}\u0000`)));
  pasted.forEach((a, k) => (out = out.split(`\u0000${k}\u0000`).join(next.get(a)!.token)));
  // En el orden en que salen en el texto (el de sus miniaturas).
  const res = live.map((a) => next.get(a) ?? a).sort((a, b) => out.indexOf(a.token) - out.indexOf(b.token));
  return { text: out, atts: res };
}

/** Nueva imagen para la caja: su marca (la que ya tenga, si está) y la lista ampliada. */
export function addAttachment(atts: Att[], path: string): { att: Att; atts: Att[] } {
  const have = atts.find((a) => a.path === path);
  if (have) return { att: have, atts };
  const used = new Set(atts.map((a) => a.label));
  const n = Math.max(0, ...atts.filter((a) => isPasted(a.path)).map((a) => Number(a.label.split(' ')[1]) || 0)) + 1;
  const label = labelFor(path, used, n);
  const att = { token: `[${label}]`, path, label };
  return { att, atts: [...atts, att] };
}

/** Quita del texto la marca de una imagen (y un espacio junto a ella). */
export function removeToken(s: string, token: string): string {
  let out = s;
  for (let i = out.indexOf(token); i >= 0; i = out.indexOf(token)) {
    let a = i;
    let b = i + token.length;
    if (a > 0 && out[a - 1] === ' ') a--;
    else if (out[b] === ' ') b++;
    out = out.slice(0, a) + out.slice(b);
  }
  return out;
}

/**
 * Si lo pegado es una imagen, la guarda en disco y promete su ruta (y evita el pegado normal).
 * Si es texto, devuelve null y el pegado sigue su curso.
 */
export function pastedImage(e: ClipboardEvent): Promise<string> | null {
  const dt = e.clipboardData;
  if (!dt) return null;
  const item = Array.from(dt.items).find((i) => i.kind === 'file' && i.type.startsWith('image/'));
  const file = item?.getAsFile();
  if (file) {
    e.preventDefault();
    return file
      .arrayBuffer()
      .then((buf) => invoke<string>('save_image', new Uint8Array(buf), { headers: { 'x-mime': file.type } }));
  }
  // WebKitGTK no siempre pasa la imagen al evento: si no hay texto, se lee del portapapeles.
  if (!Array.from(dt.types).includes('text/plain')) {
    e.preventDefault();
    return api.clipboardImage();
  }
  return null;
}
