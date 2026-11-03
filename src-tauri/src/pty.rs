//! Terminal embebida: un cliente `tmux attach` dentro de un pty, volcado a xterm.js.
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use tauri::ipc::Channel;

use crate::tmux;

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl Pty {
    /// `argv`: orden de tmux para engancharse (ver `Tmux::attach_argv`).
    pub fn spawn(
        argv: &[String],
        cols: u16,
        rows: u16,
        on_data: Channel<String>,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> Result<Pty, String> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows.max(2),
                cols: cols.max(10),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let (prog, args) = argv.split_first().ok_or("orden vacía")?;
        let mut cmd = CommandBuilder::new(prog);
        cmd.args(args);
        for k in tmux::strip_env_keys() {
            cmd.env_remove(k);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        std::thread::spawn(move || {
            let mut buf = [0u8; 16384];
            let mut carry: Vec<u8> = Vec::new();
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                carry.extend_from_slice(&buf[..n]);
                // No partir secuencias UTF-8 entre envíos.
                let cut = match std::str::from_utf8(&carry) {
                    Ok(_) => carry.len(),
                    Err(e) if e.error_len().is_none() => e.valid_up_to(),
                    Err(_) => carry.len(),
                };
                if cut == 0 {
                    continue;
                }
                let chunk: Vec<u8> = carry.drain(..cut).collect();
                if on_data
                    .send(String::from_utf8_lossy(&chunk).into_owned())
                    .is_err()
                {
                    break;
                }
            }
            on_exit();
        });

        Ok(Pty {
            master: pair.master,
            writer,
            child,
        })
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), String> {
        self.writer.write_all(data).map_err(|e| e.to_string())?;
        self.writer.flush().map_err(|e| e.to_string())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.master
            .resize(PtySize {
                rows: rows.max(2),
                cols: cols.max(10),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Mata sólo el cliente tmux (detach); la sesión sigue viva en el servidor.
        let _ = self.child.kill();
    }
}
