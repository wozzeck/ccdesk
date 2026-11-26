//! Memoria del sistema (`/proc/meminfo`) y la de cada sesión: su proceso y todos sus descendientes
//! (Claude Code, sus servidores MCP y lo que lance), en RAM y en swap.
use std::collections::HashMap;

/// En bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct System {
    pub total: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

/// Valor en kB de una línea `Clave:   1234 kB` de los ficheros de /proc, en bytes.
fn kb_line(text: &str, key: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.split(':').next() == Some(key))?;
    let n = line.split(':').nth(1)?.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(n * 1024)
}

pub fn parse_meminfo(text: &str) -> Option<System> {
    Some(System {
        total: kb_line(text, "MemTotal")?,
        available: kb_line(text, "MemAvailable")?,
        swap_total: kb_line(text, "SwapTotal").unwrap_or(0),
        swap_free: kb_line(text, "SwapFree").unwrap_or(0),
    })
}

pub fn system() -> Option<System> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

/// Memoria de un proceso (o de varios sumados), en bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    /// En RAM (VmRSS).
    pub ram: u64,
    /// En swap (VmSwap).
    pub swap: u64,
}

/// Padre y memoria de un proceso, de su `/proc/<pid>/status`. Los hilos del núcleo no tienen
/// VmRSS: cuentan 0.
fn parse_status(text: &str) -> Option<(u32, Usage)> {
    let ppid = text.lines().find_map(|l| l.strip_prefix("PPid:"))?.trim().parse().ok()?;
    let ram = kb_line(text, "VmRSS").unwrap_or(0);
    let swap = kb_line(text, "VmSwap").unwrap_or(0);
    Some((ppid, Usage { ram, swap }))
}

/// Memoria de cada raíz sumando la de todos sus descendientes. `procs`: pid → (padre, memoria).
pub fn sum_trees(procs: &HashMap<u32, (u32, Usage)>, roots: &[u32]) -> HashMap<u32, Usage> {
    let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, &(ppid, _)) in procs {
        kids.entry(ppid).or_default().push(pid);
    }
    roots
        .iter()
        .filter(|r| procs.contains_key(r))
        .map(|&root| {
            let (mut total, mut stack) = (Usage::default(), vec![root]);
            while let Some(p) = stack.pop() {
                if let Some((_, u)) = procs.get(&p) {
                    total.ram += u.ram;
                    total.swap += u.swap;
                }
                stack.extend(kids.get(&p).into_iter().flatten().copied());
            }
            (root, total)
        })
        .collect()
}

/// Memoria de cada proceso de `roots` con sus descendientes. RSS suma también las páginas
/// compartidas (el binario de Claude Code, p. ej.): es una cota, no lo que se libera exacto.
pub fn trees(roots: &[u32]) -> HashMap<u32, Usage> {
    let procs: HashMap<u32, (u32, Usage)> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let text = std::fs::read_to_string(e.path().join("status")).ok()?;
            Some((pid, parse_status(&text)?))
        })
        .collect();
    sum_trees(&procs, roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo() {
        let t = "MemTotal:       32517120 kB\nMemFree:         7686144 kB\nMemAvailable:   13590528 kB\nSwapTotal:      17775612 kB\nSwapFree:        3103744 kB\n";
        let s = parse_meminfo(t).unwrap();
        assert_eq!(s.total, 32517120 * 1024);
        assert_eq!(s.available, 13590528 * 1024);
        assert_eq!((s.swap_total, s.swap_free), (17775612 * 1024, 3103744 * 1024));
        assert!(parse_meminfo("MemTotal: 1 kB\n").is_none());
    }

    #[test]
    fn status_and_trees() {
        let st = "Name:\tclaude\nPPid:\t1947800\nVmRSS:\t  331776 kB\nVmSwap:\t   20480 kB\n";
        assert_eq!(parse_status(st), Some((1947800, Usage { ram: 331776 * 1024, swap: 20480 * 1024 })));
        assert_eq!(parse_status("Name:\tkthreadd\nPPid:\t0\n"), Some((0, Usage::default())));
        // 10 → 11 → 12, y 13 suelto; 99 no existe.
        let u = |ram, swap| Usage { ram, swap };
        let procs = HashMap::from([(10, (1, u(100, 1))), (11, (10, u(20, 2))), (12, (11, u(3, 0))), (13, (1, u(7, 7)))]);
        let t = sum_trees(&procs, &[10, 11, 99]);
        assert_eq!(t.get(&10), Some(&u(123, 3)));
        assert_eq!(t.get(&11), Some(&u(23, 2)));
        assert!(!t.contains_key(&99));
    }
}
