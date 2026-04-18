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
