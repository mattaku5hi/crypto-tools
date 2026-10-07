-- B7: the wallets of the last delivered list, so the next delivery can say
-- what changed (added / dropped) and why.
ALTER TABLE deliveries ADD COLUMN members TEXT[] NOT NULL DEFAULT '{}';
