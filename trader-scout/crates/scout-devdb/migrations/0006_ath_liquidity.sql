-- 2026-10-08: Bankr tokens (Doppler v4, 10B supply, ~2,000 airdrop holders)
-- show ATH FDVs of $380M-$6B with $0-$3.5k liquidity: one tiny trade priced
-- over the whole supply. The highest liquidity seen across observations is
-- kept; on launchpads with such artifacts a runner needs liquidity of at
-- least a configured share of its ATH.
ALTER TABLE ath ADD COLUMN max_liquidity_cents BIGINT;
