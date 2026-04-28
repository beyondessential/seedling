//! Daemon-driven ACME-DNS issuance.
//!
//! Drives `instant-acme` end-to-end for a single hostname:
//!
//! 1. Restore or create the ACME account against the configured directory.
//! 2. Open a new order for the hostname.
//! 3. Walk authorizations, publish each DNS-01 TXT record via the configured
//!    [`DnsProvider`], wait for the change to propagate, then signal the CA
//!    that the challenge is ready.
//! 4. Wait for the order to become Ready, generate a fresh ECDSA P-256
//!    keypair locally, submit a CSR, and download the resulting chain.
//! 5. Persist the cert+key into `tls_certificates` (origin = `acme_dns`),
//!    superseding any prior active row for the same hostname.
//! 6. Best-effort cleanup of the TXT record.
//!
//! The flow is fully async and reentrant: a renewal just runs this same
//! function with the same hostname; the new row supersedes the old one
//! atomically when persisted.

use std::time::Duration;

use instant_acme::{
    Account, AuthorizationStatus, CertificateIdentifier, ChallengeType, Identifier, LetsEncrypt,
    NewAccount, NewOrder, OrderStatus, RetryPolicy,
};
use rustls_pki_types::Der;
use secrecy::{ExposeSecret, SecretString};
use snafu::{ResultExt, Snafu};

use super::{
    KeyType, TlsCertOrigin, TlsCertState,
    dns::{self, DnsProvider, challenge_record_name},
    keypair, parse, store,
};
use crate::runtime::{db::DbHandle, secrets::Cipher};

/// Default Let's Encrypt production directory; override via [`IssueParams`].
pub fn default_directory_url() -> String {
    LetsEncrypt::Production.url().to_owned()
}

#[derive(Debug, Snafu)]
pub enum AcmeError {
    #[snafu(display("acme protocol error: {source}"))]
    Acme { source: instant_acme::Error },

    #[snafu(display("dns provider error: {source}"))]
    Dns { source: dns::DnsError },

    #[snafu(display("keypair generation: {source}"))]
    Keypair { source: keypair::Error },

    #[snafu(display("certificate parse: {source}"))]
    Parse { source: parse::Error },

    #[snafu(display("encryption error: {source}"))]
    Cipher {
        source: crate::runtime::secrets::Error,
    },

    #[snafu(display("storage error: {source}"))]
    Storage { source: rusqlite::Error },

    #[snafu(display("ACME flow timed out: {stage}"))]
    Timeout { stage: &'static str },

    #[snafu(display("authorization in unexpected state: {state:?}"))]
    BadAuthState { state: AuthorizationStatus },

    #[snafu(display("no DNS-01 challenge offered for {hostname}"))]
    NoDnsChallenge { hostname: String },

    #[snafu(display("order finalization returned no certificate"))]
    NoCertificate,

    #[snafu(display("account credentials serialization: {source}"))]
    AccountSerde { source: serde_json::Error },

    #[snafu(display("DNS provider {name} not configured"))]
    ProviderNotFound { name: String },
}

/// Parameters to drive a single issuance.
pub struct IssueParams<'a> {
    pub hostname: &'a str,
    pub contact_email: &'a str,
    pub directory_url: &'a str,
    pub dns_provider_name: &'a str,
    /// PEM chain of the previous certificate for this hostname, when this
    /// is a renewal. Used to populate the order's `replaces` field per
    /// RFC 9773 § 5 so the CA can skip rate-limiting on the renewal and
    /// invalidate any pending ARI advice for the old cert.
    // r[impl tls.cert.ari]
    pub previous_cert_pem: Option<&'a str>,
    /// ACME profile name forwarded to the CA via the profiles
    /// extension. `None` means "let the CA pick its default". Let's
    /// Encrypt's `shortlived` profile yields ~6-day certs; absent a
    /// profile they issue ~90-day certs. The order fails with a clear
    /// CA-side error if the directory does not advertise the profile.
    // r[impl tls.settings.cert-profile]
    pub cert_profile: Option<&'a str>,
}

/// Outcome of a successful issuance.
pub struct Issued {
    pub cert_id: i64,
    pub not_after: i64,
}

/// How long to wait between publishing the TXT record and notifying the CA
/// that the challenge is ready. Generous default to give DNS time to
/// propagate; can be tuned per-provider later.
const DNS_PROPAGATION_DELAY: Duration = Duration::from_secs(30);

/// Run the full ACME-DNS issuance flow against a single hostname.
///
/// `db` is held by `Arc` so background callers (the renewal task) can share
/// it without lifetime ceremony. DB access is sliced into short, sync
/// closures via `DbHandle::call`; network I/O happens between calls so the
/// DB worker thread never blocks on remote services.
// r[impl tls.strategy.acme-dns]
pub async fn issue(
    db: &DbHandle,
    cipher: &Cipher,
    params: IssueParams<'_>,
) -> Result<Issued, AcmeError> {
    let provider_name_owned = params.dns_provider_name.to_owned();
    let raw = db
        .call(move |db_inner| store::get_dns_provider_raw(db_inner, &provider_name_owned))
        .context(StorageSnafu)?
        .ok_or_else(|| {
            ProviderNotFoundSnafu {
                name: params.dns_provider_name.to_owned(),
            }
            .build()
        })?;
    let config_secret = cipher
        .decrypt(&raw.config_ciphertext)
        .context(CipherSnafu)?;
    let provider_entry = super::DnsProviderEntry {
        name: raw.name,
        kind: raw.kind,
        config: config_secret,
        created_at: raw.created_at,
        updated_at: raw.updated_at,
    };
    let provider = dns::build_provider(&provider_entry).context(DnsSnafu)?;

    let (account, account_id) =
        load_or_create_account(db, cipher, params.directory_url, params.contact_email).await?;

    let identifiers = vec![Identifier::Dns(params.hostname.to_owned())];
    let mut new_order = NewOrder::new(&identifiers);
    // r[impl tls.cert.ari]
    // If we know the previous cert, hand its CertificateIdentifier
    // to the CA via the order's `replaces` field. Saves us a rate-limit
    // round-trip on Let's Encrypt and lets ARI advice for the old cert
    // be invalidated cleanly. Failure to extract the identifier is not
    // fatal — we just issue without the hint.
    let replaces_owned = params
        .previous_cert_pem
        .and_then(|pem| extract_cert_identifier(pem).ok())
        .flatten();
    if let Some(ref ident) = replaces_owned {
        new_order = new_order.replaces(ident.clone());
    }
    // r[impl tls.settings.cert-profile]
    if let Some(profile) = params.cert_profile {
        new_order = new_order.profile(profile);
    }
    let mut order = account.new_order(&new_order).await.context(AcmeSnafu)?;

    let challenge_name = challenge_record_name(params.hostname);
    let mut planted: Vec<String> = Vec::new();

    {
        let mut auths = order.authorizations();
        while let Some(auth_result) = auths.next().await {
            let mut auth = auth_result.context(AcmeSnafu)?;
            match auth.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                other => return BadAuthStateSnafu { state: other }.fail(),
            }

            let mut challenge = auth.challenge(ChallengeType::Dns01).ok_or_else(|| {
                NoDnsChallengeSnafu {
                    hostname: params.hostname.to_owned(),
                }
                .build()
            })?;

            let dns_value = challenge.key_authorization().dns_value();
            provider
                .set_txt(&challenge_name, &dns_value)
                .await
                .context(DnsSnafu)?;
            planted.push(dns_value.clone());

            tokio::time::sleep(DNS_PROPAGATION_DELAY).await;

            challenge.set_ready().await.context(AcmeSnafu)?;
        }
    }

    let status = order
        .poll_ready(&RetryPolicy::default())
        .await
        .context(AcmeSnafu)?;
    if status != OrderStatus::Ready {
        // Best-effort cleanup before we bail.
        cleanup_txt(provider.as_ref(), &challenge_name, &planted).await;
        return TimeoutSnafu {
            stage: "order_ready",
        }
        .fail();
    }

    // Generate our own keypair + CSR rather than letting instant-acme do
    // it via its `rcgen` feature, so the same code path serves the manual
    // CSR flow in phase 4.
    let key = keypair::generate(KeyType::EcdsaP256).context(KeypairSnafu)?;
    let csr = keypair::build_csr(params.hostname, &key.inner).context(KeypairSnafu)?;
    order.finalize_csr(&csr.der).await.context(AcmeSnafu)?;

    let cert_chain_pem = order
        .poll_certificate(&RetryPolicy::default())
        .await
        .context(AcmeSnafu)?;

    cleanup_txt(provider.as_ref(), &challenge_name, &planted).await;

    let parsed = parse::parse_chain(&cert_chain_pem).context(ParseSnafu)?;
    let key_ct = cipher.encrypt(&key.pem).context(CipherSnafu)?;

    let metadata = parsed.metadata.clone();
    let not_after = metadata.not_after.unwrap_or(0);
    let hostname_owned = params.hostname.to_owned();
    let chain_pem = parsed.chain_pem.clone();

    let cert_id = db
        .call(move |db_inner| -> rusqlite::Result<i64> {
            let id = store::insert_certificate(
                db_inner,
                &hostname_owned,
                TlsCertState::Active,
                TlsCertOrigin::AcmeDns,
                Some(&chain_pem),
                None,
                &key_ct,
                KeyType::EcdsaP256,
                metadata,
                None,
                Some(account_id),
            )?;
            store::supersede_other_active_for_hostname(db_inner, &hostname_owned, id)?;
            Ok(id)
        })
        .context(StorageSnafu)?;

    // r[impl tls.cert.ari]
    // Pull the renewal window from the CA for the cert we just issued
    // and stamp it onto the row. The renewal task uses this in
    // preference to the fixed 1/3-lifetime threshold. Failure here is
    // best-effort: the cert itself is fine, we just don't have ARI
    // guidance and the renewal task falls back.
    if let Some(new_ident) = build_cert_identifier(&parsed) {
        match account.renewal_info(&new_ident).await {
            Ok((info, _retry_after)) => {
                let start = info.suggested_window.start.unix_timestamp();
                let end = info.suggested_window.end.unix_timestamp();
                let polled = jiff::Timestamp::now().as_second();
                let _ = db.call(move |db_inner| {
                    store::update_ari_window(db_inner, cert_id, start, end, polled)
                });
                tracing::debug!(
                    %cert_id,
                    ari_start = start,
                    ari_end = end,
                    "ACME renewal info captured"
                );
            }
            Err(e) => {
                tracing::debug!(
                    %cert_id,
                    error = %e,
                    "ACME renewal info unavailable; renewal will fall back to fixed-fraction threshold"
                );
            }
        }
    }

    Ok(Issued { cert_id, not_after })
}

/// Try to extract a [`CertificateIdentifier`] from a PEM chain. Returns
/// `None` when the leaf has no AKI extension (in which case the CA can't
/// look the cert up by RFC 9773 anyway).
fn extract_cert_identifier(
    pem: &str,
) -> std::result::Result<Option<CertificateIdentifier<'static>>, parse::Error> {
    let parsed = parse::parse_chain(pem)?;
    Ok(build_cert_identifier(&parsed))
}

fn build_cert_identifier(parsed: &parse::ParsedChain) -> Option<CertificateIdentifier<'static>> {
    let aki = parsed.leaf_aki_der.as_deref()?;
    let serial = parsed.leaf_serial_der.as_slice();
    Some(CertificateIdentifier::new(Der::from_slice(aki), Der::from_slice(serial)).into_owned())
}

/// Restore an ACME account from the encrypted credentials in the DB, or
/// create a fresh one and persist it. Returns the live `Account` plus the
/// row id for `tls_acme_accounts.id` so callers can stamp issued certs.
///
/// The lookup is keyed on the directory URL alone — a single account is
/// reused across email changes. When the operator-configured
/// `contact_email` differs from the email last persisted on the
/// account, the account's contacts are updated on the directory (RFC
/// 8555 §7.3.2) and only then is the persisted email rotated. A failed
/// update logs and proceeds with the existing email so issuance is
/// never blocked on a contact-only change.
// r[impl tls.acme.account.persist]
// r[impl tls.acme.account.contact-update]
async fn load_or_create_account(
    db: &DbHandle,
    cipher: &Cipher,
    directory_url: &str,
    contact_email: &str,
) -> Result<(Account, i64), AcmeError> {
    let directory_owned = directory_url.to_owned();
    let existing = db
        .call(move |db_inner| store::get_acme_account_for_directory(db_inner, &directory_owned))
        .context(StorageSnafu)?;

    if let Some(row) = existing {
        let plaintext = store::decrypt_acme_account_key(cipher, &row).context(CipherSnafu)?;
        let creds: instant_acme::AccountCredentials =
            serde_json::from_str(plaintext.expose_secret()).context(AccountSerdeSnafu)?;
        let account = Account::builder()
            .context(AcmeSnafu)?
            .from_credentials(creds)
            .await
            .context(AcmeSnafu)?;

        if row.contact_email != contact_email {
            let contact_uri = format!("mailto:{contact_email}");
            match account.update_contacts(&[contact_uri.as_str()]).await {
                Ok(()) => {
                    let id = row.id;
                    let new_email = contact_email.to_owned();
                    if let Err(e) = db.call(move |db_inner| {
                        store::set_acme_account_contact_email(db_inner, id, &new_email)
                    }) {
                        tracing::warn!(
                            account_id = row.id,
                            error = %e,
                            "ACME contacts updated on directory but DB row update failed"
                        );
                    } else {
                        tracing::info!(
                            account_id = row.id,
                            old = %row.contact_email,
                            new = %contact_email,
                            "updated ACME account contact email"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        account_id = row.id,
                        directory = %directory_url,
                        error = %e,
                        "directory rejected update_contacts; reusing account with prior email"
                    );
                }
            }
        }

        return Ok((account, row.id));
    }

    let contact_uri = format!("mailto:{contact_email}");
    let new_account = NewAccount {
        contact: &[contact_uri.as_str()],
        terms_of_service_agreed: true,
        only_return_existing: false,
    };

    let (account, credentials) = Account::builder()
        .context(AcmeSnafu)?
        .create(&new_account, directory_url.to_owned(), None)
        .await
        .context(AcmeSnafu)?;

    let creds_json = serde_json::to_string(&credentials).context(AccountSerdeSnafu)?;
    let account_url = account.id().to_owned();
    let creds_secret = SecretString::new(creds_json.into());
    // Encrypt before crossing into the DB closure so the closure stays Cipher-free.
    let creds_ct = cipher.encrypt(&creds_secret).context(CipherSnafu)?;

    let directory_owned = directory_url.to_owned();
    let contact_owned = contact_email.to_owned();
    let id = db
        .call(move |db_inner| {
            store::insert_acme_account_raw(
                db_inner,
                &directory_owned,
                &contact_owned,
                &account_url,
                &creds_ct,
            )
        })
        .context(StorageSnafu)?;

    Ok((account, id))
}

async fn cleanup_txt(provider: &dyn DnsProvider, name: &str, values: &[String]) {
    for v in values {
        if let Err(e) = provider.clear_txt(name, v).await {
            tracing::warn!(name = %name, error = %e, "best-effort TXT cleanup failed");
        }
    }
}
