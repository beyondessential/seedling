CREATE TABLE IF NOT EXISTS params (
    app_name   TEXT NOT NULL,
    param_name TEXT NOT NULL,
    value      TEXT NOT NULL,
    PRIMARY KEY (app_name, param_name)
);
