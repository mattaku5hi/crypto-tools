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
const MIGRATIONS: &[(i64, &str)] = &[(1, include_str!("../migrations/0001_facts.sql"))];

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

/// One launch of a creator with what is known about its outcome — the input
/// of the category derivation.
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
        for r in rows {
            n += sqlx::query(
                "INSERT INTO launches (chain, token, launchpad, creator, created_block, created_at, tx_hash, source)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (chain, token) DO NOTHING",
            )
            .bind(&r.chain)
            .bind(&r.token)
            .bind(&r.launchpad)
            .bind(&r.creator)
            .bind(r.created_block)
            .bind(r.created_at)
            .bind(&r.tx_hash)
            .bind(&r.source)
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
        for r in rows {
            n += sqlx::query(
                "INSERT INTO migrations (chain, token, launchpad, migrated_block, migrated_at, tx_hash, pool, source)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (chain, token) DO NOTHING",
            )
            .bind(&r.chain)
            .bind(&r.token)
            .bind(&r.launchpad)
            .bind(r.migrated_block)
            .bind(r.migrated_at)
            .bind(&r.tx_hash)
            .bind(&r.pool)
            .bind(&r.source)
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
        let rows = sqlx::query(
            "SELECT l.chain, l.creator, l.token, l.launchpad, l.created_at,
                    m.migrated_at, a.ath_fdv_cents
             FROM launches l
             LEFT JOIN migrations m ON m.chain = l.chain AND m.token = l.token
             LEFT JOIN ath a ON a.chain = l.chain AND a.token = l.token
             WHERE ($1::text IS NULL OR l.chain = $1) AND l.created_at >= $2
             ORDER BY l.chain, l.creator, l.created_at, l.token",
        )
        .bind(chain)
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| DevLaunchRow {
                chain: r.get("chain"),
                creator: r.get("creator"),
                token: r.get("token"),
                launchpad: r.get("launchpad"),
                created_at: r.get("created_at"),
                migrated_at: r.get("migrated_at"),
                ath_fdv_cents: r.get("ath_fdv_cents"),
            })
            .collect())
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

    /// Record a delivered export.
    ///
    /// # Errors
    /// Database failure.
    pub async fn record_delivery(
        &self,
        category: &str,
        format: &str,
        content_hash: &str,
        now: i64,
    ) -> Result<(), DevDbError> {
        sqlx::query(
            "INSERT INTO deliveries (category, format, content_hash, delivered_at) VALUES ($1, $2, $3, $4)
             ON CONFLICT (category, format) DO UPDATE SET content_hash = EXCLUDED.content_hash,
                delivered_at = EXCLUDED.delivered_at",
        )
        .bind(category)
        .bind(format)
        .bind(content_hash)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
