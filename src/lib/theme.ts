// Tema de color: claro, oscuro, sepia o el del sistema. Se aplica con `data-theme` en <html>;
// el último elegido se recuerda en el navegador para pintar bien desde el primer instante.
export type Theme = 'system' | 'light' | 'dark' | 'sepia';

const KEY = 'ccdesk.theme';
const mq = window.matchMedia('(prefers-color-scheme: dark)');
let current: Theme = 'system';

export function applyTheme(t: Theme) {
  current = t;
  document.documentElement.dataset.theme = t === 'system' ? (mq.matches ? 'dark' : 'light') : t;
  try {
    localStorage.setItem(KEY, t);
  } catch {
    /* sin almacenamiento */
  }
}

export function cachedTheme(): Theme {
  try {
    const t = localStorage.getItem(KEY);
    if (t === 'light' || t === 'dark' || t === 'sepia' || t === 'system') return t;
  } catch {
    /* sin almacenamiento */
  }
  return 'system';
}

mq.addEventListener('change', () => {
  if (current === 'system') applyTheme('system');
});
