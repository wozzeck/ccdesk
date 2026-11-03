//! Lista para autocompletar `/` en la caja de texto, como el menú de la TUI de Claude Code:
//! sus comandos propios, los skills que tiene cargados la sesión y los comandos y skills
//! personales y del proyecto (`~/.claude/…`, `<proyecto>/.claude/…`).
//!
//! Los comandos propios no están en ningún fichero: se sacan del ejecutable de Claude Code
//! (cada uno es un objeto `{type:"local",name:"…",description:"…"}`) y se guardan en caché por
//! versión. Si cambia el formato y no sale nada, quedan los skills y los comandos de disco.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::paths;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SlashCmd {
    pub name: String,
    pub description: String,
    /// `builtin` | `skill` | `command`
    pub kind: String,
    /// Abre un diálogo en la terminal (selector, panel…) en vez de mandar algo a Claude.
    pub interactive: bool,
}

/// Internos o sin sentido aquí aunque el ejecutable los traiga.
const SKIP: &[&str] = &["heapdump", "pro-trial-expired", "rate-limit-options", "workflow-launch-exec"];

/// Descripciones de los que la calculan en tiempo de ejecución (en el ejecutable no van escritas).
fn fallback_desc(name: &str) -> &'static str {
    match name {
        "exit" => "Exit the REPL",
        "resume" => "Resume a previous conversation",
        "init" => "Initialize a new CLAUDE.md file with codebase documentation",
        "login" => "Sign in with your Anthropic account",
        "diff" => "View uncommitted changes and per-turn diffs",
        "fast" => "Toggle fast mode",
        "terminal-setup" => "Set up terminal key bindings",
        _ => "",
    }
}

pub fn list(claude: &Path, cwd: &str, session_skills: Vec<(String, String)>) -> Vec<SlashCmd> {
    let mut out: Vec<SlashCmd> = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |c: SlashCmd, out: &mut Vec<SlashCmd>| {
        if seen.insert(c.name.clone()) {
            out.push(c);
        }
    };
    for (name, description) in session_skills {
        push(SlashCmd { name, description, kind: "skill".into(), interactive: false }, &mut out);
    }
    let home = paths::claude_dir();
    let proj = Path::new(cwd).join(".claude");
    for base in [proj.join("skills"), home.join("skills")] {
        for c in disk_skills(&base) {
            push(c, &mut out);
        }
    }
    for base in [proj.join("commands"), home.join("commands")] {
        for c in disk_commands(&base) {
            push(c, &mut out);
        }
    }
    for c in builtins(claude) {
        push(c, &mut out);
    }
    out
}

// ─────────── comandos propios de Claude Code ───────────

/// Precarga (en segundo plano al arrancar) para que el primer `/` no espere al ejecutable.
pub fn warm(claude: &Path) {
    let _ = builtins(claude);
}

/// Comandos ya leídos: etiqueta del ejecutable (nombre, fecha y tamaño) y lista.
type Builtins = Mutex<Option<(String, Vec<SlashCmd>)>>;

fn builtins(claude: &Path) -> Vec<SlashCmd> {
    static MEM: OnceLock<Builtins> = OnceLock::new();
    let real = std::fs::canonicalize(claude).unwrap_or_else(|_| claude.to_path_buf());
    // Etiqueta del ejecutable: nombre (la versión, en la instalación nativa), fecha y tamaño, para
    // que la caché caduque al actualizar Claude Code aunque el nombre no cambie (npm: cli.js).
    let meta = std::fs::metadata(&real).ok();
    let tag: String = format!(
        "{}-{}-{}",
        real.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
        meta.as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs()),
        meta.as_ref().map_or(0, |m| m.len())
    )
    .chars()
    .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    .collect();
    let mem = MEM.get_or_init(|| Mutex::new(None));
    let mut g = mem.lock().unwrap();
    if let Some((t, v)) = g.as_ref() {
        if *t == tag {
            return v.clone();
        }
    }
    let cache = paths::home().join(format!(".cache/ccdesk/commands-{tag}.json"));
    let v = std::fs::read_to_string(&cache)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<SlashCmd>>(&s).ok())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            let v = scan_binary(&real);
            if !v.is_empty() && !tag.is_empty() {
                let dir = cache.parent().unwrap();
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(&cache, serde_json::to_string(&v).unwrap_or_default());
                // Las cachés de versiones anteriores ya no sirven.
                for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n.starts_with("commands-") && n.ends_with(".json") && e.path() != cache {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            v
        });
    *g = Some((tag, v.clone()));
    v
}

/// Recorre el ejecutable por trozos buscando `{type:"local|local-jsx|prompt",name:"…"`. Un objeto
/// que no cabe entero al final del trozo se deja para el siguiente (se arrastra desde su inicio).
fn scan_binary(path: &Path) -> Vec<SlashCmd> {
    const CHUNK: usize = 8 << 20;
    const OBJ: usize = 1500; // lo que se lee de cada objeto
    let Ok(mut f) = std::fs::File::open(path) else { return vec![] };
    let mut out: Vec<SlashCmd> = Vec::new();
    let mut data: Vec<u8> = Vec::with_capacity(CHUNK + OBJ);
    let mut tmp = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut tmp).unwrap_or(0);
        let eof = n == 0;
        data.extend_from_slice(&tmp[..n]);
        let mut cut = data.len().saturating_sub(8); // por si «{type:"» queda partido
        let mut i = 0;
        while let Some(pos) = find(&data[i..], b"{type:\"") {
            let at = i + pos;
            if !eof && at + OBJ > data.len() {
                cut = at;
                break;
            }
            if let Some(c) = parse_obj(&data[at..data.len().min(at + OBJ)]) {
                match out.iter_mut().find(|o| o.name == c.name) {
                    Some(old) if old.description.is_empty() => old.description = c.description,
                    Some(_) => {}
                    None => out.push(c),
                }
            }
            i = at + 7;
        }
        if eof {
            break;
        }
        data.drain(..cut);
    }
    for c in out.iter_mut() {
        if c.description.is_empty() {
            c.description = fallback_desc(&c.name).to_string();
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Un objeto de comando: tipo, nombre, descripción literal y si está oculto o desactivado.
fn parse_obj(b: &[u8]) -> Option<SlashCmd> {
    let s = std::str::from_utf8(&b[..b.len().min(1500)]).unwrap_or_else(|e| {
        std::str::from_utf8(&b[..e.valid_up_to()]).unwrap_or("")
    });
    let rest = s.strip_prefix("{type:\"")?;
    let (ty, rest) = rest.split_once('"')?;
    if !matches!(ty, "local" | "local-jsx" | "prompt") {
        return None;
    }
    let rest = rest.strip_prefix(",name:\"")?;
    let (name, rest) = rest.split_once('"')?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return None;
    }
    if SKIP.contains(&name) {
        return None;
    }
    // Hasta el siguiente objeto, para no leer propiedades de otro.
    let body = rest.split("{type:\"").next().unwrap_or(rest);
    if body.contains("isHidden:!0") || body.contains("isEnabled:()=>!1") {
        return None;
    }
    let description = body
        .find("description:\"")
        .and_then(|k| {
            let d = &body[k + 13..];
            let mut out = String::new();
            let mut esc = false;
            for ch in d.chars() {
                match (esc, ch) {
                    (true, c) => {
                        out.push(c);
                        esc = false;
                    }
                    (false, '\\') => esc = true,
                    (false, '"') => return Some(out),
                    (false, c) => out.push(c),
                }
            }
            None
        })
        .unwrap_or_default();
    Some(SlashCmd {
        name: name.to_string(),
        description,
        kind: "builtin".into(),
        interactive: ty == "local-jsx",
    })
}

// ─────────── comandos y skills en disco ───────────

/// `<base>/<skill>/SKILL.md`; se saltan los que no se pueden invocar a mano.
fn disk_skills(base: &Path) -> Vec<SlashCmd> {
    let Ok(rd) = std::fs::read_dir(base) else { return vec![] };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let md = e.path().join("SKILL.md");
        let Ok(text) = std::fs::read_to_string(&md) else { continue };
        let fm = frontmatter(&text);
        if fm_get(fm, "user-invocable").as_deref() == Some("false") {
            continue;
        }
        let name = fm_get(fm, "name").unwrap_or_else(|| e.file_name().to_string_lossy().into_owned());
        let description = fm_get(fm, "description").unwrap_or_default();
        out.push(SlashCmd { name, description, kind: "skill".into(), interactive: false });
    }
    out
}

/// `<base>/**/<nombre>.md`; las subcarpetas dan el prefijo `carpeta:nombre`.
fn disk_commands(base: &Path) -> Vec<SlashCmd> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<SlashCmd>, depth: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let fname = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() && depth < 3 {
                walk(&p, &format!("{prefix}{fname}:"), out, depth + 1);
            } else if let Some(stem) = fname.strip_suffix(".md") {
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                let fm = frontmatter(&text);
                let description = fm_get(fm, "description").unwrap_or_else(|| {
                    let body = text.strip_prefix("---").and_then(|t| t.split_once("\n---")).map_or(text.as_str(), |x| x.1);
                    body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(120).collect()
                });
                out.push(SlashCmd { name: format!("{prefix}{stem}"), description, kind: "command".into(), interactive: false });
            }
        }
    }
    let mut out = Vec::new();
    walk(base, "", &mut out, 0);
    out
}

fn frontmatter(text: &str) -> &str {
    text.strip_prefix("---")
        .and_then(|t| t.split_once("\n---"))
        .map_or("", |(fm, _)| fm)
}

/// Valor de una clave del frontmatter YAML (admite los bloques `>` y `|` de varias líneas).
fn fm_get(fm: &str, key: &str) -> Option<String> {
    let mut lines = fm.lines().peekable();
    while let Some(l) = lines.next() {
        let Some(v) = l.strip_prefix(key).and_then(|r| r.strip_prefix(':')) else { continue };
        let v = v.trim();
        if v.is_empty() || v.starts_with('>') || v.starts_with('|') {
            let mut parts = Vec::new();
            while let Some(n) = lines.peek() {
                if n.starts_with(' ') || n.starts_with('\t') || n.trim().is_empty() {
                    let t = n.trim();
                    if !t.is_empty() {
                        parts.push(t.to_string());
                    }
                    lines.next();
                } else {
                    break;
                }
            }
            return Some(parts.join(" "));
        }
        return Some(v.trim_matches(['"', '\'']).to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_objects() {
        let s = br#"{type:"local",name:"compact",description:"Free up context by summarizing",isEnabled:()=>!0,load:()=>x}"#;
        let c = parse_obj(s).unwrap();
        assert_eq!((c.name.as_str(), c.description.as_str(), c.interactive), ("compact", "Free up context by summarizing", false));
        assert!(parse_obj(br#"{type:"local-jsx",name:"loops",description:"x",isEnabled:()=>!1}"#).is_none());
        assert!(parse_obj(br#"{type:"local",name:"agents",description:"x",isHidden:!0}"#).is_none());
        assert!(parse_obj(br#"{type:"text",name:"x",description:"y"}"#).is_none());
        let j = parse_obj(br#"{type:"local-jsx",name:"model",description:"Set the \"model\"",load:1}"#).unwrap();
        assert_eq!((j.description.as_str(), j.interactive), ("Set the \"model\"", true));
    }

    #[test]
    fn yaml_frontmatter() {
        let t = "---\nname: caveman\ndescription: >\n  Ultra-compressed mode.\n  Cuts tokens.\nuser-invocable: false\n---\nbody";
        let fm = frontmatter(t);
        assert_eq!(fm_get(fm, "name").as_deref(), Some("caveman"));
        assert_eq!(fm_get(fm, "description").as_deref(), Some("Ultra-compressed mode. Cuts tokens."));
        assert_eq!(fm_get(fm, "user-invocable").as_deref(), Some("false"));
    }

    /// CCDESK_TEST_SLASH=1 cargo test --release slash_real -- --nocapture
    #[test]
    fn slash_real() {
        if std::env::var("CCDESK_TEST_SLASH").is_err() {
            return;
        }
        let t = std::time::Instant::now();
        let v = scan_binary(&std::fs::canonicalize(paths::claude_bin()).unwrap());
        println!("{} comandos en {:?}", v.len(), t.elapsed());
        for c in &v {
            println!("  /{} {}— {}", c.name, if c.interactive { "[tui] " } else { "" }, c.description);
        }
        let all = list(&paths::claude_bin(), "/home/wzk", vec![]);
        println!("total con disco: {}", all.len());
    }
}
