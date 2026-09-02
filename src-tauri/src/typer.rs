use std::thread;
use std::time::Duration;

use enigo::{Enigo, Keyboard, Settings as EnigoSettings};

/// Чистая предпроверка: пустой/пробельный текст не вставляем (для тестов).
pub fn should_insert(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Вставить текст в активное окно: задержка delay_ms (целевое приложение должно
/// вернуть фокус после отпускания хоткея, см. PLAN.md «Риски»), затем ввод
/// через enigo (Unicode-ввод, кириллица поддерживается).
///
/// Блокирующий — вызывать из `spawn_blocking`/отдельного потока.
///
/// macOS: без разрешения Accessibility `Enigo::new` возвращает `NoPermission`
/// (настройки по умолчанию также показывают системный запрос разрешения) —
/// это и есть дешёвая явная проверка; полный UX разрешений — этап 6.
pub fn insert_text(text: &str, delay_ms: u64) -> Result<(), String> {
    if !should_insert(text) {
        return Ok(());
    }
    thread::sleep(Duration::from_millis(delay_ms));
    let mut enigo = Enigo::new(&EnigoSettings::default()).map_err(|e| {
        format!("Нет доступа к вводу с клавиатуры (macOS: Конфиденциальность и защита → Специальные возможности): {e}")
    })?;
    // text() использует раскладку, а для юникода (кириллица) fallback-ит на
    // посимвольный Unicode-ввод (Keyboard::text -> fast_text/посимвольно).
    enigo
        .text(text)
        .map_err(|e| format!("Не удалось ввести текст: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_not_inserted() {
        assert!(!should_insert(""));
        assert!(!should_insert("   \n\t "));
        assert!(should_insert("т"));
        assert!(should_insert("раз два три"));
    }

    #[test]
    fn insert_text_returns_ok_without_touching_enigo_for_empty() {
        // Пустая строка выходит до создания Enigo — безопасно даже без
        // Accessibility-разрешения в CI.
        assert_eq!(insert_text("", 0), Ok(()));
        assert_eq!(insert_text("  ", 0), Ok(()));
    }
}
