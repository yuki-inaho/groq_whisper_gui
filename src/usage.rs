use crate::config::{WHISPER_MODEL_V3, WHISPER_MODEL_V3_TURBO};
use std::collections::BTreeMap;
use std::time::Duration;

// Groq Speech-to-Text pricing is audio-time based. These values are used only
// for local estimates; the Groq Console remains the source of truth for billing.
const MIN_BILLABLE_AUDIO_SECONDS: f64 = 10.0;
const WHISPER_V3_TURBO_USD_PER_HOUR: f64 = 0.04;
const WHISPER_V3_USD_PER_HOUR: f64 = 0.111;

#[derive(Debug, Clone, Default)]
pub struct SessionUsage {
    by_model: BTreeMap<String, ModelUsage>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelUsage {
    pub requests: u32,
    pub actual_seconds: f64,
    pub billable_seconds: f64,
}

impl SessionUsage {
    pub fn record_transcription(&mut self, model: &str, actual_duration: Duration) {
        let entry = self.by_model.entry(model.to_string()).or_default();
        entry.requests += 1;
        entry.actual_seconds += actual_duration.as_secs_f64();
        entry.billable_seconds += billable_audio_seconds(actual_duration);
    }

    pub fn is_empty(&self) -> bool {
        self.by_model.is_empty()
    }

    pub fn model_usages(&self) -> impl Iterator<Item = (&str, &ModelUsage)> {
        self.by_model
            .iter()
            .map(|(model, usage)| (model.as_str(), usage))
    }

    pub fn total_requests(&self) -> u32 {
        self.by_model.values().map(|usage| usage.requests).sum()
    }

    pub fn total_actual_seconds(&self) -> f64 {
        self.by_model
            .values()
            .map(|usage| usage.actual_seconds)
            .sum()
    }

    pub fn total_billable_seconds(&self) -> f64 {
        self.by_model
            .values()
            .map(|usage| usage.billable_seconds)
            .sum()
    }

    pub fn estimated_total_cost_usd(&self) -> f64 {
        self.by_model
            .iter()
            .filter_map(|(model, usage)| estimate_cost_usd(model, usage.billable_seconds))
            .sum()
    }

    pub fn has_unknown_price(&self) -> bool {
        self.by_model
            .keys()
            .any(|model| model_price_per_hour_usd(model).is_none())
    }
}

impl ModelUsage {
    pub fn estimated_cost_usd(&self, model: &str) -> Option<f64> {
        estimate_cost_usd(model, self.billable_seconds)
    }
}

pub fn billable_audio_seconds(actual_duration: Duration) -> f64 {
    actual_duration
        .as_secs_f64()
        .max(MIN_BILLABLE_AUDIO_SECONDS)
}

pub fn model_price_per_hour_usd(model: &str) -> Option<f64> {
    match model {
        WHISPER_MODEL_V3_TURBO => Some(WHISPER_V3_TURBO_USD_PER_HOUR),
        WHISPER_MODEL_V3 => Some(WHISPER_V3_USD_PER_HOUR),
        _ => None,
    }
}

pub fn estimate_cost_usd(model: &str, billable_seconds: f64) -> Option<f64> {
    model_price_per_hour_usd(model).map(|hourly| (billable_seconds / 3600.0) * hourly)
}

pub fn format_seconds(seconds: f64) -> String {
    let total_seconds = seconds.round() as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}時間{minutes:02}分{seconds:02}秒")
    } else if minutes > 0 {
        format!("{minutes}分{seconds:02}秒")
    } else {
        format!("{seconds}秒")
    }
}

pub fn format_usd(cost: f64) -> String {
    if cost < 0.01 {
        format!("${cost:.5}")
    } else {
        format!("${cost:.4}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn billable_seconds_has_ten_second_minimum() {
        assert_eq!(billable_audio_seconds(Duration::from_secs(3)), 10.0);
        assert_eq!(billable_audio_seconds(Duration::from_secs(12)), 12.0);
    }

    #[test]
    fn session_usage_estimates_known_model_costs() {
        let mut usage = SessionUsage::default();
        usage.record_transcription(WHISPER_MODEL_V3_TURBO, Duration::from_secs(3));
        usage.record_transcription(WHISPER_MODEL_V3, Duration::from_secs(60));

        assert_eq!(usage.total_requests(), 2);
        assert_eq!(usage.total_billable_seconds(), 70.0);
        let expected = (10.0 / 3600.0 * 0.04) + (60.0 / 3600.0 * 0.111);
        assert!((usage.estimated_total_cost_usd() - expected).abs() < f64::EPSILON);
        assert!(!usage.has_unknown_price());
    }

    #[test]
    fn unknown_model_keeps_time_but_excludes_cost() {
        let mut usage = SessionUsage::default();
        usage.record_transcription("custom-whisper", Duration::from_secs(20));

        assert_eq!(usage.total_actual_seconds(), 20.0);
        assert_eq!(usage.estimated_total_cost_usd(), 0.0);
        assert!(usage.has_unknown_price());
    }
}
