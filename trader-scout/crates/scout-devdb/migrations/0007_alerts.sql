-- Alerts (2026-10-09): one open incident per failing key (a chain's pass, a
-- provider running on its fallback, the derivation), alerted once after a
-- threshold and closed on the first success; and the daemon's liveness, so a
-- restart after a crash / kill / power loss is reported.
CREATE TABLE incidents (
    key              TEXT PRIMARY KEY,
    first_failed_at  BIGINT  NOT NULL,
    last_failed_at   BIGINT  NOT NULL,
    failures         INTEGER NOT NULL,
    last_error       TEXT    NOT NULL,
    alerted_at       BIGINT
);

CREATE TABLE daemon_state (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    started_at  BIGINT  NOT NULL,
    last_beat   BIGINT  NOT NULL,
    clean_stop  BOOLEAN NOT NULL
);
