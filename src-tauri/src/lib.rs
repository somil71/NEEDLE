use std::path::PathBuf;
use std::process::Child;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, WebviewUrl, WebviewWindowBuilder,
};

static SERVER: Mutex<Option<Child>> = Mutex::new(None);

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Start the needle server in the background
            let needle_bin = resolve_needle_binary(app.handle());
            let child = std::process::Command::new(&needle_bin)
                .args(["serve", "--port", "7700", "--no-open"])
                .spawn()
                .expect("Failed to start Needle server");
            *SERVER.lock().unwrap() = Some(child);

            // Build system-tray menu
            let open_item = MenuItem::with_id(app, "open", "Open Needle", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_item, &quit_item])?;

            TrayIconBuilder::new()
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_or_create_window(app),
                    "quit" => {
                        kill_server();
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                        show_or_create_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // Open the window once the server is ready
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                wait_for_server();
                create_main_window(&handle);
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // Minimize to tray instead of closing
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().unwrap();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("Error running Needle desktop app");
}

fn wait_for_server() {
    for _ in 0..50 {
        if reqwest::blocking::get("http://127.0.0.1:7700/").is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn create_main_window(handle: &tauri::AppHandle) {
    WebviewWindowBuilder::new(
        handle,
        "main",
        WebviewUrl::External("http://localhost:7700".parse().unwrap()),
    )
    .title("Needle")
    .inner_size(1280.0, 820.0)
    .min_inner_size(900.0, 600.0)
    .center()
    .visible(true)
    .build()
    .expect("Failed to create Needle window");
}

fn show_or_create_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        window.show().unwrap();
        window.set_focus().unwrap();
    } else {
        create_main_window(app);
    }
}

fn kill_server() {
    if let Some(mut child) = SERVER.lock().unwrap().take() {
        let _ = child.kill();
    }
}

fn resolve_needle_binary(handle: &tauri::AppHandle) -> PathBuf {
    // Production: binary lives next to the app in a resources/ dir
    #[cfg(not(debug_assertions))]
    if let Ok(resource_dir) = handle.path().resource_dir() {
        let candidate = resource_dir.join(if cfg!(windows) {
            "needle.exe"
        } else {
            "needle"
        });
        if candidate.exists() {
            return candidate;
        }
    }

    // Development: use the workspace's release binary
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap();
    workspace_root.join("target").join("release").join(if cfg!(windows) {
        "needle.exe"
    } else {
        "needle"
    })
}
