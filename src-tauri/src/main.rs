#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::Manager;
use tokio::sync::Mutex;

mod lib;
use lib::AppState;

fn main() {
    let state = AppState {
        server_shutdown: Arc::new(AtomicBool::new(false)),
        save_dir: Arc::new(Mutex::new(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )),
        transfers: Arc::new(Mutex::new(Vec::new())),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                lib::run_server(handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            lib::get_local_info,
            lib::send_files,
            lib::get_transfers,
            lib::cancel_transfer,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
