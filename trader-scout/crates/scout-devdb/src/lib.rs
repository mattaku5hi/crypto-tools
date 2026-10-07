//! Dev tracker fact store (ADR-021): launches, migrations, ATH observations,
//! ingestion cursors and delivery hashes in PostgreSQL.
//!
//! Every write is idempotent on its natural key, so a cycle replayed after a
//! crash cannot duplicate facts: launches and migrations are immutable
//! (`ON CONFLICT DO NOTHING`), an ATH observation replaces an older one only
//! (`observed_at` ordering), cursors and deliveries are last-write-wins.
//! Queries are runtime-checked (`sqlx::query`), so building needs no database.

use sqlx::Row;
use sqlx::postgres::{PgPool, PgPoolOptions};

/// Failure of the fact store.
#[derive(Debug, thiserror::Error)]
pub enum DevDbError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
}

/// Embedded schema migrations, applied in order and recorded in
/// `schema_migrations` (sqlx's own migrator is not used: its macro feature
/// pulls a second native SQLite into the workspace, which already links one
/// through `rusqlite`).
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_facts.sql")),
    (2, include_str!("../migrations/0002_dev_identity.sql")),
    (3, include_str!("../migrations/0003_delivery_members.sql")),
    (4, include_str!("../migrations/0004_dev_histories.sql")),
];

/// Rows per `INSERT … SELECT FROM UNNEST` statement.
const INSERT_CHUNK: usize = 5_000;

/// A token launch (one per token; `creator` is the launchpad's own creator field).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub chain: String,
    pub token: String,
    pub launchpad: String,
    pub creator: String,
    pub created_block: i64,
    pub created_at: i64,
    pub tx_hash: String,
    pub source: String,
}

/// A curve graduation (the launchpad's own migration event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    pub chain: String,
    pub token: String,
    pub launchpad: String,
    pub migrated_block: i64,
    pub migrated_at: i64,
    pub tx_hash: String,
    pub pool: Option<String>,
    pub source: String,
}

/// An all-time-high observation (USD cents of the fully diluted value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AthObservation {
    pub chain: String,
    pub token: String,
    pub ath_fdv_cents: i64,
    pub ath_at: Option<i64>,
    pub source: String,
    pub observed_at: i64,
}

/// What is known about a creator address (ADR-021 amendment 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressKind {
    pub chain: String,
    pub address: String,
    pub is_contract: bool,
    /// Contracts: the single signer of every sampled launch (`None` = shared).
    pub owner: Option<String>,
    pub sampled: i32,
    pub checked_at: i64,
}

/// Launchpads without a bonding curve (no migration rate; owner decision
/// 2026-10-06). Kept in sync with the config's `launchpads_without_migration`
/// default.
pub const NO_CURVE_LAUNCHPADS: &[&str] = &["zora", "clanker"];

/// Launchpads whose creator field is the caller (`msg.sender`), so a contract
/// there may be a shared intermediary (ADR-021 amendment 1). Pons is listed
/// although its field is named `originalDeployer`: live, a launcher-service
/// contract fills it for many signers. Zora (`payoutRecipient`, mostly smart
/// wallets whose tx signer is a shared ERC-4337 bundler) and Clanker
/// (`tokenAdmin`) keep their creator field as the dev.
pub const SIGNER_RESOLVED_LAUNCHPADS: &[&str] = &["flap", "fourmeme", "pons"];

/// One launch of a dev with what is known about its outcome — the input of
/// the category derivation. `creator` is the resolved DEV (ADR-021
/// amendment 1): the creator field, except on [`SIGNER_RESOLVED_LAUNCHPADS`]
/// where a contract creator with several signers (a shared intermediary) is
/// replaced by each launch's signer (`contract:<addr>` while unresolved). A
/// single-owner contract stays its own dev (its signer may be a relayer or
/// bundler shared by many users, so it is never merged into it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevLaunchRow {
    pub chain: String,
    pub creator: String,
    pub token: String,
    pub launchpad: String,
    pub created_at: i64,
    pub migrated_at: Option<i64>,
    pub ath_fdv_cents: Option<i64>,
}

/// Connection pool to the fact store.
#[derive(Debug, Clone)]
pub struct DevDb {
    pool: PgPool,
}

impl DevDb {
    /// Connect (`postgres://…`); the URL is a secret and is not echoed in errors
    /// by this crate.
    ///
    /// # Errors
    /// Connection failure.
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, DevDbError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.max(1))
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    /// Apply the embedded schema migrations (idempotent).
    ///
    /// # Errors
    /// Migration failure.
    pub async fn migrate(&self) -> Result<(), DevDbError> {
        let mut tx = self.pool.begin().await?;
        // one migrator at a time across processes
        sqlx::query("SELECT pg_advisory_xact_lock(727372)")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS schema_migrations (version BIGINT PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT now())",
        )
        .execute(&mut *tx)
        .await?;
        for (version, sql) in MIGRATIONS {
            let done = sqlx::query("SELECT 1 FROM schema_migrations WHERE version = $1")
                .bind(version)
                .fetch_optional(&mut *tx)
                .await?
                .is_some();
            if done {
                continue;
            }
            sqlx::raw_sql(sql).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO schema_migrations (version) VALUES ($1)")
                .bind(version)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Insert launches, ignoring tokens already known. Returns the number of
    /// new rows.
    ///
    /// # Errors
    /// Database failure (the batch is atomic).
    pub async fn insert_launches(&self, rows: &[Launch]) -> Result<u64, DevDbError> {
        let mut tx = self.pool.begin().await?;
        let mut n = 0;
        // one statement per chunk (arrays via UNNEST): a backfill writes tens
        // of millions of rows
        for c in rows.chunks(INSERT_CHUNK) {
            let col = |f: fn(&Launch) -> &String| c.iter().map(f).cloned().collect::<Vec<String>>();
            n += sqlx::query(
                "INSERT INTO launches (chain, token, launchpad, creator, created_block, created_at, tx_hash, source)
                 SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::bigint[],
                                      $6::bigint[], $7::text[], $8::text[])
                 ON CONFLICT (chain, token) DO NOTHING",
            )
            .bind(col(|r| &r.chain))
            .bind(col(|r| &r.token))
            .bind(col(|r| &r.launchpad))
            .bind(col(|r| &r.creator))
            .bind(c.iter().map(|r| r.created_block).collect::<Vec<i64>>())
            .bind(c.iter().map(|r| r.created_at).collect::<Vec<i64>>())
            .bind(col(|r| &r.tx_hash))
            .bind(col(|r| &r.source))
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
        Ok(n)
    }

    /// Insert migrations, ignoring tokens already migrated. Returns new rows.
    ///
    /// # Errors
    /// Database failure (the batch is atomic).
    pub async fn insert_migrations(&self, rows: &[Migration]) -> Result<u64, DevDbError> {
        let mut tx = self.pool.begin().await?;
        let mut n = 0;
        for c in rows.chunks(INSERT_CHUNK) {
            let col =
                |f: fn(&Migration) -> &String| c.iter().map(f).cloned().collect::<Vec<String>>();
            n += sqlx::query(
                "INSERT INTO migrations (chain, token, launchpad, migrated_block, migrated_at, tx_hash, pool, source)
                 SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::bigint[], $5::bigint[],
                                      $6::text[], $7::text[], $8::text[])
                 ON CONFLICT (chain, token) DO NOTHING",
            )
            .bind(col(|r| &r.chain))
            .bind(col(|r| &r.token))
            .bind(col(|r| &r.launchpad))
            .bind(c.iter().map(|r| r.migrated_block).collect::<Vec<i64>>())
            .bind(c.iter().map(|r| r.migrated_at).collect::<Vec<i64>>())
            .bind(col(|r| &r.tx_hash))
            .bind(c.iter().map(|r| r.pool.clone()).collect::<Vec<Option<String>>>())
            .bind(col(|r| &r.source))
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
        Ok(n)
    }

    /// Record ATH observations; an observation replaces the stored one only
    /// when it is at least as recent. Returns the rows written.
    ///
    /// # Errors
    /// Database failure (the batch is atomic).
    pub async fn upsert_ath(&self, rows: &[AthObservation]) -> Result<u64, DevDbError> {
        let mut tx = self.pool.begin().await?;
        let mut n = 0;
        for r in rows {
            n += sqlx::query(
                "INSERT INTO ath (chain, token, ath_fdv_cents, ath_at, source, observed_at)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (chain, token) DO UPDATE SET
                    ath_fdv_cents = EXCLUDED.ath_fdv_cents, ath_at = EXCLUDED.ath_at,
                    source = EXCLUDED.source, observed_at = EXCLUDED.observed_at
                 WHERE EXCLUDED.observed_at >= ath.observed_at",
            )
            .bind(&r.chain)
            .bind(&r.token)
            .bind(r.ath_fdv_cents)
            .bind(r.ath_at)
            .bind(&r.source)
            .bind(r.observed_at)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
        Ok(n)
    }

    /// Last fully ingested position of a source (`None` = never ingested).
    ///
    /// # Errors
    /// Database failure.
    pub async fn cursor(&self, source: &str) -> Result<Option<String>, DevDbError> {
        let row = sqlx::query("SELECT position FROM cursors WHERE source = $1")
            .bind(source)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<String, _>("position")))
    }

    /// Store a source's position (call after its facts are committed).
    ///
    /// # Errors
    /// Database failure.
    pub async fn set_cursor(
        &self,
        source: &str,
        position: &str,
        now: i64,
    ) -> Result<(), DevDbError> {
        sqlx::query(
            "INSERT INTO cursors (source, position, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT (source) DO UPDATE SET position = EXCLUDED.position, updated_at = EXCLUDED.updated_at",
        )
        .bind(source)
        .bind(position)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every launch of every creator on `chain` (all chains when `None`) that
    /// launched at or after `since`, with its migration time and latest ATH.
    ///
    /// # Errors
    /// Database failure.
    pub async fn dev_launches(
        &self,
        chain: Option<&str>,
        since: i64,
    ) -> Result<Vec<DevLaunchRow>, DevDbError> {
        let mut all = Vec::new();
        self.for_each_dev(chain, since, |rows| all.extend_from_slice(rows))
            .await?;
        Ok(all)
    }

    /// As [`DevDb::dev_launches`], streamed: `f` gets every launch of one
    /// (chain, dev) at a time, in launch order, so memory stays bounded by the
    /// busiest dev, not by the year of launches.
    ///
    /// # Errors
    /// Database failure.
    pub async fn for_each_dev(
        &self,
        chain: Option<&str>,
        since: i64,
        mut f: impl FnMut(&[DevLaunchRow]),
    ) -> Result<(), DevDbError> {
        use futures::TryStreamExt;
        let mut rows = sqlx::query(
            "SELECT l.chain,
                    CASE WHEN l.launchpad = ANY($3) AND k.is_contract AND k.owner IS NULL
                         THEN COALESCE(l.signer, 'contract:' || l.creator)
                         ELSE l.creator END AS creator,
                    l.token, l.launchpad, l.created_at,
                    m.migrated_at, a.ath_fdv_cents
             FROM launches l
             LEFT JOIN address_kinds k ON k.chain = l.chain AND k.address = l.creator
             LEFT JOIN migrations m ON m.chain = l.chain AND m.token = l.token
             LEFT JOIN ath a ON a.chain = l.chain AND a.token = l.token
             WHERE ($1::text IS NULL OR l.chain = $1) AND l.created_at >= $2
               -- launches through a shared intermediary whose signer is not
               -- known yet belong to no dev (never one pseudo-dev per contract)
               AND NOT (l.launchpad = ANY($3) AND COALESCE(k.is_contract, false)
                        AND k.owner IS NULL AND l.signer IS NULL)
             ORDER BY 1, 2, l.created_at, l.token",
        )
        .bind(chain)
        .bind(since)
        .bind(SIGNER_RESOLVED_LAUNCHPADS)
        .fetch(&self.pool);
        let mut group: Vec<DevLaunchRow> = Vec::new();
        while let Some(r) = rows.try_next().await? {
            let row = DevLaunchRow {
                chain: r.get("chain"),
                creator: r.get("creator"),
                token: r.get("token"),
                launchpad: r.get("launchpad"),
                created_at: r.get("created_at"),
                migrated_at: r.get("migrated_at"),
                ath_fdv_cents: r.get("ath_fdv_cents"),
            };
            if group
                .last()
                .is_some_and(|g| g.chain != row.chain || g.creator != row.creator)
            {
                f(&group);
                group.clear();
            }
            group.push(row);
        }
        if !group.is_empty() {
            f(&group);
        }
        Ok(())
    }

    /// Creator addresses on [`SIGNER_RESOLVED_LAUNCHPADS`] of `chain` with no
    /// recorded kind yet (most launches first), at most `limit`.
    ///
    /// # Errors
    /// Database failure.
    pub async fn creators_without_kind(
        &self,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<String>, DevDbError> {
        let rows = sqlx::query(
            "SELECT l.creator FROM launches l
             LEFT JOIN address_kinds k ON k.chain = l.chain AND k.address = l.creator
             WHERE l.chain = $1 AND k.address IS NULL AND l.launchpad = ANY($3)
             GROUP BY l.creator HAVING count(*) >= 2 ORDER BY count(*) DESC LIMIT $2",
        )
        .bind(chain)
        .bind(limit)
        .bind(SIGNER_RESOLVED_LAUNCHPADS)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.get("creator")).collect())
    }

    /// Record (replace) what is known about an address.
    ///
    /// # Errors
    /// Database failure.
    pub async fn set_address_kind(&self, k: &AddressKind) -> Result<(), DevDbError> {
        sqlx::query(
            "INSERT INTO address_kinds (chain, address, is_contract, owner, sampled, checked_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (chain, address) DO UPDATE SET is_contract = EXCLUDED.is_contract,
                owner = EXCLUDED.owner, sampled = EXCLUDED.sampled, checked_at = EXCLUDED.checked_at",
        )
        .bind(&k.chain)
        .bind(&k.address)
        .bind(k.is_contract)
        .bind(&k.owner)
        .bind(k.sampled)
        .bind(k.checked_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Up to `n` `(token, tx_hash)` launches of `creator` on `chain` (newest first).
    ///
    /// # Errors
    /// Database failure.
    pub async fn launches_of(
        &self,
        chain: &str,
        creator: &str,
        n: i64,
    ) -> Result<Vec<(String, String)>, DevDbError> {
        let rows = sqlx::query(
            "SELECT token, tx_hash FROM launches WHERE chain = $1 AND creator = $2
             ORDER BY created_at DESC LIMIT $3",
        )
        .bind(chain)
        .bind(creator)
        .bind(n)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("token"), r.get("tx_hash")))
            .collect())
    }

    /// Launches through shared intermediaries (contract creators without a
    /// single owner) whose signer is unresolved: `(token, tx_hash)`, migrated
    /// launches first (they decide categories; a backfill under a lookup budget
    /// resolves them before the long tail), then newest first, at most `limit`.
    ///
    /// # Errors
    /// Database failure.
    pub async fn shared_launches_without_signer(
        &self,
        chain: &str,
        limit: i64,
        recent_since: i64,
    ) -> Result<Vec<(String, String)>, DevDbError> {
        let rows = sqlx::query(
            "SELECT l.token, l.tx_hash FROM launches l
             JOIN address_kinds k ON k.chain = l.chain AND k.address = l.creator
             WHERE l.chain = $1 AND k.is_contract AND k.owner IS NULL AND l.signer IS NULL
               AND l.launchpad = ANY($3)
               AND (l.created_at >= $4
                    OR EXISTS (SELECT 1 FROM migrations m WHERE m.chain = l.chain AND m.token = l.token))
             ORDER BY EXISTS (SELECT 1 FROM migrations m
                              WHERE m.chain = l.chain AND m.token = l.token) DESC,
                      l.created_at DESC
             LIMIT $2",
        )
        .bind(chain)
        .bind(limit)
        .bind(SIGNER_RESOLVED_LAUNCHPADS)
        .bind(recent_since)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("token"), r.get("tx_hash")))
            .collect())
    }

    /// Devs whose history through intermediaries is not fetched yet: signers
    /// of migrated launches through shared intermediaries, at most `limit`.
    ///
    /// # Errors
    /// Database failure.
    pub async fn history_candidates(
        &self,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<String>, DevDbError> {
        let rows = sqlx::query(
            "SELECT DISTINCT l.signer FROM launches l
             JOIN address_kinds k ON k.chain = l.chain AND k.address = l.creator
             JOIN migrations m ON m.chain = l.chain AND m.token = l.token
             WHERE l.chain = $1 AND k.is_contract AND k.owner IS NULL AND l.signer IS NOT NULL
               AND l.launchpad = ANY($3)
               AND NOT EXISTS (SELECT 1 FROM dev_histories h WHERE h.chain = l.chain AND h.dev = l.signer)
             LIMIT $2",
        )
        .bind(chain)
        .bind(limit)
        .bind(SIGNER_RESOLVED_LAUNCHPADS)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.get("signer")).collect())
    }

    /// Shared intermediaries of `chain` (contract creators without a single
    /// owner).
    ///
    /// # Errors
    /// Database failure.
    pub async fn shared_intermediaries(&self, chain: &str) -> Result<Vec<String>, DevDbError> {
        let rows = sqlx::query(
            "SELECT k.address FROM address_kinds k
             WHERE k.chain = $1 AND k.is_contract AND k.owner IS NULL
               AND EXISTS (SELECT 1 FROM launches l WHERE l.chain = k.chain
                           AND l.creator = k.address AND l.launchpad = ANY($2))
             ORDER BY k.address",
        )
        .bind(chain)
        .bind(SIGNER_RESOLVED_LAUNCHPADS)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.get("address")).collect())
    }

    /// Attribute the launches created by `intermediary` in `tx_hashes` to
    /// `signer` (only those still unresolved). Returns the rows changed.
    ///
    /// # Errors
    /// Database failure.
    pub async fn set_signer_by_txs(
        &self,
        chain: &str,
        intermediary: &str,
        tx_hashes: &[String],
        signer: &str,
    ) -> Result<u64, DevDbError> {
        Ok(sqlx::query(
            "UPDATE launches SET signer = $4
             WHERE chain = $1 AND creator = $2 AND tx_hash = ANY($3) AND signer IS NULL",
        )
        .bind(chain)
        .bind(intermediary)
        .bind(tx_hashes)
        .bind(signer)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    /// Record that a dev's history was fetched.
    ///
    /// # Errors
    /// Database failure.
    pub async fn mark_history_fetched(
        &self,
        chain: &str,
        dev: &str,
        launches: i32,
        now: i64,
    ) -> Result<(), DevDbError> {
        sqlx::query(
            "INSERT INTO dev_histories (chain, dev, launches, fetched_at) VALUES ($1, $2, $3, $4)
             ON CONFLICT (chain, dev) DO UPDATE SET launches = EXCLUDED.launches,
                fetched_at = EXCLUDED.fetched_at",
        )
        .bind(chain)
        .bind(dev)
        .bind(launches)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record the signer (tx.from) of a launch.
    ///
    /// # Errors
    /// Database failure.
    pub async fn set_launch_signer(
        &self,
        chain: &str,
        token: &str,
        signer: &str,
    ) -> Result<(), DevDbError> {
        sqlx::query("UPDATE launches SET signer = $3 WHERE chain = $1 AND token = $2")
            .bind(chain)
            .bind(token)
            .bind(signer)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Tokens of `chain` whose ATH observation is missing or stale: migrated
    /// tokens and every token of a launchpad in `no_curve` (no migration),
    /// refreshed by age — < 2 days every 12 h, < 7 days daily, < 30 days
    /// weekly, older every 90 days (a year of tokens stays within Codex's free
    /// 10k requests/month: ≈ 4.8k steady, ≈ 3.7k more for the backfill
    /// month). Missing observations first, newest launches first, at most
    /// `limit`.
    ///
    /// # Errors
    /// Database failure.
    pub async fn ath_candidates(
        &self,
        chain: &str,
        now: i64,
        limit: i64,
    ) -> Result<Vec<String>, DevDbError> {
        let rows = sqlx::query(
            "SELECT l.token FROM launches l
             LEFT JOIN migrations m ON m.chain = l.chain AND m.token = l.token
             LEFT JOIN ath a ON a.chain = l.chain AND a.token = l.token
             WHERE l.chain = $1 AND (m.token IS NOT NULL OR l.launchpad = ANY($4))
               AND (a.observed_at IS NULL OR a.observed_at < $2 - CASE
                    WHEN $2 - l.created_at < 172800 THEN 43200
                    WHEN $2 - l.created_at < 604800 THEN 86400
                    WHEN $2 - l.created_at < 2592000 THEN 604800
                    ELSE 7776000 END)
             ORDER BY (a.observed_at IS NULL) DESC, l.created_at DESC
             LIMIT $3",
        )
        .bind(chain)
        .bind(now)
        .bind(limit)
        .bind(NO_CURVE_LAUNCHPADS)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.get("token")).collect())
    }

    /// Content hash of the last delivered export of a category/format.
    ///
    /// # Errors
    /// Database failure.
    pub async fn last_delivery(
        &self,
        category: &str,
        format: &str,
    ) -> Result<Option<String>, DevDbError> {
        let row =
            sqlx::query("SELECT content_hash FROM deliveries WHERE category = $1 AND format = $2")
                .bind(category)
                .bind(format)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| r.get::<String, _>("content_hash")))
    }

    /// Record a delivered export and its wallets (for the next diff).
    ///
    /// # Errors
    /// Database failure.
    pub async fn record_delivery(
        &self,
        category: &str,
        format: &str,
        content_hash: &str,
        members: &[String],
        now: i64,
    ) -> Result<(), DevDbError> {
        sqlx::query(
            "INSERT INTO deliveries (category, format, content_hash, delivered_at, members)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (category, format) DO UPDATE SET content_hash = EXCLUDED.content_hash,
                delivered_at = EXCLUDED.delivered_at, members = EXCLUDED.members",
        )
        .bind(category)
        .bind(format)
        .bind(content_hash)
        .bind(now)
        .bind(members)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every last delivery: (category key, format, content hash, wallets).
    ///
    /// # Errors
    /// Database failure.
    pub async fn deliveries(&self) -> Result<Vec<Delivery>, DevDbError> {
        let rows = sqlx::query(
            "SELECT category, format, content_hash, members, delivered_at FROM deliveries",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| Delivery {
                category: r.get("category"),
                format: r.get("format"),
                content_hash: r.get("content_hash"),
                members: r.get("members"),
                delivered_at: r.get("delivered_at"),
            })
            .collect())
    }
}

/// The last delivered version of one list in one format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    /// `top-migr:solana`.
    pub category: String,
    pub format: String,
    pub content_hash: String,
    pub members: Vec<String>,
    pub delivered_at: i64,
}
