mod app;
mod browser;
mod commands;
mod download;
mod jobs;
mod library;
mod platform;
mod storage;

use app::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Set only when the user picks Quit from the tray. While it is false the
/// close button hides to the tray instead of terminating the app.
static IS_QUITTING: AtomicBool = AtomicBool::new(false);
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, WindowEvent};

pub use browser::{
    BrowserDownloadRequest, BrowserRequestOptions, new_browser_download_request,
    stage_browser_request,
};
pub use platform::{resolve_app_data_dir, resolve_default_download_dir};


/// Restores and focuses the existing main window. Never creates a window: the
/// whole point is to bring back the one instance that is already running.
fn focus_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn run() {
    let first_run = platform::is_first_run();
    let start_hidden = std::env::args().any(|arg| arg == "--background");
    platform::run_first_time_setup();

    let state = AppState::bootstrap().unwrap_or_else(|error| {
        panic!("failed to initialize application state: {error}");
    });

    tauri::Builder::default()
        // Must stay first: a second launch focuses the running window instead
        // of starting a duplicate engine with its own download state.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !argv.iter().any(|arg| arg == "--background") { focus_main_window(app); }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let state = app_handle.state::<AppState>();
            state.restore_download_queue(&app_handle)?;

            let show_item =
                MenuItemBuilder::with_id("show", "Open Linux Download Manager").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&show_item)
                .separator()
                .item(&quit_item)
                .build()?;

            // Dedicated flat/monochrome icon for the system tray — the
            // full-colour app icon turns into mush at 16–22 px.
            let icon_bytes = include_bytes!("../icons/tray-icon.png");
            let tray_icon = Image::from_bytes(icon_bytes)?;

            let handle_for_tray = app_handle.clone();
            TrayIconBuilder::new()
                .icon(tray_icon)
                .tooltip("Linux Download Manager")
                .menu(&tray_menu)
                .on_menu_event(move |_tray, event| {
                    match event.id().as_ref() {
                        "show" => focus_main_window(&handle_for_tray),
                        "quit" => {
                            // Mark the shutdown as intentional so any pending
                            // close event is allowed to proceed.
                            IS_QUITTING.store(true, Ordering::Relaxed);
                            handle_for_tray.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event({
                    let handle = app_handle.clone();
                    move |_tray, event| {
                        if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, .. } = event {
                            focus_main_window(&handle);
                        }
                    }
                })
                .build(app)?;

            for label in ["main", "add-download", "download-detail"] {
                let handle_for_window = app_handle.clone();
                let label_for_window = label.to_string();
                if let Some(window) = app.get_webview_window(label) {
                    window.on_window_event(move |event| {
                        if let WindowEvent::CloseRequested { api, .. } = event {
                            if IS_QUITTING.load(Ordering::Relaxed) {
                                return;
                            }
                            // Close means "hide to tray": the window disappears
                            // but the download engine, queue, browser inbox
                            // poller and tray icon all keep running.
                            api.prevent_close();
                            if let Some(w) = handle_for_window.get_webview_window(&label_for_window) {
                                let _ = w.hide();
                            }
                        }
                    });
                }
            }

            if (first_run || !start_hidden)
                && let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }

            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let state = app_handle.state::<AppState>();
                    let _ = state.poll_browser_inbox(&app_handle).await;
                    let _ = state.schedule_pending(&app_handle);
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::inspect_url,
            commands::list_downloads,
            commands::open_download_folder,
            commands::start_download,
            commands::pick_save_directory,
            commands::app_settings,
            commands::update_app_settings,
            commands::pause_download,
            commands::resume_download,
            commands::cancel_download,
            commands::clear_completed,
            commands::clear_download,
            commands::delete_download_files,
            commands::system_status,
            commands::show_add_download_window,
            commands::show_download_detail_window,
            commands::hide_app_window,
            commands::download_detail
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
