//! Lectura de `~/.claude/sessions/<pid>.json` (registro de procesos vivos de Claude Code).
use serde::Deserialize;

use crate::paths;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegEntry {
    pub pid: u32,
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub started_at: u64,
    #[serde(default)]
    pub proc_start: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub tmux: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub name_source: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub updated_at: Option<u64>,
    #[serde(default)]
    pub status_updated_at: Option<u64>,
}

/// Campo 22 de /proc/<pid>/stat (starttime), para descartar pids reciclados.
fn proc_start(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(19).map(|s| s.to_string())
}

fn ppid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(1)?.parse().ok()
}

/// El proceso y sus padres (hasta 5 niveles): claude cuelga de una shell, de wrappers…
pub fn ancestors(pid: u32) -> Vec<u32> {
    let mut out = vec![pid];
    let mut cur = pid;
    for _ in 0..5 {
        match ppid(cur) {
            Some(p) if p > 1 => {
                out.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    out
}

/// ¿`pid` es el proceso del pane o desciende de él (shell → claude, wrappers…)?
pub fn descends_from(pid: u32, pane_pid: u32) -> bool {
    ancestors(pid).contains(&pane_pid)
}

pub fn alive(pid: u32, expected_start: Option<&str>) -> bool {
    match proc_start(pid) {
        None => false,
        Some(s) => expected_start.is_none_or(|e| e == s),
    }
}

pub fn read_all() -> Vec<RegEntry> {
    let Ok(rd) = std::fs::read_dir(paths::sessions_dir()) else {
        return vec![];
    };
    let mut out: Vec<RegEntry> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| serde_json::from_str::<RegEntry>(&s).ok())
        .filter(|r| alive(r.pid, r.proc_start.as_deref()))
        // La sesión oculta con la que se lee la lista de modelos no es una conversación.
        .filter(|r| std::path::Path::new(&r.cwd) != paths::probe_dir())
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    out
}
