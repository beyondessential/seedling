//! Host facts that may be undetermined when the daemon starts, and are looked
//! for again on each reconciliation iteration until they are determined.
//!
//! A fact is determined once and not changed while the runtime runs: the
//! first determined outcome is the one recorded.

use std::sync::Arc;

use crate::system::{BoxFuture, nat64};

/// Where the probes' answers come from: the host, or a test's stand-in.
pub trait ProbeSource: Send + Sync {
    fn external_nat64(&self) -> BoxFuture<'_, Result<bool, String>>;
    fn ipv4_egress(&self) -> BoxFuture<'_, Result<bool, String>>;
    fn ipv6_egress(&self) -> BoxFuture<'_, Result<bool, String>>;
}

/// The host's own resolver and routing tables.
pub struct HostSource;

impl ProbeSource for HostSource {
    fn external_nat64(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(nat64::detect_external_nat64())
    }

    fn ipv4_egress(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(crate::system::netinfo::detect_ipv4_egress())
    }

    fn ipv6_egress(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(crate::system::netinfo::detect_ipv6_egress())
    }
}

/// One fact: determined, or why it is not yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    Determined(bool),
    Undetermined(String),
}

impl Fact {
    pub fn from_result(result: Result<bool, String>) -> Self {
        match result {
            Ok(v) => Self::Determined(v),
            Err(why) => Self::Undetermined(why),
        }
    }

    pub fn determined(&self) -> Option<bool> {
        match self {
            Self::Determined(v) => Some(*v),
            Self::Undetermined(_) => None,
        }
    }
}

/// What a round of retries moved.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Moved {
    pub nat64_detection: bool,
    pub ipv4_egress: bool,
    pub ipv6_egress: bool,
}

impl Moved {
    pub fn any(&self) -> bool {
        self.nat64_detection || self.ipv4_egress || self.ipv6_egress
    }
}

/// The facts the runtime is still looking for, and the ones it has.
pub struct HostProbes {
    source: Arc<dyn ProbeSource>,
    /// Whether the network provides NAT64. Only a question while NAT64 is
    /// being decided by detection; `None` when it never was.
    pub nat64_detection: Option<Fact>,
    pub ipv4_egress: Fact,
    pub ipv6_egress: Fact,
}

impl HostProbes {
    pub fn new(
        source: Arc<dyn ProbeSource>,
        nat64_detection: Option<Fact>,
        ipv4_egress: Fact,
        ipv6_egress: Fact,
    ) -> Self {
        Self {
            source,
            nat64_detection,
            ipv4_egress,
            ipv6_egress,
        }
    }

    /// Facts that are all determined and have nothing to look for.
    pub fn settled(ipv4_egress: bool, ipv6_egress: bool) -> Self {
        Self::new(
            Arc::new(HostSource),
            None,
            Fact::Determined(ipv4_egress),
            Fact::Determined(ipv6_egress),
        )
    }

    /// Whether any fact is still undetermined.
    pub fn pending(&self) -> bool {
        matches!(self.nat64_detection, Some(Fact::Undetermined(_)))
            || matches!(self.ipv4_egress, Fact::Undetermined(_))
            || matches!(self.ipv6_egress, Fact::Undetermined(_))
    }

    /// Look again for every undetermined fact, and say which moved, whether
    /// to determined or to a new reason it is not.
    pub async fn retry(&mut self) -> Moved {
        let mut moved = Moved::default();
        if matches!(self.nat64_detection, Some(Fact::Undetermined(_))) {
            let next = Fact::from_result(self.source.external_nat64().await);
            moved.nat64_detection = next != *self.nat64_detection.as_ref().expect("checked");
            self.nat64_detection = Some(next);
        }
        if matches!(self.ipv4_egress, Fact::Undetermined(_)) {
            let next = Fact::from_result(self.source.ipv4_egress().await);
            moved.ipv4_egress = next != self.ipv4_egress;
            self.ipv4_egress = next;
        }
        if matches!(self.ipv6_egress, Fact::Undetermined(_)) {
            let next = Fact::from_result(self.source.ipv6_egress().await);
            moved.ipv6_egress = next != self.ipv6_egress;
            self.ipv6_egress = next;
        }
        moved
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures_util::FutureExt;

    use super::*;

    /// A source whose probes fail until told to answer, and then say the host
    /// has no NAT64 and IPv6 but IPv4.
    pub(crate) struct Flaky {
        pub answering: AtomicBool,
    }

    impl Flaky {
        pub(crate) fn failing() -> Arc<Self> {
            Arc::new(Self {
                answering: AtomicBool::new(false),
            })
        }

        fn answer(&self, value: bool) -> Result<bool, String> {
            if self.answering.load(Ordering::SeqCst) {
                Ok(value)
            } else {
                Err("cannot be read".to_owned())
            }
        }
    }

    impl ProbeSource for Flaky {
        fn external_nat64(&self) -> BoxFuture<'_, Result<bool, String>> {
            async move { self.answer(false) }.boxed()
        }
        fn ipv4_egress(&self) -> BoxFuture<'_, Result<bool, String>> {
            async move { self.answer(true) }.boxed()
        }
        fn ipv6_egress(&self) -> BoxFuture<'_, Result<bool, String>> {
            async move { self.answer(false) }.boxed()
        }
    }

    // spec: NAT64#ipv6-egress
    #[tokio::test]
    async fn an_undetermined_fact_is_retried_and_the_first_determined_outcome_stays() {
        let source = Flaky::failing();
        let mut probes = HostProbes::new(
            source.clone(),
            Some(Fact::Undetermined("no answer".into())),
            Fact::Undetermined("no routes".into()),
            Fact::Undetermined("no routes".into()),
        );
        assert!(probes.pending());

        // Still failing: the reason may change, the fact does not.
        probes.retry().await;
        assert!(probes.pending());
        assert_eq!(probes.ipv6_egress.determined(), None);

        source.answering.store(true, Ordering::SeqCst);
        let moved = probes.retry().await;
        assert!(moved.nat64_detection && moved.ipv4_egress && moved.ipv6_egress);
        assert!(!probes.pending());
        assert_eq!(probes.ipv6_egress, Fact::Determined(false));
        assert_eq!(probes.ipv4_egress, Fact::Determined(true));

        // Determined, so not asked again: a different answer is not taken.
        source.answering.store(false, Ordering::SeqCst);
        assert!(!probes.retry().await.any());
        assert_eq!(probes.ipv4_egress, Fact::Determined(true));
    }
}
