//! Estado propio de la app (orden, pins, nombres, sesiones conocidas, ajustes).
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::paths;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub autostop: bool,
    pub autostop_minutes: u64,
    pub sidebar_width: u32,
    /// Sesión tmux (servidor por defecto) donde viven las sesiones: `tmux a -t <nombre>`.
    pub tmux_session: String,
    /// Panel derecho de to-dos visible.
    pub notes_open: bool,
    /// Modo YOLO: las sesiones arrancan con `--allow-dangerously-skip-permissions` y el modo
    /// bypass aparece en el desplegable.
    pub allow_yolo: bool,
    /// Tema de color: `system`, `light`, `dark` o `sepia`.
    pub theme: String,
    /// Zoom de la interfaz (Ctrl+rueda), 1.0 = 100 %.
    pub zoom: f64,
    /// Notificación de escritorio cuando Claude termina (respuesta nueva).
    pub notify_done: bool,
    /// Notificación de escritorio cuando Claude pregunta algo o pide permiso.
    pub notify_attention: bool,
    /// Enseñar en la conversación las imágenes que Claude mira o que se le pegan.
    pub show_media: bool,
    /// Orden de la lista de sesiones: `manual` (arrastrando) o `recent` (actividad más reciente
    /// arriba; las fijadas siguen en su orden manual).
    pub session_order: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autostop: false,
            autostop_minutes: 60,
            sidebar_width: 290,
            tmux_session: crate::tmux::DEFAULT_SESSION.to_string(),
            notes_open: true,
            allow_yolo: true,
            theme: "system".into(),
            zoom: 1.0,
            notify_done: true,
            notify_attention: true,
            show_media: true,
            session_order: "manual".into(),
        }
    }
}

/// Sesión gestionada (viva o detenida). La clave es el nombre de sesión tmux `cc-xxxxxxxx`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Known {
    pub sid: String,
    pub cwd: String,
    pub title: Option<String>,
    pub started_at: u64,
    pub stopped_at: Option<u64>,
    pub last_activity: u64,
    /// Desde cuándo la gestiona ccdesk (creada, adoptada o reanudada): la auto-detención no
    /// cuenta la inactividad anterior.
    pub managed_since: u64,
}

/// To-do: algo que lanzar a la sesión más adelante.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub text: String,
    pub created: u64,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Persist {
    pub version: u32,
    /// Orden manual de las no fijadas (arriba → abajo).
    pub order: Vec<String>,
    /// Fijadas, en su orden.
    pub pinned: Vec<String>,
    pub names: HashMap<String, String>,
    pub exempt: Vec<String>,
    pub known: HashMap<String, Known>,
    pub recent_cwds: Vec<String>,
    pub settings: Settings,
    /// To-dos por sesión (clave de la sesión).
    pub notes: HashMap<String, Vec<Note>>,
    /// Última sesión abierta (clave y session id, por si la clave cambia al adoptarla).
    pub last_active: Option<String>,
    pub last_active_sid: Option<String>,
}

impl Persist {
    pub fn touch_cwd(&mut self, cwd: &str) {
        self.recent_cwds.retain(|c| c != cwd);
        self.recent_cwds.insert(0, cwd.to_string());
        self.recent_cwds.truncate(30);
    }

    pub fn forget(&mut self, key: &str) {
        self.known.remove(key);
        self.order.retain(|k| k != key);
        self.pinned.retain(|k| k != key);
        self.names.remove(key);
        self.exempt.retain(|k| k != key);
        self.notes.remove(key);
    }
}

fn file() -> PathBuf {
    paths::config_dir().join("state.json")
}

pub fn load() -> Persist {
    let f = file();
    let Ok(s) = std::fs::read_to_string(&f) else {
        return Persist::default();
    };
    match serde_json::from_str(&s) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ccdesk: state.json ilegible ({e}); se aparta como .corrupt");
            let _ = std::fs::rename(&f, f.with_extension("json.corrupt"));
            Persist::default()
        }
    }
}

pub fn save(p: &Persist) {
    let f = file();
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = f.with_extension("json.tmp");
    if let Ok(s) = serde_json::to_string_pretty(p) {
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, &f);
        }
    }
}
