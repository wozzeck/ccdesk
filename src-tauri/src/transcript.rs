//! Lectura incremental del transcript JSONL de una sesión y conversión a items de UI.
use serde::Serialize;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::paths;

const MAX_OUTPUT: usize = 8000;
const MAX_INPUT: usize = 4000;

/// Completa en las imágenes de los items de una línea su fichero y su byte de inicio.
fn locate_images(items: &mut [Item], path: &str, offset: u64) {
    for it in items {
        if let Item::User { images, .. } | Item::ToolResult { images, .. } = it {
            for r in images {
                r.path = path.to_string();
                r.offset = offset;
            }
        }
    }
}

/// Imágenes de una línea en el orden en que se numeran: bloques del mensaje y, dentro de cada
/// resultado de herramienta, las suyas.
fn line_images(v: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    for b in v["message"]["content"].as_array().into_iter().flatten() {
        match b["type"].as_str() {
            Some("image") => out.push(b),
            Some("tool_result") => {
                out.extend(b["content"].as_array().into_iter().flatten().filter(|x| x["type"] == "image"))
            }
            _ => {}
        }
    }
    out
}

/// La imagen `n` de la línea que empieza en `offset`, como URL `data:` (o su URL, si la trae).
pub fn image_at(path: &Path, offset: u64, n: usize) -> Option<String> {
    let mut f = BufReader::new(File::open(path).ok()?);
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut line = Vec::new();
    f.read_until(b'\n', &mut line).ok()?;
    let v: Value = serde_json::from_slice(&line).ok()?;
    let img = *line_images(&v).get(n)?;
    let src = &img["source"];
    match src["type"].as_str()? {
        "base64" => Some(format!(
            "data:{};base64,{}",
            src["media_type"].as_str().unwrap_or("image/png"),
            src["data"].as_str()?
        )),
        "url" => src["url"].as_str().map(String::from),
        _ => None,
    }
}

/// Dónde está una imagen del transcript: línea (byte de inicio) y orden entre las de esa línea.
/// El base64 no viaja con la conversación; la interfaz lo pide al mostrarla (`transcript_image`).
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImgRef {
    pub path: String,
    pub offset: u64,
    pub n: usize,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Item {
    /// `mid_turn`: enviado mientras Claude trabajaba (llega como adjunto `queued_command`).
    User {
        ts: String,
        text: String,
        mid_turn: bool,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImgRef>,
    },
    Command { ts: String, name: String, args: String },
    Output { ts: String, text: String },
    Assistant {
        ts: String,
        msg_id: String,
        text: String,
        /// Carpeta en la que estaba Claude: base de las rutas relativas que menciona.
        #[serde(skip_serializing_if = "String::is_empty")]
        cwd: String,
    },
    Tool { ts: String, id: String, name: String, summary: String, input: String },
    /// Llamada a un subagente (herramienta Agent/Task).
    Agent {
        ts: String,
        id: String,
        description: String,
        agent_type: String,
        model: String,
        background: bool,
        prompt: String,
    },
    /// Resultado de la llamada: informe final (síncrono) o `async_launched` (segundo plano).
    AgentResult {
        ts: String,
        tool_use_id: String,
        agent_id: String,
        status: String,
        text: String,
        is_error: bool,
        model: Option<String>,
        duration_ms: Option<u64>,
        tokens: Option<u64>,
        tool_uses: Option<u64>,
    },
    /// Informe final que un subagente devuelve por «hand-back» (mensaje entre agentes).
    AgentReport { ts: String, agent_id: String, text: String },
    /// `<task-notification>`: fin de un subagente en segundo plano (o eventos de Monitor/Bash).
    TaskNotice {
        ts: String,
        tool_use_id: String,
        task_id: String,
        status: String,
        summary: String,
        result: String,
        duration_ms: Option<u64>,
        tokens: Option<u64>,
        tool_uses: Option<u64>,
    },
    ToolResult {
        ts: String,
        tool_use_id: String,
        text: String,
        is_error: bool,
        /// Imágenes del resultado (p. ej. `Read` de una foto): se cargan aparte, al verse.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImgRef>,
    },
    Notice { ts: String, text: String },
    /// Resumen que Claude Code escribe al compactar (va como mensaje de usuario, pero no lo es).
    Summary { ts: String, text: String },
    /// Cola de entrada de Claude Code (FIFO). `system`: aviso interno (`<task-notification>`…).
    QueueAdd { ts: String, text: String, system: bool },
    /// Entregado el primero de la cola (no trae texto).
    QueueDequeue { ts: String },
    /// Retirado uno concreto (absorbido a mitad de turno, cancelado…).
    QueueRemove { ts: String, text: String },
    /// `popAll`: el usuario recuperó a la caja de entrada todo lo que tenía en cola.
    QueueClear { ts: String },
    /// Tu mensaje `of` (índice en `items`) se quedó fuera de la conversación: lo interrumpiste
    /// antes de que Claude respondiera (Claude Code lo devuelve a su caja y sigue desde el anterior).
    Withdrawn { ts: String, of: usize },
}

/// Lo que hace falta de un registro para seguir el árbol de la conversación.
struct Link {
    uuid: String,
    parent: String,
    kind: String,
    ts: String,
}

/// Tu último mensaje mientras no tenga respuesta de Claude, con lo que cuelga de él (adjuntos de
/// hooks, avisos…).
struct Pending {
    uuid: String,
    parent: String,
    index: usize,
    desc: Vec<String>,
}

/// Localiza el JSONL: `transcript_path` del statusline > cwd codificado > búsqueda.
pub fn find(sid: &str, cwd: &str, hint: Option<&str>) -> Option<PathBuf> {
    if sid.is_empty() {
        return None;
    }
    if let Some(h) = hint {
        let p = PathBuf::from(h);
        if p.is_file() {
            return Some(p);
        }
    }
    let file = format!("{sid}.jsonl");
    let p = paths::projects_dir().join(paths::encode_cwd(cwd)).join(&file);
    if p.is_file() {
        return Some(p);
    }
    std::fs::read_dir(paths::projects_dir())
        .ok()?
        .flatten()
        .map(|d| d.path().join(&file))
        .find(|p| p.is_file())
}

pub struct Cache {
    pub key: String,
    pub sid: String,
    pub path: PathBuf,
    offset: u64,
    pub items: Vec<Item>,
    /// Los ficheros de subagentes marcan todas sus líneas como `isSidechain`.
    sidechain: bool,
    pending: Option<Pending>,
}

impl Cache {
    pub fn open(key: &str, sid: &str, path: &Path) -> std::io::Result<Cache> {
        Self::open_opts(key, sid, path, false)
    }

    pub fn open_opts(key: &str, sid: &str, path: &Path, sidechain: bool) -> std::io::Result<Cache> {
        let mut c = Cache {
            key: key.to_string(),
            sid: sid.to_string(),
            path: path.to_path_buf(),
            offset: 0,
            items: Vec::new(),
            sidechain,
            pending: None,
        };
        c.refresh()?;
        Ok(c)
    }

    /// Sigue el árbol para ver si tu último mensaje se quedó fuera: si, sin respuesta de Claude, la
    /// conversación sigue desde su padre, es que lo interrumpiste antes de que respondiera. Devuelve
    /// la marca, que va delante de los items de esta línea.
    fn follow(&mut self, l: &Link, items: &[Item]) -> Option<Item> {
        let mut mark = None;
        let mut answered = false;
        if let Some(p) = self.pending.as_mut() {
            if l.parent == p.uuid || p.desc.contains(&l.parent) {
                if l.kind == "assistant" {
                    answered = true;
                } else {
                    p.desc.push(l.uuid.clone());
                }
            } else if l.kind == "user" && l.parent == p.parent {
                mark = Some(Item::Withdrawn { ts: l.ts.clone(), of: p.index });
            }
        }
        if answered || mark.is_some() {
            self.pending = None;
        }
        if l.kind == "user" {
            if let Some(k) = items.iter().position(|it| matches!(it, Item::User { mid_turn: false, .. })) {
                let index = self.items.len() + usize::from(mark.is_some()) + k;
                self.pending = Some(Pending { uuid: l.uuid.clone(), parent: l.parent.clone(), index, desc: Vec::new() });
            }
        }
        mark
    }

    /// Lee las líneas completas nuevas desde el último offset y las añade a `items`; devuelve
    /// cuántos items nuevos hay (están al final de `items`). Línea a línea: los transcripts grandes
    /// (100 MB) no se cargan enteros en memoria.
    pub fn refresh(&mut self) -> std::io::Result<usize> {
        let len = std::fs::metadata(&self.path)?.len();
        if len < self.offset {
            self.offset = 0;
            self.items.clear();
            self.pending = None;
        }
        if len == self.offset {
            return Ok(0);
        }
        let mut f = BufReader::with_capacity(1 << 20, File::open(&self.path)?);
        f.seek(SeekFrom::Start(self.offset))?;
        // Nota: el JSONL es un árbol (parentUuid), pero no se filtra por rama: con llamadas a
        // herramientas en paralelo cada resultado cuelga de su llamada y sólo el último sigue la
        // cadena, así que seguirla desde la hoja escondería resultados válidos.
        let path = self.path.to_string_lossy().into_owned();
        let before = self.items.len();
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let n = f.read_until(b'\n', &mut buf)?;
            if n == 0 || buf.last() != Some(&b'\n') {
                break; // fin, o línea a medio escribir: se leerá en el próximo refresco
            }
            let start = self.offset;
            self.offset += n as u64;
            if let Ok(s) = std::str::from_utf8(&buf[..n - 1]) {
                if s.is_empty() {
                    continue;
                }
                let (mut items, link) = parse_line_link(s, self.sidechain);
                locate_images(&mut items, &path, start);
                if let Some(mark) = link.and_then(|l| self.follow(&l, &items)) {
                    self.items.push(mark);
                }
                self.items.extend(items);
            }
        }
        Ok(self.items.len() - before)
    }
}

/// Registros auxiliares (`ai-title`, `api-request-blob`…) empiezan por `{"type":"…"`;
/// los de conversación empiezan por `{"parentUuid"`. Así se descartan blobs enormes sin parsearlos.
fn quick_type(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("{\"type\":\"")?;
    let j = rest.find('"')?;
    Some(&rest[..j])
}

/// Registros de conversación: los que cuentan como actividad.
const CONVERSATION: [&str; 4] = ["user", "assistant", "system", "attachment"];

/// Hora (ms) del último registro de conversación con fecha: mensajes, avisos del sistema,
/// compactaciones… Es la última actividad de una sesión detenida: al salir con `/exit`, Claude
/// Code sólo añade registros auxiliares (`last-prompt`, `ai-title`, `cost-state`…), que no
/// cuentan. Lee desde el final por bloques, como mucho 8 MB.
pub fn last_activity(path: &Path) -> Option<u64> {
    last_activity_by(path, 64 * 1024)
}

fn last_activity_by(path: &Path, chunk: u64) -> Option<u64> {
    const LIMIT: u64 = 8 << 20;
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut end = len;
    // Final de la línea que cruza el borde con el bloque anterior: se completa con él.
    let mut carry: Vec<u8> = Vec::new();
    while end > 0 && len - end < LIMIT {
        let start = end.saturating_sub(chunk);
        let mut buf = vec![0u8; (end - start) as usize];
        f.seek(SeekFrom::Start(start)).ok()?;
        f.read_exact(&mut buf).ok()?;
        buf.extend_from_slice(&carry);
        let mut lines = buf.split(|b| *b == b'\n');
        // Con más fichero delante, el primer trozo es el final de una línea que empieza antes.
        let head = if start > 0 { lines.next().unwrap_or_default().to_vec() } else { Vec::new() };
        for line in lines.rev() {
            if let Some(t) = line_time(line) {
                return Some(t);
            }
        }
        carry = head;
        end = start;
    }
    None
}

fn line_time(line: &[u8]) -> Option<u64> {
    let s = std::str::from_utf8(line).ok()?.trim_end_matches('\r');
    // Los auxiliares se descartan sin parsearlos (los hay enormes).
    if quick_type(s).is_some_and(|t| !CONVERSATION.contains(&t)) {
        return None;
    }
    let v: Value = serde_json::from_str(s).ok()?;
    if !CONVERSATION.contains(&v.get("type")?.as_str()?) {
        return None;
    }
    iso_ms(v.get("timestamp")?.as_str()?)
}

/// «2026-11-03T18:36:56.123Z» (o con desfase, «…+01:00») → ms desde 1970.
pub fn iso_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut rest = &s[19..];
    let mut ms = 0;
    if let Some(frac) = rest.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        let digits = frac[..n].as_bytes();
        for k in 0..3 {
            ms = ms * 10 + digits.get(k).map_or(0, |c| i64::from(c - b'0'));
        }
        rest = &frac[n..];
    }
    let off = match rest {
        "Z" | "z" | "" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (hh, mm) = (rest.get(1..3)?.parse::<i64>().ok()?, rest.get(4..6)?.parse::<i64>().ok()?);
            sign * (hh * 60 + mm) * 60_000
        }
    };
    let t = (((days_from_civil(y, mo, d) * 24 + h) * 60 + mi) * 60 + se) * 1000 + ms - off;
    u64::try_from(t).ok()
}

/// Días desde el 1-1-1970 (algoritmo de Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
pub fn parse_line(line: &str) -> Vec<Item> {
    parse_line_opts(line, false)
}

pub fn parse_line_opts(line: &str, sidechain: bool) -> Vec<Item> {
    parse_line_link(line, sidechain).0
}

/// Los items de una línea y, si es un registro de la conversación, su sitio en el árbol.
fn parse_line_link(line: &str, sidechain: bool) -> (Vec<Item>, Option<Link>) {
    if let Some(t) = quick_type(line) {
        if !matches!(t, "user" | "assistant" | "system" | "queue-operation") {
            return (vec![], None);
        }
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return (vec![], None);
    };
    if !sidechain && v["isSidechain"].as_bool() == Some(true) {
        return (vec![], None);
    }
    let link = v["uuid"].as_str().map(|u| Link {
        uuid: u.to_string(),
        parent: v["parentUuid"].as_str().unwrap_or("").to_string(),
        kind: v["type"].as_str().unwrap_or("").to_string(),
        ts: v["timestamp"].as_str().unwrap_or("").to_string(),
    });
    (parse_value(&v), link)
}

fn parse_value(v: &Value) -> Vec<Item> {
    let ts = v["timestamp"].as_str().unwrap_or("").to_string();
    match v["type"].as_str() {
        Some("user") => parse_user(v, ts),
        Some("assistant") => parse_assistant(v, ts),
        Some("system") => parse_system(v, ts),
        Some("attachment") => parse_attachment(v, ts),
        Some("queue-operation") => parse_queue(v, ts),
        _ => vec![],
    }
}

fn parse_user(v: &Value, ts: String) -> Vec<Item> {
    if v["isMeta"].as_bool() == Some(true) {
        return vec![];
    }
    let content = &v["message"]["content"];
    if v["isCompactSummary"].as_bool() == Some(true) {
        let text = content
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| result_text(content));
        return vec![Item::Summary { ts, text: compact_summary(&text) }];
    }
    if let Some(s) = content.as_str() {
        return parse_user_text(s, &ts).into_iter().collect();
    }
    let mut out = Vec::new();
    // Las imágenes se numeran en el mismo orden que las recorre `line_images`.
    let mut img_n = 0;
    let mut user_imgs = Vec::new();
    for b in content.as_array().into_iter().flatten() {
        match b["type"].as_str() {
            Some("text") => {
                if let Some(s) = b["text"].as_str() {
                    out.extend(parse_user_text(s, &ts));
                }
            }
            Some("image") => {
                user_imgs.push(ImgRef { n: img_n, ..Default::default() });
                img_n += 1;
            }
            Some("tool_result") if v["toolUseResult"]["agentId"].is_string() => {
                out.push(agent_result(v, b, &ts));
            }
            Some("tool_result") => {
                let mut images = Vec::new();
                for sub in b["content"].as_array().into_iter().flatten() {
                    if sub["type"] == "image" {
                        images.push(ImgRef { n: img_n, ..Default::default() });
                        img_n += 1;
                    }
                }
                out.push(Item::ToolResult {
                    ts: ts.clone(),
                    tool_use_id: b["tool_use_id"].as_str().unwrap_or("").to_string(),
                    text: trunc(&strip_ansi(&result_text(&b["content"])), MAX_OUTPUT),
                    is_error: b["is_error"].as_bool().unwrap_or(false),
                    images,
                })
            }
            _ => {}
        }
    }
    // Imágenes pegadas por el usuario: van con su texto (o solas, en una burbuja sin texto).
    if !user_imgs.is_empty() {
        match out.iter_mut().rev().find_map(|it| match it {
            Item::User { images, .. } => Some(images),
            _ => None,
        }) {
            Some(images) => images.extend(user_imgs),
            None => out.push(Item::User { ts, text: String::new(), mid_turn: false, images: user_imgs }),
        }
    }
    out
}

fn result_text(c: &Value) -> String {
    if let Some(s) = c.as_str() {
        return s.to_string();
    }
    c.as_array()
        .map(|a| {
            a.iter()
                .map(|b| match b["type"].as_str() {
                    Some("text") => b["text"].as_str().unwrap_or("").to_string(),
                    Some("image") => "[imagen]".to_string(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn between(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let a = s.find(&open)? + open.len();
    let b = s[a..].find(&close).map(|i| a + i).unwrap_or(s.len());
    Some(s[a..b].to_string())
}

fn parse_user_text(s: &str, ts: &str) -> Option<Item> {
    let t = s.trim_start();
    let ts = ts.to_string();
    if t.is_empty()
        || t.starts_with("<local-command-caveat>")
        || t.starts_with("<system-reminder>")
    {
        return None;
    }
    if t.contains("<command-name>") {
        let name = between(t, "command-name").unwrap_or_default().trim().to_string();
        let args = between(t, "command-args").unwrap_or_default().trim().to_string();
        // `/compact` a secas ya se ve como la marca de conversación compactada, con su resumen.
        if name == "/compact" && args.is_empty() {
            return None;
        }
        return Some(Item::Command { ts, name, args });
    }
    if t.starts_with("<bash-input>") {
        return Some(Item::Command {
            ts,
            name: "!".into(),
            args: between(t, "bash-input").unwrap_or_default(),
        });
    }
    for pair in [
        ("local-command-stdout", "local-command-stderr"),
        ("bash-stdout", "bash-stderr"),
    ] {
        if t.starts_with(&format!("<{}>", pair.0)) || t.starts_with(&format!("<{}>", pair.1)) {
            let body = [between(t, pair.0), between(t, pair.1)]
                .into_iter()
                .flatten()
                .filter(|x| !x.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let text = strip_ansi(&body);
            // «Compacted (ctrl+o to see full summary)»: el resumen ya se ve arriba, y ctrl+o sólo
            // existe en la terminal.
            if text.trim().is_empty() || (text.trim_start().starts_with("Compacted") && text.contains("ctrl+o")) {
                return None;
            }
            return Some(Item::Output { ts, text: trunc(&text, MAX_OUTPUT) });
        }
    }
    if t.starts_with("[Request interrupted") {
        return Some(Item::Notice {
            ts,
            text: "Interrumpido por el usuario".into(),
        });
    }
    if t.starts_with("<task-notification>") {
        return task_notice(t, &ts);
    }
    Some(Item::User {
        ts,
        text: strip_pasted(s),
        mid_turn: false,
        images: vec![],
    })
}

/// Deja sólo el resumen: quita el preámbulo, los `<system-reminder>` y las instrucciones finales
/// que Claude Code añade para el modelo.
fn compact_summary(s: &str) -> String {
    let mut t = s.to_string();
    while let (Some(a), Some(b)) = (t.find("<system-reminder>"), t.find("</system-reminder>")) {
        if b < a {
            break;
        }
        t.replace_range(a..b + "</system-reminder>".len(), "");
    }
    if let Some(i) = t.find("Summary:\n") {
        t = t[i + "Summary:\n".len()..].to_string();
    } else if t.starts_with("This session is being continued") {
        if let Some(i) = t.find("\n\n") {
            t = t[i..].to_string();
        }
    }
    for end in [
        "\nIf you need specific details from before compaction",
        "\nContinue the conversation from where it left off",
        "\nThe messages after this summary are",
    ] {
        if let Some(i) = t.find(end) {
            t.truncate(i);
        }
    }
    t.trim().to_string()
}

fn parse_assistant(v: &Value, ts: String) -> Vec<Item> {
    let msg_id = v["message"]["id"].as_str().unwrap_or("").to_string();
    let mut out = Vec::new();
    for b in v["message"]["content"].as_array().into_iter().flatten() {
        match b["type"].as_str() {
            Some("text") => {
                let text = b["text"].as_str().unwrap_or("");
                if !text.trim().is_empty() {
                    out.push(Item::Assistant {
                        ts: ts.clone(),
                        msg_id: msg_id.clone(),
                        text: text.to_string(),
                        cwd: v["cwd"].as_str().unwrap_or("").to_string(),
                    });
                }
            }
            Some("tool_use") if matches!(b["name"].as_str(), Some("Agent" | "Task")) => {
                let input = &b["input"];
                let g = |k: &str| input[k].as_str().unwrap_or("").to_string();
                let model = [g("model"), g("effort")]
                    .into_iter()
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(Item::Agent {
                    ts: ts.clone(),
                    id: b["id"].as_str().unwrap_or("").to_string(),
                    description: g("description"),
                    agent_type: g("subagent_type"),
                    model,
                    background: input["run_in_background"].as_bool().unwrap_or(false)
                        || input["run_in_background"].as_str() == Some("true"),
                    prompt: trunc(&g("prompt"), 60_000),
                });
            }
            Some("tool_use") => {
                let name = b["name"].as_str().unwrap_or("?").to_string();
                let input = &b["input"];
                out.push(Item::Tool {
                    ts: ts.clone(),
                    id: b["id"].as_str().unwrap_or("").to_string(),
                    summary: tool_summary(&name, input),
                    input: tool_input(&name, input),
                    name,
                });
            }
            _ => {}
        }
    }
    out
}

fn parse_attachment(v: &Value, ts: String) -> Vec<Item> {
    let a = &v["attachment"];
    if a["type"].as_str() != Some("queued_command") {
        return vec![];
    }
    let origin = &a["origin"];
    if origin["kind"].as_str() == Some("peer") {
        if origin["handback"].as_bool() == Some(true) {
            let text = origin["handbackReportText"]
                .as_str()
                .or_else(|| origin["body"].as_str())
                .unwrap_or("");
            return vec![Item::AgentReport {
                ts,
                agent_id: origin["from"].as_str().unwrap_or("").to_string(),
                text: trunc(text, 200_000),
            }];
        }
        let name = origin["name"].as_str().or_else(|| origin["from"].as_str()).unwrap_or("otro agente");
        return vec![Item::Notice { ts, text: format!("Mensaje de {name}") }];
    }
    if let Some(p) = a["prompt"].as_str().filter(|p| p.trim_start().starts_with("<task-notification>")) {
        return task_notice(p.trim_start(), &ts).into_iter().collect();
    }
    let human = a["origin"]["kind"].as_str() == Some("human") || a["humanTurn"].as_bool() == Some(true);
    if !human {
        return vec![];
    }
    let text = match &a["prompt"] {
        Value::String(s) => s.clone(),
        other => result_text(other),
    };
    if text.trim().is_empty() {
        return vec![];
    }
    vec![Item::User {
        ts,
        text,
        mid_turn: true,
        images: vec![],
    }]
}

/// Se replica la cola tal cual (FIFO); la UI sólo enseña las entradas que no son avisos internos.
fn parse_queue(v: &Value, ts: String) -> Vec<Item> {
    let text = v["content"].as_str().unwrap_or("").to_string();
    match v["operation"].as_str() {
        Some("enqueue") => {
            let system = text.trim_start().starts_with('<');
            vec![Item::QueueAdd { ts, text, system }]
        }
        Some("dequeue") => vec![Item::QueueDequeue { ts }],
        Some("remove") => vec![Item::QueueRemove { ts, text }],
        Some("popAll") => vec![Item::QueueClear { ts }],
        _ => vec![],
    }
}

/// Quita las marcas `<pasted_content id="…">…</pasted_content id="…">` con que Claude Code
/// envuelve los textos pegados (mensajes enviados como pegado por versiones anteriores).
fn strip_pasted(s: &str) -> String {
    if !s.contains("pasted_content") {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<pasted_content").or_else(|| rest.find("</pasted_content")) {
        let j = rest.find("</pasted_content").map_or(i, |j| j.min(i));
        let start = rest.find("<pasted_content").map_or(j, |k| k.min(j));
        out.push_str(&rest[..start]);
        match rest[start..].find('>') {
            Some(e) => rest = &rest[start + e + 1..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

fn agent_result(v: &Value, b: &Value, ts: &str) -> Item {
    let r = &v["toolUseResult"];
    // toolUseResult.content trae el informe limpio; las notas del harness van aparte.
    let mut text = r["content"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["text"].as_str())
                .filter(|t| !t.trim_start().starts_with("[Harness note"))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        text = result_text(&b["content"]);
        if let Some(i) = text.rfind("\nagentId: ") {
            text.truncate(i);
        }
    }
    let status = r["status"].as_str().unwrap_or("completed").to_string();
    if status == "async_launched" {
        text.clear();
    }
    Item::AgentResult {
        ts: ts.to_string(),
        tool_use_id: b["tool_use_id"].as_str().unwrap_or("").to_string(),
        agent_id: r["agentId"].as_str().unwrap_or("").to_string(),
        status,
        text: trunc(&text, 200_000),
        is_error: b["is_error"].as_bool().unwrap_or(false),
        model: r["resolvedModel"].as_str().map(String::from),
        duration_ms: r["totalDurationMs"].as_u64(),
        tokens: r["totalTokens"].as_u64(),
        tool_uses: r["totalToolUseCount"].as_u64(),
    }
}

fn task_notice(t: &str, ts: &str) -> Option<Item> {
    let tag = |k: &str| between(t, k).map(|s| s.trim().to_string()).unwrap_or_default();
    let usage = tag("usage");
    let num = |k: &str| between(&usage, k).and_then(|s| s.trim().parse::<u64>().ok());
    Some(Item::TaskNotice {
        ts: ts.to_string(),
        tool_use_id: tag("tool-use-id"),
        task_id: tag("task-id"),
        status: tag("status"),
        summary: tag("summary"),
        result: trunc(&tag("result"), 200_000),
        duration_ms: num("duration_ms"),
        tokens: num("subagent_tokens"),
        tool_uses: num("tool_uses"),
    })
}

fn parse_system(v: &Value, ts: String) -> Vec<Item> {
    match v["subtype"].as_str() {
        Some("compact_boundary") => vec![Item::Notice {
            ts,
            text: "Conversación compactada".into(),
        }],
        Some("local_command") => v["content"]
            .as_str()
            .and_then(|s| parse_user_text(s, &ts))
            .into_iter()
            .collect(),
        _ => vec![],
    }
}

pub fn tool_summary(name: &str, input: &Value) -> String {
    let g = |k: &str| input.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    let s = match name {
        "Bash" => g("description").or_else(|| g("command")),
        "Read" | "Write" | "Edit" | "MultiEdit" => g("file_path"),
        "NotebookEdit" => g("notebook_path"),
        "Grep" | "Glob" => g("pattern"),
        "WebFetch" => g("url"),
        "WebSearch" => g("query"),
        "Agent" | "Task" => g("description"),
        "Skill" => g("skill"),
        "AskUserQuestion" => input["questions"][0]["question"].as_str().map(String::from),
        "TodoWrite" => Some("lista de tareas".into()),
        _ => None,
    }
    .or_else(|| {
        input
            .as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str().map(|s| s.to_string())))
    })
    .unwrap_or_default();
    let first = s.lines().next().unwrap_or("").to_string();
    trunc_line(&first, 160)
}

fn tool_input(name: &str, input: &Value) -> String {
    let s = match name {
        "Bash" => input["command"].as_str().map(|s| s.to_string()),
        _ => None,
    }
    .unwrap_or_else(|| serde_json::to_string_pretty(input).unwrap_or_default());
    trunc(&s, MAX_INPUT)
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn trunc(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let end = floor_boundary(s, max);
    format!("{}\n… ({} bytes más)", &s[..end], s.len() - end)
}

fn trunc_line(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    format!("{}…", &s[..floor_boundary(s, max)])
}

/// Quita secuencias CSI/OSC de escape ANSI.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match it.peek() {
            Some('[') => {
                it.next();
                for d in it.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            }
            Some(']') => {
                it.next();
                while let Some(d) = it.next() {
                    if d == '\u{7}' {
                        break;
                    }
                    if d == '\u{1b}' && it.peek() == Some(&'\\') {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates() {
        assert_eq!(iso_ms("1970-01-01T00:00:00Z"), Some(0));
        // 19:36:56 en Madrid: «inactiva desde 1793731016» en el registro de ccdesk.
        assert_eq!(iso_ms("2026-11-03T18:36:56.123Z"), Some(1_793_731_016_123));
        assert_eq!(iso_ms("2026-11-03T18:36:56.5Z"), Some(1_793_731_016_500));
        assert_eq!(iso_ms("2026-11-03T19:36:56+01:00"), Some(1_793_731_016_000));
        assert_eq!(iso_ms("2024-02-29T12:00:00Z"), Some(1_709_208_000_000));
        assert_eq!(iso_ms("t"), None);
    }

    #[test]
    fn last_activity_tail() {
        let d = std::env::temp_dir().join(format!("ccdesk-last-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("s.jsonl");
        let mut lines = vec![
            r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"hola"},"uuid":"a","timestamp":"2026-11-03T18:00:00.000Z"}"#.to_string(),
            format!(r#"{{"parentUuid":"a","type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"{}"}}]}},"uuid":"b","timestamp":"2026-11-03T18:36:56.123Z"}}"#, "x".repeat(300)),
            // Lo que Claude Code añade al salir: sin fecha, o con una que no es actividad.
            r#"{"type":"last-prompt","lastPrompt":"hola"}"#.to_string(),
            r#"{"type":"pr-link","url":"u","timestamp":"2026-11-03T19:40:00.000Z"}"#.to_string(),
            r#"{"type":"cost-state","x":1}"#.to_string(),
        ];
        std::fs::write(&f, lines.join("\n") + "\n").unwrap();
        // Bloques de todos los tamaños: líneas partidas entre bloques y más largas que un bloque.
        for chunk in [1, 7, 64, 333, 1 << 16] {
            assert_eq!(last_activity_by(&f, chunk), Some(1_793_731_016_123), "bloque {chunk}");
        }
        // Un registro de conversación con `type` delante también cuenta.
        lines.push(r#"{"type":"system","subtype":"informational","uuid":"c","timestamp":"2026-11-03T18:40:00.000Z"}"#.into());
        lines.push(r#"{"type":"ai-title","aiTitle":"t"}"#.into());
        std::fs::write(&f, lines.join("\n") + "\n").unwrap();
        assert_eq!(last_activity_by(&f, 7), iso_ms("2026-11-03T18:40:00Z"));
        // Sin registros de conversación con fecha.
        std::fs::write(&f, "{\"type\":\"ai-title\",\"aiTitle\":\"t\"}\n").unwrap();
        assert_eq!(last_activity_by(&f, 7), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn assistant_with_nested_type_first() {
        let l = r#"{"parentUuid":"x","isSidechain":false,"message":{"id":"m1","type":"message","role":"assistant","content":[{"type":"text","text":"hola"}]},"type":"assistant","timestamp":"t"}"#;
        let v = parse_line(l);
        assert!(matches!(v.as_slice(), [Item::Assistant { text, .. }] if text == "hola"));
        // La carpeta en la que estaba (base de las rutas relativas que menciona).
        let l = r#"{"cwd":"/home/wzk/ws/ccdesk","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"en `src/app.css`"}]},"type":"assistant","timestamp":"t"}"#;
        assert!(matches!(parse_line(l).as_slice(), [Item::Assistant { cwd, .. }] if cwd == "/home/wzk/ws/ccdesk"));
    }

    /// CCDESK_TEST_IMAGES=<transcript.jsonl> cargo test --release images_real -- --nocapture
    #[test]
    fn images_real() {
        let Ok(f) = std::env::var("CCDESK_TEST_IMAGES") else { return };
        let p = PathBuf::from(f);
        let c = Cache::open("k", "s", &p).unwrap();
        let imgs: Vec<ImgRef> = c.items.iter().flat_map(|it| match it {
            Item::User { images, .. } | Item::ToolResult { images, .. } => images.clone(),
            _ => vec![],
        }).collect();
        println!("{} imágenes", imgs.len());
        for r in imgs.iter().take(4) {
            let u = image_at(&p, r.offset, r.n).unwrap();
            println!("  offset {} n {} -> {}… ({} KB)", r.offset, r.n, &u[..30], u.len() / 1024);
        }
    }

    #[test]
    fn images_located_and_loaded() {
        let d = std::env::temp_dir().join(format!("ccdesk-img-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("t.jsonl");
        let l1 = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"mira"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAA"}}]}}"#;
        let l2 = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"BBB"}},{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"CCC"}}]}]}}"#;
        std::fs::write(&p, format!("{l1}\n{l2}\n")).unwrap();
        let c = Cache::open("k", "s", &p).unwrap();
        let imgs: Vec<&ImgRef> = c.items.iter().flat_map(|it| match it {
            Item::User { images, .. } | Item::ToolResult { images, .. } => images.iter().collect::<Vec<_>>(),
            _ => vec![],
        }).collect();
        assert_eq!(imgs.len(), 3);
        assert!(matches!(&c.items[0], Item::User { text, images, .. } if text == "mira" && images.len() == 1));
        let urls: Vec<String> = imgs.iter().map(|r| image_at(&p, r.offset, r.n).unwrap()).collect();
        assert_eq!(urls, ["data:image/png;base64,AAA", "data:image/jpeg;base64,BBB", "data:image/jpeg;base64,CCC"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn compact_command_hidden() {
        let cmd = r#"{"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name>\n <command-message>compact</command-message>\n <command-args></command-args>"}}"#;
        assert!(parse_line(cmd).is_empty());
        let out = r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>\u001b[2mCompacted (ctrl+o to see full summary)\u001b[22m</local-command-stdout>"}}"#;
        assert!(parse_line(out).is_empty());
        let with_args = r#"{"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name><command-args>céntrate en X</command-args>"}}"#;
        assert!(matches!(parse_line(with_args).as_slice(), [Item::Command { .. }]));
        let other = r#"{"type":"user","message":{"role":"user","content":"<command-name>/model</command-name><command-args></command-args>"}}"#;
        assert!(matches!(parse_line(other).as_slice(), [Item::Command { .. }]));
    }

    #[test]
    fn skips_aux_records() {
        assert!(parse_line(r#"{"type":"ai-title","aiTitle":"x"}"#).is_empty());
        assert!(parse_line(r#"{"type":"api-request-blob","message":{"role":"user","content":"x"}}"#).is_empty());
    }

    #[test]
    fn user_command_and_output() {
        let sum = r#"{"type":"user","isCompactSummary":true,"isVisibleInTranscriptOnly":true,"message":{"role":"user","content":"This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n<system-reminder>\n<tools-used-before-summary>Bash</tools-used-before-summary>\n</system-reminder>\nSummary:\n1. Intent:\n   hacer X\n\nIf you need specific details from before compaction, read the full transcript at: /x.jsonl\nContinue the conversation from where it left off.\n\nThe messages after this summary are verbatim."},"timestamp":"t"}"#;
        assert!(matches!(parse_line(sum).as_slice(), [Item::Summary { text, .. }] if text == "1. Intent:\n   hacer X"));
        let c = r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name>\n<command-args>foco</command-args>"},"timestamp":"t"}"#;
        assert!(matches!(parse_line(c).as_slice(), [Item::Command { name, args, .. }] if name == "/compact" && args == "foco"));
        let o = r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"<local-command-stdout>\u001b[1mok\u001b[0m</local-command-stdout>"},"timestamp":"t"}"#;
        assert!(matches!(parse_line(o).as_slice(), [Item::Output { text, .. }] if text == "ok"));
    }

    #[test]
    fn queued_mid_turn() {
        let add = r#"{"type":"queue-operation","operation":"enqueue","timestamp":"t","content":"hola"}"#;
        assert!(matches!(parse_line(add).as_slice(), [Item::QueueAdd { text, system: false, .. }] if text == "hola"));
        let rm = r#"{"type":"queue-operation","operation":"remove","timestamp":"t","content":"hola","reason":"absorbed_mid_turn"}"#;
        assert!(matches!(parse_line(rm).as_slice(), [Item::QueueRemove { text, .. }] if text == "hola"));
        let att = r#"{"parentUuid":"p","isSidechain":false,"attachment":{"type":"queued_command","prompt":"hola","origin":{"kind":"human"},"humanTurn":true},"type":"attachment","timestamp":"t"}"#;
        assert!(matches!(parse_line(att).as_slice(), [Item::User { text, mid_turn: true, .. }] if text == "hola"));
        // los avisos internos se marcan como system; dequeue no trae texto
        let tn = r#"{"type":"queue-operation","operation":"enqueue","timestamp":"t","content":"<task-notification>\n<task-id>x</task-id>"}"#;
        assert!(matches!(parse_line(tn).as_slice(), [Item::QueueAdd { system: true, .. }]));
        let dq = r#"{"type":"queue-operation","operation":"dequeue","timestamp":"t"}"#;
        assert!(matches!(parse_line(dq).as_slice(), [Item::QueueDequeue { .. }]));
    }

    #[test]
    fn agents() {
        let call = r#"{"parentUuid":"p","isSidechain":false,"message":{"id":"m","type":"message","role":"assistant","content":[{"type":"tool_use","id":"tu1","name":"Agent","input":{"description":"Buscar","subagent_type":"general-purpose","model":"sonnet","effort":"low","prompt":"haz X","run_in_background":true}}]},"type":"assistant","timestamp":"t"}"#;
        assert!(matches!(parse_line(call).as_slice(), [Item::Agent { id, background: true, model, .. }] if id == "tu1" && model == "sonnet/low"));
        let res = r#"{"parentUuid":"p","isSidechain":false,"type":"user","message":{"role":"user","content":[{"tool_use_id":"tu1","type":"tool_result","content":[{"type":"text","text":"informe\nagentId: a1 (use SendMessage)"}]}]},"toolUseResult":{"status":"completed","agentId":"a1","content":[{"type":"text","text":"informe"},{"type":"text","text":"[Harness note, x]"}],"totalDurationMs":1000,"totalTokens":50,"totalToolUseCount":3},"timestamp":"t"}"#;
        assert!(matches!(parse_line(res).as_slice(), [Item::AgentResult { text, tool_uses: Some(3), .. }] if text == "informe"));
        let note = r#"{"parentUuid":"p","isSidechain":false,"attachment":{"type":"queued_command","prompt":"<task-notification>\n<task-id>a1</task-id>\n<tool-use-id>tu1</tool-use-id>\n<status>completed</status>\n<summary>Agent \"Buscar\" finished</summary>\n<result>hecho</result>\n<usage><subagent_tokens>99</subagent_tokens><tool_uses>4</tool_uses><duration_ms>2000</duration_ms></usage>\n</task-notification>","origin":{"kind":"task-notification"}},"type":"attachment","timestamp":"t"}"#;
        assert!(matches!(parse_line(note).as_slice(), [Item::TaskNotice { tool_use_id, result, tokens: Some(99), .. }] if tool_use_id == "tu1" && result == "hecho"));
    }

    #[test]
    fn pasted_tags() {
        assert_eq!(strip_pasted("\n\n<pasted_content id=\"36\">\nhola\nmundo\n</pasted_content id=\"36\">\n"), "hola\nmundo");
        assert_eq!(strip_pasted("sin marcas"), "sin marcas");
    }

    #[test]
    fn handback() {
        let l = r#"{"parentUuid":"p","isSidechain":false,"attachment":{"type":"queued_command","prompt":"<agent-message from=\"a1\">x</agent-message>","origin":{"kind":"peer","from":"a1","handback":true,"handbackReportText":"informe final"}},"type":"attachment","uuid":"u9","timestamp":"t"}"#;
        assert!(matches!(parse_line(l).as_slice(), [Item::AgentReport { agent_id, text, .. }] if agent_id == "a1" && text == "informe final"));
    }

    /// CCDESK_TEST_JSONL=<fichero> cargo test real_file -- --nocapture
    #[test]
    fn real_file() {
        let Ok(p) = std::env::var("CCDESK_TEST_JSONL") else { return };
        let c = Cache::open("k", "s", Path::new(&p)).unwrap();
        let mut counts = std::collections::BTreeMap::new();
        for it in &c.items {
            let k = serde_json::to_value(it).unwrap()["kind"].as_str().unwrap().to_string();
            *counts.entry(k).or_insert(0) += 1;
        }
        println!("{counts:?}");
        assert!(counts.contains_key("assistant"));
    }
}
