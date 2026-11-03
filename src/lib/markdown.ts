import MarkdownIt from 'markdown-it';
import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import csharp from 'highlight.js/lib/languages/csharp';
import css from 'highlight.js/lib/languages/css';
import diff from 'highlight.js/lib/languages/diff';
import dockerfile from 'highlight.js/lib/languages/dockerfile';
import go from 'highlight.js/lib/languages/go';
import ini from 'highlight.js/lib/languages/ini';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import markdown from 'highlight.js/lib/languages/markdown';
import python from 'highlight.js/lib/languages/python';
import rust from 'highlight.js/lib/languages/rust';
import shell from 'highlight.js/lib/languages/shell';
import sql from 'highlight.js/lib/languages/sql';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';
import plaintext from 'highlight.js/lib/languages/plaintext';
import { OPEN_ICON, codeRef, linkifiedFile, splitRefs } from './filerefs';

const langs: Record<string, any> = {
  bash, csharp, css, diff, dockerfile, go, ini, java, javascript, json, markdown,
  python, rust, shell, sql, typescript, xml, yaml, plaintext,
};
for (const [n, l] of Object.entries(langs)) hljs.registerLanguage(n, l);
const alias: Record<string, string> = {
  sh: 'bash', zsh: 'bash', console: 'shell', js: 'javascript', ts: 'typescript',
  py: 'python', rs: 'rust', cs: 'csharp', html: 'xml', svelte: 'xml', yml: 'yaml',
  toml: 'ini', md: 'markdown', text: 'plaintext', txt: 'plaintext', jsonl: 'json',
  tsql: 'sql', mssql: 'sql', patch: 'diff',
};

function esc(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

const md: MarkdownIt = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: false,
  typographer: false,
});

md.renderer.rules.fence = (tokens, idx) => {
  const t = tokens[idx];
  const raw = (t.info || '').trim().split(/\s+/)[0].toLowerCase();
  const lang = alias[raw] || raw;
  let body: string;
  if (lang && hljs.getLanguage(lang)) {
    body = hljs.highlight(t.content, { language: lang, ignoreIllegals: true }).value;
  } else {
    body = esc(t.content);
  }
  const label = raw ? `<span class="code-lang">${esc(raw)}</span>` : '';
  return (
    `<div class="code-block">${label}` +
    `<button class="copy-code" type="button" title="Copiar código">Copiar</button>` +
    `<pre><code class="hljs${lang ? ' language-' + esc(lang) : ''}">${body}</code></pre></div>`
  );
};

// Tablas anchas: envolver para scroll horizontal.
md.renderer.rules.table_open = () => '<div class="table-wrap"><table>';
md.renderer.rules.table_close = () => '</table></div>';

// Rutas de fichero (en código en línea o en el texto): marcadas, con su icono de abrir, que sólo
// se ve si el fichero existe (filerefs.ts).
const fref = (inner: string, ref: string) =>
  `<span class="fref" data-ref="${esc(ref)}">${inner}` +
  `<button class="open-file" type="button" tabindex="-1" aria-label="Abrir el fichero">${OPEN_ICON}</button></span>`;
md.renderer.rules.code_inline = (tokens, idx) => {
  const c = tokens[idx].content;
  const code = `<code>${esc(c)}</code>`;
  const ref = codeRef(c);
  return ref ? fref(code, ref) : code;
};
md.renderer.rules.text = (tokens, idx, _opts, env) => {
  const c = tokens[idx].content;
  if (env.inLink) return esc(c);
  return splitRefs(c)
    .map((x) => (x.ref ? fref(esc(x.text), x.ref) : esc(x.text)))
    .join('');
};
// Dentro de un enlace no se buscan rutas. Un «README.md» o «app.rs» suelto, linkify lo toma por un
// dominio (http://README.md): se deja como texto, que así lleva su icono si es un fichero.
md.renderer.rules.link_open = (tokens, idx, opts, env, self) => {
  if (tokens[idx].markup === 'linkify' && linkifiedFile(tokens[idx + 1]?.content ?? '')) {
    env.fileLink = true;
    return '';
  }
  env.inLink = (env.inLink ?? 0) + 1;
  return self.renderToken(tokens, idx, opts);
};
md.renderer.rules.link_close = (tokens, idx, opts, env, self) => {
  if (env.fileLink) {
    env.fileLink = false;
    return '';
  }
  env.inLink -= 1;
  return self.renderToken(tokens, idx, opts);
};

const cache = new Map<string, string>();

export function renderMarkdown(src: string): string {
  const hit = cache.get(src);
  if (hit !== undefined) return hit;
  const html = md.render(src, {});
  if (cache.size > 2000) cache.clear();
  cache.set(src, html);
  return html;
}

export function escapeHtml(s: string): string {
  return esc(s);
}
