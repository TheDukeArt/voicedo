use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

use crate::recorder::Recorder;

pub const TRAY_ID: &str = "voicedo-main-tray";
const SHOW_ID: &str = "show-settings";
const QUIT_ID: &str = "quit";
#[cfg(debug_assertions)]
const DEBUG_CLEAR_ID: &str = "debug-clear-indicator";

pub fn show_settings_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Ready,
    Recording,
    Processing,
}

/// Состояния трея: запись «●», обработка «…», готов — пусто.
///
/// ВАЖНО (tray-icon 0.24.2, platform_impl/macos/mod.rs `set_title_inner`):
/// на macOS ветка `None` — no-op, заголовок НЕ очищается. Сбрасывается именно
/// пустой строкой `Some("")`. Потокобезопасность обеспечена самим tauri:
/// TrayIcon::set_title/set_tooltip работают через `run_item_main_thread!`
/// (внутри `app.run_on_main_thread`), дополнительных обёрток не нужно.
pub fn set_tray_state(app: &AppHandle, state: TrayState) {
    // Гонка с фоновым ASR: «готов» из завершившейся обработки не должен
    // перетирать индикатор начавшейся в это время записи.
    if state == TrayState::Ready && app.state::<Recorder>().is_recording() {
        return;
    }
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        eprintln!("[tray] indicator: tray `{TRAY_ID}` not found");
        return;
    };
    let (title, tooltip) = match state {
        TrayState::Ready => ("", "VoiceDo"),
        TrayState::Recording => ("●", "Идёт запись…"),
        TrayState::Processing => ("…", "Распознаю…"),
    };
    if let Err(e) = tray
        .set_title(Some(title))
        .and_then(|_| tray.set_tooltip(Some(tooltip)))
    {
        eprintln!("[tray] set_title failed: {e}");
        return;
    }
    println!("[tray] state -> {state:?}");
}

pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, SHOW_ID, "Показать настройки", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, QUIT_ID, "Выход", true, None::<&str>)?;
    #[cfg(debug_assertions)]
    let debug_clear_item =
        MenuItem::with_id(app, DEBUG_CLEAR_ID, "Отладка: снять индикатор", true, None::<&str>)?;
    #[cfg(debug_assertions)]
    let menu = Menu::with_items(app, &[&show_item, &debug_clear_item, &quit_item])?;
    #[cfg(not(debug_assertions))]
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| std::io::Error::other("no default window icon"))?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("VoiceDo")
        .on_menu_event(|app, event| match event.id.as_ref() {
            SHOW_ID => show_settings_window(app),
            QUIT_ID => {
                // Полное завершение процесса, минуя veto на закрытие окна.
                app.exit(0);
            }
            #[cfg(debug_assertions)]
            DEBUG_CLEAR_ID => set_tray_state(app, TrayState::Ready),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_settings_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}
