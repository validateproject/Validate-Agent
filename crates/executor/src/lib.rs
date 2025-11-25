use anyhow::{bail, Result};
use common::Action;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tracing::info;

pub async fn execute_action(action: Action) -> Result<()> {
    match action {
        Action::DisableRpc { validator } => {
            info!(validator = validator.0, "disabling RPC traffic");
            simulate_command("echo", &["disabling rpc"]).await?;
        }
        Action::EnableRpc { validator } => {
            info!(validator = validator.0, "enabling RPC traffic");
            simulate_command("echo", &["enabling rpc"]).await?;
        }
        Action::RestartValidator { validator } => {
            info!(validator = validator.0, "restarting validator process");
            simulate_command("echo", &["systemctl", "restart", "solana-validator"]).await?;
        }
        Action::ThrottleRpcClient { validator } => {
            info!(validator = validator.0, "throttling rpc client");
            simulate_command("echo", &["applying throttle"]).await?;
        }
        Action::RunMaintenanceScript {
            validator,
            script_name,
        } => {
            info!(validator = validator.0, script = %script_name, "running maintenance script");
            simulate_command("echo", &[script_name.as_str()]).await?;
        }
        Action::SendAlert { validator, message } => {
            info!(validator = validator.0, %message, "sending alert");
            simulate_command("echo", &["alert", &message]).await?;
        }
    }
    Ok(())
}

async fn simulate_command(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    // small delay keeps behavior close to real command execution
    tokio::time::sleep(Duration::from_millis(50)).await;
    if status.success() {
        Ok(())
    } else {
        bail!("command `{cmd}` failed with status {status}");
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
        execute_action(action).await.expect("action should succeed");
    }

    #[tokio::test]
    async fn executes_restart_validator() {
        let action = Action::RestartValidator {
            validator: common::ValidatorId("test".into()),
        };
        execute_action(action)
            .await
            .expect("restart should succeed");
    }
}
