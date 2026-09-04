//! Реестр «человеческих» сообщений об ошибках для пользователя (уведомления).
//! Системные тексты cpal/enigo часто неинформативны — маппим по подстрокам,
//! подсказываем, где лечить.

use crate::l10n;
use crate::recorder::RecorderError;

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// Понятное сообщение об ошибке микрофона/записи (6.1). Локализовано через
/// общий каталог (`notify.mic.*`); матчинг сырых cpal-текстов — по английским
/// подстрокам, он от локали не зависит.
pub fn microphone(e: &RecorderError) -> String {
    match e {
        RecorderError::NoInputDevice => l10n::t("notify.mic.no_device", &[]),
        RecorderError::PreferredDeviceGone(name) => {
            l10n::t("notify.mic.device_gone", &[("name", name)])
        }
        RecorderError::UnsupportedFormat(f) => {
            l10n::t("notify.mic.unsupported_format", &[("format", f)])
        }
        RecorderError::Build(raw) => {
            let lower = raw.to_lowercase();
            if contains_any(
                &lower,
                &["not authorized", "unauthorized", "permission", "permitted", "denied", "not entitled", "tcc"],
            ) {
                l10n::t("notify.mic.denied", &[])
            } else if contains_any(&lower, &["busy", "in use", "excluded"]) {
                l10n::t("notify.mic.busy", &[])
            } else if contains_any(
                &lower,
                &["not available", "no device", "not found", "devicechanged", "badparam"],
            ) {
                l10n::t("notify.mic.unavailable", &[])
            } else {
                l10n::t("notify.mic.start_failed", &[("error", raw)])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l10n::t_in;

    /// Сообщение совпадает с ожиданием в любой активной локали (тесты идут на
    /// дефолтной "en", но не должны падать, если глобальную локаль изменят).
    fn matches_any_locale(msg: &str, key: &str, params: &[(&str, &str)]) -> bool {
        msg == t_in("en", key, params) || msg == t_in("ru", key, params)
    }

    #[test]
    fn mic_denied_maps_to_privacy_hint() {
        for raw in [
            "Core Audio Error Code: -10863 (not authorized)",
            "operation not permitted",
            "Access to microphones denied by TCC",
        ] {
            let msg = microphone(&RecorderError::Build(raw.into()));
            assert!(matches_any_locale(&msg, "notify.mic.denied", &[]), "{msg}");
        }
    }

    #[test]
    fn unsupported_format_points_to_settings() {
        let msg = microphone(&RecorderError::UnsupportedFormat("DsdU8".into()));
        assert!(msg.contains("DsdU8"), "{msg}");
        assert!(matches_any_locale(
            &msg,
            "notify.mic.unsupported_format",
            &[("format", "DsdU8")]
        ), "{msg}");
    }

    #[test]
    fn preferred_device_gone_names_device() {
        let msg = microphone(&RecorderError::PreferredDeviceGone("Геймерский микрофон".into()));
        assert!(msg.contains("Геймерский микрофон"), "{msg}");
        assert!(matches_any_locale(
            &msg,
            "notify.mic.device_gone",
            &[("name", "Геймерский микрофон")]
        ), "{msg}");
    }

    #[test]
    fn mic_busy_maps_to_hint() {
        let msg = microphone(&RecorderError::Build("device is busy or in use".into()));
        assert!(matches_any_locale(&msg, "notify.mic.busy", &[]), "{msg}");
    }

    #[test]
    fn unknown_build_error_keeps_raw() {
        let msg = microphone(&RecorderError::Build("xyz weird".into()));
        assert!(msg.contains("xyz weird"), "{msg}");
        assert!(matches_any_locale(
            &msg,
            "notify.mic.start_failed",
            &[("error", "xyz weird")]
        ), "{msg}");
    }

    #[test]
    fn no_device_message_keeps_privacy_hint() {
        let msg = microphone(&RecorderError::NoInputDevice);
        assert!(matches_any_locale(&msg, "notify.mic.no_device", &[]), "{msg}");
    }
}
