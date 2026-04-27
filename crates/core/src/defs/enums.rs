use rhai::{Dynamic, Map};

#[derive(Debug, Default, Clone, Copy)]
pub enum OnUpdate {
    // l[impl const.on-update.rolling]
    #[default]
    Rolling,
    // l[impl const.on-update.replace]
    Replace,
}

impl OnUpdate {
    pub fn rhai_constant() -> Map {
        let mut map = Map::new();
        map.insert("Rolling".into(), Dynamic::from(Self::Rolling));
        map.insert("Replace".into(), Dynamic::from(Self::Replace));
        map
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub enum OnTerminate {
    // l[impl const.on-terminate.recreate]
    #[default]
    Recreate,
}

impl OnTerminate {
    pub fn rhai_constant() -> Map {
        let mut map = Map::new();
        map.insert("Recreate".into(), Dynamic::from(Self::Recreate));
        map
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub enum OnExit {
    // l[impl const.on-exit.restart]
    #[default]
    Restart,
    // l[impl const.on-exit.terminate]
    Terminate,
    // l[impl const.on-exit.restart-on-failure]
    RestartOnFailure,
}

impl OnExit {
    pub fn rhai_constant() -> Map {
        let mut map = Map::new();
        map.insert("Restart".into(), Dynamic::from(Self::Restart));
        map.insert("Terminate".into(), Dynamic::from(Self::Terminate));
        map.insert(
            "RestartOnFailure".into(),
            Dynamic::from(Self::RestartOnFailure),
        );
        map
    }
}

/// What an Ingress terminates at the edge. Pairs with [`Output`] to
/// describe what the ingress actually does with traffic; not every
/// `(Terminate, Output)` combination is valid (see
/// `ingress.tls(Terminate, Output)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminate {
    // l[impl const.terminate.tls]
    Tls,
    // l[impl const.terminate.dtls]
    Dtls,
    // l[impl const.terminate.https]
    Https,
}

impl Terminate {
    pub fn rhai_constant() -> Map {
        let mut map = Map::new();
        map.insert("Tls".into(), Dynamic::from(Self::Tls));
        map.insert("Dtls".into(), Dynamic::from(Self::Dtls));
        map.insert("Https".into(), Dynamic::from(Self::Https));
        map
    }
}

/// Protocol the Ingress emits to upstream (the bound Service). Pairs
/// with [`Terminate`] in `ingress.tls(Terminate, Output)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    // l[impl const.output.tcp]
    Tcp,
    // l[impl const.output.udp]
    Udp,
    // l[impl const.output.http1]
    Http1,
    // l[impl const.output.http2]
    Http2,
}

impl Output {
    pub fn rhai_constant() -> Map {
        let mut map = Map::new();
        map.insert("Tcp".into(), Dynamic::from(Self::Tcp));
        map.insert("Udp".into(), Dynamic::from(Self::Udp));
        map.insert("Http1".into(), Dynamic::from(Self::Http1));
        map.insert("Http2".into(), Dynamic::from(Self::Http2));
        map
    }
}
