use std::process::Output;

use snafu::prelude::*;

use crate::runtime::nat64_prefix::NAT64_PREFIX_STR;

const JOOL_INSTANCE: &str = "seedling";

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

async fn module_loaded() -> Result<bool, JoolError> {
    let output = run_command("lsmod", &[]).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return CommandFailedSnafu {
            message: format!("failed to list kernel modules: {}", stderr.trim()),
        }
        .fail();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().any(|l| l.starts_with("jool ")))
}

async fn ensure_module_loaded() -> Result<(), JoolError> {
    if module_loaded().await? {
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

/// Names of the instances in a `jool instance display --csv` listing.
///
/// The name column is found by its header rather than by position, and a
/// listing with no header or a row too short to carry it is an error: guessing
/// at a layout this does not recognise would turn "could not read the list" into
/// "the instance is not in it".
fn parse_instance_names(listing: &str) -> Result<Vec<String>, JoolError> {
    let mut lines = listing.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(header) = lines.next() else {
        return Ok(Vec::new());
    };
    let Some(name_column) = header
        .split(',')
        .position(|c| c.trim().eq_ignore_ascii_case("name"))
    else {
        return CommandFailedSnafu {
            message: format!("unrecognised jool instance listing header: {header:?}"),
        }
        .fail();
    };

    lines
        .map(|row| {
            row.split(',')
                .nth(name_column)
                .map(|name| name.trim().to_string())
                .ok_or_else(|| JoolError::CommandFailed {
                    message: format!("unrecognised jool instance listing row: {row:?}"),
                })
        })
        .collect()
}

/// Whether our instance is installed.
///
/// `instance display` lists every instance and takes no name, so the list is
/// read and searched. A module that is not loaded has no instances, and the
/// client reports that as an error of its own, so it is established first.
/// Any other failure to list is an error, never an answer.
async fn instance_exists() -> Result<bool, JoolError> {
    if !module_loaded().await? {
        return Ok(false);
    }

    let output = run_command("jool", &["instance", "display", "--csv"]).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return CommandFailedSnafu {
            message: format!("failed to list jool instances: {}", stderr.trim()),
        }
        .fail();
    }

    let listing = String::from_utf8_lossy(&output.stdout);
    Ok(parse_instance_names(&listing)?
        .iter()
        .any(|name| name == JOOL_INSTANCE))
}

async fn ensure_instance() -> Result<(), JoolError> {
    let output = run_command(
        "jool",
        &[
            "instance",
            "add",
            JOOL_INSTANCE,
            "--netfilter",
            "--pool6",
            NAT64_PREFIX_STR,
        ],
    )
    .await?;

    if output.status.success() {
        tracing::info!("created jool NAT64 instance with prefix {NAT64_PREFIX_STR}");
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);

    // EEXIST (errno 17) — the instance already exists, which is fine.
    if stderr.contains("error 17") || stderr.contains("already has a Jool instance") {
        tracing::debug!("jool NAT64 instance already exists");
        return Ok(());
    }

    CommandFailedSnafu {
        message: format!("failed to create jool instance: {}", stderr.trim()),
    }
    .fail()
}

// r[impl infra.nat64.translator.lifecycle]
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
    // r[impl infra.nat64.forwarding]
    ensure_forwarding_enabled().await?;
    ensure_module_loaded().await?;
    ensure_instance().await
}

pub async fn teardown_nat64() -> Result<(), JoolError> {
    remove_instance().await
}

async fn ensure_forwarding_enabled() -> Result<(), JoolError> {
    let checks = [
        ("net.ipv6.conf.all.forwarding", "1"),
        ("net.ipv4.conf.all.forwarding", "1"),
    ];
    for (key, val) in checks {
        let output = run_command("sysctl", &["-w", &format!("{key}={val}")]).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return CommandFailedSnafu {
                message: format!("failed to set {key}: {}", stderr.trim()),
            }
            .fail();
        }
    }
    tracing::info!("IPv4 and IPv6 forwarding enabled");
    Ok(())
}

async fn run_command(program: &str, args: &[&str]) -> Result<Output, JoolError> {
    tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .context(IoSnafu)
}

#[cfg(test)]
mod tests {
    use super::*;

    // r[verify infra.nat64.translator.lifecycle]
    #[test]
    fn our_instance_is_found_among_others() {
        let listing =
            "Namespace,Name,Framework\n0xabc123,other,Netfilter\n0xabc124,seedling,Netfilter\n";
        let names = parse_instance_names(listing).unwrap();
        assert!(names.iter().any(|n| n == JOOL_INSTANCE));
        assert_eq!(names, ["other", "seedling"]);
    }

    // r[verify infra.nat64.translator.lifecycle]
    #[test]
    fn a_similar_name_is_not_our_instance() {
        let listing = "Namespace,Name,Framework\n0xabc,seedling-old,Netfilter\n";
        let names = parse_instance_names(listing).unwrap();
        assert!(!names.iter().any(|n| n == JOOL_INSTANCE));
    }

    // r[verify infra.nat64.translator.lifecycle]
    #[test]
    fn a_listing_with_no_instances_is_empty() {
        assert!(
            parse_instance_names("Namespace,Name,Framework\n")
                .unwrap()
                .is_empty()
        );
        assert!(parse_instance_names("").unwrap().is_empty());
    }

    // r[verify infra.nat64.translator.lifecycle]
    // A listing this does not recognise must not read as "not installed",
    // which would let teardown report success over a live translator.
    #[test]
    fn an_unrecognised_listing_is_an_error_not_an_absence() {
        assert!(parse_instance_names("+-----+-----+\n| a | b |\n").is_err());
        assert!(parse_instance_names("Namespace,Name,Framework\n0xabc\n").is_err());
    }
}
