//! Notificaciones de escritorio: Claude ha terminado (respuesta nueva) o reclama atención
//! (pregunta o permiso). Se lanzan con `notify-send` (libnotify); al pulsar una, ccdesk se pone
//! delante con esa sesión abierta. No se avisa de la sesión que se está mirando (ventana con el
//! foco y esa sesión abierta), y al mirarla se cierra el aviso que tuviera.
//!
//! Cada aviso con botón mantiene vivo su `notify-send` (y un hilo que lo escucha) hasta que se
//! cierra, y Cinnamon no los cierra solos: por eso se retiran en cuanto dejan de valer (la sesión
//! cambia de estado) y, en todo caso, pasado un tiempo.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::app::{transcript_of, Core, SessionView, Snapshot};
use crate::{paths, transcript};

const ATTENTION: [&str; 2] = ["waiting", "blocked"];
/// Un «ha terminado» espera a que el reposo dure esto: entre pasos de un mismo turno el estado
/// puede parpadear.
const SETTLE: Duration = Duration::from_millis(2000);
/// Tope de vida de un aviso.
const MAX_AGE: Duration = Duration::from_secs(30 * 60);
const BODY_MAX: usize = 220;

/// Aviso abierto de una sesión: su id de libnotify (cuando notify-send lo dice), el estado de la
/// sesión al mostrarlo y cuándo.
struct Shown {
    id: Option<u32>,
    status: String,
    at: Instant,
}

#[derive(Default)]
pub struct Notifier {
    prev: HashMap<String, String>,
    /// Sesiones que acaban de quedar en reposo, a la espera de confirmar que han terminado.
    pending: HashMap<String, Instant>,
    /// Aviso abierto por sesión, para sustituirlo o cerrarlo.
    shown: Arc<Mutex<HashMap<String, Shown>>>,
}

impl Notifier {
    pub fn observe(&mut self, app: &AppHandle, core: &Core, snap: &Snapshot) {
        let focused = app
            .get_webview_window("main")
            .and_then(|w| w.is_focused().ok())
            .unwrap_or(false);
        let active = core.active.lock().unwrap().clone();
        let (on_done, on_att) = (snap.settings.notify_done, snap.settings.notify_attention);
        let mut seen = HashSet::new();
        for s in &snap.sessions {
            seen.insert(s.key.as_str());
            let prev = self.prev.insert(s.key.clone(), s.status.clone());
            if focused && active.as_deref() == Some(s.key.as_str()) {
                self.pending.remove(&s.key);
                self.close(&s.key);
                continue;
            }
            // Un aviso deja de valer cuando la sesión cambia de estado (le has escrito desde otro
            // sitio, ha vuelto a trabajar…) y, en todo caso, pasado un tiempo.
            let stale = self
                .shown
                .lock()
                .unwrap()
                .get(&s.key)
                .is_some_and(|x| x.status != s.status || x.at.elapsed() > MAX_AGE);
            if stale {
                self.close(&s.key);
            }
            let Some(prev) = prev else { continue }; // primera vez que se ve: sin historia
            let now_att = ATTENTION.contains(&s.status.as_str());
            if now_att && !ATTENTION.contains(&prev.as_str()) {
                self.pending.remove(&s.key);
                if on_att {
                    self.attention(app, s);
                }
            } else if s.status != "idle" {
                self.pending.remove(&s.key);
            } else if prev == "busy" || ATTENTION.contains(&prev.as_str()) {
                self.pending.insert(s.key.clone(), Instant::now());
            }
            if s.status == "idle" && self.pending.get(&s.key).is_some_and(|t| t.elapsed() >= SETTLE) {
                self.pending.remove(&s.key);
                if on_done {
                    self.done(app, s);
                }
            }
        }
        self.prev.retain(|k, _| seen.contains(k.as_str()));
        self.pending.retain(|k, _| seen.contains(k.as_str()));
        // Sesiones que ya no están: su aviso tampoco.
        let gone: Vec<String> = self
            .shown
            .lock()
            .unwrap()
            .keys()
            .filter(|k| !seen.contains(k.as_str()))
            .cloned()
            .collect();
        for k in gone {
            self.close(&k);
        }
    }

    fn done(&self, app: &AppHandle, s: &SessionView) {
        let body = last_said(s, false).unwrap_or_else(|| "Espera tu siguiente mensaje.".into());
        self.show(app, s, &format!("{} — ha terminado", s.title), &body);
    }

    fn attention(&self, app: &AppHandle, s: &SessionView) {
        let (what, fallback) = if s.status == "blocked" {
            ("pide permiso", "Espera tu permiso para seguir.")
        } else {
            ("te pregunta", "Espera tu respuesta.")
        };
        let body = last_said(s, true).unwrap_or_else(|| fallback.into());
        self.show(app, s, &format!("{} — {what}", s.title), &body);
    }

    fn show(&self, app: &AppHandle, s: &SessionView, summary: &str, body: &str) {
        let shown = self.shown.clone();
        let key = s.key.clone();
        // Si ya tenía un aviso abierto, el nuevo lo sustituye (mismo id).
        let replace = shown.lock().unwrap().get(&key).and_then(|x| x.id);
        shown.lock().unwrap().insert(
            key.clone(),
            Shown { id: replace, status: s.status.clone(), at: Instant::now() },
        );
        let app = app.clone();
        // Con la salida en una tubería, notify-send no vuelca su id hasta acabar: `stdbuf -oL` lo
        // saca al momento (hace falta para sustituir o cerrar el aviso mientras está abierto).
        let mut cmd = if Path::new("/usr/bin/stdbuf").exists() {
            let mut c = Command::new("/usr/bin/stdbuf");
            c.args(["-oL", "notify-send"]);
            c
        } else {
            Command::new("notify-send")
        };
        cmd.args(["-a", "ccdesk", "-p", "-A", "default=Abrir", "-h", "string:desktop-entry:ccdesk"]);
        if let Some(icon) = icon_path() {
            cmd.arg("-i").arg(icon);
        }
        if let Some(id) = replace {
            cmd.arg("-r").arg(id.to_string());
        }
        cmd.arg("--").arg(summary).arg(markup_escape(body));
        std::thread::spawn(move || {
            let Ok(mut child) = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else {
                return;
            };
            // Primero imprime su id (-p); luego, si se pulsa, el nombre de la acción.
            let mut mine = None;
            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let l = line.trim();
                    if mine.is_none() {
                        if let Ok(id) = l.parse::<u32>() {
                            mine = Some(id);
                            match shown.lock().unwrap().get_mut(&key) {
                                Some(x) => x.id = Some(id),
                                // Se retiró antes de saber su id: se cierra ahora.
                                None => close_id(id),
                            }
                            continue;
                        }
                    }
                    if l == "default" {
                        focus(&app, &key);
                    }
                }
            }
            let _ = child.wait();
            if let Some(id) = mine {
                let mut m = shown.lock().unwrap();
                if m.get(&key).and_then(|x| x.id) == Some(id) {
                    m.remove(&key);
                }
            }
        });
    }

    /// Retira el aviso abierto de esa sesión, si lo hay.
    fn close(&self, key: &str) {
        let Some(x) = self.shown.lock().unwrap().remove(key) else { return };
        if let Some(id) = x.id {
            close_id(id);
        }
    }
}

/// Cierra un aviso por su id (libnotify avisa a su notify-send, que termina).
fn close_id(id: u32) {
    std::thread::spawn(move || {
        let _ = Command::new("gdbus")
            .args([
                "call", "--session",
                "--dest", "org.freedesktop.Notifications",
                "--object-path", "/org/freedesktop/Notifications",
                "--method", "org.freedesktop.Notifications.CloseNotification",
                &id.to_string(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

/// Trae la ventana delante y abre la sesión.
fn focus(app: &AppHandle, key: &str) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    let _ = app.emit("focus-session", key);
}

/// Lo último que dijo Claude, para el cuerpo del aviso. Al terminar, su último texto; al pedir
/// atención, su último bloque: la herramienta que quiere usar o la pregunta que hace.
fn last_said(s: &SessionView, attention: bool) -> Option<String> {
    let path = transcript_of(&s.sid, &s.cwd)?;
    last_block(&path, attention)
}

fn last_block(path: &Path, attention: bool) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(512 * 1024))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    for line in text.lines().rev() {
        if !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["type"] != "assistant" || v["isSidechain"] == true {
            continue;
        }
        let Some(content) = v["message"]["content"].as_array() else { continue };
        for b in content.iter().rev() {
            match b["type"].as_str() {
                Some("text") => {
                    let t = clean(b["text"].as_str().unwrap_or(""));
                    if !t.is_empty() {
                        return Some(t);
                    }
                }
                Some("tool_use") if attention => {
                    let name = b["name"].as_str().unwrap_or("");
                    let what = transcript::tool_summary(name, &b["input"]);
                    return Some(match name {
                        "AskUserQuestion" if !what.is_empty() => clean(&what),
                        _ if what.is_empty() => format!("Quiere usar {name}"),
                        _ => clean(&format!("Quiere usar {name}: {what}")),
                    });
                }
                _ => {}
            }
        }
    }
    None
}

/// Texto plano de una línea: sin marcas de Markdown y recortado.
fn clean(s: &str) -> String {
    let flat: String = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("**", "")
        .replace('`', "")
        .trim_start_matches(['#', ' '])
        .to_string();
    if flat.chars().count() <= BODY_MAX {
        return flat;
    }
    let cut: String = flat.chars().take(BODY_MAX).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a).trim_end_matches([',', ';', ':', '.']);
    format!("{cut}…")
}

/// El cuerpo admite marcado en muchos servidores de notificaciones (Cinnamon incluido).
fn markup_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Icono de la app en disco (notify-send pide una ruta); se escribe una vez en la caché.
fn icon_path() -> Option<PathBuf> {
    static ICON: OnceLock<Option<PathBuf>> = OnceLock::new();
    ICON.get_or_init(|| {
        let bytes: &[u8] = include_bytes!("../icons/128x128.png");
        let p = paths::home().join(".cache/ccdesk/icon.png");
        if std::fs::read(&p).ok().as_deref() != Some(bytes) {
            std::fs::create_dir_all(p.parent()?).ok()?;
            std::fs::write(&p, bytes).ok()?;
        }
        Some(p)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_text() {
        assert_eq!(clean("## Hecho\n\nHe **cambiado** `app.rs`."), "Hecho He cambiado app.rs.");
        let long = "palabra ".repeat(60);
        let c = clean(&long);
        assert!(c.ends_with('…') && c.chars().count() <= BODY_MAX + 1, "{c}");
        assert_eq!(markup_escape("a < b & c"), "a &lt; b &amp; c");
    }

    #[test]
    fn last_block_from_transcript() {
        let d = std::env::temp_dir().join(format!("ccdesk-notify-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("t.jsonl");
        let lines = [
            r#"{"type":"user","message":{"content":"hola"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Ya está: **listo**."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"rm x","description":"Borrar x"}}]}}"#,
        ];
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        assert_eq!(last_block(&p, false).as_deref(), Some("Ya está: listo."));
        assert_eq!(last_block(&p, true).as_deref(), Some("Quiere usar Bash: Borrar x"));
        let q = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"AskUserQuestion","input":{"questions":[{"question":"¿Qué tema prefieres?"}]}}]}}"#;
        std::fs::write(&p, format!("{q}\n")).unwrap();
        assert_eq!(last_block(&p, true).as_deref(), Some("¿Qué tema prefieres?"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
