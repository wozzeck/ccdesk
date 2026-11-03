//! Núcleo: snapshot de sesiones, comandos Tauri y bucle de sondeo.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};

use crate::paths;
use crate::pty::Pty;
use crate::registry::{self, RegEntry};
use crate::state::{self, Known, Persist, Settings};
use crate::tmux::{self, Host, Tmux};
use crate::transcript::{self, Cache, Item};

pub struct Core {
    pub tmux: Tmux,
    pub claude: PathBuf,
    pub persist: Mutex<Persist>,
    pub active: Mutex<Option<String>>,
    pub tcache: Mutex<Option<Cache>>,
    pub pty: Mutex<Option<Pty>>,
    pub last_snap: Mutex<String>,
    pub views: Mutex<Vec<SessionView>>,
    /// Transcripts de subagentes de la sesión activa, por tool_use_id de la llamada.
    pub agents: Mutex<HashMap<String, Cache>>,
    /// Dónde vive cada sesión gestionada viva (ventana tmux o sesión antigua).
    pub hosts: Mutex<HashMap<String, Host>>,
    /// Subagentes en marcha por sesión (sid), con la hora del último escaneo.
    pub running_agents: Mutex<HashMap<String, (u64, Vec<AgentRow>)>>,
    /// Carpeta `subagents/` de cada sesión (sid), una vez localizada.
    pub agent_dirs: Mutex<HashMap<String, Option<PathBuf>>>,
    /// Último modo de permisos visto por sesión (el pie no se ve con un diálogo abierto).
    pub modes: Mutex<HashMap<String, String>>,
    /// Arranque de la app (ms): la auto-detención no actúa por inactividad anterior.
    pub started_at: u64,
    /// Opciones reales de Claude Code leídas de sus selectores (caché).
    pub opts: Mutex<OptsCache>,
    /// Desglose de `/usage` cacheado por Claude Code en ~/.claude.json (por fecha de modificación).
    pub usage: Mutex<(Option<SystemTime>, Option<Value>)>,
    /// Índice de búsqueda en las conversaciones.
    pub search: Mutex<crate::search::Index>,
}

#[derive(Default)]
pub struct OptsCache {
    /// Modelos del selector `/model` (válidos para la cuenta) y la versión de Claude Code que los dio.
    pub models: Option<(String, Vec<ModelOpt>)>,
    /// Niveles de `/effort` por modelo.
    pub efforts: HashMap<String, Vec<String>>,
    /// Ciclo de modos (Mayús+Tab) por sesión.
    pub modes: HashMap<String, Vec<String>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelOpt {
    pub n: u32,
    pub label: String,
    pub desc: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AgentRow {
    pub tool_use_id: String,
    pub agent_id: String,
    pub description: String,
    pub agent_type: String,
    pub depth: u64,
}

pub type Shared = Arc<Core>;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub key: String,
    pub sid: String,
    pub cwd: String,
    pub title: String,
    /// idle | busy | waiting | blocked | starting | stopped
    pub status: String,
    pub managed: bool,
    pub external_kind: Option<String>,
    pub pinned: bool,
    pub exempt: bool,
    pub started_at: u64,
    /// En las detenidas, la de su conversación (no la hora en que se detuvieron).
    pub last_activity: u64,
    /// Cuándo se detuvo (sólo detenidas).
    pub stopped_at: Option<u64>,
    pub pid: Option<u32>,
    pub tmux_pane: Option<String>,
    /// Otros procesos con la misma sesión abierta (escriben en el mismo transcript).
    pub dupes: Vec<String>,
    /// Dónde está en tmux (p. ej. `claude:3`), para entrar desde una terminal.
    pub location: Option<String>,
    /// To-dos apuntados para esta sesión.
    pub notes: usize,
    /// Subagentes trabajando ahora mismo (para el árbol del panel izquierdo).
    pub agents: Vec<AgentRow>,
    /// Claude espera, pero algo suyo sigue corriendo (bash en segundo plano, Monitor,
    /// subagentes): detenerla lo cortaría.
    pub background: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub sessions: Vec<SessionView>,
    pub active_status: Option<Value>,
    /// Modo de permisos de la sesión abierta, leído del pie de Claude Code.
    pub active_mode: Option<String>,
    /// Sugerencia de Claude Code para el siguiente mensaje de la sesión abierta
    /// (`{key, text}`), leída de su caja de texto en la TUI.
    pub active_suggestion: Option<Value>,
    /// Desglose de `/usage` (cuota semanal por modelo, p. ej. Fable) y cuándo se leyó.
    pub usage: Option<Value>,
    /// Última sesión abierta, para volver a ella al arrancar.
    pub last_active: Option<String>,
    pub last_active_sid: Option<String>,
    pub quota: Option<Value>,
    pub settings: Settings,
    pub recent_cwds: Vec<String>,
    pub home: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptInit {
    key: String,
    sid: String,
    path: String,
    start_index: usize,
    total: usize,
    items: Vec<Item>,
}

/// Registro de acciones (auto-detener, adoptar, detener, reiniciar) en ~/.cache/ccdesk/ccdesk.log.
pub fn log(msg: &str) {
    use std::io::Write;
    eprintln!("ccdesk: {msg}");
    let f = paths::home().join(".cache/ccdesk/ccdesk.log");
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let secs = now_ms() / 1000;
    if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(&f) {
        let _ = writeln!(fh, "{secs} {msg}");
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn ensure_conf() {
    let f = paths::tmux_conf();
    if !f.exists() {
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&f, include_str!("tmux.conf"));
    }
}

fn short(sid: &str) -> String {
    if sid.len() >= 8 {
        sid[..8].to_string()
    } else {
        "nueva sesión".into()
    }
}

/// Clave para una sesión nueva o adoptada: `cc-<8 primeros del sid>` (sin chocar con otra).
fn key_for(p: &Persist, sid: &str) -> String {
    let mut k = format!("{}{}", tmux::PREFIX, short(sid));
    while p.known.get(&k).is_some_and(|x| x.sid != sid) {
        k.push('x');
    }
    k
}

/// Nombre de ventana tmux a partir del título (corto, para la barra de estado).
fn window_name(title: &str) -> String {
    let t: String = title.chars().take(28).collect();
    if t.chars().count() < title.chars().count() {
        format!("{}…", t.trim_end())
    } else {
        t
    }
}

/// `cachedUsageUtilization` de ~/.claude.json: lo que enseña `/usage` (5h, semanal total y
/// semanales por modelo). Se relee sólo si el fichero ha cambiado.
fn usage_breakdown(core: &Core) -> Option<Value> {
    let f = paths::home().join(".claude.json");
    let mtime = std::fs::metadata(&f).and_then(|m| m.modified()).ok();
    let mut cache = core.usage.lock().unwrap();
    if cache.0 == mtime && mtime.is_some() {
        return cache.1.clone();
    }
    let v: Option<Value> = std::fs::read_to_string(&f)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|j| {
            let u = &j["cachedUsageUtilization"];
            let limits = u["utilization"]["limits"].as_array()?;
            let scoped: Vec<Value> = limits
                .iter()
                .filter(|l| l["kind"].as_str() == Some("weekly_scoped"))
                .map(|l| {
                    json!({
                        "label": l["scope"]["model"]["display_name"].as_str()
                            .or_else(|| l["scope"]["surface"].as_str()).unwrap_or("?"),
                        "percent": l["percent"],
                        "resetsAt": l["resets_at"],
                        "severity": l["severity"],
                    })
                })
                .collect();
            let pick = |k: &str| limits.iter().find(|l| l["kind"].as_str() == Some(k)).map(|l| l["percent"].clone());
            Some(json!({
                "fetchedAtMs": u["fetchedAtMs"],
                "session": pick("session"),
                "weekly": pick("weekly_all"),
                "scoped": scoped,
            }))
        });
    *cache = (mtime, v.clone());
    v
}

/// ¿Claude Code enseña un diálogo que espera respuesta antes de registrarse (confiar en la
/// carpeta, servidores MCP del proyecto, aviso de permisos…)? Se reconoce por su pie
/// («Enter to confirm · Esc to cancel», «Press Enter to continue…»), que queda abajo del todo.
/// Sólo vale mientras no hay registro: después, esas palabras pueden salir en la conversación.
fn startup_prompt(screen: &str) -> bool {
    screen.lines().rev().map(str::trim).filter(|l| !l.is_empty()).take(3).any(|l| {
        let l = l.to_lowercase();
        ["enter to confirm", "enter to select", "enter to continue"].iter().any(|f| l.contains(f))
    })
}

/// Modo de permisos según el pie de la TUI (`⏵⏵ auto mode on`, `⏸ plan mode on`…).
fn detect_mode(screen: &str) -> Option<&'static str> {
    for line in screen.lines().rev().filter(|l| !l.trim().is_empty()).take(4) {
        let l = line.to_lowercase();
        let m = if l.contains("bypass permissions on") {
            "bypassPermissions"
        } else if l.contains("accept edits on") {
            "acceptEdits"
        } else if l.contains("plan mode on") {
            "plan"
        } else if l.contains("auto mode on") {
            "auto"
        } else if l.contains("manual mode on") {
            "manual"
        } else if l.contains("don't ask") || l.contains("dontask") {
            "dontAsk"
        } else {
            continue;
        };
        return Some(m);
    }
    None
}

/// Caracteres visibles de una línea capturada con `-e`, con su atributo: (carácter, atenuado,
/// invertido). El invertido es el cursor que dibuja la propia TUI.
fn sgr_chars(line: &str) -> Vec<(char, bool, bool)> {
    let mut out = Vec::new();
    let (mut dim, mut inv) = (false, false);
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push((c, dim, inv));
            continue;
        }
        if it.peek() != Some(&'[') {
            it.next();
            continue;
        }
        it.next();
        let mut params = String::new();
        let mut fin = ' ';
        for d in it.by_ref() {
            if ('@'..='~').contains(&d) {
                fin = d;
                break;
            }
            params.push(d);
        }
        if fin != 'm' {
            continue;
        }
        let ps: Vec<u32> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
        let mut i = 0;
        while i < ps.len() {
            match ps[i] {
                0 => (dim, inv) = (false, false),
                2 => dim = true,
                22 => dim = false,
                7 => inv = true,
                27 => inv = false,
                38 | 48 | 58 => i += if ps.get(i + 1) == Some(&5) { 2 } else { 4 },
                _ => {}
            }
            i += 1;
        }
    }
    out
}

/// Sugerencia que Claude Code pinta atenuada en su caja de texto vacía al acabar un turno (se
/// acepta con Tab). La caja es la última línea `❯ ` entre dos reglas `────`; sólo cuenta si todo
/// su texto va atenuado (lo escrito por el usuario no lo va). Los ejemplos `Try "…"` de una
/// sesión nueva también van atenuados, pero no son sugerencias.
fn detect_suggestion(screen: &str) -> Option<String> {
    let lines: Vec<Vec<(char, bool, bool)>> = screen.lines().map(sgr_chars).collect();
    // La regla de arriba puede llevar el nombre de la sesión al final (`──── todo ─`).
    let is_rule = |l: &[(char, bool, bool)]| {
        let mut t = l.iter().map(|x| x.0).skip_while(|c| c.is_whitespace());
        t.by_ref().take(10).filter(|&c| c == '─').count() == 10
    };
    let i = (1..lines.len()).rev().find(|&i| {
        let l = &lines[i];
        let start = l.iter().position(|x| !x.0.is_whitespace());
        start.is_some_and(|s| l[s].0 == '❯' && l.get(s + 1).is_some_and(|x| x.0 == '\u{a0}' || x.0 == ' '))
            && is_rule(&lines[i - 1])
    })?;
    let mut parts = Vec::new();
    for (k, l) in lines[i..].iter().enumerate() {
        if k > 0 && is_rule(l) {
            let text = parts.join(" ");
            let quoted = text.starts_with("Try \"") && text.ends_with('"');
            return (!text.is_empty() && !quoted).then_some(text);
        }
        let body = if k == 0 {
            let s = l.iter().position(|x| x.0 == '❯').unwrap_or(0);
            &l[s + 2..]
        } else {
            &l[..]
        };
        let ink: Vec<_> = body.iter().filter(|x| !x.0.is_whitespace()).collect();
        if ink.iter().any(|x| !x.1 && !x.2) || (k == 0 && !ink.iter().any(|x| x.1)) {
            return None; // texto escrito por el usuario (o caja vacía)
        }
        let s: String = body.iter().map(|x| x.0).collect::<String>().replace('\u{a0}', " ");
        if !s.trim().is_empty() {
            parts.push(s.trim().to_string());
        }
    }
    None
}

fn read_mode(core: &Core, key: &str, plain: Option<&str>) -> Option<String> {
    let seen = plain.and_then(detect_mode).map(String::from);
    let mut modes = core.modes.lock().unwrap();
    match seen {
        Some(m) => {
            modes.insert(key.to_string(), m.clone());
            Some(m)
        }
        None => modes.get(key).cloned(),
    }
}

/// Flags de arranque comunes a sesiones nuevas y reanudadas.
fn launch_flags(core: &Core) -> Vec<String> {
    let yolo = core.persist.lock().unwrap().settings.allow_yolo;
    if yolo {
        vec!["--allow-dangerously-skip-permissions".into()]
    } else {
        vec![]
    }
}

/// Transcript de una sesión: la ruta que dejó el statusline o, si no, la búsqueda por carpeta.
pub(crate) fn transcript_of(sid: &str, cwd: &str) -> Option<PathBuf> {
    let hint = read_status(sid).and_then(|s| s["transcript_path"].as_str().map(String::from));
    transcript::find(sid, cwd, hint.as_deref())
}

/// Última actividad de una sesión detenida: la de su conversación (`transcript::last_activity`).
/// La del registro no sirve: al detenerla, Claude Code la pone al día y quedaría la hora de la
/// parada. Se pide en cada vuelta del sondeo, así que la ruta y el resultado se guardan mientras el
/// fichero no cambie (tamaño y fecha); si aún no tiene transcript, se vuelve a mirar cada 30 s.
fn stopped_activity(sid: &str, cwd: &str) -> Option<u64> {
    enum Seen {
        File { path: PathBuf, len: u64, mtime: Option<SystemTime>, at: Option<u64> },
        Missing(Instant),
    }
    static SEEN: Mutex<Option<HashMap<String, Seen>>> = Mutex::new(None);
    if sid.is_empty() {
        return None;
    }
    let known = {
        let g = SEEN.lock().unwrap();
        match g.as_ref().and_then(|m| m.get(sid)) {
            Some(Seen::File { path, len, mtime, at }) => {
                let meta = std::fs::metadata(path).ok();
                if meta.as_ref().is_some_and(|m| m.len() == *len && m.modified().ok() == *mtime) {
                    return *at;
                }
                meta.map(|_| path.clone())
            }
            Some(Seen::Missing(t)) if t.elapsed() < Duration::from_secs(30) => return None,
            _ => None,
        }
    };
    // Sin el bloqueo mientras se lee el fichero.
    let (seen, at) = match known.or_else(|| transcript_of(sid, cwd)) {
        Some(path) => {
            let meta = std::fs::metadata(&path).ok();
            let at = transcript::last_activity(&path);
            let (len, mtime) = (meta.as_ref().map_or(0, |m| m.len()), meta.and_then(|m| m.modified().ok()));
            (Seen::File { path, len, mtime, at }, at)
        }
        None => (Seen::Missing(Instant::now()), None),
    };
    SEEN.lock().unwrap().get_or_insert_with(HashMap::new).insert(sid.to_string(), seen);
    at
}

pub(crate) fn read_status(sid: &str) -> Option<Value> {
    if sid.is_empty() {
        return None;
    }
    let s = std::fs::read_to_string(paths::status_dir().join(format!("{sid}.json"))).ok()?;
    serde_json::from_str(&s).ok()
}

/// Cuotas de la cuenta. Cada sesión guarda el porcentaje de su última llamada a la API,
/// así que las inactivas tienen datos viejos: por ventana se toma el reset más reciente
/// y, dentro de él, el mayor porcentaje (el uso sólo crece dentro de una ventana).
/// Cuotas de la cuenta, recalculadas como mucho cada 5 s: leer y parsear todos los ficheros de
/// estado en cada vuelta del sondeo (1,2 s) era trabajo de más.
fn account_quota() -> Option<Value> {
    static CACHE: Mutex<Option<(Instant, Option<Value>)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    if let Some((t, v)) = c.as_ref() {
        if t.elapsed() < Duration::from_secs(5) {
            return v.clone();
        }
    }
    let v = account_quota_now();
    *c = Some((Instant::now(), v.clone()));
    v
}

fn account_quota_now() -> Option<Value> {
    let cutoff = SystemTime::now() - Duration::from_secs(8 * 24 * 3600);
    let mut best: HashMap<&'static str, (u64, f64)> = HashMap::new();
    for e in std::fs::read_dir(paths::status_dir()).ok()?.flatten() {
        let p = e.path();
        if !p.extension().is_some_and(|x| x == "json") {
            continue;
        }
        if e.metadata().and_then(|m| m.modified()).map_or(true, |t| t < cutoff) {
            continue;
        }
        let Some(v) = std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        else {
            continue;
        };
        for w in ["five_hour", "seven_day"] {
            let r = &v["rate_limits"][w];
            let (Some(reset), Some(pct)) = (r["resets_at"].as_u64(), r["used_percentage"].as_f64())
            else {
                continue;
            };
            let cur = best.entry(w).or_insert((0, 0.0));
            if reset > cur.0 || (reset == cur.0 && pct > cur.1) {
                *cur = (reset, pct);
            }
        }
    }
    if best.is_empty() {
        return None;
    }
    let mut out = serde_json::Map::new();
    for (w, (reset, pct)) in best {
        out.insert(w.into(), json!({ "resets_at": reset, "used_percentage": pct }));
    }
    Some(Value::Object(out))
}

fn pick_title(
    custom: Option<&String>,
    st: Option<&Value>,
    reg: Option<&RegEntry>,
    known_title: Option<&String>,
    sid: &str,
) -> String {
    if let Some(c) = custom {
        return c.clone();
    }
    if let Some(n) = st
        .and_then(|v| v["session_name"].as_str())
        .filter(|s| !s.trim().is_empty())
    {
        return n.to_string();
    }
    if let Some(r) = reg {
        if r.name_source.as_deref() != Some("derived") {
            if let Some(n) = r.name.as_ref().filter(|s| !s.is_empty()) {
                return n.clone();
            }
        }
    }
    if let Some(t) = known_title {
        return t.clone();
    }
    if let Some(n) = reg.and_then(|r| r.name.clone()) {
        return n;
    }
    short(sid)
}

/// Estado del registro de Claude Code. «shell» es su «idle con un bash/Monitor en segundo plano
/// vivo»: para todo lo demás cuenta como inactiva (se puede escribir, cambiar de modelo…); lo
/// del segundo plano va aparte, en `background`.
fn reg_status(r: &RegEntry) -> String {
    match r.status.as_deref() {
        None | Some("shell") => "idle".into(),
        Some(s) => s.to_string(),
    }
}

pub fn build(core: &Core) -> Snapshot {
    let wins = core.tmux.list();
    let tsession = core.tmux.session();
    let regs = registry::read_all();
    let mut used = vec![false; regs.len()];
    let active = core.active.lock().unwrap().clone();
    let is_active = |k: &str| active.as_deref() == Some(k);
    let now = now_ms();
    let mut active_status = None;
    let mut views: HashMap<String, SessionView> = HashMap::new();
    let mut hosts: HashMap<String, Host> = HashMap::new();
    let mut renames: Vec<(Host, String)> = Vec::new();

    let mut p = core.persist.lock().unwrap();
    let mut dirty = false;

    // 1) Sesiones gestionadas vivas: ventanas de la sesión tmux de ccdesk (y antiguas -L cc).
    // Cadena de padres de cada proceso claude, una vez por vuelta (se compara con cada ventana).
    let chains: Vec<Vec<u32>> = regs.iter().map(|r| registry::ancestors(r.pid)).collect();
    for w in &wins {
        // Su proceso claude es el del pane o desciende de él (ventanas adoptadas: shell → claude).
        let ri = regs.iter().enumerate().position(|(i, r)| {
            chains[i].contains(&w.pane_pid)
                || matches!(&w.host, Host::Legacy(n)
                    if r.tmux.as_deref().is_some_and(|t| t.starts_with(&format!("{n}:"))))
        });
        let reg = ri.map(|i| &regs[i]);
        let key = match (&w.key, reg) {
            (Some(k), _) => k.clone(),
            // Ventana con claude abierta a mano en la sesión (p. ej. por SSH): se incorpora.
            (None, Some(r)) => {
                let k = key_for(&p, &r.session_id);
                core.tmux.set_key(&w.host, &k);
                k
            }
            // Una ventana normal (una shell): no es de ccdesk.
            (None, None) => continue,
        };
        if let Some(i) = ri {
            used[i] = true;
        }
        hosts.insert(key.clone(), w.host.clone());
        let custom = p.names.get(&key).cloned();
        let pinned = p.pinned.contains(&key);
        let exempt = p.exempt.contains(&key);
        if !p.known.contains_key(&key) {
            dirty = true;
        }
        let k = p.known.entry(key.clone()).or_insert_with(|| Known {
            cwd: w.path.clone(),
            started_at: w.created * 1000,
            ..Default::default()
        });
        if let Some(r) = reg {
            if k.sid != r.session_id {
                k.sid = r.session_id.clone();
                dirty = true;
            }
            if k.cwd.is_empty() && !r.cwd.is_empty() {
                k.cwd = r.cwd.clone();
                dirty = true;
            }
        }
        // Sin claude y con una shell en el pane: se salió de claude a mano (ventana adoptada).
        // Sin registrar todavía: arranca, o espera en un diálogo previo (p. ej. confiar en una
        // carpeta nueva), que se contesta en la terminal como cualquier otra pregunta.
        let status = match reg {
            Some(r) => reg_status(r),
            None if tmux::is_shell(&w.command) => "stopped".into(),
            None if core.tmux.capture(&w.host).as_deref().is_some_and(startup_prompt) => "waiting".into(),
            None => "starting".into(),
        };
        if status == "stopped" {
            if k.stopped_at.is_none() {
                k.stopped_at = Some(now);
                dirty = true;
            }
        } else if k.stopped_at.is_some() {
            k.stopped_at = None;
            dirty = true;
        }
        let st = read_status(&k.sid);
        let title = pick_title(custom.as_ref(), st.as_ref(), reg, k.title.as_ref(), &k.sid);
        if custom.is_none()
            && (st.is_some() || reg.is_some())
            && k.title.as_deref() != Some(title.as_str())
        {
            k.title = Some(title.clone());
            dirty = true;
        }
        // Detenida (queda la shell en la ventana): la última actividad es la de su conversación.
        let last = if status == "stopped" {
            stopped_activity(&k.sid, &k.cwd).or(k.stopped_at)
        } else {
            reg.and_then(|r| r.status_updated_at.or(r.updated_at))
        }
        .unwrap_or(w.created * 1000);
        k.last_activity = last;
        if is_active(&key) {
            active_status = st;
        }
        let location = match &w.host {
            Host::Window(_) => {
                // Nombre de ventana al día con el título, para quien entra por terminal.
                let want = window_name(&title);
                if w.name != want {
                    renames.push((w.host.clone(), want));
                }
                Some(format!("{tsession}:{}", w.index.unwrap_or(0)))
            }
            Host::Legacy(n) => Some(format!("tmux -L cc · {n}")),
        };
        views.insert(
            key.clone(),
            SessionView {
                key: key.clone(),
                sid: k.sid.clone(),
                cwd: k.cwd.clone(),
                title,
                status,
                managed: true,
                external_kind: None,
                pinned,
                exempt,
                started_at: k.started_at,
                last_activity: last,
                stopped_at: k.stopped_at,
                pid: reg.map(|r| r.pid).or(Some(w.pane_pid)),
                tmux_pane: None,
                dupes: vec![],
                location,
                notes: 0,
                agents: vec![],
                background: false,
            },
        );
    }

    // 2) Gestionadas detenidas (conocidas pero sin sesión tmux).
    let live: HashSet<String> = views.keys().cloned().collect();
    let names = p.names.clone();
    let pinned_l = p.pinned.clone();
    let exempt_l = p.exempt.clone();
    for (key, k) in p.known.iter_mut() {
        if live.contains(key) {
            continue;
        }
        if k.stopped_at.is_none() {
            k.stopped_at = Some(now);
            dirty = true;
        }
        if is_active(key) {
            active_status = read_status(&k.sid);
        }
        views.insert(
            key.clone(),
            SessionView {
                key: key.clone(),
                sid: k.sid.clone(),
                cwd: k.cwd.clone(),
                title: names
                    .get(key)
                    .cloned()
                    .or_else(|| k.title.clone())
                    .unwrap_or_else(|| short(&k.sid)),
                status: "stopped".into(),
                managed: true,
                external_kind: None,
                pinned: pinned_l.contains(key),
                exempt: exempt_l.contains(key),
                started_at: k.started_at,
                last_activity: stopped_activity(&k.sid, &k.cwd).or(k.stopped_at).unwrap_or(k.last_activity),
                stopped_at: k.stopped_at,
                pid: None,
                tmux_pane: None,
                dupes: vec![],
                location: None,
                notes: 0,
                agents: vec![],
                background: false,
            },
        );
    }

    // 3) Externas: procesos claude vivos fuera de nuestro servidor tmux.
    let mut externals = Vec::new();
    for (i, r) in regs.iter().enumerate() {
        if used[i] {
            continue;
        }
        let key = format!("ext-{}", r.session_id);
        let st = read_status(&r.session_id);
        let title = pick_title(None, st.as_ref(), Some(r), None, &r.session_id);
        if is_active(&key) {
            active_status = st;
        }
        externals.push(SessionView {
            key,
            sid: r.session_id.clone(),
            cwd: r.cwd.clone(),
            title,
            status: reg_status(r),
            managed: false,
            external_kind: r.kind.clone(),
            pinned: false,
            exempt: false,
            started_at: r.started_at,
            last_activity: r.status_updated_at.or(r.updated_at).unwrap_or(r.started_at),
            stopped_at: None,
            pid: Some(r.pid),
            tmux_pane: r.tmux.clone(),
            dupes: vec![],
            location: r.tmux.clone(),
            notes: 0,
            agents: vec![],
            background: false,
        });
    }

    // Un proceso externo sobre una conversación gestionada no sale aparte: es la misma sesión.
    // Se ve como aviso (⚠) en la gestionada y, si ésta está detenida, ccdesk recoge su ventana.
    externals.retain(|e| !views.values().any(|v| !v.sid.is_empty() && v.sid == e.sid));

    // 4) Orden: fijadas, luego orden manual; las nuevas entran arriba de las no fijadas.
    let mut fresh: Vec<(u64, String)> = views
        .values()
        .filter(|v| !p.pinned.contains(&v.key) && !p.order.contains(&v.key))
        .map(|v| (v.started_at, v.key.clone()))
        .collect();
    if !fresh.is_empty() {
        fresh.sort_by_key(|x| std::cmp::Reverse(x.0));
        let mut order: Vec<String> = fresh.into_iter().map(|(_, k)| k).collect();
        order.extend(p.order.iter().cloned());
        p.order = order;
        dirty = true;
    }
    let before = (p.order.len(), p.pinned.len());
    p.order.retain(|k| views.contains_key(k));
    p.pinned.retain(|k| views.contains_key(k));
    if before != (p.order.len(), p.pinned.len()) {
        dirty = true;
    }

    // Procesos por sesión: más de uno = el mismo transcript escrito por varios a la vez.
    let describe = |r: &RegEntry| match r.tmux.as_deref() {
        Some(t) => format!("pid {} · tmux {}", r.pid, t),
        None => format!("pid {} · {}", r.pid, r.kind.as_deref().unwrap_or("sin tmux")),
    };
    let mark = |v: &mut SessionView| {
        v.dupes = regs
            .iter()
            .filter(|r| r.session_id == v.sid && Some(r.pid) != v.pid)
            .map(describe)
            .collect();
    };
    views.values_mut().for_each(mark);
    externals.iter_mut().for_each(mark);
    for v in views.values_mut().chain(externals.iter_mut()) {
        v.notes = p.notes.get(&v.key).map_or(0, |n| n.len());
        if v.status != "stopped" && !v.sid.is_empty() {
            v.agents = running_agents(core, &v.sid, &v.cwd);
        }
        if v.status == "idle" {
            let reg = v.pid.and_then(|pid| regs.iter().find(|r| r.pid == pid));
            v.background = reg.is_some_and(|r| r.status.as_deref() == Some("shell")) || !v.agents.is_empty();
        }
    }

    let mut sessions = arrange(&mut views, &p.pinned, &p.order, p.settings.session_order == "recent");
    sessions.extend(externals);

    let (last_active, last_active_sid, settings, recent_cwds) =
        (p.last_active.clone(), p.last_active_sid.clone(), p.settings.clone(), p.recent_cwds.clone());
    if dirty {
        state::save(&p);
    }
    // Lo que sigue (captura de tmux, cuotas) no necesita la persistencia bloqueada.
    drop(p);

    // Una sola captura de la sesión abierta para el modo (pie) y la sugerencia (caja de texto).
    let (active_mode, active_suggestion) = match active.as_deref().and_then(|k| Some((k, hosts.get(k)?))) {
        Some((k, h)) => {
            let screen = core.tmux.capture_ansi(h);
            let plain = screen.as_deref().map(transcript::strip_ansi);
            let mode = read_mode(core, k, plain.as_deref());
            let idle = sessions.iter().any(|s| s.key == k && s.managed && s.status == "idle");
            let sug = screen.as_deref().filter(|_| idle).and_then(detect_suggestion);
            (mode, sug.map(|t| json!({ "key": k, "text": t })))
        }
        None => (None, None),
    };
    let snap = Snapshot {
        sessions,
        active_status,
        active_mode,
        active_suggestion,
        usage: usage_breakdown(core),
        last_active,
        last_active_sid,
        quota: account_quota(),
        settings,
        recent_cwds,
        home: paths::home().to_string_lossy().into_owned(),
    };
    *core.hosts.lock().unwrap() = hosts;
    for (h, name) in renames {
        core.tmux.rename(&h, &name);
    }
    *core.views.lock().unwrap() = snap.sessions.clone();
    snap
}

/// Fijadas en su orden; el resto, en el manual o por actividad reciente (Preferencias). Con empate
/// se queda el manual (sort_by es estable).
fn arrange(views: &mut HashMap<String, SessionView>, pinned: &[String], order: &[String], recent: bool) -> Vec<SessionView> {
    let mut out: Vec<SessionView> = pinned.iter().filter_map(|k| views.remove(k)).collect();
    let mut rest: Vec<SessionView> = order.iter().filter_map(|k| views.remove(k)).collect();
    if recent {
        rest.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    }
    out.extend(rest);
    out
}

/// Subagentes en marcha de una sesión: `.meta.json` sin `finished` y transcript escrito en los
/// últimos 30 min (si un subagente muere sin marcar `finished`, deja de salir con el tiempo).
fn running_agents(core: &Core, sid: &str, cwd: &str) -> Vec<AgentRow> {
    let now = now_ms();
    if let Some((t, rows)) = core.running_agents.lock().unwrap().get(sid) {
        if now.saturating_sub(*t) < 3000 {
            return rows.clone();
        }
    }
    let dir = {
        let mut dirs = core.agent_dirs.lock().unwrap();
        dirs.entry(sid.to_string())
            .or_insert_with(|| {
                transcript_of(sid, cwd).map(|p| p.with_extension("").join("subagents"))
            })
            .clone()
    };
    let rows = dir.as_deref().map(scan_agents).unwrap_or_default();
    core.running_agents.lock().unwrap().insert(sid.to_string(), (now, rows.clone()));
    rows
}

/// Subagentes en marcha en una carpeta `subagents/`.
fn scan_agents(dir: &std::path::Path) -> Vec<AgentRow> {
    let mut rows = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        let fresh = std::time::SystemTime::now() - Duration::from_secs(30 * 60);
        for e in rd.flatten() {
            let p = e.path();
            let Some(name) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let Some(stem) = name.strip_suffix(".meta.json") else { continue };
            let jsonl = p.with_file_name(format!("{stem}.jsonl"));
            let recent = std::fs::metadata(&jsonl)
                .and_then(|m| m.modified())
                .is_ok_and(|t| t > fresh);
            if !recent {
                continue;
            }
            let Some(meta) = std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
                continue;
            };
            if meta["finished"].as_bool() == Some(true) {
                continue;
            }
            rows.push(AgentRow {
                tool_use_id: meta["toolUseId"].as_str().unwrap_or("").to_string(),
                agent_id: stem.trim_start_matches("agent-").to_string(),
                description: meta["description"].as_str().unwrap_or("subagente").to_string(),
                agent_type: meta["agentType"].as_str().unwrap_or("").to_string(),
                depth: meta["spawnDepth"].as_u64().unwrap_or(1).max(1),
            });
        }
    }
    rows.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.description.cmp(&b.description)));
    rows
}

fn host_of(core: &Core, key: &str) -> Option<Host> {
    let h = core.hosts.lock().unwrap().get(key).cloned();
    h.or_else(|| {
        build(core);
        core.hosts.lock().unwrap().get(key).cloned()
    })
}

fn find_view(core: &Core, key: &str) -> Option<SessionView> {
    let cached = core.views.lock().unwrap().iter().find(|v| v.key == key).cloned();
    cached.or_else(|| build(core).sessions.into_iter().find(|v| v.key == key))
}

/// `/exit` con Ctrl+C antes (limpia la línea de entrada). Si en 10 s no ha terminado, se
/// cierra la ventana. En ventanas adoptadas queda la shell de la que colgaba claude: se cierra.
fn graceful_stop(core: &Core, key: &str) {
    let Some(h) = host_of(core, key) else { return };
    log(&format!("detener {key} ({h:?})"));
    let pid = core.views.lock().unwrap().iter().find(|v| v.key == key).and_then(|v| v.pid);
    let t = core.tmux.clone();
    let _ = t.send_keys(&h, &["C-c"]);
    std::thread::sleep(Duration::from_millis(200));
    let _ = t.send_literal(&h, "/exit");
    std::thread::sleep(Duration::from_millis(150));
    let _ = t.send_keys(&h, &["Enter"]);
    std::thread::spawn(move || {
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(250));
            if !t.exists(&h) {
                return;
            }
            if pid.is_some_and(|p| !registry::alive(p, None)) {
                if t.pane_command(&h).is_some_and(|c| tmux::is_shell(&c)) {
                    let _ = t.kill(&h);
                }
                return;
            }
        }
        let _ = t.kill(&h);
    });
}

fn resume_inner(core: &Core, key: &str) -> Result<(), String> {
    resume_with(core, key, &[])
}

/// Reanuda con flags extra (p. ej. `--permission-mode bypassPermissions`).
fn resume_with(core: &Core, key: &str, extra: &[String]) -> Result<(), String> {
    // Ventana que sigue abierta: si claude sigue vivo no hay nada que hacer; si sólo queda la
    // shell (se salió a mano), se desvincula y la sesión se reanuda en una ventana nueva.
    let lingering = core.hosts.lock().unwrap().get(key).cloned();
    if let Some(h) = &lingering {
        let shell = core.tmux.pane_command(h).is_some_and(|c| tmux::is_shell(&c));
        if core.tmux.exists(h) && !shell {
            return Ok(());
        }
        core.tmux.unset_key(h);
    }
    let (sid, cwd, title) = {
        let p = core.persist.lock().unwrap();
        let k = p.known.get(key).ok_or("Sesión desconocida")?;
        let title = p.names.get(key).cloned().or_else(|| k.title.clone()).unwrap_or_else(|| short(&k.sid));
        (k.sid.clone(), k.cwd.clone(), title)
    };
    if sid.is_empty() {
        return Err("La sesión no tiene id; no se puede reanudar".into());
    }
    if reattach(core, key, &sid, &title)? {
        return Ok(());
    }
    let dir = paths::resolve_dir(&cwd).ok_or_else(|| format!("El directorio ya no existe: {cwd}"))?;
    let dir_s = dir.to_string_lossy().into_owned();
    let has_transcript = transcript::find(&sid, &cwd, None).is_some();
    let flag = if has_transcript { "--resume" } else { "--session-id" };
    let mut argv = vec![core.claude.to_string_lossy().into_owned(), flag.into(), sid.clone()];
    argv.extend(launch_flags(core));
    argv.extend(extra.iter().cloned());
    // Recuperar el modelo y el effort con los que estaba (último volcado del statusline).
    if let Some(st) = read_status(&sid) {
        if let Some(m) = st["model"]["id"].as_str().filter(|s| !s.is_empty()) {
            argv.extend(["--model".into(), m.to_string()]);
        }
        if let Some(e) = st["effort"]["level"].as_str().filter(|s| !s.is_empty()) {
            argv.extend(["--effort".into(), e.to_string()]);
        }
    }
    core.opts.lock().unwrap().modes.remove(key);
    let h = core.tmux.new_window(key, &window_name(&title), &dir_s, &argv)?;
    log(&format!("reanudar {key} ({h:?}) {}", argv[1..].join(" ")));
    core.hosts.lock().unwrap().insert(key.to_string(), h);
    if let Some(k) = core.persist.lock().unwrap().known.get_mut(key) {
        k.managed_since = now_ms();
    }
    Ok(())
}

/// Por qué no se puede recoger la ventana de tmux de un proceso claude.
enum PaneErr {
    /// No corre en tmux (o el registro no dice dónde).
    NoTmux,
    /// Su pane no está en el servidor tmux por defecto.
    NotInServer,
    /// El proceso ya no cuelga de ese pane.
    Mismatch,
}

/// Pane (`%N`) de un proceso claude según el registro, comprobando que el proceso cuelga de él.
fn checked_pane(tmux_loc: Option<&str>, pid: u32) -> Result<String, PaneErr> {
    let pane = tmux_loc
        .and_then(|t| t.rsplit_once('.').map(|(_, p)| p.to_string()))
        .filter(|p| p.starts_with('%'))
        .ok_or(PaneErr::NoTmux)?;
    let (pane_pid, _) = tmux::pane_info(&pane).ok_or(PaneErr::NotInServer)?;
    if !registry::descends_from(pid, pane_pid) {
        return Err(PaneErr::Mismatch);
    }
    Ok(pane)
}

/// Mueve la ventana de ese pane a la sesión de ccdesk como `key` (el proceso sigue igual).
fn adopt_host(core: &Core, key: &str, pane: &str, title: &str) -> Result<Host, String> {
    let h = core.tmux.adopt_pane(pane, key, &window_name(title))?;
    core.hosts.lock().unwrap().insert(key.to_string(), h.clone());
    Ok(h)
}

/// Una sesión gestionada y detenida que ya está abierta en otro proceso (p. ej. la pasada diaria
/// de /todo-list la abre en una pestaña del tmux normal): se recoge su ventana, como al adoptar,
/// en vez de abrir un segundo proceso sobre el mismo transcript. `Ok(false)`: no está abierta.
fn reattach(core: &Core, key: &str, sid: &str, title: &str) -> Result<bool, String> {
    let regs = registry::read_all();
    let Some(r) = regs.iter().find(|r| r.session_id == sid) else { return Ok(false) };
    let pane = checked_pane(r.tmux.as_deref(), r.pid).map_err(|e| {
        let pid = r.pid;
        match e {
            PaneErr::NoTmux => format!("Ya está abierta en otro proceso (pid {pid}) fuera de tmux: ciérrala allí antes de reanudarla aquí"),
            PaneErr::NotInServer => format!("Ya está abierta en otro proceso (pid {pid}) que no está en el tmux por defecto"),
            PaneErr::Mismatch => format!("Ya está abierta en otro proceso (pid {pid}), pero su pane de tmux no cuadra"),
        }
    })?;
    let h = adopt_host(core, key, &pane, title)?;
    log(&format!("recoger {key} ({h:?}): ya abierta en pid {} ({})", r.pid, r.tmux.as_deref().unwrap_or("")));
    let mut p = core.persist.lock().unwrap();
    if let Some(k) = p.known.get_mut(key) {
        k.managed_since = now_ms();
        k.stopped_at = None;
    }
    state::save(&p);
    Ok(true)
}

/// Gestionadas detenidas que alguien ha abierto fuera (en tmux): se recogen solas.
fn reattach_opened(core: &Core, snap: &Snapshot) {
    // Sólo si el duplicado corre en tmux (el texto de `describe` lo dice): si no, no hay nada que
    // recoger y no vale la pena releer el registro en cada vuelta.
    let in_tmux = |v: &&SessionView| v.dupes.iter().any(|d| d.contains("· tmux "));
    for v in snap.sessions.iter().filter(|v| v.managed && v.status == "stopped" && in_tmux(v)) {
        let _ = reattach(core, &v.key, &v.sid, &v.title);
    }
}

// ───────────────────────────── comandos ─────────────────────────────

#[tauri::command]
pub async fn snapshot_now(core: State<'_, Shared>) -> Result<Snapshot, String> {
    Ok(build(&core))
}

#[tauri::command]
pub async fn transcript_open(
    core: State<'_, Shared>,
    key: Option<String>,
) -> Result<Option<TranscriptInit>, String> {
    *core.active.lock().unwrap() = key.clone();
    let Some(key) = key else {
        *core.tcache.lock().unwrap() = None;
        return Ok(None);
    };
    let Some(v) = find_view(&core, &key) else {
        return Err("Sesión desconocida".into());
    };
    {
        let mut p = core.persist.lock().unwrap();
        if p.last_active.as_deref() != Some(key.as_str()) || p.last_active_sid.as_deref() != Some(v.sid.as_str()) {
            p.last_active = Some(key.clone());
            p.last_active_sid = Some(v.sid.clone()).filter(|s| !s.is_empty());
            state::save(&p);
        }
    }
    let Some(path) = transcript_of(&v.sid, &v.cwd) else {
        *core.tcache.lock().unwrap() = None;
        return Ok(None);
    };
    let cache = Cache::open(&key, &v.sid, &path).map_err(|e| e.to_string())?;
    let total = cache.items.len();
    let start = total.saturating_sub(400);
    let init = TranscriptInit {
        key,
        sid: v.sid,
        path: path.to_string_lossy().into_owned(),
        start_index: start,
        total,
        items: cache.items[start..].to_vec(),
    };
    *core.tcache.lock().unwrap() = Some(cache);
    core.agents.lock().unwrap().clear();
    Ok(Some(init))
}

#[tauri::command]
pub fn transcript_before(core: State<'_, Shared>, key: String, before: usize, count: usize) -> Vec<Item> {
    let tc = core.tcache.lock().unwrap();
    match tc.as_ref() {
        Some(c) if c.key == key => {
            let end = before.min(c.items.len());
            c.items[end.saturating_sub(count)..end].to_vec()
        }
        _ => vec![],
    }
}

#[tauri::command]
pub async fn session_new(
    core: State<'_, Shared>,
    cwd: String,
    name: Option<String>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<String, String> {
    let dir = paths::resolve_dir(&cwd).ok_or_else(|| format!("No existe el directorio: {cwd}"))?;
    let dir_s = dir.to_string_lossy().into_owned();
    let sid = uuid::Uuid::new_v4().to_string();
    let key = format!("{}{}", tmux::PREFIX, &sid[..8]);
    let mut argv = vec![
        core.claude.to_string_lossy().into_owned(),
        "--session-id".into(),
        sid.clone(),
    ];
    argv.extend(launch_flags(&core));
    if let Some(m) = model.filter(|s| !s.is_empty()) {
        argv.push("--model".into());
        argv.push(m);
    }
    if let Some(e) = effort.filter(|s| !s.is_empty()) {
        argv.push("--effort".into());
        argv.push(e);
    }
    let name = name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(n) = &name {
        argv.push("-n".into());
        argv.push(n.clone());
    }
    let wname = window_name(name.as_deref().unwrap_or("claude"));
    let h = core.tmux.new_window(&key, &wname, &dir_s, &argv)?;
    core.hosts.lock().unwrap().insert(key.clone(), h);
    let mut p = core.persist.lock().unwrap();
    p.known.insert(
        key.clone(),
        Known {
            sid,
            cwd: dir_s.clone(),
            title: name.clone(),
            started_at: now_ms(),
            managed_since: now_ms(),
            ..Default::default()
        },
    );
    if let Some(n) = name {
        p.names.insert(key.clone(), n);
    }
    p.order.retain(|k| k != &key);
    p.order.insert(0, key.clone());
    p.touch_cwd(&dir_s);
    state::save(&p);
    Ok(key)
}

#[tauri::command]
pub async fn session_stop(
    core: State<'_, Shared>,
    key: String,
    force: bool,
    background_ok: Option<bool>,
) -> Result<(), String> {
    let v = find_view(&core, &key).ok_or("Sesión desconocida")?;
    if !v.managed {
        return Err("Sólo se pueden detener sesiones gestionadas por ccdesk".into());
    }
    let kill = || match host_of(&core, &key) {
        Some(h) => core.tmux.kill(&h),
        None => Ok(()),
    };
    match v.status.as_str() {
        "stopped" => Ok(()),
        "starting" => kill(),
        _ if force => kill(),
        "idle" if v.background && background_ok != Some(true) => Err("background".into()),
        "idle" => {
            graceful_stop(&core, &key);
            Ok(())
        }
        _ => Err("busy".into()),
    }
}

#[tauri::command]
pub async fn session_resume(core: State<'_, Shared>, key: String) -> Result<(), String> {
    resume_inner(&core, &key)
}

/// Adopta una sesión externa que corre en el tmux por defecto moviendo su ventana a la sesión
/// de ccdesk: el proceso sigue igual (no se reinicia ni pierde lo que tenga en segundo plano).
fn adopt_view(core: &Core, v: &SessionView) -> Result<String, String> {
    if v.managed {
        return Err("ya está gestionada por ccdesk".into());
    }
    let pid = v.pid.ok_or("sin pid")?;
    let pane = checked_pane(v.tmux_pane.as_deref(), pid).map_err(|e| match e {
        PaneErr::NoTmux => "no corre en tmux: ciérrala y reanúdala desde ccdesk",
        PaneErr::NotInServer => "su pane no está en el servidor tmux por defecto",
        PaneErr::Mismatch => "su pane de tmux ya no es el de este proceso",
    })?;
    let key = {
        let p = core.persist.lock().unwrap();
        key_for(&p, &v.sid)
    };
    let h = adopt_host(core, &key, &pane, &v.title)?;
    log(&format!("adoptar {} → {key} ({h:?}) «{}»", v.key, v.title));
    let mut p = core.persist.lock().unwrap();
    p.known.insert(
        key.clone(),
        Known {
            sid: v.sid.clone(),
            cwd: v.cwd.clone(),
            title: Some(v.title.clone()),
            started_at: v.started_at,
            managed_since: now_ms(),
            ..Default::default()
        },
    );
    p.order.retain(|k| k != &key);
    p.order.insert(0, key.clone());
    p.touch_cwd(&v.cwd);
    // Los to-dos apuntados a la externa pasan a la gestionada.
    if let Some(n) = p.notes.remove(&v.key) {
        p.notes.entry(key.clone()).or_default().extend(n);
    }
    state::save(&p);
    Ok(key)
}

#[tauri::command]
pub async fn session_adopt(core: State<'_, Shared>, key: String) -> Result<String, String> {
    let v = find_view(&core, &key).ok_or("Sesión desconocida")?;
    adopt_view(&core, &v).map_err(|e| {
        let mut c = e.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or(e)
    })
}

#[derive(Serialize)]
pub struct AdoptReport {
    adopted: Vec<String>,
    skipped: Vec<String>,
}

/// Adopta todas las externas que se puedan (inactivas, en tmux, sin duplicados).
#[tauri::command]
pub async fn session_adopt_all(core: State<'_, Shared>) -> Result<AdoptReport, String> {
    let externals: Vec<SessionView> = build(&core).sessions.into_iter().filter(|v| !v.managed).collect();
    let mut rep = AdoptReport { adopted: vec![], skipped: vec![] };
    for v in externals {
        match adopt_view(&core, &v) {
            Ok(_) => rep.adopted.push(v.title.clone()),
            Err(e) => rep.skipped.push(format!("{}: {e}", v.title)),
        }
    }
    Ok(rep)
}

#[tauri::command]
pub async fn session_forget(core: State<'_, Shared>, key: String) -> Result<Snapshot, String> {
    if find_view(&core, &key).is_some_and(|v| v.status != "stopped") {
        return Err("Detén la sesión antes de quitarla de la lista".into());
    }
    // Si queda su ventana de tmux (la shell de una ventana adoptada tras salir de claude a mano),
    // se desvincula; si no, el siguiente sondeo la readoptaría y la sesión reaparecería fantasma.
    if let Some(h) = core.hosts.lock().unwrap().remove(&key) {
        match &h {
            Host::Window(_) => core.tmux.unset_key(&h),
            Host::Legacy(_) => {
                let _ = core.tmux.kill(&h);
            }
        }
    }
    {
        let mut p = core.persist.lock().unwrap();
        p.forget(&key);
        state::save(&p);
    }
    Ok(build(&core))
}

#[tauri::command]
pub async fn session_rename(core: State<'_, Shared>, key: String, name: String) -> Result<Snapshot, String> {
    {
        let mut p = core.persist.lock().unwrap();
        let n = name.trim();
        if n.is_empty() {
            p.names.remove(&key);
        } else {
            p.names.insert(key.clone(), n.to_string());
        }
        state::save(&p);
    }
    Ok(build(&core))
}

#[tauri::command]
/// Sin `order` (orden por recientes: sólo se arrastran las fijadas) se reordenan las fijadas y el
/// orden manual del resto se conserva para cuando se vuelva a él.
pub async fn set_layout(core: State<'_, Shared>, pinned: Vec<String>, order: Option<Vec<String>>) -> Result<Snapshot, String> {
    {
        let mut p = core.persist.lock().unwrap();
        let mut seen = HashSet::new();
        p.pinned = pinned.into_iter().filter(|k| seen.insert(k.clone())).collect();
        let order = order.unwrap_or_else(|| p.order.clone());
        p.order = order.into_iter().filter(|k| seen.insert(k.clone())).collect();
        state::save(&p);
    }
    Ok(build(&core))
}

#[tauri::command]
pub async fn set_pinned(core: State<'_, Shared>, key: String, pinned: bool) -> Result<Snapshot, String> {
    {
        let mut p = core.persist.lock().unwrap();
        p.pinned.retain(|k| k != &key);
        p.order.retain(|k| k != &key);
        if pinned {
            p.pinned.push(key);
        } else {
            p.order.insert(0, key);
        }
        state::save(&p);
    }
    Ok(build(&core))
}

#[tauri::command]
pub async fn set_exempt(core: State<'_, Shared>, key: String, exempt: bool) -> Result<Snapshot, String> {
    {
        let mut p = core.persist.lock().unwrap();
        p.exempt.retain(|k| k != &key);
        if exempt {
            p.exempt.push(key);
        }
        state::save(&p);
    }
    Ok(build(&core))
}

#[tauri::command]
pub async fn settings_update(core: State<'_, Shared>, settings: Settings) -> Result<Snapshot, String> {
    let name = settings.tmux_session.trim().to_string();
    core.tmux.set_session(&name)?;
    {
        let mut p = core.persist.lock().unwrap();
        let theme = match settings.theme.as_str() {
            "light" | "dark" | "sepia" => settings.theme.clone(),
            _ => "system".to_string(),
        };
        let zoom = if settings.zoom.is_finite() { settings.zoom.clamp(0.5, 2.5) } else { 1.0 };
        p.settings = Settings { tmux_session: name, theme, zoom, ..settings };
        state::save(&p);
    }
    Ok(build(&core))
}

/// Rutas de ficheros soltados en la ventana. Los de la NAS o Nextcloud abiertos desde el gestor
/// de ficheros llegan como `smb://…`, `davs://…`: se traducen a su ruta local de GVFS
/// (`/run/user/<uid>/gvfs/…`), que es la que pueden abrir Claude y el resto de programas.
#[tauri::command]
pub async fn resolve_paths(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .map(|p| {
            if !p.contains("://") {
                return p;
            }
            let out = std::process::Command::new("timeout")
                .args(["8", "gio", "info", &p])
                .env("LC_ALL", "C")
                .output();
            out.ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .find_map(|l| l.strip_prefix("local path: ").map(|s| s.trim().to_string()))
                })
                .unwrap_or(p)
        })
        .collect()
}

/// Carpeta de las imágenes pegadas en la caja de texto o en los to-dos.
fn pasted_dir() -> PathBuf {
    paths::home().join(".cache/ccdesk/pegados")
}

fn save_pasted(bytes: &[u8], mime: &str) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("La imagen está vacía".into());
    }
    let ext = match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        _ => "png",
    };
    let dir = pasted_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(format!("pegado-{}.{ext}", now_ms()));
    std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
    Ok(p.to_string_lossy().into_owned())
}

/// Imagen pegada que llega del evento de pegar (en binario; el tipo en la cabecera `x-mime`).
#[tauri::command]
pub fn save_image(request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("Se esperaba la imagen en binario".into());
    };
    let mime = request.headers().get("x-mime").and_then(|v| v.to_str().ok()).unwrap_or("image/png");
    save_pasted(bytes, mime)
}

/// Imagen del portapapeles del sistema (cuando WebKit no la pasa al evento de pegar).
#[tauri::command]
pub async fn clipboard_image() -> Result<String, String> {
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd).args(args).output().ok().filter(|o| o.status.success()).map(|o| o.stdout)
    };
    // X11 (xclip) y, si no, Wayland (wl-paste).
    let attempts: [(&str, &[&str], &[&str]); 2] = [
        ("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"], &["-selection", "clipboard", "-o", "-t"]),
        ("wl-paste", &["--list-types"], &["--no-newline", "--type"]),
    ];
    for (cmd, list, get) in attempts {
        let Some(types) = run(cmd, list) else { continue };
        let types = String::from_utf8_lossy(&types);
        let pick = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp"]
            .into_iter()
            .find(|t| types.lines().any(|l| l.trim() == *t));
        let Some(mime) = pick else { continue };
        let mut args = get.to_vec();
        args.push(mime);
        if let Some(bytes) = run(cmd, &args) {
            return save_pasted(&bytes, mime);
        }
    }
    Err("El portapapeles no tiene ninguna imagen".into())
}

/// Una imagen del transcript (foto que leyó Claude, captura pegada…) como URL `data:`. Sólo de
/// ficheros bajo `~/.claude/projects`.
#[tauri::command]
pub async fn transcript_image(path: String, offset: u64, n: usize) -> Result<String, String> {
    let p = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(paths::projects_dir()).map_err(|e| e.to_string())?;
    if !p.starts_with(&root) {
        return Err("fuera de los transcripts".into());
    }
    transcript::image_at(&p, offset, n).ok_or_else(|| "imagen no encontrada".into())
}

/// Sesiones cuya conversación contiene el texto (sobre el índice, que un hilo pone al día cada 10 s).
#[tauri::command]
pub async fn search_sessions(core: State<'_, Shared>, query: String) -> Result<Vec<crate::search::Hit>, String> {
    Ok(crate::search::search(&core, &query))
}

/// Lo que se puede escribir tras `/` en la sesión: comandos de Claude Code, skills y comandos propios.
#[tauri::command]
pub async fn slash_commands(core: State<'_, Shared>, key: String) -> Result<Vec<crate::slash::SlashCmd>, String> {
    let v = core.views.lock().unwrap().iter().find(|v| v.key == key).cloned();
    let (sid, cwd) = v.map(|v| (v.sid, v.cwd)).unwrap_or_default();
    let skills = crate::search::skills_of(&core, &sid);
    Ok(crate::slash::list(&core.claude, &cwd, skills))
}

#[tauri::command]
pub async fn send_input(core: State<'_, Shared>, key: String, text: String) -> Result<(), String> {
    let h = host_of(&core, &key).ok_or("La sesión no está en marcha")?;
    type_and_enter(&core, &h, &text)
}

fn type_and_enter(core: &Core, h: &Host, text: &str) -> Result<(), String> {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.trim().is_empty() {
        return Ok(());
    }
    // Tecleado, no pegado: un pegado grande se pliega como «[Pasted text]» y le llega al
    // modelo como texto pegado de otra fuente.
    core.tmux.type_text(h, text)?;
    std::thread::sleep(Duration::from_millis(120));
    core.tmux.send_keys(h, &["Enter"])
}

/// Caja de texto de la TUI a la vista: la línea `❯ ` justo bajo una regla (`────`). Recién
/// registrada la sesión puede no estar pintada aún, y lo tecleado se perdería.
fn input_ready(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().collect();
    let is_rule = |l: &str| l.trim_start().chars().take(10).filter(|&c| c == '─').count() == 10;
    (1..lines.len()).any(|i| {
        let mut cs = lines[i].trim_start().chars();
        cs.next() == Some('❯') && matches!(cs.next(), None | Some(' ' | '\u{a0}')) && is_rule(lines[i - 1])
    })
}

/// Lo enviado mientras la sesión arranca (o se reanuda): espera a que Claude se registre y su caja
/// de texto esté a la vista, y entonces lo teclea como `send_input`. Un diálogo previo (confiar en
/// la carpeta…) se espera también. Si la sesión no llega a arrancar (se detiene, se sale del
/// diálogo con «No»…), da error y el texto vuelve a la caja.
#[tauri::command]
pub async fn send_when_ready(core: State<'_, Shared>, key: String, text: String) -> Result<(), String> {
    let core = core.inner().clone();
    let job = tauri::async_runtime::spawn_blocking(move || {
        const FAIL: &str = "La sesión no ha llegado a arrancar: el mensaje vuelve a la caja";
        let t0 = Instant::now();
        // Recién pedida la reanudación, la sesión sigue un momento como detenida.
        let mut began = false;
        let h = loop {
            let status = core.views.lock().unwrap().iter().find(|v| v.key == key).map(|v| v.status.clone());
            match status.as_deref() {
                Some("idle" | "busy") => {
                    if let Some(h) = core.hosts.lock().unwrap().get(&key).cloned() {
                        break h;
                    }
                }
                Some("stopped") if began || t0.elapsed() > Duration::from_secs(20) => return Err(FAIL.to_string()),
                Some("stopped") => {}
                Some(_) => began = true,
                None => return Err("Sesión desconocida".to_string()),
            }
            if t0.elapsed() > Duration::from_secs(15 * 60) {
                return Err(FAIL.to_string());
            }
            std::thread::sleep(Duration::from_millis(300));
        };
        for _ in 0..25 {
            if core.tmux.capture(&h).as_deref().is_some_and(input_ready) {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        type_and_enter(&core, &h, &text)
    });
    job.await.map_err(|e| e.to_string())?
}

fn safe_arg(s: &str) -> bool {
    !s.is_empty() && s.len() <= 60 && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.[]".contains(c))
}

// ─────────── selectores de Claude Code (/model, /effort) ───────────
// Se usan los selectores de la propia TUI: dan las opciones reales y su tecla «s» aplica el
// cambio sólo a esta sesión (`/model <x>` o `/effort <x>` lo guardarían como predeterminado).

/// Filas numeradas del selector de modelos: (número, etiqueta, descripción), cursor y «+N más».
/// Sólo cuenta lo que hay bajo su cabecera: encima sigue a la vista la conversación, y una línea
/// suya puede empezar por un número con punto (un «0.9» partido al ajustar el párrafo).
fn picker_rows(screen: &str) -> (Vec<ModelOpt>, Option<u32>, usize) {
    let mut rows = Vec::new();
    let (mut cursor, mut more) = (None, 0);
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.iter().rposition(|l| l.contains("Select model")).map_or(0, |i| i + 1);
    for line in &lines[start..] {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix('…') {
            more = rest.trim().trim_start_matches('+').split_whitespace().next()
                .and_then(|x| x.parse().ok()).unwrap_or(0);
            continue;
        }
        let mut is_cursor = false;
        let mut body = t;
        loop {
            if let Some(r) = body.strip_prefix('❯') { is_cursor = true; body = r.trim_start(); }
            else if let Some(r) = body.strip_prefix('↑').or_else(|| body.strip_prefix('↓')) { body = r.trim_start(); }
            else { break; }
        }
        let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
        // «N.» seguido de espacio, como en el selector («1.  Default»), no «0.9».
        if digits.is_empty() || !body[digits.len()..].starts_with(". ") { continue; }
        let Ok(n) = digits.parse::<u32>() else { continue };
        let rest = body[digits.len() + 1..].trim();
        let (label, desc) = match rest.find("  ") {
            Some(i) => (rest[..i].trim(), rest[i..].trim()),
            None => (rest, ""),
        };
        let label = label.trim_end_matches('✔').trim().to_string();
        if is_cursor { cursor = Some(n); }
        rows.push(ModelOpt { n, label, desc: desc.to_string() });
    }
    (rows, cursor, more)
}

/// Niveles del deslizador de `/effort` y el seleccionado (el que está bajo `▲`). Sólo cuentan
/// las palabras que caen bajo el propio deslizador (detrás van «Ultracode» y «Tab to toggle»).
fn effort_slider(screen: &str) -> Option<(Vec<String>, usize)> {
    let lines: Vec<&str> = screen.lines().collect();
    let mi = lines.iter().position(|l| l.contains('▲'))?;
    let bar: Vec<char> = lines[mi].chars().collect();
    let mark = bar.iter().position(|&c| c == '▲')?;
    let is_bar = |c: char| c == '─' || c == '▲';
    let left = bar.iter().position(|&c| is_bar(c))?;
    let right = bar.iter().rposition(|&c| is_bar(c))?;
    let chars: Vec<char> = lines.get(mi + 1)?.chars().collect();
    let mut words: Vec<(String, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_alphanumeric() {
            let start = i;
            while i < chars.len() && chars[i].is_alphanumeric() {
                i += 1;
            }
            if start + 3 >= left && i <= right + 4 {
                words.push((chars[start..i].iter().collect(), start, i));
            }
        } else {
            i += 1;
        }
    }
    if words.len() < 2 {
        return None;
    }
    let cur = words
        .iter()
        .enumerate()
        .min_by_key(|(_, (_, a, b))| ((a + b) / 2).abs_diff(mark))?
        .0;
    Some((words.into_iter().map(|(w, _, _)| w).collect(), cur))
}

fn idle_host(core: &Core, key: &str) -> Result<Host, String> {
    let v = find_view(core, key).ok_or("Sesión desconocida")?;
    if !v.managed || v.status != "idle" {
        return Err("Espera a que Claude termine (sólo se puede con la sesión inactiva)".into());
    }
    host_of(core, key).ok_or_else(|| "La sesión no está en marcha".into())
}

/// Abre un selector (`/model`, `/effort`) y espera a que aparezca.
fn open_picker(t: &Tmux, h: &Host, cmd: &str, marker: &str) -> Result<String, String> {
    t.send_literal(h, cmd)?;
    std::thread::sleep(Duration::from_millis(250));
    t.send_keys(h, &["Enter"])?;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(120));
        if let Some(sc) = t.capture(h) {
            if sc.contains(marker) && sc.contains("Esc to cancel") {
                return Ok(sc);
            }
        }
    }
    let _ = t.send_keys(h, &["Escape"]);
    Err(format!("No se abrió el selector {cmd}"))
}

fn step_and_capture(t: &Tmux, h: &Host, key: &str) -> String {
    let _ = t.send_keys(h, &[key]);
    std::thread::sleep(Duration::from_millis(130));
    t.capture(h).unwrap_or_default()
}

/// Recorre el selector de modelos entero: sólo se ven unas filas. Es circular (se para al volver
/// a la de partida); si no lo fuera, al llegar abajo sube hasta arriba.
fn read_model_list(t: &Tmux, h: &Host, first: &str) -> Vec<ModelOpt> {
    let (rows, start, _) = picker_rows(first);
    let mut all: HashMap<u32, ModelOpt> = rows.into_iter().map(|r| (r.n, r)).collect();
    let (mut dir, mut prev) = ("Down", start);
    for _ in 0..80 {
        let (rows, cur, _) = picker_rows(&step_and_capture(t, h, dir));
        for r in rows {
            all.entry(r.n).or_insert(r);
        }
        if cur.is_none() || (dir == "Down" && cur == start) {
            break;
        }
        if cur == prev {
            if dir == "Up" {
                break;
            }
            dir = "Up";
        }
        prev = cur;
    }
    let mut list: Vec<ModelOpt> = all.into_values().collect();
    list.sort_by_key(|m| m.n);
    list
}

/// Nombre de la sesión tmux oculta (no empieza por `cc-`: no se lista).
const PROBE_SESSION: &str = "ccdesk-sonda";

/// Lee el selector `/model` en un Claude Code aparte, oculto y sin servidores MCP ni hooks, en vez
/// de en una conversación: abrirlo allí la deja con un «Kept model as …» que también le llega a
/// Claude. Su transcript se borra al acabar.
fn probe_models(t: &Tmux, claude: &Path) -> Result<Vec<ModelOpt>, String> {
    let dir = paths::probe_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dir_s = dir.to_string_lossy().into_owned();
    let sid = uuid::Uuid::new_v4().to_string();
    let argv: Vec<String> = [
        claude.to_string_lossy().as_ref(),
        "--session-id",
        &sid,
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--settings",
        r#"{"disableAllHooks":true,"disableClaudeAiConnectors":true}"#,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let h = t.spawn_hidden(PROBE_SESSION, &dir_s, &argv)?;
    let pid = t.pane_pid(&h);
    let res = probe_read(t, &h);
    let _ = t.kill(&h);
    if let Some(pid) = pid {
        // Que termine de escribir antes de borrar lo que deja.
        for _ in 0..30 {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let tdir = paths::projects_dir().join(paths::encode_cwd(&dir_s));
    let _ = std::fs::remove_file(tdir.join(format!("{sid}.jsonl")));
    let _ = std::fs::remove_dir_all(tdir.join(&sid));
    let _ = std::fs::remove_dir(&tdir); // sólo si ha quedado vacía
    res
}

fn probe_read(t: &Tmux, h: &Host) -> Result<Vec<ModelOpt>, String> {
    let t0 = Instant::now();
    // La primera vez pregunta si se confía en la carpeta (Intro = sí, la opción marcada).
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let Some(sc) = t.capture(h) else {
            return Err("La consulta de modelos se cerró al arrancar Claude Code".into());
        };
        if input_ready(&sc) {
            break;
        }
        if startup_prompt(&sc) {
            t.send_keys(h, &["Enter"])?;
            std::thread::sleep(Duration::from_millis(600));
        }
        if t0.elapsed() > Duration::from_secs(25) {
            return Err("Claude Code no terminó de arrancar para leer los modelos".into());
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    let first = open_picker(t, h, "/model", "Select model")?;
    let list = read_model_list(t, h, &first);
    let _ = t.send_keys(h, &["Escape"]);
    if list.is_empty() {
        Err("No he podido leer el selector de modelos".into())
    } else {
        Ok(list)
    }
}

/// Versión de Claude Code que arrancan las sesiones nuevas: el nombre del binario al que lleva el
/// enlace (`~/.local/share/claude/versions/2.1.317`) o, si no, `claude --version`.
fn claude_version(bin: &Path) -> String {
    let real = std::fs::canonicalize(bin).ok();
    if let Some(name) = real.as_deref().and_then(Path::file_name).and_then(|s| s.to_str()) {
        if name.contains('.') && name.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return name.to_string();
        }
    }
    std::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().next().map(String::from))
        .unwrap_or_default()
}

#[derive(Serialize, Deserialize)]
struct ModelsCache {
    version: String,
    models: Vec<ModelOpt>,
}

fn cached_models(core: &Core, ver: &str) -> Option<Vec<ModelOpt>> {
    if let Some((v, m)) = &core.opts.lock().unwrap().models {
        if v == ver && !m.is_empty() {
            return Some(m.clone());
        }
    }
    let c: ModelsCache = serde_json::from_str(&std::fs::read_to_string(paths::models_cache()).ok()?).ok()?;
    if c.version != ver || c.models.is_empty() {
        return None;
    }
    core.opts.lock().unwrap().models = Some((c.version, c.models.clone()));
    Some(c.models)
}

/// Una consulta a la vez (el arranque en segundo plano y el desplegable pueden coincidir).
static PROBE_LOCK: Mutex<()> = Mutex::new(());

/// Modelos de la versión instalada de Claude Code: los guardados o, si ha cambiado de versión,
/// los que da la sesión oculta.
pub fn models_now(core: &Core) -> Result<Vec<ModelOpt>, String> {
    let ver = claude_version(&core.claude);
    if let Some(m) = cached_models(core, &ver) {
        return Ok(m);
    }
    let _g = PROBE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = cached_models(core, &ver) {
        return Ok(m);
    }
    let list = probe_models(&core.tmux, &core.claude)?;
    log(&format!("modelos de Claude Code {ver}: {}", list.iter().map(|m| m.label.as_str()).collect::<Vec<_>>().join(", ")));
    core.opts.lock().unwrap().models = Some((ver.clone(), list.clone()));
    if let Ok(s) = serde_json::to_string_pretty(&ModelsCache { version: ver, models: list.clone() }) {
        let _ = std::fs::write(paths::models_cache(), s);
    }
    Ok(list)
}

/// Modelos que ofrece Claude Code (lista completa del selector `/model`). No depende de la sesión:
/// se lee aparte, sin dejar rastro en ninguna conversación.
#[tauri::command]
pub async fn session_models(core: State<'_, Shared>, key: String) -> Result<Vec<ModelOpt>, String> {
    let _ = key;
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || models_now(&core)).await.map_err(|e| e.to_string())?
}

/// Cambia el modelo sólo para esta sesión: cursor a su fila del selector y «s». Se busca por la
/// etiqueta: una sesión que siga con una versión anterior de Claude Code puede tener otra lista.
/// `n` (su posición en la lista leída) da el sentido más corto.
#[tauri::command]
pub async fn session_set_model(core: State<'_, Shared>, key: String, n: u32, label: Option<String>) -> Result<(), String> {
    let h = idle_host(&core, &key)?;
    let t = &core.tmux;
    let mut sc = open_picker(t, &h, "/model", "Select model")?;
    let label = label.unwrap_or_default();
    let known = core.opts.lock().unwrap().models.as_ref().map_or(0, |(_, m)| m.len());
    let total = known.max(picker_rows(&sc).0.len()).max(1) as i64;
    let (mut dir, mut prev): (Option<&str>, Option<u32>) = (None, None);
    for _ in 0..(total * 2 + 4) {
        let (rows, cur, _) = picker_rows(&sc);
        let Some(c) = cur else { break };
        let hit = if label.is_empty() {
            c == n
        } else {
            rows.iter().any(|r| r.n == c && r.label == label)
        };
        if hit {
            t.send_keys(&h, &["s"])?;
            std::thread::sleep(Duration::from_millis(400));
            if t.capture(&h).is_some_and(|x| x.contains("Select model")) {
                let _ = t.send_keys(&h, &["Escape"]);
                return Err("El selector no aceptó el cambio".into());
            }
            return Ok(());
        }
        // Camino más corto hasta `n` en una lista circular; si no está ahí, se sigue en el mismo
        // sentido hasta dar la vuelta (y se invierte si la lista resulta tener tope).
        let d = *dir.get_or_insert_with(|| {
            let down = (n as i64 - c as i64).rem_euclid(total);
            if down <= total - down { "Down" } else { "Up" }
        });
        if prev == Some(c) {
            dir = Some(if d == "Down" { "Up" } else { "Down" });
        }
        prev = Some(c);
        sc = step_and_capture(t, &h, dir.unwrap_or("Down"));
    }
    let _ = t.send_keys(&h, &["Escape"]);
    Err("No encuentro ese modelo en el selector".into())
}

#[derive(Serialize)]
pub struct EffortOpts {
    levels: Vec<String>,
    current: String,
}

/// Niveles de effort que ofrece Claude Code para el modelo de la sesión.
#[tauri::command]
pub async fn session_efforts(core: State<'_, Shared>, key: String) -> Result<EffortOpts, String> {
    let v = find_view(&core, &key).ok_or("Sesión desconocida")?;
    let model = read_status(&v.sid).and_then(|s| s["model"]["id"].as_str().map(String::from)).unwrap_or_default();
    let cur = read_status(&v.sid).and_then(|s| s["effort"]["level"].as_str().map(String::from)).unwrap_or_default();
    if let Some(l) = core.opts.lock().unwrap().efforts.get(&model) {
        return Ok(EffortOpts { levels: l.clone(), current: cur });
    }
    let h = idle_host(&core, &key)?;
    let sc = open_picker(&core.tmux, &h, "/effort", "Effort")?;
    let _ = core.tmux.send_keys(&h, &["Escape"]);
    let (levels, i) = effort_slider(&sc).ok_or("No he podido leer los niveles de effort")?;
    let current = levels.get(i).cloned().unwrap_or(cur);
    core.opts.lock().unwrap().efforts.insert(model, levels.clone());
    Ok(EffortOpts { levels, current })
}

/// Cambia el effort sólo para esta sesión: deslizador de `/effort` hasta el nivel y «s».
#[tauri::command]
pub async fn session_set_effort(core: State<'_, Shared>, key: String, effort: String) -> Result<(), String> {
    let h = idle_host(&core, &key)?;
    let mut sc = open_picker(&core.tmux, &h, "/effort", "Effort")?;
    for _ in 0..12 {
        let Some((levels, cur)) = effort_slider(&sc) else { break };
        let Some(target) = levels.iter().position(|l| *l == effort) else {
            let _ = core.tmux.send_keys(&h, &["Escape"]);
            return Err(format!("Claude no ofrece el effort «{effort}» para este modelo"));
        };
        if cur == target {
            core.tmux.send_keys(&h, &["s"])?;
            std::thread::sleep(Duration::from_millis(400));
            if core.tmux.capture(&h).is_some_and(|x| x.contains("Esc to cancel")) {
                let _ = core.tmux.send_keys(&h, &["Escape"]);
                return Err("El selector no aceptó el cambio".into());
            }
            return Ok(());
        }
        sc = step_and_capture(&core.tmux, &h, if target > cur { "Right" } else { "Left" });
    }
    let _ = core.tmux.send_keys(&h, &["Escape"]);
    Err("No he podido mover el deslizador de effort".into())
}

/// Refresca el desglose de cuotas abriendo `/usage` en la sesión (inactiva) y cerrándolo en
/// cuanto Claude Code actualiza su caché.
#[tauri::command]
pub async fn usage_refresh(core: State<'_, Shared>, key: String) -> Result<(), String> {
    let h = idle_host(&core, &key)?;
    let read = || {
        std::fs::read_to_string(paths::home().join(".claude.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|j| j["cachedUsageUtilization"]["fetchedAtMs"].as_u64())
    };
    let before = read();
    core.tmux.send_literal(&h, "/usage")?;
    std::thread::sleep(Duration::from_millis(250));
    core.tmux.send_keys(&h, &["Enter"])?;
    // Un único Escape al final (dos seguidos abrirían el menú de rebobinar de Claude Code).
    let mut updated = false;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(200));
        if read() != before {
            updated = true;
            break;
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    let panel_open = core.tmux.capture(&h).is_some_and(|sc| sc.contains("Current session") || sc.contains("Current week"));
    if panel_open {
        let _ = core.tmux.send_keys(&h, &["Escape"]);
    }
    if updated { Ok(()) } else { Err("Claude Code no ha actualizado las cuotas".into()) }
}

/// Ciclo real de modos de la sesión (Mayús+Tab): se recorre una vuelta y se vuelve al de partida.
#[tauri::command]
pub async fn session_modes(core: State<'_, Shared>, key: String) -> Result<Vec<String>, String> {
    if let Some(m) = core.opts.lock().unwrap().modes.get(&key) {
        return Ok(m.clone());
    }
    let h = idle_host(&core, &key)?;
    let read = |h: &Host| core.tmux.capture(h).as_deref().and_then(detect_mode).map(String::from);
    let start = read(&h).ok_or("No veo el modo actual en el pie de la sesión")?;
    let mut cycle = vec![start.clone()];
    for _ in 0..8 {
        core.tmux.send_keys(&h, &["BTab"])?;
        std::thread::sleep(Duration::from_millis(350));
        match read(&h) {
            Some(m) if m == start => break,
            Some(m) if !cycle.contains(&m) => cycle.push(m),
            _ => {}
        }
    }
    // Asegurar que queda en el modo de partida.
    for _ in 0..8 {
        if read(&h).as_deref() == Some(start.as_str()) { break; }
        core.tmux.send_keys(&h, &["BTab"])?;
        std::thread::sleep(Duration::from_millis(350));
    }
    core.opts.lock().unwrap().modes.insert(key, cycle.clone());
    Ok(cycle)
}

/// Lleva la sesión al modo de permisos pedido pulsando Mayús+Tab y leyendo el pie.
#[tauri::command]
pub async fn session_set_mode(core: State<'_, Shared>, key: String, mode: String) -> Result<String, String> {
    let h = host_of(&core, &key).ok_or("La sesión no está en marcha")?;
    let mut seen = Vec::new();
    for _ in 0..8 {
        // Sin pie a la vista (un selector o diálogo abierto en la terminal) no se pulsa nada: el
        // Mayús+Tab iría a ciegas a ese diálogo.
        let Some(c) = core.tmux.capture(&h).as_deref().and_then(detect_mode).map(String::from) else {
            return Err("No veo el pie de Claude Code en la terminal (¿hay un diálogo abierto?): no he tocado el modo".into());
        };
        core.modes.lock().unwrap().insert(key.clone(), c.clone());
        if c == mode {
            return Ok(c);
        }
        if seen.contains(&c) && seen.len() >= 2 && seen[0] == c {
            break; // vuelta completa sin encontrarlo
        }
        seen.push(c);
        core.tmux.send_keys(&h, &["BTab"])?;
        std::thread::sleep(Duration::from_millis(350));
    }
    if mode == "bypassPermissions" {
        // La UI lo reconoce y ofrece reiniciar la sesión en modo YOLO.
        Err("YOLO_UNAVAILABLE".into())
    } else {
        Err(format!("No se ha podido poner el modo «{mode}» en esta sesión"))
    }
}

/// Detiene la sesión (inactiva) y la reanuda enseguida, opcionalmente en otro modo de permisos.
/// Sirve para habilitar el modo YOLO en una sesión que arrancó sin él.
#[tauri::command]
pub async fn session_restart(core: State<'_, Shared>, key: String, mode: Option<String>) -> Result<(), String> {
    let v = find_view(&core, &key).ok_or("Sesión desconocida")?;
    if !v.managed {
        return Err("Sólo se pueden reiniciar sesiones gestionadas por ccdesk".into());
    }
    if v.status != "idle" {
        return Err("Espera a que Claude termine para reiniciarla".into());
    }
    if v.background {
        return Err("Tiene trabajo en segundo plano (comandos, Monitor o subagentes) y reiniciarla lo cortaría. Espera a que acabe.".into());
    }
    let h = host_of(&core, &key).ok_or("La sesión no está en marcha")?;
    graceful_stop(&core, &key);
    let mut gone = false;
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(250));
        if !core.tmux.exists(&h) || v.pid.is_some_and(|p| !registry::alive(p, None)) {
            gone = true;
            break;
        }
    }
    if !gone {
        return Err("No se ha podido detener la sesión".into());
    }
    // Margen para que se cierre la ventana adoptada (la shell que queda tras salir de claude).
    std::thread::sleep(Duration::from_millis(600));
    let extra = match mode.as_deref() {
        Some(m) if safe_arg(m) => vec!["--permission-mode".to_string(), m.to_string()],
        _ => vec![],
    };
    resume_with(&core, &key, &extra)
}

#[tauri::command]
pub fn send_keys(core: State<'_, Shared>, key: String, keys: Vec<String>) -> Result<(), String> {
    let h = host_of(&core, &key).ok_or("La sesión no está en marcha")?;
    let k: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
    core.tmux.send_keys(&h, &k)
}

#[tauri::command]
pub fn term_open(
    app: AppHandle,
    core: State<'_, Shared>,
    key: String,
    cols: u16,
    rows: u16,
    on_data: Channel<String>,
    gen: u64,
) -> Result<(), String> {
    *core.pty.lock().unwrap() = None;
    let h = host_of(&core, &key).ok_or("La sesión no está en marcha")?;
    let argv = core.tmux.attach_argv(&h);
    let mut slot = core.pty.lock().unwrap();
    let k2 = key.clone();
    // `gen`: el pty vigente según la interfaz; el cierre de uno anterior puede llegar después de
    // abrir el siguiente y no debe confundirse con él.
    let pty = Pty::spawn(&argv, cols, rows, on_data, move || {
        let _ = app.emit("term-exit", json!({ "key": k2, "gen": gen }));
    })?;
    *slot = Some(pty);
    Ok(())
}

#[tauri::command]
pub async fn term_write(core: State<'_, Shared>, data: String) -> Result<(), String> {
    match core.pty.lock().unwrap().as_mut() {
        Some(p) => p.write(data.as_bytes()),
        None => Ok(()),
    }
}

#[tauri::command]
pub fn term_resize(core: State<'_, Shared>, cols: u16, rows: u16) -> Result<(), String> {
    match core.pty.lock().unwrap().as_ref() {
        Some(p) => p.resize(cols, rows),
        None => Ok(()),
    }
}

#[tauri::command]
pub fn term_close(core: State<'_, Shared>) {
    *core.pty.lock().unwrap() = None;
}

/// Lanza un programa y se desentiende, pero recogiendo su salida en un hilo: si no, al terminar
/// quedaría como proceso zombi colgando de ccdesk.
fn spawn_detached(cmd: &str, arg: impl AsRef<std::ffi::OsStr>) -> Result<(), String> {
    let mut child = std::process::Command::new(cmd)
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    let ok = ["http://", "https://", "mailto:", "file://"]
        .iter()
        .any(|p| url.starts_with(p));
    if !ok {
        return Err("Esquema no permitido".into());
    }
    spawn_detached("xdg-open", &url)
}

#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    let p = paths::resolve_dir(&path).ok_or("No existe")?;
    spawn_detached("xdg-open", p)
}

/// Qué es cada ruta: 1 fichero, 2 carpeta, 0 no existe. Para el icono de abrir que va tras las
/// rutas de la conversación.
#[tauri::command]
pub fn files_exist(list: Vec<String>) -> Vec<u8> {
    list.iter()
        .map(|p| {
            let p = paths::expand_home(p);
            if p.is_file() {
                1
            } else if p.is_dir() {
                2
            } else {
                0
            }
        })
        .collect()
}

/// Abre un fichero con su aplicación predeterminada (una carpeta, en el gestor de archivos).
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    let p = paths::expand_home(&path);
    if !p.is_file() && !p.is_dir() {
        return Err(format!("Ya no existe: {path}"));
    }
    spawn_detached("xdg-open", p)
}

/// Imagen de disco, para verla en grande dentro de ccdesk (como las miniaturas de la conversación)
/// al pulsar el icono que va tras su ruta. Los bytes van tal cual (la interfaz recibe un ArrayBuffer).
#[tauri::command]
pub async fn image_file(path: String) -> Result<tauri::ipc::Response, String> {
    let p = paths::expand_home(&path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"].contains(&ext.as_str()) {
        return Err("No es una imagen".into());
    }
    let meta = std::fs::metadata(&p).ok().filter(|m| m.is_file()).ok_or_else(|| format!("Ya no existe: {path}"))?;
    if meta.len() > 40 << 20 {
        return Err("Imagen demasiado grande para verla aquí".into());
    }
    std::fs::read(&p).map(tauri::ipc::Response::new).map_err(|e| e.to_string())
}

/// Zoom de la interfaz (lo guarda el frontend en los ajustes).
#[tauri::command]
pub fn set_zoom(window: tauri::WebviewWindow, zoom: f64) -> Result<(), String> {
    window.set_zoom(zoom.clamp(0.5, 2.5)).map_err(|e| e.to_string())
}

/// Scratchpad de la sesión: `<tmp>/claude-<uid>/<cwd codificado>/<sid>/scratchpad`. Se busca en
/// todas las carpetas de proyecto, porque la del arranque puede no ser la carpeta actual.
fn scratchpad_dir(sid: &str) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    if sid.is_empty() || !sid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let uid = std::fs::metadata("/proc/self").ok()?.uid();
    let tmp = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    std::fs::read_dir(tmp.join(format!("claude-{uid}")))
        .ok()?
        .flatten()
        .map(|e| e.path().join(sid).join("scratchpad"))
        .find(|p| p.is_dir())
}

#[tauri::command]
pub fn open_scratchpad(sid: String) -> Result<(), String> {
    let p = scratchpad_dir(&sid)
        .ok_or("Esta sesión no tiene scratchpad (Claude Code lo crea en /tmp y se pierde al reiniciar el equipo)")?;
    spawn_detached("xdg-open", p)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivity {
    agent_id: String,
    tool_uses: usize,
    recent: Vec<String>,
    last_text: String,
    finished: bool,
}

/// `subagents/agent-<id>.jsonl` de la sesión activa cuyo `.meta.json` apunta a esta llamada.
fn agent_file(core: &Core, tool_use_id: &str) -> Option<(PathBuf, bool)> {
    let main = core.tcache.lock().unwrap().as_ref()?.path.clone();
    let dir = main.with_extension("").join("subagents");
    for e in std::fs::read_dir(&dir).ok()?.flatten() {
        let p = e.path();
        let name = p.file_name()?.to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".meta.json") else {
            continue;
        };
        let Ok(meta) = std::fs::read_to_string(&p)
            .map_err(|_| ())
            .and_then(|s| serde_json::from_str::<Value>(&s).map_err(|_| ()))
        else {
            continue;
        };
        if meta["toolUseId"].as_str() == Some(tool_use_id) {
            let finished = meta["finished"].as_bool().unwrap_or(false);
            return Some((dir.join(format!("{stem}.jsonl")), finished));
        }
    }
    None
}

/// Abre (o refresca) el transcript del subagente y aplica `f` a sus items.
fn with_agent<T>(core: &Core, tool_use_id: &str, f: impl FnOnce(&Cache, bool) -> T) -> Option<T> {
    let (path, finished) = agent_file(core, tool_use_id)?;
    let mut map = core.agents.lock().unwrap();
    if !map.contains_key(tool_use_id) {
        let c = Cache::open_opts(tool_use_id, "", &path, true).ok()?;
        map.insert(tool_use_id.to_string(), c);
    }
    let c = map.get_mut(tool_use_id)?;
    let _ = c.refresh();
    Some(f(c, finished))
}

/// Progreso de un subagente: herramientas usadas, las últimas y su último texto.
#[tauri::command]
pub async fn agent_activity(core: State<'_, Shared>, tool_use_id: String) -> Result<Option<AgentActivity>, String> {
    Ok(with_agent(&core, &tool_use_id, |c, finished| {
        let mut tool_uses = 0;
        let mut recent = Vec::new();
        let mut last_text = String::new();
        for it in &c.items {
            match it {
                Item::Tool { name, summary, .. } => {
                    tool_uses += 1;
                    recent.push(if summary.is_empty() { name.clone() } else { format!("{name} · {summary}") });
                }
                Item::Agent { description, .. } => {
                    tool_uses += 1;
                    recent.push(format!("Agent · {description}"));
                }
                Item::Assistant { text, .. } => last_text = text.clone(),
                _ => {}
            }
        }
        let n = recent.len();
        let recent = recent.split_off(n.saturating_sub(3));
        let agent_id = c
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().trim_start_matches("agent-").to_string())
            .unwrap_or_default();
        let mut lt: String = last_text.chars().take(400).collect();
        if lt.len() < last_text.len() {
            lt.push('…');
        }
        AgentActivity { agent_id, tool_uses, recent, last_text: lt, finished }
    }))
}

/// Conversación completa del subagente (como la principal, con sus herramientas).
#[tauri::command]
pub async fn agent_transcript(core: State<'_, Shared>, tool_use_id: String) -> Result<Vec<Item>, String> {
    with_agent(&core, &tool_use_id, |c, _| {
        let start = c.items.len().saturating_sub(2000);
        c.items[start..].to_vec()
    })
    .ok_or_else(|| "No encuentro el transcript de este subagente".to_string())
}

#[tauri::command]
pub fn notes_get(core: State<'_, Shared>, key: String) -> Vec<state::Note> {
    core.persist.lock().unwrap().notes.get(&key).cloned().unwrap_or_default()
}

#[tauri::command]
pub fn notes_save(core: State<'_, Shared>, key: String, notes: Vec<state::Note>) {
    let mut p = core.persist.lock().unwrap();
    if notes.is_empty() {
        p.notes.remove(&key);
    } else {
        p.notes.insert(key, notes);
    }
    state::save(&p);
}

/// Historial de prompts de la TUI (`~/.claude/history.jsonl`) para un proyecto, del más
/// reciente al más antiguo, con los textos pegados (`[Pasted text #N …]`) reconstruidos.
#[tauri::command]
pub async fn prompt_history(cwd: String, limit: usize) -> Result<Vec<String>, String> {
    let f = paths::claude_dir().join("history.jsonl");
    let data = std::fs::read_to_string(&f).map_err(|e| e.to_string())?;
    let mut out: Vec<String> = Vec::new();
    for line in data.lines().rev() {
        if out.len() >= limit {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["project"].as_str() != Some(cwd.as_str()) {
            continue;
        }
        let mut text = v["display"].as_str().unwrap_or("").to_string();
        if let Some(pc) = v["pastedContents"].as_object() {
            for (id, p) in pc {
                let Some(content) = p["content"].as_str() else { continue };
                let tag = format!("[Pasted text #{id}");
                if let Some(a) = text.find(&tag) {
                    if let Some(b) = text[a..].find(']') {
                        text.replace_range(a..a + b + 1, content);
                    }
                }
            }
        }
        if text.trim().is_empty() || out.last() == Some(&text) {
            continue;
        }
        out.push(text);
    }
    Ok(out)
}

// ───────────────────────────── sondeo ─────────────────────────────

fn poll_transcript(app: &AppHandle, core: &Core, tick: u64) {
    let Some(key) = core.active.lock().unwrap().clone() else {
        return;
    };
    let view_sid = || {
        core.views
            .lock()
            .unwrap()
            .iter()
            .find(|v| v.key == key)
            .map(|v| (v.sid.clone(), v.cwd.clone()))
    };
    let mut tc = core.tcache.lock().unwrap();
    match tc.as_mut() {
        Some(c) if c.key == key => {
            let mut payload = None;
            if let Ok(n) = c.refresh() {
                if n > 0 {
                    let start = c.items.len() - n;
                    payload = Some(json!({
                        "key": c.key, "path": c.path, "startIndex": start, "items": c.items[start..]
                    }));
                }
            }
            let sid_changed = tick.is_multiple_of(5)
                && view_sid().is_some_and(|(sid, _)| !sid.is_empty() && sid != c.sid);
            drop(tc);
            if let Some(p) = payload {
                let _ = app.emit("transcript-append", p);
            }
            if sid_changed {
                let _ = app.emit("transcript-available", &key);
            }
        }
        _ => {
            drop(tc);
            if !tick.is_multiple_of(5) {
                return;
            }
            if let Some((sid, cwd)) = view_sid() {
                if transcript_of(&sid, &cwd).is_some() {
                    let _ = app.emit("transcript-available", &key);
                }
            }
        }
    }
}

fn autostop(core: &Core) {
    let (on, mins, since) = {
        let p = core.persist.lock().unwrap();
        let since: HashMap<String, u64> = p.known.iter().map(|(k, v)| (k.clone(), v.managed_since)).collect();
        (p.settings.autostop, p.settings.autostop_minutes, since)
    };
    if !on || mins == 0 {
        return;
    }
    let active = core.active.lock().unwrap().clone();
    let views = core.views.lock().unwrap().clone();
    let now = now_ms();
    for v in views.iter().filter(|v| {
        v.managed
            && v.status == "idle"
            && !v.background
            && !v.exempt
            && active.as_deref() != Some(v.key.as_str())
    }) {
        // Inactiva desde lo último entre: su actividad, que ccdesk la gestiona y el arranque
        // de la app (adoptar una sesión parada hace horas no debe detenerla al momento).
        let idle_since = v
            .last_activity
            .max(since.get(&v.key).copied().unwrap_or(0))
            .max(core.started_at);
        if now.saturating_sub(idle_since) > mins * 60_000 {
            log(&format!("auto-stop {} ({}) inactiva desde {}", v.key, v.title, idle_since / 1000));
            graceful_stop(core, &v.key);
        }
    }
}

pub fn spawn_poller(app: AppHandle, core: Shared) {
    std::thread::spawn(move || {
        let mut tick: u64 = 0;
        let mut notifier = crate::notify::Notifier::default();
        loop {
            poll_transcript(&app, &core, tick);
            if tick.is_multiple_of(3) {
                let snap = build(&core);
                notifier.observe(&app, &core, &snap);
                reattach_opened(&core, &snap);
                if let Ok(s) = serde_json::to_string(&snap) {
                    let mut last = core.last_snap.lock().unwrap();
                    if *last != s {
                        *last = s;
                        drop(last);
                        let _ = app.emit("snapshot", &snap);
                    }
                }
            }
            if tick % 75 == 74 {
                autostop(&core);
            }
            tick = tick.wrapping_add(1);
            std::thread::sleep(Duration::from_millis(400));
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn model_picker_rows() {
        let sc = "   Select model\n   ↑ 2.  Opus 5.5               For complex work\n     3.  Fable 5.1              For your toughest challenges\n   ❯ 4.  Sonnet 5.5 ✔           Most efficient\n      … +9 models\n";
        let (rows, cur, more) = super::picker_rows(sc);
        assert_eq!(rows.iter().map(|r| (r.n, r.label.as_str())).collect::<Vec<_>>(), vec![(2, "Opus 5.5"), (3, "Fable 5.1"), (4, "Sonnet 5.5")]);
        assert_eq!((cur, more), (Some(4), 9));
        let (_, _, more1) = super::picker_rows("      … +1 model\n");
        assert_eq!(more1, 1);
        // La conversación sigue a la vista encima del selector: una línea suya que empieza por
        // «0.9 → …» (párrafo partido al ajustarlo) no es un modelo.
        let sc = "  **5. Los 13 sin categoría**, con lo que les pondría: Grapadora Maped, Minas\n  0.9 → Oficina; Cable 2.5 mm² y Cables de tela → Electricidad; Mosquetones,\n  1. Mover algo\n ────────\n   Select model\n   Switch between Claude models.\n\n   ❯ 1.  Default (recommended)   Opus 5.5\n     2.  Opus 5.5 ✔             For complex work\n";
        let (rows, cur, _) = super::picker_rows(sc);
        assert_eq!(rows.iter().map(|r| (r.n, r.label.as_str())).collect::<Vec<_>>(), vec![(1, "Default (recommended)"), (2, "Opus 5.5")]);
        assert_eq!(cur, Some(1));
        // Sin cabecera a la vista (no debería pasar), «0.9» tampoco cuenta como fila.
        assert!(super::picker_rows("  0.9 → Oficina\n").0.is_empty());
    }

    /// Prueba real: arranca la sesión oculta y lee el selector (`cargo test -- --ignored sonda`).
    #[test]
    #[ignore]
    fn sonda_modelos() {
        let t = crate::tmux::Tmux::new(crate::paths::tmux_conf(), "claude");
        let list = super::probe_models(&t, &crate::paths::claude_bin()).expect("sonda");
        eprintln!("{list:#?}");
        assert!(list.len() >= 3);
        assert!(list.iter().all(|m| m.n >= 1 && !m.label.is_empty()));
        // Ni transcript ni sesión tmux ni entrada en el registro de procesos.
        let tdir = crate::paths::projects_dir().join(crate::paths::encode_cwd(&crate::paths::probe_dir().to_string_lossy()));
        let left = std::fs::read_dir(&tdir).map(|d| d.flatten().count()).unwrap_or(0);
        assert_eq!(left, 0, "quedan ficheros en {}", tdir.display());
        assert!(t.capture(&crate::tmux::Host::Legacy(super::PROBE_SESSION.into())).is_none());
        let probe = crate::paths::probe_dir();
        let regs = std::fs::read_dir(crate::paths::sessions_dir()).unwrap().flatten().filter_map(|e| std::fs::read_to_string(e.path()).ok());
        let mut stale = 0;
        for r in regs {
            if r.contains(&format!("\"cwd\":\"{}\"", probe.display())) {
                let pid: u32 = serde_json::from_str::<serde_json::Value>(&r).unwrap()["pid"].as_u64().unwrap() as u32;
                if std::path::Path::new(&format!("/proc/{pid}")).exists() {
                    stale += 1;
                }
            }
        }
        assert_eq!(stale, 0, "la sesión oculta sigue viva");
    }

    #[test]
    fn effort_levels() {
        let sc = "   Effort\n         Faster                             Smarter\n         ────────────────────▲─────────────────────      Ultracode  off\n         low     medium     high     xhigh      max      Tab to toggle\n";
        let (levels, cur) = super::effort_slider(sc).unwrap();
        assert_eq!(levels, vec!["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(levels[cur], "high");
    }

    #[test]
    fn modes_from_footer() {
        use super::detect_mode;
        assert_eq!(detect_mode("x\n  ⏵⏵ auto mode on (shift+tab to cycle)\n"), Some("auto"));
        assert_eq!(detect_mode("  ⏸ manual mode on\n\n"), Some("manual"));
        assert_eq!(detect_mode("⏵⏵ accept edits on (shift+tab to cycle)"), Some("acceptEdits"));
        assert_eq!(detect_mode("⏸ plan mode on (shift+tab to cycle)"), Some("plan"));
        assert_eq!(detect_mode("⏵⏵ bypass permissions on (shift+tab to cycle)"), Some("bypassPermissions"));
        assert_eq!(detect_mode("nada"), None);
    }

    /// CCDESK_TEST_CLIP=1 cargo test --release clip_real -- --nocapture (lee el portapapeles)
    #[test]
    fn clip_real() {
        if std::env::var("CCDESK_TEST_CLIP").is_err() {
            return;
        }
        match tauri::async_runtime::block_on(super::clipboard_image()) {
            Ok(p) => {
                let n = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                println!("guardada: {p} ({n} bytes)");
                let _ = std::fs::remove_file(&p);
            }
            Err(e) => println!("sin imagen: {e}"),
        }
    }

    /// CCDESK_TEST_URI=<uri> cargo test --release resolve_real -- --nocapture
    #[test]
    fn resolve_real() {
        let Ok(u) = std::env::var("CCDESK_TEST_URI") else { return };
        let r = tauri::async_runtime::block_on(super::resolve_paths(vec![u, "/tmp/a b.txt".into()]));
        println!("{r:?}");
    }

    /// Lo lanzado con `spawn_detached` se recoge: no queda ningún hijo zombi.
    #[test]
    fn detached_children_are_reaped() {
        for _ in 0..3 {
            super::spawn_detached("true", "x").unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
        let me = std::process::id();
        let zombies = std::fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path().join("stat")).ok())
            .filter(|st| {
                let after = &st[st.rfind(')').unwrap_or(0) + 1..];
                let mut f = after.split_whitespace();
                let state = f.next().unwrap_or("");
                let ppid: u32 = f.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                ppid == me && state == "Z"
            })
            .count();
        assert_eq!(zombies, 0);
    }

    #[test]
    fn suggestion_from_prompt_box() {
        use super::detect_suggestion;
        let rule = "\x1b[38;5;244m────────────────────────────────────────";
        let foot = "\x1b[39m  \x1b[34m~\x1b[38;5;246m   Opus 5.5\n  \x1b[38;5;220m⏵⏵ auto mode on\x1b[39m";
        let scr = |input: &str| format!("\x1b[38;5;246m✻ Worked for 1m\x1b[39m\n\n{rule} todo ─\n{input}\n{rule}\n{foot}\n");
        // Captura real de una sesión en espera.
        let real = "\x1b[39m❯\u{a0}\x1b[2mcrea otra hija con el segundo punto urgente\x1b[0m";
        assert_eq!(detect_suggestion(&scr(real)).as_deref(), Some("crea otra hija con el segundo punto urgente"));
        // Con el cursor (invertido) encima de la primera letra.
        let cur = "❯\u{a0}\x1b[7m\x1b[2mc\x1b[27mrea otra\x1b[0m";
        assert_eq!(detect_suggestion(&scr(cur)).as_deref(), Some("crea otra"));
        // Caja vacía, texto escrito por el usuario y ejemplo de sesión nueva: nada.
        assert_eq!(detect_suggestion(&scr("\x1b[38;5;246m❯\u{a0}\x1b[39m")), None);
        assert_eq!(detect_suggestion(&scr("❯\u{a0}hola\x1b[7m \x1b[27m")), None);
        assert_eq!(detect_suggestion(&scr("❯\u{a0}\x1b[2mTry \"fix lint errors\"\x1b[0m")), None);
        // Partida en dos líneas por la propia TUI.
        let two = format!("❯\u{a0}\x1b[2mrevisa los tests\x1b[0m\n  \x1b[2mque fallan\x1b[0m");
        assert_eq!(detect_suggestion(&scr(&two)).as_deref(), Some("revisa los tests que fallan"));
        // Sin caja (un selector abierto): nada.
        assert_eq!(detect_suggestion("  Select model\n   ❯ 4.  Sonnet 5.5\n"), None);
    }

    /// CCDESK_TEST_SCREEN=<fichero de `tmux capture-pane -p -e`> cargo test screen -- --nocapture
    #[test]
    fn screen() {
        let Ok(f) = std::env::var("CCDESK_TEST_SCREEN") else { return };
        let s = std::fs::read_to_string(f).unwrap();
        println!("sugerencia: {:?}", super::detect_suggestion(&s));
    }

    #[test]
    fn scan_running_agents() {
        let d = std::env::temp_dir().join(format!("ccdesk-agents-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let w = |n: &str, s: &str| std::fs::write(d.join(n), s).unwrap();
        w("agent-a1.meta.json", r#"{"agentType":"general-purpose","description":"En marcha","toolUseId":"tu1","spawnDepth":1}"#);
        w("agent-a1.jsonl", "{}\n");
        w("agent-a2.meta.json", r#"{"agentType":"web-fetch","description":"Terminado","toolUseId":"tu2","finished":true}"#);
        w("agent-a2.jsonl", "{}\n");
        w("agent-a3.meta.json", r#"{"description":"Sin transcript"}"#);
        let rows = super::scan_agents(&d);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].tool_use_id.as_str(), rows[0].agent_id.as_str()), ("tu1", "a1"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn files_exist() {
        let d = std::env::temp_dir().join(format!("ccdesk-files-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.md");
        std::fs::write(&f, "x").unwrap();
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        // Fichero 1, carpeta 2 (también con la barra final), lo que no existe 0.
        let slash = format!("{}/", s(&d));
        assert_eq!(super::files_exist(vec![s(&f), s(&d), slash, s(&d.join("no.md"))]), vec![1, 2, 2, 0]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn image_file() {
        let d = std::env::temp_dir().join(format!("ccdesk-img-file-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.PNG"), b"\x89PNG\r\n\x1a\n").unwrap();
        std::fs::write(d.join("a.txt"), "x").unwrap();
        let r = |name: &str| tauri::async_runtime::block_on(super::image_file(d.join(name).to_string_lossy().into_owned()));
        assert!(r("a.PNG").is_ok());
        assert_eq!(r("a.txt").err().unwrap(), "No es una imagen");
        assert!(r("no.png").err().unwrap().starts_with("Ya no existe"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn arrange() {
        let v = |key: &str, last: u64| super::SessionView {
            key: key.into(), sid: String::new(), cwd: String::new(), title: key.into(), status: "idle".into(),
            managed: true, external_kind: None, pinned: false, exempt: false, started_at: 0, last_activity: last,
            stopped_at: None, pid: None, tmux_pane: None, dupes: vec![], location: None, notes: 0, agents: vec![], background: false,
        };
        let views = || std::collections::HashMap::from([("p1", 10), ("p2", 99), ("a", 5), ("b", 50), ("c", 50), ("d", 70)]
            .map(|(k, t)| (k.to_string(), v(k, t))));
        let s = |x: &[&str]| x.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        let keys = |r: Vec<super::SessionView>| r.into_iter().map(|v| v.key).collect::<Vec<_>>();
        let (pinned, order) = (s(&["p1", "p2"]), s(&["a", "b", "c", "d"]));
        assert_eq!(keys(super::arrange(&mut views(), &pinned, &order, false)), s(&["p1", "p2", "a", "b", "c", "d"]));
        // Recientes: las fijadas, en su orden (aunque p2 sea más reciente); el resto por actividad, y
        // con empate (b, c) manda el orden manual.
        assert_eq!(keys(super::arrange(&mut views(), &pinned, &order, true)), s(&["p1", "p2", "d", "b", "c", "a"]));
    }

    #[test]
    fn input_ready() {
        let rule = "─".repeat(40);
        let repl = format!("● hola\n\n{rule}\n❯\u{a0}\n{rule}\n  ~/ws/ccdesk Opus 5.5\n  ⏵⏵ auto mode on (shift+tab to cycle)\n");
        assert!(super::input_ready(&repl));
        assert!(super::input_ready(&format!("{rule} todo ─\n❯\u{a0}algo escrito\n{rule}\n")));
        // Diálogo de confianza: su ❯ no va bajo una regla.
        let trust = "────────────\n Accessing workspace:\n\n /home/wzk/ws/md-live\n\n ❯ No, exit\n   Yes, I trust this folder\n\n Enter to confirm · Esc to cancel\n";
        assert!(!super::input_ready(trust));
        assert!(!super::input_ready(""));
    }

    #[test]
    fn startup_prompt() {
        // Diálogo de confianza de Claude Code 2.1.317 en una carpeta nueva (pane capturado).
        let trust = "────────────────────────────────────────\n Accessing workspace:\n\n /home/wzk/ws/md-live\n\n Quick safety check: Is this a project you created or one you trust?\n\n Claude Code'll be able to read, edit, and execute files here.\n\n Security guide\n\n ❯ No, exit\n   Yes, I trust this folder\n\n Enter to confirm · Esc to cancel\n\n\n";
        assert!(super::startup_prompt(trust));
        assert!(super::startup_prompt(" Press Enter to continue…\n"));
        // Conversación reanudada que menciona el pie más arriba: abajo están la caja y el pie normal.
        let resumed = " ● Pulsa Enter to confirm · Esc to cancel en el diálogo.\n\n───────\n❯ \n───────\n  ⏵⏵ bypass permissions on (shift+tab to cycle)\n";
        assert!(!super::startup_prompt(resumed));
        assert!(!super::startup_prompt(""));
    }

    /// CCDESK_TEST_HISTORY=<cwd> cargo test history -- --nocapture
    #[test]
    fn history() {
        let Ok(cwd) = std::env::var("CCDESK_TEST_HISTORY") else { return };
        let h = tauri::async_runtime::block_on(super::prompt_history(cwd, 5)).unwrap();
        for (i, p) in h.iter().enumerate() {
            println!("{i}: {}", p.chars().take(70).collect::<String>().replace('\n', "⏎"));
        }
        assert!(!h.is_empty());
    }
}
