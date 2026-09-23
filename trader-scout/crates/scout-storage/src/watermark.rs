//! Durable watermarks. See ARCHITECTURE.md §12: "существует несколько
//! явных watermarks: fetched, raw_durable, decoded, ledger_applied,
//! finalized. Не выдавать fetched за processed."
//!
//! A watermark records how far processing has *durably* progressed for
//! a given scope (e.g. one wallet, one token scan) — never advanced
//! past what has actually been persisted, so a crash-and-resume never
//! silently skips unprocessed data.

use crate::connection::{StorageError, StoreConnection};

/// Which stage of the pipeline this watermark tracks. Kept as distinct
/// stages rather than one "progress" number, because "fetched" does not
/// imply "decoded", and "decoded" does not imply "applied to the
/// ledger" — conflating them is exactly what ARCHITECTURE.md §12 warns
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WatermarkKind {
    Fetched,
    RawDurable,
    Decoded,
    LedgerApplied,
    Finalized,
}

impl WatermarkKind {
    fn as_str(self) -> &'static str {
        match self {
            WatermarkKind::Fetched => "fetched",
            WatermarkKind::RawDurable => "raw_durable",
            WatermarkKind::Decoded => "decoded",
            WatermarkKind::LedgerApplied => "ledger_applied",
            WatermarkKind::Finalized => "finalized",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "fetched" => Some(WatermarkKind::Fetched),
            "raw_durable" => Some(WatermarkKind::RawDurable),
            "decoded" => Some(WatermarkKind::Decoded),
            "ledger_applied" => Some(WatermarkKind::LedgerApplied),
            "finalized" => Some(WatermarkKind::Finalized),
            _ => None,
        }
    }
}

/// One durable watermark record: a scope (e.g. a wallet or scan-request
/// identifier), which pipeline stage, and an opaque position string
/// (e.g. a canonical block/slot position — the exact encoding is owned
/// by the caller, this store treats it as opaque text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watermark {
    pub scope: String,
    pub kind: WatermarkKind,
    pub position: String,
}

/// Durable watermark persistence over a [`StoreConnection`].
#[derive(Debug)]
pub struct WatermarkStore<'a> {
    store: &'a StoreConnection,
}

impl<'a> WatermarkStore<'a> {
    #[must_use]
    pub fn new(store: &'a StoreConnection) -> Self {
        Self { store }
    }

    /// Upsert a watermark. This is the only mutation path — a watermark
    /// is set to reflect durably-persisted progress, never advanced
    /// speculatively ahead of what has actually been written.
    pub fn set(&self, watermark: &Watermark) -> Result<(), StorageError> {
        let now_ms = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(0);
        self.store.raw().execute(
            "INSERT INTO watermarks (scope, kind, position, updated_at_unix_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(scope, kind) DO UPDATE SET
                position = excluded.position,
                updated_at_unix_ms = excluded.updated_at_unix_ms",
            rusqlite::params![
                watermark.scope,
                watermark.kind.as_str(),
                watermark.position,
                now_ms
            ],
        )?;
        Ok(())
    }

    /// Read the current watermark for a `(scope, kind)` pair, if any has
    /// ever been recorded. `None` means "no durable progress yet" — a
    /// resume path must treat this as "start from the beginning of the
    /// declared scope," never as an error.
    pub fn get(&self, scope: &str, kind: WatermarkKind) -> Result<Option<Watermark>, StorageError> {
        let mut stmt = self.store.raw().prepare(
            "SELECT scope, kind, position FROM watermarks WHERE scope = ?1 AND kind = ?2",
        )?;
        let mut rows = stmt.query(rusqlite::params![scope, kind.as_str()])?;
        if let Some(row) = rows.next()? {
            let scope: String = row.get(0)?;
            let kind_str: String = row.get(1)?;
            let position: String = row.get(2)?;
            let kind = WatermarkKind::from_str(&kind_str).unwrap_or(WatermarkKind::Fetched);
            Ok(Some(Watermark {
                scope,
                kind,
                position,
            }))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_watermark_returns_none_not_an_error() {
        let store = StoreConnection::open_in_memory().unwrap();
        let watermarks = WatermarkStore::new(&store);
        let result = watermarks
            .get("wallet:0xabc", WatermarkKind::Fetched)
            .unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn set_then_get_roundtrips() {
        let store = StoreConnection::open_in_memory().unwrap();
        let watermarks = WatermarkStore::new(&store);
        let wm = Watermark {
            scope: "wallet:0xabc".to_string(),
            kind: WatermarkKind::Decoded,
            position: "block:12345".to_string(),
        };
        watermarks.set(&wm).unwrap();
        let fetched = watermarks
            .get("wallet:0xabc", WatermarkKind::Decoded)
            .unwrap();
        assert_eq!(fetched, Some(wm));
    }

    #[test]
    fn different_kinds_for_same_scope_are_independent() {
        // ARCHITECTURE.md §12: fetched/raw_durable/decoded/ledger_applied/
        // finalized are distinct watermarks, never conflated into one.
        let store = StoreConnection::open_in_memory().unwrap();
        let watermarks = WatermarkStore::new(&store);
        watermarks
            .set(&Watermark {
                scope: "wallet:0xabc".to_string(),
                kind: WatermarkKind::Fetched,
                position: "block:200".to_string(),
            })
            .unwrap();
        watermarks
            .set(&Watermark {
                scope: "wallet:0xabc".to_string(),
                kind: WatermarkKind::LedgerApplied,
                position: "block:100".to_string(),
            })
            .unwrap();

        let fetched = watermarks
            .get("wallet:0xabc", WatermarkKind::Fetched)
            .unwrap()
            .unwrap();
        let applied = watermarks
            .get("wallet:0xabc", WatermarkKind::LedgerApplied)
            .unwrap()
            .unwrap();
        assert_eq!(fetched.position, "block:200");
        assert_eq!(applied.position, "block:100");
    }

    #[test]
    fn setting_same_scope_and_kind_again_updates_not_duplicates() {
        let store = StoreConnection::open_in_memory().unwrap();
        let watermarks = WatermarkStore::new(&store);
        let base = Watermark {
            scope: "wallet:0xabc".to_string(),
            kind: WatermarkKind::Fetched,
            position: "block:100".to_string(),
        };
        watermarks.set(&base).unwrap();
        watermarks
            .set(&Watermark {
                position: "block:200".to_string(),
                ..base.clone()
            })
            .unwrap();

        let count: i64 = store
            .raw()
            .query_row("SELECT COUNT(*) FROM watermarks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let current = watermarks
            .get("wallet:0xabc", WatermarkKind::Fetched)
            .unwrap()
            .unwrap();
        assert_eq!(current.position, "block:200");
    }

    #[test]
    fn different_scopes_do_not_collide() {
        let store = StoreConnection::open_in_memory().unwrap();
        let watermarks = WatermarkStore::new(&store);
        watermarks
            .set(&Watermark {
                scope: "wallet:A".to_string(),
                kind: WatermarkKind::Fetched,
                position: "block:1".to_string(),
            })
            .unwrap();
        watermarks
            .set(&Watermark {
                scope: "wallet:B".to_string(),
                kind: WatermarkKind::Fetched,
                position: "block:2".to_string(),
            })
            .unwrap();

        let a = watermarks
            .get("wallet:A", WatermarkKind::Fetched)
            .unwrap()
            .unwrap();
        let b = watermarks
            .get("wallet:B", WatermarkKind::Fetched)
            .unwrap()
            .unwrap();
        assert_eq!(a.position, "block:1");
        assert_eq!(b.position, "block:2");
    }
}
