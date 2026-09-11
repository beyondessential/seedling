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
    /// A certificate still inside its validity window beats an expired one.
    /// The serving lookup filters expired rows out before ranking, so this is
    /// a no-op there — but the control-plane matcher deliberately keeps them,
    /// because the renewal scheduler has to see an expiring certificate.
    /// Without this rank an expired certificate naming the hostname exactly
    /// would outrank the newer, valid wildcard that is actually being served,
    /// and the two sides would disagree about what answers for the hostname.
    unexpired: bool,
    /// A certificate clients accept beats one they will not, and it ranks
    /// above specificity: a certificate clients reject is no use for the
    /// hostname however precisely it names it. Resolution and supersession
    /// have to agree here — supersession refuses to retire a CA-issued
    /// certificate in favour of a self-signed one, and if resolution then
    /// served the self-signed one anyway the refusal would buy nothing.
    trusted: bool,
    /// Among certificates that are equally servable, an exact SAN entry beats
    /// a wildcard that merely covers the name (RFC 6125 §6.4.4 gives the exact
    /// match precedence), so a broad certificate arriving later does not
    /// displace one issued for this hostname while both work. It ranks *below*
    /// trust so that a wildcard clients accept still takes over from a
    /// dedicated certificate they do not.
    exact: bool,
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
    not_after: Option<i64>,
    created_at: i64,
    id: i64,
    now: i64,
) -> Option<Rank> {
    if !parse::san_covers(sans, hostname) {
        return None;
    }
    // One lowercase pass over the hostname rather than one per SAN entry:
    // ranking runs once per active certificate per lookup.
    let host_lc = hostname.to_ascii_lowercase();
    Some(Rank {
        unexpired: !not_after.is_some_and(|na| na <= now),
        trusted: !self_signed,
        exact: sans.iter().any(|s| s.eq_ignore_ascii_case(&host_lc)),
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
            None,
            100,
            1,
            0,
        )
        .unwrap();
        let wildcard = rank(
            &sans(&["*.example.com"]),
            "auto.example.com",
            false,
            None,
            200,
            2,
            0,
        )
        .unwrap();
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
            None,
            100,
            1,
            0,
        )
        .unwrap();
        let ss = rank(
            &sans(&["auto.example.com"]),
            "auto.example.com",
            true,
            None,
            200,
            2,
            0,
        )
        .unwrap();
        assert!(trusted > ss);
    }

    /// Trust ranks above specificity: a dedicated certificate clients reject
    /// must not hold a hostname against a wildcard they accept.
    // r[verify tls.strategy.manual]
    #[test]
    fn a_trusted_wildcard_outranks_a_self_signed_exact_match() {
        let wildcard = rank(&sans(&["*.doma.in"]), "b.doma.in", false, None, 100, 1, 0).unwrap();
        let self_signed_exact =
            rank(&sans(&["b.doma.in"]), "b.doma.in", true, None, 200, 2, 0).unwrap();
        assert!(wildcard > self_signed_exact);
    }

    #[test]
    fn otherwise_the_newest_wins() {
        let older = rank(
            &sans(&["a.example.com"]),
            "a.example.com",
            false,
            None,
            100,
            1,
            0,
        )
        .unwrap();
        let newer = rank(
            &sans(&["a.example.com"]),
            "a.example.com",
            false,
            None,
            200,
            2,
            0,
        )
        .unwrap();
        assert!(newer > older);
    }

    /// The control-plane matcher keeps expired rows so the renewal scheduler
    /// can see them, which would otherwise let an expired exact match outrank
    /// the valid wildcard that is really being served — and the rollup would
    /// file an expiry fault for a hostname that is correctly covered.
    // r[verify tls.strategy.manual]
    #[test]
    fn a_valid_wildcard_outranks_an_expired_exact_match() {
        let now = 1_000;
        let expired = rank(
            &sans(&["foo.example.com"]),
            "foo.example.com",
            false,
            Some(now - 1),
            200,
            2,
            now,
        )
        .unwrap();
        let valid = rank(
            &sans(&["*.example.com"]),
            "foo.example.com",
            false,
            Some(now + 86_400),
            100,
            1,
            now,
        )
        .unwrap();
        assert!(valid > expired);
    }

    #[test]
    fn a_cert_that_does_not_cover_the_name_does_not_rank() {
        assert!(
            rank(
                &sans(&["other.example.com"]),
                "a.example.com",
                false,
                None,
                1,
                1,
                0
            )
            .is_none()
        );
    }
}
