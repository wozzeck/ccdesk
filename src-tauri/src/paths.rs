use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn claude_dir() -> PathBuf {
    home().join(".claude")
}

/// Registro oficial de procesos vivos de Claude Code: un `<pid>.json` por proceso.
pub fn sessions_dir() -> PathBuf {
    claude_dir().join("sessions")
}

pub fn projects_dir() -> PathBuf {
    claude_dir().join("projects")
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("ccdesk")
}

/// Carpeta de la sesión oculta con la que se leen las opciones de Claude Code (lista de modelos).
pub fn probe_dir() -> PathBuf {
    home().join(".cache/ccdesk/sonda")
}

/// Lista de modelos leída con la sesión oculta, con la versión de Claude Code que la dio.
pub fn models_cache() -> PathBuf {
    home().join(".cache/ccdesk/modelos.json")
}

/// Donde `statusline.sh` vuelca el JSON de cada sesión (ruta fija con $HOME).
pub fn status_dir() -> PathBuf {
    home().join(".cache/ccdesk/status")
}

pub fn tmux_conf() -> PathBuf {
    config_dir().join("tmux.conf")
}

fn is_exec(p: &Path) -> bool {
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Ruta absoluta del binario `claude` (CCDESK_CLAUDE > PATH > ~/.local/bin).
pub fn claude_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("CCDESK_CLAUDE") {
        return PathBuf::from(p);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let c = dir.join("claude");
            if is_exec(&c) {
                return c;
            }
        }
    }
    let c = home().join(".local/bin/claude");
    if is_exec(&c) {
        return c;
    }
    PathBuf::from("claude")
}

/// Expande `~` y normaliza; devuelve None si no es un directorio existente.
/// `~` y `~/…`, en la carpeta personal.
pub fn expand_home(input: &str) -> PathBuf {
    let t = input.trim();
    if t == "~" {
        home()
    } else if let Some(rest) = t.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(t)
    }
}

pub fn resolve_dir(input: &str) -> Option<PathBuf> {
    let t = input.trim();
    let p = if t == "~" {
        home()
    } else if let Some(rest) = t.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(t)
    };
    let p = std::fs::canonicalize(&p).ok()?;
    p.is_dir().then_some(p)
}

/// Codificación de cwd que usa Claude Code para `~/.claude/projects/<dir>`.
pub fn encode_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
