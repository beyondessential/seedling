//! Strategy chains: which strategy is responsible for a hostname's
//! certificate, and when responsibility moves on.
//!
//! Pure over its inputs: no database, no clock. [`super::state`] gathers the
//! facts and calls [`resolve`]; nothing else decides where a hostname has got
//! to in its chain.
//!
//! A chain runs in cycles. A cycle starts when a hostname has no usable
//! certificate or the one serving it reaches its renewal point, and within a
//! cycle each strategy gets half of the time left before the certificate being
//! replaced expires. The position the clock implies is computed here from the
//! certificate and the cycle's recorded start, never stored, so a restart
//! resumes where it was. Only what the clock cannot say is recorded (a
//! refusal, a revocation, an open order): see [`Row`].

/// One way of obtaining a hostname's certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// A certificate requested from Canopy over the relay.
    // r[impl tls.strategy.canopy]
    Canopy,
    /// A public certificate the proxy obtains over HTTP-01.
    Http01,
    /// The proxy's own internal CA, for names no public CA can issue for.
    InternalCa,
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Canopy => "canopy",
            Self::Http01 => "acme_http01",
            Self::InternalCa => "internal_ca",
        }
    }

    /// Whether the proxy, not the runtime, carries this strategy out.
    pub fn is_proxy(self) -> bool {
        matches!(self, Self::Http01 | Self::InternalCa)
    }
}

/// The chain of a hostname bound to the Canopy-only policy.
// r[impl tls.strategy.canopy-only]
pub const CANOPY_ONLY: [Strategy; 1] = [Strategy::Canopy];

/// The chain of a hostname with no operator policy.
///
/// `canopy_applies` is whether Canopy can be asked about the hostname at all:
/// a name a public CA can issue for, declared by at least one app with a
/// Canopy type. It decides whether Canopy is in the chain, since a hostname
/// Canopy cannot be asked about must not wait out a Canopy window before the
/// proxy gets to renew it.
// r[impl tls.strategy.default]
pub fn default_chain(internal_name: bool, canopy_applies: bool) -> Vec<Strategy> {
    if internal_name {
        vec![Strategy::InternalCa]
    } else if canopy_applies {
        vec![Strategy::Canopy, Strategy::Http01]
    } else {
        vec![Strategy::Http01]
    }
}

/// Which certificate a cycle is replacing.
///
/// A certificate the runtime stores is named by its id; one the proxy holds
/// has none, and is told apart from another by its expiry. Both `None` means
/// there was no certificate to replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub cert_id: Option<i64>,
    pub not_after: Option<i64>,
}

impl Identity {
    pub const NONE: Self = Self {
        cert_id: None,
        not_after: None,
    };

    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }
}

/// The certificate currently serving a hostname, if it is usable: inside its
/// validity window and not revoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Serving {
    pub identity: Identity,
    /// When a cycle starts for it: the start of the CA's suggested window when
    /// it supplied one, otherwise when a third of its lifetime remains.
    // r[impl tls.chain.cycle]
    pub renewal_point: i64,
}

/// What is recorded for a hostname, for the cases the clock cannot answer.
///
/// A row whose `identity` is not the certificate being replaced belongs to an
/// earlier cycle and is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub identity: Identity,
    /// The chain index of the strategy responsible as of `since`.
    pub step: usize,
    /// When that strategy became responsible.
    pub since: i64,
    /// When an order with the responsible strategy was opened, if one is.
    pub open_order_since: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct Inputs<'a> {
    pub chain: &'a [Strategy],
    pub now: i64,
    pub serving: Option<Serving>,
    pub row: Option<&'a Row>,
    /// Whether a Canopy offer is live, which decides whether a hostname with
    /// nothing to replace skips Canopy.
    pub offer_live: bool,
    /// An operator retry is queued: a cycle starts now whatever the certificate
    /// says.
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cycle {
    /// A usable certificate that has not reached its renewal point.
    Idle { next_cycle_at: i64 },
    Running(Running),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub identity: Identity,
    pub step: usize,
    pub responsible: Strategy,
    /// When the responsible strategy became responsible.
    pub since: i64,
    /// When the next strategy takes over, if there is one and the certificate
    /// being replaced has an expiry to measure it from.
    pub fall_through_at: Option<i64>,
    /// An order with the responsible strategy is open.
    pub open_order: bool,
}

/// Where a hostname stands in its chain.
// r[impl tls.chain.cycle]
// r[impl tls.chain.ladder]
// r[impl tls.chain.no-certificate]
// r[impl tls.chain.return]
pub fn resolve(inputs: &Inputs<'_>) -> Cycle {
    let Inputs {
        chain,
        now,
        serving,
        row,
        offer_live,
        force,
    } = *inputs;
    assert!(!chain.is_empty(), "a chain has at least one strategy");
    let last = chain.len() - 1;

    match serving {
        Some(serving) => {
            let row = row.filter(|r| r.identity == serving.identity);
            let (step, since) = match row {
                Some(r) => (r.step.min(last), r.since),
                None if force => (0, now),
                None if now >= serving.renewal_point => (0, serving.renewal_point),
                None => {
                    return Cycle::Idle {
                        next_cycle_at: serving.renewal_point,
                    };
                }
            };
            running(
                chain,
                serving.identity,
                step,
                since,
                now,
                row.is_some_and(|r| r.open_order_since.is_some()),
            )
        }
        None => {
            // A certificate revoked by its issuer is still the one being
            // replaced: its row says so, and the ladder is measured from its
            // expiry for as long as that lies ahead.
            // r[impl tls.canopy.revoked]
            if let Some(r) = row
                && !r.identity.is_none()
                && r.identity.not_after.is_some_and(|e| e > now)
            {
                return running(
                    chain,
                    r.identity,
                    r.step.min(last),
                    r.since,
                    now,
                    r.open_order_since.is_some(),
                );
            }

            let pinned = row.filter(|r| r.identity.is_none() && r.open_order_since.is_some());
            let step = match pinned {
                Some(r) => r.step.min(last),
                None => chain
                    .iter()
                    .position(|s| available(*s, chain.len(), offer_live))
                    .unwrap_or(last),
            };
            Cycle::Running(Running {
                identity: Identity::NONE,
                step,
                responsible: chain[step],
                since: pinned.map_or(now, |r| r.since),
                fall_through_at: None,
                open_order: pinned.is_some(),
            })
        }
    }
}

/// Whether a strategy can be asked right now, for a hostname with nothing to
/// replace. Only Canopy can be unavailable, and not when it is the whole
/// chain: a Canopy-only hostname is never moved on for want of an offer.
fn available(strategy: Strategy, chain_len: usize, offer_live: bool) -> bool {
    match strategy {
        Strategy::Canopy => offer_live || chain_len == 1,
        Strategy::Http01 | Strategy::InternalCa => true,
    }
}

fn running(
    chain: &[Strategy],
    identity: Identity,
    step: usize,
    since: i64,
    now: i64,
    open_order: bool,
) -> Cycle {
    let (step, since) = advance(chain.len(), step, since, identity.not_after, now);
    Cycle::Running(Running {
        identity,
        step,
        responsible: chain[step],
        since,
        fall_through_at: fall_through_at(chain.len(), step, since, identity.not_after),
        open_order,
    })
}

/// When the strategy that became responsible at `since` falls through: halfway
/// between then and the expiry of what is being replaced.
// r[impl tls.chain.ladder]
fn halfway(since: i64, expiry: i64) -> i64 {
    since + (expiry - since).max(0) / 2
}

fn fall_through_at(len: usize, step: usize, since: i64, expiry: Option<i64>) -> Option<i64> {
    if step + 1 >= len {
        return None;
    }
    expiry.map(|e| halfway(since, e))
}

/// Move down the chain for each fall-through point the clock has passed. Each
/// strategy becomes responsible at the point its predecessor fell through, not
/// at the moment this runs, so the answer does not depend on when it is asked.
fn advance(
    len: usize,
    mut step: usize,
    mut since: i64,
    expiry: Option<i64>,
    now: i64,
) -> (usize, i64) {
    let Some(expiry) = expiry else {
        return (step, since);
    };
    while step + 1 < len {
        let at = halfway(since, expiry);
        if at > now {
            break;
        }
        step += 1;
        since = at;
    }
    (step, since)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    fn ident(id: i64, not_after: i64) -> Identity {
        Identity {
            cert_id: Some(id),
            not_after: Some(not_after),
        }
    }

    /// A 90 day certificate issued at 0 whose renewal point is two thirds of
    /// the way through.
    fn serving() -> Serving {
        Serving {
            identity: ident(1, 90 * DAY),
            renewal_point: 60 * DAY,
        }
    }

    fn inputs<'a>(
        chain: &'a [Strategy],
        now: i64,
        serving: Option<Serving>,
        row: Option<&'a Row>,
    ) -> Inputs<'a> {
        Inputs {
            chain,
            now,
            serving,
            row,
            offer_live: true,
            force: false,
        }
    }

    const TWO: [Strategy; 2] = [Strategy::Canopy, Strategy::Http01];

    fn running_of(cycle: Cycle) -> Running {
        match cycle {
            Cycle::Running(r) => r,
            other => panic!("expected a running cycle, got {other:?}"),
        }
    }

    // r[verify tls.chain.cycle]
    #[test]
    fn a_usable_certificate_before_its_renewal_point_is_left_alone() {
        let cycle = resolve(&inputs(&TWO, 59 * DAY, Some(serving()), None));
        assert_eq!(
            cycle,
            Cycle::Idle {
                next_cycle_at: 60 * DAY
            }
        );
    }

    // r[verify tls.chain.cycle]
    // r[verify tls.chain.ladder]
    #[test]
    fn the_first_strategy_is_responsible_from_the_renewal_point() {
        let r = running_of(resolve(&inputs(&TWO, 60 * DAY, Some(serving()), None)));
        assert_eq!(r.responsible, Strategy::Canopy);
        assert_eq!(r.since, 60 * DAY);
        assert_eq!(r.fall_through_at, Some(75 * DAY));
    }

    // r[verify tls.chain.ladder]
    #[test]
    fn the_second_strategy_takes_over_five_sixths_through() {
        let r = running_of(resolve(&inputs(&TWO, 75 * DAY, Some(serving()), None)));
        assert_eq!(r.responsible, Strategy::Http01);
        assert_eq!(r.since, 75 * DAY);
        assert_eq!(r.fall_through_at, None);
    }

    // r[verify tls.chain.ladder]
    #[test]
    fn each_step_halves_what_is_left_for_a_longer_chain() {
        let chain = [Strategy::Canopy, Strategy::Http01, Strategy::InternalCa];
        let at = |now| running_of(resolve(&inputs(&chain, now, Some(serving()), None)));

        let first = at(60 * DAY);
        assert_eq!(first.fall_through_at, Some(75 * DAY));
        let second = at(75 * DAY);
        assert_eq!(second.responsible, Strategy::Http01);
        assert_eq!(second.fall_through_at, Some(82 * DAY + DAY / 2));
        let third = at(83 * DAY);
        assert_eq!(third.responsible, Strategy::InternalCa);
        assert_eq!(third.fall_through_at, None);
    }

    // r[verify tls.chain.ladder]
    #[test]
    fn the_answer_does_not_depend_on_when_it_is_asked() {
        let late = running_of(resolve(&inputs(&TWO, 89 * DAY, Some(serving()), None)));
        assert_eq!(late.responsible, Strategy::Http01);
        assert_eq!(late.since, 75 * DAY, "it became responsible at the point, not when asked");
    }

    // r[verify tls.chain.ladder]
    #[test]
    fn a_suggested_window_moves_the_whole_ladder() {
        let s = Serving {
            identity: ident(1, 90 * DAY),
            renewal_point: 40 * DAY,
        };
        let r = running_of(resolve(&inputs(&TWO, 40 * DAY, Some(s), None)));
        assert_eq!(r.fall_through_at, Some(65 * DAY));
    }

    // r[verify tls.chain.refusal]
    #[test]
    fn a_recorded_refusal_moves_a_hostname_on_ahead_of_the_clock() {
        let row = Row {
            identity: serving().identity,
            step: 1,
            since: 61 * DAY,
            open_order_since: None,
        };
        let r = running_of(resolve(&inputs(&TWO, 62 * DAY, Some(serving()), Some(&row))));
        assert_eq!(r.responsible, Strategy::Http01);
        assert_eq!(r.since, 61 * DAY);
    }

    // r[verify tls.chain.refusal]
    #[test]
    fn a_step_recorded_past_the_end_of_the_chain_is_the_last_strategy() {
        let row = Row {
            identity: serving().identity,
            step: 9,
            since: 61 * DAY,
            open_order_since: None,
        };
        let r = running_of(resolve(&inputs(&TWO, 62 * DAY, Some(serving()), Some(&row))));
        assert_eq!(r.step, 1);
    }

    // r[verify tls.chain.return]
    #[test]
    fn a_row_for_an_earlier_certificate_is_ignored() {
        let row = Row {
            identity: ident(7, 30 * DAY),
            step: 1,
            since: 20 * DAY,
            open_order_since: None,
        };
        let cycle = resolve(&inputs(&TWO, 10 * DAY, Some(serving()), Some(&row)));
        assert_eq!(
            cycle,
            Cycle::Idle {
                next_cycle_at: 60 * DAY
            },
            "a new certificate starts the next cycle from the first strategy"
        );
    }

    // r[verify tls.cert.force-retry]
    // r[verify tls.chain.return]
    #[test]
    fn an_operator_retry_starts_a_cycle_at_once_from_the_first_strategy() {
        let mut i = inputs(&TWO, 10 * DAY, Some(serving()), None);
        i.force = true;
        let r = running_of(resolve(&i));
        assert_eq!(r.responsible, Strategy::Canopy);
        assert_eq!(r.since, 10 * DAY);
        assert_eq!(r.fall_through_at, Some(50 * DAY));
    }

    // r[verify tls.chain.cycle]
    #[test]
    fn a_cycle_started_early_by_a_retry_goes_on_running() {
        let row = Row {
            identity: serving().identity,
            step: 0,
            since: 10 * DAY,
            open_order_since: None,
        };
        let r = running_of(resolve(&inputs(&TWO, 20 * DAY, Some(serving()), Some(&row))));
        assert_eq!(r.responsible, Strategy::Canopy);
    }

    // r[verify tls.chain.no-certificate]
    #[test]
    fn nothing_to_replace_starts_with_the_first_available_strategy() {
        let r = running_of(resolve(&inputs(&TWO, 5, None, None)));
        assert_eq!(r.responsible, Strategy::Canopy);
        assert_eq!(r.fall_through_at, None, "there is no expiry to measure from");
    }

    // r[verify tls.chain.no-certificate]
    #[test]
    fn nothing_to_replace_skips_canopy_at_once_without_a_live_offer() {
        let mut i = inputs(&TWO, 5, None, None);
        i.offer_live = false;
        let r = running_of(resolve(&i));
        assert_eq!(r.responsible, Strategy::Http01);
    }

    // r[verify tls.chain.no-certificate]
    #[test]
    fn nothing_to_replace_stays_with_a_strategy_that_keeps_failing() {
        let row = Row {
            identity: Identity::NONE,
            step: 0,
            since: 1,
            open_order_since: None,
        };
        let r = running_of(resolve(&inputs(&TWO, 10 * DAY, None, Some(&row))));
        assert_eq!(
            r.responsible,
            Strategy::Canopy,
            "no clock moves a hostname with nothing to replace"
        );
    }

    // r[verify tls.chain.no-certificate]
    #[test]
    fn an_open_order_keeps_a_hostname_with_its_strategy_when_the_offer_goes() {
        let row = Row {
            identity: Identity::NONE,
            step: 0,
            since: 1,
            open_order_since: Some(1),
        };
        let mut i = inputs(&TWO, 100, None, Some(&row));
        i.offer_live = false;
        let r = running_of(resolve(&i));
        assert_eq!(r.responsible, Strategy::Canopy);
        assert!(r.open_order);
    }

    // r[verify tls.strategy.canopy-only]
    #[test]
    fn a_canopy_only_chain_is_never_skipped_for_want_of_an_offer() {
        let chain = [Strategy::Canopy];
        let mut i = inputs(&chain, 5, None, None);
        i.offer_live = false;
        let r = running_of(resolve(&i));
        assert_eq!(r.responsible, Strategy::Canopy);
    }

    // r[verify tls.strategy.canopy-only]
    #[test]
    fn a_canopy_only_chain_has_no_fall_through_point() {
        let chain = [Strategy::Canopy];
        let r = running_of(resolve(&inputs(&chain, 89 * DAY, Some(serving()), None)));
        assert_eq!(r.responsible, Strategy::Canopy);
        assert_eq!(r.fall_through_at, None);
    }

    // r[verify tls.canopy.revoked]
    #[test]
    fn a_revoked_certificate_is_still_the_one_being_replaced() {
        let row = Row {
            identity: ident(1, 90 * DAY),
            step: 1,
            since: 30 * DAY,
            open_order_since: None,
        };
        let r = running_of(resolve(&inputs(&TWO, 31 * DAY, None, Some(&row))));
        assert_eq!(r.responsible, Strategy::Http01);
        assert_eq!(r.identity, row.identity);
    }

    // r[verify tls.chain.no-certificate]
    #[test]
    fn a_revoked_certificate_that_has_since_expired_is_nothing_to_replace() {
        let row = Row {
            identity: ident(1, 30 * DAY),
            step: 1,
            since: 20 * DAY,
            open_order_since: None,
        };
        let r = running_of(resolve(&inputs(&TWO, 31 * DAY, None, Some(&row))));
        assert!(r.identity.is_none());
        assert_eq!(r.responsible, Strategy::Canopy);
    }

    // r[verify tls.strategy.default]
    #[test]
    fn the_default_chain_has_canopy_only_where_canopy_applies() {
        assert_eq!(
            default_chain(false, true),
            vec![Strategy::Canopy, Strategy::Http01]
        );
        assert_eq!(default_chain(false, false), vec![Strategy::Http01]);
        assert_eq!(
            default_chain(true, true),
            vec![Strategy::InternalCa],
            "Canopy is never asked for a name no public CA can issue for"
        );
    }
}
