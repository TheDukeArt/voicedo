mod asr;
mod hotkey;
mod recorder;
mod settings;
mod tray;
mod typer;

use tauri::{AppHandle, WindowEvent};
use tauri_plugin_notification::NotificationExt;

fn notify(app: &AppHandle, message: &str) {
    let _ = app
        .notification()
        .builder()
        .title("VoiceDo")
        .body(message)
        .show();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .manage(recorder::Recorder::default())
        .invoke_handler(tauri::generate_handler![
            settings::get_settings,
            settings::save_settings,
            asr::test_connection
        ])
        .setup(|app| {
            tray::build_tray(app.handle())?;
            let s = settings::load_settings(app.handle());
            // Битый hotkey в store не должен ломать запуск: фолбэк на ОС-дефолт.
            let (to_register, warning) =
                hotkey::startup_hotkey(&s.hotkey, &settings::Settings::default().hotkey);
            if let Some(w) = &warning {
                eprintln!("[setup] WARNING: {w} (в store: «{}»)", s.hotkey);
                notify(app.handle(), w);
            }
            if let Some(h) = &to_register {
                if let Err(e) = hotkey::apply_hotkey(app.handle(), h) {
                    eprintln!("[setup] {e}");
                    notify(app.handle(), &e);
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Закрытие окна настроек сворачивает приложение в трей, не завершая процесс.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
