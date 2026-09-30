mod app;
mod browser;
mod commands;
mod download;
mod jobs;
mod platform;
mod single_instance;
mod storage;

use app::AppState;
use std::time::Duration;
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, WindowEvent};

pub use browser::{BrowserDownloadRequest, new_browser_download_request, stage_browser_request};
pub use platform::{resolve_app_data_dir, resolve_default_download_dir};
pub use single_instance::try_activate_existing_instance;

pub fn run() {
    platform::run_first_time_setup();

    let state = AppState::bootstrap().unwrap_or_else(|error| {
        panic!("failed to initialize application state: {error}");
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let state = app_handle.state::<AppState>();
            state.restore_download_queue(&app_handle)?;

            single_instance::start_single_instance_listener(app_handle.clone());

            let show_item = MenuItemBuilder::with_id("show", "Show Linux Download Manager").build(app)?;
            let add_item = MenuItemBuilder::with_id("add", "Add New Download...").build(app)?;
            let settings_item = MenuItemBuilder::with_id("settings", "Settings...").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&show_item)
                .item(&add_item)
                .separator()
                .item(&settings_item)
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
                        "show" => {
                            if let Some(window) = handle_for_tray.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                            }
                        }
                        "add" => {
                            if let Some(window) = handle_for_tray.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                                let _ = window.emit("tray-action", "add");
                            }
                        }
                        "settings" => {
                            if let Some(window) = handle_for_tray.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                                let _ = window.emit("tray-action", "settings");
                            }
                        }
                        "quit" => {
                            single_instance::cleanup_socket();
                            handle_for_tray.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event({
                    let handle = app_handle.clone();
                    move |_tray, event| {
                        if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, .. } = event {
                            if let Some(window) = handle.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            let handle_for_window = app_handle.clone();
            if let Some(window) = app.get_webview_window("main") {
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Some(w) = handle_for_window.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                });
            }

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
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
            commands::start_download,
            commands::pick_save_directory,
            commands::app_settings,
            commands::update_app_settings,
            commands::pause_download,
            commands::resume_download,
            commands::cancel_download,
            commands::clear_completed,
            commands::system_status,
            commands::delete_download,
            commands::open_file,
            commands::open_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
