//! Solana pump.fun launch / migration ingestion (B2, ADR-021).
//!
//! Two address histories (B0.1 measurements): every `create` / `create_v2`
//! touches the `mint_authority` PDA [`PUMP_MINT_AUTHORITY`] (≈ 53k
//! transactions/day), every graduation to PumpSwap is signed by the migration
//! wallet [`PUMP_MIGRATOR`] (≈ 1.8k/day). Each source reads its address's
//! successful transactions in a block-time window `[cursor, now − settle)`,
//! decodes the self-CPI `CreateEvent` / `CompletePumpAmmMigrationEvent`
//! (`scout_dex_solana::decode_pump_lifecycle`) and inserts facts idempotently;
//! then the cursor (the next window's inclusive start, unix seconds) moves.
//! A pass cut by the page budget moves the cursor to the newest block time it
//! saw (that second is re-read next time; inserts are idempotent).
//!
//! The dev of a pump.fun launch is `CreateEvent.creator` (the coin creator,
//! normally the signer `user`). Works with any `HistoryProvider` that honors
//! `scan_block_time_range` and returns oldest first (Helius
//! `getTransactionsForAddress`; a standard-RPC fallback is A8).

use futures::StreamExt;
use scout_api::{HistoryProvider, ProviderError, ScanRequest, ScanTask};
use scout_core::{
    AddressBytes, ChainFamily, ChainKey, GenesisIdentity, NetworkId, RawPayload,
    RawSolanaTransaction, SolanaCluster, SolanaExecutionStatus, WalletKey,
};
use scout_devdb::{DevDb, DevDbError, Launch, Migration};
use scout_dex_solana::{PumpLifecycleEvent, decode_pump_lifecycle};
use tokio_util::sync::CancellationToken;

/// pump.fun `mint_authority` PDA (seeds `mint-authority`), used by
/// `create` / `create_v2` only.
pub const PUMP_MINT_AUTHORITY: &str = "TSLvdd1pWpHVjahSpsvCXUbgwsL3JAcvokwaKt1eokM";
/// The wallet that signs pump.fun → PumpSwap migrations.
pub const PUMP_MIGRATOR: &str = "39azUYFWPz3VHgKCf3VChUwbpURdCHRxjWVowf5jUJjg";

/// What a source stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolanaFact {
    Launch,
    Migration,
}

/// One Solana ingestion source.
#[derive(Debug, Clone, Copy)]
pub struct SolanaSource {
    /// Cursor key.
    pub key: &'static str,
    pub address: &'static str,
    pub kind: SolanaFact,
}

/// The pump.fun sources (launches first, so a migration in the same pass finds
/// its launch).
pub const SOLANA_SOURCES: [SolanaSource; 2] = [
    SolanaSource {
        key: "solana:pump:create",
        address: PUMP_MINT_AUTHORITY,
        kind: SolanaFact::Launch,
    },
    SolanaSource {
        key: "solana:pump:migrate",
        address: PUMP_MIGRATOR,
        kind: SolanaFact::Migration,
    },
];

/// Failure of a Solana ingestion pass.
#[derive(Debug, thiserror::Error)]
pub enum SolanaIngestError {
    #[error("provider: {0}")]
    Provider(#[from] ProviderError),
    #[error("{0}")]
    Db(#[from] DevDbError),
    #[error("cursor of {0} is not a unix time")]
    BadCursor(String),
    #[error("{0} is not a Solana address")]
    BadAddress(&'static str),
    #[error("standard-RPC fallback: {0}")]
    Fallback(String),
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SolanaIngestReport {
    /// Window `[from, to)` in unix seconds.
    pub from: i64,
    pub to: i64,
    pub transactions: usize,
    pub decoded: usize,
    pub inserted: u64,
    /// Lifecycle events with a broken layout (coverage gaps).
    pub undecodable: usize,
    /// The page budget ended the pass before `to`.
    pub truncated: bool,
}

fn b58(bytes: &[u8]) -> String {
    bs58::encode(bytes).into_string()
}

fn wallet(address: &'static str) -> Result<WalletKey, SolanaIngestError> {
    let bytes: [u8; 32] = bs58::decode(address)
        .into_vec()
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or(SolanaIngestError::BadAddress(address))?;
    Ok(WalletKey {
        chain: ChainKey {
            family: ChainFamily::Solana,
            network_id: NetworkId::SolanaCluster(SolanaCluster::Mainnet),
            genesis_identity: GenesisIdentity::Unverified,
        },
        address: AddressBytes::Solana(bytes),
    })
}

/// Facts of one transaction for `kind`: `(launches, migrations, undecodable)`.
#[must_use]
pub fn facts_of(
    tx: &RawSolanaTransaction,
    kind: SolanaFact,
    source: &str,
) -> (Vec<Launch>, Vec<Migration>, usize) {
    if !matches!(tx.execution, SolanaExecutionStatus::Succeeded) {
        return (Vec::new(), Vec::new(), 0);
    }
    facts_from(
        &tx.instructions,
        tx.slot,
        tx.block_time,
        &b58(&tx.signature),
        kind,
        source,
    )
}

/// Facts in the flattened instructions of one SUCCESSFUL transaction.
#[must_use]
pub fn facts_from(
    instructions: &[scout_core::RawSolanaInstruction],
    slot: u64,
    block_time: Option<i64>,
    signature: &str,
    kind: SolanaFact,
    source: &str,
) -> (Vec<Launch>, Vec<Migration>, usize) {
    let (mut launches, mut migrations, mut bad) = (Vec::new(), Vec::new(), 0usize);
    let slot = i64::try_from(slot).unwrap_or(i64::MAX);
    for ix in instructions {
        match (decode_pump_lifecycle(ix), kind) {
            (Ok(Some(PumpLifecycleEvent::Create(c))), SolanaFact::Launch) => {
                launches.push(Launch {
                    chain: "solana".to_string(),
                    token: b58(&c.mint),
                    launchpad: "pump".to_string(),
                    creator: b58(&c.creator),
                    created_block: slot,
                    created_at: block_time.unwrap_or(c.timestamp),
                    tx_hash: signature.to_string(),
                    source: source.to_string(),
                });
            }
            (Ok(Some(PumpLifecycleEvent::Migration(m))), SolanaFact::Migration) => {
                migrations.push(Migration {
                    chain: "solana".to_string(),
                    token: b58(&m.mint),
                    launchpad: "pump".to_string(),
                    migrated_block: slot,
                    migrated_at: block_time.unwrap_or(m.timestamp),
                    tx_hash: signature.to_string(),
                    pool: Some(b58(&m.pool)),
                    source: source.to_string(),
                });
            }
            (Err(_), _) => bad += 1,
            _ => {}
        }
    }
    (launches, migrations, bad)
}

/// Rows buffered before a database write.
const FLUSH_ROWS: usize = 2_000;

/// Ingest one source over `[cursor or now − start_secs_back, now − settle_secs)`.
///
/// # Errors
/// Provider or database failure; facts stored so far stay and the cursor is
/// not moved past them.
pub async fn ingest_solana_source(
    db: &DevDb,
    provider: &dyn HistoryProvider,
    src: &SolanaSource,
    start_secs_back: i64,
    settle_secs: i64,
    now: i64,
) -> Result<SolanaIngestReport, SolanaIngestError> {
    let from = match db.cursor(src.key).await? {
        Some(c) => c
            .parse::<i64>()
            .map_err(|_| SolanaIngestError::BadCursor(src.key.to_string()))?,
        None => now.saturating_sub(start_secs_back),
    };
    let to = now.saturating_sub(settle_secs);
    if from >= to {
        return Ok(SolanaIngestReport {
            from,
            to,
            ..SolanaIngestReport::default()
        });
    }
    let (mut report, newest_seen) = scan_window(db, provider, src, from, to).await?;
    report.from = from;
    report.to = to;
    let next = if report.truncated {
        newest_seen.unwrap_or(from).clamp(from, to)
    } else {
        to
    };
    db.set_cursor(src.key, &next.to_string(), now).await?;
    Ok(report)
}

/// Read, decode and store `[from, to)` of one source (no cursor change);
/// returns the report and the newest block time seen.
async fn scan_window(
    db: &DevDb,
    provider: &dyn HistoryProvider,
    src: &SolanaSource,
    from: i64,
    to: i64,
) -> Result<(SolanaIngestReport, Option<i64>), SolanaIngestError> {
    let mut report = SolanaIngestReport {
        from,
        to,
        ..SolanaIngestReport::default()
    };
    let task = ScanTask {
        request: ScanRequest::WalletActivity {
            wallet: wallet(src.address)?,
        },
        description: format!("dev-tracker: {}", src.key),
    };
    let mut stream =
        provider.scan_block_time_range(task, CancellationToken::new(), Some(from), Some(to));
    let (mut launches, mut migrations) = (Vec::new(), Vec::new());
    let mut newest_seen: Option<i64> = None;
    while let Some(item) = stream.next().await {
        let envelope = item?;
        report.truncated |= envelope.truncated;
        let RawPayload::SolanaTransaction(tx) = &envelope.payload else {
            continue;
        };
        report.transactions += 1;
        if let Some(t) = tx.block_time {
            newest_seen = Some(newest_seen.map_or(t, |n| n.max(t)));
        }
        let (l, m, bad) = facts_of(tx, src.kind, src.key);
        report.decoded += l.len() + m.len();
        report.undecodable += bad;
        launches.extend(l);
        migrations.extend(m);
        if launches.len() + migrations.len() >= FLUSH_ROWS {
            report.inserted += db.insert_launches(&launches).await?;
            report.inserted += db.insert_migrations(&migrations).await?;
            launches.clear();
            migrations.clear();
        }
    }
    report.inserted += db.insert_launches(&launches).await?;
    report.inserted += db.insert_migrations(&migrations).await?;
    Ok((report, newest_seen))
}

const DAY: i64 = 86_400;

/// Cursor key marking one backfilled UTC day of a source.
#[must_use]
pub fn backfill_day_key(src: &SolanaSource, day_start: i64) -> String {
    format!("backfill:{}:{day_start}", src.key)
}

/// UTC day starts of the backfill window `[now − days, today)`, newest first.
#[must_use]
pub fn backfill_days(now: i64, days: i64) -> Vec<i64> {
    let today = now.div_euclid(DAY).saturating_mul(DAY);
    (1..=days.max(0))
        .map(|i| today.saturating_sub(i.saturating_mul(DAY)))
        .collect()
}

/// Backfill one UTC day of one source. `Ok(None)` = already done. A day is
/// marked done only when read completely; a day cut by the page budget is
/// returned with `truncated` and read again next time (idempotent inserts).
///
/// # Errors
/// Provider or database failure (the day stays not done).
pub async fn backfill_solana_day(
    db: &DevDb,
    provider: &dyn HistoryProvider,
    src: &SolanaSource,
    day_start: i64,
    now: i64,
) -> Result<Option<SolanaIngestReport>, SolanaIngestError> {
    let key = backfill_day_key(src, day_start);
    if db.cursor(&key).await?.is_some() {
        return Ok(None);
    }
    let (report, _) =
        scan_window(db, provider, src, day_start, day_start.saturating_add(DAY)).await?;
    if !report.truncated {
        db.set_cursor(&key, "done", now).await?;
    }
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;
    use scout_core::RawSolanaInstruction;
    use scout_dex_solana::{
        COMPLETE_PUMP_AMM_MIGRATION_EVENT_DISCRIMINATOR, CREATE_EVENT_DISCRIMINATOR,
        EVENT_CPI_DISCRIMINATOR, PUMP_PROGRAM_ID_BYTES,
    };

    fn event(disc: [u8; 8], payload: Vec<u8>) -> RawSolanaInstruction {
        let mut data = EVENT_CPI_DISCRIMINATOR.to_vec();
        data.extend(disc);
        data.extend(payload);
        RawSolanaInstruction {
            program_id: PUMP_PROGRAM_ID_BYTES,
            accounts: vec![],
            data,
            instruction_index: 2,
        }
    }

    fn create_payload() -> Vec<u8> {
        let mut p = Vec::new();
        for s in ["N", "S", "U"] {
            p.extend(1u32.to_le_bytes());
            p.extend(s.as_bytes());
        }
        for b in [1u8, 2, 3, 4] {
            p.extend([b; 32]);
        }
        p.extend(99i64.to_le_bytes());
        p
    }

    fn migration_payload() -> Vec<u8> {
        let mut p = [5u8; 32].to_vec();
        p.extend([6u8; 32]);
        p.extend([0u8; 24]);
        p.extend([7u8; 32]);
        p.extend(77i64.to_le_bytes());
        p.extend([8u8; 32]);
        p
    }

    fn tx(instructions: Vec<RawSolanaInstruction>, ok: bool) -> RawSolanaTransaction {
        RawSolanaTransaction {
            block_time: Some(1_000),
            signature: [9; 64],
            execution: if ok {
                SolanaExecutionStatus::Succeeded
            } else {
                SolanaExecutionStatus::Failed {
                    error: "x".to_string(),
                }
            },
            slot: 123,
            transaction_index: 0,
            instructions,
            token_balance_changes: vec![],
            fee_lamports: 5_000,
            fee_payer: [0; 32],
            signers: vec![],
            native_balance_changes: vec![],
            log_messages: None,
        }
    }

    #[test]
    fn launches_and_migrations_from_events() {
        let t = tx(
            vec![
                event(CREATE_EVENT_DISCRIMINATOR, create_payload()),
                event(
                    COMPLETE_PUMP_AMM_MIGRATION_EVENT_DISCRIMINATOR,
                    migration_payload(),
                ),
                event(CREATE_EVENT_DISCRIMINATOR, vec![1, 2]),
            ],
            true,
        );
        let (l, m, bad) = facts_of(&t, SolanaFact::Launch, "k");
        assert_eq!(m.len(), 0, "a launch source stores launches only");
        assert_eq!(bad, 1);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].token, b58(&[1; 32]));
        assert_eq!(l[0].creator, b58(&[4; 32]));
        assert_eq!(
            l[0].created_at, 1_000,
            "block time wins over the event clock"
        );
        assert_eq!(l[0].created_block, 123);
        assert_eq!(l[0].tx_hash, b58(&[9; 64]));
        let (l, m, _) = facts_of(&t, SolanaFact::Migration, "k");
        assert!(l.is_empty());
        assert_eq!(m[0].token, b58(&[6; 32]));
        assert_eq!(m[0].pool.as_deref(), Some(b58(&[8; 32]).as_str()));
    }

    #[test]
    fn failed_transactions_carry_no_facts() {
        let t = tx(
            vec![event(CREATE_EVENT_DISCRIMINATOR, create_payload())],
            false,
        );
        assert_eq!(facts_of(&t, SolanaFact::Launch, "k"), (vec![], vec![], 0));
    }

    #[test]
    fn backfill_days_are_whole_utc_days_newest_first() {
        let now = 1_791_400_000; // 2026-10-07T19:06Z
        let d = backfill_days(now, 3);
        assert_eq!(d, [1_791_244_800, 1_791_158_400, 1_791_072_000]);
        assert!(d.iter().all(|x| x % 86_400 == 0));
        assert!(backfill_days(now, 0).is_empty());
    }

    #[test]
    fn source_addresses_are_valid() {
        for s in SOLANA_SOURCES {
            wallet(s.address).unwrap();
        }
    }
}
