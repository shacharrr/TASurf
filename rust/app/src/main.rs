//! TASurf as a desktop app: the editor in its own window, on Windows, macOS and
//! Linux. The Rust back end (the same one `tasurf editor` runs) serves the editor
//! page on 127.0.0.1 inside this process, and the window shows it. The page is the
//! same file a browser gets (tasurf/editor_static/index.html); the app needs no
//! Python and no browser.
//!
//! It opens on the Setup page (rust/src/launch.rs behind it): where the game, SVR
//! and the project are, which map and window size, and Launch (the game through SVR);
//! then Edit and Export.
//!
//!     tasurf-editor [timeline]

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::Value;
use tasurf::{config, editor, launch, py};
use tauri::webview::PageLoadEvent;
use tauri::window::Color;
use tauri::{AppHandle, Manager, Theme, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

/// A fixed port keeps the page's address, and with it what the page remembers
/// (the Visuals toolbox, auto-preview). 8765 is left to `tasurf editor` in a
/// browser. If it's taken, any free port. TASURF_APP_PORT overrides it.
const PORT: u16 = 8768;

/// Navigating here closes the app (the page can't call into the app otherwise).
const CLOSE_PATH: &str = "/__tasurf_close__";

/// A closing window doesn't run the page's unsaved-changes check, so the app asks
/// the page first: the same question, then the navigation above.
const CLOSE_SCRIPT: &str = r#"(() => {
  try {
    if (typeof S !== "undefined" && S.dirty) {
      if (!confirm("Discard unsaved changes and close TASurf?")) return;
      S.dirty = false;
    }
  } catch (e) { }
  location.replace("/__tasurf_close__");
})();"#;

/// The editor page has loaded (else a close just closes).
static EDITOR_LOADED: AtomicBool = AtomicBool::new(false);

/// The page's own background (--bg in index.html): the window shows it before the page draws, not white.
const BACKGROUND: Color = Color(0x14, 0x16, 0x1a, 0xff);

/// Tells the page it's in the app, not a browser: it then keeps the browser's right-click menu, the mouse's back
/// and forward buttons and file drops to itself (index.html, TASURF_APP).
const APP_SCRIPT: &str = "window.TASURF_APP = true;";

/// An editor, not a web page: the browser's keyboard shortcuts (reload, print, find, caret browsing, back and
/// forward...) are off. Typing and editing keys, and the page's own shortcuts, work as before.
#[cfg(windows)]
fn app_like(window: &WebviewWindow) {
    let _ = window.with_webview(|webview| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
        use windows_core::Interface;
        let settings = webview.controller().CoreWebView2().and_then(|core| core.Settings());
        if let Ok(s3) = settings.and_then(|s| s.cast::<ICoreWebView2Settings3>()) {
            let _ = s3.SetAreBrowserAcceleratorKeysEnabled(false.into());
        }
    });
}

#[cfg(not(windows))]
fn app_like(_window: &WebviewWindow) {}

/// The window as an app's: a dark title bar, the page's background behind it, no page zoom (Ctrl+wheel, pinch) and
/// no form autofill.
fn window_builder<'a>(app: &'a tauri::App, url: WebviewUrl) -> WebviewWindowBuilder<'a, tauri::Wry, tauri::App> {
    WebviewWindowBuilder::new(app, "main", url)
        .title("TASurf")
        .theme(Some(Theme::Dark))
        .background_color(BACKGROUND)
        .zoom_hotkeys_enabled(false)
        .general_autofill_enabled(false)
        .initialization_script(APP_SCRIPT)
}

/// Start the editor's server on a background thread; returns its port and the editor.
fn start_editor() -> Result<u16, String> {
    let root = config::repo_root();
    let found = std::env::var("TASURF_ROOT").map_or(false, |r| !r.is_empty())
        || root.join("plugin").join("scripting").join("tasurf.sp").is_file();
    if !found {
        return Err(format!("No TASurf folder found around {} or around this app.", root.display()));
    }
    let port = std::env::var("TASURF_APP_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(PORT);
    let listener = editor::make_server(port)
        .or_else(|_| editor::make_server(0))
        .map_err(|e| format!("Couldn't open a local port for the editor: {}", e))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    launch::ensure_project();
    let ed = Arc::new(editor::Editor::new(launch::timelines_dir(), config::load(), None));
    std::thread::spawn(move || editor::serve_on(listener, ed));
    Ok(port)
}

/// Navigations a window may make: the editor itself and the app's own pages.
/// Links anywhere else open in the system browser.
fn allow(url: &Url, port: u16, app: &AppHandle) -> bool {
    let ours = url.scheme() == "http" && url.host_str() == Some("127.0.0.1") && url.port() == Some(port);
    if ours && url.path() == CLOSE_PATH {
        let app = app.clone();
        std::thread::spawn(move || app.exit(0));
        return false;
    }
    if ours || matches!(url.scheme(), "tauri" | "about" | "data") || url.host_str() == Some("tauri.localhost") {
        return true;
    }
    if matches!(url.scheme(), "http" | "https") {
        editor::open_browser(url.as_str());
    }
    false
}

fn page_url(port: u16, page: &str, fragment: &str) -> Url {
    let mut url = format!("http://127.0.0.1:{}/{}", port, page);
    if !fragment.is_empty() {
        url.push('#');
        url.push_str(fragment);
    }
    url.parse().expect("a local address")
}

/// The window: the page opens on Setup, or on Edit with a timeline (the fragment). It stays hidden until the
/// page has loaded (its splash then shows at once), or for a few seconds at most.
fn open_editor(app: &tauri::App, port: u16, fragment: &str) -> tauri::Result<()> {
    let handle = app.handle().clone();
    let window = window_builder(app, WebviewUrl::External(page_url(port, "", fragment)))
        .on_navigation(move |url| allow(url, port, &handle))
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Finished {
                EDITOR_LOADED.store(payload.url().host_str() == Some("127.0.0.1"), Ordering::SeqCst);
                if !window.is_visible().unwrap_or(true) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .inner_size(1440.0, 900.0)
        .min_inner_size(1000.0, 640.0)
        .center()
        .visible(false)
        .disable_drag_drop_handler() // the page's own mouse drags, as in a browser
        .build()?;
    app_like(&window);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        if !window.is_visible().unwrap_or(true) {
            let _ = window.show();
        }
    });
    Ok(())
}

/// The game a launch started takes the focus when its window opens: once its map is loaded (the plugin's
/// heartbeat is newer than the launch), the editor gets it back, with the game behind it.
fn refocus(app: AppHandle) {
    let mut done = 0.0;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let st = launch::launch_state();
        let started = st.get("started").and_then(Value::as_f64).unwrap_or(0.0);
        if st.get("state").and_then(Value::as_str) != Some("running") || started == done {
            continue;
        }
        let beat = config::load().game_data_dir().map(|d| d.join("requests").join("heartbeat.txt"));
        let fresh = beat
            .and_then(|b| std::fs::metadata(b).and_then(|m| m.modified()).ok())
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map_or(false, |m| m.as_secs_f64() > started + 1.0);
        if fresh {
            done = started;
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
            }
        }
    }
}

fn main() {
    let started = start_editor();
    std::thread::spawn(launch::tidy); // Metamod off again after a session the app didn't see end
    // a timeline to open, as `tasurf editor` takes it (a name or a path)
    let timeline = std::env::args().nth(1).map(|t| editor::stem(&config::pypath(&t))).unwrap_or_default();

    tauri::Builder::default()
        .setup(move |app| {
            match &started {
                Ok(port) => {
                    open_editor(app, *port, &timeline)?;
                    let handle = app.handle().clone();
                    std::thread::spawn(move || refocus(handle));
                }
                Err(why) => {
                    window_builder(app, WebviewUrl::App("index.html".into()))
                        .initialization_script(&format!("window.TASURF_ERROR = {};", py::dumps(&Value::String(why.clone()), true)))
                        .inner_size(1440.0, 900.0)
                        .center()
                        .build()?;
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && EDITOR_LOADED.load(Ordering::SeqCst) {
                    if let Some(w) = window.app_handle().get_webview_window("main") {
                        if w.eval(CLOSE_SCRIPT).is_ok() {
                            api.prevent_close();
                        }
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running TASurf");
}
