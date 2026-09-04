//! Единый каталог строк для Rust и фронта (`src/lib/i18n/*.json`, один JSON на
//! язык). Rust берёт `tray.*`, `err.*`, `notify.*`; `ui.*` — фронт.
//! Плейсхолдеры `{name}` в стиле `format!`, подстановка — своя (не `format!`:
//! ключ и язык неизвестны на этапе компиляции).
//! Фолбэки: отсутствующий ключ или неизвестный язык (zh) → EN; при промахе в EN
//! возвращается сам ключ.

use std::sync::{OnceLock, RwLock};

use serde_json::Value;

const EN_JSON: &str = include_str!("../../src/lib/i18n/en.json");
const RU_JSON: &str = include_str!("../../src/lib/i18n/ru.json");

fn catalogs() -> &'static (Value, Value) {
    static CATALOGS: OnceLock<(Value, Value)> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        let en: Value = serde_json::from_str(EN_JSON).expect("en.json must be valid JSON");
        let ru: Value = serde_json::from_str(RU_JSON).expect("ru.json must be valid JSON");
        (en, ru)
    })
}

fn locale_cell() -> &'static RwLock<String> {
    static LOCALE: OnceLock<RwLock<String>> = OnceLock::new();
    LOCALE.get_or_init(|| RwLock::new("en".to_string()))
}

/// Активная разрешённая локаль: "en" | "ru" | "zh".
pub fn locale() -> String {
    locale_cell()
        .read()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "en".to_string())
}

/// Строка "auto" → конкретная локаль по системной локали (sys-locale:
/// macOS — AppleLocale, вида "ru_RU"/"ru-RU"); прочее — "en", "zh" — "zh"
/// (каталога пока нет, t() даёт EN-фолбэк).
pub fn resolve(setting: &str) -> String {
    match setting {
        "en" | "ru" | "zh" => setting.to_string(),
        _ => {
            let sys = sys_locale::get_locale().unwrap_or_default();
            let tag = sys.replace('_', "-").to_lowercase();
            let prefix = tag.split('-').next().unwrap_or("");
            match prefix {
                "ru" => "ru".to_string(),
                "zh" => "zh".to_string(),
                _ => "en".to_string(),
            }
        }
    }
}

/// Установить разрешённую локаль; true, если она реально изменилась.
pub fn set_locale(resolved: &str) -> bool {
    let cell = locale_cell();
    let mut current = match cell.write() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if *current == resolved {
        false
    } else {
        *current = resolved.to_string();
        true
    }
}

fn lookup<'a>(catalog: &'a Value, key: &str) -> Option<&'a str> {
    let mut cur = catalog;
    for part in key.split('.') {
        cur = cur.get(part)?;
    }
    cur.as_str()
}

fn expand(raw: &str, params: &[(&str, &str)]) -> String {
    let mut out = raw.to_string();
    for (name, value) in params {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Перевод по ключу с подстановкой `{name}`; фолбэк на EN.
pub fn t(key: &str, params: &[(&str, &str)]) -> String {
    t_in(&locale(), key, params)
}

pub fn t_in(loc: &str, key: &str, params: &[(&str, &str)]) -> String {
    let (en, ru) = catalogs();
    let current = if loc == "ru" { ru } else { en };
    let raw = lookup(current, key)
        .or_else(|| lookup(en, key))
        .unwrap_or(key);
    expand(raw, params)
}

/// Категория плюрализации CLDR: RU — one/few/many (1/2-4/5+ с учётом 11-14),
/// EN — one/other, ZH — всегда other.
pub fn plural_category(loc: &str, n: u64) -> &'static str {
    match loc {
        "ru" => {
            let m10 = n % 10;
            let m100 = n % 100;
            if m10 == 1 && m100 != 11 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }
        "zh" => "other",
        _ => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
    }
}

/// Локализованное «{n} <форма слова>»: форма берётся из объекта
/// `{key}.one/.few/.many/.other`; для zh — EN-фолбэк.
pub fn plural_in(loc: &str, key: &str, n: u64) -> String {
    let (en, ru) = catalogs();
    let order: [&Value; 2] = if loc == "ru" { [ru, en] } else { [en, en] };
    for (i, catalog) in order.iter().enumerate() {
        let lang = if loc == "ru" && i == 0 { "ru" } else { "en" };
        let cat = plural_category(lang, n);
        let form = lookup(catalog, &format!("{key}.{cat}"))
            .or_else(|| lookup(catalog, &format!("{key}.other")));
        if let Some(word) = form {
            return format!("{n} {word}");
        }
    }
    format!("{n} {key}")
}

pub fn plural(key: &str, n: u64) -> String {
    plural_in(&locale(), key, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_catalogs_have_identical_key_sets() {
        fn leaves(v: &Value, prefix: &str, out: &mut Vec<String>) {
            match v {
                Value::Object(map) => {
                    for (k, child) in map {
                        leaves(child, &format!("{prefix}{k}."), out);
                    }
                }
                _ => out.push(prefix.trim_end_matches('.').to_string()),
            }
        }
        let (en, ru) = catalogs();
        let (mut en_keys, mut ru_keys) = (Vec::new(), Vec::new());
        leaves(en, "", &mut en_keys);
        leaves(ru, "", &mut ru_keys);
        en_keys.sort();
        ru_keys.sort();
        assert_eq!(en_keys, ru_keys, "каталоги EN и RU должны совпадать по ключам");
        assert!(en_keys.len() > 80, "каталог подозрительно мал: {}", en_keys.len());
    }

    #[test]
    fn t_falls_back_to_en_for_missing_locale_and_key() {
        // zh: каталога нет — всегда EN
        assert_eq!(t_in("zh", "tray.menu.quit", &[]), "Quit");
        // ru-ключа нет (такого нет, но проверим фолбэк на EN через несуществующий путь)
        assert_eq!(t_in("ru", "notify.nope.missing", &[]), "notify.nope.missing");
        // известный ключ RU отличается от EN
        assert_eq!(t_in("ru", "tray.menu.quit", &[]), "Выход");
        assert_eq!(t_in("en", "tray.menu.quit", &[]), "Quit");
    }

    #[test]
    fn placeholders_are_expanded() {
        let s = t_in("ru", "notify.mic.device_gone", &[("name", "Геймерский микрофон")]);
        assert!(s.contains("Геймерский микрофон"), "{s}");
        assert!(!s.contains("{name}"), "{s}");
        let e = t_in("en", "err.server", &[("status", "500"), ("body", "boom")]);
        assert_eq!(e, "Server error (HTTP 500): boom");
    }

    #[test]
    fn ru_plural_rules() {
        assert_eq!(plural_in("ru", "tray.words", 1), "1 слово");
        assert_eq!(plural_in("ru", "tray.words", 2), "2 слова");
        assert_eq!(plural_in("ru", "tray.words", 5), "5 слов");
        assert_eq!(plural_in("ru", "tray.words", 11), "11 слов");
        assert_eq!(plural_in("ru", "tray.words", 21), "21 слово");
        assert_eq!(plural_in("ru", "tray.words", 112), "112 слов");
        assert_eq!(plural_in("ru", "tray.words", 1234), "1234 слова");
        assert_eq!(plural_in("ru", "tray.words", 0), "0 слов");
    }

    #[test]
    fn en_plural_rules() {
        assert_eq!(plural_in("en", "tray.words", 1), "1 word");
        assert_eq!(plural_in("en", "tray.words", 2), "2 words");
        assert_eq!(plural_in("en", "tray.words", 0), "0 words");
        assert_eq!(plural_in("en", "tray.words", 11), "11 words");
    }

    #[test]
    fn zh_plural_is_other_with_en_fallback() {
        assert_eq!(plural_in("zh", "tray.words", 1), "1 word");
        assert_eq!(plural_in("zh", "tray.words", 5), "5 words");
        assert_eq!(plural_category("zh", 3), "other");
    }

    #[test]
    fn resolve_maps_known_settings_and_system_prefix() {
        assert_eq!(resolve("en"), "en");
        assert_eq!(resolve("ru"), "ru");
        assert_eq!(resolve("zh"), "zh");
        // "auto"/неизвестное — по системной локали; результат всегда из трёх
        let r = resolve("auto");
        assert!(matches!(r.as_str(), "en" | "ru" | "zh"), "{r}");
    }

    #[test]
    fn default_locale_is_resolved_one_of_three() {
        // Глобальную локаль в тестах не меняем (иначе гонки с Display-тестами);
        // проверяем только начальное состояние.
        assert!(matches!(locale().as_str(), "en" | "ru" | "zh"));
    }
}
