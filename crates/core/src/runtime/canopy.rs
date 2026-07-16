//! Canopy reporting provider.
//!
//! Holds this instance's Canopy registration and, while one is present,
//! pushes a status report to Canopy on a fixed cadence. Enrolment claims a
//! server record the operator pre-created in Canopy: the operator-supplied
//! ticket is decrypted with its passphrase, a fresh device key is minted,
//! and possession of that key is proven to Canopy over a begin/complete
//! challenge. The stored registration is encrypted at rest and bound to the
//! machine by the registration store.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use bestool_canopy::{
    CanopyClient, ClientBuilderFactory, TAILSCALE_URL, device_identity,
    registration::{self, Registration},
    schema::{BeginArgs, BeginResponse, CompleteArgs, CompleteResponse},
    tailscale_client,
};
use jiff::Timestamp;
use p256::{
    SecretKey,
    ecdsa::{Signature, SigningKey, signature::Signer as _},
    pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _, EncodePublicKey as _},
};
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::{Value, json};
use snafu::Snafu;
use tokio::{sync::Notify, task::JoinHandle};
use tracing::{debug, error, info, warn};

use crate::{runtime::apps::AppRegistry, system::ContainerRuntime};

// r[canopy.push]
/// How often an enrolled instance reports to Canopy.
const PUSH_INTERVAL: Duration = Duration::from_secs(60);

// r[canopy.push.fault]
/// Consecutive report failures tolerated before escalating to error-level
/// logging.
const ESCALATE_AFTER_FAILURES: u32 = 5;

/// How long the enrolment handshake requests may take.
const ENROL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Snafu)]
pub enum CanopyError {
    // i[canopy.enrol.single]
    #[snafu(display("this instance already holds a Canopy registration"))]
    AlreadyEnrolled,
    #[snafu(display("invalid enrolment ticket: {reason}"))]
    InvalidTicket { reason: String },
    #[snafu(display("could not decrypt enrolment ticket (wrong passphrase?): {reason}"))]
    Decrypt { reason: String },
    #[snafu(display("canopy rejected the enrolment: {reason}"))]
    Rejected { reason: String },
    #[snafu(display("enrolment failed: {reason}"))]
    Internal { reason: String },
}

/// The decrypted enrolment ticket payload.
///
/// No `Debug` derive on purpose: `token` is a bearer secret and must never
/// be logged.
#[derive(Deserialize)]
struct EnrolTicket {
    v: String,
    api_url: String,
    server_id: String,
    token: String,
}

/// The non-secret half of a stored registration, kept in memory so the OI
/// status surface never has to touch the encrypted store.
#[derive(Debug, Clone)]
pub struct RegistrationInfo {
    pub server_id: String,
    pub device_id: Option<String>,
    pub api_url: String,
}

/// Reporting state the OI status surface reads.
#[derive(Debug, Clone, Default)]
pub struct PushStatus {
    pub last_push_at: Option<Timestamp>,
    pub last_push_error: Option<String>,
    pub last_response: Option<Value>,
}

pub struct CanopyProvider {
    /// Directory holding the encrypted registration (`{data_dir}/canopy`).
    dir: PathBuf,
    registry: Arc<RwLock<AppRegistry>>,
    container_runtime: Arc<dyn ContainerRuntime>,
    start_time: Instant,
    kick: Notify,
    registration: RwLock<Option<RegistrationInfo>>,
    client: RwLock<Option<Arc<CanopyClient>>>,
    push_status: RwLock<PushStatus>,
}

impl CanopyProvider {
    pub fn new(
        data_dir: &Path,
        registry: Arc<RwLock<AppRegistry>>,
        container_runtime: Arc<dyn ContainerRuntime>,
    ) -> Arc<Self> {
        Arc::new(Self {
            dir: data_dir.join("canopy"),
            registry,
            container_runtime,
            start_time: Instant::now(),
            kick: Notify::new(),
            registration: RwLock::new(None),
            client: RwLock::new(None),
            push_status: RwLock::new(PushStatus::default()),
        })
    }

    pub fn registration_info(&self) -> Option<RegistrationInfo> {
        self.registration.read().clone()
    }

    pub fn push_status(&self) -> PushStatus {
        self.push_status.read().clone()
    }

    /// Spawn the reporting loop. Loads any stored registration first, so an
    /// enrolled instance resumes reporting across restarts.
    pub fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        let provider = Arc::clone(&self);
        tokio::spawn(async move {
            provider.load_stored_registration().await;
            provider.run().await;
        })
    }

    // r[impl canopy.registration]
    async fn load_stored_registration(&self) {
        match registration::load_from(&self.dir).await {
            Ok(Some(reg)) => {
                if let Err(e) = self.adopt_registration(reg).await {
                    warn!("canopy: stored registration unusable: {e}");
                }
            }
            Ok(None) => debug!("canopy: no registration; reporting disabled"),
            Err(e) => warn!("canopy: could not read stored registration: {e:#}"),
        }
    }

    /// Record a registration's non-secret info and build the reporting
    /// client from its device key.
    async fn adopt_registration(&self, reg: Registration) -> Result<(), CanopyError> {
        let (Some(server_id), Some(api_url), Some(device_key)) = (
            reg.server_id.clone(),
            reg.api_url.clone(),
            reg.device_key.clone(),
        ) else {
            return Err(CanopyError::Internal {
                reason: "stored registration is missing fields".into(),
            });
        };

        let base_url = api_url.parse().map_err(|e| CanopyError::Internal {
            reason: format!("stored api_url invalid: {e}"),
        })?;
        let tailscale_url = TAILSCALE_URL.parse().expect("static URL is valid");
        let client =
            CanopyClient::with_urls(base_url, tailscale_url, Some(&device_key), client_builder)
                .await
                .map_err(|e| CanopyError::Internal {
                    reason: format!("building canopy client: {e:#}"),
                })?
                .ok_or_else(|| CanopyError::Internal {
                    reason: "no route to canopy (no tailnet and no device key)".into(),
                })?;

        *self.registration.write() = Some(RegistrationInfo {
            server_id,
            device_id: reg.device_id.clone(),
            api_url,
        });
        *self.client.write() = Some(Arc::new(client));
        Ok(())
    }

    // r[impl canopy.push]
    async fn run(self: Arc<Self>) {
        let mut consecutive_failures: u32 = 0;
        loop {
            if let Some((client, server_id)) = self.reporting_handles() {
                match self.push_once(&client, &server_id).await {
                    Ok(response) => {
                        consecutive_failures = 0;
                        // r[impl canopy.push.response]
                        info!(response = %response, "canopy: report accepted");
                        *self.push_status.write() = PushStatus {
                            last_push_at: Some(Timestamp::now()),
                            last_push_error: None,
                            last_response: Some(response),
                        };
                    }
                    Err(reason) => {
                        consecutive_failures += 1;
                        // r[impl canopy.push.fault]
                        if consecutive_failures >= ESCALATE_AFTER_FAILURES {
                            error!(
                                attempt = consecutive_failures,
                                "canopy: report failed: {reason}"
                            );
                        } else {
                            warn!(
                                attempt = consecutive_failures,
                                "canopy: report failed: {reason}"
                            );
                        }
                        let mut status = self.push_status.write();
                        status.last_push_at = Some(Timestamp::now());
                        status.last_push_error = Some(reason);
                    }
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(PUSH_INTERVAL) => {}
                _ = self.kick.notified() => {
                    debug!("canopy: immediate report triggered");
                }
            }
        }
    }

    fn reporting_handles(&self) -> Option<(Arc<CanopyClient>, String)> {
        let client = self.client.read().clone()?;
        let server_id = self.registration.read().as_ref()?.server_id.clone();
        Some((client, server_id))
    }

    async fn push_once(&self, client: &CanopyClient, server_id: &str) -> Result<Value, String> {
        let payload = self.build_payload().await;
        let response = client
            .status(server_id, &payload)
            .await
            .map_err(|e| format!("{e:#}"))?;
        serde_json::to_value(&response).map_err(|e| format!("encoding response: {e}"))
    }

    // r[impl canopy.push]
    async fn build_payload(&self) -> Value {
        let proxy = self
            .component_state(&["seedling-caddy-blue", "seedling-caddy-green"])
            .await;
        let resolver = self
            .component_state(&["seedling-resolver-blue", "seedling-resolver-green"])
            .await;

        let (apps_total, apps_running) = {
            let reg = self.registry.read();
            let apps = reg.list();
            let running = apps
                .iter()
                .filter(|(_, status)| status.name() == "running")
                .count();
            (apps.len(), running)
        };
        let apps_check = if apps_total == 0 {
            json!({ "check": "apps", "result": "passed", "summary": "no apps registered" })
        } else if apps_running == apps_total {
            json!({
                "check": "apps",
                "result": "passed",
                "summary": format!("{apps_total} apps running"),
            })
        } else {
            json!({
                "check": "apps",
                "result": "warning",
                "summary": format!("{apps_running}/{apps_total} apps running"),
            })
        };

        let hostname = whoami::devicename()
            .or_else(|_| whoami::hostname())
            .unwrap_or_else(|_| "unknown".into());

        json!({
            "source": "seedling",
            "seedlingVersion": env!("CARGO_PKG_VERSION"),
            "hostname": hostname,
            "uptimeSecs": self.start_time.elapsed().as_secs(),
            "health": [
                component_check("proxy", "reverse proxy", proxy),
                component_check("resolver", "DNS resolver", resolver),
                apps_check,
            ],
        })
    }

    async fn component_state(&self, containers: &[&str]) -> &'static str {
        use crate::system::ContainerStatus;
        for name in containers {
            if let Ok(Some(s)) = self.container_runtime.inspect(name).await
                && matches!(s.status, ContainerStatus::Running)
            {
                return "running";
            }
        }
        "stopped"
    }

    // i[impl canopy.enrol]
    pub async fn enrol(
        &self,
        ticket_b64: &str,
        passphrase: &str,
    ) -> Result<(String, String), CanopyError> {
        // i[impl canopy.enrol.single]
        if self.registration.read().is_some() {
            return Err(CanopyError::AlreadyEnrolled);
        }

        let encrypted = STANDARD
            .decode(ticket_b64.split_whitespace().collect::<String>())
            .map_err(|e| CanopyError::InvalidTicket {
                reason: format!("not base64: {e}"),
            })?;
        // Reject a bogus ticket before touching the passphrase, so a failed
        // decode can't be mistaken for a wrong passphrase.
        if !encrypted.starts_with(b"age-encryption.org/v1") {
            return Err(CanopyError::InvalidTicket {
                reason: "not a Canopy enrolment ticket".into(),
            });
        }

        let ticket = {
            let passphrase = passphrase.to_owned();
            tokio::task::spawn_blocking(move || decrypt_ticket(&encrypted, &passphrase))
                .await
                .map_err(|e| CanopyError::Internal {
                    reason: format!("decrypt task failed: {e}"),
                })??
        };
        if ticket.v != "enroll-1" {
            return Err(CanopyError::InvalidTicket {
                reason: format!("unsupported ticket version {:?}", ticket.v),
            });
        }
        let api_url: bestool_canopy::reqwest::Url =
            ticket
                .api_url
                .parse()
                .map_err(|e| CanopyError::InvalidTicket {
                    reason: format!("api_url is not a valid URL: {e}"),
                })?;
        if api_url.scheme() != "https" {
            return Err(CanopyError::InvalidTicket {
                reason: format!("api_url must be https, got {:?}", api_url.scheme()),
            });
        }
        let server_id =
            uuid::Uuid::parse_str(&ticket.server_id).map_err(|e| CanopyError::InvalidTicket {
                reason: format!("server_id is not a valid UUID: {e}"),
            })?;

        // Mint a fresh device key: the signature and SPKI derive from it,
        // and it becomes this instance's identity to Canopy.
        let secret = SecretKey::random(&mut rand_core::OsRng);
        let device_key_pem = secret
            .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
            .map_err(|e| CanopyError::Internal {
                reason: format!("encoding device key: {e}"),
            })?
            .to_string();
        let spki_der = secret
            .public_key()
            .to_public_key_der()
            .map_err(|e| CanopyError::Internal {
                reason: format!("encoding device public key: {e}"),
            })?
            .as_bytes()
            .to_vec();
        let signing_key =
            SigningKey::from_pkcs8_pem(&device_key_pem).map_err(|e| CanopyError::Internal {
                reason: format!("loading device key for signing: {e}"),
            })?;

        let transport = enrolment_transport(&device_key_pem).await?;
        let spki_b64 = STANDARD.encode(&spki_der);

        // Step 1: begin — fetch the challenge nonce.
        let begin: BeginResponse = post_step(
            &transport,
            &api_url,
            "begin",
            &BeginArgs::builder()
                .server_id(server_id)
                .token(ticket.token.clone())
                .maybe_spki(transport.carries_spki_in_body().then(|| spki_b64.clone()))
                .build(),
        )
        .await?;
        if begin.channel_binding_required {
            return Err(CanopyError::Rejected {
                reason: "canopy requires TLS channel binding, which seedling does not support yet"
                    .into(),
            });
        }
        let nonce_bytes =
            STANDARD
                .decode(begin.nonce.trim())
                .map_err(|e| CanopyError::Rejected {
                    reason: format!("decoding challenge nonce: {e}"),
                })?;

        // Step 2: prove possession of the device key by signing the
        // transcript: nonce || server id bytes || SPKI DER.
        let mut transcript = Vec::with_capacity(nonce_bytes.len() + 16 + spki_der.len());
        transcript.extend_from_slice(&nonce_bytes);
        transcript.extend_from_slice(server_id.as_bytes());
        transcript.extend_from_slice(&spki_der);
        let signature: Signature = signing_key.sign(&transcript);
        let signature_b64 = STANDARD.encode(signature.to_der().as_bytes());

        let complete: CompleteResponse = post_step(
            &transport,
            &api_url,
            "complete",
            &CompleteArgs::builder()
                .server_id(server_id)
                .nonce(begin.nonce.clone())
                .signature(signature_b64)
                .maybe_spki(transport.carries_spki_in_body().then(|| spki_b64.clone()))
                .build(),
        )
        .await?;

        // r[impl canopy.registration]
        let reg = Registration {
            server_id: Some(complete.server_id.to_string()),
            device_key: Some(device_key_pem),
            device_id: Some(complete.device_id.to_string()),
            api_url: Some(api_url.to_string()),
            ..Registration::default()
        };
        registration::store_in(&self.dir, &reg)
            .await
            .map_err(|e| CanopyError::Internal {
                reason: format!("storing registration: {e:#}"),
            })?;

        self.adopt_registration(reg).await?;
        info!(server_id = %complete.server_id, device_id = %complete.device_id, "canopy: enrolled");
        // Report immediately so the operator sees the channel work end to end.
        self.kick.notify_one();
        Ok((
            complete.server_id.to_string(),
            complete.device_id.to_string(),
        ))
    }

    // i[impl canopy.deregister]
    pub async fn deregister(&self) -> Result<bool, CanopyError> {
        let removed =
            registration::delete_in(&self.dir)
                .await
                .map_err(|e| CanopyError::Internal {
                    reason: format!("removing registration: {e:#}"),
                })?;
        *self.client.write() = None;
        *self.registration.write() = None;
        *self.push_status.write() = PushStatus::default();
        if removed {
            info!("canopy: deregistered");
        }
        Ok(removed)
    }
}

fn client_builder() -> bestool_canopy::reqwest::ClientBuilder {
    bestool_canopy::reqwest::ClientBuilder::new()
}

fn component_check(name: &str, label: &str, state: &'static str) -> Value {
    if state == "running" {
        json!({ "check": name, "result": "passed", "summary": format!("{label} running") })
    } else {
        json!({ "check": name, "result": "failed", "summary": format!("{label} {state}") })
    }
}

/// Synchronous on purpose: the passphrase identity is not `Send`, so the
/// decrypt future must be driven to completion on one thread (see the
/// `spawn_blocking` at the call site — scrypt key derivation is CPU-bound).
fn decrypt_ticket(encrypted: &[u8], passphrase: &str) -> Result<EnrolTicket, CanopyError> {
    let pass =
        algae_cli::passphrases::Passphrase::new(secrecy::SecretString::from(passphrase.to_owned()));
    let reader = futures_util::io::Cursor::new(encrypted.to_vec());
    let mut plaintext: Vec<u8> = Vec::new();
    futures::executor::block_on(algae_cli::streams::decrypt_stream(
        reader,
        &mut plaintext,
        Box::new(pass),
    ))
    .map_err(|e| CanopyError::Decrypt {
        reason: format!("{e:#}"),
    })?;
    serde_json::from_slice(&plaintext).map_err(|e| CanopyError::InvalidTicket {
        reason: format!("decrypted ticket is not valid JSON: {e}"),
    })
}

/// Which network path the enrolment handshake takes: the canopy tailnet when
/// reachable (SPKI carried in the body), otherwise public mTLS against the
/// ticket's api_url (SPKI read from the client certificate).
enum EnrolTransport {
    Tailscale(bestool_canopy::reqwest::Client),
    Mtls(bestool_canopy::reqwest::Client),
}

impl EnrolTransport {
    fn client(&self) -> &bestool_canopy::reqwest::Client {
        match self {
            Self::Tailscale(c) | Self::Mtls(c) => c,
        }
    }

    fn carries_spki_in_body(&self) -> bool {
        matches!(self, Self::Tailscale(_))
    }

    fn url(
        &self,
        api_url: &bestool_canopy::reqwest::Url,
        step: &str,
    ) -> Result<bestool_canopy::reqwest::Url, CanopyError> {
        let raw = match self {
            Self::Tailscale(_) => format!("{TAILSCALE_URL}/public/servers/register/{step}"),
            Self::Mtls(_) => {
                return api_url
                    .join(&format!("/servers/register/{step}"))
                    .map_err(|e| CanopyError::Internal {
                        reason: format!("building register/{step} URL: {e}"),
                    });
            }
        };
        raw.parse().map_err(|e| CanopyError::Internal {
            reason: format!("building register/{step} URL: {e}"),
        })
    }
}

async fn enrolment_transport(device_key_pem: &str) -> Result<EnrolTransport, CanopyError> {
    let factory: ClientBuilderFactory = Arc::new(client_builder);
    if let Some(client) = tailscale_client(&factory).await {
        debug!("canopy: enrolling over the canopy tailnet");
        return Ok(EnrolTransport::Tailscale(client));
    }
    debug!("canopy: tailnet unreachable; enrolling over public mTLS");
    let identity = device_identity(device_key_pem).map_err(|e| CanopyError::Internal {
        reason: format!("building device identity: {e:#}"),
    })?;
    let client = client_builder()
        .identity(identity)
        .use_rustls_tls()
        .timeout(ENROL_TIMEOUT)
        .build()
        .map_err(|e| CanopyError::Internal {
            reason: format!("building mTLS client: {e}"),
        })?;
    Ok(EnrolTransport::Mtls(client))
}

/// RFC-7807-style problem body. Canopy's register errors are intentionally
/// opaque; surface whatever title/detail it gives.
#[derive(Deserialize)]
struct Problem {
    title: Option<String>,
    detail: Option<String>,
}

async fn post_step<T: serde::de::DeserializeOwned>(
    transport: &EnrolTransport,
    api_url: &bestool_canopy::reqwest::Url,
    step: &str,
    body: &impl serde::Serialize,
) -> Result<T, CanopyError> {
    let url = transport.url(api_url, step)?;
    let resp = transport
        .client()
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| CanopyError::Rejected {
            reason: format!("calling register/{step}: {e}"),
        })?;

    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| CanopyError::Rejected {
        reason: format!("reading register/{step} response: {e}"),
    })?;

    if status.is_success() {
        return serde_json::from_slice(&bytes).map_err(|e| CanopyError::Rejected {
            reason: format!("parsing register/{step} response: {e}"),
        });
    }

    let reason = match serde_json::from_slice::<Problem>(&bytes) {
        Ok(problem) => {
            let title = problem.title.unwrap_or_else(|| "enrolment failed".into());
            match problem.detail {
                Some(detail) => format!("register/{step} ({status}): {title}: {detail}"),
                None => format!("register/{step} ({status}): {title}"),
            }
        }
        Err(_) => format!(
            "register/{step} ({status}): {}",
            String::from_utf8_lossy(&bytes)
        ),
    };
    Err(CanopyError::Rejected { reason })
}

#[cfg(test)]
mod tests;
