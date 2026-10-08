-- 2026-10-08: Zora coins with 1-8 holders showed ATHs of $1.4M-$99M (one tiny
-- trade in an empty pool). Holders are stored with each ATH observation; the
-- derivation counts a runner only with enough holders. Existing observations
-- have no holder count yet, so they are made stale to be observed again.
ALTER TABLE ath ADD COLUMN holders BIGINT;
UPDATE ath SET observed_at = 0;
