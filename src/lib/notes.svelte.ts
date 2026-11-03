// To-dos de la sesión abierta (compartido entre el panel derecho y la caja de texto).
import { api, type Note } from './api';

function newId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  }
}

class NotesStore {
  key = $state<string | null>(null);
  list = $state<Note[]>([]);
  /** id de la tarjeta recién creada (para darle el foco). */
  fresh = $state<string | null>(null);
  private timer: ReturnType<typeof setTimeout> | undefined;

  async load(key: string | null) {
    if (key === this.key) return;
    this.flush();
    this.key = key;
    this.list = [];
    if (!key) return;
    try {
      const l = await api.notesGet(key);
      if (this.key === key) this.list = l;
    } catch {
      /* sin to-dos */
    }
  }

  /** `focus`: la tarjeta nueva toma el foco para escribir en ella («+ Nuevo to-do»); desde la
   *  caja (Alt+Enter) no, para seguir escribiendo allí. */
  add(text = '', focus = true): Note {
    const n: Note = { id: newId(), text, created: Date.now() };
    this.list.push(n);
    this.fresh = focus ? n.id : null;
    this.save(true);
    return n;
  }

  update(id: string, text: string) {
    const n = this.list.find((x) => x.id === id);
    if (n && n.text !== text) {
      n.text = text;
      this.save();
    }
  }

  remove(id: string) {
    this.list = this.list.filter((x) => x.id !== id);
    this.save(true);
  }

  save(now = false) {
    clearTimeout(this.timer);
    if (now) this.flush();
    else this.timer = setTimeout(() => this.flush(), 400);
  }

  flush() {
    clearTimeout(this.timer);
    this.timer = undefined;
    if (this.key) api.notesSave(this.key, $state.snapshot(this.list)).catch(() => {});
  }
}

export const notes = new NotesStore();
