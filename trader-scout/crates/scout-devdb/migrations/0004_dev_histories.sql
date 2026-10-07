-- ADR-021 amendment 3: a dev behind a shared intermediary becomes a candidate
-- when one of its launches migrates; its other launches through intermediaries
-- are then found once in its own transaction history (recorded here) and
-- matched to launches by transaction hash.
CREATE INDEX launches_by_tx ON launches (chain, tx_hash);

CREATE TABLE dev_histories (
    chain       TEXT   NOT NULL,
    dev         TEXT   NOT NULL,
    launches    INTEGER NOT NULL,   -- launches attributed from the history
    fetched_at  BIGINT NOT NULL,
    PRIMARY KEY (chain, dev)
);
