-- ADR-021 amendment 1: the dev behind a launch. The launchpad's creator field
-- can be a contract: a shared intermediary (Flap VaultPortal, launcher
-- services: many signers) or a single operator's bot (one signer).

ALTER TABLE launches ADD COLUMN signer TEXT; -- tx.from, resolved when needed

CREATE TABLE address_kinds (
    chain        TEXT    NOT NULL,
    address      TEXT    NOT NULL,
    is_contract  BOOLEAN NOT NULL,
    -- contracts only: the single signer seen in every sampled launch, or NULL
    -- when the samples disagree (shared intermediary)
    owner        TEXT,
    sampled      INTEGER NOT NULL DEFAULT 0,
    checked_at   BIGINT  NOT NULL,
    PRIMARY KEY (chain, address)
);
