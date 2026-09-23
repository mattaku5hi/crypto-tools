//! SQLite WAL connection setup. See ARCHITECTURE.md §12: "SQLite пишет
//! батчами через dedicated writer thread; соединения/очереди
//! ограничены... Несколько CLI processes координируются транзакциями,
//! unique constraints, busy timeout и ограниченными retries."

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// A single SQLite connection configured for this workspace's storage
/// contract: WAL journal mode (concurrent readers, one writer — S21),
/// a bounded busy timeout (never an unbounded retry loop across
/// multiple CLI processes sharing one store), and foreign keys enforced.
///
/// This type does not itself provide multi-connection pooling or a
/// dedicated writer thread — per ARCHITECTURE.md §12 those are the
/// engine's responsibility once scout-engine exists. This is the
/// connection-configuration primitive they will build on.
#[derive(Debug)]
pub struct StoreConnection {
    conn: Connection,
}

impl StoreConnection {
    /// Open (or create) a store at `path`. Sets `journal_mode = WAL` and
    /// a bounded busy timeout — never `busy_timeout = 0` (unbounded
    /// immediate failure) or an unbounded wait.
    pub fn open<P: AsRef<Path>>(path: P, busy_timeout: Duration) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(busy_timeout)?;
        let store = Self { conn };
        store.run_migrations()?;
        Ok(store)
    }

    /// In-memory store for tests: same schema, no file, no WAL (WAL
    /// requires a real filesystem; `journal_mode` on `:memory:` falls
    /// back to `memory` mode automatically, which is fine for tests that
    /// don't exercise multi-connection concurrency).
    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.run_migrations()?;
        Ok(store)
    }

    #[must_use]
    pub fn raw(&self) -> &Connection {
        &self.conn
    }

    fn run_migrations(&self) -> Result<(), StorageError> {
        // Schema versioning via a single-row table, not an external
        // migration framework — this workspace's storage needs are
        // small and evolving; a heavier migration tool is not justified
        // yet (ADR-worthy decision if that changes).
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS watermarks (
                scope TEXT NOT NULL,
                kind TEXT NOT NULL,
                position TEXT NOT NULL,
                updated_at_unix_ms INTEGER NOT NULL,
                PRIMARY KEY (scope, kind)
            );
            ",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_in_memory_store_and_creates_schema() {
        let store = StoreConnection::open_in_memory().unwrap();
        let count: i64 = store
            .raw()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='watermarks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn open_on_disk_sets_wal_journal_mode() {
        let dir = std::env::temp_dir().join(format!("scout_storage_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");
        let store = StoreConnection::open(&db_path, Duration::from_secs(5)).unwrap();
        let mode: String = store
            .raw()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrations_are_idempotent_when_reopening_same_store() {
        let dir = std::env::temp_dir().join(format!(
            "scout_storage_test_idempotent_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");
        {
            let _store = StoreConnection::open(&db_path, Duration::from_secs(5)).unwrap();
        }
        // Reopening must not error on "table already exists" (IF NOT
        // EXISTS) — durable resume across process restarts depends on
        // this (ARCHITECTURE.md §12's crash/resume contract).
        let _store2 = StoreConnection::open(&db_path, Duration::from_secs(5)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
