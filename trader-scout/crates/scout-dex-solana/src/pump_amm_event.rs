//! PumpSwap AMM `BuyEvent` / `SellEvent` decoder (Anchor event-CPI) and
//! trade/event pairing.
//!
//! Wire form: `EVENT_CPI_DISCRIMINATOR (8) ++ event discriminator (8) ++
//! Borsh event`, emitted as an inner self-invocation of the AMM program.
//! Layout source: the pinned IDL `types.BuyEvent` / `types.SellEvent`
//! (`pump_amm_idl_e0687ae9.json`); tests re-read the IDL.
//!
//! ## Length policy (same shape as the bonding-curve `TradeEvent`)
//!
//! The REQUIRED prefix `timestamp ..= coin_creator_fee` (352 bytes) is
//! decoded exactly; shorter is `Malformed`. Later IDL fields are decoded in
//! order while bytes remain: a field is `None` iff the buffer ended exactly
//! at its boundary, a buffer ending inside a field is `Malformed`. Bytes
//! after the last known field are counted in `trailing_bytes` (the live
//! program appends 8 bytes beyond the pinned IDL; their meaning is NOT
//! guessed), bounded by [`MAX_AMM_TRAILING_EVENT_BYTES`]. `ix_name` is
//! bounded, UTF-8 and control-character free. Chain strings are untrusted.
//!
//! ## Quote consideration (see ADR-012 evidence in the tests)
//!
//! - Buy cost in quote units = `quote_amount_in_with_lp_fee + protocol_fee
//!   + coin_creator_fee` ([`BuyEvent::quote_cost`]). This equals
//!   `user_quote_amount_in` for `buy` but NOT for `buy_exact_quote_in`,
//!   where `quote_amount_in` is the gross spend (== `spendable_quote_in`)
//!   and `user_quote_amount_in` is only the net pool credit.
//! - Sell proceeds = `quote_amount_out_without_lp_fee - protocol_fee -
//!   coin_creator_fee` ([`SellEvent::quote_proceeds`]), equal to
//!   `user_quote_amount_out` in every fixture.
//!
//! ## Pairing
//!
//! A trade's own event is the first Buy/Sell event after it and before the
//! next AMM trade instruction (execution order of the flattened list). It
//! must agree on user, pool, side and both user token accounts, otherwise
//! the pair is reported as `Mismatch` (never re-paired).

use scout_core::{RawSolanaInstruction, SolanaPubkey};

use crate::bonding_curve_buy::{EVENT_CPI_DISCRIMINATOR, TradeSide};
use crate::pump_amm::{DecodedPumpAmmTrade, PumpAmmDecoder, PumpAmmInstructionOutcome};

/// `BuyEvent` discriminator.
pub const AMM_BUY_EVENT_DISCRIMINATOR: [u8; 8] = [0x67, 0xf4, 0x52, 0x1f, 0x2c, 0xf5, 0x77, 0x77];
/// `SellEvent` discriminator.
pub const AMM_SELL_EVENT_DISCRIMINATOR: [u8; 8] = [0x3e, 0x2f, 0x37, 0x0a, 0xa5, 0x03, 0xdc, 0x2a];
/// Event-CPI header: tag + event discriminator.
pub const AMM_EVENT_CPI_HEADER_LEN: usize = 16;
/// Required fixed prefix (`timestamp ..= coin_creator_fee`), without header.
pub const AMM_EVENT_REQUIRED_LEN: usize = 352;
/// Upper bound on bytes after the last IDL field.
pub const MAX_AMM_TRAILING_EVENT_BYTES: usize = 256;
/// Upper bound on `ix_name` bytes.
pub const MAX_AMM_IX_NAME_BYTES: usize = 64;

/// The 24 IDL events other than `BuyEvent`/`SellEvent`.
pub const AMM_OTHER_EVENTS: [(&str, [u8; 8]); 24] = [
    (
        "AdminCtoPoolEvent",
        [0x2f, 0x23, 0xa3, 0xf9, 0x96, 0x9d, 0x93, 0x7a],
    ),
    (
        "AdminUpdateTokenIncentivesEvent",
        [0x93, 0xfa, 0x6c, 0x78, 0xf7, 0x1d, 0x43, 0xde],
    ),
    (
        "BoostBuyAndBurnEvent",
        [0x3f, 0x45, 0x1c, 0x16, 0x30, 0x5c, 0xc2, 0xb9],
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
        "CollectCoinCreatorFeeEvent",
        [0xe8, 0xf5, 0xc2, 0xee, 0xea, 0xda, 0x3a, 0x59],
    ),
    (
        "CreateConfigEvent",
        [0x6b, 0x34, 0x59, 0x81, 0x37, 0xe2, 0x51, 0x16],
    ),
    (
        "CreatePoolEvent",
        [0xb1, 0x31, 0x0c, 0xd2, 0xa0, 0x76, 0xa7, 0x74],
    ),
    (
        "DepositEvent",
        [0x78, 0xf8, 0x3d, 0x53, 0x1f, 0x8e, 0x6b, 0x90],
    ),
    (
        "DisableEvent",
        [0x6b, 0xfd, 0xc1, 0x4c, 0xe4, 0xca, 0x1b, 0x68],
    ),
    (
        "ExtendAccountEvent",
        [0x61, 0x61, 0xd7, 0x90, 0x5d, 0x92, 0x16, 0x7c],
    ),
    (
        "InitBoostEvent",
        [0xae, 0x7c, 0x4a, 0xf9, 0x04, 0x51, 0xf6, 0x11],
    ),
    (
        "InitUserVolumeAccumulatorEvent",
        [0x86, 0x24, 0x0d, 0x48, 0xe8, 0x65, 0x82, 0xd8],
    ),
    (
        "MigratePoolCoinCreatorEvent",
        [0xaa, 0xdd, 0x52, 0xc7, 0x93, 0xa5, 0xf7, 0x2e],
    ),
    (
        "ReservedFeeRecipientsEvent",
        [0x2b, 0xbc, 0xfa, 0x12, 0xdd, 0x4b, 0xbb, 0x5f],
    ),
    (
        "SetBondingCurveCoinCreatorEvent",
        [0xf2, 0xe7, 0xeb, 0x66, 0x41, 0x63, 0xbd, 0xd3],
    ),
    (
        "SetBoostAuthorityEvent",
        [0x59, 0x80, 0xf0, 0x8d, 0x5b, 0xca, 0x47, 0x69],
    ),
    (
        "SetMetaplexCoinCreatorEvent",
        [0x96, 0x6b, 0xc7, 0x7b, 0x7c, 0xcf, 0x66, 0xe4],
    ),
    (
        "SyncUserVolumeAccumulatorEvent",
        [0xc5, 0x7a, 0xa7, 0x7c, 0x74, 0x51, 0x5b, 0xff],
    ),
    (
        "UpdateAdminEvent",
        [0xe1, 0x98, 0xab, 0x57, 0xf6, 0x3f, 0x42, 0xea],
    ),
    (
        "UpdateCreatorFeeConfigEvent",
        [0x98, 0xc6, 0x7c, 0x7c, 0x6a, 0xf6, 0x7f, 0xbf],
    ),
    (
        "UpdateFeeConfigEvent",
        [0x5a, 0x17, 0x41, 0x23, 0x3e, 0xf4, 0xbc, 0xd0],
    ),
    (
        "WithdrawEvent",
        [0x16, 0x09, 0x85, 0x1a, 0xa0, 0x2c, 0x47, 0xc0],
    ),
];

fn other_event_name(discriminator: &[u8; 8]) -> Option<&'static str> {
    AMM_OTHER_EVENTS
        .iter()
        .find(|(_, d)| d == discriminator)
        .map(|(n, _)| *n)
}

/// Decoded `BuyEvent`. Fields from `track_volume` on are `None` when the
/// event ended before them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuyEvent {
    pub timestamp: i64,
    pub base_amount_out: u64,
    pub max_quote_amount_in: u64,
    pub user_base_token_reserves: u64,
    pub user_quote_token_reserves: u64,
    pub pool_base_token_reserves: u64,
    pub pool_quote_token_reserves: u64,
    pub quote_amount_in: u64,
    pub lp_fee_basis_points: u64,
    pub lp_fee: u64,
    pub protocol_fee_basis_points: u64,
    pub protocol_fee: u64,
    pub quote_amount_in_with_lp_fee: u64,
    pub user_quote_amount_in: u64,
    pub pool: SolanaPubkey,
    pub user: SolanaPubkey,
    pub user_base_token_account: SolanaPubkey,
    pub user_quote_token_account: SolanaPubkey,
    pub protocol_fee_recipient: SolanaPubkey,
    pub protocol_fee_recipient_token_account: SolanaPubkey,
    pub coin_creator: SolanaPubkey,
    pub coin_creator_fee_basis_points: u64,
    pub coin_creator_fee: u64,
    pub track_volume: Option<bool>,
    pub total_unclaimed_tokens: Option<u64>,
    pub total_claimed_tokens: Option<u64>,
    pub current_sol_volume: Option<u64>,
    pub last_update_timestamp: Option<i64>,
    pub min_base_amount_out: Option<u64>,
    pub ix_name: Option<String>,
    pub cashback_fee_basis_points: Option<u64>,
    pub cashback: Option<u64>,
    pub buyback_fee_basis_points: Option<u64>,
    pub buyback_fee: Option<u64>,
    pub virtual_quote_reserves: Option<i128>,
    pub can_boost: Option<bool>,
    pub base_supply: Option<u64>,
    pub holder_rewards_bps: Option<u64>,
    pub holder_rewards: Option<u64>,
    /// Name of the last IDL field present in the buffer.
    pub last_field_present: &'static str,
    /// Bytes after the last known field (unknown appended fields).
    pub trailing_bytes: usize,
    /// Total instruction data length (header included).
    pub data_len: usize,
    pub instruction_index: u32,
}

impl BuyEvent {
    /// Total quote the user paid: `quote_amount_in_with_lp_fee +
    /// protocol_fee + coin_creator_fee` (checked; `None` on overflow).
    /// `buyback_fee`, `cashback` and `holder_rewards` are NOT added (they
    /// are carved out of the protocol/creator fees; fixtures reconcile
    /// exactly without them).
    #[must_use]
    pub fn quote_cost(&self) -> Option<u64> {
        self.quote_amount_in_with_lp_fee
            .checked_add(self.protocol_fee)?
            .checked_add(self.coin_creator_fee)
    }
}

/// Decoded `SellEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SellEvent {
    pub timestamp: i64,
    pub base_amount_in: u64,
    pub min_quote_amount_out: u64,
    pub user_base_token_reserves: u64,
    pub user_quote_token_reserves: u64,
    pub pool_base_token_reserves: u64,
    pub pool_quote_token_reserves: u64,
    pub quote_amount_out: u64,
    pub lp_fee_basis_points: u64,
    pub lp_fee: u64,
    pub protocol_fee_basis_points: u64,
    pub protocol_fee: u64,
    pub quote_amount_out_without_lp_fee: u64,
    pub user_quote_amount_out: u64,
    pub pool: SolanaPubkey,
    pub user: SolanaPubkey,
    pub user_base_token_account: SolanaPubkey,
    pub user_quote_token_account: SolanaPubkey,
    pub protocol_fee_recipient: SolanaPubkey,
    pub protocol_fee_recipient_token_account: SolanaPubkey,
    pub coin_creator: SolanaPubkey,
    pub coin_creator_fee_basis_points: u64,
    pub coin_creator_fee: u64,
    pub cashback_fee_basis_points: Option<u64>,
    pub cashback: Option<u64>,
    pub buyback_fee_basis_points: Option<u64>,
    pub buyback_fee: Option<u64>,
    pub virtual_quote_reserves: Option<i128>,
    pub can_boost: Option<bool>,
    pub base_supply: Option<u64>,
    pub holder_rewards_bps: Option<u64>,
    pub holder_rewards: Option<u64>,
    pub last_field_present: &'static str,
    pub trailing_bytes: usize,
    pub data_len: usize,
    pub instruction_index: u32,
}

impl SellEvent {
    /// Net quote the user received: `quote_amount_out_without_lp_fee -
    /// protocol_fee - coin_creator_fee` (checked; `None` on underflow).
    #[must_use]
    pub fn quote_proceeds(&self) -> Option<u64> {
        self.quote_amount_out_without_lp_fee
            .checked_sub(self.protocol_fee)?
            .checked_sub(self.coin_creator_fee)
    }
}

/// A decoded Buy or Sell event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpAmmEvent {
    Buy(Box<BuyEvent>),
    Sell(Box<SellEvent>),
}

impl PumpAmmEvent {
    #[must_use]
    pub fn side(&self) -> TradeSide {
        match self {
            Self::Buy(_) => TradeSide::Buy,
            Self::Sell(_) => TradeSide::Sell,
        }
    }

    #[must_use]
    pub fn user(&self) -> SolanaPubkey {
        match self {
            Self::Buy(e) => e.user,
            Self::Sell(e) => e.user,
        }
    }

    #[must_use]
    pub fn pool(&self) -> SolanaPubkey {
        match self {
            Self::Buy(e) => e.pool,
            Self::Sell(e) => e.pool,
        }
    }

    #[must_use]
    pub fn user_base_token_account(&self) -> SolanaPubkey {
        match self {
            Self::Buy(e) => e.user_base_token_account,
            Self::Sell(e) => e.user_base_token_account,
        }
    }

    #[must_use]
    pub fn user_quote_token_account(&self) -> SolanaPubkey {
        match self {
            Self::Buy(e) => e.user_quote_token_account,
            Self::Sell(e) => e.user_quote_token_account,
        }
    }

    /// Base amount moved (`base_amount_out` / `base_amount_in`).
    #[must_use]
    pub fn base_amount(&self) -> u64 {
        match self {
            Self::Buy(e) => e.base_amount_out,
            Self::Sell(e) => e.base_amount_in,
        }
    }

    /// Quote consideration from the event: buy cost or sell proceeds
    /// (`None` on arithmetic overflow/underflow = malformed event).
    #[must_use]
    pub fn quote_consideration(&self) -> Option<u64> {
        match self {
            Self::Buy(e) => e.quote_cost(),
            Self::Sell(e) => e.quote_proceeds(),
        }
    }
}

/// Classification of one instruction as an AMM event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpAmmEventOutcome {
    /// Program id is not the scoped AMM program.
    NotMine,
    /// AMM instruction that is not an event-CPI self-invocation.
    NotEventCpi,
    Trade(PumpAmmEvent),
    /// Event-CPI of an IDL event that is not Buy/Sell; counted only.
    OtherEvent {
        discriminator: [u8; 8],
        name: &'static str,
    },
    /// Event-CPI whose discriminator is not in the IDL. COVERAGE GAP.
    UnknownEvent {
        discriminator: [u8; 8],
    },
    /// Event-CPI with a broken structure. COVERAGE GAP.
    Malformed {
        reason: String,
    },
}

/// Classify one instruction ASSUMED to belong to the AMM program (no
/// program-id check; see [`PumpAmmDecoder::classify_event`]).
#[must_use]
pub fn classify_pump_amm_event(instruction: &RawSolanaInstruction) -> PumpAmmEventOutcome {
    let data = &instruction.data;
    if data.get(0..8) != Some(EVENT_CPI_DISCRIMINATOR.as_slice()) {
        return PumpAmmEventOutcome::NotEventCpi;
    }
    let Some(disc) = data
        .get(8..AMM_EVENT_CPI_HEADER_LEN)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
    else {
        return PumpAmmEventOutcome::Malformed {
            reason: format!(
                "event-CPI instruction has {} data bytes, fewer than the 16-byte header",
                data.len()
            ),
        };
    };
    let is_buy = disc == AMM_BUY_EVENT_DISCRIMINATOR;
    let is_sell = disc == AMM_SELL_EVENT_DISCRIMINATOR;
    if !is_buy && !is_sell {
        return match other_event_name(&disc) {
            Some(name) => PumpAmmEventOutcome::OtherEvent {
                discriminator: disc,
                name,
            },
            None => PumpAmmEventOutcome::UnknownEvent {
                discriminator: disc,
            },
        };
    }
    let payload = data.get(AMM_EVENT_CPI_HEADER_LEN..).unwrap_or_default();
    let idx = instruction.instruction_index;
    let result = if is_buy {
        decode_buy(payload, data.len(), idx).map(|e| PumpAmmEvent::Buy(Box::new(e)))
    } else {
        decode_sell(payload, data.len(), idx).map(|e| PumpAmmEvent::Sell(Box::new(e)))
    };
    match result {
        Ok(ev) => PumpAmmEventOutcome::Trade(ev),
        Err(reason) => PumpAmmEventOutcome::Malformed { reason },
    }
}

impl PumpAmmDecoder {
    /// Program-id-gated event classification.
    #[must_use]
    pub fn classify_event(&self, instruction: &RawSolanaInstruction) -> PumpAmmEventOutcome {
        if self.is_program(instruction) {
            classify_pump_amm_event(instruction)
        } else {
            PumpAmmEventOutcome::NotMine
        }
    }
}

struct Reader<'a> {
    event: &'static str,
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
                self.pos = self.pos.saturating_add(n);
                Ok(s)
            }
            None => Err(format!(
                "{} ends inside field `{field}` (needs {n} bytes, {} remain)",
                self.event,
                self.remaining()
            )),
        }
    }

    fn u64(&mut self, field: &'static str) -> Result<u64, String> {
        let a: [u8; 8] = self
            .take(8, field)?
            .try_into()
            .map_err(|_| format!("`{field}` not 8 bytes"))?;
        Ok(u64::from_le_bytes(a))
    }

    fn i64(&mut self, field: &'static str) -> Result<i64, String> {
        let a: [u8; 8] = self
            .take(8, field)?
            .try_into()
            .map_err(|_| format!("`{field}` not 8 bytes"))?;
        Ok(i64::from_le_bytes(a))
    }

    fn i128(&mut self, field: &'static str) -> Result<i128, String> {
        let a: [u8; 16] = self
            .take(16, field)?
            .try_into()
            .map_err(|_| format!("`{field}` not 16 bytes"))?;
        Ok(i128::from_le_bytes(a))
    }

    fn u32(&mut self, field: &'static str) -> Result<u32, String> {
        let a: [u8; 4] = self
            .take(4, field)?
            .try_into()
            .map_err(|_| format!("`{field}` not 4 bytes"))?;
        Ok(u32::from_le_bytes(a))
    }

    fn pubkey(&mut self, field: &'static str) -> Result<SolanaPubkey, String> {
        self.take(32, field)?
            .try_into()
            .map_err(|_| format!("`{field}` not 32 bytes"))
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
        if len > MAX_AMM_IX_NAME_BYTES {
            return Err(format!(
                "`{field}` length {len} exceeds the {MAX_AMM_IX_NAME_BYTES}-byte bound"
            ));
        }
        let bytes = self.take(len, field)?;
        let s = std::str::from_utf8(bytes).map_err(|_| format!("`{field}` is not valid UTF-8"))?;
        if s.chars().any(char::is_control) {
            return Err(format!("`{field}` contains control characters"));
        }
        Ok(s.to_owned())
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

    fn finish(&self, data_len: usize) -> Result<usize, String> {
        let trailing = self.remaining();
        if trailing > MAX_AMM_TRAILING_EVENT_BYTES {
            return Err(format!(
                "{} has {trailing} bytes after the last known field, over the \
                 {MAX_AMM_TRAILING_EVENT_BYTES}-byte bound (data_len {data_len})",
                self.event
            ));
        }
        Ok(trailing)
    }
}

fn check_required(event: &'static str, payload: &[u8]) -> Result<(), String> {
    if payload.len() < AMM_EVENT_REQUIRED_LEN {
        return Err(format!(
            "{event} payload has {} bytes, fewer than the {AMM_EVENT_REQUIRED_LEN} required through `coin_creator_fee`",
            payload.len()
        ));
    }
    Ok(())
}

fn decode_buy(payload: &[u8], data_len: usize, instruction_index: u32) -> Result<BuyEvent, String> {
    check_required("BuyEvent", payload)?;
    let mut r = Reader {
        event: "BuyEvent",
        buf: payload,
        pos: 0,
        ended: false,
        last: "coin_creator_fee",
    };
    let timestamp = r.i64("timestamp")?;
    let base_amount_out = r.u64("base_amount_out")?;
    let max_quote_amount_in = r.u64("max_quote_amount_in")?;
    let user_base_token_reserves = r.u64("user_base_token_reserves")?;
    let user_quote_token_reserves = r.u64("user_quote_token_reserves")?;
    let pool_base_token_reserves = r.u64("pool_base_token_reserves")?;
    let pool_quote_token_reserves = r.u64("pool_quote_token_reserves")?;
    let quote_amount_in = r.u64("quote_amount_in")?;
    let lp_fee_basis_points = r.u64("lp_fee_basis_points")?;
    let lp_fee = r.u64("lp_fee")?;
    let protocol_fee_basis_points = r.u64("protocol_fee_basis_points")?;
    let protocol_fee = r.u64("protocol_fee")?;
    let quote_amount_in_with_lp_fee = r.u64("quote_amount_in_with_lp_fee")?;
    let user_quote_amount_in = r.u64("user_quote_amount_in")?;
    let pool = r.pubkey("pool")?;
    let user = r.pubkey("user")?;
    let user_base_token_account = r.pubkey("user_base_token_account")?;
    let user_quote_token_account = r.pubkey("user_quote_token_account")?;
    let protocol_fee_recipient = r.pubkey("protocol_fee_recipient")?;
    let protocol_fee_recipient_token_account = r.pubkey("protocol_fee_recipient_token_account")?;
    let coin_creator = r.pubkey("coin_creator")?;
    let coin_creator_fee_basis_points = r.u64("coin_creator_fee_basis_points")?;
    let coin_creator_fee = r.u64("coin_creator_fee")?;
    let track_volume = r.opt("track_volume", Reader::bool)?;
    let total_unclaimed_tokens = r.opt("total_unclaimed_tokens", Reader::u64)?;
    let total_claimed_tokens = r.opt("total_claimed_tokens", Reader::u64)?;
    let current_sol_volume = r.opt("current_sol_volume", Reader::u64)?;
    let last_update_timestamp = r.opt("last_update_timestamp", Reader::i64)?;
    let min_base_amount_out = r.opt("min_base_amount_out", Reader::u64)?;
    let ix_name = r.opt("ix_name", Reader::string)?;
    let cashback_fee_basis_points = r.opt("cashback_fee_basis_points", Reader::u64)?;
    let cashback = r.opt("cashback", Reader::u64)?;
    let buyback_fee_basis_points = r.opt("buyback_fee_basis_points", Reader::u64)?;
    let buyback_fee = r.opt("buyback_fee", Reader::u64)?;
    let virtual_quote_reserves = r.opt("virtual_quote_reserves", Reader::i128)?;
    let can_boost = r.opt("can_boost", Reader::bool)?;
    let base_supply = r.opt("base_supply", Reader::u64)?;
    let holder_rewards_bps = r.opt("holder_rewards_bps", Reader::u64)?;
    let holder_rewards = r.opt("holder_rewards", Reader::u64)?;
    let trailing_bytes = r.finish(data_len)?;
    Ok(BuyEvent {
        timestamp,
        base_amount_out,
        max_quote_amount_in,
        user_base_token_reserves,
        user_quote_token_reserves,
        pool_base_token_reserves,
        pool_quote_token_reserves,
        quote_amount_in,
        lp_fee_basis_points,
        lp_fee,
        protocol_fee_basis_points,
        protocol_fee,
        quote_amount_in_with_lp_fee,
        user_quote_amount_in,
        pool,
        user,
        user_base_token_account,
        user_quote_token_account,
        protocol_fee_recipient,
        protocol_fee_recipient_token_account,
        coin_creator,
        coin_creator_fee_basis_points,
        coin_creator_fee,
        track_volume,
        total_unclaimed_tokens,
        total_claimed_tokens,
        current_sol_volume,
        last_update_timestamp,
        min_base_amount_out,
        ix_name,
        cashback_fee_basis_points,
        cashback,
        buyback_fee_basis_points,
        buyback_fee,
        virtual_quote_reserves,
        can_boost,
        base_supply,
        holder_rewards_bps,
        holder_rewards,
        last_field_present: r.last,
        trailing_bytes,
        data_len,
        instruction_index,
    })
}

fn decode_sell(
    payload: &[u8],
    data_len: usize,
    instruction_index: u32,
) -> Result<SellEvent, String> {
    check_required("SellEvent", payload)?;
    let mut r = Reader {
        event: "SellEvent",
        buf: payload,
        pos: 0,
        ended: false,
        last: "coin_creator_fee",
    };
    let timestamp = r.i64("timestamp")?;
    let base_amount_in = r.u64("base_amount_in")?;
    let min_quote_amount_out = r.u64("min_quote_amount_out")?;
    let user_base_token_reserves = r.u64("user_base_token_reserves")?;
    let user_quote_token_reserves = r.u64("user_quote_token_reserves")?;
    let pool_base_token_reserves = r.u64("pool_base_token_reserves")?;
    let pool_quote_token_reserves = r.u64("pool_quote_token_reserves")?;
    let quote_amount_out = r.u64("quote_amount_out")?;
    let lp_fee_basis_points = r.u64("lp_fee_basis_points")?;
    let lp_fee = r.u64("lp_fee")?;
    let protocol_fee_basis_points = r.u64("protocol_fee_basis_points")?;
    let protocol_fee = r.u64("protocol_fee")?;
    let quote_amount_out_without_lp_fee = r.u64("quote_amount_out_without_lp_fee")?;
    let user_quote_amount_out = r.u64("user_quote_amount_out")?;
    let pool = r.pubkey("pool")?;
    let user = r.pubkey("user")?;
    let user_base_token_account = r.pubkey("user_base_token_account")?;
    let user_quote_token_account = r.pubkey("user_quote_token_account")?;
    let protocol_fee_recipient = r.pubkey("protocol_fee_recipient")?;
    let protocol_fee_recipient_token_account = r.pubkey("protocol_fee_recipient_token_account")?;
    let coin_creator = r.pubkey("coin_creator")?;
    let coin_creator_fee_basis_points = r.u64("coin_creator_fee_basis_points")?;
    let coin_creator_fee = r.u64("coin_creator_fee")?;
    let cashback_fee_basis_points = r.opt("cashback_fee_basis_points", Reader::u64)?;
    let cashback = r.opt("cashback", Reader::u64)?;
    let buyback_fee_basis_points = r.opt("buyback_fee_basis_points", Reader::u64)?;
    let buyback_fee = r.opt("buyback_fee", Reader::u64)?;
    let virtual_quote_reserves = r.opt("virtual_quote_reserves", Reader::i128)?;
    let can_boost = r.opt("can_boost", Reader::bool)?;
    let base_supply = r.opt("base_supply", Reader::u64)?;
    let holder_rewards_bps = r.opt("holder_rewards_bps", Reader::u64)?;
    let holder_rewards = r.opt("holder_rewards", Reader::u64)?;
    let trailing_bytes = r.finish(data_len)?;
    Ok(SellEvent {
        timestamp,
        base_amount_in,
        min_quote_amount_out,
        user_base_token_reserves,
        user_quote_token_reserves,
        pool_base_token_reserves,
        pool_quote_token_reserves,
        quote_amount_out,
        lp_fee_basis_points,
        lp_fee,
        protocol_fee_basis_points,
        protocol_fee,
        quote_amount_out_without_lp_fee,
        user_quote_amount_out,
        pool,
        user,
        user_base_token_account,
        user_quote_token_account,
        protocol_fee_recipient,
        protocol_fee_recipient_token_account,
        coin_creator,
        coin_creator_fee_basis_points,
        coin_creator_fee,
        cashback_fee_basis_points,
        cashback,
        buyback_fee_basis_points,
        buyback_fee,
        virtual_quote_reserves,
        can_boost,
        base_supply,
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
pub struct AmmPairMismatch {
    pub user: bool,
    pub pool: bool,
    pub side: bool,
    pub user_base_token_account: bool,
    pub user_quote_token_account: bool,
}

/// Pairing status of one decoded trade instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmmTradeEventPairing {
    /// Event found and consistent on user, pool, side and both user token accounts.
    Paired(PumpAmmEvent),
    /// No Buy/Sell event between this trade and the next trade instruction.
    MissingEvent,
    /// An event was found but disagrees with the instruction; not re-paired.
    Mismatch {
        event: PumpAmmEvent,
        mismatch: AmmPairMismatch,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedAmmTrade {
    pub trade: DecodedPumpAmmTrade,
    pub pairing: AmmTradeEventPairing,
}

/// Result of pairing one transaction; every category is counted.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmmPairingReport {
    pub trades: Vec<PairedAmmTrade>,
    /// Buy/Sell events not claimed by any trade instruction.
    pub orphan_events: Vec<PumpAmmEvent>,
    pub other_events: usize,
    pub unknown_events: usize,
    pub malformed_events: usize,
    /// Trade-discriminator instructions that failed to decode.
    pub malformed_trades: usize,
    /// Unknown-discriminator instructions of the program (COVERAGE GAP).
    pub unknown_instructions: usize,
    /// Known non-trade instructions (excluding the event-CPI tag).
    pub non_trade_instructions: usize,
}

impl AmmPairingReport {
    #[must_use]
    pub fn paired(&self) -> usize {
        self.trades
            .iter()
            .filter(|t| matches!(t.pairing, AmmTradeEventPairing::Paired(_)))
            .count()
    }

    #[must_use]
    pub fn missing(&self) -> usize {
        self.trades
            .iter()
            .filter(|t| matches!(t.pairing, AmmTradeEventPairing::MissingEvent))
            .count()
    }

    #[must_use]
    pub fn mismatched(&self) -> usize {
        self.trades
            .iter()
            .filter(|t| matches!(t.pairing, AmmTradeEventPairing::Mismatch { .. }))
            .count()
    }
}

/// Pair each decoded AMM trade of one transaction with the event of its own
/// execution. `instructions` must be the flattened list in execution order.
#[must_use]
pub fn pair_amm_trades_with_events(
    decoder: &PumpAmmDecoder,
    instructions: &[RawSolanaInstruction],
    slot: u64,
    transaction_index: u64,
) -> AmmPairingReport {
    let mut report = AmmPairingReport::default();
    // Index of the trade still waiting for its event.
    let mut open: Option<usize> = None;
    for ix in instructions {
        match decoder.classify(ix, slot, transaction_index) {
            PumpAmmInstructionOutcome::Trade(trade) => {
                report.trades.push(PairedAmmTrade {
                    trade: *trade,
                    pairing: AmmTradeEventPairing::MissingEvent,
                });
                open = report.trades.len().checked_sub(1);
                continue;
            }
            PumpAmmInstructionOutcome::Malformed {
                variant: Some(_), ..
            } => {
                report.malformed_trades += 1;
                open = None;
                continue;
            }
            PumpAmmInstructionOutcome::UnknownDiscriminator { .. } => {
                report.unknown_instructions += 1;
                continue;
            }
            PumpAmmInstructionOutcome::NonTrade(name)
                if name != crate::pump_amm::AMM_EVENT_CPI_NAME =>
            {
                report.non_trade_instructions += 1;
            }
            _ => {}
        }
        match decoder.classify_event(ix) {
            PumpAmmEventOutcome::Trade(ev) => {
                let slot_ref = open.take().and_then(|i| report.trades.get_mut(i));
                match slot_ref {
                    Some(p) => {
                        let mismatch = AmmPairMismatch {
                            user: ev.user() != p.trade.user,
                            pool: ev.pool() != p.trade.pool,
                            side: ev.side() != p.trade.side,
                            user_base_token_account: ev.user_base_token_account()
                                != p.trade.user_base_token_account,
                            user_quote_token_account: ev.user_quote_token_account()
                                != p.trade.user_quote_token_account,
                        };
                        p.pairing = if mismatch.user
                            || mismatch.pool
                            || mismatch.side
                            || mismatch.user_base_token_account
                            || mismatch.user_quote_token_account
                        {
                            AmmTradeEventPairing::Mismatch {
                                event: ev,
                                mismatch,
                            }
                        } else {
                            AmmTradeEventPairing::Paired(ev)
                        };
                    }
                    None => report.orphan_events.push(ev),
                }
            }
            PumpAmmEventOutcome::OtherEvent { .. } => report.other_events += 1,
            PumpAmmEventOutcome::UnknownEvent { .. } => report.unknown_events += 1,
            PumpAmmEventOutcome::Malformed { .. } => report.malformed_events += 1,
            PumpAmmEventOutcome::NotMine | PumpAmmEventOutcome::NotEventCpi => {}
        }
    }
    report
}
