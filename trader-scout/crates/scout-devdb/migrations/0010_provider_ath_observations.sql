-- Provider ATH extrema stay distinct from annual/daily price windows and ATH FDV.
CREATE TABLE ath_price_observations (
    chain                    TEXT NOT NULL,
    token                    TEXT NOT NULL,
    source                   TEXT NOT NULL,
    highest_price_usd        TEXT,
    timestamp_ms             BIGINT,
    pool_id                  TEXT,
    highest_market_cap_usd   TEXT,
    observed_at              BIGINT NOT NULL,
    response_sha256           TEXT NOT NULL CHECK (response_sha256 ~ '^[0-9a-f]{64}$'),
    scope                    TEXT NOT NULL CHECK (scope = 'provider-recording-window-unknown'),
    CHECK (highest_price_usd IS NULL OR
           (length(highest_price_usd) BETWEEN 1 AND 80 AND highest_price_usd ~ '^[0-9]+(\.[0-9]+)?$')),
    CHECK (highest_market_cap_usd IS NULL OR
           (length(highest_market_cap_usd) BETWEEN 1 AND 80 AND highest_market_cap_usd ~ '^[0-9]+(\.[0-9]+)?$')),
    CHECK (timestamp_ms IS NULL OR timestamp_ms >= 0),
    PRIMARY KEY (chain, token, source)
);

-- Monthly application allocation shared across credential rotations. Every
-- network attempt is reserved before HTTP, including failed responses.
CREATE TABLE provider_request_budgets (
    provider                 TEXT NOT NULL,
    quota_group              TEXT NOT NULL,
    billing_period           TEXT NOT NULL CHECK (billing_period ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'),
    application_limit        BIGINT NOT NULL CHECK (application_limit > 0),
    prior_application_usage  BIGINT NOT NULL CHECK (prior_application_usage >= 0),
    reserved_attempts        BIGINT NOT NULL DEFAULT 0 CHECK (reserved_attempts >= 0),
    current_key_fingerprint  TEXT NOT NULL CHECK (current_key_fingerprint ~ '^[0-9a-f]{64}$'),
    updated_at               BIGINT NOT NULL,
    CHECK (prior_application_usage + reserved_attempts <= application_limit),
    PRIMARY KEY (provider, quota_group, billing_period)
);
