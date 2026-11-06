//! Sesiones de Claude en tmux.
//!
//! Disposición actual: servidor tmux por defecto, una sesión (nombre configurable, «claude»
//! por defecto) con **una ventana por sesión de Claude**. Así `tmux a -t claude` desde
//! cualquier terminal (o por SSH) da exactamente las mismas sesiones que el cliente. La
//! ventana guarda su clave en la opción `@ccdesk_key`.
//!
//! La terminal integrada de la app usa una sesión agrupada propia (`<nombre>-gui`): comparte
//! las ventanas pero tiene su propia ventana activa, para no mover la vista de quien esté en SSH.
//!
//! Disposición antigua (sólo para sesiones que siguen vivas allí): servidor `-L cc` con una
//! sesión tmux `cc-*` por cada una.
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};

pub const LEGACY_SOCKET: &str = "cc";
pub const PREFIX: &str = "cc-";
pub const DEFAULT_SESSION: &str = "claude";
const KEY_OPT: &str = "@ccdesk_key";
/// Claude Code trata como «pegado» un bloque grande que llega de golpe: se teclea troceado.
/// Pegados de como mucho esto: desde unos 1000 caracteres Claude Code los pliega como
/// «[Pasted text #n]».
const PASTE_CHUNK: usize = 400;

/// Extensiones con las que Claude Code toma un pegado por ruta de imagen (`\.(png|jpe?g|gif|webp)$`).
const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// Variables heredadas que no deben llegar a las sesiones (anidamiento).
const STRIP_ENV: &[&str] = &[
    "TMUX",
    "TMUX_PANE",
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_TEAMMATE_MODE",
    "AI_AGENT",
];

const PASS_ENV: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "SSH_AUTH_SOCK",
    "PATH",
    "LANG",
];

pub fn strip_env_keys() -> Vec<String> {
    std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|k| STRIP_ENV.contains(&k.as_str()) || k.starts_with("CLAUDE_CODE_"))
        .collect()
}

pub fn clean_env(cmd: &mut Command) {
    for k in strip_env_keys() {
        cmd.env_remove(k);
    }
}

pub fn is_shell(cmd: &str) -> bool {
    matches!(cmd, "bash" | "zsh" | "sh" | "dash" | "fish" | "ksh" | "tcsh")
}

/// Dónde vive una sesión gestionada.
#[derive(Debug, Clone, PartialEq)]
pub enum Host {
    /// Sesión tmux `cc-*` del servidor antiguo `-L cc`.
    Legacy(String),
    /// Ventana (`@N`) de la sesión de ccdesk en el servidor por defecto.
    Window(String),
}

#[derive(Debug, Clone)]
pub struct TWin {
    pub host: Host,
    pub key: Option<String>,
    pub index: Option<u32>,
    pub name: String,
    pub pane_pid: u32,
    pub created: u64,
    pub path: String,
    pub command: String,
}

#[derive(Clone)]
pub struct Tmux {
    /// tmux.conf del servidor antiguo.
    pub conf: PathBuf,
    /// Nombre de la sesión tmux (preferencia).
    session: Arc<RwLock<String>>,
}

/// Nombre válido para una sesión tmux (sin `:` ni `.`, que tmux usa en los destinos).
pub fn valid_session_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 40 && !n.contains([':', '.', ' ', '\t'])
}

fn target(h: &Host) -> String {
    match h {
        Host::Legacy(n) => format!("={n}:"),
        Host::Window(id) => id.clone(),
    }
}

/// Trozos de `text` para `send-keys -l` que tmux no altera. Un argumento que acaba en `;` lo toma
/// por separador de órdenes y se come el `;` («uno;» se tecleaba «uno»: a una línea de código le
/// faltaba el `;` final). Lo de delante de los `;` finales va tal cual y cada `;`, aparte y escapado.
fn literal_pieces(text: &str) -> Vec<&str> {
    let body = text.trim_end_matches(';');
    let mut v = Vec::new();
    if !body.is_empty() {
        v.push(body);
    }
    v.extend(std::iter::repeat_n("\\;", text.len() - body.len()));
    v
}

/// Trozos en que se pega una línea: de como mucho `PASTE_CHUNK` caracteres y cortada antes de la
/// última letra de cada extensión de imagen. Si un pegado son sólo rutas y alguna acaba en .png,
/// .jpg…, Claude Code intenta abrirla como imagen y, si no lo es, inserta el texto más tarde, donde
/// esté entonces el cursor (lo pegado después queda delante). Sin ESC ni CR: dentro de un pegado
/// entre corchetes, un ESC podría cerrarlo antes de tiempo.
fn paste_pieces(line: &str) -> Vec<String> {
    let line: String = line.chars().filter(|&c| c != '\x1b' && c != '\r').collect();
    // Índices de byte: las extensiones son ASCII y el paso a minúsculas no mueve nada.
    let lower = line.to_ascii_lowercase();
    let cuts = lower.match_indices('.').filter_map(|(i, _)| {
        IMAGE_EXTS.iter().find(|e| lower[i + 1..].starts_with(*e)).map(|e| i + e.len())
    });
    let mut out = Vec::new();
    let mut start = 0;
    for end in cuts.chain([line.len()]) {
        let chars: Vec<char> = line[start..end].chars().collect();
        out.extend(chars.chunks(PASTE_CHUNK).map(|c| c.iter().collect::<String>()));
        start = end;
    }
    out
}

/// Orden de tmux que pega su entrada estándar en `target` (pegado entre corchetes, `-p`) a través
/// del búfer `buf`, que luego borra (`-d`).
fn paste_cmd<'a>(buf: &'a str, target: &'a str) -> Vec<&'a str> {
    vec!["load-buffer", "-b", buf, "-", ";", "paste-buffer", "-p", "-d", "-b", buf, "-t", target]
}

fn output(mut c: Command) -> Result<String, String> {
    let out = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("tmux: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

impl Tmux {
    pub fn new(conf: PathBuf, session: &str) -> Self {
        let name = if valid_session_name(session) { session } else { DEFAULT_SESSION };
        Tmux { conf, session: Arc::new(RwLock::new(name.to_string())) }
    }

    pub fn session(&self) -> String {
        self.session.read().unwrap().clone()
    }

    fn view(&self) -> String {
        format!("{}-gui", self.session())
    }

    /// Cambia el nombre de la sesión; si la antigua existe (y la nueva no), la renombra en tmux.
    pub fn set_session(&self, name: &str) -> Result<(), String> {
        if !valid_session_name(name) {
            return Err("Nombre de sesión no válido (sin espacios, «:» ni «.»)".into());
        }
        let old = self.session();
        if old == name {
            return Ok(());
        }
        let exists = |n: &str| self.run(false, &["has-session", "-t", &format!("={n}")]).is_ok();
        if exists(&old) {
            if exists(name) {
                return Err(format!("Ya existe una sesión tmux «{name}»"));
            }
            self.run(false, &["rename-session", "-t", &format!("={old}"), name])?;
            // La sesión agrupada de la terminal integrada sigue al nuevo nombre: si se matara, la
            // terminal que estuviera abierta se quedaría muerta.
            let _ = self.run(false, &["rename-session", "-t", &format!("={old}-gui"), &format!("{name}-gui")]);
        }
        *self.session.write().unwrap() = name.to_string();
        Ok(())
    }

    fn base(&self, legacy: bool) -> Command {
        let mut c = Command::new("tmux");
        if legacy {
            c.arg("-L").arg(LEGACY_SOCKET);
            if self.conf.exists() {
                c.arg("-f").arg(&self.conf);
            }
        }
        clean_env(&mut c);
        c.stdin(Stdio::null());
        c
    }

    fn run(&self, legacy: bool, args: &[&str]) -> Result<String, String> {
        let mut c = self.base(legacy);
        c.args(args);
        output(c)
    }

    fn run_on(&self, h: &Host, args: &[&str]) -> Result<String, String> {
        self.run(matches!(h, Host::Legacy(_)), args)
    }

    /// Como `run_on`, con `input` por la entrada estándar de tmux.
    fn run_on_input(&self, h: &Host, input: &str, args: &[&str]) -> Result<(), String> {
        use std::io::Write;
        let mut c = self.base(matches!(h, Host::Legacy(_)));
        c.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| format!("tmux: {e}"))?;
        // Al soltarla, tmux ve el final de la entrada.
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.as_bytes()).map_err(|e| format!("tmux: {e}"))?;
        }
        let out = child.wait_with_output().map_err(|e| format!("tmux: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    fn session_exists(&self) -> bool {
        self.run(false, &["has-session", "-t", &format!("={}", self.session())]).is_ok()
    }

    /// Sesiones vivas: ventanas de la sesión de ccdesk y sesiones antiguas `cc-*`.
    pub fn list(&self) -> Vec<TWin> {
        let mut out = Vec::new();
        let fmt = "#{window_id}\t#{window_index}\t#{@ccdesk_key}\t#{pane_pid}\t#{window_activity}\t#{pane_current_path}\t#{pane_current_command}\t#{window_name}";
        if let Ok(s) = self.run(false, &["list-windows", "-t", &format!("={}", self.session()), "-F", fmt]) {
            for l in s.lines() {
                let f: Vec<&str> = l.split('\t').collect();
                if f.len() < 8 {
                    continue;
                }
                let Ok(pane_pid) = f[3].parse() else { continue };
                out.push(TWin {
                    host: Host::Window(f[0].to_string()),
                    key: Some(f[2].to_string()).filter(|k| !k.is_empty()),
                    index: f[1].parse().ok(),
                    name: f[7].to_string(),
                    pane_pid,
                    created: f[4].parse().unwrap_or(0),
                    path: f[5].to_string(),
                    command: f[6].to_string(),
                });
            }
        }
        let fmt = "#{session_name}\t#{pane_pid}\t#{session_created}\t#{pane_current_path}\t#{pane_current_command}";
        if let Ok(s) = self.run(true, &["list-sessions", "-F", fmt]) {
            for l in s.lines() {
                let f: Vec<&str> = l.split('\t').collect();
                if f.len() < 5 || !f[0].starts_with(PREFIX) {
                    continue;
                }
                let Ok(pane_pid) = f[1].parse() else { continue };
                out.push(TWin {
                    host: Host::Legacy(f[0].to_string()),
                    key: Some(f[0].to_string()),
                    index: None,
                    name: f[0].to_string(),
                    pane_pid,
                    created: f[2].parse().unwrap_or(0),
                    path: f[3].to_string(),
                    command: f[4].to_string(),
                });
            }
        }
        out
    }

    pub fn exists(&self, h: &Host) -> bool {
        match h {
            Host::Legacy(n) => self.run(true, &["has-session", "-t", &format!("={n}")]).is_ok(),
            Host::Window(id) => self
                .run(false, &["display-message", "-p", "-t", id, "#{window_id}"])
                .is_ok_and(|s| s.trim() == id),
        }
    }

    /// Texto visible del pane (para leer el pie de Claude Code: modo de permisos).
    pub fn capture(&self, h: &Host) -> Option<String> {
        self.run_on(h, &["capture-pane", "-p", "-t", &target(h)]).ok()
    }

    /// Como `capture`, pero con los atributos de color (para distinguir el texto atenuado).
    pub fn capture_ansi(&self, h: &Host) -> Option<String> {
        self.run_on(h, &["capture-pane", "-p", "-e", "-t", &target(h)]).ok()
    }

    pub fn pane_command(&self, h: &Host) -> Option<String> {
        self.run_on(h, &["display-message", "-p", "-t", &target(h), "#{pane_current_command}"])
            .ok()
            .map(|s| s.trim().to_string())
    }

    fn mark(&self, id: &str, key: &str, name: &str) {
        let _ = self.run(false, &["set-option", "-w", "-t", id, KEY_OPT, key]);
        let _ = self.run(false, &["set-option", "-w", "-t", id, "automatic-rename", "off"]);
        let _ = self.run(false, &["set-option", "-w", "-t", id, "allow-rename", "off"]);
        let _ = self.run(false, &["rename-window", "-t", id, name]);
    }

    pub fn set_key(&self, h: &Host, key: &str) {
        if let Host::Window(id) = h {
            let _ = self.run(false, &["set-option", "-w", "-t", id, KEY_OPT, key]);
        }
    }

    /// Desvincula la ventana de ccdesk (queda como una ventana normal de la sesión).
    pub fn unset_key(&self, h: &Host) {
        if let Host::Window(id) = h {
            let _ = self.run(false, &["set-option", "-w", "-u", "-t", id, KEY_OPT]);
        }
    }

    pub fn rename(&self, h: &Host, name: &str) {
        if let Host::Window(id) = h {
            let _ = self.run(false, &["rename-window", "-t", id, name]);
        }
    }

    /// Nueva ventana en la sesión de ccdesk ejecutando `argv` directamente (sin shell): el pid
    /// del pane es el de `claude` y la ventana se cierra sola cuando termina.
    pub fn new_window(&self, key: &str, name: &str, cwd: &str, argv: &[String]) -> Result<Host, String> {
        let session = self.session();
        let mut c = self.base(false);
        if self.session_exists() {
            c.args(["new-window", "-d", "-t", &format!("={session}:"), "-n", name, "-c", cwd]);
        } else {
            c.args(["new-session", "-d", "-s", &session, "-n", name, "-c", cwd, "-x", "200", "-y", "50"]);
        }
        c.args(["-P", "-F", "#{window_id}"]);
        for k in PASS_ENV {
            if let Ok(v) = std::env::var(k) {
                c.arg("-e").arg(format!("{k}={v}"));
            }
        }
        c.arg("--").args(argv);
        let id = output(c)?.trim().to_string();
        if !id.starts_with('@') {
            return Err(format!("tmux no devolvió la ventana ({id})"));
        }
        self.mark(&id, key, name);
        Ok(Host::Window(id))
    }

    /// Sesión oculta en el servidor antiguo (`-L cc`) ejecutando `argv`: como su nombre no empieza
    /// por `cc-`, no sale en la lista de sesiones. Sirve para consultar a Claude Code sin tocar
    /// ninguna conversación del usuario. Si ya existía (de una consulta interrumpida), se rehace.
    pub fn spawn_hidden(&self, name: &str, cwd: &str, argv: &[String]) -> Result<Host, String> {
        debug_assert!(!name.starts_with(PREFIX));
        let _ = self.run(true, &["kill-session", "-t", &format!("={name}")]);
        let mut c = self.base(true);
        c.args(["new-session", "-d", "-s", name, "-x", "200", "-y", "50", "-c", cwd]);
        for k in PASS_ENV {
            if let Ok(v) = std::env::var(k) {
                c.arg("-e").arg(format!("{k}={v}"));
            }
        }
        c.arg("--").args(argv);
        output(c)?;
        Ok(Host::Legacy(name.to_string()))
    }

    /// Pid del proceso del pane (el propio `claude` en las sesiones que lo ejecutan directamente).
    pub fn pane_pid(&self, h: &Host) -> Option<u32> {
        self.run_on(h, &["display-message", "-p", "-t", &target(h), "#{pane_pid}"]).ok()?.trim().parse().ok()
    }

    /// Mueve a la sesión de ccdesk la ventana del servidor por defecto que contiene `pane`
    /// (adoptar sin reiniciar el proceso). Devuelve None si el pane no existe.
    pub fn adopt_pane(&self, pane: &str, key: &str, name: &str) -> Result<Host, String> {
        let id = self
            .run(false, &["display-message", "-p", "-t", pane, "#{window_id}"])?
            .trim()
            .to_string();
        if !id.starts_with('@') {
            return Err("no encuentro su ventana de tmux".into());
        }
        let session = self.session();
        let mut placeholder = None;
        if !self.session_exists() {
            let p = self.run(false, &["new-session", "-d", "-s", &session, "-P", "-F", "#{window_id}"])?;
            placeholder = Some(p.trim().to_string());
        }
        self.run(false, &["move-window", "-s", &id, "-t", &format!("={session}:")])?;
        if let Some(p) = placeholder {
            let _ = self.run(false, &["kill-window", "-t", &p]);
        }
        self.mark(&id, key, name);
        Ok(Host::Window(id))
    }

    pub fn kill(&self, h: &Host) -> Result<(), String> {
        match h {
            Host::Legacy(n) => self.run(true, &["kill-session", "-t", &format!("={n}")]).map(|_| ()),
            Host::Window(id) => self.run(false, &["kill-window", "-t", id]).map(|_| ()),
        }
    }

    pub fn send_keys(&self, h: &Host, keys: &[&str]) -> Result<(), String> {
        let t = target(h);
        let mut args = vec!["send-keys", "-t", t.as_str()];
        args.extend_from_slice(keys);
        self.run_on(h, &args).map(|_| ())
    }

    /// Teclea `text` literalmente.
    pub fn send_literal(&self, h: &Host, text: &str) -> Result<(), String> {
        let t = target(h);
        for piece in literal_pieces(text) {
            self.run_on(h, &["send-keys", "-t", &t, "-l", "--", piece])?;
        }
        Ok(())
    }

    /// Escribe un texto de varias líneas en la caja de Claude Code: cada línea pegada (pegado entre
    /// corchetes, en trozos: `paste_pieces`) y Ctrl+J entre líneas, que lo inserta sin enviar.
    /// Tecleado con `send-keys -l`, Claude Code lo desordenaba: le llega a trozos y, si uno largo y
    /// uno corto llegan casi a la vez, inserta el largo con retraso y el corto se queda detrás del
    /// cursor, al final de todo («…26.11.2.» y, tres líneas más abajo, «zip»). Pegado de una vez, lo
    /// plegaría como «[Pasted text #1 +7 lines]».
    pub fn type_text(&self, h: &Host, text: &str) -> Result<(), String> {
        for (k, line) in text.split('\n').enumerate() {
            if k > 0 {
                self.send_keys(h, &["C-j"])?;
            }
            for piece in paste_pieces(line) {
                self.paste(h, &piece)?;
            }
        }
        Ok(())
    }

    /// Pega `text` en el pane como pegado entre corchetes. Va por la entrada estándar: como
    /// argumento, tmux se comería un `;` final.
    fn paste(&self, h: &Host, text: &str) -> Result<(), String> {
        let buf = format!("ccdesk-{}", std::process::id());
        let t = target(h);
        let r = self.run_on_input(h, text, &paste_cmd(&buf, &t));
        if r.is_err() {
            // Que no quede en la lista de búferes de tmux, a mano de un prefijo+].
            let _ = self.run_on(h, &["delete-buffer", "-b", &buf]);
        }
        r
    }

    /// Orden para la terminal integrada (se ejecuta dentro de un pty).
    pub fn attach_argv(&self, h: &Host) -> Vec<String> {
        let s = |x: &str| x.to_string();
        match h {
            Host::Legacy(n) => {
                let mut v = vec![s("tmux"), s("-L"), s(LEGACY_SOCKET)];
                if self.conf.exists() {
                    v.extend([s("-f"), self.conf.to_string_lossy().into_owned()]);
                }
                v.extend([s("attach-session"), s("-t"), format!("={n}")]);
                v
            }
            Host::Window(id) => {
                // Sesión agrupada efímera: misma lista de ventanas, ventana activa propia.
                let (session, view) = (self.session(), self.view());
                let _ = self.run(false, &["kill-session", "-t", &format!("={view}")]);
                vec![
                    s("tmux"), s("new-session"), s("-t"), session, s("-s"), view.clone(),
                    s(";"), s("set-option"), s("-t"), view.clone(), s("destroy-unattached"), s("on"),
                    s(";"), s("set-option"), s("-t"), view.clone(), s("status"), s("off"),
                    s(";"), s("set-option"), s("-t"), view.clone(), s("mouse"), s("on"),
                    s(";"), s("select-window"), s("-t"), format!("={view}:{id}"),
                ]
            }
        }
    }
}

/// pid del proceso del pane y su ventana en el servidor por defecto.
pub fn pane_info(pane: &str) -> Option<(u32, String)> {
    let mut c = Command::new("tmux");
    clean_env(&mut c);
    c.stdin(Stdio::null());
    c.args(["display-message", "-p", "-t", pane, "#{pane_pid}\t#{window_id}"]);
    let s = output(c).ok()?;
    let (pid, win) = s.trim().split_once('\t')?;
    Some((pid.parse().ok()?, win.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_pieces_trailing_semicolons() {
        assert_eq!(literal_pieces("uno;"), ["uno", r"\;"]);
        assert_eq!(literal_pieces("a; b"), ["a; b"]);
        assert_eq!(literal_pieces("i;;"), ["i", r"\;", r"\;"]);
        assert_eq!(literal_pieces(";"), [r"\;"]);
        assert_eq!(literal_pieces(r"x\;"), [r"x\", r"\;"]);
        assert!(literal_pieces("").is_empty());
    }

    /// Servidor tmux propio y aislado para las pruebas: siempre con `-L` (con `$TMUX` puesto, sin
    /// él se hablaría con el servidor de quien corre los tests) y sin `TMUX` en el entorno.
    struct Server(String);
    impl Server {
        fn new(tag: &str) -> Self {
            Server(format!("ccdesk-test-{tag}-{}", std::process::id()))
        }
        fn tm(&self, args: &[&str]) -> Result<String, String> {
            let mut c = Command::new("tmux");
            clean_env(&mut c);
            c.stdin(Stdio::null()).args(["-L", &self.0, "-f", "/dev/null"]).args(args);
            output(c)
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            // tmux deja el fichero del socket al cerrar el servidor.
            let sock = self.tm(&["display-message", "-p", "#{socket_path}"]);
            let _ = self.tm(&["kill-server"]);
            if let Ok(p) = sock {
                let _ = std::fs::remove_file(p.trim());
            }
        }
    }
    fn wait(ok: &dyn Fn() -> bool) {
        for _ in 0..60 {
            if ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Lo que Claude Code tomaría por ruta(s) de imagen (y abriría aparte, con retraso).
    fn claude_image_path(s: &str) -> bool {
        let t = s.trim();
        let t = ['"', '\''].iter().find_map(|q| t.strip_prefix(*q).and_then(|x| x.strip_suffix(*q))).unwrap_or(t);
        let low = t.to_ascii_lowercase();
        IMAGE_EXTS.iter().any(|e| low.ends_with(&format!(".{e}")))
    }

    #[test]
    fn paste_pieces_cuts() {
        let p = |s: &str| paste_pieces(s);
        assert_eq!(p("hola, ¿qué tal?"), ["hola, ¿qué tal?"]);
        assert!(p("").is_empty());
        assert_eq!(p("/home/w/.cache/ccdesk/pegados/pegado-1.png"), ["/home/w/.cache/ccdesk/pegados/pegado-1.pn", "g"]);
        assert_eq!(p("'/a/b.JPG'"), ["'/a/b.JP", "G'"]);
        assert_eq!(p("/a/1.png /a/2.jpeg"), ["/a/1.pn", "g /a/2.jpe", "g"]);
        assert_eq!(p("sin\x1b escapes\r"), ["sin escapes"]);
        let long = "ñandú ".repeat(170);
        let pieces = p(&long);
        assert_eq!(pieces.len(), 3);
        assert!(pieces.iter().all(|x| x.chars().count() <= PASTE_CHUNK));
        // Cortar no cambia el texto y ningún trozo parece una ruta de imagen.
        for s in [
            "mira esto: /home/w/x/pegado-17938.png",
            "\"/tmp/a b/foto.webp\"",
            "/x/a.png /x/b.gif /x/c.jpg",
            "fichero.PNG, otro.jpeg y nada más",
            "/no/es/imagen.md",
            &format!("{}.png", "x".repeat(PASTE_CHUNK - 2)),
        ] {
            let pieces = p(s);
            assert_eq!(pieces.concat(), s, "{s}");
            assert!(pieces.iter().all(|x| !claude_image_path(x) && !x.is_empty()), "{s}: {pieces:?}");
        }
    }

    /// Con un tmux de verdad: lo pegado llega tal cual, entre las marcas de pegado si la aplicación
    /// las ha pedido (como Claude Code).
    #[test]
    fn paste_real_tmux() {
        let sv = Server::new("paste");
        if sv.tm(&["-V"]).is_err() {
            return; // sin tmux
        }
        // `cat -vT` enseña los bytes de control y los no ASCII con letras: el ESC de las marcas
        // como «^[», el tabulador como «^I», los de la «ñ» como «M-…».
        let cat_v = |s: &str| -> String {
            let one = |b: u8| match b {
                0..=31 => format!("^{}", (b + 64) as char),
                127 => "^?".to_string(),
                _ => (b as char).to_string(),
            };
            s.bytes().map(|b| if b >= 128 { format!("M-{}", one(b - 128)) } else { one(b) }).collect()
        };
        sv.tm(&["new-session", "-d", "-s", "t", "-x", "200", "-y", "20", "printf '\\033[?2004h'; stty -echo; exec cat -vT"]).unwrap();
        wait(&|| sv.tm(&["display-message", "-p", "-t", "t", "#{pane_current_command}"]).is_ok_and(|c| c.trim() == "cat"));
        let id = sv.tm(&["display-message", "-p", "-t", "t", "#{window_id}"]).unwrap();
        let h = Host::Window(id.trim().to_string());
        let cases = ["uno;", "ñandú\tdos", r"tres\;", "let x = 1;"];
        for s in cases {
            // La misma orden que `Tmux::paste`, contra el servidor de prueba.
            let mut c = Command::new("tmux");
            clean_env(&mut c);
            c.args(["-L", &sv.0]).args(paste_cmd("b", &target(&h)));
            c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
            let mut child = c.spawn().unwrap();
            std::io::Write::write_all(&mut child.stdin.take().unwrap(), s.as_bytes()).unwrap();
            assert!(child.wait().unwrap().success());
            sv.tm(&["send-keys", "-t", "t", "Enter"]).unwrap();
        }
        let lines = || -> Vec<String> {
            let out = sv.tm(&["capture-pane", "-p", "-t", "t"]).unwrap_or_default();
            out.lines().filter(|l| !l.is_empty()).map(String::from).collect()
        };
        wait(&|| lines().len() >= cases.len());
        let want: Vec<String> = cases.iter().map(|s| format!("^[[200~{}^[[201~", cat_v(s))).collect();
        assert_eq!(lines(), want);
        // El búfer con nombre no se queda en tmux.
        assert_eq!(sv.tm(&["list-buffers"]).unwrap_or_default().trim(), "");
    }

    /// Con un tmux de verdad, en un servidor propio y aislado: lo tecleado llega tal cual.
    #[test]
    fn literal_pieces_real_tmux() {
        let sv = Server::new("literal");
        if sv.tm(&["-V"]).is_err() {
            return; // sin tmux
        }
        sv.tm(&["new-session", "-d", "-s", "t", "-x", "200", "-y", "20", "stty -echo; exec cat"]).unwrap();
        wait(&|| sv.tm(&["display-message", "-p", "-t", "t", "#{pane_current_command}"]).is_ok_and(|c| c.trim() == "cat"));
        let cases = ["uno;", r"dos\;", r"tres\\;", "i;;", ";", "a; b", r"c\", "let x = 1;", "ñandú;"];
        for s in cases {
            for p in literal_pieces(s) {
                sv.tm(&["send-keys", "-t", "t", "-l", "--", p]).unwrap();
            }
            sv.tm(&["send-keys", "-t", "t", "-l", "--", "|"]).unwrap();
            sv.tm(&["send-keys", "-t", "t", "Enter"]).unwrap();
        }
        let lines = || -> Vec<String> {
            let out = sv.tm(&["capture-pane", "-p", "-t", "t"]).unwrap_or_default();
            out.lines().filter(|l| !l.is_empty()).map(String::from).collect()
        };
        wait(&|| lines().len() >= cases.len());
        let want: Vec<String> = cases.iter().map(|s| format!("{s}|")).collect();
        assert_eq!(lines(), want);
    }
}
