//! Una sola copia de ccdesk. Si se abre otra (menú, terminal) con una ya en marcha, se lo dice a
//! la primera, que trae su ventana al frente, y termina sin cargar nada: dos copias se pisarían
//! `state.json` y repetirían los avisos.
//!
//! Se hablan por un socket Unix del espacio abstracto: el núcleo lo suelta al morir el proceso,
//! sin ficheros viejos que limpiar ni carreras al hacerlo. El nombre sale de la carpeta de
//! configuración, así que una copia con otro `HOME` (pruebas) va por su cuenta.

use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

pub enum Claim {
    /// No había otra: esta es la principal y atiende aquí a las que se abran después.
    First(UnixListener),
    /// Había otra y ya ha traído su ventana al frente.
    Raised,
    /// Hay otra, pero no contesta.
    NoAnswer,
}

/// Se queda con el nombre o, si lo tiene otra copia, le pide que se muestre y le pasa el
/// identificador de arranque del lanzador. Si la otra no confirma (se está cerrando), reintenta
/// hasta poder quedárselo; si sigue sin contestar, `NoAnswer`.
pub fn claim(config: &Path, startup_id: Option<&str>) -> io::Result<Claim> {
    claim_as(&name(config), startup_id, Duration::from_secs(3), Duration::from_secs(6))
}

/// Nombre del socket: FNV-1a de la carpeta de configuración (estable entre compilaciones, al
/// contrario que `DefaultHasher`).
fn name(config: &Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in config.as_os_str().as_bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    format!("ccdesk-{h:016x}")
}

fn claim_as(name: &str, startup_id: Option<&str>, ack: Duration, give_up: Duration) -> io::Result<Claim> {
    let addr = SocketAddr::from_abstract_name(name)?;
    let deadline = Instant::now() + give_up;
    loop {
        match UnixListener::bind_addr(&addr) {
            Ok(l) => return Ok(Claim::First(l)),
            Err(e) if e.kind() == ErrorKind::AddrInUse => {}
            Err(e) => return Err(e),
        }
        if ask_raise(&addr, startup_id, ack).is_ok() {
            return Ok(Claim::Raised);
        }
        if Instant::now() >= deadline {
            return Ok(Claim::NoAnswer);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn ask_raise(addr: &SocketAddr, startup_id: Option<&str>, ack: Duration) -> io::Result<()> {
    let mut s = UnixStream::connect_addr(addr)?;
    s.set_write_timeout(Some(Duration::from_secs(1)))?;
    s.set_read_timeout(Some(ack))?;
    let id = startup_id.filter(|id| valid_id(id)).unwrap_or("");
    s.write_all(format!("{id}\n").as_bytes())?;
    let mut answer = [0u8; 3];
    s.read_exact(&mut answer)?;
    if &answer == b"ok\n" {
        Ok(())
    } else {
        Err(ErrorKind::InvalidData.into())
    }
}

/// Identificador de arranque aceptable: ASCII visible y corto (viaja en una línea).
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.bytes().all(|b| b.is_ascii_graphic())
}

/// Atiende a las copias que se abran después. `raise` recibe su identificador de arranque y dice
/// si la ventana ha salido; solo entonces se confirma. Si no (esta copia se está cerrando), la
/// otra reintenta y acaba quedándose el nombre.
pub fn serve(listener: UnixListener, raise: impl Fn(Option<String>) -> bool + Send + 'static) {
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut s) = conn else { continue };
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let mut line = String::new();
            if BufReader::new(&s).take(512).read_line(&mut line).is_err() {
                continue;
            }
            let id = line.trim();
            if raise(valid_id(id).then(|| id.to_string())) {
                let _ = s.write_all(b"ok\n");
            }
        }
    });
}

/// Trae al frente la ventana principal porque se ha abierto otra copia. Se hace en el hilo de
/// GTK y se espera a que termine: si esta copia se está cerrando no llega a hacerse y no se
/// confirma.
pub fn bring_to_front(app: &AppHandle, startup_id: Option<String>) -> bool {
    let (tx, rx) = mpsc::channel();
    let h = app.clone();
    let queued = app.run_on_main_thread(move || {
        let _ = tx.send(show_main(&h, startup_id.as_deref()));
    });
    queued.is_ok() && rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false)
}

/// El foco se pide con la hora de la pulsación que abrió la otra copia (va en el identificador
/// de arranque del menú), y el gestor de ventanas lo concede; al recibir ese identificador, GTK
/// cierra además el aviso de «iniciando» del lanzador. Sin él (desde una terminal) se pide con
/// hora 0, que muffin no filtra. `set_focus` de Tauri no sirve: GTK pone la hora del último uso
/// de ccdesk y, si después se ha usado otra ventana, muffin solo hace parpadear su botón.
fn show_main(app: &AppHandle, startup_id: Option<&str>) -> bool {
    use gtk::prelude::*;
    let Some(w) = app.get_webview_window("main") else { return false };
    let _ = w.unminimize();
    let _ = w.show();
    let Ok(gw) = w.gtk_window() else { return w.set_focus().is_ok() };
    if let Some(id) = startup_id {
        gw.set_startup_id(id);
    }
    match gw.window() {
        Some(win) => {
            win.show();
            win.focus(startup_id.and_then(startup_time).unwrap_or(0));
        }
        None => gw.present(),
    }
    true
}

/// Hora (del servidor X) de la pulsación: las cifras que siguen al último `_TIME` del
/// identificador de arranque, como las lee GTK.
pub fn startup_time(id: &str) -> Option<u32> {
    let (_, t) = id.rsplit_once("_TIME")?;
    let digits = &t[..t.bytes().take_while(u8::is_ascii_digit).count()];
    digits.parse::<u32>().ok().filter(|&t| t != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(tag: &str) -> String {
        format!("ccdesk-test-{}-{tag}", std::process::id())
    }

    #[test]
    fn time_from_startup_id() {
        assert_eq!(startup_time("cinnamon-1234-tarkan-ccdesk-0_TIME98765"), Some(98765));
        assert_eq!(startup_time("x_TIME12abc"), Some(12));
        assert_eq!(startup_time("a_TIME1-b_TIME2"), Some(2));
        assert_eq!(startup_time("x_TIME0"), None);
        assert_eq!(startup_time("x_TIME"), None);
        assert_eq!(startup_time("sin-hora"), None);
    }

    #[test]
    fn name_depends_on_config_dir() {
        let a = name(Path::new("/home/a/.config/ccdesk"));
        assert_eq!(a, name(Path::new("/home/a/.config/ccdesk")));
        assert_ne!(a, name(Path::new("/tmp/prueba/.config/ccdesk")));
        assert!(a.len() < 100);
    }

    #[test]
    fn second_copy_raises_the_first() {
        let n = unique("raise");
        let ack = Duration::from_secs(2);
        let wait = Duration::from_secs(3);
        let Claim::First(l) = claim_as(&n, None, ack, wait).unwrap() else {
            panic!("la primera debe quedarse el nombre");
        };
        let (tx, rx) = mpsc::channel();
        serve(l, move |id| tx.send(id).is_ok());
        // Desde el menú: llega el identificador de arranque.
        assert!(matches!(claim_as(&n, Some("menu-1_TIME42"), ack, wait).unwrap(), Claim::Raised));
        assert_eq!(rx.recv().unwrap().as_deref(), Some("menu-1_TIME42"));
        // Desde una terminal (sin identificador), o con uno que no vale: llega None.
        assert!(matches!(claim_as(&n, None, ack, wait).unwrap(), Claim::Raised));
        assert_eq!(rx.recv().unwrap(), None);
        assert!(matches!(claim_as(&n, Some("con espacio\n"), ack, wait).unwrap(), Claim::Raised));
        assert_eq!(rx.recv().unwrap(), None);
        // Con otra carpeta de configuración es otra copia independiente.
        assert!(matches!(claim_as(&unique("otra"), None, ack, wait).unwrap(), Claim::First(_)));
    }

    #[test]
    fn takes_over_when_the_first_is_closing() {
        // La principal recibe el aviso pero no confirma (se está cerrando) y luego desaparece.
        let n = unique("closing");
        let Claim::First(l) = claim_as(&n, None, Duration::from_secs(1), Duration::from_secs(1)).unwrap()
        else {
            panic!("la primera debe quedarse el nombre");
        };
        let first = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            std::thread::sleep(Duration::from_millis(300));
            drop(s);
            drop(l);
        });
        let t0 = Instant::now();
        let got = claim_as(&n, None, Duration::from_secs(1), Duration::from_secs(5)).unwrap();
        assert!(matches!(got, Claim::First(_)), "debe quedarse el nombre al irse la otra");
        assert!(t0.elapsed() < Duration::from_secs(3));
        first.join().unwrap();
    }

    #[test]
    fn hung_first_gives_no_answer() {
        // La principal tiene el nombre pero no atiende: no se abre otra copia.
        let n = unique("hung");
        let Claim::First(_l) = claim_as(&n, None, Duration::from_millis(100), Duration::ZERO).unwrap()
        else {
            panic!("la primera debe quedarse el nombre");
        };
        let got = claim_as(&n, None, Duration::from_millis(100), Duration::from_millis(500)).unwrap();
        assert!(matches!(got, Claim::NoAnswer));
    }
}
