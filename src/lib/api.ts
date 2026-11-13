import { invoke, Channel } from '@tauri-apps/api/core';

export type Status = 'idle' | 'busy' | 'waiting' | 'blocked' | 'starting' | 'stopped';

export interface SessionView {
  key: string;
  sid: string;
  cwd: string;
  title: string;
  status: Status;
  managed: boolean;
  externalKind: string | null;
  pinned: boolean;
  exempt: boolean;
  startedAt: number;
  /** En las detenidas, la de su conversación (no la hora en que se detuvieron). */
  lastActivity: number;
  /** Cuándo se detuvo (sólo detenidas). */
  stoppedAt: number | null;
  pid: number | null;
  tmuxPane: string | null;
  /** Otros procesos con la misma sesión abierta. */
  dupes: string[];
  /** Dónde está en tmux (p. ej. claude:3). */
  location: string | null;
  /** To-dos apuntados. */
  notes: number;
  /** Subagentes trabajando ahora. */
  agents: AgentRow[];
  /** Claude espera, pero algo suyo sigue corriendo (bash en segundo plano, Monitor, subagentes). */
  background: boolean;
  /** Marca de la última respuesta de Claude: cambia cuando responde algo (null si no se sabe). */
  lastReply: number | null;
}

export interface AgentRow {
  toolUseId: string;
  agentId: string;
  description: string;
  agentType: string;
  depth: number;
}

export interface Note {
  id: string;
  text: string;
  created: number;
}

/** Algo que se puede escribir tras «/»: comando de Claude Code, skill o comando propio. */
export interface SlashCmd {
  name: string;
  description: string;
  kind: 'builtin' | 'skill' | 'command';
  /** Abre un diálogo en la terminal en vez de mandar algo a Claude. */
  interactive: boolean;
}

/** Coincidencia en la conversación de una sesión (la más reciente, con su contexto). */
export interface SearchHit {
  key: string;
  count: number;
  before: string;
  hit: string;
  after: string;
}

/** Ctrl+F en lo aún no cargado: un item (por su índice) y cuántas veces sale lo buscado. */
export interface FindHit {
  index: number;
  count: number;
}

/** Imagen del transcript: su línea (byte de inicio) y su orden en ella; se pide aparte. */
export interface ImgRef {
  path: string;
  offset: number;
  n: number;
}

export interface Settings {
  autostop: boolean;
  autostopMinutes: number;
  sidebarWidth: number;
  tmuxSession: string;
  notesOpen: boolean;
  allowYolo: boolean;
  theme: 'system' | 'light' | 'dark' | 'sepia';
  zoom: number;
  /** Notificación de escritorio cuando Claude termina. */
  notifyDone: boolean;
  /** Notificación de escritorio cuando Claude pregunta o pide permiso. */
  notifyAttention: boolean;
  /** Enseñar en la conversación las imágenes que Claude mira o que se le pegan. */
  showMedia: boolean;
  /** Orden de la lista: manual (arrastrando) o por actividad reciente (las fijadas, manual). */
  sessionOrder: 'manual' | 'recent';
  /** Modo de permisos con el que arrancan las sesiones nuevas ('' = el de Claude Code). */
  newSessionMode: string;
}

export interface RateWindow {
  used_percentage?: number;
  resets_at?: number;
}

/** Cambio de modelo o effort pedido con la sesión ocupada: se aplica en cuanto queda inactiva. */
export interface PendingChange {
  model?: { n: number; label: string };
  effort?: string;
}

/** Elegido con la sesión inactiva, encolado, o anulado lo encolado (se eligió el que ya tiene). */
export type ChangeResult = 'applied' | 'queued' | 'cancelled';

export interface Snapshot {
  sessions: SessionView[];
  activeStatus: any | null;
  activeMode: string | null;
  /** Sugerencia de Claude Code para el siguiente mensaje (texto atenuado de su caja, Tab). */
  activeSuggestion: { key: string; text: string } | null;
  /** Desglose de /usage (caché de Claude Code): semanales por modelo, etc. */
  usage: {
    fetchedAtMs: number | null;
    session: number | null;
    weekly: number | null;
    scoped: { label: string; percent: number; resetsAt: string; severity: string }[];
  } | null;
  lastActive: string | null;
  lastActiveSid: string | null;
  /** Sesiones con algo sin leer (guardadas: sobreviven a cerrar la app). */
  unread: string[];
  quota: { five_hour?: RateWindow; seven_day?: RateWindow } | null;
  /** Cambios de modelo y effort encolados, por sesión. */
  pending: Record<string, PendingChange>;
  settings: Settings;
  recentCwds: string[];
  home: string;
}

export type Item =
  | { kind: 'user'; ts: string; text: string; midTurn: boolean; images?: ImgRef[] }
  | { kind: 'command'; ts: string; name: string; args: string }
  | { kind: 'output'; ts: string; text: string }
  | { kind: 'assistant'; ts: string; msgId: string; text: string; cwd?: string }
  | { kind: 'tool'; ts: string; id: string; name: string; summary: string; input: string; shot?: string }
  | {
      kind: 'agent';
      ts: string;
      id: string;
      description: string;
      agentType: string;
      model: string;
      background: boolean;
      prompt: string;
    }
  | {
      kind: 'agentResult';
      ts: string;
      toolUseId: string;
      agentId: string;
      status: string;
      text: string;
      isError: boolean;
      model: string | null;
      durationMs: number | null;
      tokens: number | null;
      toolUses: number | null;
    }
  | { kind: 'agentReport'; ts: string; agentId: string; text: string }
  | {
      kind: 'taskNotice';
      ts: string;
      toolUseId: string;
      taskId: string;
      status: string;
      summary: string;
      result: string;
      durationMs: number | null;
      tokens: number | null;
      toolUses: number | null;
    }
  | { kind: 'toolResult'; ts: string; toolUseId: string; text: string; isError: boolean; images?: ImgRef[] }
  | { kind: 'notice'; ts: string; text: string }
  | { kind: 'summary'; ts: string; text: string }
  | { kind: 'queueAdd'; ts: string; text: string; system: boolean }
  | { kind: 'queueDequeue'; ts: string }
  /** Lo que entregó una salida de la cola: tu mensaje o (`system`) un aviso interno, recortado. */
  | { kind: 'queueTaken'; ts: string; text: string; system: boolean }
  | { kind: 'queueRemove'; ts: string; text: string }
  | { kind: 'queueClear'; ts: string }
  /** Tu mensaje `of` (índice global) se quedó fuera: lo interrumpiste antes de que respondiera. */
  | { kind: 'withdrawn'; ts: string; of: number };

export interface AgentActivity {
  agentId: string;
  toolUses: number;
  recent: string[];
  lastText: string;
  finished: boolean;
}

export interface TranscriptInit {
  key: string;
  sid: string;
  path: string;
  startIndex: number;
  total: number;
  items: Item[];
}

export interface TranscriptAppend {
  key: string;
  path: string;
  startIndex: number;
  items: Item[];
}

export const api = {
  snapshot: () => invoke<Snapshot>('snapshot_now'),
  transcriptOpen: (key: string | null) =>
    invoke<TranscriptInit | null>('transcript_open', { key }),
  clipboardImage: () => invoke<string>('clipboard_image'),
  resolvePaths: (paths: string[]) => invoke<string[]>('resolve_paths', { paths }),
  transcriptImage: (path: string, offset: number, n: number) =>
    invoke<string>('transcript_image', { path, offset, n }),
  transcriptBefore: (key: string, before: number, count: number) =>
    invoke<Item[]>('transcript_before', { key, before, count }),
  /** Del más reciente al más antiguo. */
  transcriptFind: (key: string, query: string, before: number) =>
    invoke<FindHit[]>('transcript_find', { key, query, before }),
  /** Item del mensaje que enseña el resultado de la búsqueda de sesiones (null: no sale). */
  transcriptLocate: (key: string, query: string) =>
    invoke<number | null>('transcript_locate', { key, query }),
  sessionNew: (cwd: string, name?: string, model?: string, effort?: string) =>
    invoke<string>('session_new', { cwd, name, model, effort }),
  /** Selector de carpetas del sistema; empieza en `start`. null si se cancela. */
  pickDir: (start: string) => invoke<string | null>('pick_dir', { start }),
  sessionStop: (key: string, force = false, backgroundOk = false) =>
    invoke<void>('session_stop', { key, force, backgroundOk }),
  sessionResume: (key: string) => invoke<void>('session_resume', { key }),
  sessionAdopt: (key: string) => invoke<string>('session_adopt', { key }),
  sessionAdoptAll: () =>
    invoke<{ adopted: string[]; skipped: string[] }>('session_adopt_all'),
  sessionForget: (key: string) => invoke<Snapshot>('session_forget', { key }),
  sessionRename: (key: string, name: string) => invoke<Snapshot>('session_rename', { key, name }),
  searchSessions: (query: string) => invoke<SearchHit[]>('search_sessions', { query }),
  slashCommands: (key: string) => invoke<SlashCmd[]>('slash_commands', { key }),
  /** Sin `order` (orden por recientes) sólo cambian las fijadas; el orden manual se conserva. */
  setLayout: (pinned: string[], order: string[] | null) =>
    invoke<Snapshot>('set_layout', { pinned, order }),
  setPinned: (key: string, pinned: boolean) => invoke<Snapshot>('set_pinned', { key, pinned }),
  setUnread: (key: string, unread: boolean) => invoke<void>('set_unread', { key, unread }),
  setExempt: (key: string, exempt: boolean) => invoke<Snapshot>('set_exempt', { key, exempt }),
  settingsUpdate: (settings: Settings) => invoke<Snapshot>('settings_update', { settings }),
  sendInput: (key: string, text: string) => invoke<void>('send_input', { key, text }),
  /** Teclea el texto en cuanto la sesión (que arranca o se reanuda) esté lista. */
  sendWhenReady: (key: string, text: string) => invoke<void>('send_when_ready', { key, text }),
  sendKeys: (key: string, keys: string[]) => invoke<void>('send_keys', { key, keys }),
  /** Esc a Claude. Devuelve el mensaje que Claude Code devolvió a su caja (y se ha vaciado), si lo hubo. */
  interrupt: (key: string) => invoke<string | null>('interrupt', { key }),
  sessionModels: (key: string) =>
    invoke<{ n: number; label: string; desc: string }[]>('session_models', { key }),
  sessionEfforts: (key: string) =>
    invoke<{ levels: string[]; current: string }>('session_efforts', { key }),
  sessionModes: (key: string) => invoke<string[]>('session_modes', { key }),
  usageRefresh: () => invoke<void>('usage_refresh'),
  setModel: (key: string, n: number, label: string) => invoke<ChangeResult>('session_set_model', { key, n, label }),
  setEffort: (key: string, effort: string) => invoke<ChangeResult>('session_set_effort', { key, effort }),
  setMode: (key: string, mode: string) => invoke<string>('session_set_mode', { key, mode }),
  sessionRestart: (key: string, mode?: string) => invoke<void>('session_restart', { key, mode }),
  termOpen: (key: string, cols: number, rows: number, onData: Channel<string>, gen: number) =>
    invoke<void>('term_open', { key, cols, rows, onData, gen }),
  termWrite: (data: string) => invoke<void>('term_write', { data }),
  termResize: (cols: number, rows: number) => invoke<void>('term_resize', { cols, rows }),
  termClose: () => invoke<void>('term_close'),
  openUrl: (url: string) => invoke<void>('open_url', { url }),
  openPath: (path: string) => invoke<void>('open_path', { path }),
  /** ¿Son ficheros (no carpetas)? Rutas absolutas o con `~/`. */
  /** 1 fichero, 2 carpeta, 0 no existe. */
  filesExist: (list: string[]) => invoke<number[]>('files_exist', { list }),
  openFile: (path: string) => invoke<void>('open_file', { path }),
  /** Bytes de una imagen de disco (png, jpg, gif, webp, bmp, svg). */
  imageFile: (path: string) => invoke<ArrayBuffer>('image_file', { path }),
  openScratchpad: (sid: string) => invoke<void>('open_scratchpad', { sid }),
  setZoom: (zoom: number) => invoke<void>('set_zoom', { zoom }),
  agentActivity: (toolUseId: string) =>
    invoke<AgentActivity | null>('agent_activity', { toolUseId }),
  agentTranscript: (toolUseId: string) => invoke<Item[]>('agent_transcript', { toolUseId }),
  notesGet: (key: string) => invoke<Note[]>('notes_get', { key }),
  notesSave: (key: string, notes: Note[]) => invoke<void>('notes_save', { key, notes }),
  promptHistory: (cwd: string, limit = 300) =>
    invoke<string[]>('prompt_history', { cwd, limit }),
};

export { Channel };

export function errText(e: unknown): string {
  return typeof e === 'string' ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

export function shortPath(p: string, home: string): string {
  if (!p) return '';
  let s = home && p.startsWith(home) ? '~' + p.slice(home.length) : p;
  const parts = s.split('/');
  if (parts.length > 4) s = '…/' + parts.slice(-2).join('/');
  return s;
}

export function ago(ms: number, now = Date.now()): string {
  if (!ms) return '';
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return 'ahora';
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min`;
  const h = Math.round(m / 60);
  if (h < 48) return `${h} h`;
  return `${Math.round(h / 24)} d`;
}

export const BUSY: Status[] = ['busy', 'starting'];
export const ATTENTION: Status[] = ['waiting', 'blocked'];

export function fmtDuration(ms: number | null | undefined): string {
  if (!ms && ms !== 0) return '';
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ${s % 60} s`;
  return `${Math.floor(m / 60)} h ${m % 60} min`;
}

export function fmtTokens(n: number | null | undefined): string {
  if (!n && n !== 0) return '';
  if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M';
  if (n >= 1e3) return Math.round(n / 1e3) + 'k';
  return String(n);
}

/** claude-sonnet-5-5 → sonnet 5.5 */
export function shortModel(id: string | null | undefined): string {
  if (!id) return '';
  const m = id.match(/^claude-([a-z]+)-(\d+)-(\d+)/);
  return m ? `${m[1]} ${m[2]}.${m[3]}` : id;
}
