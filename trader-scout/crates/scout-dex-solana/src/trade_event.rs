//! pump.fun `TradeEvent` decoder (Anchor event-CPI) and trade/event pairing.
//!
//! Every pump.fun trade makes an inner self-invocation of
//! `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` whose data is
//! `EVENT_CPI_DISCRIMINATOR (8) ++ event discriminator (8) ++ Borsh event`.
//! Layout source: the pinned IDL (`pump_idl_e0687ae9.json`, `types.TradeEvent`
//! and `types.Shareholder`); unit/integration tests re-read the IDL.
//!
//! ## Length policy (invariants #16, #18; mirrors ADR-009's arg policy)
//!
//! Live event data lengths vary because the program appended fields over
//! time and `ix_name`/`shareholders` are variable-length. So:
//! - the REQUIRED prefix `mint ..= creator_fee` is decoded exactly; a shorter
//!   payload is `Malformed`;
//! - later fields are decoded in IDL order while bytes remain. A field is
//!   `None` iff the buffer ended exactly at the boundary before it; a buffer
//!   ending inside a field is `Malformed` (never guessed);
//! - bytes after the last known field are counted in `trailing_bytes`
//!   (future appended fields), bounded by [`MAX_TRAILING_EVENT_BYTES`];
//! - `ix_name` is bounded ([`MAX_IX_NAME_BYTES`]), must be UTF-8 without
//!   control characters; `shareholders` at most [`MAX_SHAREHOLDERS`].
//!
//! Chain strings are untrusted: `ix_name` is exposed only as a validated
//! `String` and error texts never embed raw event bytes.
//!
//! ## Pairing
//!
//! The flattened instruction list does not retain top-level group
//! boundaries, only execution order (top-level instruction, then its inner
//! instructions). A trade's own event is the first `TradeEvent` after it
//! and before the next pump trade instruction, which is exactly "same
//! top-level group, after the trade" for the nesting Solana executes. The
//! decoded event must then agree with the instruction on mint, user and
//! side, otherwise the pair is reported as `Mismatch` (never re-paired).

use scout_core::{RawSolanaInstruction, SolanaPubkey};

use crate::bonding_curve_buy::{
    BondingCurveBuyDecoder, DecodedBondingCurveTrade, EVENT_CPI_DISCRIMINATOR,
    PumpInstructionOutcome, TradeSide,
};

/// `TradeEvent` discriminator (IDL `events`).
pub const TRADE_EVENT_DISCRIMINATOR: [u8; 8] = [0xbd, 0xdb, 0x7f, 0xd3, 0x4e, 0xe6, 0x61, 0xee];
/// Upper bound on bytes after the last known field.
pub const MAX_TRAILING_EVENT_BYTES: usize = 256;
/// Upper bound on `ix_name` bytes.
pub const MAX_IX_NAME_BYTES: usize = 64;
/// Upper bound on `shareholders` entries.
pub const MAX_SHAREHOLDERS: usize = 64;
/// Event-CPI header: tag + event discriminator.
pub const EVENT_CPI_HEADER_LEN: usize = 16;
/// Required fixed prefix length (`mint ..= creator_fee`), without header.
pub const TRADE_EVENT_REQUIRED_LEN: usize = 217;

/// All IDL events (name, discriminator), in IDL order.
pub const EVENT_DISCRIMINATORS: [(&str, [u8; 8]); 28] = [
    (
        "AddQuoteControlMintEvent",
        [0xa4, 0xaf, 0x57, 0x4a, 0x58, 0x2d, 0x21, 0x3e],
    ),
    (
        "AdminCtoEvent",
        [0x6e, 0x7c, 0xe2, 0x62, 0xaa, 0xff, 0x11, 0x78],
    ),
    (
        "AdminSetIdlAuthorityEvent",
        [0xf5, 0x3b, 0x46, 0x22, 0x4b, 0xb9, 0x6d, 0x5c],
    ),
    (
        "AdminUpdateTokenIncentivesEvent",
        [0x93, 0xfa, 0x6c, 0x78, 0xf7, 0x1d, 0x43, 0xde],
    ),
    (
        "ClaimCashbackEvent",
        [0xe2, 0xd6, 0xf6, 0x21, 0x07, 0xf2, 0x93, 0xe5],
    ),
    (
        "ClaimTokenIncentivesEvent",
        [0x4f, 0xac, 0xf6, 0x31, 0xcd, 0x5b, 0xce, 0xe8],
    ),
    (
        "CloseUserVolumeAccumulatorEvent",
        [0x92, 0x9f, 0xbd, 0xac, 0x92, 0x58, 0x38, 0xf4],
    ),
    (
        "CollectCreatorFeeEvent",
        [0x7a, 0x02, 0x7f, 0x01, 0x0e, 0xbf, 0x0c, 0xaf],
    ),
    (
        "CompleteEvent",
        [0x5f, 0x72, 0x61, 0x9c, 0xd4, 0x2e, 0x98, 0x08],
    ),
    (
        "CompletePumpAmmMigrationEvent",
        [0xbd, 0xe9, 0x5d, 0xb9, 0x5c, 0x94, 0xea, 0x94],
    ),
    (
        "CreateEvent",
        [0x1b, 0x72, 0xa9, 0x4d, 0xde, 0xeb, 0x63, 0x76],
    ),
    (
        "DistributeCreatorFeesEvent",
        [0xa5, 0x37, 0x81, 0x70, 0x04, 0xb3, 0xca, 0x28],
    ),
    (
        "DistributeFeeToHoldersEvent",
        [0xe3, 0xbe, 0xd7, 0xce, 0xb0, 0xb4, 0xa5, 0x84],
    ),
    (
        "ExtendAccountEvent",
        [0x61, 0x61, 0xd7, 0x90, 0x5d, 0x92, 0x16, 0x7c],
    ),
    (
        "InitUserVolumeAccumulatorEvent",
        [0x86, 0x24, 0x0d, 0x48, 0xe8, 0x65, 0x82, 0xd8],
    ),
    (
        "MigrateBondingCurveCreatorEvent",
        [0x9b, 0xa7, 0x68, 0xdc, 0xd5, 0x6c, 0xf3, 0x03],
    ),
    (
        "MinimumDistributableFeeEvent",
        [0xa8, 0xd8, 0x84, 0xef, 0xeb, 0xb6, 0x31, 0x34],
    ),
    (
        "RemoveQuoteControlMintEvent",
        [0x2e, 0x21, 0x56, 0x86, 0x00, 0xd5, 0xd1, 0x30],
    ),
    (
        "ReservedFeeRecipientsEvent",
        [0x2b, 0xbc, 0xfa, 0x12, 0xdd, 0x4b, 0xbb, 0x5f],
    ),
    (
        "SetCreatorEvent",
        [0xed, 0x34, 0x7b, 0x25, 0xf5, 0xfb, 0x48, 0xd2],
    ),
    (
        "SetMetaplexCreatorEvent",
        [0x8e, 0xcb, 0x06, 0x20, 0x7f, 0x69, 0xbf, 0xa2],
    ),
    (
        "SetParamsEvent",
        [0xdf, 0xc3, 0x9f, 0xf6, 0x3e, 0x30, 0x8f, 0x83],
    ),
    (
        "SetQuoteControlAdminEvent",
        [0x4a, 0xf8, 0x8d, 0x45, 0xca, 0x51, 0x1e, 0xf7],
    ),
    (
        "SyncUserVolumeAccumulatorEvent",
        [0xc5, 0x7a, 0xa7, 0x7c, 0x74, 0x51, 0x5b, 0xff],
    ),
    (
        "TradeEvent",
        [0xbd, 0xdb, 0x7f, 0xd3, 0x4e, 0xe6, 0x61, 0xee],
    ),
    (
        "UpdateCreatorFeeConfigEvent",
        [0x98, 0xc6, 0x7c, 0x7c, 0x6a, 0xf6, 0x7f, 0xbf],
    ),
    (
        "UpdateGlobalAuthorityEvent",
        [0xb6, 0xc3, 0x89, 0x2a, 0x23, 0xce, 0xcf, 0xf7],
    ),
    (
        "UpdateMayhemVirtualParamsEvent",
        [0x75, 0x7b, 0xe4, 0xb6, 0xa1, 0xa8, 0xdc, 0xd6],
    ),
];

/// Look up an event name in [`EVENT_DISCRIMINATORS`].
#[must_use]
pub fn event_name(discriminator: &[u8; 8]) -> Option<&'static str> {
    EVENT_DISCRIMINATORS
        .iter()
        .find(|(_, d)| d == discriminator)
        .map(|(n, _)| *n)
}

/// IDL `Shareholder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shareholder {
    pub address: SolanaPubkey,
    pub share_bps: u16,
}

/// Decoded `TradeEvent`. Fields after `creator_fee` are `None` when the
/// emitted event ended before them (older program layout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeEvent {
    pub mint: SolanaPubkey,
    pub sol_amount: u64,
    pub token_amount: u64,
    pub is_buy: bool,
    pub user: SolanaPubkey,
    pub timestamp: i64,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub fee_recipient: SolanaPubkey,
    pub fee_basis_points: u64,
    pub fee: u64,
    pub creator: SolanaPubkey,
    pub creator_fee_basis_points: u64,
    pub creator_fee: u64,
    pub track_volume: Option<bool>,
    pub total_unclaimed_tokens: Option<u64>,
    pub total_claimed_tokens: Option<u64>,
    pub current_sol_volume: Option<u64>,
    pub last_update_timestamp: Option<i64>,
    pub ix_name: Option<String>,
    pub mayhem_mode: Option<bool>,
    pub cashback_fee_basis_points: Option<u64>,
    pub cashback: Option<u64>,
    pub buyback_fee_basis_points: Option<u64>,
    pub buyback_fee: Option<u64>,
    pub shareholders: Option<Vec<Shareholder>>,
    pub quote_mint: Option<SolanaPubkey>,
    pub quote_amount: Option<u64>,
    pub virtual_quote_reserves: Option<u64>,
    pub real_quote_reserves: Option<u64>,
    pub holder_rewards_bps: Option<u64>,
    pub holder_rewards: Option<u64>,
    /// Name of the last IDL field present in the buffer.
    pub last_field_present: &'static str,
    /// Bytes after the last known field (unknown appended fields).
    pub trailing_bytes: usize,
    /// Total instruction data length (header included), for diagnostics.
    pub data_len: usize,
    pub instruction_index: u32,
}

/// Classification of one instruction as a pump.fun event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpEventOutcome {
    /// Program id is not the confirmed pump.fun program.
    NotMine,
    /// Pump instruction that is not an event-CPI self-invocation.
    NotEventCpi,
    Trade(Box<TradeEvent>),
    /// Event-CPI of an IDL event that is not `TradeEvent`; counted only.
    OtherEvent {
        discriminator: [u8; 8],
        name: &'static str,
    },
    /// Event-CPI whose event discriminator is not in the IDL. COVERAGE GAP.
    UnknownEvent {
        discriminator: [u8; 8],
    },
    /// Event-CPI with a broken structure. COVERAGE GAP.
    Malformed {
        reason: String,
    },
}

/// Classify one instruction ASSUMED to belong to the pump.fun program (no
/// program-id check; use [`BondingCurveBuyDecoder::classify_event`]).
#[must_use]
pub fn classify_pump_event(instruction: &RawSolanaInstruction) -> PumpEventOutcome {
    let data = &instruction.data;
    if data.get(0..8) != Some(EVENT_CPI_DISCRIMINATOR.as_slice()) {
        return PumpEventOutcome::NotEventCpi;
    }
    let Some(disc) = data
        .get(8..EVENT_CPI_HEADER_LEN)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
    else {
        return PumpEventOutcome::Malformed {
            reason: format!(
                "event-CPI instruction has {} data bytes, fewer than the 16-byte header",
                data.len()
            ),
        };
    };
    if disc != TRADE_EVENT_DISCRIMINATOR {
        return match event_name(&disc) {
            Some(name) => PumpEventOutcome::OtherEvent {
                discriminator: disc,
                name,
            },
            None => PumpEventOutcome::UnknownEvent {
                discriminator: disc,
            },
        };
    }
    let payload = data.get(EVENT_CPI_HEADER_LEN..).unwrap_or_default();
    match decode_trade_event(payload, data.len(), instruction.instruction_index) {
        Ok(ev) => PumpEventOutcome::Trade(Box::new(ev)),
        Err(reason) => PumpEventOutcome::Malformed { reason },
    }
}

impl BondingCurveBuyDecoder {
    /// Program-id-gated event classification.
    #[must_use]
    pub fn classify_event(&self, instruction: &RawSolanaInstruction) -> PumpEventOutcome {
        if self.is_program(instruction) {
            classify_pump_event(instruction)
        } else {
            PumpEventOutcome::NotMine
        }
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    ended: bool,
    last: &'static str,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize, field: &'static str) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n);
        let slice = end.and_then(|e| self.buf.get(self.pos..e));
        match slice {
            Some(s) => {
                self.pos += n;
                Ok(s)
            }
            None => Err(format!(
                "TradeEvent ends inside field `{field}` (needs {n} bytes, {} remain)",
                self.remaining()
            )),
        }
    }

    fn u64(&mut self, field: &'static str) -> Result<u64, String> {
        let s = self.take(8, field)?;
        let a: [u8; 8] = s.try_into().map_err(|_| format!("`{field}` not 8 bytes"))?;
        Ok(u64::from_le_bytes(a))
    }

    fn i64(&mut self, field: &'static str) -> Result<i64, String> {
        let s = self.take(8, field)?;
        let a: [u8; 8] = s.try_into().map_err(|_| format!("`{field}` not 8 bytes"))?;
        Ok(i64::from_le_bytes(a))
    }

    fn u32(&mut self, field: &'static str) -> Result<u32, String> {
        let s = self.take(4, field)?;
        let a: [u8; 4] = s.try_into().map_err(|_| format!("`{field}` not 4 bytes"))?;
        Ok(u32::from_le_bytes(a))
    }

    fn u16(&mut self, field: &'static str) -> Result<u16, String> {
        let s = self.take(2, field)?;
        let a: [u8; 2] = s.try_into().map_err(|_| format!("`{field}` not 2 bytes"))?;
        Ok(u16::from_le_bytes(a))
    }

    fn pubkey(&mut self, field: &'static str) -> Result<SolanaPubkey, String> {
        let s = self.take(32, field)?;
        s.try_into().map_err(|_| format!("`{field}` not 32 bytes"))
    }

    fn bool(&mut self, field: &'static str) -> Result<bool, String> {
        match self.take(1, field)? {
            [0] => Ok(false),
            [1] => Ok(true),
            _ => Err(format!("`{field}` is not a Borsh bool (0/1)")),
        }
    }

    fn string(&mut self, field: &'static str) -> Result<String, String> {
        let len = usize::try_from(self.u32(field)?)
            .map_err(|_| format!("`{field}` length does not fit usize"))?;
        if len > MAX_IX_NAME_BYTES {
            return Err(format!(
                "`{field}` length {len} exceeds the {MAX_IX_NAME_BYTES}-byte bound"
            ));
        }
        let bytes = self.take(len, field)?;
        let s = std::str::from_utf8(bytes).map_err(|_| format!("`{field}` is not valid UTF-8"))?;
        if s.chars().any(char::is_control) {
            return Err(format!("`{field}` contains control characters"));
        }
        Ok(s.to_owned())
    }

    fn shareholders(&mut self, field: &'static str) -> Result<Vec<Shareholder>, String> {
        let n = usize::try_from(self.u32(field)?)
            .map_err(|_| format!("`{field}` count does not fit usize"))?;
        if n > MAX_SHAREHOLDERS {
            return Err(format!(
                "`{field}` count {n} exceeds the {MAX_SHAREHOLDERS}-entry bound"
            ));
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(Shareholder {
                address: self.pubkey(field)?,
                share_bps: self.u16(field)?,
            });
        }
        Ok(out)
    }

    /// Optional-tail field: `None` iff the buffer ended at this boundary.
    fn opt<T>(
        &mut self,
        field: &'static str,
        f: impl FnOnce(&mut Self, &'static str) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        if self.ended || self.remaining() == 0 {
            self.ended = true;
            return Ok(None);
        }
        let v = f(self, field)?;
        self.last = field;
        Ok(Some(v))
    }
}

fn decode_trade_event(
    payload: &[u8],
    data_len: usize,
    instruction_index: u32,
) -> Result<TradeEvent, String> {
    if payload.len() < TRADE_EVENT_REQUIRED_LEN {
        return Err(format!(
            "TradeEvent payload has {} bytes, fewer than the {TRADE_EVENT_REQUIRED_LEN} required \
             through `creator_fee`",
            payload.len()
        ));
    }
    let mut r = Reader {
        buf: payload,
        pos: 0,
        ended: false,
        last: "creator_fee",
    };
    let mint = r.pubkey("mint")?;
    let sol_amount = r.u64("sol_amount")?;
    let token_amount = r.u64("token_amount")?;
    let is_buy = r.bool("is_buy")?;
    let user = r.pubkey("user")?;
    let timestamp = r.i64("timestamp")?;
    let virtual_sol_reserves = r.u64("virtual_sol_reserves")?;
    let virtual_token_reserves = r.u64("virtual_token_reserves")?;
    let real_sol_reserves = r.u64("real_sol_reserves")?;
    let real_token_reserves = r.u64("real_token_reserves")?;
    let fee_recipient = r.pubkey("fee_recipient")?;
    let fee_basis_points = r.u64("fee_basis_points")?;
    let fee = r.u64("fee")?;
    let creator = r.pubkey("creator")?;
    let creator_fee_basis_points = r.u64("creator_fee_basis_points")?;
    let creator_fee = r.u64("creator_fee")?;
    let track_volume = r.opt("track_volume", Reader::bool)?;
    let total_unclaimed_tokens = r.opt("total_unclaimed_tokens", Reader::u64)?;
    let total_claimed_tokens = r.opt("total_claimed_tokens", Reader::u64)?;
    let current_sol_volume = r.opt("current_sol_volume", Reader::u64)?;
    let last_update_timestamp = r.opt("last_update_timestamp", Reader::i64)?;
    let ix_name = r.opt("ix_name", Reader::string)?;
    let mayhem_mode = r.opt("mayhem_mode", Reader::bool)?;
    let cashback_fee_basis_points = r.opt("cashback_fee_basis_points", Reader::u64)?;
    let cashback = r.opt("cashback", Reader::u64)?;
    let buyback_fee_basis_points = r.opt("buyback_fee_basis_points", Reader::u64)?;
    let buyback_fee = r.opt("buyback_fee", Reader::u64)?;
    let shareholders = r.opt("shareholders", Reader::shareholders)?;
    let quote_mint = r.opt("quote_mint", Reader::pubkey)?;
    let quote_amount = r.opt("quote_amount", Reader::u64)?;
    let virtual_quote_reserves = r.opt("virtual_quote_reserves", Reader::u64)?;
    let real_quote_reserves = r.opt("real_quote_reserves", Reader::u64)?;
    let holder_rewards_bps = r.opt("holder_rewards_bps", Reader::u64)?;
    let holder_rewards = r.opt("holder_rewards", Reader::u64)?;
    let trailing_bytes = r.remaining();
    if trailing_bytes > MAX_TRAILING_EVENT_BYTES {
        return Err(format!(
            "TradeEvent has {trailing_bytes} bytes after the last known field, over the \
             {MAX_TRAILING_EVENT_BYTES}-byte bound"
        ));
    }
    Ok(TradeEvent {
        mint,
        sol_amount,
        token_amount,
        is_buy,
        user,
        timestamp,
        virtual_sol_reserves,
        virtual_token_reserves,
        real_sol_reserves,
        real_token_reserves,
        fee_recipient,
        fee_basis_points,
        fee,
        creator,
        creator_fee_basis_points,
        creator_fee,
        track_volume,
        total_unclaimed_tokens,
        total_claimed_tokens,
        current_sol_volume,
        last_update_timestamp,
        ix_name,
        mayhem_mode,
        cashback_fee_basis_points,
        cashback,
        buyback_fee_basis_points,
        buyback_fee,
        shareholders,
        quote_mint,
        quote_amount,
        virtual_quote_reserves,
        real_quote_reserves,
        holder_rewards_bps,
        holder_rewards,
        last_field_present: r.last,
        trailing_bytes,
        data_len,
        instruction_index,
    })
}

/// Which instruction/event fields disagreed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairMismatch {
    pub mint: bool,
    pub user: bool,
    pub side: bool,
}

/// Pairing status of one decoded trade instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeEventPairing {
    /// Event found and consistent on mint, user and side.
    Paired(Box<TradeEvent>),
    /// No `TradeEvent` between this trade and the next trade instruction.
    MissingEvent,
    /// An event was found but disagrees with the instruction; not re-paired.
    Mismatch {
        event: Box<TradeEvent>,
        mismatch: PairMismatch,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedTrade {
    pub trade: DecodedBondingCurveTrade,
    pub pairing: TradeEventPairing,
}

/// Result of pairing one transaction. Every category is counted; nothing is
/// dropped silently.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TradeEventPairingReport {
    pub trades: Vec<PairedTrade>,
    /// `TradeEvent`s not claimed by any trade instruction.
    pub orphan_events: Vec<TradeEvent>,
    pub other_events: usize,
    pub unknown_events: usize,
    pub malformed_events: usize,
    /// Pump trade-discriminator instructions that failed to decode.
    pub malformed_trades: usize,
}

impl TradeEventPairingReport {
    #[must_use]
    pub fn paired(&self) -> usize {
        self.trades
            .iter()
            .filter(|t| matches!(t.pairing, TradeEventPairing::Paired(_)))
            .count()
    }

    #[must_use]
    pub fn missing(&self) -> usize {
        self.trades
            .iter()
            .filter(|t| matches!(t.pairing, TradeEventPairing::MissingEvent))
            .count()
    }

    #[must_use]
    pub fn mismatched(&self) -> usize {
        self.trades
            .iter()
            .filter(|t| matches!(t.pairing, TradeEventPairing::Mismatch { .. }))
            .count()
    }
}

/// Pair each decoded pump trade instruction of one transaction with the
/// `TradeEvent` of its own execution. `instructions` must be the flattened
/// list in execution order (as the Helius provider produces it).
#[must_use]
pub fn pair_trades_with_events(
    decoder: &BondingCurveBuyDecoder,
    instructions: &[RawSolanaInstruction],
    slot: u64,
    transaction_index: u64,
) -> TradeEventPairingReport {
    let mut report = TradeEventPairingReport::default();
    // Index of the trade still waiting for its event.
    let mut open: Option<usize> = None;
    for ix in instructions {
        match decoder.classify(ix, slot, transaction_index) {
            PumpInstructionOutcome::Trade(trade) => {
                report.trades.push(PairedTrade {
                    trade,
                    pairing: TradeEventPairing::MissingEvent,
                });
                open = report.trades.len().checked_sub(1);
                continue;
            }
            PumpInstructionOutcome::Malformed {
                variant: Some(_), ..
            } => {
                report.malformed_trades += 1;
                open = None;
                continue;
            }
            _ => {}
        }
        match classify_pump_event(ix) {
            PumpEventOutcome::Trade(ev) if decoder.is_program(ix) => {
                let slot_ref = open.take().and_then(|i| report.trades.get_mut(i));
                match slot_ref {
                    Some(p) => {
                        let mismatch = PairMismatch {
                            mint: ev.mint != p.trade.mint,
                            user: ev.user != p.trade.user,
                            side: ev.is_buy != (p.trade.side == TradeSide::Buy),
                        };
                        p.pairing = if mismatch.mint || mismatch.user || mismatch.side {
                            TradeEventPairing::Mismatch {
                                event: ev,
                                mismatch,
                            }
                        } else {
                            TradeEventPairing::Paired(ev)
                        };
                    }
                    None => report.orphan_events.push(*ev),
                }
            }
            PumpEventOutcome::OtherEvent { .. } if decoder.is_program(ix) => {
                report.other_events += 1;
            }
            PumpEventOutcome::UnknownEvent { .. } if decoder.is_program(ix) => {
                report.unknown_events += 1;
            }
            PumpEventOutcome::Malformed { .. } if decoder.is_program(ix) => {
                report.malformed_events += 1;
            }
            _ => {}
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use scout_api::DeploymentScope;

    use super::*;
    use crate::bonding_curve_buy::{PumpTradeVariant, hex8};

    fn idl() -> serde_json::Value {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/p0/measurements/fixtures/pump_idl_e0687ae9.json");
        serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
    }

    fn disc(v: &serde_json::Value) -> [u8; 8] {
        let b: Vec<u8> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|n| u8::try_from(n.as_u64().unwrap()).unwrap())
            .collect();
        b.try_into().unwrap()
    }

    fn pump_id() -> SolanaPubkey {
        let mut k = [0u8; 32];
        k[0] = 0x6e;
        k[31] = 0xf8;
        k
    }

    fn decoder() -> BondingCurveBuyDecoder {
        BondingCurveBuyDecoder::new(DeploymentScope {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Solana,
                network_id: scout_core::NetworkId::SolanaCluster(
                    scout_core::SolanaCluster::Mainnet,
                ),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            contract_addresses: vec![scout_core::AddressBytes::Solana(pump_id())],
            active_from: 0,
            active_until: None,
        })
    }

    #[test]
    fn trade_discriminator_and_event_table_equal_idl() {
        let idl = idl();
        let events = idl["events"].as_array().unwrap();
        let from_idl: Vec<(String, [u8; 8])> = events
            .iter()
            .map(|e| {
                (
                    e["name"].as_str().unwrap().to_owned(),
                    disc(&e["discriminator"]),
                )
            })
            .collect();
        let ours: Vec<(String, [u8; 8])> = EVENT_DISCRIMINATORS
            .iter()
            .map(|(n, d)| ((*n).to_owned(), *d))
            .collect();
        assert_eq!(ours, from_idl);
        assert_eq!(hex8(&TRADE_EVENT_DISCRIMINATOR), "bddb7fd34ee661ee");
        assert_eq!(event_name(&TRADE_EVENT_DISCRIMINATOR), Some("TradeEvent"));
    }

    #[test]
    fn layout_matches_idl_trade_event_fields() {
        let idl = idl();
        let t = idl["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "TradeEvent")
            .unwrap();
        let fields: Vec<&str> = t["type"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        // Required prefix = fields up to creator_fee, all fixed-size.
        let pos = fields.iter().position(|f| *f == "creator_fee").unwrap();
        assert_eq!(pos + 1, 16);
        // 6 pubkeys-ish: mint,user,fee_recipient,creator = 4*32; 1 bool;
        // 11 u64/i64 fields.
        assert_eq!(TRADE_EVENT_REQUIRED_LEN, 4 * 32 + 1 + 11 * 8);
        assert_eq!(fields.len(), 34);
        assert_eq!(fields[fields.len() - 1], "holder_rewards");
        let sh = idl["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "Shareholder")
            .unwrap();
        assert_eq!(sh["type"]["fields"][0]["type"], "pubkey");
        assert_eq!(sh["type"]["fields"][1]["type"], "u16");
    }

    fn le(v: u64) -> [u8; 8] {
        v.to_le_bytes()
    }

    fn pk(tag: u8) -> [u8; 32] {
        [tag; 32]
    }

    /// Required prefix with distinguishable values.
    fn prefix(mint: u8, user: u8, is_buy: bool) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend(pk(mint));
        p.extend(le(1_000));
        p.extend(le(2_000));
        p.push(u8::from(is_buy));
        p.extend(pk(user));
        p.extend(le(1_790_000_000));
        for v in [11, 12, 13, 14] {
            p.extend(le(v));
        }
        p.extend(pk(0xfe));
        p.extend(le(95));
        p.extend(le(5));
        p.extend(pk(0xcc));
        p.extend(le(30));
        p.extend(le(6));
        assert_eq!(p.len(), TRADE_EVENT_REQUIRED_LEN);
        p
    }

    /// Fields after `creator_fee` through `ix_name`.
    fn mid(ix_name: &[u8]) -> Vec<u8> {
        let mut p = vec![1u8];
        for v in [1, 2, 3] {
            p.extend(le(v));
        }
        p.extend(le(4)); // last_update_timestamp
        p.extend(u32::try_from(ix_name.len()).unwrap().to_le_bytes());
        p.extend(ix_name);
        p
    }

    /// Everything after `ix_name`, with `n` shareholders.
    fn tail(n: u32) -> Vec<u8> {
        let mut p = vec![0u8];
        for v in [1, 2, 3, 4] {
            p.extend(le(v));
        }
        p.extend(n.to_le_bytes());
        for i in 0..n {
            p.extend(pk(u8::try_from(i % 200).unwrap()));
            p.extend(7u16.to_le_bytes());
        }
        p.extend(pk(0x55));
        for v in [5, 6, 7, 8, 9] {
            p.extend(le(v));
        }
        p
    }

    fn full(ix_name: &[u8], n: u32) -> Vec<u8> {
        let mut p = prefix(1, 2, true);
        p.extend(mid(ix_name));
        p.extend(tail(n));
        p
    }

    fn ev_ix(payload: &[u8], idx: u32) -> RawSolanaInstruction {
        let mut data = EVENT_CPI_DISCRIMINATOR.to_vec();
        data.extend(TRADE_EVENT_DISCRIMINATOR);
        data.extend(payload);
        RawSolanaInstruction {
            program_id: pump_id(),
            accounts: vec![pk(0xea)],
            data,
            instruction_index: idx,
        }
    }

    fn decode(payload: &[u8]) -> PumpEventOutcome {
        decoder().classify_event(&ev_ix(payload, 0))
    }

    fn trade_of(o: PumpEventOutcome) -> TradeEvent {
        match o {
            PumpEventOutcome::Trade(t) => *t,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn full_event_decodes_every_field() {
        let e = trade_of(decode(&full(b"buy", 2)));
        assert_eq!(e.mint, pk(1));
        assert_eq!(e.user, pk(2));
        assert!(e.is_buy);
        assert_eq!((e.sol_amount, e.token_amount), (1_000, 2_000));
        assert_eq!(e.creator_fee, 6);
        assert_eq!(e.track_volume, Some(true));
        assert_eq!(e.last_update_timestamp, Some(4));
        assert_eq!(e.ix_name.as_deref(), Some("buy"));
        assert_eq!(e.mayhem_mode, Some(false));
        assert_eq!(e.buyback_fee, Some(4));
        assert_eq!(e.shareholders.as_ref().unwrap().len(), 2);
        assert_eq!(e.quote_mint, Some(pk(0x55)));
        assert_eq!(e.holder_rewards, Some(9));
        assert_eq!(e.last_field_present, "holder_rewards");
        assert_eq!(e.trailing_bytes, 0);
    }

    #[test]
    fn boundary_ended_buffers_give_nones() {
        let pre = prefix(1, 2, false);
        let e = trade_of(decode(&pre));
        assert_eq!(e.last_field_present, "creator_fee");
        assert!(e.track_volume.is_none() && e.ix_name.is_none() && e.holder_rewards.is_none());
        assert!(!e.is_buy);

        let mut upto_ix = pre.clone();
        upto_ix.extend(mid(b"sell"));
        let e = trade_of(decode(&upto_ix));
        assert_eq!(e.last_field_present, "ix_name");
        assert_eq!(e.ix_name.as_deref(), Some("sell"));
        assert!(e.mayhem_mode.is_none() && e.shareholders.is_none() && e.quote_mint.is_none());

        // Ends right after shareholders (before quote_mint).
        let mut upto_sh = upto_ix.clone();
        let t = tail(0);
        upto_sh.extend(&t[..1 + 4 * 8 + 4]);
        let e = trade_of(decode(&upto_sh));
        assert_eq!(e.last_field_present, "shareholders");
        assert_eq!(e.shareholders, Some(vec![]));
        assert!(e.quote_mint.is_none() && e.holder_rewards.is_none());
    }

    #[test]
    fn mid_field_truncation_is_malformed_at_every_cut() {
        let full = full(b"buy_v2", 1);
        for cut in 0..full.len() {
            let out = decode(&full[..cut]);
            let at_boundary = matches!(out, PumpEventOutcome::Trade(_));
            if cut < TRADE_EVENT_REQUIRED_LEN {
                assert!(
                    matches!(out, PumpEventOutcome::Malformed { .. }),
                    "cut {cut}"
                );
            } else if let PumpEventOutcome::Trade(e) = &out {
                // Only genuine field boundaries decode, and then never
                // consume past the buffer.
                assert_eq!(e.trailing_bytes, 0, "cut {cut}");
            } else {
                assert!(
                    matches!(out, PumpEventOutcome::Malformed { .. }),
                    "cut {cut}"
                );
                assert!(!at_boundary);
            }
        }
        // The cut inside the u64 right after the prefix is Malformed.
        let mut p = prefix(1, 2, true);
        p.extend([1, 0, 0]);
        assert!(matches!(decode(&p), PumpEventOutcome::Malformed { .. }));
        // The cut inside ix_name bytes is Malformed.
        let mut p = prefix(1, 2, true);
        p.extend(mid(b"abcdef"));
        p.pop();
        assert!(matches!(decode(&p), PumpEventOutcome::Malformed { .. }));
    }

    #[test]
    fn trailing_bytes_are_recorded_and_bounded() {
        let mut p = full(b"buy", 0);
        p.extend([9u8; 10]);
        let e = trade_of(decode(&p));
        assert_eq!(e.trailing_bytes, 10);
        assert_eq!(e.last_field_present, "holder_rewards");

        let mut p = full(b"buy", 0);
        p.extend(vec![9u8; MAX_TRAILING_EVENT_BYTES]);
        assert_eq!(
            trade_of(decode(&p)).trailing_bytes,
            MAX_TRAILING_EVENT_BYTES
        );
        p.push(9);
        assert!(matches!(decode(&p), PumpEventOutcome::Malformed { .. }));
    }

    #[test]
    fn over_bound_strings_and_vectors_are_malformed() {
        let ok = vec![b'a'; MAX_IX_NAME_BYTES];
        assert!(matches!(decode(&full(&ok, 0)), PumpEventOutcome::Trade(_)));
        let long = vec![b'a'; MAX_IX_NAME_BYTES + 1];
        assert!(matches!(
            decode(&full(&long, 0)),
            PumpEventOutcome::Malformed { .. }
        ));
        // A huge declared length with no body must not allocate or panic.
        let mut p = prefix(1, 2, true);
        p.push(1);
        for v in [1, 2, 3, 4] {
            p.extend(le(v));
        }
        p.extend(u32::MAX.to_le_bytes());
        assert!(matches!(decode(&p), PumpEventOutcome::Malformed { .. }));
        // Invalid UTF-8 and control characters.
        assert!(matches!(
            decode(&full(&[0xff, 0xfe], 0)),
            PumpEventOutcome::Malformed { .. }
        ));
        assert!(matches!(
            decode(&full(b"a\nb", 0)),
            PumpEventOutcome::Malformed { .. }
        ));
        // Shareholders bound.
        let n = u32::try_from(MAX_SHAREHOLDERS).unwrap();
        assert!(matches!(
            decode(&full(b"buy", n)),
            PumpEventOutcome::Trade(_)
        ));
        assert!(matches!(
            decode(&full(b"buy", n + 1)),
            PumpEventOutcome::Malformed { .. }
        ));
        // Non-0/1 bool.
        let mut p = full(b"buy", 0);
        p[32 + 8 + 8] = 2; // is_buy
        assert!(matches!(decode(&p), PumpEventOutcome::Malformed { .. }));
    }

    #[test]
    fn classification_gates() {
        let mut foreign = ev_ix(&full(b"buy", 0), 0);
        foreign.program_id = pk(0x99);
        assert_eq!(
            decoder().classify_event(&foreign),
            PumpEventOutcome::NotMine
        );

        let mut plain = ev_ix(&[], 0);
        plain.data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9];
        assert_eq!(
            decoder().classify_event(&plain),
            PumpEventOutcome::NotEventCpi
        );

        let mut short = ev_ix(&[], 0);
        short.data = EVENT_CPI_DISCRIMINATOR.to_vec();
        assert!(matches!(
            decoder().classify_event(&short),
            PumpEventOutcome::Malformed { .. }
        ));

        let mut other = ev_ix(&[], 0);
        other.data = EVENT_CPI_DISCRIMINATOR.to_vec();
        other.data.extend(EVENT_DISCRIMINATORS[10].1);
        assert_eq!(
            decoder().classify_event(&other),
            PumpEventOutcome::OtherEvent {
                discriminator: EVENT_DISCRIMINATORS[10].1,
                name: "CreateEvent"
            }
        );
        let mut unknown = ev_ix(&[], 0);
        unknown.data = EVENT_CPI_DISCRIMINATOR.to_vec();
        unknown.data.extend([0xab; 8]);
        assert_eq!(
            decoder().classify_event(&unknown),
            PumpEventOutcome::UnknownEvent {
                discriminator: [0xab; 8]
            }
        );
    }

    // ---- pairing ---------------------------------------------------------

    fn trade_ix(variant: PumpTradeVariant, mint: u8, user: u8, idx: u32) -> RawSolanaInstruction {
        let spec = variant.spec();
        let mut accounts: Vec<SolanaPubkey> = (0..spec.min_accounts)
            .map(|i| pk(u8::try_from(i).unwrap().wrapping_add(100)))
            .collect();
        accounts[spec.mint_idx] = pk(mint);
        accounts[spec.user_idx] = pk(user);
        if let Some(q) = spec.quote_mint_idx {
            accounts[q] = pk(0x77);
        }
        let mut data = spec.discriminator.to_vec();
        data.extend(le(10));
        data.extend(le(20));
        RawSolanaInstruction {
            program_id: pump_id(),
            accounts,
            data,
            instruction_index: idx,
        }
    }

    fn event_for(mint: u8, user: u8, is_buy: bool, idx: u32) -> RawSolanaInstruction {
        let mut p = prefix(mint, user, is_buy);
        p.extend(mid(b"buy"));
        ev_ix(&p, idx)
    }

    fn other_program_ix(idx: u32) -> RawSolanaInstruction {
        RawSolanaInstruction {
            program_id: pk(0x42),
            accounts: vec![],
            data: vec![1, 2, 3],
            instruction_index: idx,
        }
    }

    #[test]
    fn router_with_two_trades_pairs_one_to_one_in_order() {
        let ixs = vec![
            other_program_ix(0), // router
            trade_ix(PumpTradeVariant::Buy, 1, 2, 1),
            other_program_ix(2), // token transfer
            event_for(1, 2, true, 3),
            trade_ix(PumpTradeVariant::Sell, 3, 4, 4),
            event_for(3, 4, false, 5),
        ];
        let r = pair_trades_with_events(&decoder(), &ixs, 1, 1);
        assert_eq!((r.paired(), r.missing(), r.mismatched()), (2, 0, 0));
        assert!(r.orphan_events.is_empty());
        match (&r.trades[0].pairing, &r.trades[1].pairing) {
            (TradeEventPairing::Paired(a), TradeEventPairing::Paired(b)) => {
                assert_eq!((a.instruction_index, b.instruction_index), (3, 5));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn missing_mismatch_and_orphan_are_explicit() {
        // Trade 1 has no event; the event after trade 2 is for trade 2.
        let ixs = vec![
            trade_ix(PumpTradeVariant::Buy, 1, 2, 0),
            trade_ix(PumpTradeVariant::Buy, 5, 6, 1),
            event_for(5, 6, true, 2),
        ];
        let r = pair_trades_with_events(&decoder(), &ixs, 1, 1);
        assert_eq!((r.paired(), r.missing(), r.mismatched()), (1, 1, 0));
        assert_eq!(r.trades[0].pairing, TradeEventPairing::MissingEvent);

        // Wrong mint / user / side each flagged, never re-paired.
        for (m, u, b, want) in [
            (9, 2, true, (true, false, false)),
            (1, 9, true, (false, true, false)),
            (1, 2, false, (false, false, true)),
        ] {
            let ixs = vec![
                trade_ix(PumpTradeVariant::Buy, 1, 2, 0),
                event_for(m, u, b, 1),
            ];
            let r = pair_trades_with_events(&decoder(), &ixs, 1, 1);
            match &r.trades[0].pairing {
                TradeEventPairing::Mismatch { mismatch, .. } => {
                    assert_eq!((mismatch.mint, mismatch.user, mismatch.side), want);
                }
                other => panic!("{other:?}"),
            }
        }

        // Second event for one trade, and an event with no trade, are orphans.
        let ixs = vec![
            event_for(1, 2, true, 0),
            trade_ix(PumpTradeVariant::Buy, 1, 2, 1),
            event_for(1, 2, true, 2),
            event_for(1, 2, true, 3),
        ];
        let r = pair_trades_with_events(&decoder(), &ixs, 1, 1);
        assert_eq!(r.paired(), 1);
        assert_eq!(r.orphan_events.len(), 2);
    }

    #[test]
    fn malformed_trade_does_not_steal_the_previous_trades_event() {
        let mut bad = trade_ix(PumpTradeVariant::Buy, 1, 2, 1);
        bad.data.truncate(12);
        let ixs = vec![
            trade_ix(PumpTradeVariant::Buy, 1, 2, 0),
            bad,
            event_for(1, 2, true, 2),
        ];
        let r = pair_trades_with_events(&decoder(), &ixs, 1, 1);
        assert_eq!(r.malformed_trades, 1);
        assert_eq!(r.missing(), 1);
        assert_eq!(r.orphan_events.len(), 1);
    }

    #[test]
    fn foreign_program_events_are_ignored_by_pairing() {
        let mut foreign = event_for(1, 2, true, 1);
        foreign.program_id = pk(0x99);
        let ixs = vec![trade_ix(PumpTradeVariant::Buy, 1, 2, 0), foreign];
        let r = pair_trades_with_events(&decoder(), &ixs, 1, 1);
        assert_eq!(r.missing(), 1);
        assert!(r.orphan_events.is_empty());
    }
}
