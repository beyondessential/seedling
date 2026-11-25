//! Host facts that may be undetermined when the daemon starts, and are looked
//! for again on each reconciliation iteration until they are determined.
//!
//! A fact is determined once and not changed while the runtime runs: the
//! first determined outcome is the one recorded.

use std::sync::Arc;

use crate::system::BoxFuture;

/// Where the probes' answers come from: the host, through its platform, or a
/// test's stand-in.
pub trait ProbeSource: Send + Sync {
    fn external_nat64(&self) -> BoxFuture<'_, Result<bool, String>>;
    fn ipv4_egress(&self) -> BoxFuture<'_, Result<bool, String>>;
    fn ipv6_egress(&self) -> BoxFuture<'_, Result<bool, String>>;
    fn timezone(&self) -> BoxFuture<'_, Result<String, String>>;
    fn available_threads(&self) -> BoxFuture<'_, Result<i64, String>>;
    fn available_memory(&self) -> BoxFuture<'_, Result<i64, String>>;
    fn node_name(&self) -> BoxFuture<'_, Result<String, String>>;
}

/// A source for hosts with nothing left to look for, whose facts were all
/// determined before probing began.
struct NothingToProbe;

impl ProbeSource for NothingToProbe {
    fn external_nat64(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async { Err("nothing to probe".to_owned()) })
    }

    fn ipv4_egress(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async { Err("nothing to probe".to_owned()) })
    }

    fn ipv6_egress(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async { Err("nothing to probe".to_owned()) })
    }

    fn timezone(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Err("nothing to probe".to_owned()) })
    }

    fn available_threads(&self) -> BoxFuture<'_, Result<i64, String>> {
        Box::pin(async { Err("nothing to probe".to_owned()) })
    }

    fn available_memory(&self) -> BoxFuture<'_, Result<i64, String>> {
        Box::pin(async { Err("nothing to probe".to_owned()) })
    }

    fn node_name(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Err("nothing to probe".to_owned()) })
    }
}

/// One fact: determined, or why it is not yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact<T = bool> {
    Determined(T),
    Undetermined(String),
}

impl<T: Clone> Fact<T> {
    pub fn from_result(result: Result<T, String>) -> Self {
        match result {
            Ok(v) => Self::Determined(v),
            Err(why) => Self::Undetermined(why),
        }
    }

    pub fn determined(&self) -> Option<T> {
        match self {
            Self::Determined(v) => Some(v.clone()),
            Self::Undetermined(_) => None,
        }
    }

    pub fn into_result(self) -> Result<T, String> {
        match self {
            Self::Determined(v) => Ok(v),
            Self::Undetermined(why) => Err(why),
        }
    }

    /// Why the fact is not determined, if it is not.
    pub fn undetermined(&self) -> Option<&str> {
        match self {
            Self::Determined(_) => None,
            Self::Undetermined(why) => Some(why),
        }
    }
}

/// What a round of retries moved.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Moved {
    pub nat64_detection: bool,
    pub ipv4_egress: bool,
    pub ipv6_egress: bool,
    pub timezone: bool,
    pub available_threads: bool,
    pub available_memory: bool,
    pub node_name: bool,
}

impl Moved {
    pub fn any(&self) -> bool {
        self.nat64_detection
            || self.ipv4_egress
            || self.ipv6_egress
            || self.timezone
            || self.available_threads
            || self.available_memory
            || self.node_name
    }
}

/// The host constants that hold a fault of their own kind while any of them
/// is undetermined.
// spec: CONST#determining-host-constants
#[derive(Debug, Clone)]
pub struct HostConstants {
    pub available_threads: Fact<i64>,
    pub available_memory: Fact<i64>,
    pub node_name: Fact<String>,
}

impl HostConstants {
    /// Every constant as `source` gives it now.
    pub async fn detect(source: &dyn ProbeSource) -> Self {
        Self {
            available_threads: Fact::from_result(source.available_threads().await),
            available_memory: Fact::from_result(source.available_memory().await),
            node_name: Fact::from_result(source.node_name().await),
        }
    }

    /// Every constant determined, as a host with nothing to look for has.
    pub fn settled() -> Self {
        Self {
            available_threads: Fact::Determined(1),
            available_memory: Fact::Determined(1 << 30),
            node_name: Fact::Determined(String::new()),
        }
    }

    /// The constants still undetermined, by name, with why.
    pub fn undetermined(&self) -> Vec<(&'static str, &str)> {
        [
            ("AVAILABLE_THREADS", self.available_threads.undetermined()),
            ("AVAILABLE_MEMORY", self.available_memory.undetermined()),
            ("NODE_NAME", self.node_name.undetermined()),
        ]
        .into_iter()
        .filter_map(|(name, why)| why.map(|why| (name, why)))
        .collect()
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
    /// The host's IANA time zone name.
    pub timezone: Fact<String>,
    pub constants: HostConstants,
}

impl HostProbes {
    pub fn new(
        source: Arc<dyn ProbeSource>,
        nat64_detection: Option<Fact>,
        ipv4_egress: Fact,
        ipv6_egress: Fact,
        timezone: Fact<String>,
        constants: HostConstants,
    ) -> Self {
        Self {
            source,
            nat64_detection,
            ipv4_egress,
            ipv6_egress,
            timezone,
            constants,
        }
    }

    /// Facts that are all determined and have nothing to look for.
    pub fn settled(ipv4_egress: bool, ipv6_egress: bool) -> Self {
        Self::new(
            Arc::new(NothingToProbe),
            None,
            Fact::Determined(ipv4_egress),
            Fact::Determined(ipv6_egress),
            Fact::Determined("UTC".to_owned()),
            HostConstants::settled(),
        )
    }

    /// Whether any fact is still undetermined.
    pub fn pending(&self) -> bool {
        matches!(self.nat64_detection, Some(Fact::Undetermined(_)))
            || matches!(self.ipv4_egress, Fact::Undetermined(_))
            || matches!(self.ipv6_egress, Fact::Undetermined(_))
            || matches!(self.timezone, Fact::Undetermined(_))
            || !self.constants.undetermined().is_empty()
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
        if matches!(self.timezone, Fact::Undetermined(_)) {
            let next = Fact::from_result(self.source.timezone().await);
            moved.timezone = next != self.timezone;
            self.timezone = next;
        }
        let constants = &mut self.constants;
        if matches!(constants.available_threads, Fact::Undetermined(_)) {
            let next = Fact::from_result(self.source.available_threads().await);
            moved.available_threads = next != constants.available_threads;
            constants.available_threads = next;
        }
        if matches!(constants.available_memory, Fact::Undetermined(_)) {
            let next = Fact::from_result(self.source.available_memory().await);
            moved.available_memory = next != constants.available_memory;
            constants.available_memory = next;
        }
        if matches!(constants.node_name, Fact::Undetermined(_)) {
            let next = Fact::from_result(self.source.node_name().await);
            moved.node_name = next != constants.node_name;
            constants.node_name = next;
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

    impl Flaky {
        fn answer_with<T>(&self, value: T) -> Result<T, String> {
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
        fn timezone(&self) -> BoxFuture<'_, Result<String, String>> {
            async move {
                if self.answering.load(Ordering::SeqCst) {
                    Ok("Pacific/Auckland".to_owned())
                } else {
                    Err("cannot be read".to_owned())
                }
            }
            .boxed()
        }
        fn available_threads(&self) -> BoxFuture<'_, Result<i64, String>> {
            async move { self.answer_with(4) }.boxed()
        }
        fn available_memory(&self) -> BoxFuture<'_, Result<i64, String>> {
            async move { self.answer_with(1 << 30) }.boxed()
        }
        fn node_name(&self) -> BoxFuture<'_, Result<String, String>> {
            async move { self.answer_with("host".to_owned()) }.boxed()
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
            Fact::Undetermined("no zone".into()),
            HostConstants {
                available_threads: Fact::Undetermined("no answer".into()),
                available_memory: Fact::Undetermined("no answer".into()),
                node_name: Fact::Undetermined("no answer".into()),
            },
        );
        assert!(probes.pending());

        // Still failing: the reason may change, the fact does not.
        probes.retry().await;
        assert!(probes.pending());
        assert_eq!(probes.ipv6_egress.determined(), None);

        source.answering.store(true, Ordering::SeqCst);
        let moved = probes.retry().await;
        assert!(moved.nat64_detection && moved.ipv4_egress && moved.ipv6_egress && moved.timezone);
        assert!(moved.available_threads && moved.available_memory && moved.node_name);
        assert_eq!(
            probes.constants.node_name,
            Fact::Determined("host".to_owned())
        );
        assert!(!probes.pending());
        assert_eq!(probes.ipv6_egress, Fact::Determined(false));
        assert_eq!(probes.ipv4_egress, Fact::Determined(true));
        assert_eq!(
            probes.timezone,
            Fact::Determined("Pacific/Auckland".to_owned())
        );

        // Determined, so not asked again: a different answer is not taken.
        source.answering.store(false, Ordering::SeqCst);
        assert!(!probes.retry().await.any());
        assert_eq!(probes.ipv4_egress, Fact::Determined(true));
    }
}
