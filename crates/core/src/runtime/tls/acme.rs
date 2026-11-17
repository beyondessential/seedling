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
//! 6. Best-effort cleanup of the TXT records, however the challenge phase
//!    ended.
//!
//! The flow is fully async and reentrant: a renewal just runs this same
//! function with the same hostname; the new row supersedes the old one
//! atomically when persisted.

use std::time::Duration;

use instant_acme::{
    Account, AuthorizationStatus, CertificateIdentifier, ChallengeType, Identifier, LetsEncrypt,
    NewAccount, NewOrder, Order, OrderStatus, RetryPolicy,
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

    #[snafu(display("ACME account {account_id} is no longer stored"))]
    AccountGone { account_id: i64 },

    #[snafu(display("certificate carries no authority key identifier to name it by"))]
    NoIdentifier,
}

/// A certificate an issuance replaces.
pub struct PreviousCert<'a> {
    pub chain_pem: &'a str,
    /// The ACME account that issued it; `None` for a certificate obtained
    /// any other way.
    pub acme_account_id: Option<i64>,
}

/// Parameters to drive a single issuance.
pub struct IssueParams<'a> {
    pub hostname: &'a str,
    pub contact_email: &'a str,
    pub directory_url: &'a str,
    pub dns_provider_name: &'a str,
    /// The certificate this issuance replaces, when it is a renewal.
    // spec: TLS#renewal
    pub previous: Option<PreviousCert<'a>>,
    /// ACME profile name forwarded to the CA via the profiles
    /// extension. `None` means "let the CA pick its default". Let's
    /// Encrypt's `shortlived` profile yields ~6-day certs; absent a
    /// profile they issue ~90-day certs. The order fails with a clear
    /// CA-side error if the directory does not advertise the profile.
    // spec: TLS#acme-accounts-and-settings
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
// spec: TLS#acme-dns-01-issuance
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
    let new_order = || {
        let order = NewOrder::new(&identifiers);
        // spec: TLS#acme-accounts-and-settings
        match params.cert_profile {
            Some(profile) => order.profile(profile),
            None => order,
        }
    };
    let replaces = replaces_field(params.previous.as_ref(), account_id);
    let mut order = match replaces {
        Some(ident) => match account.new_order(&new_order().replaces(ident)).await {
            // A directory without renewal information takes no `replaces`,
            // and the order is refused before it is sent.
            Err(instant_acme::Error::Unsupported(_)) => {
                account.new_order(&new_order()).await.context(AcmeSnafu)?
            }
            other => other.context(AcmeSnafu)?,
        },
        None => account.new_order(&new_order()).await.context(AcmeSnafu)?,
    };

    let challenge_name = challenge_record_name(params.hostname);
    let mut planted: Vec<String> = Vec::new();
    let obtained = obtain_chain(
        &mut order,
        provider.as_ref(),
        &challenge_name,
        params.hostname,
        &mut planted,
    )
    .await;
    // Whatever way the challenge phase ended, the records it published are
    // retracted here, outside it, so no early return inside it can leave one
    // in the operator's zone.
    // spec: TLS#acme-dns-01-issuance
    cleanup_txt(provider.as_ref(), &challenge_name, &planted).await;
    let (cert_chain_pem, key) = obtained?;

    let parsed = parse::parse_chain(&cert_chain_pem).context(ParseSnafu)?;
    let key_ct = cipher.encrypt(&key.pem).context(CipherSnafu)?;

    let metadata = parsed.metadata.clone();
    let not_after = metadata.not_after.unwrap_or(0);
    let hostname_owned = params.hostname.to_owned();
    let chain_pem = parsed.chain_pem.clone();

    let cert_id = db
        .call(move |db_inner| -> rusqlite::Result<i64> {
            let id = store::insert_and_supersede(
                db_inner,
                store::NewCertificate {
                    hostname: &hostname_owned,
                    requested_hostnames: None,
                    state: TlsCertState::Active,
                    origin: TlsCertOrigin::AcmeDns,
                    cert_pem: Some(&chain_pem),
                    csr_pem: None,
                    key_ciphertext: &key_ct,
                    key_type: KeyType::EcdsaP256,
                    metadata,
                    note: None,
                    acme_account_id: Some(account_id),
                    renews: None,
                },
            )?;
            Ok(id)
        })
        .context(StorageSnafu)?;

    // spec: TLS#renewal
    // Pull the renewal window from the CA for the cert we just issued
    // and stamp it onto the row. The renewal task uses this in
    // preference to the fixed 1/3-lifetime threshold. Failure here is
    // best-effort: the cert itself is fine, we just don't have ARI
    // guidance and the renewal task falls back.
    if let Some(new_ident) = build_cert_identifier(&parsed) {
        match account.renewal_info(&new_ident).await {
            Ok((info, retry_after)) => {
                let start = info.suggested_window.start.unix_timestamp();
                let end = info.suggested_window.end.unix_timestamp();
                let polled = jiff::Timestamp::now().as_second();
                let next = next_ari_poll(polled, Some(retry_after));
                let _ = db.call(move |db_inner| {
                    store::update_ari_window(db_inner, cert_id, start, end, polled, next)
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

/// The fewest seconds until the next renewal-information fetch, whatever the
/// CA's Retry-After asks: RFC 9773 §4.3.2 has the client set limits.
const ARI_MIN_INTERVAL_SECS: i64 = 60;
/// The most seconds until the next fetch, whatever the CA asks.
const ARI_MAX_INTERVAL_SECS: i64 = 86_400;
/// How long after a failed fetch the next is made, RFC 9773 §4.3.3.
const ARI_AFTER_FAILURE_SECS: i64 = 6 * 3600;

/// When renewal information fetched at `now` is next fetched: after the CA's
/// Retry-After, held within limits, or six hours after a failed fetch.
// spec: TLS#renewal
pub fn next_ari_poll(now: i64, retry_after: Option<Duration>) -> i64 {
    match retry_after {
        Some(after) => {
            let secs = i64::try_from(after.as_secs()).unwrap_or(i64::MAX);
            now + secs.clamp(ARI_MIN_INTERVAL_SECS, ARI_MAX_INTERVAL_SECS)
        }
        None => now + ARI_AFTER_FAILURE_SECS,
    }
}

/// Fetch a certificate's renewal information again and record it, or, when
/// the fetch fails, keep the window it holds and try again later.
// spec: TLS#renewal
pub async fn refresh_renewal_info(db: &DbHandle, cipher: &Cipher, poll: &store::AriPoll) {
    let cert_id = poll.cert_id;
    let fetched = fetch_renewal_info(db, cipher, poll).await;
    let now = jiff::Timestamp::now().as_second();
    let recorded = match fetched {
        Ok((start, end, retry_after)) => {
            let next = next_ari_poll(now, Some(retry_after));
            db.call(move |db| store::update_ari_window(db, cert_id, start, end, now, next))
        }
        Err(e) => {
            tracing::warn!(%cert_id, error = %e, "ACME renewal info re-fetch failed; keeping the stored window");
            let next = next_ari_poll(now, None);
            db.call(move |db| store::defer_ari_poll(db, cert_id, next))
        }
    };
    if let Err(e) = recorded {
        tracing::error!(%cert_id, error = %e, "could not record ACME renewal info");
    }
}

async fn fetch_renewal_info(
    db: &DbHandle,
    cipher: &Cipher,
    poll: &store::AriPoll,
) -> Result<(i64, i64, Duration), AcmeError> {
    let account_id = poll.acme_account_id;
    let row = db
        .call(move |db| store::get_acme_account_by_id(db, account_id))
        .context(StorageSnafu)?
        .ok_or_else(|| AccountGoneSnafu { account_id }.build())?;
    let plaintext = store::decrypt_acme_account_key(cipher, &row).context(CipherSnafu)?;
    let creds: instant_acme::AccountCredentials =
        serde_json::from_str(plaintext.expose_secret()).context(AccountSerdeSnafu)?;
    let account = Account::builder()
        .context(AcmeSnafu)?
        .from_credentials(creds)
        .await
        .context(AcmeSnafu)?;
    let parsed = parse::parse_chain(&poll.chain_pem).context(ParseSnafu)?;
    let ident = build_cert_identifier(&parsed).ok_or_else(|| NoIdentifierSnafu.build())?;
    let (info, retry_after) = account.renewal_info(&ident).await.context(AcmeSnafu)?;
    Ok((
        info.suggested_window.start.unix_timestamp(),
        info.suggested_window.end.unix_timestamp(),
        retry_after,
    ))
}

/// The part of an order during which the CA needs the DNS-01 challenge
/// records: answer every pending authorization, then finalise the order and
/// download the chain.
///
/// Every record published is pushed onto `planted` as soon as the provider
/// accepts it, so the caller can retract them however this returns.
async fn obtain_chain(
    order: &mut Order,
    provider: &dyn DnsProvider,
    challenge_name: &str,
    hostname: &str,
    planted: &mut Vec<String>,
) -> Result<(String, keypair::GeneratedKey), AcmeError> {
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
                    hostname: hostname.to_owned(),
                }
                .build()
            })?;

            let dns_value = challenge.key_authorization().dns_value();
            provider
                .set_txt(challenge_name, &dns_value)
                .await
                .context(DnsSnafu)?;
            planted.push(dns_value);

            tokio::time::sleep(DNS_PROPAGATION_DELAY).await;

            challenge.set_ready().await.context(AcmeSnafu)?;
        }
    }

    let status = order
        .poll_ready(&RetryPolicy::default())
        .await
        .context(AcmeSnafu)?;
    if status != OrderStatus::Ready {
        return TimeoutSnafu {
            stage: "order_ready",
        }
        .fail();
    }

    // Generate our own keypair + CSR rather than letting instant-acme do
    // it via its `rcgen` feature, so the same code path serves the manual
    // CSR flow.
    let key = keypair::generate(KeyType::EcdsaP256).context(KeypairSnafu)?;
    let csr = keypair::build_csr(&[hostname], &key.inner).context(KeypairSnafu)?;
    order.finalize_csr(&csr.der).await.context(AcmeSnafu)?;

    let cert_chain_pem = order
        .poll_certificate(&RetryPolicy::default())
        .await
        .context(AcmeSnafu)?;
    Ok((cert_chain_pem, key))
}

/// What an order placed through `account_id` names in its `replaces` field.
///
/// Only a certificate this account issued: the CA knows no other by its
/// identifier, as RFC 9773 §5 asks. Failure to extract the identifier is not
/// fatal — the order is placed without the hint.
// spec: TLS#renewal
fn replaces_field(
    previous: Option<&PreviousCert<'_>>,
    account_id: i64,
) -> Option<CertificateIdentifier<'static>> {
    previous
        .filter(|p| p.acme_account_id == Some(account_id))
        .and_then(|p| extract_cert_identifier(p.chain_pem).ok())
        .flatten()
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
// spec: TLS#acme-accounts-and-settings
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

#[cfg(test)]
mod tests {
    use rcgen::{CertificateParams, DistinguishedName, Issuer, KeyPair, PKCS_ECDSA_P256_SHA256};

    use super::*;

    /// A leaf signed by a CA, so it carries the authority key identifier
    /// RFC 9773 identifies a certificate by.
    fn ca_signed_leaf() -> String {
        let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.distinguished_name = DistinguishedName::new();
        ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca.use_authority_key_identifier_extension = true;
        let issuer = Issuer::new(ca, KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap());
        let mut leaf = CertificateParams::new(vec!["a.example.com".to_owned()]).unwrap();
        leaf.use_authority_key_identifier_extension = true;
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        leaf.signed_by(&key, &issuer).unwrap().pem()
    }

    // spec: TLS#renewal
    #[test]
    fn renewal_info_is_fetched_again_after_retry_after_within_limits() {
        let now = 1_000_000;
        assert_eq!(
            next_ari_poll(now, Some(Duration::from_secs(21_600))),
            now + 21_600
        );
        assert_eq!(next_ari_poll(now, Some(Duration::from_secs(1))), now + 60);
        assert_eq!(
            next_ari_poll(now, Some(Duration::from_secs(7 * 86_400))),
            now + 86_400
        );
        assert_eq!(next_ari_poll(now, None), now + 6 * 3600);
    }

    // spec: TLS#renewal
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_re_fetch_keeps_the_stored_window_and_tries_again_later() {
        let cipher = Cipher::for_tests();
        let db = crate::runtime::db::Db::open_in_memory().unwrap();
        let now = jiff::Timestamp::now().as_second();
        let pem = ca_signed_leaf();
        let account_id = store::insert_acme_account(
            &db,
            &cipher,
            "https://acme.invalid/directory",
            "ops@example.com",
            "https://acme.invalid/acct/1",
            &SecretString::new("not credentials".into()),
        )
        .unwrap();
        let cert_id = store::insert_certificate(
            &db,
            store::NewCertificate {
                hostname: "a.example.com",
                requested_hostnames: None,
                state: TlsCertState::Active,
                origin: TlsCertOrigin::AcmeDns,
                cert_pem: Some(&pem),
                csr_pem: None,
                key_ciphertext: b"key",
                key_type: KeyType::EcdsaP256,
                metadata: store::CertMetadata {
                    not_after: Some(now + 30 * 86_400),
                    ..Default::default()
                },
                note: None,
                acme_account_id: Some(account_id),
                renews: None,
            },
        )
        .unwrap();
        store::update_ari_window(&db, cert_id, now + 100, now + 200, now - 10, now - 1).unwrap();
        let db = DbHandle::from_db(db);

        let polls = db.call(move |db| store::ari_polls(db, now)).unwrap();
        assert_eq!(
            polls.len(),
            1,
            "an active ACME certificate in ARI use is polled"
        );
        refresh_renewal_info(&db, &cipher, &polls[0]).await;

        let row = db
            .call(move |db| store::get_certificate(db, cert_id))
            .unwrap()
            .unwrap();
        assert_eq!(row.ari_window_start, Some(now + 100));
        assert_eq!(row.ari_window_end, Some(now + 200));
        let polls = db.call(move |db| store::ari_polls(db, now)).unwrap();
        assert!(polls[0].next_poll_at >= now + 6 * 3600 - 5);
    }

    // spec: TLS#renewal
    #[test]
    fn replaces_names_only_a_certificate_the_same_account_issued() {
        let pem = ca_signed_leaf();
        let previous = |acme_account_id| PreviousCert {
            chain_pem: &pem,
            acme_account_id,
        };
        assert!(replaces_field(Some(&previous(Some(7))), 7).is_some());
        assert!(replaces_field(Some(&previous(Some(8))), 7).is_none());
        assert!(replaces_field(Some(&previous(None)), 7).is_none());
        assert!(replaces_field(None, 7).is_none());
    }
}
