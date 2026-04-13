use super::{DataPlaneError, NftablesDataPlane};
use crate::system::types::ServiceRoute;

impl NftablesDataPlane {
    /// Delete all seedling-managed IPv6 static /128 routes in the fd5e::/16
    /// range using the `ip` CLI. Using the CLI instead of rtnetlink directly
    /// allows us to isolate whether EINVAL is a library/message-construction
    /// issue or genuine kernel behaviour.
    #[tracing::instrument(level = "trace", skip(self))]
    pub(super) async fn delete_managed_routes(&self) -> Result<(), DataPlaneError> {
        // `ip -j route show` emits JSON; each element has a "dst" key.
        // For IPv6 host (/128) routes, iproute2 may omit the prefix length.
        let out = tokio::process::Command::new("ip")
            .args([
                "-6", "-j", "route", "show", "proto", "static", "table", "main",
            ])
            .output()
            .await
            .map_err(|e| DataPlaneError::Netlink {
                source: Box::new(e),
            })?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(DataPlaneError::Netlink {
                source: format!("ip route show failed: {}", stderr.trim()).into(),
            });
        }

        let routes: Vec<serde_json::Value> =
            serde_json::from_slice(&out.stdout).unwrap_or_default();

        for route in routes {
            let dst = match route["dst"].as_str() {
                Some(d) => d,
                None => continue,
            };

            // Only touch /128 routes in our managed range.
            // Host routes may appear without the "/128" suffix.
            let addr_part = dst.split('/').next().unwrap_or(dst);
            let prefix_len = dst
                .split('/')
                .nth(1)
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(128); // no slash → host route → /128

            if prefix_len != 128 {
                continue;
            }
            if !addr_part.starts_with("fd5e") {
                continue;
            }

            let del = tokio::process::Command::new("ip")
                .args([
                    "-6", "route", "del", dst, "proto", "static", "table", "main",
                ])
                .output()
                .await
                .map_err(|e| DataPlaneError::Netlink {
                    source: Box::new(e),
                })?;

            if !del.status.success() {
                let stderr = String::from_utf8_lossy(&del.stderr);
                return Err(DataPlaneError::Netlink {
                    source: format!(
                        "ip route del {} failed (exit {:?}): {}",
                        dst,
                        del.status.code(),
                        stderr.trim()
                    )
                    .into(),
                });
            }
        }

        Ok(())
    }

    /// Add or replace a service route using the `ip` CLI.
    #[tracing::instrument(level = "trace", skip(self))]
    pub(super) async fn add_service_route(&self, svc: &ServiceRoute) -> Result<(), DataPlaneError> {
        let dst = format!("{}/128", svc.service_ip);

        // Build the argument list for: ip -6 route replace <args>
        let mut args: Vec<String> = vec!["route".into(), "replace".into()];

        match svc.backends.len() {
            0 => {
                args.push("blackhole".into());
                args.push(dst);
            }
            1 => {
                args.push(dst);
                args.extend(["via".into(), svc.backends[0].to_string()]);
            }
            _ => {
                args.push(dst);
                for b in &svc.backends {
                    args.extend(["nexthop".into(), "via".into(), b.to_string()]);
                }
            }
        }

        args.extend([
            "proto".into(),
            "static".into(),
            "table".into(),
            "main".into(),
        ]);

        let out = tokio::process::Command::new("ip")
            .arg("-6")
            .args(&args)
            .output()
            .await
            .map_err(|e| DataPlaneError::Netlink {
                source: Box::new(e),
            })?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(DataPlaneError::Netlink {
                source: format!(
                    "ip -6 {} failed (exit {:?}): {}",
                    args.join(" "),
                    out.status.code(),
                    stderr.trim()
                )
                .into(),
            });
        }

        Ok(())
    }
}
