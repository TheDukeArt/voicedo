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

/// Скрытый эндпоинт распознавания Chrome/Chromium. Неофициальный: может
/// перестать работать в любой момент; публичный ключ — константа из исходников
/// Chromium / python `speech_recognition`, это НЕ секрет.
const GOOGLE_SPEECH_API_URL: &str = "https://www.google.com/speech-api/v2/recognize";
const GOOGLE_PUBLIC_API_KEY: &str = "AIzaSyBOti4mM-6x9WDnZIjIeyEU21OpBXqWBgw";
/// Лимит длительности запроса ~15 с; проверяем с запасом.
const GOOGLE_MAX_DURATION_SECS: f64 = 14.5;
/// Хинты 401/403 — ключи каталога (`l10n`), не сам текст.
const AUTH_HINT_TOKEN: &str = "err.auth_token";
const AUTH_HINT_GOOGLE: &str = "err.auth_google";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Qwen,
    Google,
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
            "google" | "chromium" => Ok(Provider::Google),
            _ => Ok(Provider::OpenAi),
        }
    }
}

#[derive(Debug)]
pub enum AsrError {
    /// Подсказка зависит от провайдера (токен у OpenAI/Qwen vs лимиты Google)
    Unauthorized(&'static str),
    NotFound,
    /// HTTP-статус + фрагмент тела ответа (до ~200 символов)
    Server(u16, String),
    /// Ключ/эндпоинт/модель валидны, но сервер не нашёл речи в аудио
    /// (DashScope: ASR_RESPONSE_HAVE_NO_WORDS)
    NoSpeech,
    Network,
    Timeout,
    /// Подсказка: что именно не так с ответом — ключ каталога `err.hint.*`
    BadResponse(&'static str),
    InvalidEndpoint,
    /// Запись длиннее лимита неофициального Google API (~15 с)
    TooLong,
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
        use crate::l10n;
        let msg = match self {
            AsrError::Unauthorized(hint_key) => {
                let hint = l10n::t(hint_key, &[]);
                return write!(f, "{}", l10n::t("err.unauthorized", &[("hint", &hint)]));
            }
            AsrError::NotFound => l10n::t("err.not_found", &[]),
            AsrError::Server(code, body) => {
                return write!(
                    f,
                    "{}",
                    l10n::t("err.server", &[("status", &code.to_string()), ("body", body)])
                )
            }
            AsrError::NoSpeech => l10n::t("err.no_speech_test", &[]),
            AsrError::Network => l10n::t("err.network", &[]),
            AsrError::Timeout => l10n::t("err.timeout", &[]),
            AsrError::BadResponse(hint_key) => {
                // hint_key — ключ каталога err.hint.* (как hint у Unauthorized)
                let hint = l10n::t(hint_key, &[]);
                return write!(f, "{}", l10n::t("err.bad_response", &[("hint", &hint)]));
            }
            AsrError::InvalidEndpoint => l10n::t("err.invalid_endpoint", &[]),
            AsrError::TooLong => l10n::t("err.too_long", &[]),
        };
        f.write_str(&msg)
    }
}

impl std::error::Error for AsrError {}

/// Общий разбор HTTP-статуса для обоих провайдеров.
fn check_status(status: reqwest::StatusCode) -> Option<AsrError> {
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Some(AsrError::Unauthorized(AUTH_HINT_TOKEN));
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

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| AsrError::BadResponse("err.hint.json_text_field"))?;
    body.get("text")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or(AsrError::BadResponse("err.hint.json_text_field"))
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

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| AsrError::BadResponse("err.hint.json_output_text"))?;
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
        .ok_or(AsrError::BadResponse("err.hint.missing_text_field"))
}

/// Разбор нашего WAV (RIFF/WAVE, PCM 16-bit) в моно-сэмплы и частоту.
/// Ходит по чанкам (не предполагая жёстко 44-байтный заголовок), как пишет
/// [`crate::recorder::encode_wav_i16`].
fn parse_wav_pcm16(wav: &[u8]) -> Result<(Vec<i16>, u32), AsrError> {
    let bad = |hint: &'static str| AsrError::BadResponse(hint);
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err(bad("err.hint.wav_header"));
    }
    let mut pos = 12usize;
    let mut rate: Option<u32> = None;
    let mut channels: Option<u16> = None;
    let mut bits: Option<u16> = None;
    let mut data: Option<(usize, usize)> = None;
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let size = u32::from_le_bytes(wav[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8..(pos + 8 + size).min(wav.len());
        match id {
            b"fmt " if size >= 16 => {
                let f = &wav[body];
                channels = Some(u16::from_le_bytes(f[2..4].try_into().unwrap()));
                rate = Some(u32::from_le_bytes(f[4..8].try_into().unwrap()));
                bits = Some(u16::from_le_bytes(f[14..16].try_into().unwrap()));
            }
            b"data" => {
                data = Some((body.start, body.end));
            }
            _ => {}
        }
        // чанки в RIFF выровнены по 2 байта
        pos = pos + 8 + size + (size % 2);
    }
    let (Some(rate), Some(channels), Some(bits)) = (rate, channels, bits) else {
        return Err(bad("err.hint.wav_fmt_chunk"));
    };
    if channels != 1 || bits != 16 {
        return Err(bad("err.hint.wav_pcm16_mono"));
    }
    let (start, end) = data.ok_or_else(|| bad("err.hint.wav_data_chunk"))?;
    let samples = wav[start..end]
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok((samples, rate))
}

/// Кодирование mono 16-bit PCM в FLAC (чистый Rust, crate `flacenc`).
/// Пустой/ошибочный результат — пустой Vec (проверяется вызывающим кодом).
pub fn encode_pcm_to_flac(samples: &[i16], rate: u32) -> Vec<u8> {
    use flacenc::{bitsink::ByteSink, component::BitRepr, error::Verify, source::MemSource};
    let i32_samples: Vec<i32> = samples.iter().map(|&s| i32::from(s)).collect();
    let Ok(config) = flacenc::config::Encoder::default().into_verified() else {
        return Vec::new();
    };
    let source = MemSource::from_samples(&i32_samples, 1, 16, rate as usize);
    let Ok(stream) = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
    else {
        return Vec::new();
    };
    let mut sink = ByteSink::new();
    if stream.write(&mut sink).is_err() {
        return Vec::new();
    }
    sink.as_slice().to_vec()
}

/// Маппинг короткого кода языка из настроек в BCP-47 для Google (`ru` → `ru-RU`).
/// Пусто → дефолт Chromium `en-US`; неизвестный/готовый код — как есть.
fn google_lang(language: &str) -> String {
    let code = language.trim().to_ascii_lowercase();
    if code.is_empty() {
        return "en-US".to_string();
    }
    let region = match code.as_str() {
        "ru" => "RU",
        "en" => "US",
        "es" => "ES",
        "fr" => "FR",
        "de" => "DE",
        "it" => "IT",
        "pt" => "PT",
        "pl" => "PL",
        "tr" => "TR",
        "uk" => "UA",
        "zh" => "CN",
        "ja" => "JP",
        _ => "",
    };
    if region.is_empty() {
        language.trim().to_string()
    } else {
        format!("{code}-{region}")
    }
}

/// Парсер NDJSON-ответа Google speech-api: из каждой строки с непустым
/// `result` берётся `result[i].alternative[0].transcript`, всё склеивается
/// через пробел. Тишина (одни `{"result":[]}`) — пустая строка, не ошибка.
fn parse_google_ndjson(body: &str) -> Result<String, AsrError> {
    if body.trim_start().starts_with('<') {
        return Err(AsrError::BadResponse("err.hint.google_html_captcha"));
    }
    let mut parts: Vec<String> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        // XSSI-префикс Some(')']}-подобных ответов и пустые строки пропускаем
        if line.is_empty() || line.starts_with(")]}'") {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|_| AsrError::BadResponse("err.hint.ndjson_parse"))?;
        let Some(results) = value.get("result").and_then(|r| r.as_array()) else {
            continue;
        };
        for item in results {
            if let Some(t) = item
                .get("alternative")
                .and_then(|a| a.get(0))
                .and_then(|a| a.get("transcript"))
                .and_then(|t| t.as_str())
            {
                let t = t.trim();
                if !t.is_empty() {
                    parts.push(t.to_string());
                }
            }
        }
    }
    Ok(parts.join(" "))
}

/// Отправка на скрытый эндпоинт Chrome speech-api: WAV → PCM → FLAC (16 кГц
/// моно), NDJSON-парсинг ответа. Эндпоинт/токен/модель не нужны; публичный
/// ключ — константа. Лимит ~15 с на запись.
pub async fn transcribe_wav_google(language: &str, wav: &[u8]) -> Result<String, AsrError> {
    let (samples, rate) = parse_wav_pcm16(wav)?;
    let duration_secs = samples.len() as f64 / rate.max(1) as f64;
    if duration_secs > GOOGLE_MAX_DURATION_SECS {
        return Err(AsrError::TooLong);
    }
    let flac = encode_pcm_to_flac(&samples, rate);
    if flac.is_empty() {
        return Err(AsrError::BadResponse("err.hint.flac_empty"));
    }
    let url = reqwest::Url::parse_with_params(
        GOOGLE_SPEECH_API_URL,
        &[
            ("client", "chromium"),
            ("lang", google_lang(language).as_str()),
            ("key", GOOGLE_PUBLIC_API_KEY),
            ("pFilter", "0"),
        ],
    )
    .map_err(|_| AsrError::InvalidEndpoint)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|_| AsrError::Network)?;

    let response = client
        .post(url)
        .header("Content-Type", format!("audio/x-flac; rate={rate}"))
        .body(flac)
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
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(AsrError::Unauthorized(AUTH_HINT_GOOGLE));
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(AsrError::NotFound);
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AsrError::Server(status.as_u16(), body_snippet(&body)));
    }
    let body = response.text().await.map_err(|_| AsrError::Network)?;
    parse_google_ndjson(&body)
}

/// Диспетчер: выбор реализации по провайдеру.
/// Для Google эндпоинт/токен/модель игнорируются (публичный неофициальный API).
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
        Provider::Google => transcribe_wav_google(language, wav).await,
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
            text: Some(crate::l10n::t("err.test_no_speech_ok", &[])),
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
        assert!(matches!(err, AsrError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn mock_wrong_token_is_unauthorized() {
        let addr = spawn_mock();
        let endpoint = format!("http://{}/v1", addr);
        let wav = generate_silence_wav();
        let err = transcribe_wav(&endpoint, "wrongkey", "whisper-1", "ru", &wav)
            .await
            .expect_err("should fail");
        assert!(matches!(err, AsrError::Unauthorized(_)));
        let s = err.to_string();
        let expected = |loc: &str| {
            crate::l10n::t_in(
                loc,
                "err.unauthorized",
                &[("hint", &crate::l10n::t_in(loc, AUTH_HINT_TOKEN, &[]))],
            )
        };
        assert!(s == expected("en") || s == expected("ru"), "{s}");
    }

    #[test]
    fn provider_parses_from_settings_string() {
        assert_eq!(Provider::from_str("qwen").unwrap(), Provider::Qwen);
        assert_eq!(Provider::from_str("DashScope").unwrap(), Provider::Qwen);
        assert_eq!(Provider::from_str("openai").unwrap(), Provider::OpenAi);
        assert_eq!(Provider::from_str("").unwrap(), Provider::OpenAi);
        assert_eq!(Provider::from_str("что-то").unwrap(), Provider::OpenAi);
    }

    /// Равно переводу ключа в любой из двух каталогов (глобальная локаль в
    /// тестах — "en", но матчим обе на всякий случай).
    fn matches_any_locale(msg: &str, key: &str, params: &[(&str, &str)]) -> bool {
        msg == crate::l10n::t_in("en", key, params)
            || msg == crate::l10n::t_in("ru", key, params)
    }

    #[test]
    fn server_error_display_includes_body_snippet() {
        let long = "x".repeat(500);
        let snippet = body_snippet(&long);
        let err = AsrError::Server(500, snippet.clone());
        let s = err.to_string();
        assert!(s.contains("HTTP 500"), "{s}");
        assert!(s.contains("xxx"), "{s}");
        assert!(s.ends_with('…'));
        assert!(matches_any_locale(
            &s,
            "err.server",
            &[("status", "500"), ("body", &snippet)]
        ));
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
        assert!(matches!(err, AsrError::Unauthorized(_)));
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
        assert!(matches_any_locale(err.to_string().as_str(), "err.no_speech_test", &[]));
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
                assert!(matches_any_locale(
                    err.to_string().as_str(),
                    "err.server",
                    &[("status", "400"), ("body", body)]
                ));
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

    // --- Этап 8: Google (неофициальный speech-api) ---

    /// Реальный живой ответ (контракт верифицирован оркестратором).
    const GOOGLE_NDJSON_FIXTURE: &str = concat!(
        "{\"result\":[]}\n",
        "{\"result\":[{\"alternative\":[{\"transcript\":\"раз два три\",\"confidence\":0.68},",
        "{\"transcript\":\"раз и три\"}],\"final\":true}],\"result_index\":0}\n",
    );

    #[test]
    fn google_ndjson_fixture_parses_first_alternatives() {
        let text = parse_google_ndjson(GOOGLE_NDJSON_FIXTURE).expect("ok");
        assert_eq!(text, "раз два три");
    }

    #[test]
    fn google_ndjson_silence_is_empty_not_error() {
        let body = "{\"result\":[]}\n{\"result\":[]}\n";
        assert_eq!(parse_google_ndjson(body).expect("silence is ok"), "");
    }

    #[test]
    fn google_ndjson_joins_multiple_results() {
        let body = concat!(
            "{\"result\":[{\"alternative\":[{\"transcript\":\"привет\"}]}]}\n",
            "{\"result\":[{\"alternative\":[{\"transcript\":\"мир\"}]}]}\n",
        );
        assert_eq!(parse_google_ndjson(body).expect("ok"), "привет мир");
    }

    #[test]
    fn google_ndjson_skips_xssi_prefix() {
        let body = format!(")]}}'\n{GOOGLE_NDJSON_FIXTURE}");
        assert_eq!(parse_google_ndjson(&body).expect("ok"), "раз два три");
    }

    #[test]
    fn google_html_response_is_bad_response_with_captcha_hint() {
        let err = parse_google_ndjson("<html><body>Captcha</body></html>")
            .expect_err("html should fail");
        match err {
            AsrError::BadResponse(hint_key) => {
                assert_eq!(hint_key, "err.hint.google_html_captcha", "{hint_key}");
                // сообщение в активной локали упоминает captcha (EN-значение ключа)
                assert!(
                    err.to_string().to_lowercase().contains("captcha"),
                    "{err}"
                );
            }
            other => panic!("expected BadResponse, got {other:?}"),
        }
    }

    #[test]
    fn google_unauthorized_hint_mentions_api() {
        let google = AsrError::Unauthorized(AUTH_HINT_GOOGLE).to_string();
        let expected = |loc: &str| {
            crate::l10n::t_in(
                loc,
                "err.unauthorized",
                &[("hint", &crate::l10n::t_in(loc, AUTH_HINT_GOOGLE, &[]))],
            )
        };
        assert!(google == expected("en") || google == expected("ru"), "{google}");
        // сообщение для OpenAI/Qwen отличается от google-хинта
        let token = AsrError::Unauthorized(AUTH_HINT_TOKEN).to_string();
        assert_ne!(token, google);
    }

    #[test]
    fn wav_parses_back_to_pcm() {
        let wav = generate_silence_wav();
        let (samples, rate) = parse_wav_pcm16(&wav).expect("our wav must parse");
        assert_eq!(rate, 16_000);
        assert_eq!(samples.len(), SILENCE_SAMPLES);
        assert!(samples.iter().all(|&s| s == 0));
        // не-WAV на входе — ошибка, не паника
        assert!(matches!(
            parse_wav_pcm16(b"hello world"),
            Err(AsrError::BadResponse(_))
        ));
    }

    #[test]
    fn flac_encodes_pcm_with_magic() {
        let (samples, rate) = parse_wav_pcm16(&generate_silence_wav()).unwrap();
        let flac = encode_pcm_to_flac(&samples, rate);
        assert!(!flac.is_empty(), "пустой результат для тишины");
        assert_eq!(&flac[0..4], b"fLaC");
        // тишина отлично сжимается: заведомо меньше исходного PCM
        assert!(flac.len() < samples.len() * 2, "flac {} bytes", flac.len());
        // тон тоже кодируется непусто
        let tone: Vec<i16> = (0..1600)
            .map(|i| (f64::sin(i as f64 * 0.05) * 12000.0) as i16)
            .collect();
        let flac_tone = encode_pcm_to_flac(&tone, 16_000);
        assert!(!flac_tone.is_empty());
        assert_eq!(&flac_tone[0..4], b"fLaC");
    }

    #[test]
    fn google_lang_maps_short_codes_to_bcp47() {
        assert_eq!(google_lang("ru"), "ru-RU");
        assert_eq!(google_lang("EN"), "en-US");
        assert_eq!(google_lang(""), "en-US");
        assert_eq!(google_lang("  "), "en-US");
        // уже полный или неизвестный код — как есть
        assert_eq!(google_lang("ru-RU"), "ru-RU");
        assert_eq!(google_lang("xx"), "xx");
    }

    #[tokio::test]
    async fn google_rejects_too_long_before_network() {
        // 16 с тишины — больше лимита ~15 с; запрос не уходит (проверка до отправки)
        let wav = crate::recorder::encode_wav_i16(&vec![0i16; 16_000 * 16], 16_000);
        let err = transcribe_wav_google("ru", &wav)
            .await
            .expect_err("16 s must be rejected");
        assert!(matches!(err, AsrError::TooLong));
        assert!(matches_any_locale(err.to_string().as_str(), "err.too_long", &[]), "{err}");
    }

    #[test]
    fn provider_parses_google() {
        assert_eq!(Provider::from_str("google").unwrap(), Provider::Google);
        assert_eq!(Provider::from_str("Google").unwrap(), Provider::Google);
        assert_eq!(Provider::from_str("chromium").unwrap(), Provider::Google);
    }

    /// Живой запрос к Google speech-api (одноразовая проверка контракта).
    /// Запуск: cargo test google_live -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn google_live() {
        // 1 с тона 440 Гц: распознавания не ждём, проверяем контракт (Ok, возможно пусто)
        let tone: Vec<i16> = (0..16_000)
            .map(|i| {
                (f64::sin(2.0 * std::f64::consts::PI * 440.0 * i as f64 / 16_000.0) * 12000.0)
                    as i16
            })
            .collect();
        let wav = crate::recorder::encode_wav_i16(&tone, 16_000);
        let text = transcribe(Provider::Google, "", "", "", "ru", &wav)
            .await
            .expect("google live should respond");
        println!("google_live recognized: «{text}»");
    }

}
