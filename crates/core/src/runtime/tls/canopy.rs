//! Canopy as a certificate source.
//!
//! The runtime generates and holds the key, sends Canopy only a signing
//! request, and collects the chain by asking again. Canopy proves control of
//! the name, issues, and renews; the runtime stores what comes back like any
//! other runtime-managed certificate.
//!
//! Three layers, kept apart so each can be tested without the others:
//!
//! - [`classify_response`] and [`classify_failure`] turn what Canopy said into
//!   an [`Answer`]. The one place where "refused" is told from "could not be
//!   asked", which the chain treats very differently.
//! - [`apply`] turns an [`Answer`] into stored state, in one transaction.
//! - [`Engine`] ties them to the relay: find or make the key, ask, apply.
//!
//! Whether to ask at all is not decided here: [`super::state`] decides, and the
//! caller hands this module an [`Ask`] describing the position it decided on.

use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};
use snafu::{ResultExt, Snafu};

use super::{
    AttemptOutcome, AttemptTrigger, KeyType, TlsCertState, TlsCertificate, TlsChainRow,
    chain::{self, Identity},
    keypair,
    parse::{self, ParsedChain},
    state::AUTO_RETRY_DEBOUNCE_SECS,
    store::{self, NewCertificate},
};
use crate::runtime::{
    db::{Db, DbHandle},
    secrets::Cipher,
};
use bestool_canopy::schema::CertificateResponse;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("storage error: {source}"))]
    Storage { source: rusqlite::Error },

    #[snafu(display("encryption error: {source}"))]
    Cipher {
        source: crate::runtime::secrets::Error,
    },

    #[snafu(display("key error: {source}"))]
    Key { source: keypair::Error },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Why a request to Canopy did not produce an answer about the certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayFailure {
    /// Canopy answered with an error status.
    Status { status: u16, body: String },
    /// The request did not reach Canopy, or its answer did not come back.
    Unreachable(String),
}

/// Carries a certificate request to Canopy.
pub trait CertificateRelay: Send + Sync {
    /// `csr_der_b64` is the signing request, DER, base64. `app_type` is the
    /// Canopy application type to name, when exactly one applies.
    fn request(
        &self,
        name: &str,
        csr_der_b64: &str,
        app_type: Option<&str>,
    ) -> impl Future<Output = std::result::Result<CertificateResponse, RelayFailure>> + Send;
}

/// The relay a connected client offers, as a [`CertificateRelay`].
pub struct CanopyRelay {
    canopy: Arc<crate::oi::canopy::CanopyState>,
}

impl CanopyRelay {
    pub fn new(canopy: Arc<crate::oi::canopy::CanopyState>) -> Self {
        Self { canopy }
    }
}

impl CertificateRelay for CanopyRelay {
    async fn request(
        &self,
        name: &str,
        csr_der_b64: &str,
        app_type: Option<&str>,
    ) -> std::result::Result<CertificateResponse, RelayFailure> {
        use bestool_canopy::{CanopyClient, CanopyHttpError, schema::RequestCertificateArgs};

        let client = CanopyClient::with_transport(crate::oi::canopy::OiCanopyTransport::new(
            Arc::clone(&self.canopy),
        ));
        let args = RequestCertificateArgs::builder()
            .name(name.to_owned())
            .csr(csr_der_b64.to_owned())
            .maybe_application_type(app_type.map(|t| t.to_owned().into()))
            .build();
        match client.certificates_request(&args).await {
            Ok(response) => Ok(response),
            Err(report) => Err(match report.downcast_ref::<CanopyHttpError>() {
                Some(e) => RelayFailure::Status {
                    status: e.status.as_u16(),
                    body: e.body.clone(),
                },
                None => RelayFailure::Unreachable(format!("{report}")),
            }),
        }
    }
}

/// What Canopy's answer means for the hostname.
// r[impl tls.canopy.outcomes]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// A chain that can be served now.
    Issued { chain_pem: String },
    /// The order is open.
    Pending,
    /// Canopy's authority has said no: the server lacks the TLS grant, the name
    /// is outside its group's domains, or no managed zone covers it. This is
    /// the only answer that moves a hostname down its chain ahead of the clock.
    Refused { reason: String },
    /// The order failed. Canopy may still be retrying; the runtime keeps
    /// asking.
    OrderFailed { reason: String },
    /// The question could not be put or answered: Canopy paused, the device
    /// not attached, the relay down, an answer not understood. Not a refusal.
    Unavailable { reason: String },
    /// The certificate was revoked or its key condemned. It is not to be
    /// served, and the key is to be replaced.
    Revoked { reason: String },
    /// Canopy rejected the signing request itself. The fault is the runtime's
    /// own.
    Malformed { reason: String },
}

/// The most of a reason from Canopy that is kept.
const REASON_LIMIT: usize = 500;

/// A reason as it is safe to put in a fault, a log line and the operator's view.
///
/// What Canopy says of an order travels through whichever client is carrying the
/// relay and ends up interpolated into a log line and a fault description, so a
/// line feed or a terminal escape in it would forge log lines and garble the
/// operator's screen. Control characters become spaces rather than being dropped,
/// so nothing is silently joined into a different word, and the whole is bounded.
fn tidy(reason: impl AsRef<str>) -> String {
    let mut out: String = reason
        .as_ref()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(REASON_LIMIT)
        .collect();
    let trimmed = out.trim();
    if trimmed.len() != out.len() {
        out = trimmed.to_owned();
    }
    out
}

/// Classify Canopy's answer to a request that reached it.
// r[impl tls.canopy.outcomes]
// r[impl tls.canopy.revoked]
pub fn classify_response(response: &CertificateResponse) -> Answer {
    if response.key_must_be_replaced {
        return Answer::Revoked {
            reason: "Canopy reports the key must be replaced".to_owned(),
        };
    }
    if response.revoked {
        return Answer::Revoked {
            reason: "Canopy reports the certificate revoked".to_owned(),
        };
    }
    if response.usable
        && let Some(chain) = &response.chain
    {
        return Answer::Issued {
            chain_pem: chain.clone(),
        };
    }
    match response.state.as_str() {
        "pending" => Answer::Pending,
        "failed" => Answer::OrderFailed {
            reason: response
                .last_error
                .as_deref()
                .map_or_else(|| "the order failed".to_owned(), tidy),
        },
        "revoked" => Answer::Revoked {
            reason: "Canopy reports the certificate revoked".to_owned(),
        },
        "issued" => Answer::Unavailable {
            reason: "Canopy reports a certificate issued that cannot be served".to_owned(),
        },
        // A state this runtime has not been taught is not "pending": reading it
        // so would wait on an order that may never exist.
        other => Answer::Unavailable {
            reason: format!(
                "Canopy answered with a state this runtime does not recognise: {}",
                tidy(other)
            ),
        },
    }
}

/// Where Canopy names the cause of a problem.
///
/// The `type` of a problem document from Canopy is a URI reference under this,
/// so the whole reference identifies the cause. Matching the last path segment
/// instead would read `https://gateway.example/errors/conflict` from something
/// on the relay path as Canopy's own word, and a refusal is the one answer that
/// moves a hostname off its strategy for the rest of the cycle.
const CANOPY_CAUSE: &str = "/errors/";

/// Classify a request that did not produce an answer about the certificate.
///
/// Canopy's errors carry a status and a problem-details body naming the cause.
/// A refusal is the authority's own no, and stands until someone acts: the
/// server lacks the TLS grant, the name is outside the domains its group
/// controls, an operator has denied the name to this server, the server
/// contradicts the application type named, or no zone Canopy manages covers the
/// name at all.
///
/// Everything else is retried. A paused server and a name Canopy has no
/// declaration of are both conditions that pass without anyone acting on this
/// side, a condemned key calls for a new one, and a cause this runtime does not
/// recognise, or one named by something other than Canopy, is no word of
/// Canopy's to act on.
// r[impl tls.canopy.outcomes]
pub fn classify_failure(failure: &RelayFailure) -> Answer {
    let (status, body) = match failure {
        RelayFailure::Status { status, body } => (*status, body.as_str()),
        RelayFailure::Unreachable(reason) => {
            return Answer::Unavailable {
                reason: format!("Canopy could not be reached: {}", tidy(reason)),
            };
        }
    };
    let problem: Option<Value> = serde_json::from_str(body).ok();
    let cause = problem
        .as_ref()
        .and_then(|p| p.get("type"))
        .and_then(Value::as_str)
        .and_then(|t| t.strip_prefix(CANOPY_CAUSE))
        .map(tidy);
    let detail = problem
        .as_ref()
        .and_then(|p| p.get("detail"))
        .and_then(Value::as_str)
        .map(tidy)
        .or_else(|| cause.clone())
        .unwrap_or_else(|| format!("Canopy answered {status}"));

    match (status, cause.as_deref()) {
        // The grant is withheld, the name is outside the group's domains, or an
        // operator has denied it to this server.
        (403, Some("auth-insufficient-permissions" | "name-not-entitled" | "dns-name-denied")) => {
            Answer::Refused { reason: detail }
        }
        // No zone Canopy manages covers the name, so it can publish nothing for
        // it; or the server contradicts the application type the request named.
        (409, Some("conflict" | "dns-name-type-mismatch")) => Answer::Refused { reason: detail },
        (409, Some("certificate-key-compromised")) => Answer::Revoked { reason: detail },
        // The runtime's own request was rejected. Only Canopy saying so makes
        // it the runtime's fault to report.
        (400, Some("bad-request")) => Answer::Malformed { reason: detail },
        _ => Answer::Unavailable {
            reason: format!("Canopy answered {status}: {detail}"),
        },
    }
}

/// Where the hostname stood when it was decided that Canopy is to be asked.
#[derive(Debug, Clone)]
pub struct Ask {
    pub hostname: String,
    /// The application type to name, when exactly one applies.
    pub app_type: Option<String>,
    /// What the cycle replaces.
    pub replacing: Identity,
    /// The chain index of the strategy responsible, and when it became so.
    pub step: usize,
    pub since: i64,
    pub chain_len: usize,
    /// Canopy is the strategy responsible. It is asked without being so to
    /// collect an order that is open or a renewal that is due, and a refusal
    /// then moves nothing.
    pub canopy_responsible: bool,
    pub trigger: AttemptTrigger,
}

/// What asking changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A chain was stored.
    Issued {
        cert_id: i64,
    },
    /// The order is open and nothing newer than what is held has arrived.
    Pending,
    Refused(String),
    /// The order failed, or the question could not be put; asked again later.
    Failed(String),
    /// The signing request was rejected as malformed.
    Rejected(String),
    /// The certificate was revoked or its key condemned; the key was replaced
    /// and a new order opened.
    Replaced(String),
}

/// The key held for a hostname.
struct KeyMaterial {
    id: i64,
    spki: Vec<u8>,
    csr_der: Vec<u8>,
}

/// The SHA-256 of a SubjectPublicKeyInfo, as hex: how Canopy names a key.
pub fn key_fingerprint(spki_der: &[u8]) -> String {
    Sha256::digest(spki_der)
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// Asks Canopy for certificates and keeps what it says.
pub struct Engine<R> {
    db: DbHandle,
    cipher: Arc<Cipher>,
    relay: R,
}

impl<R: CertificateRelay> Engine<R> {
    pub fn new(db: DbHandle, cipher: Arc<Cipher>, relay: R) -> Self {
        Self { db, cipher, relay }
    }

    /// Ask Canopy for the hostname's certificate, and keep the answer.
    ///
    /// The same call opens an order and collects it, and is safe to repeat.
    // r[impl tls.canopy.request]
    // r[impl tls.canopy.collect]
    pub async fn ask(&self, ask: Ask) -> Asked {
        match self.request_and_apply(&ask).await {
            Ok(ApplyResult::Done(outcome)) => Ok(outcome),
            Ok(ApplyResult::Replaced(replacement)) => self.reorder(ask, replacement).await,
            Err(error) => Err(Box::new(Failed { ask, error })),
        }
    }

    /// A certificate Canopy reported revoked, or whose key it reported
    /// condemned: stop serving it, replace the key and open a new order.
    // r[impl tls.canopy.revoked]
    pub async fn revoke_and_reorder(&self, ask: Ask, cert_id: i64, reason: String) -> Asked {
        match self.revoke(&ask, cert_id, reason) {
            Ok(replacement) => self.reorder(ask, replacement).await,
            Err(error) => Err(Box::new(Failed { ask, error })),
        }
    }

    /// Stop serving a revoked certificate and pass the hostname to the next
    /// strategy, without going on to ask for the replacement.
    ///
    /// Database work alone, reaching no relay, so a caller bounding what it
    /// spends on the relay does not hold this back: a certificate Canopy has
    /// revoked stops being served the moment the runtime learns of it. The
    /// chain row it leaves carries the open order, which is what has a later
    /// opportunity ask for the replacement.
    // r[impl tls.canopy.revoked]
    pub fn revoke(&self, ask: &Ask, cert_id: i64, reason: String) -> Result<Replacement> {
        let for_apply = ask.clone();
        let now = jiff::Timestamp::now().as_second();
        self.db
            .call(move |db| {
                db.atomically(|db| apply_revocation(db, &for_apply, cert_id, &reason, now))
            })
            .context(StorageSnafu)
    }

    /// Open the order for the key that replaced a condemned one.
    ///
    /// Once, not in a loop: an answer that condemns the new key as well is left
    /// for the next opportunity rather than chased here.
    async fn reorder(&self, ask: Ask, replacement: Replacement) -> Asked {
        let reason = replacement.reason.clone();
        // The revocation has already written where the hostname stands, so a
        // failure from here belongs to that position and not to the one the
        // caller asked from.
        let next = Ask {
            replacing: replacement.replacing,
            step: replacement.step,
            since: replacement.since,
            ..ask
        };
        match self.request_and_apply(&next).await {
            Ok(_) => Ok(Outcome::Replaced(reason)),
            Err(error) => Err(Box::new(Failed { ask: next, error })),
        }
    }

    async fn request_and_apply(&self, ask: &Ask) -> Result<ApplyResult> {
        let key = self.key_for(&ask.hostname)?;
        let csr_b64 = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&key.csr_der)
        };
        let answer = match self
            .relay
            .request(&ask.hostname, &csr_b64, ask.app_type.as_deref())
            .await
        {
            Ok(response) => classify_response(&response),
            Err(failure) => classify_failure(&failure),
        };

        let for_apply = ask.clone();
        let now = jiff::Timestamp::now().as_second();
        self.db
            .call(move |db| {
                db.atomically(|db| apply(db, &for_apply, key.id, &key.spki, answer, now))
            })
            .context(StorageSnafu)
    }

    /// The key for a hostname, generating and storing one when none is held.
    ///
    /// Stored before the request goes out, so a restart between the two finds
    /// the key again and asks about the same one rather than opening a second
    /// order.
    // r[impl tls.canopy.key]
    fn key_for(&self, hostname: &str) -> Result<KeyMaterial> {
        let host = hostname.to_owned();
        let existing = self
            .db
            .call(move |db| store::find_canopy_key(db, &host))
            .context(StorageSnafu)?;
        let (id, pair) = match existing {
            Some(row) => {
                let pem = self
                    .cipher
                    .decrypt(&row.key_ciphertext)
                    .context(CipherSnafu)?;
                use secrecy::ExposeSecret as _;
                let pair = keypair::from_pem(pem.expose_secret()).context(KeySnafu)?;
                (row.id, pair)
            }
            None => {
                let generated = keypair::generate(KeyType::EcdsaP256).context(KeySnafu)?;
                let csr = keypair::build_csr(&[hostname], &generated.inner).context(KeySnafu)?;
                let ciphertext = self.cipher.encrypt(&generated.pem).context(CipherSnafu)?;
                let host = hostname.to_owned();
                let id = self
                    .db
                    .call(move |db| {
                        store::insert_certificate(
                            db,
                            NewCertificate {
                                hostname: &host,
                                requested_hostnames: Some(&[host.as_str()]),
                                state: TlsCertState::CsrPending,
                                origin: super::TlsCertOrigin::Canopy,
                                cert_pem: None,
                                csr_pem: Some(&csr.pem),
                                key_ciphertext: &ciphertext,
                                key_type: KeyType::EcdsaP256,
                                metadata: store::CertMetadata::default(),
                                note: None,
                                acme_account_id: None,
                                renews: None,
                            },
                        )
                    })
                    .context(StorageSnafu)?;
                (id, generated.inner)
            }
        };
        let csr = keypair::build_csr(&[hostname], &pair).context(KeySnafu)?;
        use rcgen::PublicKeyData as _;
        Ok(KeyMaterial {
            id,
            spki: pair.subject_public_key_info(),
            csr_der: csr.der,
        })
    }
}

/// What applying an answer did, as far as the engine needs to know.
enum ApplyResult {
    Done(Outcome),
    Replaced(Replacement),
}

/// A request that could not be put, and where the hostname stands as the
/// engine left it.
///
/// The position is carried because the engine may have moved the hostname
/// before the failure: a revocation writes where the hostname now stands and
/// then asks for the replacement, so a failure between the two belongs to what
/// the revocation wrote. Recording it against the position the caller started
/// from would undo the revocation, putting the hostname back on Canopy with
/// nothing to replace and no order outstanding.
#[derive(Debug)]
pub struct Failed {
    pub ask: Ask,
    pub error: Error,
}

/// What asking Canopy produced: the answer, or the failure and where the
/// hostname stands.
///
/// The failure is boxed because it carries the whole question, several times
/// the size of any answer, and these calls answer rather than fail in all but
/// the exceptional case.
pub type Asked = std::result::Result<Outcome, Box<Failed>>;

/// Where a hostname stands after its certificate was revoked.
#[derive(Debug, Clone)]
pub struct Replacement {
    replacing: Identity,
    step: usize,
    since: i64,
    reason: String,
}

/// The recorded position for the hostname, set to where the decision found it.
fn position(db: &Db, ask: &Ask) -> rusqlite::Result<(chain::Row, Option<String>)> {
    let recorded =
        store::get_chain_row(db, &ask.hostname)?.filter(|r| r.row.identity == ask.replacing);
    Ok(match recorded {
        Some(TlsChainRow {
            row, last_error, ..
        }) => (
            chain::Row {
                step: ask.step,
                since: ask.since,
                ..row
            },
            last_error,
        ),
        None => (
            chain::Row {
                identity: ask.replacing,
                step: ask.step,
                since: ask.since,
                open_order_since: None,
                retry_not_before: None,
            },
            None,
        ),
    })
}

/// Close the attempt that is open for a hostname, or when none is and the
/// outcome is worth a log entry, open and close one.
///
/// Whether one is open is asked of the log itself rather than taken from the
/// hostname's recorded position: the two drifted apart whenever something
/// rewrote the position while an order was in flight, and an attempt the caller
/// believed closed then stayed open for ever.
// r[impl tls.canopy.attempts]
fn close_attempt(
    db: &Db,
    ask: &Ask,
    outcome: AttemptOutcome,
    cert_id: Option<i64>,
    error: Option<&str>,
    log_if_none: bool,
) -> rusqlite::Result<()> {
    let id = match store::latest_pending_attempt(db, &ask.hostname)? {
        Some(id) => id,
        None if log_if_none => store::insert_attempt(db, &ask.hostname, ask.trigger)?,
        None => return Ok(()),
    };
    store::finalize_attempt(db, id, outcome, cert_id, error)
}

/// Turn an answer into stored state.
///
/// Everything an answer changes is written together by the caller's
/// transaction: the certificate, the attempt log, and the hostname's recorded
/// position. A restart can then find the hostname where the answer left it and
/// never half-way.
// r[impl tls.canopy.outcomes]
fn apply(
    db: &Db,
    ask: &Ask,
    key_id: i64,
    key_spki: &[u8],
    answer: Answer,
    now: i64,
) -> rusqlite::Result<ApplyResult> {
    let (mut row, _) = position(db, ask)?;

    match answer {
        Answer::Issued { chain_pem } => {
            let parsed: ParsedChain = match parse::parse_chain(&chain_pem) {
                Ok(parsed) => parsed,
                Err(e) => {
                    return unavailable(
                        db,
                        row,
                        ask,
                        format!("Canopy returned a chain this runtime cannot read: {e}"),
                    );
                }
            };
            // r[impl tls.cert.validation.san-coverage]
            // The signing request named exactly the hostname, so a chain for
            // anything else is not what was asked for, whoever sent it: coverage
            // is all that binds a certificate to a name, and one that carried
            // other names would be served for them.
            let names: Vec<String> = parsed
                .san_dns_names
                .iter()
                .map(|n| n.to_ascii_lowercase())
                .collect();
            if names.len() != 1 || names[0] != ask.hostname.to_ascii_lowercase() {
                return unavailable(
                    db,
                    row,
                    ask,
                    format!(
                        "Canopy returned a certificate for {}, not for exactly {}",
                        if names.is_empty() {
                            "no names".to_owned()
                        } else {
                            // The names come from the certificate, which a
                            // relay chooses as much as it chooses its answers.
                            tidy(names.join(", "))
                        },
                        ask.hostname
                    ),
                );
            }
            if parsed.metadata.not_after.is_some_and(|end| end <= now) {
                return unavailable(
                    db,
                    row,
                    ask,
                    "Canopy returned a certificate that has already expired".to_owned(),
                );
            }
            // Both ends of the window: a certificate whose validity has not begun
            // is refused by every client it is served to, so storing it would
            // leave the hostname serving nothing while reading as covered.
            if parsed.metadata.not_before.is_some_and(|start| start > now) {
                return unavailable(
                    db,
                    row,
                    ask,
                    "Canopy returned a certificate that is not valid yet".to_owned(),
                );
            }
            match parse::leaf_spki_der(&chain_pem) {
                Ok(spki) if spki == key_spki => {}
                Ok(_) => {
                    return unavailable(
                        db,
                        row,
                        ask,
                        "Canopy returned a certificate for a key this runtime does not hold"
                            .to_owned(),
                    );
                }
                Err(e) => {
                    return unavailable(
                        db,
                        row,
                        ask,
                        format!("Canopy returned a chain this runtime cannot read: {e}"),
                    );
                }
            }

            let held = store::get_certificate(db, key_id)?;
            let newer = match &held {
                Some(h) if h.state == TlsCertState::Active => {
                    match (h.not_after, parsed.metadata.not_after) {
                        (Some(held_end), Some(new_end)) => new_end > held_end,
                        // Without both expiries nothing says this is newer, and
                        // replacing a working certificate on that is not warranted.
                        _ => false,
                    }
                }
                _ => true,
            };
            if !newer {
                // The renewal has not landed: what Canopy holds is what is held.
                return pending(db, row, ask, now);
            }
            if !store::activate_canopy_chain(db, key_id, &parsed.chain_pem, &parsed.metadata)? {
                return unavailable(
                    db,
                    row,
                    ask,
                    "the key for this certificate is no longer held".to_owned(),
                );
            }
            close_attempt(db, ask, AttemptOutcome::Success, Some(key_id), None, true)?;
            // The cycle is over: the new certificate is what the next one
            // replaces, so what was recorded for this one belongs to the past.
            store::clear_chain_row(db, &ask.hostname)?;
            Ok(ApplyResult::Done(Outcome::Issued { cert_id: key_id }))
        }
        Answer::Pending => pending(db, row, ask, now),
        Answer::Refused { reason } => {
            // r[impl tls.chain.refusal]
            close_attempt(db, ask, AttemptOutcome::Failure, None, Some(&reason), true)?;
            let advances =
                ask.canopy_responsible && !ask.replacing.is_none() && ask.step + 1 < ask.chain_len;
            if advances {
                row.step = ask.step + 1;
                row.since = now;
            }
            row.open_order_since = None;
            row.retry_not_before = Some(now + AUTO_RETRY_DEBOUNCE_SECS);
            store::put_chain_row(db, &ask.hostname, &row, Some(&reason))?;
            Ok(ApplyResult::Done(Outcome::Refused(reason)))
        }
        Answer::OrderFailed { reason } => {
            // r[impl tls.chain.no-certificate]
            // A strategy that fails is asked again at the after-failure cadence,
            // as one that refuses is. Canopy goes on retrying the order on its
            // own side meanwhile, and the next ask collects whatever came of it.
            close_attempt(db, ask, AttemptOutcome::Failure, None, Some(&reason), false)?;
            row.open_order_since = None;
            row.retry_not_before = Some(now + AUTO_RETRY_DEBOUNCE_SECS);
            store::put_chain_row(db, &ask.hostname, &row, Some(&reason))?;
            Ok(ApplyResult::Done(Outcome::Failed(reason)))
        }
        Answer::Unavailable { reason } => unavailable(db, row, ask, reason),
        Answer::Malformed { reason } => {
            close_attempt(db, ask, AttemptOutcome::Failure, None, Some(&reason), false)?;
            row.open_order_since = None;
            row.retry_not_before = Some(now + AUTO_RETRY_DEBOUNCE_SECS);
            store::put_chain_row(db, &ask.hostname, &row, Some(&reason))?;
            Ok(ApplyResult::Done(Outcome::Rejected(reason)))
        }
        Answer::Revoked { reason } => Ok(ApplyResult::Replaced(apply_revocation(
            db, ask, key_id, &reason, now,
        )?)),
    }
}

/// The order is open: note it, once.
// r[impl tls.canopy.attempts]
fn pending(db: &Db, mut row: chain::Row, ask: &Ask, now: i64) -> rusqlite::Result<ApplyResult> {
    // One entry per order, asked of the log rather than of the position, which
    // something may have rewritten while the order was in flight.
    if store::latest_pending_attempt(db, &ask.hostname)?.is_none() {
        store::insert_attempt(db, &ask.hostname, ask.trigger)?;
    }
    row.open_order_since.get_or_insert(now);
    row.retry_not_before = None;
    store::put_chain_row(db, &ask.hostname, &row, None)?;
    Ok(ApplyResult::Done(Outcome::Pending))
}

/// The question could not be put or answered. Nothing about the order is
/// learnt, so nothing about it changes but the reason.
///
/// No wait is recorded, so the question is put again on the next report tick.
/// That tick is the back-off, and it is one question per hostname per interval
/// at the report's own pace, only while an offer is live to carry it. The
/// causes here (the relay down, a paused server, an answer not understood) are
/// ones that pass on Canopy's side without the runtime doing anything, and the
/// hour [`AUTO_RETRY_DEBOUNCE_SECS`] holds a failed order back would leave a
/// hostname without a certificate for that hour after the relay returned. A
/// failure to put the question at all, which the runtime itself causes and
/// which does not pass by waiting, is held back for that hour by the driver.
fn unavailable(
    db: &Db,
    mut row: chain::Row,
    ask: &Ask,
    reason: String,
) -> rusqlite::Result<ApplyResult> {
    row.retry_not_before = None;
    store::put_chain_row(db, &ask.hostname, &row, Some(&reason))?;
    Ok(ApplyResult::Done(Outcome::Failed(reason)))
}

/// Stop serving a revoked certificate, destroy its key, and record where the
/// hostname stands: the revoked certificate is the one being replaced, and the
/// hostname has passed to the next strategy at once.
// r[impl tls.canopy.revoked]
fn apply_revocation(
    db: &Db,
    ask: &Ask,
    cert_id: i64,
    reason: &str,
    now: i64,
) -> rusqlite::Result<Replacement> {
    let held: Option<TlsCertificate> = store::get_certificate(db, cert_id)?;
    let was_serving = held
        .as_ref()
        .is_some_and(|c| c.state == TlsCertState::Active && c.cert_pem.is_some());
    store::revoke_canopy_certificate(db, cert_id)?;
    close_attempt(db, ask, AttemptOutcome::Failure, None, Some(reason), false)?;

    let (replacing, step, since) = if was_serving {
        (
            Identity {
                cert_id: Some(cert_id),
                not_after: held.as_ref().and_then(|c| c.not_after),
            },
            1.min(ask.chain_len.saturating_sub(1)),
            now,
        )
    } else {
        (ask.replacing, ask.step, ask.since)
    };
    store::put_chain_row(
        db,
        &ask.hostname,
        &chain::Row {
            identity: replacing,
            step,
            since,
            // A replacement is outstanding from here until Canopy delivers one
            // or refuses: the hostname serves on the next strategy meanwhile,
            // and this is what has it go on asking.
            // r[impl tls.canopy.revoked]
            open_order_since: Some(now),
            retry_not_before: None,
        },
        Some(reason),
    )?;
    Ok(Replacement {
        replacing,
        step,
        since,
        reason: reason.to_owned(),
    })
}

/// A Canopy certificate the runtime holds that Canopy reports revoked or whose
/// key it reports condemned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condemned {
    pub hostname: String,
    pub cert_id: i64,
    pub reason: String,
}

/// Which of the Canopy certificates the runtime holds does `held`, the list
/// Canopy returns with each report, say are revoked or have a condemned key?
///
/// Matched on the name and on the key's fingerprint, so a revocation of an
/// older key for the same name is not read as one of the key now held.
// r[impl tls.canopy.revoked]
pub fn condemned_by(
    db: &Db,
    held: &[bestool_canopy::schema::HeldCertificate],
) -> rusqlite::Result<Vec<Condemned>> {
    let mut out = Vec::new();
    if held.is_empty() {
        return Ok(out);
    }
    for (id, hostname, pem) in store::list_active_canopy_certificates(db)? {
        let Ok(spki) = parse::leaf_spki_der(&pem) else {
            continue;
        };
        let fingerprint = key_fingerprint(&spki);
        let name = hostname.to_ascii_lowercase();
        for h in held {
            if h.name.to_ascii_lowercase() == name
                && h.key_fingerprint.eq_ignore_ascii_case(&fingerprint)
                && (h.revoked || h.key_must_be_replaced)
            {
                out.push(Condemned {
                    hostname: hostname.clone(),
                    cert_id: id,
                    reason: if h.key_must_be_replaced {
                        "Canopy reports the key must be replaced".to_owned()
                    } else {
                        "Canopy reports the certificate revoked".to_owned()
                    },
                });
                break;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
