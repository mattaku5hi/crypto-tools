//! EVM launch / migration ingestion (B2, ADR-021): one `eth_getLogs` pass per
//! source (chain + launchpad emitter + event) from its cursor to the head
//! minus confirmations; decoded facts are inserted idempotently, then the
//! cursor moves. A crash between the insert and the cursor update only
//! replays already-known facts.
//!
//! Creator fields per launchpad (B0 measurements, topic0 = keccak of the
//! signature): Flap `TokenCreated.creator`, four.meme `TokenCreate.creator`,
//! Pons `TokenLaunched` topic 3 (`originalDeployer`), Zora `CoinCreatedV4` /
//! `CreatorCoinCreated` topic 2 (`payoutRecipient`), Zora `TrendCoinCreated`
//! topic 1 (`caller`), Clanker v4 `TokenCreated` topic 2 (`tokenAdmin`), Robinhood Doppler `Create`
//! (no creator field): `tx.from`, or the smart account of the `UserOperationEvent` when the
//! transaction went to an ERC-4337 EntryPoint (one receipt per launch).

use std::collections::BTreeMap;

use alloy_primitives::{Address, B256, address, b256};
use scout_api::DecodeOutcome;
use scout_core::RawEvmLog;
use scout_devdb::{DevDb, DevDbError, Launch, Migration};
use scout_dex_evm::{
    FLAP_LAUNCHED_TO_DEX_TOPIC0, FLAP_PORTAL_BSC, FLAP_TOKEN_CREATED_TOPIC0,
    FOURMEME_TOKEN_CREATE_TOPIC0, decode_flap_launched_to_dex, decode_flap_token_created,
    decode_fourmeme_token_create,
};
use scout_providers::{EvmReceiptInfo, EvmRpcClient, EvmSourceError, LogFilter};

/// Flap Portal on Robinhood Chain (Bitquery's Flap.sh API docs).
pub const FLAP_PORTAL_ROBINHOOD: Address = address!("26605f322f7ff986f381bb9a6e3f5dab0beaeb09");
/// Pons V2 launch factory (Robinhood, ADR-020 amendment 8).
pub const PONS_FACTORY: Address = address!("7eD598BcEf8bd9Edd8C97A195C6d13f40801EC7e");
/// four.meme TokenManager V1 and TokenManager2 V2 (BSC).
pub const FOURMEME_MANAGERS: [Address; 2] = [
    address!("EC4549caDcE5DA21Df6E6422d448034B5233bFbC"),
    address!("5c952063c7fc8610FFDB798152D69F0B9550762b"),
];
/// Zora coin factory (Base).
pub const ZORA_FACTORY: Address = address!("777777751622c0d3258f214F9DF38E35BF45baF3");
/// Clanker v4 factory (Base).
pub const CLANKER_V4: Address = address!("E85A59c628F7d27878ACeB4bf3b35733630083a9");

/// four.meme TokenManager2 `LiquidityAdded(address base, uint256 offers,
/// address quote, uint256 funds)` — the graduation: emitted right after the
/// PancakeSwap `PairCreated` in the buy that completes the curve (3/3 sampled
/// migrations, 2026-10-07; ≈ 4/day, matching Codex's migrated list).
pub const FOURMEME_LIQUIDITY_ADDED_TOPIC0: B256 =
    b256!("c18aa71171b358b706fe3dd345299685ba21a5316c66ffa9e319268b033c44b0");
/// Doppler Airlock on Robinhood (Bitquery; 2026-10-07 measurements addendum).
pub const DOPPLER_AIRLOCK_ROBINHOOD: Address = address!("eb7c034704ef8dcd2d32324c1545f62fb4ad0862");
/// Doppler `Create(address asset, address indexed numeraire, address
/// initializer, address poolOrHook)` — no creator field.
pub const DOPPLER_CREATE_TOPIC0: B256 =
    b256!("68ff1cfcdcf76864161555fc0de1878d8f83ec6949bf351df74d8a4a1a2679ab");
/// ERC-4337 EntryPoints v0.7 and v0.6: a create sent through them is signed by
/// a bundler; the creator is the smart account of its `UserOperationEvent`.
pub const ENTRY_POINTS: [Address; 2] = [
    address!("0000000071727De22E5E9d8BAf0edAc6f37da032"),
    address!("5FF137D4b0FDCD49DcA30c7CF57E578a026d2789"),
];
/// `UserOperationEvent(bytes32 indexed userOpHash, address indexed sender,
/// address indexed paymaster, uint256 nonce, bool success, uint256
/// actualGasCost, uint256 actualGasUsed)`.
pub const USER_OPERATION_EVENT_TOPIC0: B256 =
    b256!("49628fd1471006c1482da88028e9ce4dbb080b815c9b0344d39e5a8e6ec1419f");
/// `TokenLaunched(address,address,address,address,uint256,uint256)`.
pub const PONS_TOKEN_LAUNCHED_TOPIC0: B256 =
    b256!("8d4aad4953d0ca700d468f3753aa14432d1b35b43ec6409f051fb6aa43a89607");
/// `PoolGraduated(address,uint256,uint256,uint256)`.
pub const PONS_POOL_GRADUATED_TOPIC0: B256 =
    b256!("0a44ef75df69c534f43cd6c1aa3ef8983065fe5fe79ef9e79f6494e6f258c259");
/// Zora `CoinCreatedV4(…)`.
pub const ZORA_COIN_CREATED_V4_TOPIC0: B256 =
    b256!("2de436107c2096e039c98bbcc3c5a2560583738ce15c234557eecb4d3221aa81");
/// Zora `CreatorCoinCreated(…)`.
pub const ZORA_CREATOR_COIN_CREATED_TOPIC0: B256 =
    b256!("74b670d628e152daa36ca95dda7cb0002d6ea7a37b55afe4593db7abd1515781");
/// Zora `TrendCoinCreated(…)`.
pub const ZORA_TREND_COIN_CREATED_TOPIC0: B256 =
    b256!("fb9e81c36dd4134d4eb5055e20d3238cb29a63daef0dd0f8b0f131da85891959");
/// Clanker v4 `TokenCreated(…)`.
pub const CLANKER_V4_TOKEN_CREATED_TOPIC0: B256 =
    b256!("9299d1d1a88d8e1abdc591ae7a167a6bc63a8f17d695804e9091ee33aa89fb67");

/// What a source's event yields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    Launch,
    Migration,
}

/// One decoded fact before it gets its block time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFact {
    pub token: Address,
    /// Launch: creator. Migration: the DEX pool, when the event names it.
    pub party: Option<Address>,
    /// Event's own timestamp (Flap) — else interpolated from block numbers.
    pub timestamp: Option<u64>,
}

/// Decoder of one source's event.
pub type FactDecoder = fn(&RawEvmLog) -> Option<DecodedFact>;

/// One ingestion source.
#[derive(Debug, Clone, Copy)]
pub struct EvmSource {
    /// Cursor key, e.g. `bsc:flap:launch`.
    pub key: &'static str,
    pub chain: &'static str,
    pub launchpad: &'static str,
    pub kind: FactKind,
    pub emitters: &'static [Address],
    pub topic0: B256,
    pub decode: FactDecoder,
    /// The event names no creator: it is read from the transaction receipt
    /// ([`creator_from_receipt`]).
    pub creator_from_tx: bool,
}

fn topic_address(log: &RawEvmLog, i: usize) -> Option<Address> {
    let t = log.topics.get(i)?;
    t.0.get(..12)?
        .iter()
        .all(|b| *b == 0)
        .then(|| Address::from_word(*t))
}

fn data_address(log: &RawEvmLog, word: usize) -> Option<Address> {
    let w = log
        .data
        .get(word.checked_mul(32)?..word.checked_mul(32)?.checked_add(32)?)?;
    let (pad, addr) = (w.get(..12)?, w.get(12..)?);
    pad.iter()
        .all(|b| *b == 0)
        .then(|| Address::from_slice(addr))
}

fn flap_launch(log: &RawEvmLog) -> Option<DecodedFact> {
    match decode_flap_token_created(log) {
        DecodeOutcome::Decoded(c) => Some(DecodedFact {
            token: c.token,
            party: Some(c.creator),
            timestamp: u64::try_from(c.timestamp).ok(),
        }),
        _ => None,
    }
}

fn flap_migration(log: &RawEvmLog) -> Option<DecodedFact> {
    match decode_flap_launched_to_dex(log) {
        DecodeOutcome::Decoded(m) => Some(DecodedFact {
            token: m.token,
            party: Some(m.pool),
            timestamp: None,
        }),
        _ => None,
    }
}

fn doppler_launch(log: &RawEvmLog) -> Option<DecodedFact> {
    (log.data.len() == 3 * 32 && log.topics.len() == 2).then_some(())?;
    Some(DecodedFact {
        token: data_address(log, 0)?,
        party: None,
        timestamp: None,
    })
}

/// The creator behind a launch event at `log_index` of a transaction: the
/// smart account of the first `UserOperationEvent` after that log when the
/// transaction went to an ERC-4337 EntryPoint (each user operation's event
/// follows its own execution), else the transaction's `from`.
#[must_use]
pub fn creator_from_receipt(receipt: &EvmReceiptInfo, log_index: u64) -> Option<Address> {
    if receipt.to.is_some_and(|t| ENTRY_POINTS.contains(&t)) {
        receipt
            .logs
            .iter()
            .filter(|l| {
                ENTRY_POINTS.contains(&l.address)
                    && l.topics.first() == Some(&USER_OPERATION_EVENT_TOPIC0)
                    && l.log_index > log_index
            })
            .min_by_key(|l| l.log_index)
            .and_then(|l| topic_address(l, 2))
    } else {
        receipt.from
    }
}

fn fourmeme_migration(log: &RawEvmLog) -> Option<DecodedFact> {
    (log.data.len() == 4 * 32 && log.topics.len() == 1).then_some(())?;
    Some(DecodedFact {
        token: data_address(log, 0)?,
        party: None,
        timestamp: None,
    })
}

fn fourmeme_launch(log: &RawEvmLog) -> Option<DecodedFact> {
    match decode_fourmeme_token_create(log) {
        DecodeOutcome::Decoded(c) => Some(DecodedFact {
            token: c.token,
            party: Some(c.creator),
            timestamp: None,
        }),
        _ => None,
    }
}

fn pons_launch(log: &RawEvmLog) -> Option<DecodedFact> {
    Some(DecodedFact {
        token: topic_address(log, 1)?,
        party: Some(topic_address(log, 3)?),
        timestamp: None,
    })
}

fn pons_migration(log: &RawEvmLog) -> Option<DecodedFact> {
    Some(DecodedFact {
        token: topic_address(log, 1)?,
        party: None,
        timestamp: None,
    })
}

fn zora_coin(log: &RawEvmLog) -> Option<DecodedFact> {
    // head: currency, uri*, name*, symbol*, coin, PoolKey(5), poolKeyHash, version*
    Some(DecodedFact {
        token: data_address(log, 4)?,
        party: Some(topic_address(log, 2)?),
        timestamp: None,
    })
}

fn zora_trend(log: &RawEvmLog) -> Option<DecodedFact> {
    // head: symbol*, coin, PoolKey(5), poolKeyHash, poolConfig*, version*
    Some(DecodedFact {
        token: data_address(log, 1)?,
        party: Some(topic_address(log, 1)?),
        timestamp: None,
    })
}

fn clanker_launch(log: &RawEvmLog) -> Option<DecodedFact> {
    Some(DecodedFact {
        token: topic_address(log, 1)?,
        party: Some(topic_address(log, 2)?),
        timestamp: None,
    })
}

/// Every EVM source of the dev tracker (B0).
pub const EVM_SOURCES: &[EvmSource] = &[
    EvmSource {
        key: "bsc:flap:launch",
        chain: "bsc",
        launchpad: "flap",
        kind: FactKind::Launch,
        emitters: &[FLAP_PORTAL_BSC],
        topic0: FLAP_TOKEN_CREATED_TOPIC0,
        decode: flap_launch,
        creator_from_tx: false,
    },
    EvmSource {
        key: "bsc:flap:migration",
        chain: "bsc",
        launchpad: "flap",
        kind: FactKind::Migration,
        emitters: &[FLAP_PORTAL_BSC],
        topic0: FLAP_LAUNCHED_TO_DEX_TOPIC0,
        decode: flap_migration,
        creator_from_tx: false,
    },
    EvmSource {
        key: "bsc:fourmeme:launch",
        chain: "bsc",
        launchpad: "fourmeme",
        kind: FactKind::Launch,
        emitters: &FOURMEME_MANAGERS,
        topic0: FOURMEME_TOKEN_CREATE_TOPIC0,
        decode: fourmeme_launch,
        creator_from_tx: false,
    },
    EvmSource {
        key: "bsc:fourmeme:migration",
        chain: "bsc",
        launchpad: "fourmeme",
        kind: FactKind::Migration,
        emitters: &FOURMEME_MANAGERS,
        topic0: FOURMEME_LIQUIDITY_ADDED_TOPIC0,
        decode: fourmeme_migration,
        creator_from_tx: false,
    },
    EvmSource {
        key: "robinhood:pons:launch",
        chain: "robinhood",
        launchpad: "pons",
        kind: FactKind::Launch,
        emitters: &[PONS_FACTORY],
        topic0: PONS_TOKEN_LAUNCHED_TOPIC0,
        decode: pons_launch,
        creator_from_tx: false,
    },
    EvmSource {
        key: "robinhood:pons:migration",
        chain: "robinhood",
        launchpad: "pons",
        kind: FactKind::Migration,
        emitters: &[PONS_FACTORY],
        topic0: PONS_POOL_GRADUATED_TOPIC0,
        decode: pons_migration,
        creator_from_tx: false,
    },
    EvmSource {
        key: "robinhood:flap:launch",
        chain: "robinhood",
        launchpad: "flap",
        kind: FactKind::Launch,
        emitters: &[FLAP_PORTAL_ROBINHOOD],
        topic0: FLAP_TOKEN_CREATED_TOPIC0,
        decode: flap_launch,
        creator_from_tx: false,
    },
    EvmSource {
        key: "robinhood:flap:migration",
        chain: "robinhood",
        launchpad: "flap",
        kind: FactKind::Migration,
        emitters: &[FLAP_PORTAL_ROBINHOOD],
        topic0: FLAP_LAUNCHED_TO_DEX_TOPIC0,
        decode: flap_migration,
        creator_from_tx: false,
    },
    EvmSource {
        key: "base:zora:coin",
        chain: "base",
        launchpad: "zora",
        kind: FactKind::Launch,
        emitters: &[ZORA_FACTORY],
        topic0: ZORA_COIN_CREATED_V4_TOPIC0,
        decode: zora_coin,
        creator_from_tx: false,
    },
    EvmSource {
        key: "base:zora:creator-coin",
        chain: "base",
        launchpad: "zora",
        kind: FactKind::Launch,
        emitters: &[ZORA_FACTORY],
        topic0: ZORA_CREATOR_COIN_CREATED_TOPIC0,
        decode: zora_coin,
        creator_from_tx: false,
    },
    EvmSource {
        key: "base:zora:trend-coin",
        chain: "base",
        launchpad: "zora",
        kind: FactKind::Launch,
        emitters: &[ZORA_FACTORY],
        topic0: ZORA_TREND_COIN_CREATED_TOPIC0,
        decode: zora_trend,
        creator_from_tx: false,
    },
    EvmSource {
        key: "base:clanker:launch",
        chain: "base",
        launchpad: "clanker",
        kind: FactKind::Launch,
        emitters: &[CLANKER_V4],
        topic0: CLANKER_V4_TOKEN_CREATED_TOPIC0,
        decode: clanker_launch,
        creator_from_tx: false,
    },
    EvmSource {
        key: "robinhood:doppler:launch",
        chain: "robinhood",
        launchpad: "doppler",
        kind: FactKind::Launch,
        emitters: &[DOPPLER_AIRLOCK_ROBINHOOD],
        topic0: DOPPLER_CREATE_TOPIC0,
        decode: doppler_launch,
        creator_from_tx: true,
    },
];

/// Failure of an ingestion pass.
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("rpc: {0}")]
    Rpc(#[from] EvmSourceError),
    #[error("{0}")]
    Db(#[from] DevDbError),
    #[error("cursor of {0} is not a block number")]
    BadCursor(String),
}

/// Result of one pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IngestReport {
    pub from_block: u64,
    pub to_block: u64,
    pub logs: usize,
    pub decoded: usize,
    pub inserted: u64,
    pub undecodable: usize,
}

/// Linear block-time estimate between two known `(block, unix)` points.
fn interpolate(block: u64, a: (u64, u64), b: (u64, u64)) -> u64 {
    if b.0 <= a.0 {
        return a.1;
    }
    let span_b = u128::from(b.0 - a.0);
    let span_t = u128::from(b.1.saturating_sub(a.1));
    let off = u128::from(block.saturating_sub(a.0));
    let t = span_t.saturating_mul(off).checked_div(span_b).unwrap_or(0);
    a.1.saturating_add(u64::try_from(t).unwrap_or(0))
}

/// Ingest one source up to `head - confirmations`, starting after its cursor
/// (or at `start_block` when it has none).
///
/// # Errors
/// RPC or database failure (the cursor only moves after the facts are stored).
pub async fn ingest_source(
    db: &DevDb,
    rpc: &EvmRpcClient,
    src: &EvmSource,
    head: u64,
    confirmations: u64,
    start_block: u64,
    now: i64,
) -> Result<IngestReport, IngestError> {
    let from = match db.cursor(src.key).await? {
        Some(c) => c
            .parse::<u64>()
            .map_err(|_| IngestError::BadCursor(src.key.to_string()))?
            .saturating_add(1),
        None => start_block,
    };
    let to = head.saturating_sub(confirmations);
    if from > to {
        return Ok(IngestReport {
            from_block: from,
            to_block: to,
            ..IngestReport::default()
        });
    }
    let report = ingest_range(db, rpc, src, from, to).await?;
    db.set_cursor(src.key, &to.to_string(), now).await?;
    Ok(report)
}

/// Cursor key of a source's backfill: the lowest block already backfilled.
#[must_use]
pub fn backfill_key(src: &EvmSource) -> String {
    format!("backfill:{}", src.key)
}

/// One backfill step of a source: the `step` blocks below its backfill cursor
/// (first call: below the forward cursor, i.e. where live data starts), never
/// below `floor`. `Ok(None)` when the source is backfilled down to `floor`.
/// Facts are idempotent, so the overlap with live data is harmless.
///
/// # Errors
/// RPC or database failure (the backfill cursor moves only after the facts
/// are stored, so a stopped backfill resumes where it left off).
pub async fn backfill_step(
    db: &DevDb,
    rpc: &EvmRpcClient,
    src: &EvmSource,
    floor: u64,
    step: u64,
    head: u64,
    now: i64,
) -> Result<Option<IngestReport>, IngestError> {
    let key = backfill_key(src);
    let parse = |c: String| {
        c.parse::<u64>()
            .map_err(|_| IngestError::BadCursor(key.clone()))
    };
    let top = match db.cursor(&key).await? {
        Some(c) => parse(c)?,
        None => match db.cursor(src.key).await? {
            Some(c) => parse(c)?.saturating_add(1),
            None => head,
        },
    };
    if top <= floor {
        return Ok(None);
    }
    let from = top.saturating_sub(step.max(1)).max(floor);
    let report = ingest_range(db, rpc, src, from, top.saturating_sub(1)).await?;
    db.set_cursor(&key, &from.to_string(), now).await?;
    Ok(Some(report))
}

/// Fetch, decode and store one source's facts in blocks `from..=to` (no
/// cursor change).
///
/// # Errors
/// RPC or database failure.
pub async fn ingest_range(
    db: &DevDb,
    rpc: &EvmRpcClient,
    src: &EvmSource,
    from: u64,
    to: u64,
) -> Result<IngestReport, IngestError> {
    let mut report = IngestReport {
        from_block: from,
        to_block: to,
        ..IngestReport::default()
    };
    if from > to {
        return Ok(report);
    }
    let filter = LogFilter {
        addresses: src.emitters.to_vec(),
        topics: [Some(vec![src.topic0]), None, None, None],
    };
    let out = rpc.get_logs(&filter, from, to).await?;
    report.logs = out.logs.len();
    let decoded: Vec<(&RawEvmLog, DecodedFact)> = out
        .logs
        .iter()
        .filter_map(|l| (src.decode)(l).map(|f| (l, f)))
        .collect();
    report.undecodable = report.logs - decoded.len();
    report.decoded = decoded.len();
    // Block times: the event's own timestamp, else interpolation between the
    // first and last block of this batch (two timestamp reads per pass).
    let needs_time: Vec<u64> = decoded
        .iter()
        .filter(|(_, f)| f.timestamp.is_none())
        .map(|(l, _)| l.block_number)
        .collect();
    let mut known: BTreeMap<u64, u64> = BTreeMap::new();
    if let (Some(lo), Some(hi)) = (needs_time.iter().min(), needs_time.iter().max()) {
        known = rpc.block_timestamps(&[*lo, *hi]).await?;
    }
    let time_of = |block: u64, own: Option<u64>| -> i64 {
        let t = own.unwrap_or_else(|| {
            let mut it = known.iter();
            match (it.next(), it.next_back()) {
                (Some((&a, &ta)), Some((&b, &tb))) => interpolate(block, (a, ta), (b, tb)),
                (Some((_, &ta)), None) => ta,
                _ => 0,
            }
        });
        i64::try_from(t).unwrap_or(i64::MAX)
    };
    let hash_of = |l: &RawEvmLog| {
        out.tx_hashes
            .get(&(l.block_number, l.transaction_index))
            .map_or_else(String::new, |h| format!("{h:#x}"))
    };
    let block_i64 = |b: u64| i64::try_from(b).unwrap_or(i64::MAX);
    report.inserted = match src.kind {
        FactKind::Launch => {
            // creators named only by the transaction (Doppler): one receipt each
            let mut from_tx: BTreeMap<(u64, u64), Address> = BTreeMap::new();
            if src.creator_from_tx {
                let mut wanted: Vec<(&RawEvmLog, B256)> = Vec::new();
                for (l, _) in decoded.iter().filter(|(_, f)| f.party.is_none()) {
                    if let Some(h) = out.tx_hashes.get(&(l.block_number, l.transaction_index)) {
                        wanted.push((l, *h));
                    }
                }
                let hashes: Vec<B256> = wanted.iter().map(|(_, h)| *h).collect();
                let receipts = rpc.receipts_by_hashes(&hashes).await?;
                for ((l, _), r) in wanted.iter().zip(&receipts) {
                    if let Some(c) = creator_from_receipt(r, l.log_index) {
                        from_tx.insert((l.block_number, l.log_index), c);
                    }
                }
            }
            let rows: Vec<Launch> = decoded
                .iter()
                .filter_map(|(l, f)| {
                    let creator = f
                        .party
                        .or_else(|| from_tx.get(&(l.block_number, l.log_index)).copied())?;
                    Some(Launch {
                        chain: src.chain.to_string(),
                        token: format!("{:#x}", f.token),
                        launchpad: src.launchpad.to_string(),
                        creator: format!("{creator:#x}"),
                        created_block: block_i64(l.block_number),
                        created_at: time_of(l.block_number, f.timestamp),
                        tx_hash: hash_of(l),
                        source: src.key.to_string(),
                    })
                })
                .collect();
            db.insert_launches(&rows).await?
        }
        FactKind::Migration => {
            let rows: Vec<Migration> = decoded
                .iter()
                .map(|(l, f)| Migration {
                    chain: src.chain.to_string(),
                    token: format!("{:#x}", f.token),
                    launchpad: src.launchpad.to_string(),
                    migrated_block: block_i64(l.block_number),
                    migrated_at: time_of(l.block_number, f.timestamp),
                    tx_hash: hash_of(l),
                    pool: f.party.map(|p| format!("{p:#x}")),
                    source: src.key.to_string(),
                })
                .collect();
            db.insert_migrations(&rows).await?
        }
    };
    Ok(report)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn doppler_creator_is_the_sender_or_the_user_operation_account() {
        let word = |a: Address| B256::left_padding_from(a.as_slice());
        let log = |address: Address, topics: Vec<B256>, log_index: u64| RawEvmLog {
            address,
            topics,
            data: alloy_primitives::Bytes::new(),
            block_number: 1,
            transaction_index: 0,
            log_index,
        };
        let uop = |sender: Address, i: u64| {
            log(
                ENTRY_POINTS[0],
                vec![
                    USER_OPERATION_EVENT_TOPIC0,
                    B256::ZERO,
                    word(sender),
                    B256::ZERO,
                ],
                i,
            )
        };
        let (eoa, bundler, alice, bob) = (
            address!("1111111111111111111111111111111111111111"),
            address!("2222222222222222222222222222222222222222"),
            address!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            address!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        );
        let receipt = |to: Address, logs: Vec<RawEvmLog>| EvmReceiptInfo {
            tx_hash: B256::ZERO,
            block_number: 1,
            transaction_index: 0,
            status: scout_core::EvmTxStatus::Success,
            gas_used: 0,
            effective_gas_price: None,
            l1_fee: None,
            logs,
            from: Some(bundler),
            to: Some(to),
        };
        let mut direct = receipt(DOPPLER_AIRLOCK_ROBINHOOD, vec![]);
        direct.from = Some(eoa);
        assert_eq!(creator_from_receipt(&direct, 3), Some(eoa));
        // a bundle of two user operations: each create belongs to the next event
        let bundle = receipt(ENTRY_POINTS[0], vec![uop(alice, 5), uop(bob, 9)]);
        assert_eq!(creator_from_receipt(&bundle, 2), Some(alice));
        assert_eq!(creator_from_receipt(&bundle, 7), Some(bob));
        assert_eq!(creator_from_receipt(&bundle, 12), None, "never the bundler");
    }

    #[test]
    fn fourmeme_graduation_names_the_base_token() {
        // live LiquidityAdded of 0x7f81…ffff (GameStop), BSC 2026-10-06
        let data = alloy_primitives::hex::decode(concat!(
            "0000000000000000000000007f81e5709dfd12ff706f9166c21c07b13773ffff",
            "000000000000000000000000000000000000000000a56fa5b99019a5c8000000",
            "00000000000000000000000055d398326f99059ff775485246999027b3197955",
            "00000000000000000000000000000000000000000000027d82c8dcd8e9e08a04",
        ))
        .unwrap();
        let mut log = RawEvmLog {
            address: FOURMEME_MANAGERS[1],
            topics: vec![FOURMEME_LIQUIDITY_ADDED_TOPIC0],
            data: data.into(),
            block_number: 1,
            transaction_index: 0,
            log_index: 0,
        };
        assert_eq!(
            fourmeme_migration(&log).unwrap().token,
            address!("7f81e5709dfd12ff706f9166c21c07b13773ffff")
        );
        log.topics.push(B256::ZERO);
        assert!(fourmeme_migration(&log).is_none(), "shape is checked");
    }

    #[test]
    fn interpolation_is_linear_and_bounded() {
        assert_eq!(interpolate(150, (100, 1_000), (200, 2_000)), 1_500);
        assert_eq!(interpolate(100, (100, 1_000), (200, 2_000)), 1_000);
        assert_eq!(interpolate(150, (100, 1_000), (100, 1_000)), 1_000);
    }

    #[test]
    fn every_source_topic_is_the_keccak_of_its_signature() {
        let k = |s: &str| alloy_primitives::keccak256(s.as_bytes());
        let pk = "(address,address,uint24,int24,address)";
        assert_eq!(
            DOPPLER_CREATE_TOPIC0,
            k("Create(address,address,address,address)")
        );
        assert_eq!(
            USER_OPERATION_EVENT_TOPIC0,
            k("UserOperationEvent(bytes32,address,address,uint256,bool,uint256,uint256)")
        );
        assert_eq!(
            FOURMEME_LIQUIDITY_ADDED_TOPIC0,
            k("LiquidityAdded(address,uint256,address,uint256)")
        );
        assert_eq!(
            PONS_TOKEN_LAUNCHED_TOPIC0,
            k("TokenLaunched(address,address,address,address,uint256,uint256)")
        );
        assert_eq!(
            PONS_POOL_GRADUATED_TOPIC0,
            k("PoolGraduated(address,uint256,uint256,uint256)")
        );
        assert_eq!(
            ZORA_COIN_CREATED_V4_TOPIC0,
            k(&format!(
                "CoinCreatedV4(address,address,address,address,string,string,string,address,{pk},bytes32,string)"
            ))
        );
        assert_eq!(
            ZORA_CREATOR_COIN_CREATED_TOPIC0,
            k(&format!(
                "CreatorCoinCreated(address,address,address,address,string,string,string,address,{pk},bytes32,string)"
            ))
        );
        assert_eq!(
            ZORA_TREND_COIN_CREATED_TOPIC0,
            k(&format!(
                "TrendCoinCreated(address,string,address,{pk},bytes32,bytes,string)"
            ))
        );
        assert_eq!(
            CLANKER_V4_TOKEN_CREATED_TOPIC0,
            k(
                "TokenCreated(address,address,address,string,string,string,string,string,int24,address,bytes32,address,address,address,uint256,address[])"
            )
        );
    }
}
