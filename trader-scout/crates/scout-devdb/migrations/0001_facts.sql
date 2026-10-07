-- ADR-021: immutable chain facts and refreshable observations of the dev tracker.

CREATE TABLE launches (
    chain          TEXT        NOT NULL,
    token          TEXT        NOT NULL,
    launchpad      TEXT        NOT NULL,
    creator        TEXT        NOT NULL,
    created_block  BIGINT      NOT NULL,
    created_at     BIGINT      NOT NULL, -- unix seconds
    tx_hash        TEXT        NOT NULL,
    source         TEXT        NOT NULL,
    PRIMARY KEY (chain, token)
);
CREATE INDEX launches_by_creator ON launches (chain, creator, created_at);

CREATE TABLE migrations (
    chain          TEXT        NOT NULL,
    token          TEXT        NOT NULL,
    launchpad      TEXT        NOT NULL,
    migrated_block BIGINT      NOT NULL,
    migrated_at    BIGINT      NOT NULL,
    tx_hash        TEXT        NOT NULL,
    pool           TEXT,
    source         TEXT        NOT NULL,
    PRIMARY KEY (chain, token)
);

CREATE TABLE ath (
    chain          TEXT        NOT NULL,
    token          TEXT        NOT NULL,
    ath_fdv_cents  BIGINT      NOT NULL, -- USD cents (integers only, no floats)
    ath_at         BIGINT,
    source         TEXT        NOT NULL,
    observed_at    BIGINT      NOT NULL,
    PRIMARY KEY (chain, token)
);

CREATE TABLE cursors (
    source         TEXT        PRIMARY KEY,
    position       TEXT        NOT NULL,
    updated_at     BIGINT      NOT NULL
);

CREATE TABLE deliveries (
    category       TEXT        NOT NULL,
    format         TEXT        NOT NULL,
    content_hash   TEXT        NOT NULL,
    delivered_at   BIGINT      NOT NULL,
    PRIMARY KEY (category, format)
);
