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
 * o un aviso, con su texto). Los comandos (`/compact`…) no dejan entrega: los casa su propia línea
 * (a `/compact` a secas, el resumen de la compactación). Sólo si no llega nada de eso (versiones
 * que no lo marcaban) se da por salida la primera, al llegar el siguiente apunte o un mensaje sin
 * marca de entrega; mientras tanto, lo recién salido sigue en la cola hasta que se escribe su
 * entrega.
 * La cola vive en la memoria del proceso: si Claude Code se cierra con algo en ella (un aviso que
 * esperaba al siguiente turno, p. ej.), se pierde sin que se apunte su salida. Por eso, con
 * `since` (arranque del proceso actual, en ms), la réplica se vacía al llegar lo primero de ese
 * proceso; con `since` infinito (no hay proceso) no queda nada en cola.
 * `textOf`: texto de un apunte que no lo trae (por su índice en `items`).
 */
export function queueReplica(
  items: Item[],
  textOf: (idx: number) => string = () => '',
  since = 0,
): Queued[] {
  const q: Queued[] = [];
  let owed = 0; // salidas a la espera de su entrega
  let current = since <= 0; // ya ha llegado algo del proceso actual (o no se sabe cuándo arrancó)
  const drop = (ok: (x: Queued) => boolean) => {
    const k = q.findIndex(ok);
    if (k >= 0) q.splice(k, 1);
    return k >= 0;
  };
  const settle = () => {
    for (; owed > 0; owed--) q.shift();
  };
  /** Se ha ejecutado el comando `text` (`/compact`, `/todo-list`…): fuera su apunte, y con él la
   *  salida que tuviera pendiente. */
  const ran = (text: string) => {
    if (!drop((x) => !x.system && squash(x.raw || x.text) === text)) return false;
    if (owed > 0) owed--;
    return true;
  };
  items.forEach((it, idx) => {
    if (!current && Date.parse(it.ts) >= since) {
      current = true;
      q.length = 0;
      owed = 0;
    }
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
        // Un comando en cola sale sin marca de entrega: lo casa la línea del propio comando, que
        // llega al ejecutarse. Darlo por salida al primero de la cola fallaría si delante queda un
        // aviso interno que Claude Code consumió sin apuntarlo: se iría el aviso, que no se ve, y
        // el comando se quedaría «en cola» para siempre.
        if (it.kind === 'command' && ran(squash(it.args ? `${it.name} ${it.args}` : it.name))) break;
        // Un mensaje sin marca de entrega delante (versiones que no la apuntaban, o escrito con
        // Claude esperando): las salidas pendientes ya se entregaron.
        if (!(it.kind === 'user' && it.midTurn) && items[idx - 1]?.kind !== 'queueTaken') settle();
        break;
      case 'summary':
        // `/compact` a secas no deja item de comando (no se enseña: ya está el resumen); lo casa el
        // resumen con que acaba la compactación.
        ran('/compact');
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
  return current ? q : [];
}
