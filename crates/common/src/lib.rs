use anyhow::Result;
use config::Config as RawConfig;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ValidatorId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidatorMetrics {
    pub slot_lag: i64,
    pub vote_success_rate: f64,
    pub cpu_usage: f64,
    pub ram_usage_gb: f64,
    pub disk_usage_pct: f64,
    pub rpc_qps: f64,
    pub rpc_error_rate: f64,
    pub last_updated: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    SlotLagHigh,
    VoteFailureSpike,
    HardwareOverload,
    DiskAlmostFull,
    RpcOverload,
    NetworkUnstable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    DisableRpc {
        validator: ValidatorId,
    },
    EnableRpc {
        validator: ValidatorId,
    },
    RestartValidator {
        validator: ValidatorId,
    },
    ThrottleRpcClient {
        validator: ValidatorId,
    },
    RunMaintenanceScript {
        validator: ValidatorId,
        script_name: String,
    },
    SendAlert {
        validator: ValidatorId,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Playbook {
    pub id: String,
    pub trigger: IssueKind,
    pub steps: Vec<Action>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidatorConfig {
    pub id: ValidatorId,
    pub host: String,
    pub prometheus_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub validators: Vec<ValidatorConfig>,
    pub redis_url: String,
}

pub fn load_config() -> Result<Config> {
    let settings = RawConfig::builder()
        .add_source(config::File::with_name("config").required(true))
        .add_source(config::Environment::with_prefix("VALIDATOR_COPILOT").separator("__"))
        .build()?;
    let cfg: Config = settings.try_deserialize()?;
    Ok(cfg)
}

/// Compute a rough risk score for a validator. Higher means riskier.
pub fn risk_score(metrics: &ValidatorMetrics) -> f64 {
    let mut score = 0.0;
    score += (metrics.slot_lag.max(0) as f64 / 100.0).min(1.0) * 0.25;
    score += ((1.0 - metrics.vote_success_rate).max(0.0)).min(1.0) * 0.2;
    score += metrics.cpu_usage.clamp(0.0, 1.0) * 0.15;
    score += (metrics.disk_usage_pct / 100.0).clamp(0.0, 1.0) * 0.1;
    score += (metrics.rpc_error_rate).clamp(0.0, 1.0) * 0.1;
    score += (metrics.rpc_qps / 2000.0).min(1.0) * 0.1;
    score += (metrics.ram_usage_gb / 128.0).min(1.0) * 0.1;
    score.min(1.0)
}

pub fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_score_increases_with_slot_lag() {
        let base = ValidatorMetrics {
            slot_lag: 0,
            vote_success_rate: 0.99,
            cpu_usage: 0.2,
            ram_usage_gb: 16.0,
            disk_usage_pct: 40.0,
            rpc_qps: 100.0,
            rpc_error_rate: 0.001,
            last_updated: 0,
        };
        let low = risk_score(&base);
        let mut degraded = base.clone();
        degraded.slot_lag = 200;
        let high = risk_score(&degraded);
        assert!(high > low);
        assert!(high <= 1.0);
    }

    #[test]
    fn validator_metrics_serde_roundtrip() {
        let metrics = ValidatorMetrics {
            slot_lag: 10,
            vote_success_rate: 0.95,
            cpu_usage: 0.5,
            ram_usage_gb: 32.0,
            disk_usage_pct: 55.0,
            rpc_qps: 500.0,
            rpc_error_rate: 0.01,
            last_updated: 123456,
        };
        let json = serde_json::to_string(&metrics).expect("serialize");
        let back: ValidatorMetrics = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(metrics, back);
    }
}
