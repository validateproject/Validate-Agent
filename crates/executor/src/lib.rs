use anyhow::{bail, Result};
use common::Action;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tracing::info;

pub mod proto {
    tonic::include_proto!("executor.v1");
}

/// Executes an action locally on the validator host.
pub async fn execute_action(action: Action) -> Result<()> {
    match action {
        Action::DisableRpc { validator } => {
            info!(validator = validator.0, "disabling RPC traffic");
            run_command("echo disabling rpc").await?;
        }
        Action::EnableRpc { validator } => {
            info!(validator = validator.0, "enabling RPC traffic");
            run_command("echo enabling rpc").await?;
        }
        Action::RestartValidator { validator } => {
            info!(validator = validator.0, "restarting validator process");
            run_command("echo restarting validator").await?;
        }
        Action::ThrottleRpcClient { validator } => {
            info!(validator = validator.0, "throttling rpc client");
            run_command("echo throttling rpc client").await?;
        }
        Action::RunMaintenanceScript {
            validator,
            script_name,
        } => {
            info!(validator = validator.0, script = %script_name, "running maintenance script");
            run_command(&format!("sh {}", script_name)).await?;
        }
        Action::SendAlert { validator, message } => {
            info!(validator = validator.0, %message, "sending alert");
            run_command(&format!("echo alert: {message}")).await?;
        }
    }
    Ok(())
}

async fn run_command(command: &str) -> Result<()> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    if status.success() {
        Ok(())
    } else {
        bail!("command `{command}` failed with status {status}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn executes_disable_rpc() {
        let action = Action::DisableRpc {
            validator: common::ValidatorId("test".into()),
        };
        execute_action(action)
            .await
            .expect("disable rpc should succeed with stub command");
    }
}
