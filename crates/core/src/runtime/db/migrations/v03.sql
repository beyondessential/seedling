CREATE TABLE IF NOT EXISTS caddy_state (
    singleton        INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
    active_container TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS caddy_proxy_config (
    singleton   INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
    config_json TEXT    NOT NULL
);
