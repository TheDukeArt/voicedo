use std::str::FromStr;
use std::time::Instant;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;

use crate::{asr, recorder, settings, tray, typer};

/// Разбор строки хоткея из настроек (вид «Cmd+Shift+Space») в Shortcut.
pub fn parse_hotkey(value: &str) -> Result<Shortcut, String> {
    Shortcut::from_str(value.trim())
        .map_err(|e| format!("Не удалось разобрать хоткей «{value}»: {e}"))
}

/// Нормализованное представление уже зарегистрированного хоткея (для проверки
/// «менялся ли» при автосохранении настроек) + сам Shortcut для точечного
/// unregister прежнего.
static REGISTERED: std::sync::Mutex<Option<(String, Shortcut)>> = std::sync::Mutex::new(None);

fn normalized(value: &str) -> String {
    value.split('+').map(|t| t.trim().to_uppercase()).collect::<Vec<_>>().join("+")
}

/// Что регистрировать при старте: (hotkey или None, предупреждение или None).
/// Битое значение в store не должно ломать запуск: регистрируется ОС-дефолт,
/// пользователю показывается предупреждение.
pub fn startup_hotkey(stored: &str, os_default: &str) -> (Option<String>, Option<String>) {
    if parse_hotkey(stored).is_ok() {
        return (Some(stored.to_string()), None);
    }
    if parse_hotkey(os_default).is_ok() {
        (
            Some(os_default.to_string()),
            Some(format!(
                "Хоткей в настройках некорректен — используется «{os_default}», исправьте в окне настроек"
            )),
        )
    } else {
        (
            None,
            Some(
                "Хоткей в настройках некорректен и системный дефолт не парсится — автозапись отключена"
                    .to_string(),
            ),
        )
    }
}

/// Перерегистрация: сначала пробуем зарегистрировать новый хоткей и только затем
/// снимаем прежний — при неудачной регистрации (сочетание занято) старый продолжает работать.
/// Ошибка возвращается при непарсящейся строке или недоступности регистрации.
pub fn apply_hotkey(app: &AppHandle, value: &str) -> Result<(), String> {
    let shortcut = parse_hotkey(value)?;
    let wanted = normalized(value);
    let gs = app.global_shortcut();
    {
        let current = REGISTERED.lock().map(|g| g.clone()).unwrap_or(None);
        if current.as_ref().is_some_and(|(n, _)| *n == wanted) && gs.is_registered(shortcut.clone())
        {
            return Ok(());
        }
    }
    let prev = REGISTERED.lock().ok().and_then(|mut g| g.take());
    let result = gs.on_shortcut(shortcut, |app, _shortcut, event| match event.state {
        ShortcutState::Pressed => on_pressed(app),
        ShortcutState::Released => on_released(app),
    });
    match result {
        Ok(()) => {
            if let Some((_, old)) = &prev {
                let _ = gs.unregister(old.clone());
            }
            if let Ok(mut g) = REGISTERED.lock() {
                *g = Some((wanted, shortcut));
            }
            println!("[hotkey] hotkey registered: {value}");
            Ok(())
        }
        Err(e) => {
            if let (Ok(mut g), Some(p)) = (REGISTERED.lock(), prev) {
                *g = Some(p);
            }
            Err(format!("Не удалось зарегистрировать хоткей «{value}»: {e}"))
        }
    }
}

fn on_pressed(app: &AppHandle) {
    let recorder_state = app.state::<recorder::Recorder>();
    // Защита от Pressed без предыдущего Released: повторное нажатие игнорируется.
    if recorder_state.is_recording() {
        println!("[hotkey] pressed while already recording — ignored");
        return;
    }
    match recorder_state.start() {
        Ok(()) => {
            tray::set_tray_state(app, tray::TrayState::Recording);
            println!("[hotkey] recording started");
        }
        Err(e) => {
            eprintln!("[hotkey] failed to start recording: {e}");
            notify(app, &e.to_string());
        }
    }
}

/// Предпроверка настроек перед отправкой в ASR (чистая функция, для тестов).
pub fn should_transcribe(s: &settings::Settings) -> Result<(), &'static str> {
    if s.endpoint.trim().is_empty() || s.token.trim().is_empty() {
        return Err("Настройки ASR не заполнены — укажите эндпоинт и токен в окне настроек");
    }
    Ok(())
}

fn on_released(app: &AppHandle) {
    let recorder_state = app.state::<recorder::Recorder>();
    match recorder_state.stop() {
        Some(recorded) => {
            println!(
                "[hotkey] recording stopped: {:.2} s, {} raw samples @ {} Hz, WAV {} bytes",
                recorded.duration.as_secs_f64(),
                recorded.raw_samples,
                recorded.device_rate,
                recorded.wav.len()
            );
            dispatch_to_asr(app, recorded.wav);
        }
        None => {
            // Released без активной записи — на всякий случай гасим индикатор.
            tray::set_tray_state(app, tray::TrayState::Ready);
            println!("[hotkey] released without active recording — ignored");
        }
    }
}

/// Отправка записанного WAV на распознавание в фоновой задаче.
/// Вставка текста — этап 5; здесь только лог/уведомление/состояние трея.
fn dispatch_to_asr(app: &AppHandle, wav: Vec<u8>) {
    let s = settings::load_settings(app);
    if let Err(reason) = should_transcribe(&s) {
        eprintln!("[asr] skipped: {reason}");
        tray::set_tray_state(app, tray::TrayState::Ready);
        notify(app, reason);
        return;
    }
    tray::set_tray_state(app, tray::TrayState::Processing);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let provider = asr::Provider::from_str(&s.provider).unwrap_or_default();
        let started = Instant::now();
        let result =
            asr::transcribe(provider, &s.endpoint, &s.token, &s.model, &s.language, &wav).await;
        match result {
            Ok(text) => {
                println!(
                    "[asr] recognized ({} ms): «{text}»",
                    started.elapsed().as_millis()
                );
                // 5.3: вставка, если текст непустой (пусто/NoSpeech — не вставляем).
                if typer::should_insert(&text) {
                    let delay = s.insert_delay_ms;
                    let chars = text.chars().count();
                    let app_t = app.clone();
                    match tauri::async_runtime::spawn_blocking(move || {
                        typer::insert_text(&text, delay)
                    })
                    .await
                    {
                        Ok(Ok(())) => {
                            println!("[typer] inserted {chars} chars (delay {delay} ms)")
                        }
                        Ok(Err(e)) => {
                            eprintln!("[typer] failed: {e}");
                            notify(&app_t, &format!("Не удалось вставить текст: {e}"));
                        }
                        Err(e) => eprintln!("[typer] spawn_blocking failed: {e}"),
                    }
                }
            }
            Err(e) => {
                eprintln!("[asr] error: {e}");
                let msg = match e {
                    asr::AsrError::NoSpeech => "Речь не распознана — в записи тишина".to_string(),
                    other => format!("Распознавание не удалось: {other}"),
                };
                notify(&app, &msg);
            }
        }
        tray::set_tray_state(&app, tray::TrayState::Ready);
    });
}

fn notify(app: &AppHandle, message: &str) {
    let _ = app
        .notification()
        .builder()
        .title("VoiceDo")
        .body(message)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_plugin_global_shortcut::{Code, Modifiers};

    #[test]
    fn parses_macos_default() {
        let sc = parse_hotkey("Cmd+Shift+Space").expect("should parse");
        assert_eq!(sc, Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Space));
    }

    #[test]
    fn parses_windows_default() {
        let sc = parse_hotkey("Ctrl+Shift+Space").expect("should parse");
        assert_eq!(
            sc,
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space)
        );
    }

    #[test]
    fn parses_ctrl_alt_fn() {
        let sc = parse_hotkey("Ctrl+Alt+F12").expect("should parse");
        assert_eq!(
            sc,
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::F12)
        );
    }

    #[test]
    fn tolerates_whitespace_and_case() {
        assert!(parse_hotkey(" cmd +shift+ space ").is_ok());
    }

    #[test]
    fn shift_with_key_parses_but_trailing_plus_is_broken() {
        assert!(parse_hotkey("Shift+v").is_ok());
        assert!(parse_hotkey("Shift+").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_hotkey("Cmd+Shift+").is_err());
        assert!(parse_hotkey("Shift+").is_err());
        assert!(parse_hotkey("").is_err());
        assert!(parse_hotkey("Cmd+Shift+A+B").is_err());
    }

    #[test]
    fn startup_uses_stored_when_valid() {
        let (reg, warning) = startup_hotkey("Ctrl+Alt+F12", "Cmd+Shift+Space");
        assert_eq!(reg.as_deref(), Some("Ctrl+Alt+F12"));
        assert!(warning.is_none());
    }

    #[test]
    fn startup_falls_back_to_os_default_on_broken_stored() {
        let (reg, warning) = startup_hotkey("Shift+", "Cmd+Shift+Space");
        assert_eq!(reg.as_deref(), Some("Cmd+Shift+Space"));
        let w = warning.expect("should warn");
        assert!(w.contains("некорректен"), "{w}");
        assert!(w.contains("Cmd+Shift+Space"), "{w}");
    }

    #[test]
    fn startup_without_any_valid_hotkey_disables_recording() {
        let (reg, warning) = startup_hotkey("Shift+", "");
        assert!(reg.is_none());
        assert!(warning.expect("should warn").contains("автозапись отключена"));
    }

    fn settings_with(endpoint: &str, token: &str) -> settings::Settings {
        settings::Settings {
            endpoint: endpoint.to_string(),
            token: token.to_string(),
            ..settings::Settings::default()
        }
    }

    #[test]
    fn should_transcribe_requires_endpoint_and_token() {
        assert!(should_transcribe(&settings_with("  ", "tok")).is_err());
        assert!(should_transcribe(&settings_with("https://x/v1", "   ")).is_err());
        assert!(should_transcribe(&settings_with("", "")).is_err());
        assert!(should_transcribe(&settings_with("https://x/v1", "tok")).is_ok());
    }

    #[test]
    fn should_transcribe_error_message_is_user_facing() {
        let err = should_transcribe(&settings_with("", "")).unwrap_err();
        assert!(err.contains("Настройки ASR не заполнены"), "{err}");
    }
}
