mod logic;
mod download;
mod tools;

use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::UpdaterExt;

#[tauri::command]
fn read_clipboard(app: AppHandle) -> Option<String> {
    app.clipboard().read_text().ok()
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
fn default_dir(app: AppHandle) -> String {
    app.path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map(|d| d.join("Grabber").to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Opens the folder and highlights the file when we know it.
#[tauri::command]
fn reveal(app: AppHandle, file: Option<String>, dir: String) -> Result<(), String> {
    if let Some(f) = file.filter(|f| std::path::Path::new(f).exists()) {
        if tauri_plugin_opener::reveal_item_in_dir(&f).is_ok() {
            return Ok(());
        }
    }
    let _ = std::fs::create_dir_all(&dir);
    app.opener().open_path(dir, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
fn busy(app: AppHandle) -> bool {
    download::any_running(&app)
}

#[tauri::command]
async fn app_update_check(app: AppHandle) -> Result<Option<String>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(u)) => Ok(Some(u.version.clone())),
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn app_update_install(app: AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // a second launch just brings the open window forward
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(download::Jobs::default())
        .invoke_handler(tauri::generate_handler![
            tools::tools_status,
            tools::install_tools,
            tools::update_engine,
            tools::reinstall_tool,
            download::probe,
            download::start_download,
            download::cancel_download,
            read_clipboard,
            pick_folder,
            default_dir,
            reveal,
            app_version,
            busy,
            app_update_check,
            app_update_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Grabber");
}
