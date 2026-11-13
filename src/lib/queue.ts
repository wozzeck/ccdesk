// Réplica de la cola de entrada de Claude Code a partir de lo que apunta en el transcript.
import type { Item } from './api';

/** Para comparar lo enviado con lo que apunta Claude Code, que no guarda los blancos tal cual (un
 *  tabulador tecleado lo apunta como espacios): cuentan las palabras, no los blancos entre ellas. */
export const squash = (s: string) => s.replace(/\s+/g, ' ').trim();

export interface Queued {
  /** Índice del apunte (`queueAdd`) en `items`. */
  idx: number;
  /** Texto tal como lo apuntó Claude Code (a veces vacío, p. ej. durante /compact). */
  raw: string;
  /** Texto para enseñarlo: el apuntado o, si no lo hay, el que da `textOf`. */
  text: string;
  /** Aviso interno (`<task-notification>`…): cuenta para la cola, pero no se enseña. */
  system: boolean;
}

/**
 * Lo que sigue en la cola de entrada de Claude Code, en orden.
 *
 * Una salida de la cola (`queueDequeue`) no dice cuál sale, y no siempre es la primera: Claude Code
 * saca por prioridad, y algunos avisos internos los descarta sin apuntarlo (se quedarían delante
 * para siempre). Por eso cada salida se casa con la entrega que la sigue (`queueTaken`: tu mensaje
 * o un aviso, con su texto). Sólo si no llega ninguna (versiones que no la marcaban) se da por
 * salida la primera, al llegar el siguiente apunte o un mensaje sin marca de entrega; mientras
 * tanto, lo recién salido sigue en la cola hasta que se escribe su entrega.
 * `textOf`: texto de un apunte que no lo trae (por su índice en `items`).
 */
export function queueReplica(items: Item[], textOf: (idx: number) => string = () => ''): Queued[] {
  const q: Queued[] = [];
  let owed = 0; // salidas a la espera de su entrega
  const drop = (ok: (x: Queued) => boolean) => {
    const k = q.findIndex(ok);
    if (k >= 0) q.splice(k, 1);
    return k >= 0;
  };
  const settle = () => {
    for (; owed > 0; owed--) q.shift();
  };
  items.forEach((it, idx) => {
    switch (it.kind) {
      case 'queueAdd':
        settle();
        q.push({ idx, raw: it.text, text: it.text || textOf(idx), system: it.system });
        break;
      case 'queueDequeue':
        owed++;
        break;
      case 'queueTaken': {
        const paired = owed > 0;
        if (paired) owed--;
        const got = squash(it.text);
        // Tu mensaje llega tal cual. Lo que llega como aviso viene recortado, y a veces envuelto
        // («Another Claude session sent a message: <agent-message…»); también llegan así textos
        // sin pinta de aviso (los programados, «Your claude.ai usage limit has reset…»): basta con
        // que coincida el principio.
        const same = (x: Queued) => {
          const r = squash(x.raw || x.text);
          if (!r) return false;
          if (!it.system) return r === got;
          const head = r.slice(0, 120);
          return x.system ? got.includes(head) : got.startsWith(head);
        };
        if (!drop(same) && paired) drop((x) => x.system === it.system);
        break;
      }
      case 'user':
      case 'command':
        // Un mensaje sin marca de entrega delante (versiones que no la apuntaban, o escrito con
        // Claude esperando): las salidas pendientes ya se entregaron.
        if (!(it.kind === 'user' && it.midTurn) && items[idx - 1]?.kind !== 'queueTaken') settle();
        break;
      case 'queueClear':
        q.length = 0;
        owed = 0;
        break;
      case 'queueRemove':
        // Retirado uno concreto (absorbido a mitad de turno, cancelado…): por su texto o, si no lo
        // trae, el primero sin texto.
        if (!drop((x) => x.raw === it.text) && !drop((x) => x.text === it.text)) drop((x) => !x.raw);
        break;
    }
  });
  return q;
}
