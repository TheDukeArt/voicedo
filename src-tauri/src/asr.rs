use std::fmt;
use std::str::FromStr;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use reqwest::multipart::{Form, Part};
use serde::Serialize;
use tauri::AppHandle;

use crate::settings;

const REQUEST_TIMEOUT_SECS: u64 = 60;
const SILENCE_SAMPLES: usize = 1600; // 0.1 с при 16 кГц
const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Qwen,
}

impl Default for Provider {
    fn default() -> Self {
        Provider::OpenAi
    }
}

impl FromStr for Provider {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "qwen" | "dashscope" => Ok(Provider::Qwen),
            _ => Ok(Provider::OpenAi),
        }
    }
}

#[derive(Debug)]
pub enum AsrError {
    Unauthorized,
    NotFound,
    /// HTTP-статус + фрагмент тела ответа (до ~200 символов)
    Server(u16, String),
    /// Ключ/эндпоинт/модель валидны, но сервер не нашёл речи в аудио
    /// (DashScope: ASR_RESPONSE_HAVE_NO_WORDS)
    NoSpeech,
    Network,
    Timeout,
    BadResponse,
    InvalidEndpoint,
}

/// Если в теле ошибки DashScope есть маркер «нет речи» — это успех проверки подключения.
const NO_SPEECH_MARKER: &str = "ASR_RESPONSE_HAVE_NO_WORDS";

/// Фрагмент тела ответа для сообщений об ошибке (не более ~200 символов).
fn body_snippet(body: &str) -> String {
    const MAX_CHARS: usize = 200;
    let trimmed = body.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.to_string();
    }
    trimmed.chars().take(MAX_CHARS).collect::<String>() + "…"
}

/// Классификация не-успешного ответа: NoSpeech по телу, иначе Server со сниппетом.
fn classify_error_body(status: u16, body: &str) -> AsrError {
    if body.contains(NO_SPEECH_MARKER) {
        return AsrError::NoSpeech;
    }
    AsrError::Server(status, body_snippet(body))
}

/// Проверка JSON-ответа 2xx на наличие ошибки DashScope в полях code/message
/// (некоторые шлюзы отдают HAVE_NO_WORDS с HTTP 200).
fn no_speech_in_json(value: &serde_json::Value) -> bool {
    [
        value.get("code").and_then(|v| v.as_str()),
        value.get("message").and_then(|v| v.as_str()),
    ]
    .iter()
    .flatten()
    .any(|s| s.contains(NO_SPEECH_MARKER))
}

impl fmt::Display for AsrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            AsrError::Unauthorized => "401/403: ошибка авторизации — проверьте токен в настройках",
            AsrError::NotFound => "404: эндпоинт не найден — проверьте адрес API",
            AsrError::Server(code, body) => {
                return write!(f, "Ошибка сервера (HTTP {code}): {body}")
            }
            AsrError::NoSpeech => "Сервер доступен, но в тестовом аудио нет речи",
            AsrError::Network => "Сеть недоступна — не удалось выполнить запрос",
            AsrError::Timeout => "Превышен таймаут запроса (60 с)",
            AsrError::BadResponse => "Некорректный ответ сервера — ожидался JSON с полем \"text\"",
            AsrError::InvalidEndpoint => "Некорректный адрес эндпоинта",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for AsrError {}

/// Общий разбор HTTP-статуса для обоих провайдеров.
fn check_status(status: reqwest::StatusCode) -> Option<AsrError> {
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Some(AsrError::Unauthorized);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Some(AsrError::NotFound);
    }
    None
}

/// Склейка базового URL эндпоинта с путём `/audio/transcriptions`.
/// Убирает завершающие слэши; если путь уже оканчивается на `/audio/transcriptions`,
/// не дублирует его.
pub fn build_transcription_url(endpoint: &str) -> Result<reqwest::Url, AsrError> {
    let base = endpoint.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(AsrError::InvalidEndpoint);
    }
    let full = if base.ends_with("/audio/transcriptions") {
        base.to_string()
    } else {
        format!("{base}/audio/transcriptions")
    };
    reqwest::Url::parse(&full).map_err(|_| AsrError::InvalidEndpoint)
}

/// Генерация тестового WAV в памяти: тишина 0.1 с, 16 кГц, моно, 16-bit PCM.
/// Заголовок кодирует общий хелпер [`crate::recorder::encode_wav_i16`].
pub fn generate_silence_wav() -> Vec<u8> {
    crate::recorder::encode_wav_i16(&vec![0i16; SILENCE_SAMPLES], SAMPLE_RATE)
}

/// Отправка WAV на OpenAI-совместимый эндпоинт `/audio/transcriptions`.
pub async fn transcribe_wav(
    endpoint: &str,
    token: &str,
    model: &str,
    language: &str,
    wav: &[u8],
) -> Result<String, AsrError> {
    let url = build_transcription_url(endpoint)?;

    let file_part = Part::bytes(wav.to_vec())
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|_| AsrError::InvalidEndpoint)?;

    let mut form = Form::new()
        .part("file", file_part)
        .text("model", model.to_string());
    if !language.trim().is_empty() {
        form = form.text("language", language.trim().to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|_| AsrError::Network)?;

    let response = client
        .post(url)
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                AsrError::Timeout
            } else {
                AsrError::Network
            }
        })?;

    let status = response.status();
    if let Some(err) = check_status(status) {
        return Err(err);
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(classify_error_body(status.as_u16(), &body));
    }

    let body: serde_json::Value = response.json().await.map_err(|_| AsrError::BadResponse)?;
    body.get("text")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or(AsrError::BadResponse)
}

/// Отправка WAV на DashScope-native эндпоинт multimodal-generation.
/// Эндпоинт используется КАК ЕСТЬ (без склейки путей).
/// Язык (`language`) пока не передаётся: подтверждённого поля для qwen-audio ASR
/// в native-контракте нет. TODO: проверить language_hints/parameters и добавить.
pub async fn transcribe_wav_qwen(
    endpoint: &str,
    token: &str,
    model: &str,
    _language: &str,
    wav: &[u8],
) -> Result<String, AsrError> {
    let base = endpoint.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(AsrError::InvalidEndpoint);
    }
    let url = reqwest::Url::parse(base).map_err(|_| AsrError::InvalidEndpoint)?;

    let data_uri = format!("data:audio/wav;base64,{}", B64.encode(wav));
    let payload = serde_json::json!({
        "model": model,
        "input": {
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "input_audio",
                    "input_audio": { "data": data_uri }
                }]
            }]
        },
        "parameters": { "format": "wav", "sample_rate": "16000" }
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|_| AsrError::Network)?;

    let response = client
        .post(url)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .header("X-DashScope-SSE", "disable")
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                AsrError::Timeout
            } else {
                AsrError::Network
            }
        })?;

    let status = response.status();
    if let Some(err) = check_status(status) {
        return Err(err);
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(classify_error_body(status.as_u16(), &body));
    }

    let body: serde_json::Value = response.json().await.map_err(|_| AsrError::BadResponse)?;
    if no_speech_in_json(&body) {
        return Err(AsrError::NoSpeech);
    }
    // Основной путь: output.output.text; фолбэки: output.text, output.output.sentence.text
    let output = body.get("output");
    let inner = output.and_then(|o| o.get("output"));
    inner
        .and_then(|o| o.get("text"))
        .and_then(|t| t.as_str())
        .or_else(|| output.and_then(|o| o.get("text")).and_then(|t| t.as_str()))
        .or_else(|| {
            inner
                .and_then(|o| o.get("sentence"))
                .and_then(|s| s.get("text"))
                .and_then(|t| t.as_str())
        })
        .map(|s| s.to_string())
        .ok_or(AsrError::BadResponse)
}

/// Диспетчер: выбор реализации по провайдеру.
pub async fn transcribe(
    provider: Provider,
    endpoint: &str,
    token: &str,
    model: &str,
    language: &str,
    wav: &[u8],
) -> Result<String, AsrError> {
    match provider {
        Provider::OpenAi => transcribe_wav(endpoint, token, model, language, wav).await,
        Provider::Qwen => transcribe_wav_qwen(endpoint, token, model, language, wav).await,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    pub text: Option<String>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn test_connection(app: AppHandle) -> TestResult {
    let s = settings::load_settings(&app);
    let provider = Provider::from_str(&s.provider).unwrap_or_default();
    let wav = generate_silence_wav();
    let started = Instant::now();
    match transcribe(provider, &s.endpoint, &s.token, &s.model, &s.language, &wav).await {
        Ok(text) => TestResult {
            ok: true,
            text: Some(text),
            latency_ms: started.elapsed().as_millis() as u64,
            error: None,
        },
        // Для теста тишиной «речь не обнаружена» — признак валидности ключа/эндпоинта/модели
        Err(AsrError::NoSpeech) => TestResult {
            ok: true,
            text: Some(
                "(эндпоинт отвечает, речь не обнаружена — это нормально для теста тишиной)".to_string(),
            ),
            latency_ms: started.elapsed().as_millis() as u64,
            error: None,
        },
        Err(e) => TestResult {
            ok: false,
            text: None,
            latency_ms: started.elapsed().as_millis() as u64,
            error: Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    #[test]
    fn silence_wav_header_is_valid() {
        let wav = generate_silence_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(data_len, SILENCE_SAMPLES * 2);
        assert_eq!(wav.len(), 44 + data_len);
        let riff_len = u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff_len + 8, wav.len());
        // 16-bit PCM mono 16 кГц
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        // тишина
        assert!(wav[44..].iter().all(|&b| b == 0));
    }

    #[test]
    fn url_joins_reasonably() {
        assert_eq!(
            build_transcription_url("http://127.0.0.1:18080/v1").unwrap().as_str(),
            "http://127.0.0.1:18080/v1/audio/transcriptions"
        );
        assert_eq!(
            build_transcription_url("https://api.openai.com/v1/").unwrap().as_str(),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        // уже полный путь — не дублируем
        assert_eq!(
            build_transcription_url("https://example.com/v1/audio/transcriptions").unwrap().as_str(),
            "https://example.com/v1/audio/transcriptions"
        );
        assert!(matches!(
            build_transcription_url("не url"),
            Err(AsrError::InvalidEndpoint)
        ));
        assert!(matches!(
            build_transcription_url("   "),
            Err(AsrError::InvalidEndpoint)
        ));
    }

    /// Локальный мок ASR-сервера: без `Bearer testkey` → 401, с ним → 200 {"text":"привет"}.
    fn spawn_mock() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(
                        stream.try_clone().expect("clone stream")
                    );
                    let mut line = String::new();
                    let mut headers = vec![];
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let mut content_len = 0usize;
                    loop {
                        let mut h = String::new();
                        if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                            break;
                        }
                        if h.to_lowercase().starts_with("content-length:") {
                            content_len = h[15..].trim().parse().unwrap_or(0);
                        }
                        headers.push(h.to_lowercase());
                    }
                    let mut body = vec![0u8; content_len];
                    if content_len > 0 {
                        let _ = reader.read_exact(&mut body);
                    }
                    let auth_ok = headers
                        .iter()
                        .any(|h| h.starts_with("authorization:") && h.contains("bearer testkey"));
                    let response = if auth_ok {
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"text\":\"привет\"}"
                    } else {
                        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"error\":{\"message\":\"Invalid API key\"}}"
                    };
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn mock_valid_token_returns_text() {
        let addr = spawn_mock();
        let endpoint = format!("http://{}/v1", addr);
        let wav = generate_silence_wav();
        let text = transcribe_wav(&endpoint, "testkey", "whisper-1", "", &wav)
            .await
            .expect("ok");
        assert_eq!(text, "привет");
    }

    /// То же, но против внешнего python-мока (scripts/mock_asr.py, 127.0.0.1:18080).
    /// Запуск: python3 ../scripts/mock_asr.py & cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn external_mock_scenarios() {
        let endpoint = "http://127.0.0.1:18080/v1";
        let wav = generate_silence_wav();
        let text = transcribe_wav(endpoint, "testkey", "whisper-1", "", &wav)
            .await
            .expect("valid token should succeed");
        assert_eq!(text, "привет");
        let err = transcribe_wav(endpoint, "bad", "whisper-1", "", &wav)
            .await
            .expect_err("bad token should fail");
        assert!(matches!(err, AsrError::Unauthorized));
    }

    #[tokio::test]
    async fn mock_wrong_token_is_unauthorized() {
        let addr = spawn_mock();
        let endpoint = format!("http://{}/v1", addr);
        let wav = generate_silence_wav();
        let err = transcribe_wav(&endpoint, "wrongkey", "whisper-1", "ru", &wav)
            .await
            .expect_err("should fail");
        assert!(matches!(err, AsrError::Unauthorized));
        assert!(err.to_string().contains("авторизации"));
    }

    #[test]
    fn provider_parses_from_settings_string() {
        assert_eq!(Provider::from_str("qwen").unwrap(), Provider::Qwen);
        assert_eq!(Provider::from_str("DashScope").unwrap(), Provider::Qwen);
        assert_eq!(Provider::from_str("openai").unwrap(), Provider::OpenAi);
        assert_eq!(Provider::from_str("").unwrap(), Provider::OpenAi);
        assert_eq!(Provider::from_str("что-то").unwrap(), Provider::OpenAi);
    }

    #[test]
    fn server_error_display_includes_body_snippet() {
        let long = "x".repeat(500);
        let err = AsrError::Server(500, body_snippet(&long));
        let s = err.to_string();
        assert!(s.starts_with("Ошибка сервера (HTTP 500): xxx"));
        assert!(s.ends_with('…'));
        assert!(s.chars().count() < 250);
    }

    #[test]
    fn error_body_without_marker_is_server_error() {
        let err = classify_error_body(
            500,
            "{\"code\":\"InternalError\",\"message\":\"boom\"}",
        );
        match err {
            AsrError::Server(500, body) => assert!(body.contains("InternalError")),
            other => panic!("expected Server, got {other:?}"),
        }
    }

    /// Мок DashScope-native эндпоинта:
    /// - без `Bearer testkey` → 401;
    /// - model == "silence-marker" → 400 ASR_RESPONSE_HAVE_NO_WORDS;
    /// - валидный JSON с input_audio.data data:audio/wav;base64 → 200 output.output.text;
    /// - путь, дополненный до /audio/transcriptions (ошибочная склейка) → 404.
    fn spawn_qwen_mock() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                    let mut content_len = 0usize;
                    let mut auth_ok = false;
                    let mut is_json = false;
                    loop {
                        let mut h = String::new();
                        if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                            break;
                        }
                        let hl = h.to_lowercase();
                        if hl.starts_with("content-length:") {
                            content_len = h[15..].trim().parse().unwrap_or(0);
                        }
                        if hl.starts_with("authorization:") && hl.contains("bearer testkey") {
                            auth_ok = true;
                        }
                        if hl.starts_with("content-type:") && hl.contains("application/json") {
                            is_json = true;
                        }
                    }
                    let mut body = vec![0u8; content_len];
                    if content_len > 0 {
                        let _ = reader.read_exact(&mut body);
                    }
                    let body = String::from_utf8_lossy(&body).to_string();
                    let response = if !auth_ok {
                        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"code\":\"InvalidApiKey\",\"message\":\"Invalid API key\"}".to_string()
                    } else if path.ends_with("/audio/transcriptions") {
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}".to_string()
                    } else if body.contains("silence-marker") {
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"request_id\":\"r-1\",\"code\":\"CLIENT_ERROR\",\"message\":\"ASR_RESPONSE_HAVE_NO_WORDS\"}".to_string()
                    } else if is_json
                        && path.ends_with("/multimodal-generation/generation")
                        && body.contains("input_audio")
                        && body.contains("data:audio/wav;base64,")
                    {
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"output\":{\"output\":{\"text\":\"раз два три проверка. \"},\"request_id\":\"r-2\"},\"request_id\":\"r-2\"}".to_string()
                    } else {
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"code\":\"BadRequest\",\"message\":\"unexpected request\"}".to_string()
                    };
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn qwen_mock_valid_token_returns_text() {
        let addr = spawn_qwen_mock();
        // Полный native-эндпоинт: путь должен использоваться как есть
        let endpoint =
            format!("http://{addr}/api/v1/services/aigc/multimodal-generation/generation");
        let wav = generate_silence_wav();
        let text = transcribe_wav_qwen(&endpoint, "testkey", "qwen-audio-3.0-asr-flash", "ru", &wav)
            .await
            .expect("ok");
        assert_eq!(text, "раз два три проверка. ");
    }

    #[tokio::test]
    async fn qwen_mock_wrong_token_is_unauthorized() {
        let addr = spawn_qwen_mock();
        let endpoint = format!("http://{addr}/api/v1/services/aigc/multimodal-generation/generation");
        let wav = generate_silence_wav();
        let err = transcribe_wav_qwen(&endpoint, "bad", "qwen-audio-3.0-asr-flash", "", &wav)
            .await
            .expect_err("should fail");
        assert!(matches!(err, AsrError::Unauthorized));
    }

    #[tokio::test]
    async fn qwen_mock_silence_is_no_speech() {
        let addr = spawn_qwen_mock();
        let endpoint = format!("http://{addr}/api/v1/services/aigc/multimodal-generation/generation");
        let wav = generate_silence_wav();
        let err = transcribe_wav_qwen(&endpoint, "testkey", "silence-marker", "", &wav)
            .await
            .expect_err("silence should error");
        assert!(matches!(err, AsrError::NoSpeech));
        assert_eq!(
            err.to_string(),
            "Сервер доступен, но в тестовом аудио нет речи"
        );
    }

    #[tokio::test]
    async fn qwen_mock_server_error_includes_body_snippet() {
        let addr = spawn_qwen_mock();
        // Другой путь на том же хосте → 400 "unexpected request" без маркера no-words
        let err = transcribe_wav_qwen(
            &format!("http://{addr}/bad/path"),
            "testkey",
            "qwen-audio-3.0-asr-flash",
            "",
            &generate_silence_wav(),
        )
        .await
        .expect_err("should fail");
        match err {
            AsrError::Server(400, ref body) => {
                assert!(body.contains("BadRequest"));
                assert!(err.to_string().starts_with("Ошибка сервера (HTTP 400):"));
            }
            other => panic!("expected Server(400), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatcher_selects_qwen_provider() {
        let addr = spawn_qwen_mock();
        let endpoint = format!("http://{addr}/api/v1/services/aigc/multimodal-generation/generation");
        let wav = generate_silence_wav();
        let text = transcribe(
            Provider::Qwen,
            &endpoint,
            "testkey",
            "qwen-audio-3.0-asr-flash",
            "",
            &wav,
        )
        .await
        .expect("ok");
        assert!(text.contains("раз два три"));
    }

}
