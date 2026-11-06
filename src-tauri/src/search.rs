//! Búsqueda en las conversaciones: lo que escribe el usuario y lo que contesta Claude (sin
//! salidas de herramientas, que son casi todo el peso del transcript y sólo meterían ruido).
//! El texto se guarda en memoria junto con una versión «plegada» (minúsculas y sin tildes)
//! donde se busca. Los transcripts sólo crecen, así que cada refresco lee lo nuevo.

use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;

use crate::app::{transcript_of, Core};
use crate::transcript::{self, Item};

const BEFORE: usize = 40;
const AFTER: usize = 90;

#[derive(Default)]
pub struct Index {
    docs: HashMap<String, Doc>, // por session id
}

struct Doc {
    path: PathBuf,
    offset: u64,
    msgs: Vec<Msg>,
    /// Skills que Claude Code anunció a la sesión (adjunto `skill_listing`): (nombre, descripción).
    skills: Vec<(String, String)>,
}

/// Un adjunto `skill_listing`: el inicial trae la lista entera; los demás, altas.
struct Listing {
    initial: bool,
    items: Vec<(String, String)>,
}

struct Msg {
    text: String,
    /// Mismo texto plegado carácter a carácter (misma cantidad de caracteres que `text`).
    fold: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub key: String,
    pub count: usize,
    /// Fragmento de la coincidencia más reciente: antes, lo encontrado y después.
    pub before: String,
    pub hit: String,
    pub after: String,
}

/// Minúscula y sin tilde; siempre un carácter por carácter.
pub fn fold_char(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        _ => c,
    }
}

pub fn fold(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

/// Pone al día el índice con las sesiones de la lista (lee sólo lo añadido a cada transcript).
pub fn refresh(core: &Core) {
    // Un refresco a la vez: dos a la par leerían lo mismo y duplicarían mensajes.
    static BUSY: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    let views: Vec<(String, String)> =
        core.views.lock().unwrap().iter().map(|v| (v.sid.clone(), v.cwd.clone())).collect();
    for (sid, cwd) in views {
        if sid.is_empty() {
            continue;
        }
        let known = core.search.lock().unwrap().docs.get(&sid).map(|d| (d.path.clone(), d.offset));
        let (path, offset) = match known.filter(|(p, _)| p.is_file()) {
            Some(x) => x,
            None => {
                let Some(p) = transcript_of(&sid, &cwd) else { continue };
                (p, 0)
            }
        };
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if len == offset {
            continue;
        }
        // Si encogió (reescrito), se vuelve a leer entero.
        let from = if len < offset { 0 } else { offset };
        let Some((msgs, listings, read)) = read_from(&path, from) else { continue };
        let mut idx = core.search.lock().unwrap();
        let doc = idx
            .docs
            .entry(sid)
            .or_insert_with(|| Doc { path: path.clone(), offset: 0, msgs: vec![], skills: vec![] });
        if from == 0 {
            doc.msgs.clear();
            doc.skills.clear();
        }
        doc.path = path;
        doc.offset = from + read;
        doc.msgs.extend(msgs);
        for l in listings {
            if l.initial {
                doc.skills = l.items;
            } else {
                for it in l.items {
                    if !doc.skills.iter().any(|(n, _)| *n == it.0) {
                        doc.skills.push(it);
                    }
                }
            }
        }
    }
}

/// Mensajes de conversación desde `from` hasta la última línea completa, y bytes consumidos.
/// Se lee línea a línea: algunos transcripts pasan de 100 MB y no hace falta tenerlos enteros.
fn read_from(path: &PathBuf, from: u64) -> Option<(Vec<Msg>, Vec<Listing>, u64)> {
    let mut f = BufReader::with_capacity(1 << 20, File::open(path).ok()?);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut msgs = Vec::new();
    let mut listings = Vec::new();
    let mut used = 0u64;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = f.read_until(b'\n', &mut buf).ok()?;
        if n == 0 || buf.last() != Some(&b'\n') {
            break; // fin, o línea a medio escribir: se leerá en el próximo refresco
        }
        used += n as u64;
        let line = String::from_utf8_lossy(&buf);
        // Las respuestas de herramientas (lo más pesado) ni se analizan.
        if line.contains("\"type\":\"tool_result\"") {
            continue;
        }
        if line.contains("\"type\":\"skill_listing\"") {
            if let Some(l) = parse_listing(&line) {
                listings.push(l);
            }
            continue;
        }
        for it in transcript::parse_line_opts(line.trim_end(), false) {
            let text = match it {
                Item::User { text, .. } | Item::Assistant { text, .. } => text,
                _ => continue,
            };
            if !text.trim().is_empty() {
                let fold = fold(&text);
                msgs.push(Msg { text, fold });
            }
        }
    }
    Some((msgs, listings, used))
}

fn parse_listing(line: &str) -> Option<Listing> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let a = &v["attachment"];
    let mut items = Vec::new();
    for l in a["content"].as_str()?.lines() {
        let Some(rest) = l.strip_prefix("- ") else { continue };
        let (name, desc) = rest.split_once(": ").unwrap_or((rest, ""));
        if !name.is_empty() && !name.contains(' ') {
            items.push((name.to_string(), desc.trim().to_string()));
        }
    }
    Some(Listing { initial: a["isInitial"].as_bool() == Some(true), items })
}

/// Skills anunciados a la sesión `sid` (vacío si su transcript aún no tiene la lista).
pub fn skills_of(core: &Core, sid: &str) -> Vec<(String, String)> {
    core.search.lock().unwrap().docs.get(sid).map(|d| d.skills.clone()).unwrap_or_default()
}

/// Hilo que mantiene el índice al día (cada 10 s lee sólo lo añadido a cada transcript). Las
/// búsquedas y el menú «/» leen el índice tal cual, sin esperar.
pub fn spawn_indexer(core: std::sync::Arc<Core>) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        loop {
            refresh(&core);
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    });
}

/// Sesiones cuya conversación contiene `query` (sin distinguir mayúsculas ni tildes).
pub fn search(core: &Core, query: &str) -> Vec<Hit> {
    let q = fold(query.trim());
    if q.chars().count() < 2 {
        return vec![];
    }
    let keys: Vec<(String, String)> =
        core.views.lock().unwrap().iter().map(|v| (v.key.clone(), v.sid.clone())).collect();
    let idx = core.search.lock().unwrap();
    let mut out = Vec::new();
    for (key, sid) in keys {
        let Some(doc) = idx.docs.get(&sid) else { continue };
        let mut count = 0;
        let mut last = None;
        for m in &doc.msgs {
            let mut it = m.fold.match_indices(&q).peekable();
            if let Some(&(first, _)) = it.peek() {
                count += it.count();
                last = Some((m, first));
            }
        }
        if let Some((m, at)) = last {
            let (before, hit, after) = snippet(&m.text, &m.fold, at, q.chars().count());
            out.push(Hit { key, count, before, hit, after });
        }
    }
    out
}

/// Item del mensaje cuyo fragmento enseña `search`: el más reciente de lo escrito o contestado que
/// contiene `query`, con el mismo criterio. Al pulsar el resultado, la conversación va ahí.
pub fn locate(items: &[Item], query: &str) -> Option<usize> {
    let q = fold(query.trim());
    if q.chars().count() < 2 {
        return None;
    }
    items.iter().rposition(|it| match it {
        Item::User { text, .. } | Item::Assistant { text, .. } => fold(text).contains(&q),
        _ => false,
    })
}

/// Coincidencias de Ctrl+F en un item de la conversación (`find_items`).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Found {
    pub index: usize,
    pub count: usize,
}

/// Lo que se ve de un item en la conversación (aproximado: el Markdown va sin pintar).
fn visible(it: &Item) -> Vec<&str> {
    match it {
        Item::User { text, .. }
        | Item::Output { text, .. }
        | Item::Assistant { text, .. }
        | Item::ToolResult { text, .. }
        | Item::Notice { text, .. }
        | Item::Summary { text, .. }
        | Item::AgentReport { text, .. } => vec![text.as_str()],
        Item::Command { name, args, .. } => vec![name.as_str(), args.as_str()],
        Item::Tool { name, summary, input, .. } => vec![name.as_str(), summary.as_str(), input.as_str()],
        Item::Agent { description, prompt, .. } => vec![description.as_str(), prompt.as_str()],
        // El de uno lanzado en segundo plano es el aviso de arranque, que no se enseña.
        Item::AgentResult { status, text, .. } if status != "async_launched" => vec![text.as_str()],
        _ => vec![],
    }
}

/// Items (por su índice en `items`) donde sale `query`, y cuántas veces, del más reciente al más
/// antiguo. Como en la vista: sin distinguir mayúsculas ni tildes, y un espacio vale por cualquier
/// racha de blancos.
pub fn find_items(items: &[Item], query: &str) -> Vec<Found> {
    let words = |s: String| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let q = words(fold(query));
    if q.is_empty() {
        return vec![];
    }
    let spaced = q.contains(' ');
    let mut out = Vec::new();
    for (index, it) in items.iter().enumerate().rev() {
        let count: usize = visible(it)
            .into_iter()
            .map(|t| {
                let f = fold(t);
                if spaced { words(f).matches(&q).count() } else { f.matches(&q).count() }
            })
            .sum();
        if count > 0 {
            out.push(Found { index, count });
        }
    }
    out
}

/// Fragmento alrededor de la coincidencia (`at`: byte en el texto plegado).
fn snippet(text: &str, fold: &str, at: usize, qlen: usize) -> (String, String, String) {
    let ci = fold[..at].chars().count();
    let chars: Vec<char> = text.chars().collect();
    // En una línea y sin las marcas de Markdown más comunes.
    let flat = |s: &[char]| {
        let t: String = s.iter().collect();
        t.replace("**", "").replace('`', "").split_whitespace().collect::<Vec<_>>().join(" ")
    };
    // Recortado a palabras enteras.
    let mut b0 = ci.saturating_sub(BEFORE);
    if b0 > 0 {
        if let Some(k) = chars[b0..ci].iter().position(|c| c.is_whitespace()) {
            b0 += k + 1;
        }
    }
    let mut a1 = (ci + qlen + AFTER).min(chars.len());
    if a1 < chars.len() {
        if let Some(k) = chars[ci + qlen..a1].iter().rposition(|c| c.is_whitespace()) {
            a1 = ci + qlen + k;
        }
    }
    let mut before = flat(&chars[b0..ci]);
    if b0 > 0 {
        before = format!("…{before}");
    }
    if ci > 0 && chars[ci - 1].is_whitespace() {
        before.push(' ');
    }
    let hit = flat(&chars[ci..ci + qlen]);
    let mut after = flat(&chars[ci + qlen..a1]);
    if chars.get(ci + qlen).is_some_and(|c| c.is_whitespace()) {
        after.insert(0, ' ');
    }
    if a1 < chars.len() {
        after.push('…');
    }
    (before, hit, after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_keeps_length() {
        let s = "Configuración ÁRBOL Ñandú çedilla İstanbul";
        assert_eq!(fold(s).chars().count(), s.chars().count());
        assert_eq!(fold("Configuración"), "configuracion");
    }

    /// CCDESK_TEST_SEARCH=<texto> cargo test --release search_real -- --nocapture
    #[test]
    fn search_real() {
        let Ok(q) = std::env::var("CCDESK_TEST_SEARCH") else { return };
        let st: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(crate::paths::config_dir().join("state.json")).unwrap()).unwrap();
        let t0 = std::time::Instant::now();
        let (mut docs, mut bytes, mut nmsg) = (vec![], 0usize, 0usize);
        for (_, k) in st["known"].as_object().unwrap() {
            let sid = k["sid"].as_str().unwrap_or("");
            let Some(p) = transcript::find(sid, k["cwd"].as_str().unwrap_or(""), None) else { continue };
            let (msgs, _, _) = read_from(&p, 0).unwrap();
            bytes += msgs.iter().map(|m| m.text.len() + m.fold.len()).sum::<usize>();
            nmsg += msgs.len();
            docs.push((sid.to_string(), msgs));
        }
        println!("indexado: {} sesiones, {nmsg} mensajes, {:.1} MB en memoria, {:?}", docs.len(), bytes as f64 / 1e6, t0.elapsed());
        let t1 = std::time::Instant::now();
        let qf = fold(&q);
        for (sid, msgs) in &docs {
            let mut n = 0; let mut last = None;
            for m in msgs { let c = m.fold.matches(&qf).count(); if c > 0 { n += c; last = Some(m); } }
            if let Some(m) = last {
                let at = m.fold.rfind(&qf).unwrap();
                let (b, h, a) = snippet(&m.text, &m.fold, at, qf.chars().count());
                println!("  {} ×{n}: {b}[{h}]{a}", &sid[..8]);
            }
        }
        println!("búsqueda: {:?}", t1.elapsed());
    }

    #[test]
    fn find_items_newest_first() {
        let ts = String::new;
        let items = vec![
            Item::User { ts: ts(), text: "Revisa la Configuración".into(), mid_turn: false, images: vec![] },
            Item::Tool {
                ts: ts(),
                id: "t".into(),
                name: "Read".into(),
                summary: "/x/configuracion.md".into(),
                input: "{}".into(),
                shot: String::new(),
            },
            Item::AgentResult {
                ts: ts(),
                tool_use_id: "t2".into(),
                agent_id: "a".into(),
                status: "async_launched".into(),
                text: "configuración".into(),
                is_error: false,
                model: None,
                duration_ms: None,
                tokens: None,
                tool_uses: None,
            },
            Item::QueueAdd { ts: ts(), text: "configuración".into(), system: false },
            Item::Assistant {
                ts: ts(),
                msg_id: "m".into(),
                text: "La **configuración**\n  del tema; CONFIGURACIÓN otra vez".into(),
                cwd: String::new(),
            },
        ];
        let f = |index, count| Found { index, count };
        assert_eq!(find_items(&items, "configuración"), vec![f(4, 2), f(1, 1), f(0, 1)]);
        // Un espacio vale por cualquier racha de blancos (aquí, el salto de línea del Markdown).
        assert_eq!(find_items(&items, "CONFIGURACION**  del"), vec![f(4, 1)]);
        assert!(find_items(&items, "  ").is_empty());
        assert!(find_items(&items[..4], "otra vez").is_empty());
        // El resultado de la búsqueda de sesiones: sólo lo escrito o contestado, el más reciente.
        assert_eq!(locate(&items, " CONFIGURACIÓN "), Some(4));
        assert_eq!(locate(&items[..4], "configuracion"), Some(0));
        assert_eq!(locate(&items, "configuracion.md"), None); // sólo en la herramienta
        assert_eq!(locate(&items, "c"), None);
    }

    #[test]
    fn snippet_around_match() {
        let text = "Primero revisé el fichero.\nLuego cambié la Configuración del tema y probé.";
        let f = fold(text);
        let at = f.find("configuracion").unwrap();
        let (b, h, a) = snippet(text, &f, at, "configuracion".chars().count());
        assert_eq!(h, "Configuración");
        assert!(b.ends_with("cambié la "), "{b:?}");
        assert!(a.starts_with(" del tema"), "{a:?}");
        assert!(b.starts_with('…'));
    }
}

