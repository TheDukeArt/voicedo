use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_store::StoreExt;

const STORE_PATH: &str = "settings.json";
const STORE_KEY: &str = "settings";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub endpoint: String,
    pub token: String,
    pub model: String,
    pub language: String,
    pub hotkey: String,
    pub insert_delay_ms: u64,
    /// Автостарт при входе в систему. false по умолчанию — старые store-файлы
    /// без ключа не должны включать его.
    pub autostart: bool,
    /// Тема интерфейса: "system" | "light" | "dark".
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Имя входного аудиоустройства (cpal); пусто — системное по умолчанию.
    #[serde(default)]
    pub input_device: String,
}

fn default_provider() -> String {
    "openai".to_string()
}

fn default_theme() -> String {
    "system".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            endpoint: String::new(),
            token: String::new(),
            model: "whisper-1".to_string(),
            language: String::new(),
            hotkey: if cfg!(target_os = "macos") {
                "Cmd+Shift+Space".to_string()
            } else {
                "Ctrl+Shift+Space".to_string()
            },
            insert_delay_ms: 50,
            autostart: false,
            theme: default_theme(),
            input_device: String::new(),
        }
    }
}

pub fn load_settings(app: &AppHandle) -> Settings {
    app.store(STORE_PATH)
        .ok()
        .and_then(|store| store.get(STORE_KEY))
        .and_then(|value| serde_json::from_value::<Settings>(value).ok())
        .unwrap_or_default()
}

#[tauri::command]
pub async fn get_settings(app: AppHandle) -> Settings {
    load_settings(&app)
}

/// Привести состояние плагина автостарта в соответствие настройке
/// (tauri-plugin-autostart, macOS: LaunchAgent).
pub fn sync_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    let result = if enabled { manager.enable() } else { manager.disable() };
    match result {
        Ok(()) => {
            log::info!("[autostart] синхронизирован: enabled={enabled}");
            Ok(())
        }
        Err(e) => {
            log::error!("[autostart] не удалось применить (enabled={enabled}): {e}");
            Err(format!("Не удалось изменить автостарт: {e}"))
        }
    }
}

#[tauri::command]
pub async fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let prev = load_settings(&app);
    // Валидация хоткея ДО записи в store: невалидное значение отклоняется целиком,
    // в store остаётся прежнее, прежний хоткей продолжает работать.
    crate::hotkey::parse_hotkey(&settings.hotkey)?;
    let store = app.store(STORE_PATH).map_err(|e| e.to_string())?;
    let value = serde_json::to_value(&settings).map_err(|e| e.to_string())?;
    store.set(STORE_KEY.to_string(), value);
    store.save().map_err(|e| e.to_string())?;
    // Hotkey валиден; регистрация отдельно (например, сочетание занято другим приложением).
    crate::hotkey::apply_hotkey(&app, &settings.hotkey)?;
    // Автостарт меняем только если значение реально изменилось.
    if prev.autostart != settings.autostart {
        sync_autostart(&app, settings.autostart)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_store_without_autostart_defaults_to_false() {
        let old = r#"{
            "provider": "qwen",
            "endpoint": "https://example.com/api",
            "token": "t",
            "model": "m",
            "language": "ru",
            "hotkey": "Shift+v",
            "insertDelayMs": 50
        }"#;
        let s: Settings = serde_json::from_str(old).expect("old settings should deserialize");
        assert!(!s.autostart, "старые настройки не должны включать автостарт");
        assert_eq!(s.model, "m");
    }

    #[test]
    fn old_store_without_theme_defaults_to_system() {
        let old = r#"{
            "provider": "openai",
            "endpoint": "https://api.openai.com/v1",
            "token": "t",
            "model": "whisper-1",
            "language": "ru",
            "hotkey": "Cmd+Shift+Space",
            "insertDelayMs": 50,
            "autostart": false
        }"#;
        let s: Settings = serde_json::from_str(old).expect("store without theme should deserialize");
        assert_eq!(s.theme, "system");
        // и по умолчанию, и round-trip
        assert_eq!(Settings::default().theme, "system");
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["theme"], "system");
    }

    #[test]
    fn old_store_without_input_device_defaults_to_empty() {
        let old = r#"{
            "provider": "openai",
            "endpoint": "https://api.openai.com/v1",
            "token": "t",
            "model": "whisper-1",
            "language": "ru",
            "hotkey": "Cmd+Shift+Space",
            "insertDelayMs": 50,
            "autostart": false,
            "theme": "dark"
        }"#;
        let s: Settings = serde_json::from_str(old).expect("store without inputDevice should deserialize");
        assert!(s.input_device.is_empty());
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["inputDevice"], "");
    }

    #[test]
    fn autostart_roundtrips() {
        let s = Settings {
            autostart: true,
            ..Default::default()
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["autostart"], true);
        let back: Settings = serde_json::from_value(json).unwrap();
        assert!(back.autostart);
    }
}
