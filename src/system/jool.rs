use std::process::Output;

use snafu::prelude::*;

const JOOL_INSTANCE: &str = "seedling";
const NAT64_PREFIX: &str = "64:ff9b::/96";

#[derive(Debug, Snafu)]
pub enum JoolError {
    #[snafu(display("jool kernel module is not available: {message}"))]
    ModuleUnavailable { message: String },

    #[snafu(display("failed to load jool kernel module: {message}"))]
    ModuleLoadFailed { message: String },

    #[snafu(display("jool command failed: {message}"))]
    CommandFailed { message: String },

    #[snafu(display("I/O error running jool command: {source}"))]
    Io { source: std::io::Error },
}

async fn ensure_module_loaded() -> Result<(), JoolError> {
    let output = run_command("lsmod", &[]).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.lines().any(|l| l.starts_with("jool ")) {
        return Ok(());
    }

    let output = run_command("modprobe", &["jool"]).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return ModuleLoadFailedSnafu {
            message: stderr.trim().to_string(),
        }
        .fail();
    }

    tracing::info!("loaded jool kernel module");
    Ok(())
}

async fn instance_exists() -> Result<bool, JoolError> {
    let output = run_command("jool", &["instance", "display", JOOL_INSTANCE]).await?;
    Ok(output.status.success())
}

async fn ensure_instance() -> Result<(), JoolError> {
    if instance_exists().await? {
        tracing::debug!("jool NAT64 instance already exists");
        return Ok(());
    }

    let output = run_command(
        "jool",
        &[
            "instance",
            "add",
            JOOL_INSTANCE,
            "--netfilter",
            "--pool6",
            NAT64_PREFIX,
        ],
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return CommandFailedSnafu {
            message: format!("failed to create jool instance: {}", stderr.trim()),
        }
        .fail();
    }

    tracing::info!("created jool NAT64 instance with prefix {NAT64_PREFIX}");
    Ok(())
}

async fn remove_instance() -> Result<(), JoolError> {
    if !instance_exists().await? {
        return Ok(());
    }

    let output = run_command("jool", &["instance", "remove", JOOL_INSTANCE]).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return CommandFailedSnafu {
            message: format!("failed to remove jool instance: {}", stderr.trim()),
        }
        .fail();
    }

    tracing::info!("removed jool NAT64 instance");
    Ok(())
}

pub async fn setup_nat64() -> Result<(), JoolError> {
    ensure_module_loaded().await?;
    ensure_instance().await
}

pub async fn teardown_nat64() -> Result<(), JoolError> {
    remove_instance().await
}

async fn run_command(program: &str, args: &[&str]) -> Result<Output, JoolError> {
    tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .context(IoSnafu)
}
