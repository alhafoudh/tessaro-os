-- The MQTT brokers this client reaches devices through (brokers.rs), by the
-- name `--mqtt @NAME` gives them. The password, the CA and the client
-- certificate's key are kept here as typed, like the nodes' tokens: the
-- file is 0600.
CREATE TABLE brokers (
    name TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    username TEXT,
    password TEXT,
    prefix TEXT NOT NULL DEFAULT 'tessaro',
    ca TEXT,
    client_cert TEXT,
    client_key TEXT
);
