use anyhow::{Context, Result};
use axum::{extract::State, routing::get, Json, Router};
use common::{
    risk_score, Action, Config, IssueKind, Playbook, ValidatorConfig, ValidatorId, ValidatorMetrics,
};
use executor::proto::executor_client::ExecutorClient;
use executor::proto::{ActionEnvelope, MetricsWatchRequest};
use serde::Serialize;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::interval;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};

const ACTION_POLL_INTERVAL_SECS: u64 = 10;
const MAX_RAM_GB: f64 = 128.0;
const DEFAULT_SERVER_ADDR: &str = "http://127.0.0.1:50051";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = Arc::new(common::load_config()?);
    let server_addr =
        env::var("EXECUTOR_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_SERVER_ADDR.to_string());
    let channel = tonic::transport::Endpoint::from_shared(server_addr.clone())?
        .connect()
        .await
        .context("failed to connect to executor daemon")?;
    let metrics_client = ExecutorClient::new(channel.clone());
    let action_client = ExecutorClient::new(channel);

    let metrics_cache = MetricsCache::default();

    let metrics_task_cache = metrics_cache.clone();
    tokio::spawn(async move {
        subscribe_metrics_loop(metrics_client, metrics_task_cache).await;
    });
    let agent_cfg = cfg.clone();
    let agent_metrics_cache = metrics_cache.clone();
    tokio::spawn(async move {
        if let Err(err) = run_agent_loop(action_client, agent_cfg, agent_metrics_cache).await {
            error!(?err, "agent loop terminated");
        }
    });

    let app_state = AppState {
        config: cfg.clone(),
        metrics: metrics_cache,
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

async fn subscribe_metrics_loop(
    mut client: ExecutorClient<tonic::transport::Channel>,
    cache: MetricsCache,
) {
    let request = tonic::Request::new(MetricsWatchRequest {
        validator_ids: vec![],
        include_snapshot: true,
    });
    match client.subscribe_metrics(request).await {
        Ok(mut stream) => {
            let mut inner = stream.into_inner();
            while let Ok(Some(update)) = inner.message().await {
                match serde_json::from_str::<ValidatorMetrics>(&update.metrics_json) {
                    Ok(metrics) => {
                        cache.insert(update.validator_id.clone(), metrics).await;
                    }
                    Err(err) => {
                        error!(validator = update.validator_id, ?err, "invalid metrics payload");
                    }
                }
            }
        }
        Err(err) => {
            error!(?err, "metrics subscription failed");
        }
    }
}

async fn run_agent_loop(
    mut client: ExecutorClient<tonic::transport::Channel>,
    config: Arc<Config>,
    metrics: MetricsCache,
) -> Result<()> {
    let mut ticker = interval(Duration::from_secs(ACTION_POLL_INTERVAL_SECS));
    info!("agent loop started for {} validators", config.validators.len());
    loop {
        ticker.tick().await;
        let snapshot = metrics.snapshot().await;
        for validator in &config.validators {
            let Some(metrics) = snapshot.get(&validator.id.0) else {
                continue;
            };
            if let Some(issue) = detect_issue(metrics) {
                let playbook = choose_playbook(issue, &validator.id);
                info!(
                    validator = validator.id.0,
                    issue = ?issue,
                    playbook = %playbook.id,
                    "issue detected, dispatching actions via executor"
                );
                for action in playbook.steps {
                    let action_json = serde_json::to_string(&action)?;
                    let request = tonic::Request::new(ActionEnvelope {
                        validator_id: validator.id.0.clone(),
                        action_json,
                    });
                    if let Err(err) = client.submit_action(request).await {
                        error!(validator = validator.id.0, ?err, "failed to submit action");
                    }
                }
            }
        }
    }
}

async fn health() -> &'static str {
    "ok"
}

async fn pending_actions() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "pending": 0 }))
}

async fn actions_summary() -> Json<ActionsResponse> {
    Json(ActionsResponse { pending: 0 })
}

async fn list_validators(State(state): State<AppState>) -> Json<ValidatorsResponse> {
    let snapshot = state.metrics.snapshot().await;
    let mut validators = Vec::with_capacity(state.config.validators.len());

    for cfg in &state.config.validators {
        let metrics_opt = snapshot.get(&cfg.id.0).cloned();
        let (status, risk) = match metrics_opt.as_ref() {
            Some(metrics) => (
                detect_issue(metrics)
                    .map(|i| format!("{:?}", i))
                    .unwrap_or_else(|| "ok".into()),
                Some(risk_score(metrics)),
            ),
            None => ("no_data".into(), None),
        };
        validators.push(ValidatorSummary {
            id: cfg.id.0.clone(),
            host: cfg.host.clone(),
            prometheus_url: cfg.prometheus_url.clone(),
            metrics: metrics_opt,
            status,
            risk_score: risk,
        });
    }

    Json(ValidatorsResponse { validators })
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    metrics: MetricsCache,
}

#[derive(Clone, Default)]
struct MetricsCache {
    inner: Arc<Mutex<HashMap<String, ValidatorMetrics>>>,
}

impl MetricsCache {
    async fn insert(&self, id: String, metrics: ValidatorMetrics) {
        self.inner.lock().await.insert(id, metrics);
    }

    async fn snapshot(&self) -> HashMap<String, ValidatorMetrics> {
        self.inner.lock().await.clone()
    }
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
