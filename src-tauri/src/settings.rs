use serde::{Deserialize, Serialize};
use tauri::AppHandle;
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
}

fn default_provider() -> String {
    "openai".to_string()
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

#[tauri::command]
pub async fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    // Валидация хоткея ДО записи в store: невалидное значение отклоняется целиком,
    // в store остаётся прежнее, прежний хоткей продолжает работать.
    crate::hotkey::parse_hotkey(&settings.hotkey)?;
    let store = app.store(STORE_PATH).map_err(|e| e.to_string())?;
    let value = serde_json::to_value(&settings).map_err(|e| e.to_string())?;
    store.set(STORE_KEY.to_string(), value);
    store.save().map_err(|e| e.to_string())?;
    // Hotkey валиден; регистрация отдельно (например, сочетание занято другим приложением).
    crate::hotkey::apply_hotkey(&app, &settings.hotkey)
}
