use anyhow::Result;
use common::{now_ts, ValidatorConfig, ValidatorMetrics};
use rand::Rng;
use redis::AsyncCommands;
use std::time::Duration;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = common::load_config()?;
    let client = redis::Client::open(cfg.redis_url.clone())?;
    let mut conn = redis::aio::ConnectionManager::new(client).await?;

    info!(
        "starting metrics collector for {} validators",
        cfg.validators.len()
    );
    let mut ticker = tokio::time::interval(Duration::from_secs(10));
    loop {
        ticker.tick().await;
        for validator in &cfg.validators {
            if let Err(err) = collect_metrics_for_validator(validator, &mut conn).await {
                error!(
                    validator = validator.id.0,
                    "failed collecting metrics: {err:?}"
                );
            }
        }
    }
}

/// Simulates scraping metrics and persists them into Redis.
pub async fn collect_metrics_for_validator(
    cfg: &ValidatorConfig,
    redis: &mut redis::aio::ConnectionManager,
) -> Result<()> {
    let mut rng = rand::thread_rng();
    let metrics = ValidatorMetrics {
        slot_lag: rng.gen_range(0..150),
        vote_success_rate: rng.gen_range(80..100) as f64 / 100.0,
        cpu_usage: rng.gen_range(10..95) as f64 / 100.0,
        ram_usage_gb: rng.gen_range(8..96) as f64,
        disk_usage_pct: rng.gen_range(20..95) as f64,
        rpc_qps: rng.gen_range(0..1500) as f64,
        rpc_error_rate: rng.gen_range(0..10) as f64 / 100.0,
        last_updated: now_ts(),
    };

    let key = format!("validator:metrics:{}", cfg.id.0);
    let payload = serde_json::to_string(&metrics)?;
    redis.set::<_, _, ()>(&key, payload).await?;
    info!(validator = cfg.id.0, host = %cfg.host, "metrics updated");
    Ok(())
}
