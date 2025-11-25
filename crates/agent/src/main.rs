use anyhow::Result;
use axum::{extract::State, routing::get, Json, Router};
use common::{risk_score, Action, Config, IssueKind, Playbook, ValidatorId, ValidatorMetrics};
use redis::AsyncCommands;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};

const ACTION_QUEUE: &str = "actions:queue";
const MAX_RAM_GB: f64 = 128.0;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = common::load_config()?;
    let client = redis::Client::open(cfg.redis_url.clone())?;
    let agent_conn = redis::aio::ConnectionManager::new(client.clone()).await?;
    let http_conn = redis::aio::ConnectionManager::new(client).await?;

    let agent_cfg = cfg.clone();
    tokio::spawn(async move {
        if let Err(err) = run_agent(agent_cfg, agent_conn).await {
            error!(?err, "agent loop terminated");
        }
    });

    let app_state = AppState {
        redis: Arc::new(Mutex::new(http_conn)),
        config: Arc::new(cfg),
    };
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/debug/actions/pending", get(pending_actions))
        .route("/api/validators", get(list_validators))
        .route("/api/actions", get(actions_summary))
        .with_state(app_state)
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    info!("agent http listening on 0.0.0.0:3000");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn run_agent(cfg: Config, mut redis: redis::aio::ConnectionManager) -> Result<()> {
    let mut ticker = tokio::time::interval(Duration::from_secs(10));
    info!("agent loop started for {} validators", cfg.validators.len());
    loop {
        ticker.tick().await;
        for v in &cfg.validators {
            let key = format!("validator:metrics:{}", v.id.0);
            let payload: Option<String> = match redis.get(&key).await {
                Ok(val) => val,
                Err(err) => {
                    error!(validator = v.id.0, "failed to fetch metrics: {err:?}");
                    continue;
                }
            };
            let Some(payload) = payload else {
                info!(validator = v.id.0, "no metrics yet");
                continue;
            };
            let metrics: ValidatorMetrics = match serde_json::from_str(&payload) {
                Ok(m) => m,
                Err(err) => {
                    error!(validator = v.id.0, "invalid metrics payload: {err:?}");
                    continue;
                }
            };
            if let Some(issue) = detect_issue(&metrics) {
                let playbook = choose_playbook(issue, &v.id);
                info!(
                    validator = v.id.0,
                    issue = ?issue,
                    playbook = %playbook.id,
                    "issue detected, enqueuing actions"
                );
                for action in playbook.steps {
                    let action_json = serde_json::to_string(&action)?;
                    if let Err(err) = redis.rpush::<_, _, i64>(ACTION_QUEUE, action_json).await {
                        error!(validator = v.id.0, "failed to enqueue action: {err:?}");
                    }
                }
            }
        }
    }
}

async fn health() -> &'static str {
    "ok"
}

async fn pending_actions(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut conn = state.redis.lock().await;
    let pending = pending_count(&mut conn).await;
    Json(serde_json::json!({ "pending": pending }))
}

async fn actions_summary(State(state): State<AppState>) -> Json<ActionsResponse> {
    let mut conn = state.redis.lock().await;
    let pending = pending_count(&mut conn).await;
    Json(ActionsResponse { pending })
}

async fn list_validators(State(state): State<AppState>) -> Json<ValidatorsResponse> {
    let mut conn = state.redis.lock().await;
    let mut validators = Vec::with_capacity(state.config.validators.len());

    for cfg in &state.config.validators {
        let key = format!("validator:metrics:{}", cfg.id.0);
        let payload: Option<String> = conn.get(&key).await.unwrap_or(None);
        let (metrics, status, risk) = match payload {
            Some(json) => match serde_json::from_str::<ValidatorMetrics>(&json) {
                Ok(metrics) => {
                    let status = detect_issue(&metrics)
                        .map(|i| format!("{:?}", i))
                        .unwrap_or_else(|| "ok".into());
                    let risk = Some(risk_score(&metrics));
                    (Some(metrics), status, risk)
                }
                Err(err) => {
                    error!(validator = cfg.id.0, ?err, "failed to decode metrics");
                    (None, "invalid_metrics".into(), None)
                }
            },
            None => (None, "no_data".into(), None),
        };

        validators.push(ValidatorSummary {
            id: cfg.id.0.clone(),
            host: cfg.host.clone(),
            prometheus_url: cfg.prometheus_url.clone(),
            metrics,
            status,
            risk_score: risk,
        });
    }

    Json(ValidatorsResponse { validators })
}

#[derive(Clone)]
struct AppState {
    redis: Arc<Mutex<redis::aio::ConnectionManager>>,
    config: Arc<Config>,
}

#[derive(Serialize)]
struct ActionsResponse {
    pending: i64,
}

#[derive(Serialize)]
struct ValidatorsResponse {
    validators: Vec<ValidatorSummary>,
}

#[derive(Serialize)]
struct ValidatorSummary {
    id: String,
    host: String,
    prometheus_url: String,
    metrics: Option<ValidatorMetrics>,
    status: String,
    risk_score: Option<f64>,
}

async fn pending_count(conn: &mut redis::aio::ConnectionManager) -> i64 {
    conn.llen(ACTION_QUEUE).await.unwrap_or(0)
}

/// Detect issues using simple rule-based logic.
pub fn detect_issue(metrics: &ValidatorMetrics) -> Option<IssueKind> {
    if metrics.slot_lag > 50 {
        return Some(IssueKind::SlotLagHigh);
    }
    if metrics.vote_success_rate < 0.8 {
        return Some(IssueKind::VoteFailureSpike);
    }
    if metrics.cpu_usage > 0.9 || metrics.ram_usage_gb > 0.9 * MAX_RAM_GB {
        return Some(IssueKind::HardwareOverload);
    }
    if metrics.disk_usage_pct > 90.0 {
        return Some(IssueKind::DiskAlmostFull);
    }
    if metrics.rpc_qps > 1000.0 && metrics.rpc_error_rate > 0.05 {
        return Some(IssueKind::RpcOverload);
    }
    None
}

/// Hard-coded playbooks that can be swapped for learned policies later.
pub fn choose_playbook(issue: IssueKind, validator: &ValidatorId) -> Playbook {
    match issue {
        IssueKind::SlotLagHigh => Playbook {
            id: "slot-lag-recovery".into(),
            trigger: issue,
            steps: vec![
                Action::DisableRpc {
                    validator: validator.clone(),
                },
                Action::RestartValidator {
                    validator: validator.clone(),
                },
                Action::EnableRpc {
                    validator: validator.clone(),
                },
            ],
        },
        IssueKind::RpcOverload => Playbook {
            id: "rpc-overload".into(),
            trigger: issue,
            steps: vec![
                Action::ThrottleRpcClient {
                    validator: validator.clone(),
                },
                Action::SendAlert {
                    validator: validator.clone(),
                    message: "RPC overload detected".into(),
                },
            ],
        },
        IssueKind::DiskAlmostFull => Playbook {
            id: "disk-cleanup".into(),
            trigger: issue,
            steps: vec![Action::RunMaintenanceScript {
                validator: validator.clone(),
                script_name: "cleanup-logs.sh".into(),
            }],
        },
        IssueKind::HardwareOverload => Playbook {
            id: "hardware-throttle".into(),
            trigger: issue,
            steps: vec![
                Action::DisableRpc {
                    validator: validator.clone(),
                },
                Action::SendAlert {
                    validator: validator.clone(),
                    message: "Hardware overload detected".into(),
                },
            ],
        },
        IssueKind::VoteFailureSpike => Playbook {
            id: "vote-health".into(),
            trigger: issue,
            steps: vec![Action::SendAlert {
                validator: validator.clone(),
                message: "Vote success degraded".into(),
            }],
        },
        _ => Playbook {
            id: "unknown-issue".into(),
            trigger: issue,
            steps: vec![Action::SendAlert {
                validator: validator.clone(),
                message: "Unknown issue detected".into(),
            }],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_metrics() -> ValidatorMetrics {
        ValidatorMetrics {
            slot_lag: 0,
            vote_success_rate: 0.99,
            cpu_usage: 0.2,
            ram_usage_gb: 16.0,
            disk_usage_pct: 30.0,
            rpc_qps: 100.0,
            rpc_error_rate: 0.001,
            last_updated: 0,
        }
    }

    #[test]
    fn detects_slot_lag_issue() {
        let mut m = base_metrics();
        m.slot_lag = 75;
        assert_eq!(detect_issue(&m), Some(IssueKind::SlotLagHigh));
    }

    #[test]
    fn detects_vote_failure_issue() {
        let mut m = base_metrics();
        m.vote_success_rate = 0.5;
        assert_eq!(detect_issue(&m), Some(IssueKind::VoteFailureSpike));
    }

    #[test]
    fn detects_hardware_overload_issue() {
        let mut m = base_metrics();
        m.cpu_usage = 0.95;
        assert_eq!(detect_issue(&m), Some(IssueKind::HardwareOverload));
    }
}
