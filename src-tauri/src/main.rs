mod service;

use service::{CleanupReport, ScanProgress, Scanner, Snapshot};
use std::sync::{Arc, Mutex};

type AppState = Arc<Mutex<Scanner>>;

async fn run_operation<T: Send + 'static>(
    state: AppState,
    operation: impl FnOnce(&mut Scanner) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut scanner = state
            .try_lock()
            .map_err(|_| "正在扫描或清理，请等待当前操作完成。".to_owned())?;
        operation(&mut scanner)
    })
    .await
    .map_err(|error| format!("后台任务失败：{error}"))?
}

#[tauri::command]
async fn scan(
    state: tauri::State<'_, AppState>,
    on_progress: tauri::ipc::Channel<ScanProgress>,
) -> Result<Snapshot, String> {
    run_operation(state.inner().clone(), move |scanner| {
        scanner.scan(|event| on_progress.send(event).map_err(|error| error.to_string()))
    })
    .await
}

#[tauri::command]
async fn disk_access() -> Result<String, String> {
    // Independent from the scan mutex, so a large directory cannot delay permission feedback.
    tauri::async_runtime::spawn_blocking(|| {
        let home = clean_my_mac::home_dir().map_err(|error| error.to_string())?;
        Ok(clean_my_mac::detect_full_disk_access(&home)
            .label()
            .to_owned())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn clean(
    ids: Vec<String>,
    confirmed: bool,
    state: tauri::State<'_, AppState>,
) -> Result<CleanupReport, String> {
    run_operation(state.inner().clone(), move |scanner| {
        scanner.clean(&ids, confirmed)
    })
    .await
}

#[tauri::command]
async fn open_settings() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        clean_my_mac::open_full_disk_access_settings().map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

fn main() {
    if !cfg!(target_os = "macos") {
        eprintln!("Clean My Mac 仅支持 macOS");
        std::process::exit(1);
    }
    let home = clean_my_mac::home_dir().expect("无法确定用户主目录");
    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(Scanner::new(home))))
        .invoke_handler(tauri::generate_handler![
            scan,
            disk_access,
            clean,
            open_settings
        ])
        .run(tauri::generate_context!())
        .expect("无法启动 Clean My Mac");
}
