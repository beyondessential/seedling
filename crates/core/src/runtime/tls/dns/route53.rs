//! AWS Route 53 implementation of the DNS provider trait.
//!
//! The provider stores the AWS access key and secret in the encrypted
//! provider config. At call time we build an ad-hoc AWS SDK client with
//! those static credentials so they never live in process env vars.

use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_route53 as route53;
use route53::error::{ProvideErrorMetadata, SdkError};
use route53::operation::RequestId;
use route53::types::{
    Change, ChangeAction, ChangeBatch, ResourceRecord, ResourceRecordSet, RrType,
};
use serde::Deserialize;

use super::{ApiSnafu, DnsError, DnsFuture, DnsProvider, NoZoneSnafu};

/// Format an AWS SDK error with the detail Route 53 actually surfaces:
/// the SDK-level kind (timeout, dispatch, construction, response, or
/// service), and for service errors the AWS error code (e.g.
/// `AccessDenied`, `Throttling`, `InvalidChangeBatch`), the message,
/// the HTTP status, and the request id.
///
/// The default `Display` for `SdkError` produces "service error" for
/// the most common case; that's almost never enough to triage the
/// failure (IAM denial vs throttle vs unknown zone all collapse to the
/// same string), hence this helper.
fn format_sdk_error<E>(operation: &str, err: SdkError<E>) -> String
where
    E: ProvideErrorMetadata + std::fmt::Debug,
{
    let request_id = err.request_id().map(str::to_owned);
    match err {
        SdkError::ConstructionFailure(_) => {
            format!("{operation}: request construction failure (likely a client bug, not the API)")
        }
        SdkError::TimeoutError(_) => format!("{operation}: request timed out before a response"),
        SdkError::DispatchFailure(d) => {
            // Network / DNS / TLS failure — the request never made it
            // to the service.
            let kind = if d.is_io() {
                "io error"
            } else if d.is_timeout() {
                "dispatch timeout"
            } else if d.is_user() {
                "user error"
            } else {
                "dispatch failure"
            };
            format!("{operation}: {kind} reaching AWS endpoint: {d:?}")
        }
        SdkError::ResponseError(r) => format!(
            "{operation}: AWS returned an unparseable response (status={}): {:?}",
            r.raw().status().as_u16(),
            r
        ),
        SdkError::ServiceError(svc) => {
            let inner = svc.err();
            let status = svc.raw().status().as_u16();
            let code = inner.code().unwrap_or("(no code)");
            let message = inner.message().unwrap_or("(no message)");
            let req_id = request_id.as_deref().unwrap_or("(no request id)");
            format!("{operation}: {code}: {message} (http={status}, request_id={req_id})")
        }
        // SdkError is non-exhaustive — future variants get a generic
        // fallback that still includes Debug detail.
        other => format!("{operation}: {other:?}"),
    }
}

/// JSON shape stored in `tls_dns_providers.config_ciphertext` for
/// `kind = 'route53'`.
#[derive(Debug, Deserialize)]
pub struct Config {
    pub access_key_id: String,
    pub secret_access_key: String,
    /// Region only matters for the SDK signer; Route 53 itself is global.
    /// Defaults to `us-east-1` if omitted.
    #[serde(default = "default_region")]
    pub region: String,
}

fn default_region() -> String {
    "us-east-1".to_owned()
}

pub struct Route53Provider {
    config: Config,
}

impl Route53Provider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    async fn build_client(&self) -> route53::Client {
        let creds = Credentials::new(
            self.config.access_key_id.clone(),
            self.config.secret_access_key.clone(),
            None,
            None,
            "seedling-tls",
        );
        let cfg = aws_config::defaults(BehaviorVersion::latest())
            .credentials_provider(creds)
            .region(Region::new(self.config.region.clone()))
            .load()
            .await;
        route53::Client::new(&cfg)
    }

    /// Locate the hosted zone whose name is the longest dotted-suffix of
    /// `name`. The zone's name is returned with its trailing dot intact so
    /// callers can match it against record FQDNs.
    async fn find_zone(&self, client: &route53::Client, name: &str) -> Result<Zone, DnsError> {
        // Walk label-by-label from longest to shortest until a hosted zone
        // matches. Using `list_hosted_zones_by_name` once with `dns_name`
        // returns at most one page starting at that name; if the matching
        // zone has a name shorter than the start, it won't be in this
        // response. So we just enumerate all zones (paginated) up front.
        let stripped = name.strip_suffix('.').unwrap_or(name);
        let labels: Vec<&str> = stripped.split('.').collect();

        let mut zones: Vec<Zone> = Vec::new();
        let mut paginator = client.list_hosted_zones().into_paginator().send();
        while let Some(page) = paginator.next().await {
            let page = page.map_err(|e| {
                ApiSnafu {
                    message: format_sdk_error("list_hosted_zones", e),
                }
                .build()
            })?;
            for hz in page.hosted_zones {
                zones.push(Zone {
                    id: hz.id,
                    name: hz.name,
                });
            }
        }

        // Try suffixes longest-first.
        for start in 0..labels.len() {
            let candidate = format!("{}.", labels[start..].join("."));
            if let Some(z) = zones.iter().find(|z| z.name == candidate) {
                return Ok(z.clone());
            }
        }
        NoZoneSnafu {
            name: name.to_owned(),
        }
        .fail()
    }

    async fn change(&self, action: ChangeAction, name: &str, value: &str) -> Result<(), DnsError> {
        let client = self.build_client().await;
        let zone = self.find_zone(&client, name).await?;

        let fqdn = if name.ends_with('.') {
            name.to_owned()
        } else {
            format!("{name}.")
        };

        let record = ResourceRecord::builder()
            .value(format!("\"{value}\""))
            .build()
            .map_err(|e| {
                ApiSnafu {
                    message: format!("build resource record: {e}"),
                }
                .build()
            })?;

        let rrset = ResourceRecordSet::builder()
            .name(&fqdn)
            .r#type(RrType::Txt)
            .ttl(60)
            .resource_records(record)
            .build()
            .map_err(|e| {
                ApiSnafu {
                    message: format!("build rrset: {e}"),
                }
                .build()
            })?;

        let change = Change::builder()
            .action(action)
            .resource_record_set(rrset)
            .build()
            .map_err(|e| {
                ApiSnafu {
                    message: format!("build change: {e}"),
                }
                .build()
            })?;

        let batch = ChangeBatch::builder()
            .changes(change)
            .build()
            .map_err(|e| {
                ApiSnafu {
                    message: format!("build batch: {e}"),
                }
                .build()
            })?;

        client
            .change_resource_record_sets()
            .hosted_zone_id(&zone.id)
            .change_batch(batch)
            .send()
            .await
            .map_err(|e| {
                ApiSnafu {
                    message: format_sdk_error("change_resource_record_sets", e),
                }
                .build()
            })?;

        Ok(())
    }
}

#[derive(Clone)]
struct Zone {
    id: String,
    name: String,
}

impl DnsProvider for Route53Provider {
    fn set_txt<'a>(&'a self, name: &'a str, value: &'a str) -> DnsFuture<'a> {
        Box::pin(async move { self.change(ChangeAction::Upsert, name, value).await })
    }

    fn clear_txt<'a>(&'a self, name: &'a str, value: &'a str) -> DnsFuture<'a> {
        Box::pin(async move {
            match self.change(ChangeAction::Delete, name, value).await {
                Ok(()) => Ok(()),
                // Route 53 returns `InvalidChangeBatch` when the record set
                // doesn't exist or doesn't match. Idempotency requires we
                // treat absence as success.
                Err(DnsError::Api { message, .. }) if message.contains("InvalidChangeBatch") => {
                    Ok(())
                }
                Err(e) => Err(e),
            }
        })
    }
}
