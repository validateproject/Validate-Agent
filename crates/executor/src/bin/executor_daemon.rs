use anyhow::Result;
use executor::execute_action;
use redis::AsyncCommands;
use tracing::{error, info};

const ACTION_QUEUE: &str = "actions:queue";
const ACTION_HISTORY: &str = "actions:history";

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

    info!("executor daemon listening for actions on {}", ACTION_QUEUE);
    loop {
        let result: Option<(String, String)> = conn.blpop(ACTION_QUEUE, 0.0).await?;
        let Some((_, payload)) = result else {
            continue;
        };
        let action: common::Action = match serde_json::from_str(&payload) {
            Ok(a) => a,
            Err(err) => {
                error!("failed to decode action: {err:?}");
                continue;
            }
        };

        match execute_action(action.clone()).await {
            Ok(_) => {
                info!("action executed successfully");
                if let Err(err) = conn
                    .lpush::<_, _, i64>(
                        ACTION_HISTORY,
                        serde_json::json!({ "action": action, "status": "ok" }).to_string(),
                    )
                    .await
                {
                    error!(?err, "failed to record successful action");
                }
            }
            Err(err) => {
                error!(?err, "action execution failed");
                if let Err(push_err) = conn
                    .lpush::<_, _, i64>(
                        ACTION_HISTORY,
                        serde_json::json!({
                            "action": action,
                            "status": "error",
                            "message": err.to_string()
                        })
                        .to_string(),
                    )
                    .await
                {
                    error!(?push_err, "failed to record failed action");
                }
            }
        }
    }
}
