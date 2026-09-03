//! Локальная статистика диктовок: дневные бакеты в `stats.json`
//! (tauri-plugin-store). Приватность: данные не покидают устройство.

use std::collections::BTreeMap;
use std::sync::Mutex;

use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

const STORE_PATH: &str = "stats.json";
const STORE_KEY: &str = "stats";
/// Дневные бакеты старше этого окна удаляются при записи.
const RETENTION_DAYS: i64 = 90;
/// Длина чарта в днях (включая сегодня).
const CHART_DAYS: i64 = 14;

/// Сериализует read-modify-write: параллельные диктовки не должны
/// затирать бакеты друг друга.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DayStats {
    pub words: u64,
    pub chars: u64,
    pub sessions: u64,
    pub audio_sec: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsData {
    /// Дневные бакеты, ключ — локальная дата «YYYY-MM-DD»
    /// (лексикографический порядок совпадает с хронологическим).
    pub days: BTreeMap<String, DayStats>,
    /// Пожизненный итог: переживает ротацию дневных бакетов.
    pub lifetime: DayStats,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartPoint {
    pub date: String,
    pub words: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsSummary {
    pub today: DayStats,
    pub week_words: u64,
    pub lifetime: DayStats,
    pub streak_days: u64,
    pub best_day_words: u64,
    pub minutes_saved_today: u64,
    pub minutes_saved_total: u64,
    pub chart: Vec<ChartPoint>,
}

fn date_key(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn count_words(text: &str) -> u64 {
    text.split_whitespace().count() as u64
}

/// Сэкономленные минуты печати: слова / скорость_печати (слов/мин).
/// Нулевая скорость из настроек не должна ломать формулу — климпим.
pub fn minutes_saved(words: u64, wpm: u32) -> u64 {
    if words == 0 {
        return 0;
    }
    let wpm = wpm.clamp(1, 300) as f64;
    (words as f64 / wpm).round() as u64
}

/// Серия дней подряд с диктовками (сегодняшний пустой день не рвёт серию).
pub fn streak_days(days: &BTreeMap<String, DayStats>, today: NaiveDate) -> u64 {
    let mut d = today;
    if !days.contains_key(&date_key(d)) {
        d -= chrono::Duration::days(1);
    }
    let mut streak = 0;
    while days.contains_key(&date_key(d)) {
        streak += 1;
        d -= chrono::Duration::days(1);
    }
    streak
}

/// Бакеты старше RETENTION_DAYS удаляются (словарные ключи = хронология).
fn prune(data: &mut StatsData, today: NaiveDate) {
    let cutoff = date_key(today - chrono::Duration::days(RETENTION_DAYS));
    let stale: Vec<String> = data
        .days
        .keys()
        .take_while(|k| k.as_str() < cutoff.as_str())
        .cloned()
        .collect();
    for k in stale {
        data.days.remove(&k);
    }
}

/// Учёт успешной диктовки; возвращает обновлённые показатели за сегодня.
pub fn record_dictation(app: &AppHandle, words: u64, chars: u64, audio_sec: f64) -> DayStats {
    let _guard = WRITE_LOCK.lock();
    let today = Local::now().date_naive();
    let mut data = load_data(app);
    prune(&mut data, today);
    let day = data.days.entry(date_key(today)).or_default();
    day.words += words;
    day.chars += chars;
    day.sessions += 1;
    day.audio_sec += audio_sec;
    data.lifetime.words += words;
    data.lifetime.chars += chars;
    data.lifetime.sessions += 1;
    data.lifetime.audio_sec += audio_sec;
    let today_stats = *day;
    save_data(app, &data);
    today_stats
}

pub fn summarize(data: &StatsData, today: NaiveDate, wpm: u32) -> StatsSummary {
    let today_key = date_key(today);
    let today_stats = data.days.get(&today_key).copied().unwrap_or_default();
    let week_start = date_key(today - chrono::Duration::days(6));
    let week_words: u64 = data
        .days
        .range(week_start..=today_key)
        .map(|(_, d)| d.words)
        .sum();
    let chart: Vec<ChartPoint> = (0..CHART_DAYS)
        .rev()
        .map(|off| {
            let key = date_key(today - chrono::Duration::days(off));
            let words = data.days.get(&key).map(|d| d.words).unwrap_or(0);
            ChartPoint { date: key, words }
        })
        .collect();
    StatsSummary {
        today: today_stats,
        week_words,
        lifetime: data.lifetime,
        streak_days: streak_days(&data.days, today),
        best_day_words: data.days.values().map(|d| d.words).max().unwrap_or(0),
        minutes_saved_today: minutes_saved(today_stats.words, wpm),
        minutes_saved_total: minutes_saved(data.lifetime.words, wpm),
        chart,
    }
}

fn load_data(app: &AppHandle) -> StatsData {
    app.store(STORE_PATH)
        .ok()
        .and_then(|store| store.get(STORE_KEY))
        .and_then(|value| serde_json::from_value::<StatsData>(value).ok())
        .unwrap_or_default()
}

fn save_data(app: &AppHandle, data: &StatsData) {
    let Ok(store) = app.store(STORE_PATH) else {
        log::error!("[stats] store `{STORE_PATH}` недоступен");
        return;
    };
    match serde_json::to_value(data) {
        Ok(value) => {
            store.set(STORE_KEY.to_string(), value);
            if let Err(e) = store.save() {
                log::error!("[stats] не удалось сохранить: {e}");
            }
        }
        Err(e) => log::error!("[stats] не удалось сериализовать: {e}"),
    }
}

#[tauri::command]
pub async fn get_stats(app: AppHandle) -> StatsSummary {
    let wpm = crate::settings::load_settings(&app).typing_speed_wpm;
    summarize(&load_data(&app), Local::now().date_naive(), wpm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(words: u64) -> DayStats {
        DayStats {
            words,
            chars: words * 5,
            sessions: 1,
            audio_sec: words as f64,
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn counts_words() {
        assert_eq!(count_words("привет мир"), 2);
        assert_eq!(count_words("  много   пробелов  "), 2);
        assert_eq!(count_words(""), 0);
    }

    #[test]
    fn minutes_saved_basic() {
        assert_eq!(minutes_saved(1200, 40), 30);
        assert_eq!(minutes_saved(0, 40), 0);
        assert_eq!(minutes_saved(10, 0), 10); // wpm=0 климпится до 1
        assert_eq!(minutes_saved(100, 40), 3); // 2.5 → 3
    }

    #[test]
    fn streak_counts_consecutive_days() {
        let today = date(2026, 9, 4);
        let mut days = BTreeMap::new();
        for off in 0..3 {
            days.insert(date_key(today - chrono::Duration::days(off)), day(10));
        }
        assert_eq!(streak_days(&days, today), 3);
    }

    #[test]
    fn streak_survives_empty_today() {
        let today = date(2026, 9, 4);
        let mut days = BTreeMap::new();
        days.insert(date_key(today - chrono::Duration::days(1)), day(10));
        days.insert(date_key(today - chrono::Duration::days(2)), day(10));
        assert_eq!(streak_days(&days, today), 2);
    }

    #[test]
    fn streak_breaks_on_gap() {
        let today = date(2026, 9, 4);
        let mut days = BTreeMap::new();
        days.insert(date_key(today), day(10));
        days.insert(date_key(today - chrono::Duration::days(2)), day(10));
        assert_eq!(streak_days(&days, today), 1);
        assert_eq!(streak_days(&BTreeMap::new(), today), 0);
    }

    #[test]
    fn summarize_aggregates_week_chart_and_best_day() {
        let today = date(2026, 9, 4);
        let mut days = BTreeMap::new();
        days.insert(date_key(today), day(100));
        days.insert(date_key(today - chrono::Duration::days(6)), day(50));
        days.insert(date_key(today - chrono::Duration::days(7)), day(999)); // вне недели
        let data = StatsData {
            days,
            lifetime: day(1),
        };
        let s = summarize(&data, today, 40);
        assert_eq!(s.week_words, 150);
        assert_eq!(s.today.words, 100);
        assert_eq!(s.best_day_words, 999);
        assert_eq!(s.minutes_saved_today, 3);
        assert_eq!(s.chart.len(), 14);
        assert_eq!(s.chart.last().unwrap().words, 100);
        assert_eq!(s.chart.first().unwrap().words, 0);
    }

    #[test]
    fn prune_removes_stale_buckets() {
        let today = date(2026, 9, 4);
        let mut data = StatsData::default();
        data.days.insert(date_key(today), day(1));
        data.days
            .insert(date_key(today - chrono::Duration::days(RETENTION_DAYS + 1)), day(1));
        prune(&mut data, today);
        assert_eq!(data.days.len(), 1);
        assert_eq!(data.days.keys().next().unwrap(), &date_key(today));
    }

    #[test]
    fn stats_data_roundtrips_camelcase() {
        let mut data = StatsData::default();
        data.days.insert("2026-09-04".to_string(), day(3));
        let json = serde_json::to_value(&data).unwrap();
        assert!(json["days"]["2026-09-04"]["audioSec"].is_number());
        let back: StatsData = serde_json::from_value(json).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn empty_store_deserializes_to_default() {
        let d: StatsData = serde_json::from_str("{}").unwrap_or_default();
        assert!(d.days.is_empty());
        assert_eq!(d.lifetime, DayStats::default());
    }
}
