// Hora de los mensajes: «hace 5 minutos», «ayer a las 14:05»… y la fecha completa para el tooltip.
// Un único reloj para toda la app (cada 30 s): cada mensaje sólo lee `clock.now` y Svelte cambia el
// texto de los que lo necesiten, sin un temporizador por mensaje.

class Clock {
  now = $state(Date.now());
  constructor() {
    setInterval(() => (this.now = Date.now()), 30_000);
  }
}
export const clock = new Clock();

const hm = (t: number) => new Date(t).toLocaleTimeString('es-ES', { hour: '2-digit', minute: '2-digit' });

/** Cuánto hace, en palabras. */
export function ago(t: number, now: number): string {
  const s = Math.max(0, (now - t) / 1000);
  if (s < 45) return 'hace unos segundos';
  if (s < 90) return 'hace un minuto';
  const m = Math.round(s / 60);
  if (m < 60) return `hace ${m} minutos`;
  if (m < 90) return 'hace una hora';
  const h = Math.round(m / 60);
  if (h < 24) return `hace ${h} horas`;
  // Desde un día atrás, por días del calendario.
  const day = (x: number) => {
    const d = new Date(x);
    d.setHours(0, 0, 0, 0);
    return d.getTime();
  };
  const days = Math.round((day(now) - day(t)) / 86_400_000);
  if (days <= 1) return `ayer a las ${hm(t)}`;
  if (days < 7) return `hace ${days} días`;
  const d = new Date(t);
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  return d.toLocaleDateString('es-ES', sameYear ? { day: 'numeric', month: 'short' } : { day: 'numeric', month: 'short', year: 'numeric' });
}

/** «martes, 3 de noviembre de 2026, 16:59:12». */
export function fullDate(t: number): string {
  return new Date(t).toLocaleString('es-ES', {
    weekday: 'long',
    day: 'numeric',
    month: 'long',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}
