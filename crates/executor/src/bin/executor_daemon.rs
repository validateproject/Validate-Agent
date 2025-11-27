use anyhow::Result;
use common::{Action, ValidatorConfig, ValidatorMetrics};
use executor::proto::executor_server::{Executor, ExecutorServer};
use executor::proto::{
    ActionEnvelope, ActionResult, ConnectRequest, MetricsUpdate, MetricsWatchRequest, ReportAck,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status};
use tracing::{error, info};

const DEFAULT_GRPC_ADDR: &str = "0.0.0.0:50051";

type ActionStream =
    Pin<Box<dyn Stream<Item = Result<ActionEnvelope, Status>> + Send + 'static>>;
type MetricsStream =
    Pin<Box<dyn Stream<Item = Result<MetricsUpdate, Status>> + Send + 'static>>;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = common::load_config()?;
    let listen_addr: SocketAddr = env::var("EXECUTOR_LISTEN_ADDR")
        .unwrap_or_else(|_| DEFAULT_GRPC_ADDR.to_string())
        .parse()
        .expect("invalid EXECUTOR_LISTEN_ADDR");

    let state = SharedState::new(cfg.validators.clone());
    let svc = ControlService { state };

    info!("executor control plane listening on {}", listen_addr);
    tonic::transport::Server::builder()
        .add_service(ExecutorServer::new(svc))
        .serve(listen_addr)
        .await?;
    Ok(())
}

#[derive(Clone)]
struct SharedState {
    inner: Arc<Mutex<StateInner>>,
    metrics_tx: broadcast::Sender<MetricsUpdate>,
}

struct StateInner {
    validators: HashMap<String, ValidatorConfig>,
    clients: HashMap<String, mpsc::Sender<ActionEnvelope>>,
    pending_actions: HashMap<String, VecDeque<ActionEnvelope>>,
    latest_metrics: HashMap<String, ValidatorMetrics>,
}

impl SharedState {
    fn new(validators: Vec<ValidatorConfig>) -> Self {
        let (metrics_tx, _) = broadcast::channel(256);
        let validators_map = validators
            .into_iter()
            .map(|cfg| (cfg.id.0.clone(), cfg))
            .collect();
        let inner = StateInner {
            validators: validators_map,
            clients: HashMap::new(),
            pending_actions: HashMap::new(),
            latest_metrics: HashMap::new(),
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
            metrics_tx,
        }
    }

    async fn authorize(&self, validator_id: &str, token: &str) -> Result<ValidatorConfig, Status> {
        let inner = self.inner.lock().await;
        let Some(cfg) = inner.validators.get(validator_id) else {
            return Err(Status::not_found("validator not registered"));
        };
        if cfg.auth_token != token {
            return Err(Status::unauthenticated("invalid auth token"));
        }
        Ok(cfg.clone())
    }

    async fn attach_client(
        &self,
        validator_id: String,
        sender: mpsc::Sender<ActionEnvelope>,
    ) -> Result<(), Status> {
        let mut inner = self.inner.lock().await;
        if !inner.validators.contains_key(&validator_id) {
            return Err(Status::not_found("validator not registered"));
        }
        inner.clients.insert(validator_id.clone(), sender);
        inner.flush(&validator_id);
        Ok(())
    }

    async fn enqueue_action(&self, action: ActionEnvelope) -> Result<(), Status> {
        let validator_id = action.validator_id.clone();
        let mut inner = self.inner.lock().await;
        if !inner.validators.contains_key(&validator_id) {
            return Err(Status::not_found("validator not registered"));
        }
        inner
            .pending_actions
            .entry(validator_id.clone())
            .or_default()
            .push_back(action);
        inner.flush(&validator_id);
        Ok(())
    }

    async fn record_metrics(&self, mut update: MetricsUpdate) -> Result<(), Status> {
        let metrics: ValidatorMetrics = serde_json::from_str(&update.metrics_json)
            .map_err(|err| Status::invalid_argument(format!("invalid metrics payload: {err}")))?;
        {
            let mut inner = self.inner.lock().await;
            let Some(cfg) = inner.validators.get(&update.validator_id) else {
                return Err(Status::not_found("validator not registered"));
            };
            if cfg.auth_token != update.auth_token {
                return Err(Status::unauthenticated("invalid auth token"));
            }
            inner
                .latest_metrics
                .insert(update.validator_id.clone(), metrics);
        }
        update.auth_token.clear();
        let _ = self.metrics_tx.send(update);
        Ok(())
    }

    async fn snapshot(&self, filter: &HashSet<String>) -> Vec<MetricsUpdate> {
        let inner = self.inner.lock().await;
        let include_all = filter.is_empty();
        inner
            .latest_metrics
            .iter()
            .filter_map(|(id, metrics)| {
                if include_all || filter.contains(id) {
                    Some(MetricsUpdate {
                        validator_id: id.clone(),
                        auth_token: String::new(),
                        metrics_json: serde_json::to_string(metrics).unwrap_or_default(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    fn metrics_sender(&self) -> broadcast::Sender<MetricsUpdate> {
        self.metrics_tx.clone()
    }
}

impl StateInner {
    fn flush(&mut self, validator_id: &str) {
        let Some(sender) = self.clients.get_mut(validator_id) else {
            return;
        };
        let Some(queue) = self.pending_actions.get_mut(validator_id) else {
            return;
        };
        while let Some(action) = queue.pop_front() {
            match sender.try_send(action.clone()) {
                Ok(_) => continue,
                Err(mpsc::error::TrySendError::Full(item)) => {
                    queue.push_front(item);
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(item)) => {
                    queue.push_front(item);
                    self.clients.remove(validator_id);
                    break;
                }
            }
        }
    }
}

struct ControlService {
    state: SharedState,
}

#[tonic::async_trait]
impl Executor for ControlService {
    type StreamActionsStream = ActionStream;
    type SubscribeMetricsStream = MetricsStream;

    async fn stream_actions(
        &self,
        request: Request<ConnectRequest>,
    ) -> Result<Response<Self::StreamActionsStream>, Status> {
        let ConnectRequest {
            validator_id,
            auth_token,
        } = request.into_inner();

        let cfg = self.state.authorize(&validator_id, &auth_token).await?;
        info!(validator = cfg.id.0, "validator client connected");

        let (tx, rx) = mpsc::channel(32);
        self.state
            .attach_client(cfg.id.0.clone(), tx)
            .await
            .map_err(|err| {
                error!(?err, "failed to attach client");
                err
            })?;

        let stream = ReceiverStream::new(rx).map(|msg| Ok(msg));
        Ok(Response::new(Box::pin(stream) as ActionStream))
    }

    async fn report_result(
        &self,
        request: Request<ActionResult>,
    ) -> Result<Response<ReportAck>, Status> {
        let ActionResult {
            validator_id,
            action_json,
            success,
            message,
        } = request.into_inner();

        let action: Action = serde_json::from_str(&action_json)
            .map_err(|err| Status::invalid_argument(format!("invalid action payload: {err}")))?;

        if success {
            info!(validator = validator_id, action = ?action, "action completed successfully");
        } else {
            error!(
                validator = validator_id,
                action = ?action,
                %message,
                "action failed"
            );
        }
        Ok(Response::new(ReportAck {}))
    }

    async fn publish_metrics(
        &self,
        request: Request<MetricsUpdate>,
    ) -> Result<Response<ReportAck>, Status> {
        let update = request.into_inner();
        self.state.record_metrics(update).await?;
        Ok(Response::new(ReportAck {}))
    }

    async fn subscribe_metrics(
        &self,
        request: Request<MetricsWatchRequest>,
    ) -> Result<Response<Self::SubscribeMetricsStream>, Status> {
        let req = request.into_inner();
        let filter: HashSet<String> = req.validator_ids.into_iter().collect();
        let include_snapshot = req.include_snapshot;
        let include_all = filter.is_empty();
        let filter = Arc::new(filter);

        let snapshot_stream = if include_snapshot {
            let snapshot = self.state.snapshot(&filter).await;
            tokio_stream::iter(snapshot.into_iter().map(Ok)).boxed()
        } else {
            tokio_stream::empty().boxed()
        };

        let metrics_tx = self.state.metrics_sender();
        let broadcast_stream = BroadcastStream::new(metrics_tx.subscribe())
            .filter_map(move |event| {
                let filter = filter.clone();
                async move {
                    match event {
                        Ok(mut update) => {
                            if include_all || filter.contains(&update.validator_id) {
                                update.auth_token.clear();
                                Some(Ok(update))
                            } else {
                                None
                            }
                        }
                        Err(_) => None,
                    }
                }
            })
            .boxed();

        let combined = snapshot_stream.chain(broadcast_stream);
        Ok(Response::new(Box::pin(combined) as MetricsStream))
    }

    async fn submit_action(
        &self,
        request: Request<ActionEnvelope>,
    ) -> Result<Response<ReportAck>, Status> {
        let envelope = request.into_inner();
        let action: Action = serde_json::from_str(&envelope.action_json)
            .map_err(|err| Status::invalid_argument(format!("invalid action payload: {err}")))?;
        if validator_id(&action) != envelope.validator_id {
            return Err(Status::invalid_argument(
                "validator id mismatch between envelope and action",
            ));
        }
        self.state.enqueue_action(envelope).await?;
        Ok(Response::new(ReportAck {}))
    }
}

fn validator_id(action: &Action) -> String {
    match action {
        Action::DisableRpc { validator }
        | Action::EnableRpc { validator }
        | Action::RestartValidator { validator }
        | Action::ThrottleRpcClient { validator }
        | Action::RunMaintenanceScript { validator, .. }
        | Action::SendAlert { validator, .. } => validator.0.clone(),
    }
}
