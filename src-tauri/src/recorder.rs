use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, InputCallbackInfo, Sample, SampleFormat, SupportedStreamConfig};

/// Целевая частота для ASR: 16 кГц, моно, 16-bit PCM (см. PLAN.md «Параметры аудио»).
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug)]
pub enum RecorderError {
    /// Ни одно входное устройство не найдено
    NoInputDevice,
    /// Не удалось собрать поток (ошибка cpal)
    Build(String),
    /// Формат сэмплов устройства не поддерживается
    UnsupportedFormat(String),
}

impl fmt::Display for RecorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            RecorderError::NoInputDevice => {
                "Микрофон не найден — проверьте, что входное устройство подключено и разрешено (macOS: Настройки → Конфиденциальность → Микрофон)"
            }
            RecorderError::Build(e) => {
                return write!(f, "Не удалось открыть поток записи: {e}")
            }
            RecorderError::UnsupportedFormat(fmt) => {
                return write!(f, "Неподдерживаемый формат сэмплов устройства: {fmt}")
            }
        };
        f.write_str(msg)
    }
}

impl std::error::Error for RecorderError {}

/// Активная сессия записи: cpal-стрим нужно держать живым до остановки.
struct ActiveRecording {
    stream: cpal::Stream,
    samples: Arc<Mutex<Vec<i16>>>,
    rate: u32,
    started: Instant,
}

// cpal::Stream намеренно !Send; мы владеем им только через Mutex внутри одного
// процесса, а start/stop вызываются из обработчика глобальных хоткеев.
unsafe impl Send for ActiveRecording {}

/// Managed-state: состояние записи микрофона (push-to-talk).
#[derive(Default)]
pub struct Recorder {
    active: Mutex<Option<ActiveRecording>>,
}

/// Результат завершённой записи.
pub struct Recorded {
    /// WAV 16 кГц моно 16-bit, готовый к отправке
    pub wav: Vec<u8>,
    /// Длительность удержания клавиши
    pub duration: std::time::Duration,
    /// Сколько сэмпловmono захвачено на частоте устройства
    pub raw_samples: usize,
    /// Частота захвата устройства
    pub device_rate: u32,
}

impl Recorder {
    pub fn is_recording(&self) -> bool {
        self.active.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Старт захвата. Повторный вызов во время активной записи — no-op (защита от
    /// Pressed без Released).
    pub fn start(&self) -> Result<(), RecorderError> {
        if self.is_recording() {
            return Ok(());
        }
        let (device, config) = pick_input_config()?;
        let rate = config.sample_rate();
        let channels = config.channels().max(1) as usize;
        let samples: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));

        let stream = build_input_stream(&device, &config, samples.clone(), channels)?;
        stream.play().map_err(|e| RecorderError::Build(e.to_string()))?;

        let active = ActiveRecording {
            stream,
            samples,
            rate,
            started: Instant::now(),
        };
        let mut guard = self.active.lock().map_err(|_| RecorderError::Build("state poisoned".into()))?;
        *guard = Some(active);
        log(&format!(
            "recording started: {channels} ch @ {rate} Hz"
        ));
        Ok(())
    }

    /// Стоп захвата; возвращает None, если записи не было.
    pub fn stop(&self) -> Option<Recorded> {
        let active = self.active.lock().ok()?.take()?;
        let ActiveRecording { stream, samples, rate, started } = active;
        // Drop останавливает поток; после этого колбэк больше не пишет.
        drop(stream);
        let duration = started.elapsed();
        let raw = samples
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default();
        let mono16k = resample_linear(&raw, rate, TARGET_SAMPLE_RATE);
        let wav = encode_wav_i16(&mono16k, TARGET_SAMPLE_RATE);
        Some(Recorded {
            wav,
            duration,
            raw_samples: raw.len(),
            device_rate: rate,
        })
    }
}

fn log(msg: &str) {
    println!("[recorder] {msg}");
}

/// Best-effort выбор устройства и конфига: сначала пробуем найти 16 кГц на любом
/// входном устройстве (моно предпочтительно), иначе — первое попавшееся входное
/// устройство с его дефолтным конфигом (частоту приведём ресемплингом на стопе).
fn pick_input_config() -> Result<(cpal::Device, SupportedStreamConfig), RecorderError> {
    let host = cpal::default_host();
    let mut fallback: Option<(cpal::Device, SupportedStreamConfig)> = None;

    let mut candidates: Vec<cpal::Device> = Vec::new();
    if let Some(d) = host.default_input_device() {
        candidates.push(d);
    }
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if !candidates.iter().any(|c| *c == device) {
                candidates.push(device);
            }
        }
    }

    for device in candidates {
        let Ok(configs) = device.supported_input_configs() else {
            continue;
        };
        let mut mono_16k = None;
        let mut any_16k = None;
        let mut first = None;
        for range in configs {
            if first.is_none() {
                first = Some(range.clone());
            }
            if !range.contains_rate(TARGET_SAMPLE_RATE) {
                continue;
            }
            let cfg = range.clone().with_sample_rate(TARGET_SAMPLE_RATE);
            if range.channels() == 1 && mono_16k.is_none() {
                mono_16k = Some(cfg);
            } else if any_16k.is_none() {
                any_16k = Some(cfg);
            }
        }
        if let Some(config) = mono_16k.or(any_16k) {
            return Ok((device, config));
        }
        if fallback.is_none() {
            if let Some(range) = first {
                fallback = Some((device, range.with_max_sample_rate()));
            }
        }
    }
    fallback.ok_or(RecorderError::NoInputDevice)
}

/// Сборка входного потока с конверсией в моно-i16 на лету (даунмикс каналов).
fn build_input_stream(
    device: &cpal::Device,
    config: &SupportedStreamConfig,
    samples: Arc<Mutex<Vec<i16>>>,
    channels: usize,
) -> Result<cpal::Stream, RecorderError> {
    let err_fn = |err: cpal::Error| eprintln!("[recorder] stream error: {err}");
    let stream_config = config.config();

    macro_rules! build {
        ($ty:ty) => {{
            let buf = samples.clone();
            device.build_input_stream(
                stream_config.clone(),
                move |data: &[$ty], _: &InputCallbackInfo| {
                    capture_into(&buf, data, channels);
                },
                err_fn,
                None,
            )
        }};
    }

    let format = config.sample_format();
    let stream = match format {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I8 => build!(i8),
        other => return Err(RecorderError::UnsupportedFormat(other.to_string())),
    };
    stream.map_err(|e| RecorderError::Build(e.to_string()))
}

/// Перевод сырых сэмплов устройства в моно-i16 и дописывание в буфер.
fn capture_into<S>(buf: &Arc<Mutex<Vec<i16>>>, data: &[S], channels: usize)
where
    S: Sample,
    f32: FromSample<S>,
{
    let Ok(mut guard) = buf.try_lock() else { return };
    if channels <= 1 {
        guard.extend(data.iter().map(|&s| f32_to_i16(f32::from_sample(s))));
    } else {
        for frame in data.chunks(channels) {
            let mean =
                frame.iter().map(|&s| f32::from_sample(s)).sum::<f32>() / frame.len() as f32;
            guard.push(f32_to_i16(mean));
        }
    }
}

#[inline]
fn f32_to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Линейный ресемплинг моно-буфера i16 (например, 48 кГц → 16 кГц).
pub fn resample_linear(input: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if input.is_empty() || from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }
    if from_rate == to_rate {
        return input.to_vec();
    }
    let out_len = (input.len() as u64 * to_rate as u64 / from_rate as u64) as usize;
    let ratio = from_rate as f64 / to_rate as f64;
    let last = input.len() - 1;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let i0 = pos.floor() as usize;
            let frac = pos - i0 as f64;
            let i1 = (i0 + 1).min(last);
            let v = input[i0.min(last)] as f64 * (1.0 - frac) + input[i1] as f64 * frac;
            (v.clamp(i16::MIN as f64, i16::MAX as f64)) as i16
        })
        .collect()
}

/// Кодировка моно 16-bit PCM в WAV (RIFF-заголовок 44 байта + данные).
/// Общий хелпер для рекордера и тест-тишины в asr.rs.
pub fn encode_wav_i16(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    const CHANNELS: u16 = 1;
    const BITS_PER_SAMPLE: u16 = 16;
    let data_len = (samples.len() * CHANNELS as usize * (BITS_PER_SAMPLE / 8) as usize) as u32;
    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes()); // RIFF chunk size
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&CHANNELS.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * CHANNELS as u32 * (BITS_PER_SAMPLE as u32 / 8);
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&(CHANNELS * BITS_PER_SAMPLE / 8).to_le_bytes()); // block align
    buf.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_matches_samples() {
        let samples: Vec<i16> = (0..320).map(|i| (i * 100 - 16000) as i16).collect();
        let wav = encode_wav_i16(&samples, 16_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(data_len, samples.len() * 2);
        assert_eq!(wav.len(), 44 + data_len);
        let riff_len = u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff_len + 8, wav.len());
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1); // PCM
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1); // mono
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32_000); // byte rate
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2); // block align
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16); // bits
        // данные — LE-копия сэмплов
        for (i, &s) in samples.iter().enumerate() {
            let got = i16::from_le_bytes(wav[44 + i * 2..46 + i * 2].try_into().unwrap());
            assert_eq!(got, s, "sample {i}");
        }
    }

    #[test]
    fn wav_empty_is_valid_header_only() {
        let wav = encode_wav_i16(&[], 16_000);
        assert_eq!(wav.len(), 44);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0);
    }

    #[test]
    fn resample_same_rate_is_identity() {
        let input: Vec<i16> = (0..100).map(|i| i * 10).collect();
        assert_eq!(resample_linear(&input, 16_000, 16_000), input);
    }

    #[test]
    fn resample_48k_to_16k_length_and_linear_trend() {
        // 10 мс @ 48 кГц = 480 сэмплов → 160 @ 16 кГц
        let input: Vec<i16> = (0..480).map(|i| (i * 50).clamp(-32768, 32767) as i16).collect();
        let out = resample_linear(&input, 48_000, 16_000);
        assert_eq!(out.len(), 160);
        // Линейный сигнал при линейной интерполяции воспроизводится точно:
        // out[j] соответствует input[j*3]
        for j in 0..out.len() {
            assert_eq!(out[j], input[j * 3], "out[{j}]");
        }
        // Монотонность сохранена
        assert!(out.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn resample_48k_to_16k_sine_stays_close() {
        // Синус 1 кГц, 480 сэмплов (10 мс) при 48 кГц
        let input: Vec<i16> = (0..480)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                (10_000.0 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) as i16
            })
            .collect();
        let out = resample_linear(&input, 48_000, 16_000);
        assert_eq!(out.len(), 160);
        // Эталон — тот же синус, взятый на_positions выходов с частотой 16 кГц
        let mut max_err = 0f64;
        for (j, &v) in out.iter().enumerate() {
            let t = j as f64 / 16_000.0;
            let reference = 10_000.0 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin();
            max_err = max_err.max((v as f64 - reference).abs());
        }
        // Погрешность линейной интерполяции синуса 1 кГц @ 48 кГц мала (<0.5% амплитуды)
        assert!(max_err < 50.0, "max_err = {max_err}");
    }

    #[test]
    fn f32_to_i16_clamps() {
        assert_eq!(f32_to_i16(2.0), 32767);
        assert_eq!(f32_to_i16(-2.0), -32767);
        assert_eq!(f32_to_i16(1.0), 32767);
        assert_eq!(f32_to_i16(0.0), 0);
    }

    #[test]
    fn capture_downmixes_channels() {
        let buf = Arc::new(Mutex::new(Vec::<i16>::new()));
        // два канала, значения 0.5 и -0.5 → средний 0
        let data: [f32; 4] = [0.5, -0.5, 0.5, -0.5];
        capture_into(&buf, &data, 2);
        let g = buf.lock().unwrap();
        assert_eq!(g.len(), 2);
        assert!(g.iter().all(|&v| v.abs() <= 16)); // ~0 с учётом округления
    }
}
