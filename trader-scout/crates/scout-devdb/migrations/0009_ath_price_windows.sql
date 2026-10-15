-- Birdeye price-window observations are not Codex ATH-FDV observations.
CREATE TABLE ath_price_windows (
    chain             TEXT NOT NULL,
    token             TEXT NOT NULL,
    source            TEXT NOT NULL,
    window_from       BIGINT NOT NULL,
    window_to         BIGINT NOT NULL,
    max_price_usd     TEXT,
    high_candle_at    BIGINT,
    first_candle_at   BIGINT,
    last_candle_at    BIGINT,
    returned_candles  INTEGER NOT NULL CHECK (returned_candles BETWEEN 0 AND 5000),
    observed_at       BIGINT NOT NULL,
    CHECK (max_price_usd IS NULL OR
           (length(max_price_usd) <= 80 AND max_price_usd ~ '^[0-9]+(\.[0-9]+)?$')),
    CHECK (window_from >= 0 AND window_to >= window_from),
    CHECK ((returned_candles = 0 AND max_price_usd IS NULL AND high_candle_at IS NULL
            AND first_candle_at IS NULL AND last_candle_at IS NULL)
        OR (returned_candles > 0 AND max_price_usd IS NOT NULL AND high_candle_at IS NOT NULL
            AND first_candle_at IS NOT NULL AND last_candle_at IS NOT NULL)),
    PRIMARY KEY (chain, token, source, window_from, window_to)
);
