use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::MentionConfig;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MentionSignal {
    pub market_ticker: String,
    pub event_id: Option<String>,
    #[serde(default)]
    pub event_day: Option<String>,
    pub phrase: Option<String>,
    pub p_hat: Option<f64>,
    pub p_lo: Option<f64>,
    pub p_hi: Option<f64>,
    pub model_version: Option<String>,
    pub features_as_of: Option<DateTime<Utc>>,
    pub data_freshness_sec: Option<f64>,
    pub calibration_n: usize,
    pub status: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub close_time: Option<DateTime<Utc>>,
}

impl MentionSignal {
    pub fn is_usable_at(&self, now: DateTime<Utc>, max_age: Duration) -> bool {
        if self.status != "ok" {
            return false;
        }
        let (Some(p_hat), Some(p_lo), Some(p_hi), Some(as_of), Some(model_version)) = (
            self.p_hat,
            self.p_lo,
            self.p_hi,
            self.features_as_of,
            self.model_version.as_ref(),
        ) else {
            return false;
        };
        if model_version.is_empty()
            || self.event_id.as_deref().unwrap_or_default().is_empty()
            || !p_hat.is_finite()
            || !p_lo.is_finite()
            || !p_hi.is_finite()
            || !(0.0..=1.0).contains(&p_lo)
            || !(0.0..=1.0).contains(&p_hat)
            || !(0.0..=1.0).contains(&p_hi)
            || p_lo > p_hat
            || p_hat > p_hi
            || as_of > now
        {
            return false;
        }
        (now - as_of).to_std().is_ok_and(|age| age <= max_age)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SignalVerdict {
    pub edge_estimate: Option<f64>,
    pub ci_low: Option<f64>,
    pub ci_high: Option<f64>,
    pub n_days: i64,
    pub as_of: Option<DateTime<Utc>>,
    pub model_version: Option<String>,
    pub live_gate_passed: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MentionHealth {
    pub status: String,
    pub model_version: Option<String>,
    pub last_corpus_update: Option<String>,
    pub latest_verdict: SignalVerdict,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct SignalSnapshot {
    received_at: Instant,
    signals: Vec<MentionSignal>,
    health: MentionHealth,
}

#[derive(Clone)]
pub struct MentionSignalClient {
    http: Client,
    base_url: String,
    max_age: Duration,
    cache: Arc<RwLock<Option<SignalSnapshot>>>,
}

impl MentionSignalClient {
    pub fn new(config: &MentionConfig) -> Result<Self> {
        let base_url =
            reqwest::Url::parse(&config.signal_url).context("Invalid mention signal URL")?;
        if base_url.scheme() != "http"
            || !matches!(base_url.host_str(), Some("127.0.0.1" | "localhost"))
        {
            anyhow::bail!("Mention signal service must use a localhost HTTP URL");
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .context("Failed to create mention signal HTTP client")?;
        Ok(Self {
            http,
            base_url: config.signal_url.trim_end_matches('/').to_string(),
            max_age: Duration::from_secs(config.max_signal_age_secs),
            cache: Arc::new(RwLock::new(None)),
        })
    }

    pub async fn refresh(&self) -> Result<()> {
        *self.cache.write() = None;
        let mut signals: Vec<MentionSignal> = self
            .http
            .get(format!("{}/signals", self.base_url))
            .send()
            .await
            .context("Mention /signals request failed")?
            .error_for_status()
            .context("Mention /signals returned an error")?
            .json()
            .await
            .context("Invalid mention /signals response")?;
        let health: MentionHealth = self
            .http
            .get(format!("{}/health", self.base_url))
            .send()
            .await
            .context("Mention /health request failed")?
            .error_for_status()
            .context("Mention /health returned an error")?
            .json()
            .await
            .context("Invalid mention /health response")?;
        signals.retain(|signal| signal.model_version.as_deref() == health.model_version.as_deref());
        *self.cache.write() = Some(SignalSnapshot {
            received_at: Instant::now(),
            signals,
            health,
        });
        Ok(())
    }

    fn current_snapshot(&self) -> Option<SignalSnapshot> {
        self.cache
            .read()
            .clone()
            .filter(|snapshot| snapshot.received_at.elapsed() <= self.max_age)
    }

    pub fn signals(&self) -> Vec<MentionSignal> {
        let now = Utc::now();
        self.current_snapshot()
            .map(|snapshot| {
                snapshot
                    .signals
                    .into_iter()
                    .filter(|signal| signal.is_usable_at(now, self.max_age))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn all_signals(&self) -> Vec<MentionSignal> {
        self.current_snapshot()
            .map(|snapshot| snapshot.signals)
            .unwrap_or_default()
    }

    pub fn health(&self) -> Option<MentionHealth> {
        self.current_snapshot().map(|snapshot| snapshot.health)
    }

    pub fn is_fresh(&self) -> bool {
        self.current_snapshot()
            .is_some_and(|snapshot| snapshot.health.status == "ok")
    }

    pub fn max_age(&self) -> Duration {
        self.max_age
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MentionSignal {
        MentionSignal {
            market_ticker: "KXTRUMPMENTION-26OCT01-CHINA".into(),
            event_id: Some("KXTRUMPMENTION-26OCT01".into()),
            event_day: Some("26OCT01".into()),
            phrase: Some("China".into()),
            p_hat: Some(0.7),
            p_lo: Some(0.6),
            p_hi: Some(0.8),
            model_version: Some("test".into()),
            features_as_of: Some(Utc::now()),
            data_freshness_sec: Some(2.0),
            calibration_n: 400,
            status: "ok".into(),
            reason: None,
            event_type: Some("speech".into()),
            close_time: None,
        }
    }

    #[test]
    fn only_fresh_well_formed_ok_signals_are_usable() {
        let mut signal = sample();
        let now = Utc::now();
        assert!(signal.is_usable_at(now, Duration::from_secs(30)));
        signal.status = "abstain".into();
        assert!(!signal.is_usable_at(now, Duration::from_secs(30)));
        signal.status = "ok".into();
        signal.p_lo = Some(0.9);
        assert!(!signal.is_usable_at(now, Duration::from_secs(30)));
    }

    #[test]
    fn service_url_must_be_loopback() {
        let mut config = MentionConfig::default();
        config.signal_url = "http://example.com".into();
        assert!(MentionSignalClient::new(&config).is_err());
    }
}
