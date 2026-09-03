use tauri::{
    image::Image,
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
        log::error!("[tray] indicator: tray `{TRAY_ID}` not found");
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
        log::error!("[tray] set_title failed: {e}");
        return;
    }
    log::info!("[tray] state -> {state:?}");
}

/// Тултип с сегодняшней статистикой — после успешной диктовки.
pub fn set_stats_tooltip(app: &AppHandle, words_today: u64) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    if let Err(e) = tray.set_tooltip(Some(&format!(
        "VoiceDo — сегодня: {}",
        plural_words(words_today)
    ))) {
        log::error!("[tray] set_tooltip failed: {e}");
    }
}

/// Русская плюрализация: 1 слово, 2 слова, 5 слов.
fn plural_words(n: u64) -> String {
    let m10 = n % 10;
    let m100 = n % 100;
    let word = if m10 == 1 && m100 != 11 {
        "слово"
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        "слова"
    } else {
        "слов"
    };
    format!("{n} {word}")
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

    // Фирменный глиф-волна: на macOS — template (чёрный+альфа), система сама
    // инвертирует его для тёмной темы меню-бара. На Windows template не
    // поддерживается — берём белую версию (видна и на светлом, и на тёмном трее).
    let icon = if cfg!(windows) {
        Image::from_bytes(include_bytes!("../../assets/tray/voicedo_tray_white_44.png"))
    } else {
        Image::from_bytes(include_bytes!("../../assets/tray/voicedo_tray_44.png"))
    }
    .map_err(|e| std::io::Error::other(format!("tray icon: {e}")))?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(!cfg!(windows))
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

#[cfg(test)]
mod tests {
    use super::plural_words;

    #[test]
    fn pluralization_cases() {
        assert_eq!(plural_words(1), "1 слово");
        assert_eq!(plural_words(2), "2 слова");
        assert_eq!(plural_words(5), "5 слов");
        assert_eq!(plural_words(11), "11 слов");
        assert_eq!(plural_words(21), "21 слово");
        assert_eq!(plural_words(112), "112 слов");
        assert_eq!(plural_words(1234), "1234 слова");
        assert_eq!(plural_words(0), "0 слов");
    }
}
