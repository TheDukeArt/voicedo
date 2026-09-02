//! Реестр «человеческих» сообщений об ошибках для пользователя (уведомления).
//! Системные тексты cpal/enigo часто неинформативны — маппим по подстрокам,
//! подсказываем, где лечить.

use crate::recorder::RecorderError;

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// Понятное сообщение об ошибке микрофона/записи (6.1).
pub fn microphone(e: &RecorderError) -> String {
    match e {
        RecorderError::NoInputDevice => e.to_string(),
        RecorderError::UnsupportedFormat(f) => {
            format!("Микрофон: неподдерживаемый формат {f} — попробуйте другое входное устройство")
        }
        RecorderError::Build(raw) => {
            let lower = raw.to_lowercase();
            if contains_any(
                &lower,
                &["not authorized", "unauthorized", "permission", "permitted", "denied", "not entitled", "tcc"],
            ) {
                "Нет доступа к микрофону — разрешите: Системные настройки → Конфиденциальность и защита → Микрофон (затем перезапустите VoiceDo)".to_string()
            } else if contains_any(&lower, &["busy", "in use", "excluded"]) {
                "Микрофон занят другим приложением — закройте его и попробуйте снова".to_string()
            } else if contains_any(
                &lower,
                &["not available", "no device", "not found", "devicechanged", "badparam"],
            ) {
                "Микрофон недоступен — проверьте, что устройство подключено и выбрано (Системные настройки → Звук)".to_string()
            } else {
                format!("Не удалось начать запись: {raw}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mic_denied_maps_to_privacy_hint() {
        for raw in [
            "Core Audio Error Code: -10863 (not authorized)",
            "operation not permitted",
            "Access to microphones denied by TCC",
        ] {
            let msg = microphone(&RecorderError::Build(raw.into()));
            assert!(msg.contains("Конфиденциальность"), "{msg}");
            assert!(msg.contains("Микрофон"), "{msg}");
        }
    }

    #[test]
    fn mic_busy_maps_to_hint() {
        let msg = microphone(&RecorderError::Build("device is busy or in use".into()));
        assert!(msg.contains("занят"), "{msg}");
    }

    #[test]
    fn unknown_build_error_keeps_raw() {
        let msg = microphone(&RecorderError::Build("xyz weird".into()));
        assert!(msg.contains("xyz weird"), "{msg}");
        assert!(msg.contains("Не удалось начать запись"), "{msg}");
    }

    #[test]
    fn no_device_message_keeps_privacy_hint() {
        let msg = microphone(&RecorderError::NoInputDevice);
        assert!(msg.contains("Микрофон не найден"), "{msg}");
        assert!(msg.contains("Конфиденциальность"), "{msg}");
    }
}
