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
    Account, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt, NewAccount, NewOrder,
    OrderStatus, RetryPolicy,
};
use secrecy::{ExposeSecret, SecretString};
use snafu::{ResultExt, Snafu};

use super::{
    KeyType, TlsCertOrigin, TlsCertState,
    dns::{self, DnsProvider, challenge_record_name},
    keypair, parse,
    store::{self, CertMetadata},
};
use crate::runtime::{db::Db, secrets::Cipher};

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
// r[impl tls.strategy.acme-dns]
pub async fn issue(db: &Db, cipher: &Cipher, params: IssueParams<'_>) -> Result<Issued, AcmeError> {
    let provider_entry = store::get_dns_provider(db, cipher, params.dns_provider_name)
        .context(StorageSnafu)?
        .ok_or_else(|| {
            ProviderNotFoundSnafu {
                name: params.dns_provider_name.to_owned(),
            }
            .build()
        })?;
    let provider = dns::build_provider(&provider_entry).context(DnsSnafu)?;

    let (account, account_id) =
        load_or_create_account(db, cipher, params.directory_url, params.contact_email).await?;

    let identifiers = vec![Identifier::Dns(params.hostname.to_owned())];
    let mut order = account
        .new_order(&NewOrder::new(&identifiers))
        .await
        .context(AcmeSnafu)?;

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

    let cert_id = store::insert_certificate(
        db,
        params.hostname,
        TlsCertState::Active,
        TlsCertOrigin::AcmeDns,
        Some(&parsed.chain_pem),
        None,
        &key_ct,
        KeyType::EcdsaP256,
        metadata,
        None,
        Some(account_id),
    )
    .context(StorageSnafu)?;

    store::supersede_other_active_for_hostname(db, params.hostname, cert_id)
        .context(StorageSnafu)?;

    Ok(Issued { cert_id, not_after })
}

/// Restore an ACME account from the encrypted credentials in the DB, or
/// create a fresh one and persist it. Returns the live `Account` plus the
/// row id for `tls_acme_accounts.id` so callers can stamp issued certs.
// r[impl tls.acme.account.persist]
async fn load_or_create_account(
    db: &Db,
    cipher: &Cipher,
    directory_url: &str,
    contact_email: &str,
) -> Result<(Account, i64), AcmeError> {
    if let Some(row) =
        store::get_acme_account(db, directory_url, contact_email).context(StorageSnafu)?
    {
        let plaintext = store::decrypt_acme_account_key(cipher, &row).context(CipherSnafu)?;
        let creds: instant_acme::AccountCredentials =
            serde_json::from_str(plaintext.expose_secret()).context(AccountSerdeSnafu)?;
        let account = Account::builder()
            .context(AcmeSnafu)?
            .from_credentials(creds)
            .await
            .context(AcmeSnafu)?;
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

    let id = store::insert_acme_account(
        db,
        cipher,
        directory_url,
        contact_email,
        &account_url,
        &creds_secret,
    )
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
