mod app;
mod notify;
mod paths;
mod pty;
mod registry;
mod search;
mod slash;
mod state;
mod tmux;
mod transcript;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

/// Último cambio de tamaño/posición de la ventana (ms); se guarda ~1 s después.
static LAST_GEOMETRY_CHANGE: AtomicU64 = AtomicU64::new(0);

/// Lo que se recuerda de la ventana: todo menos la decoración (va sin ella, con botones propios).
fn window_flags() -> StateFlags {
    StateFlags::all() & !StateFlags::DECORATIONS
}

fn main() {
    // Sin compositing por GPU WebKitGTK ocupa ~25 MB menos; esta UI no lo necesita.
    if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }
    app::ensure_conf();
    let persist = state::load();
    let core = Arc::new(app::Core {
        tmux: tmux::Tmux::new(paths::tmux_conf(), &persist.settings.tmux_session),
        claude: paths::claude_bin(),
        persist: Mutex::new(persist),
        active: Mutex::new(None),
        tcache: Mutex::new(None),
        pty: Mutex::new(None),
        last_snap: Mutex::new(String::new()),
        views: Mutex::new(Vec::new()),
        agents: Mutex::new(std::collections::HashMap::new()),
        hosts: Mutex::new(std::collections::HashMap::new()),
        running_agents: Mutex::new(std::collections::HashMap::new()),
        agent_dirs: Mutex::new(std::collections::HashMap::new()),
        modes: Mutex::new(std::collections::HashMap::new()),
        started_at: app::now_ms(),
        opts: Mutex::new(app::OptsCache::default()),
        usage: Mutex::new((None, None)),
        search: Mutex::new(search::Index::default()),
    });
    let poller_core = core.clone();

    tauri::Builder::default()
        // Restaura tamaño, posición y maximizado al arrancar.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(window_flags())
                .build(),
        )
        .on_window_event(|_window, event| {
            if matches!(
                event,
                tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Moved(_)
            ) {
                LAST_GEOMETRY_CHANGE.store(app::now_ms(), Ordering::Relaxed);
            }
        })
        .manage(core)
        .setup(move |app| {
            app::spawn_poller(app.handle().clone(), poller_core.clone());
            // Índice de búsqueda en las conversaciones (hilo propio, al día cada 10 s) y precarga
            // de los comandos de Claude Code, en segundo plano y ya con la lista hecha.
            search::spawn_indexer(poller_core.clone());
            let claude = poller_core.claude.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(4));
                slash::warm(&claude);
            });
            // Lista de modelos del desplegable: si Claude Code ha cambiado de versión, se lee ya
            // (con una sesión oculta, unos segundos) para que al abrirlo esté.
            let models_core = poller_core.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(8));
                if let Err(e) = app::models_now(&models_core) {
                    app::log(&format!("lista de modelos: {e}"));
                }
            });
            // Zoom guardado (Ctrl+rueda), antes de que se vea la interfaz.
            let zoom = poller_core.persist.lock().unwrap().settings.zoom;
            if zoom.is_finite() && (zoom - 1.0).abs() > 0.01 {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.set_zoom(zoom.clamp(0.5, 2.5));
                }
            }
            // SIGTERM/SIGINT/SIGHUP (kill, apagado, cierre de sesión) → salida ordenada: la
            // ventana se desmapea antes de destruirse. Si el proceso muere de golpe, la lista de
            // ventanas de Cinnamon puede quedarse con una entrada fantasma (X reutiliza el XID).
            let exit_handle = app.handle().clone();
            std::thread::spawn(move || {
                use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
                let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP])
                else {
                    return;
                };
                if signals.forever().next().is_some() {
                    let _ = exit_handle.save_window_state(window_flags());
                    exit_handle.exit(0);
                }
            });
            // El plugin sólo guarda al cerrar la ventana; si el proceso muere (kill, cierre de
            // sesión) se perdería. Se guarda también tras cada cambio, cuando se estabiliza.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut saved = 0;
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    let t = LAST_GEOMETRY_CHANGE.load(Ordering::Relaxed);
                    if t != saved && app::now_ms().saturating_sub(t) > 800 {
                        let _ = handle.save_window_state(window_flags());
                        saved = t;
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app::snapshot_now,
            app::transcript_open,
            app::transcript_before,
            app::transcript_find,
            app::session_new,
            app::session_stop,
            app::session_resume,
            app::session_adopt,
            app::session_adopt_all,
            app::session_forget,
            app::session_rename,
            app::set_layout,
            app::set_pinned,
            app::set_unread,
            app::set_exempt,
            app::settings_update,
            app::send_input,
            app::send_when_ready,
            app::files_exist,
            app::open_file,
            app::image_file,
            app::send_keys,
            app::session_set_model,
            app::session_set_effort,
            app::session_set_mode,
            app::session_models,
            app::session_efforts,
            app::session_modes,
            app::usage_refresh,
            app::session_restart,
            app::term_open,
            app::term_write,
            app::term_resize,
            app::term_close,
            app::open_url,
            app::open_path,
            app::open_scratchpad,
            app::set_zoom,
            app::prompt_history,
            app::agent_activity,
            app::agent_transcript,
            app::notes_get,
            app::notes_save,
            app::search_sessions,
            app::slash_commands,
            app::transcript_image,
            app::save_image,
            app::clipboard_image,
            app::resolve_paths,
        ])
        .run(tauri::generate_context!())
        .expect("error al arrancar ccdesk");
}
