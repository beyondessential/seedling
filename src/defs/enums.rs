use rhai::{Dynamic, Map};

// r[const.on-update.rolling]
// r[const.on-update.replace]
#[derive(Debug, Default, Clone, Copy)]
pub enum OnUpdate {
    #[default]
    Rolling,
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

// r[const.on-terminate.recreate]
#[derive(Debug, Default, Clone, Copy)]
pub enum OnTerminate {
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

// r[const.on-exit.restart]
// r[const.on-exit.terminate]
// r[const.on-exit.restart-on-failure]
#[derive(Debug, Default, Clone, Copy)]
pub enum OnExit {
    #[default]
    Restart,
    Terminate,
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
