use anyhow::Result;
use common::ValidatorMetrics;
use executor::proto::executor_client::ExecutorClient;
use executor::proto::MetricsWatchRequest;
use redis::AsyncCommands;
use std::env;
use tracing::{error, info};

const DEFAULT_SERVER_ADDR: &str = "http://127.0.0.1:50051";

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
    let server_addr =
        env::var("EXECUTOR_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_SERVER_ADDR.to_string());
    let mut grpc = ExecutorClient::connect(server_addr.clone())
        .await
        .map_err(|err| anyhow::anyhow!("failed to connect to executor daemon: {err}"))?;

    info!(
        "metrics collector writing Redis metrics for {} validators",
        cfg.validators.len()
    );

    let request = tonic::Request::new(MetricsWatchRequest {
        validator_ids: vec![],
        include_snapshot: true,
    });
    let mut stream = grpc.subscribe_metrics(request).await?.into_inner();

    while let Some(update) = stream.message().await? {
        match serde_json::from_str::<ValidatorMetrics>(&update.metrics_json) {
            Ok(metrics) => {
                let key = format!("validator:metrics:{}", update.validator_id);
                let payload = serde_json::to_string(&metrics)?;
                if let Err(err) = conn.set::<_, _, ()>(&key, payload).await {
                    error!(
                        validator = update.validator_id,
                        ?err,
                        "failed to persist metrics"
                    );
                } else {
                    info!(validator = update.validator_id, "metrics synced to redis");
                }
            }
            Err(err) => {
                error!(
                    validator = update.validator_id,
                    ?err,
                    "failed to decode metrics payload"
                );
            }
        }
    }
    Ok(())
}
