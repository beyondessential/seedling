//! The one rule for deciding which stored certificate answers for a hostname.
//!
//! Both matchers — the serving lookup in [`super::store`] and the
//! control-plane one in [`super::state`] — rank candidates through here, so
//! the rule cannot be narrowed on one side and not the other. Each supplies
//! its own pre-filter (they disagree about expiry, deliberately) and its own
//! way of reading rows; the decision itself lives here.

use super::parse;

/// Where a certificate ranks as the answer for a hostname. Higher is better;
/// the field order is the tie-break order, and [`Ord`] is derived to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rank {
    /// An exact SAN entry beats a wildcard that merely covers the name. RFC
    /// 6125 §6.4.4 gives the exact match precedence, and without it a broader
    /// certificate uploaded later shadows the one issued for this hostname.
    exact: bool,
    /// A certificate clients accept beats one they will not. Resolution and
    /// supersession have to agree here: supersession refuses to retire a
    /// CA-issued certificate in favour of a self-signed one, and if resolution
    /// then served the self-signed one anyway the refusal would buy nothing.
    trusted: bool,
    created_at: i64,
    id: i64,
}

/// Rank a certificate's SAN set against `hostname`, or `None` if it does not
/// cover it.
// r[impl tls.strategy.manual]
// r[impl tls.cert.validation.san-coverage]
pub fn rank(
    sans: &[String],
    hostname: &str,
    self_signed: bool,
    created_at: i64,
    id: i64,
) -> Option<Rank> {
    if !parse::san_covers(sans, hostname) {
        return None;
    }
    let host_lc = hostname.to_ascii_lowercase();
    Some(Rank {
        exact: sans.iter().any(|s| s.to_ascii_lowercase() == host_lc),
        trusted: !self_signed,
        created_at,
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sans(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    // r[verify tls.strategy.manual]
    #[test]
    fn an_exact_san_outranks_a_newer_wildcard() {
        let exact = rank(
            &sans(&["auto.example.com"]),
            "auto.example.com",
            false,
            100,
            1,
        )
        .unwrap();
        let wildcard = rank(&sans(&["*.example.com"]), "auto.example.com", false, 200, 2).unwrap();
        assert!(
            exact > wildcard,
            "a cert issued for the name is not shadowed"
        );
    }

    // r[verify tls.strategy.manual]
    #[test]
    fn a_trusted_cert_outranks_a_newer_self_signed_one() {
        let trusted = rank(
            &sans(&["auto.example.com"]),
            "auto.example.com",
            false,
            100,
            1,
        )
        .unwrap();
        let ss = rank(
            &sans(&["auto.example.com"]),
            "auto.example.com",
            true,
            200,
            2,
        )
        .unwrap();
        assert!(trusted > ss);
    }

    #[test]
    fn otherwise_the_newest_wins() {
        let older = rank(&sans(&["a.example.com"]), "a.example.com", false, 100, 1).unwrap();
        let newer = rank(&sans(&["a.example.com"]), "a.example.com", false, 200, 2).unwrap();
        assert!(newer > older);
    }

    #[test]
    fn a_cert_that_does_not_cover_the_name_does_not_rank() {
        assert!(rank(&sans(&["other.example.com"]), "a.example.com", false, 1, 1).is_none());
    }
}
