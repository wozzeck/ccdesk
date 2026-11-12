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
    /// Modo de permisos con el que arrancan las sesiones nuevas (`--permission-mode`). Vacío: el
    /// de la configuración de Claude Code.
    pub new_session_mode: String,
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
            new_session_mode: String::new(),
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
    /// Sesiones con algo sin leer (una respuesta nueva, o marcadas a mano), por clave: así
    /// siguen marcadas al volver a abrir la app.
    pub unread: Vec<String>,
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
        self.unread.retain(|k| k != key);
    }

    /// Funde la entrada `from` en `into` (la misma conversación con dos claves): `into` hereda el
    /// sitio de `from` en ancladas, orden y exentas si no lo tiene, el nombre puesto a mano si no
    /// tiene uno, sus to-dos y lo sin leer, y `from` se olvida.
    pub fn merge_key(&mut self, from: &str, into: &str) {
        if from == into {
            return;
        }
        for list in [&mut self.pinned, &mut self.order, &mut self.exempt] {
            if list.iter().any(|k| k == into) {
                list.retain(|k| k != from);
            } else if let Some(i) = list.iter().position(|k| k == from) {
                list[i] = into.to_string();
            }
        }
        if let Some(n) = self.names.remove(from) {
            self.names.entry(into.to_string()).or_insert(n);
        }
        if let Some(n) = self.notes.remove(from) {
            self.notes.entry(into.to_string()).or_default().extend(n);
        }
        if self.set_unread(from, false) {
            self.set_unread(into, true);
        }
        if self.last_active.as_deref() == Some(from) {
            self.last_active = Some(into.to_string());
        }
        self.known.remove(from);
    }

    /// Marca o desmarca «sin leer»; true si ha cambiado.
    pub fn set_unread(&mut self, key: &str, unread: bool) -> bool {
        if self.unread.iter().any(|k| k == key) == unread {
            return false;
        }
        if unread {
            self.unread.push(key.to_string());
        } else {
            self.unread.retain(|k| k != key);
        }
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_marks() {
        let mut p = Persist::default();
        assert!(p.set_unread("a", true));
        assert!(!p.set_unread("a", true)); // ya lo estaba
        assert!(p.set_unread("b", true));
        assert!(p.set_unread("a", false));
        assert!(!p.set_unread("a", false));
        assert_eq!(p.unread, ["b"]);
        p.forget("b");
        assert!(p.unread.is_empty());
        // Un state.json de antes, sin el campo, se sigue leyendo.
        let old: Persist = serde_json::from_str(r#"{"version":1,"order":["x"]}"#).unwrap();
        assert!(old.unread.is_empty() && old.order == ["x"]);
    }

    #[test]
    fn merge_keys() {
        let note = |t: &str| Note { id: t.into(), text: t.into(), created: 0 };
        let mut p = Persist::default();
        // «todo» anclada como cc-590b75fd y, duplicada al incorporarla con un id provisional, cc-8e1ac618.
        p.known.insert("cc-590b75fd".into(), Known { sid: "590b75fd".into(), ..Default::default() });
        p.known.insert("cc-8e1ac618".into(), Known { sid: "590b75fd".into(), ..Default::default() });
        p.pinned = vec!["cc-590b75fd".into()];
        p.order = vec!["cc-a".into(), "cc-8e1ac618".into()];
        p.exempt = vec!["cc-8e1ac618".into()];
        p.names.insert("cc-8e1ac618".into(), "provisional".into());
        p.notes.insert("cc-590b75fd".into(), vec![note("vieja")]);
        p.notes.insert("cc-8e1ac618".into(), vec![note("nueva")]);
        p.unread = vec!["cc-8e1ac618".into()];
        p.last_active = Some("cc-8e1ac618".into());
        p.merge_key("cc-8e1ac618", "cc-590b75fd");
        assert_eq!(p.known.keys().collect::<Vec<_>>(), ["cc-590b75fd"]);
        assert_eq!(p.pinned, ["cc-590b75fd"]);
        assert_eq!(p.order, ["cc-a", "cc-590b75fd"]); // anclada: `arrange` la pinta una vez
        assert_eq!(p.exempt, ["cc-590b75fd"]);
        assert_eq!(p.names.get("cc-590b75fd").map(String::as_str), Some("provisional"));
        assert_eq!(p.notes["cc-590b75fd"].iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["vieja", "nueva"]);
        assert_eq!(p.unread, ["cc-590b75fd"]);
        assert_eq!(p.last_active.as_deref(), Some("cc-590b75fd"));
        // El nombre que ya tuviera la que se queda no se pisa.
        p.known.insert("cc-x".into(), Known::default());
        p.names.insert("cc-x".into(), "otro".into());
        p.merge_key("cc-x", "cc-590b75fd");
        assert_eq!(p.names.get("cc-590b75fd").map(String::as_str), Some("provisional"));
        assert!(!p.names.contains_key("cc-x"));
    }
}
