//! The SAN index: which stored certificates could cover a hostname, found
//! without parsing any of them.
//!
//! Every certificate with a PEM has an index row recording whether its leaf is
//! self-issued, and one row per SAN DNS name it carries, lowercased. A hostname's
//! candidates are the certificates naming one of [`parse::covering_sans`], so
//! resolving a hostname costs what the certificates that could cover it cost,
//! not what the whole store does.
//!
//! **An index, never a source of truth.** A claim about a certificate stored
//! beside it and trusted rather than confirmed is how certificates were once
//! served for names they did not carry. Whatever the index selects is confirmed
//! against the certificate's own PEM before it is acted on, and a row the PEM
//! contradicts is rewritten from the PEM.
//!
//! Confirmation catches a row that says too much. It cannot catch one that is
//! missing: a certificate the index does not list for a hostname is never a
//! candidate for it, and finding that out would mean the full scan the index
//! exists to avoid. So the index is written in the same transaction as the
//! certificate, a trigger drops a certificate's index rows when its PEM
//! changes, and a background [sweep](spawn) indexes what is unindexed and, less
//! often, rebuilds every row from its PEM.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, params};
use snafu::{ResultExt, Snafu};

use super::{TlsCertState, TlsCertificate, parse};
use crate::runtime::db::{Db, DbHandle};

/// Index certificate `id` from `pem`, replacing whatever was indexed for it.
///
/// For the paths that store a certificate. A PEM that cannot be parsed is an
/// error rather than a certificate covering nothing: what the certificate
/// covers could not be determined, and storing it unindexed would hide it from
/// every lookup with nothing said.
// r[impl tls.cert.serve]
pub fn index_stored(conn: &Connection, id: i64, pem: &str) -> rusqlite::Result<()> {
    let facts = parse::leaf_facts(pem).map_err(|e| {
        super::store::unreadable(format!(
            "certificate {id} could not be parsed for the SAN index: {e}"
        ))
    })?;
    write(conn, id, &Entry::of(Some(&facts)))
}

/// Record `entry` as what certificate `id` covers, replacing whatever was
/// indexed for it.
fn write(conn: &Connection, id: i64, entry: &Entry) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM tls_cert_index WHERE cert_id = ?1", [id])?;
    conn.execute(
        "INSERT INTO tls_cert_index (cert_id, self_issued) VALUES (?1, ?2)",
        params![id, entry.self_issued],
    )?;
    let mut insert =
        conn.prepare_cached("INSERT INTO tls_cert_sans (cert_id, san) VALUES (?1, ?2)")?;
    for san in &entry.sans {
        insert.execute(params![id, san])?;
    }
    Ok(())
}

/// Rewrite certificate `id`'s index from `facts`, after confirming its PEM
/// contradicted what the index said.
///
/// The caller goes on to the next candidate whether or not this works, so a
/// failure is reported here and not returned.
pub(super) fn repair(db: &Db, id: i64, facts: Option<&parse::LeafFacts>) {
    if let Err(e) = super::store::atomically(db, || write(&db.conn, id, &Entry::of(facts))) {
        tracing::error!(
            cert_id = id,
            error = %e,
            "could not rewrite a certificate's SAN index entry from its PEM"
        );
    }
}

/// What the index says about one certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    /// `None` when the PEM could not be parsed.
    pub self_issued: Option<bool>,
    pub sans: BTreeSet<String>,
}

impl Entry {
    fn of(facts: Option<&parse::LeafFacts>) -> Self {
        Self {
            self_issued: facts.map(|f| f.self_issued),
            sans: facts
                .iter()
                .flat_map(|f| &f.san_dns_names)
                .map(|s| s.to_ascii_lowercase())
                .collect(),
        }
    }
}

/// The index entries of a set of certificates, held in memory for a
/// [`super::state::Snapshot`] to resolve many hostnames against.
#[derive(Debug, Clone, Default)]
pub struct SanIndex {
    entries: BTreeMap<i64, Entry>,
    by_san: HashMap<String, Vec<i64>>,
}

impl SanIndex {
    /// The index entries of every active certificate.
    ///
    /// An active certificate with a PEM and no entry is parsed here, so the
    /// snapshot's answer does not wait on the sweep: the snapshot reads every
    /// certificate row anyway, and finding the unindexed ones among them costs
    /// no parse when there are none.
    pub fn load_active(db: &Db, certificates: &[TlsCertificate]) -> rusqlite::Result<Self> {
        let mut entries: BTreeMap<i64, Entry> = BTreeMap::new();
        {
            let mut stmt = db.conn.prepare_cached(
                "SELECT i.cert_id, i.self_issued, s.san
                 FROM tls_cert_index i
                 JOIN tls_certificates c ON c.id = i.cert_id
                 LEFT JOIN tls_cert_sans s ON s.cert_id = i.cert_id
                 WHERE c.state = 'active'",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let self_issued: Option<bool> = row.get(1)?;
                let san: Option<String> = row.get(2)?;
                let entry = entries.entry(id).or_insert_with(|| Entry {
                    self_issued,
                    sans: BTreeSet::new(),
                });
                entry.sans.extend(san);
            }
        }

        let unindexed: Vec<&TlsCertificate> = certificates
            .iter()
            .filter(|c| c.state == TlsCertState::Active && !entries.contains_key(&c.id))
            .collect();
        for cert in unindexed {
            let Some(pem) = cert.cert_pem.as_deref() else {
                tracing::warn!(
                    cert_id = cert.id,
                    "active certificate has no stored PEM; it can never be served"
                );
                continue;
            };
            tracing::error!(
                cert_id = cert.id,
                "active certificate is missing from the SAN index; parsing it directly"
            );
            entries.insert(cert.id, Entry::of(parse_reporting(cert.id, pem).as_ref()));
        }

        for (id, entry) in &entries {
            if entry.self_issued.is_none() {
                tracing::warn!(
                    cert_id = id,
                    "stored certificate could not be parsed; it can never be served"
                );
            }
        }

        Ok(Self::from_entries(entries))
    }

    /// The index of `certificates` built by parsing each, as a sweep would.
    #[cfg(test)]
    pub fn parsed(certificates: &[TlsCertificate]) -> Self {
        Self::from_entries(
            certificates
                .iter()
                .filter_map(|c| {
                    let pem = c.cert_pem.as_deref()?;
                    Some((c.id, Entry::of(parse::leaf_facts(pem).ok().as_ref())))
                })
                .collect(),
        )
    }

    fn from_entries(entries: BTreeMap<i64, Entry>) -> Self {
        let mut by_san: HashMap<String, Vec<i64>> = HashMap::new();
        for (id, entry) in &entries {
            for san in &entry.sans {
                by_san.entry(san.clone()).or_default().push(*id);
            }
        }
        Self { entries, by_san }
    }

    /// The certificates the index says could cover `hostname`, each with the
    /// SAN entries that matched and whether it is self-issued.
    pub fn candidates(&self, hostname: &str) -> Vec<Candidate> {
        let mut matched: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        for key in parse::covering_sans(hostname) {
            for id in self.by_san.get(&key).into_iter().flatten() {
                matched.entry(*id).or_default().push(key.clone());
            }
        }
        matched
            .into_iter()
            .filter_map(|(id, sans)| {
                let self_issued = self.entries.get(&id)?.self_issued?;
                Some(Candidate {
                    id,
                    sans,
                    self_issued,
                })
            })
            .collect()
    }
}

/// A certificate the index says could cover a hostname.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: i64,
    /// The certificate's SAN entries that cover the hostname, which is all
    /// ranking needs of its SAN set.
    pub sans: Vec<String>,
    pub self_issued: bool,
}

/// The candidates for `hostname` among active certificates inside their
/// validity window at `now`, with what ranking needs of each row.
pub(super) fn servable_candidates(
    db: &Db,
    hostname: &str,
    now: i64,
) -> rusqlite::Result<Vec<ServableCandidate>> {
    let keys = parse::covering_sans(hostname);
    let wildcard = keys.get(1).unwrap_or(&keys[0]);
    let mut stmt = db.conn.prepare_cached(
        "SELECT c.id, c.created_at, c.not_after, i.self_issued, s.san
         FROM tls_cert_sans s
         JOIN tls_cert_index i ON i.cert_id = s.cert_id
         JOIN tls_certificates c ON c.id = s.cert_id
         WHERE s.san IN (?1, ?2)
           AND i.self_issued IS NOT NULL
           AND c.state = 'active'
           AND (c.not_before IS NULL OR c.not_before <= ?3)
           AND (c.not_after IS NULL OR c.not_after > ?3)",
    )?;
    let mut rows = stmt.query(params![keys[0], wildcard, now])?;
    let mut by_id: BTreeMap<i64, ServableCandidate> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let san: String = row.get(4)?;
        match by_id.get_mut(&id) {
            Some(candidate) => candidate.sans.push(san),
            None => {
                by_id.insert(
                    id,
                    ServableCandidate {
                        id,
                        created_at: row.get(1)?,
                        not_after: row.get(2)?,
                        self_issued: row.get(3)?,
                        sans: vec![san],
                    },
                );
            }
        }
    }
    Ok(by_id.into_values().collect())
}

/// A candidate for the serving lookup, as read from the index.
#[derive(Debug, Clone)]
pub(super) struct ServableCandidate {
    pub id: i64,
    pub created_at: i64,
    pub not_after: Option<i64>,
    pub self_issued: bool,
    pub sans: Vec<String>,
}

/// Parse `pem`, reporting a failure rather than returning it: an unreadable
/// certificate is indexed as covering nothing.
fn parse_reporting(id: i64, pem: &str) -> Option<parse::LeafFacts> {
    match parse::leaf_facts(pem) {
        Ok(facts) => Some(facts),
        Err(e) => {
            tracing::warn!(
                cert_id = id,
                error = %e,
                "stored certificate could not be parsed; indexing it as covering nothing"
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Sweeps
// ---------------------------------------------------------------------------

/// How often the sweep for unindexed certificates runs. It parses nothing
/// unless it finds one, so it can run often.
pub const UNINDEXED_SWEEP_PERIOD: Duration = Duration::from_secs(5 * 60);

/// How often every certificate's index entry is rebuilt from its PEM.
pub const FULL_SWEEP_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);

/// Which certificates a sweep looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sweep {
    /// Certificates with a PEM and no index entry: what a write path that
    /// forgot to index leaves, and what the trigger leaves when a PEM changes
    /// without being reindexed.
    Unindexed,
    /// Every certificate with a PEM: catches what the other cannot see, such
    /// as SAN rows removed by hand.
    All,
}

/// Bring the index entries of the certificates `sweep` covers into line with
/// their PEMs, returning how many were rewritten.
///
/// The PEMs are read in one database call, parsed off the database thread,
/// and written back in a second call, so the database thread is never held for
/// a parse. A certificate whose PEM changed in between is left alone: whatever
/// changed it indexed it, or the trigger left it for the next sweep.
///
/// Anything rewritten means a write path let the index drift, which is a
/// defect, so each one is logged as an error. It is not filed as a fault: an
/// operator can do nothing about it, and the sweep has already repaired it.
pub async fn sweep(db: &DbHandle, sweep: Sweep) -> Result<usize, SweepError> {
    let read = db
        .call(move |db| read_for_sweep(db, sweep))
        .context(StorageSnafu)?;
    if read.is_empty() {
        return Ok(0);
    }
    let derived = tokio::task::spawn_blocking(move || {
        read.into_iter()
            .map(|(id, pem)| {
                let entry = Entry::of(parse_reporting(id, &pem).as_ref());
                (id, pem, entry)
            })
            .collect::<Vec<_>>()
    })
    .await
    .context(ParseSnafu)?;
    db.call(move |db| super::store::atomically(db, || write_back(db, derived)))
        .context(StorageSnafu)
}

fn read_for_sweep(db: &Db, sweep: Sweep) -> rusqlite::Result<Vec<(i64, String)>> {
    let sql = match sweep {
        Sweep::Unindexed => {
            "SELECT c.id, c.cert_pem FROM tls_certificates c
             WHERE c.cert_pem IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM tls_cert_index i WHERE i.cert_id = c.id)"
        }
        Sweep::All => "SELECT id, cert_pem FROM tls_certificates WHERE cert_pem IS NOT NULL",
    };
    let mut stmt = db.conn.prepare_cached(sql)?;
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

fn write_back(db: &Db, derived: Vec<(i64, String, Entry)>) -> rusqlite::Result<usize> {
    let mut rewritten = 0;
    for (id, pem, entry) in derived {
        let current: Option<Option<String>> = db
            .conn
            .query_row(
                "SELECT cert_pem FROM tls_certificates WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if current.flatten().as_deref() != Some(pem.as_str()) {
            continue;
        }
        let indexed = read_entry(db, id)?;
        if indexed.as_ref() == Some(&entry) {
            continue;
        }
        tracing::error!(
            cert_id = id,
            indexed = ?indexed,
            from_pem = ?entry,
            "certificate's SAN index entry did not match its PEM; rewriting it"
        );
        write(&db.conn, id, &entry)?;
        rewritten += 1;
    }
    Ok(rewritten)
}

fn read_entry(db: &Db, id: i64) -> rusqlite::Result<Option<Entry>> {
    let Some(self_issued) = db
        .conn
        .query_row(
            "SELECT self_issued FROM tls_cert_index WHERE cert_id = ?1",
            [id],
            |row| row.get::<_, Option<bool>>(0),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut stmt = db
        .conn
        .prepare_cached("SELECT san FROM tls_cert_sans WHERE cert_id = ?1")?;
    let sans = stmt
        .query_map([id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(Entry { self_issued, sans }))
}

#[derive(Debug, Snafu)]
pub enum SweepError {
    #[snafu(display("storage error: {source}"))]
    Storage { source: rusqlite::Error },

    #[snafu(display("parsing task failed: {source}"))]
    Parse { source: tokio::task::JoinError },
}

/// Spawn the background sweeps on the current Tokio runtime.
///
/// The unindexed sweep runs at once and then every [`UNINDEXED_SWEEP_PERIOD`];
/// the full sweep every [`FULL_SWEEP_PERIOD`]. The task runs for the life of the
/// daemon. A failed pass, whether the database refused or the parsing task
/// died, is logged and the next pass runs at its usual time: the interval is the
/// back-off, nothing is fatal, and nothing ends the task.
pub fn spawn(db: DbHandle) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut unindexed = tokio::time::interval(UNINDEXED_SWEEP_PERIOD);
        let mut full = tokio::time::interval(FULL_SWEEP_PERIOD);
        // The full sweep's first tick would otherwise fire at once, alongside
        // the unindexed sweep's, and repeat work just done.
        full.tick().await;
        unindexed.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        full.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let which = tokio::select! {
                _ = unindexed.tick() => Sweep::Unindexed,
                _ = full.tick() => Sweep::All,
            };
            match sweep(&db, which).await {
                Ok(0) => {}
                Ok(n) => tracing::warn!(sweep = ?which, rewritten = n, "tls: SAN index repaired"),
                Err(e) => tracing::warn!(sweep = ?which, error = %e, "tls: SAN index sweep failed"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tls::{
        KeyType, TlsCertOrigin,
        store::{self, CertMetadata, NewCertificate},
        test_support::self_signed_pem,
    };

    fn now() -> i64 {
        jiff::Timestamp::now().as_second()
    }

    fn insert_pem(db: &Db, pem: &str) -> rusqlite::Result<i64> {
        store::insert_certificate(
            db,
            NewCertificate {
                hostname: "label.example.com",
                requested_hostnames: None,
                state: TlsCertState::Active,
                origin: TlsCertOrigin::Manual,
                cert_pem: Some(pem),
                csr_pem: None,
                key_ciphertext: b"key",
                key_type: KeyType::EcdsaP256,
                metadata: CertMetadata {
                    not_before: Some(now() - 86_400),
                    not_after: Some(now() + 86_400),
                    ..Default::default()
                },
                note: None,
                acme_account_id: None,
                renews: None,
            },
        )
    }

    fn insert(db: &Db, sans: &[&str]) -> i64 {
        insert_pem(db, &self_signed_pem(sans)).unwrap()
    }

    fn indexed_sans(db: &Db, id: i64) -> Option<BTreeSet<String>> {
        read_entry(db, id).unwrap().map(|e| e.sans)
    }

    fn served(db: &Db, hostname: &str) -> Option<i64> {
        store::find_active_for_hostname(db, hostname)
            .unwrap()
            .map(|c| c.id)
    }

    /// The spec bounds a lookup by the certificates that could cover the
    /// hostname: one nothing names has no candidates to parse, however many
    /// certificates are stored.
    // r[verify tls.cert.serve]
    #[test]
    fn a_hostname_no_certificate_names_has_no_candidates() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..20 {
            insert(&db, &[&format!("host{i}.example.com")]);
        }
        assert!(
            servable_candidates(&db, "unknown.example.org", now())
                .unwrap()
                .is_empty()
        );
        assert!(served(&db, "unknown.example.org").is_none());
    }

    // r[verify tls.cert.serve]
    // r[verify tls.cert.validation.san-coverage]
    #[test]
    fn a_wildcard_is_found_for_a_name_one_label_below_it_only() {
        let db = Db::open_in_memory().unwrap();
        let wildcard = insert(&db, &["*.Example.com"]);
        assert_eq!(served(&db, "shop.example.com"), Some(wildcard));
        assert_eq!(served(&db, "SHOP.example.com"), Some(wildcard));
        assert_eq!(served(&db, "example.com"), None);
        assert_eq!(served(&db, "a.shop.example.com"), None);
    }

    /// History stays in the index; the join to the certificate's state is what
    /// keeps a superseded certificate from being served.
    // r[verify tls.cert.serve]
    #[test]
    fn a_superseded_certificate_is_not_served() {
        let db = Db::open_in_memory().unwrap();
        let old = insert(&db, &["example.com"]);
        let new = insert(&db, &["example.com"]);
        assert_eq!(
            store::supersede_other_active_for_hostname(&db, new).unwrap(),
            1
        );
        assert!(indexed_sans(&db, old).is_some());
        assert_eq!(served(&db, "example.com"), Some(new));
    }

    #[test]
    fn deleting_a_certificate_removes_its_index_entry() {
        let db = Db::open_in_memory().unwrap();
        let id = insert(&db, &["example.com", "www.example.com"]);
        assert!(store::delete_certificate(&db, id).unwrap());
        let rows: i64 = db
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM tls_cert_index) + (SELECT COUNT(*) FROM tls_cert_sans)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0);
    }

    /// What a certificate covers could not be determined, which is not the
    /// same as covering nothing, so the certificate is not stored at all.
    #[test]
    fn a_certificate_whose_pem_will_not_parse_is_not_stored() {
        let db = Db::open_in_memory().unwrap();
        assert!(insert_pem(&db, "not a certificate").is_err());
        let rows: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tls_certificates", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    /// An index row claiming a name the certificate does not carry is caught
    /// by confirming against the PEM, rewritten from it, and passed over for
    /// the next candidate.
    // r[verify tls.cert.serve]
    // r[verify tls.cert.validation.san-coverage]
    #[test]
    fn an_index_entry_the_pem_contradicts_is_rewritten_and_passed_over() {
        let db = Db::open_in_memory().unwrap();
        let covering = insert(&db, &["example.com"]);
        let other = insert(&db, &["other.example.com"]);
        db.conn
            .execute(
                "INSERT INTO tls_cert_sans (cert_id, san) VALUES (?1, 'example.com')",
                [other],
            )
            .unwrap();

        assert_eq!(served(&db, "example.com"), Some(covering));
        assert_eq!(
            indexed_sans(&db, other).unwrap(),
            BTreeSet::from(["other.example.com".to_owned()]),
        );
    }

    /// A PEM changed without reindexing leaves the certificate unindexed, not
    /// stale, and the unindexed sweep picks it up.
    #[tokio::test]
    async fn a_pem_changed_behind_the_index_is_reindexed_by_the_sweep() {
        let db = DbHandle::open_in_memory().unwrap();
        let id = db.call(|db| insert(db, &["old.example.com"]));
        let replacement = self_signed_pem(&["new.example.com"]);
        db.call(move |db| {
            db.conn
                .execute(
                    "UPDATE tls_certificates SET cert_pem = ?1 WHERE id = ?2",
                    params![replacement, id],
                )
                .unwrap();
            assert!(read_entry(db, id).unwrap().is_none());
        });

        assert_eq!(sweep(&db, Sweep::Unindexed).await.unwrap(), 1);
        assert_eq!(db.call(|db| served(db, "new.example.com")), Some(id));
        assert_eq!(db.call(|db| served(db, "old.example.com")), None);
        assert_eq!(sweep(&db, Sweep::Unindexed).await.unwrap(), 0);
    }

    /// SAN rows removed by hand leave the index row in place, which only the
    /// full sweep can see.
    #[tokio::test]
    async fn san_rows_removed_by_hand_are_restored_by_the_full_sweep() {
        let db = DbHandle::open_in_memory().unwrap();
        let id = db.call(|db| insert(db, &["example.com"]));
        db.call(|db| db.conn.execute("DELETE FROM tls_cert_sans", []).unwrap());

        assert_eq!(sweep(&db, Sweep::Unindexed).await.unwrap(), 0);
        assert_eq!(db.call(|db| served(db, "example.com")), None);
        assert_eq!(sweep(&db, Sweep::All).await.unwrap(), 1);
        assert_eq!(db.call(|db| served(db, "example.com")), Some(id));
        assert_eq!(sweep(&db, Sweep::All).await.unwrap(), 0);
    }

    /// A certificate whose PEM changed between the sweep reading it and
    /// writing back keeps whatever indexed the change.
    #[test]
    fn a_sweep_does_not_overwrite_a_certificate_changed_since_it_read_it() {
        let db = Db::open_in_memory().unwrap();
        let stale_pem = self_signed_pem(&["old.example.com"]);
        let id = insert_pem(&db, &self_signed_pem(&["new.example.com"])).unwrap();
        let stale = Entry::of(parse::leaf_facts(&stale_pem).ok().as_ref());

        assert_eq!(write_back(&db, vec![(id, stale_pem, stale)]).unwrap(), 0);
        assert_eq!(
            indexed_sans(&db, id).unwrap(),
            BTreeSet::from(["new.example.com".to_owned()]),
        );
    }
}
