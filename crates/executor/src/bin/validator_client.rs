use anyhow::{anyhow, Context, Result};
use common::{Action, ValidatorMetrics};
use executor::execute_action;
use executor::proto::executor_client::ExecutorClient;
use executor::proto::{ActionResult, ConnectRequest, MetricsUpdate};
use reqwest::Client as HttpClient;
use std::collections::HashMap;
use std::env;
use std::time::Duration;
use tokio::time::{interval, sleep};
use tonic::transport::{Channel, Endpoint};
use tonic::Status;
use tracing::{error, info, warn};

const DEFAULT_SERVER_ADDR: &str = "http://127.0.0.1:50051";
const DEFAULT_PROM_URL: &str = "http://127.0.0.1:9100/metrics";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let server_addr = env::var("EXECUTOR_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_SERVER_ADDR.into());
    let validator_id =
        env::var("VALIDATOR_ID").context("VALIDATOR_ID environment variable is required")?;
    let auth_token = env::var("VALIDATOR_AUTH_TOKEN")
        .context("VALIDATOR_AUTH_TOKEN environment variable is required")?;
    let prometheus_url =
        env::var("VALIDATOR_METRICS_URL").unwrap_or_else(|_| DEFAULT_PROM_URL.to_string());

    loop {
        if let Err(err) =
            run_client(&server_addr, &validator_id, &auth_token, &prometheus_url).await
        {
            error!(?err, "validator client loop failed, retrying in 3s");
            sleep(Duration::from_secs(3)).await;
        }
    }
}

async fn run_client(
    server_addr: &str,
    validator_id: &str,
    auth_token: &str,
    prometheus_url: &str,
) -> Result<()> {
    let channel = Endpoint::from_shared(server_addr.to_string())?
        .connect()
        .await
        .with_context(|| format!("failed to connect to executor server at {server_addr}"))?;
    let mut action_client = ExecutorClient::new(channel.clone());
    let mut report_client = ExecutorClient::new(channel.clone());
    let mut metrics_client = ExecutorClient::new(channel);

    let request = tonic::Request::new(ConnectRequest {
        validator_id: validator_id.to_string(),
        auth_token: auth_token.to_string(),
    });

    let mut stream = action_client.stream_actions(request).await?.into_inner();
    info!(validator = validator_id, "connected to control plane");

    let metrics_task = tokio::spawn(publish_metrics_loop(
        metrics_client,
        validator_id.to_string(),
        auth_token.to_string(),
        prometheus_url.to_string(),
    ));

    while let Some(msg) = stream.message().await? {
        let action: Action = serde_json::from_str(&msg.action_json)
            .map_err(|err| anyhow!("invalid action payload: {err}"))?;
        info!(validator = validator_id, "executing action from server");

        let execution_result = execute_action(action.clone()).await;
        let (success, message) = match execution_result {
            Ok(_) => (true, String::from("ok")),
            Err(err) => (false, err.to_string()),
        };

        report_client
            .report_result(tonic::Request::new(ActionResult {
                validator_id: validator_id.to_string(),
                action_json: msg.action_json.clone(),
                success,
                message,
            }))
            .await
            .map_err(map_status)?;
    }

    metrics_task.abort();
    Err(anyhow!("action stream closed by server"))
}

async fn publish_metrics_loop(
    mut client: ExecutorClient<Channel>,
    validator_id: String,
    auth_token: String,
    prometheus_url: String,
) {
    let http = HttpClient::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("failed to build http client");
    let mut ticker = interval(Duration::from_secs(5));
    loop {
        ticker.tick().await;
        match scrape_validator_metrics(&http, &validator_id, &prometheus_url).await {
            Ok(metrics) => {
                let payload = MetricsUpdate {
                    validator_id: validator_id.clone(),
                    auth_token: auth_token.clone(),
                    metrics_json: match serde_json::to_string(&metrics) {
                        Ok(json) => json,
                        Err(err) => {
                            error!(?err, "failed to serialize metrics");
                            continue;
                        }
                    },
                };
                if let Err(err) = client.publish_metrics(tonic::Request::new(payload)).await {
                    error!(?err, "failed to publish metrics update");
                }
            }
            Err(err) => {
                warn!(?err, "failed to scrape local metrics");
            }
        }
    }
}

async fn scrape_validator_metrics(
    http: &HttpClient,
    validator_id: &str,
    url: &str,
) -> Result<ValidatorMetrics> {
    let response = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed HTTP request to {url}"))?;
    let body = response
        .error_for_status()
        .with_context(|| format!("non-success HTTP status from {url}"))?
        .text()
        .await
        .context("failed reading response body")?;
    parse_prometheus_samples(&body, validator_id)
}

fn parse_prometheus_samples(body: &str, validator_id: &str) -> Result<ValidatorMetrics> {
    let samples = parse_samples_map(body, validator_id);
    let value_for = |name: &str| -> Result<f64> {
        samples
            .get(name)
            .copied()
            .ok_or_else(|| anyhow!("missing {name} metric for validator {validator_id}"))
    };

    Ok(ValidatorMetrics {
        slot_lag: value_for("validator_slot_lag")? as i64,
        vote_success_rate: value_for("validator_vote_success_rate")?,
        cpu_usage: value_for("validator_cpu_usage")?,
        ram_usage_gb: value_for("validator_ram_usage_gb")?,
        disk_usage_pct: value_for("validator_disk_usage_pct")?,
        rpc_qps: value_for("validator_rpc_qps")?,
        rpc_error_rate: value_for("validator_rpc_error_rate")?,
        last_updated: common::now_ts(),
    })
}

fn parse_samples_map(body: &str, validator_id: &str) -> HashMap<String, f64> {
    let mut samples = HashMap::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let metric_label = match parts.next() {
            Some(val) => val,
            None => continue,
        };
        let value = match parts.next() {
            Some(val) => val,
            None => continue,
        };
        let (metric_name, labels) = if let Some(pos) = metric_label.find('{') {
            let name = &metric_label[..pos];
            let rest = &metric_label[pos + 1..];
            match rest.find('}') {
                Some(end) => (name, Some(&rest[..end])),
                None => continue,
            }
        } else {
            (metric_label, None)
        };

        if let Some(labels) = labels {
            if !labels_match_validator(labels, validator_id) {
                continue;
            }
        }

        if samples.contains_key(metric_name) {
            continue;
        }

        if let Ok(parsed) = value.parse::<f64>() {
            samples.insert(metric_name.to_string(), parsed);
        }
    }
    samples
}

fn labels_match_validator(labels: &str, validator_id: &str) -> bool {
    if validator_id.is_empty() {
        return true;
    }
    for pair in labels.split(',') {
        let mut kv = pair.splitn(2, '=');
        let key = kv.next().unwrap_or("").trim();
        let raw_value = kv.next().unwrap_or("").trim();
        if key == "id" {
            let normalized = raw_value.trim_matches('"');
            return normalized == validator_id;
        }
    }
    true
}

fn map_status(err: Status) -> anyhow::Error {
    anyhow!("gRPC error: {err}")
}

