//! 6.6: тестовая диктовка из окна настроек (микрофон → ASR, без хоткея и без
//! вставки). Результат/ошибка — событием `dictation-test-result` с seq для
//! отбрасывания устаревших ответов при перезапуске теста.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{asr, error_msg, hotkey, l10n, recorder, settings, tray};

pub const DICTATION_TEST_EVENT: &str = "dictation-test-result";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DictationResult {
    pub seq: u64,
    pub ok: bool,
    pub text: Option<String>,
    pub error: Option<String>,
    pub latency_ms: u64,
}

/// Фильтр устаревших результатов (тот же критерий, что на фронте).
#[must_use]
pub fn is_stale(current_seq: u64, incoming_seq: u64) -> bool {
    incoming_seq < current_seq
}

#[derive(Default)]
pub struct TestPanel {
    seq: AtomicU64,
    /// seq последнего запуска (его и несёт актуальный результат)
    current: AtomicU64,
    /// Идёт ли тестовая запись, начатая именно панелью (иначе слот Recorder
    /// занят хоткеем).
    recording: AtomicBool,
}

impl TestPanel {
    fn next_seq(&self) -> u64 {
        let s = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.current.store(s, Ordering::SeqCst);
        s
    }

    fn current_seq(&self) -> u64 {
        self.current.load(Ordering::SeqCst)
    }
}

#[tauri::command]
pub fn start_test_dictation(
    app: AppHandle,
    panel: State<'_, TestPanel>,
) -> Result<u64, String> {
    let rec = app.state::<recorder::Recorder>();
    let panel_own = panel.recording.load(Ordering::SeqCst);
    if rec.is_recording() && !panel_own {
        return Err(l10n::t("notify.test.hotkey_busy", &[]));
    }
    if panel_own {
        // Перезапуск теста: предыдущий результат будет отброшен по seq.
        let _ = rec.stop();
    }
    match rec.start(&settings::load_settings(&app).input_device) {
        Ok(()) => {
            panel.recording.store(true, Ordering::SeqCst);
            let seq = panel.next_seq();
            tray::set_tray_state(&app, tray::TrayState::Recording);
            log::info!("[test-panel] test dictation started (seq {seq})");
            Ok(seq)
        }
        Err(e) => {
            log::error!("[test-panel] start failed: {e}");
            Err(error_msg::microphone(&e))
        }
    }
}

#[tauri::command]
pub fn stop_test_dictation(app: AppHandle, panel: State<'_, TestPanel>) -> Result<(), String> {
    if !panel.recording.swap(false, Ordering::SeqCst) {
        return Err(l10n::t("notify.test.not_running", &[]));
    }
    let seq = panel.current_seq();
    let rec = app.state::<recorder::Recorder>();
    let Some(recorded) = rec.stop() else {
        tray::set_tray_state(&app, tray::TrayState::Ready);
        emit(&app, DictationResult {
            seq,
            ok: false,
            text: None,
            error: Some(l10n::t("notify.test.record_failed", &[])),
            latency_ms: 0,
        });
        return Ok(());
    };
    log::info!(
        "[test-panel] stopped: {:.2} s, WAV {} bytes (seq {seq})",
        recorded.duration.as_secs_f64(),
        recorded.wav.len()
    );
    let s = settings::load_settings(&app);
    if let Err(reason) = hotkey::should_transcribe(&s) {
        tray::set_tray_state(&app, tray::TrayState::Ready);
        emit(&app, DictationResult { seq, ok: false, text: None, error: Some(reason), latency_ms: 0 });
        return Ok(());
    }
    tray::set_tray_state(&app, tray::TrayState::Processing);
    let app_bg = app.clone();
    tauri::async_runtime::spawn(async move {
        let provider = asr::Provider::from_str(&s.provider).unwrap_or_default();
        let started = Instant::now();
        let result =
            asr::transcribe(provider, &s.endpoint, &s.token, &s.model, &s.language, &recorded.wav)
                .await;
        let latency_ms = started.elapsed().as_millis() as u64;
        let payload = match result {
            Ok(text) => {
                log::info!("[test-panel] recognized (seq {seq}, {latency_ms} ms): «{text}»");
                DictationResult { seq, ok: true, text: Some(text), error: None, latency_ms }
            }
            Err(e) => {
                log::error!("[test-panel] error (seq {seq}): {e}");
                let msg = match e {
                    asr::AsrError::NoSpeech => l10n::t("notify.asr.no_speech", &[]),
                    other => l10n::t("notify.asr.failed", &[("error", &other.to_string())]),
                };
                DictationResult { seq, ok: false, text: None, error: Some(msg), latency_ms }
            }
        };
        // Устаревший результат (тест уже перезапущен) не отправляем.
        let current = app_bg.state::<TestPanel>().current_seq();
        if is_stale(current, seq) {
            log::info!("[test-panel] dropping stale result (seq {seq}, current {current})");
        } else {
            emit(&app_bg, payload);
        }
        tray::set_tray_state(&app_bg, tray::TrayState::Ready);
    });
    Ok(())
}

fn emit(app: &AppHandle, result: DictationResult) {
    if let Err(e) = app.emit(DICTATION_TEST_EVENT, result) {
        log::error!("[test-panel] emit failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_filter() {
        assert!(is_stale(2, 1));
        assert!(!is_stale(2, 2));
        assert!(!is_stale(2, 3));
        assert!(is_stale(1, 0));
    }

    #[test]
    fn result_serializes_camel_case_with_seq() {
        let json = serde_json::to_value(DictationResult {
            seq: 7,
            ok: true,
            text: Some("раз два три".into()),
            error: None,
            latency_ms: 123,
        })
        .unwrap();
        assert_eq!(json["seq"], 7);
        assert_eq!(json["ok"], true);
        assert_eq!(json["text"], "раз два три");
        assert!(json.get("error").unwrap().is_null());
        assert_eq!(json["latencyMs"], 123);
    }
}
