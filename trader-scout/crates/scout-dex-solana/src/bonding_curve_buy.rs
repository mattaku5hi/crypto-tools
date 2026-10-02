//! pump.fun bonding-curve instruction decoder (all six trade variants
//! plus an explicit non-trade table).
//!
//! Scope note (AGENTS.md invariant #16): decodes
//! `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` specifically -- a
//! **confirmed** deployment per `docs/p0/deployment-registry.md`'s
//! "2026-10-01 confirmation" section (on-chain `executable: true`,
//! invoked in 5/5 probed transactions) against the official IDL
//! (`github.com/pump-fun/pump-public-docs`, `idl/pump.json`, commit
//! [`PUMP_IDL_COMMIT`]) committed verbatim as
//! `docs/p0/measurements/fixtures/pump_idl_e0687ae9.json`
//! (sha256 [`PUMP_IDL_SHA256`]). Every discriminator, account position
//! and argument layout below is derived from that file, and the unit
//! tests re-read the committed JSON and assert equality.
//!
//! ## Instruction classes (invariant #18)
//!
//! For an instruction of the confirmed program the outcome is exactly
//! one of:
//! - **Trade**: one of the six IDL trade instructions (`buy`,
//!   `buy_exact_sol_in`, `sell`, `buy_v2`, `buy_exact_quote_in_v2`,
//!   `sell_v2`), decoded into [`DecodedBondingCurveTrade`].
//! - **Known non-trade**: one of the other 41 IDL instructions or the
//!   Anchor event-CPI self-invocation tag `e445a52e51cb9a1d`
//!   ([`NON_TRADE_INSTRUCTIONS`]); counted by the caller, not decoded.
//! - **Malformed**: a trade discriminator with data shorter than the
//!   required args or more than [`MAX_TRAILING_ARG_BYTES`] trailing
//!   bytes, or too few accounts, or data shorter than 8 bytes.
//! - **Unknown discriminator**: 8 bytes that are in neither table. A
//!   COVERAGE GAP (the program changed or the IDL is stale), never
//!   `NotMine`.
//!
//! `NotMine` is reserved for instructions whose program id is not the
//! confirmed program (the program-id gate is load-bearing: the `buy`
//! discriminator was observed colliding with an Anchor `#[event_cpi]`
//! log under the unrelated PumpSwap AMM program).
//!
//! ## Account counts
//!
//! Account checks are "at least the IDL count". Live evidence
//! (`pump_bonding_curve_buy_probe.json`, slot 452380124): a top-level
//! `sell` carries 16 accounts (IDL 14) and `buy` carries 18 (IDL 16),
//! i.e. Anchor remaining accounts trail the fixed list. They never
//! shift the fixed positions read here. Live v2 buys carry 27 accounts
//! (one `buy_exact_quote_in_v2` had 28), `buy`/`buy_exact_sol_in` 18.
//!
//! ## Arg-length policy (invariants #16, #18)
//!
//! Live evidence, `docs/p0/measurements/fixtures/pump_variants_live_2026-10-02.json`
//! (captured 2026-10-02T13:40:20Z by `bins/scout-capture`, 3 pages,
//! successful transactions): `buy` 25 B x15 and 24 B x9;
//! `buy_exact_sol_in` 25 B x5 and 24 B x4; `buy_exact_quote_in_v2`
//! 24 B x14 and 25 B x2; `buy_v2` 24 B x2; `sell` 24 B x41; `sell_v2`
//! 24 B x20. A separate live run saw a successful `buy_exact_sol_in`
//! of 26 B (no committed sample). So the required args are always
//! present while trailing bytes vary, and the program executes them
//! successfully (Anchor Borsh does not require consuming all
//! instruction data; pump's `OptionBool` tolerates absence).
//!
//! Per variant, data must be >= 8 + the REQUIRED args (all args except
//! a trailing `track_volume: OptionBool`); those are parsed exactly.
//! Then, with `t` = bytes after the required args:
//! - `t == 0`: `track_volume = None`, `trailing_arg_bytes = 0`.
//! - `t == 1` and the variant has `track_volume`: `track_volume =
//!   Some(raw byte)`, `trailing_arg_bytes = 0`.
//! - any other `t <= `[`MAX_TRAILING_ARG_BYTES`] (e.g. 26 B
//!   `buy_exact_sol_in`, or 25 B `buy_exact_quote_in_v2` which has no
//!   `track_volume`): `track_volume = None`, `trailing_arg_bytes = t`;
//!   the encoding is NOT guessed.
//! - shorter than required, or `t > MAX_TRAILING_ARG_BYTES`: Malformed.
//!
//! ## Verification status per variant (invariant #16)
//!
//! A variant is [`VariantVerification::FixtureVerified`] only if a
//! SUCCESSFUL real-transaction golden fixture has the decoded
//! user/mint positions asserted against that same transaction's
//! token-balance deltas. Otherwise it is
//! [`VariantVerification::IdlOnly`]: decoded for layout/coverage, but
//! the qualification layer must not turn it into a confirmed buyer.
//!
//! Live promotion evidence (`pump_variants_live_2026-10-02.json`,
//! asserted in `scout-engine/tests/solana_buyer_intersect.rs`): 6/6
//! `buy_exact_sol_in`, 3/3 `buy`, 2/2 `buy_v2` and 5/5
//! `buy_exact_quote_in_v2` successful txs confirm the decoded
//! user/mint layout. For `buy` (== `amount`), `buy_v2` (== `amount`) and
//! `buy_exact_quote_in_v2` (>= `min_tokens_out`) the user's owner-keyed
//! net delta is > 0 in every tx. For `buy_exact_sol_in` 5 txs have a
//! positive user delta (>= `min_tokens_out`); the 6th (`bUh87USDuBC4...`)
//! is a router-forward: the layout positions are right, the user's net
//! is 0 and the tokens land on a different owner's new account with no
//! instruction evidence. That is a correct negative case (no buy for
//! either owner), not a layout failure, so all four are
//! FixtureVerified.
//!
//! Args are carried as raw `u64` by their IDL names; no floats.

/// Upper bound on bytes after the required args; more is Malformed so
/// arbitrary blobs are never accepted as trades.
pub const MAX_TRAILING_ARG_BYTES: usize = 32;

use scout_api::{DecodeOutcome, DeploymentScope, TxDecoder};
use scout_core::{RawSolanaInstruction, SolanaPubkey};

/// Official IDL commit the tables below were derived from.
pub const PUMP_IDL_COMMIT: &str = "e0687ae9b7e064a0f54efc7297c65eecfbba3a8f";
/// sha256 of the committed IDL file
/// `docs/p0/measurements/fixtures/pump_idl_e0687ae9.json`.
pub const PUMP_IDL_SHA256: &str =
    "ffe966c42f1af41652ee753fe2f1e3f7cd4077d7e6f49faf3138959c8b56064b";

/// `buy` (`66063d1201daebea`).
pub const BUY_INSTRUCTION_DISCRIMINATOR: [u8; 8] = [0x66, 0x06, 0x3d, 0x12, 0x01, 0xda, 0xeb, 0xea];
/// `buy_exact_sol_in` (`38fc74089edfcd5f`).
pub const BUY_EXACT_SOL_IN_INSTRUCTION_DISCRIMINATOR: [u8; 8] =
    [0x38, 0xfc, 0x74, 0x08, 0x9e, 0xdf, 0xcd, 0x5f];
/// `sell` (`33e685a4017f83ad`).
pub const SELL_INSTRUCTION_DISCRIMINATOR: [u8; 8] =
    [0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad];
/// `buy_v2` (`b817ee6167c5d33d`).
pub const BUY_V2_INSTRUCTION_DISCRIMINATOR: [u8; 8] =
    [0xb8, 0x17, 0xee, 0x61, 0x67, 0xc5, 0xd3, 0x3d];
/// `buy_exact_quote_in_v2` (`c2ab1c46684d5b2f`).
pub const BUY_EXACT_QUOTE_IN_V2_INSTRUCTION_DISCRIMINATOR: [u8; 8] =
    [0xc2, 0xab, 0x1c, 0x46, 0x68, 0x4d, 0x5b, 0x2f];
/// `sell_v2` (`5df6823ce7e940b2`).
pub const SELL_V2_INSTRUCTION_DISCRIMINATOR: [u8; 8] =
    [0x5d, 0xf6, 0x82, 0x3c, 0xe7, 0xe9, 0x40, 0xb2];
/// Anchor `#[event_cpi]` self-invocation tag (`e445a52e51cb9a1d`):
/// not an IDL instruction, a known non-trade.
pub const EVENT_CPI_DISCRIMINATOR: [u8; 8] = [0xe4, 0x45, 0xa5, 0x2e, 0x51, 0xcb, 0x9a, 0x1d];
/// Name used for the event-CPI tag in diagnostics.
pub const EVENT_CPI_NAME: &str = "<anchor-event-cpi>";

/// The 41 non-trade IDL instructions plus the event-CPI tag.
pub const NON_TRADE_INSTRUCTIONS: [(&str, [u8; 8]); 42] = [
    (
        "add_quote_control_mint",
        [0x02, 0x0e, 0x3d, 0x8a, 0xaa, 0x8e, 0x0e, 0x5f],
    ),
    (
        "add_quote_mint",
        [0x6f, 0x79, 0x15, 0x38, 0x28, 0x18, 0x5e, 0xd1],
    ),
    (
        "admin_cto",
        [0x7d, 0x7e, 0xd6, 0x86, 0x4d, 0xe5, 0xbc, 0x59],
    ),
    (
        "admin_set_idl_authority",
        [0x08, 0xd9, 0x60, 0xe7, 0x90, 0x68, 0xc0, 0x05],
    ),
    (
        "admin_update_token_incentives",
        [0xd1, 0x0b, 0x73, 0x57, 0xd5, 0x17, 0x7c, 0xcc],
    ),
    (
        "claim_cashback",
        [0x25, 0x3a, 0x23, 0x7e, 0xbe, 0x35, 0xe4, 0xc5],
    ),
    (
        "claim_cashback_v2",
        [0x7a, 0xf3, 0xcc, 0x41, 0x5e, 0x74, 0x1d, 0x37],
    ),
    (
        "claim_token_incentives",
        [0x10, 0x04, 0x47, 0x1c, 0xcc, 0x01, 0x28, 0x1b],
    ),
    (
        "close_user_volume_accumulator",
        [0xf9, 0x45, 0xa4, 0xda, 0x96, 0x67, 0x54, 0x8a],
    ),
    (
        "collect_creator_fee",
        [0x14, 0x16, 0x56, 0x7b, 0xc6, 0x1c, 0xdb, 0x84],
    ),
    (
        "collect_creator_fee_v2",
        [0xcf, 0x11, 0x8a, 0xf2, 0x04, 0x22, 0x13, 0x38],
    ),
    ("create", [0x18, 0x1e, 0xc8, 0x28, 0x05, 0x1c, 0x07, 0x77]),
    (
        "create_v2",
        [0xd6, 0x90, 0x4c, 0xec, 0x5f, 0x8b, 0x31, 0xb4],
    ),
    (
        "distribute_creator_fees",
        [0xa5, 0x72, 0x67, 0x00, 0x79, 0xce, 0xf7, 0x51],
    ),
    (
        "distribute_creator_fees_v2",
        [0xff, 0xcb, 0x13, 0x4f, 0xf4, 0x44, 0x08, 0x9f],
    ),
    (
        "distribute_fee_to_holders",
        [0x62, 0x36, 0x91, 0x61, 0x02, 0x46, 0xad, 0x2b],
    ),
    (
        "extend_account",
        [0xea, 0x66, 0xc2, 0xcb, 0x96, 0x48, 0x3e, 0xe5],
    ),
    (
        "get_minimum_distributable_fee",
        [0x75, 0xe1, 0x7f, 0xca, 0x86, 0x5f, 0x44, 0x23],
    ),
    (
        "init_user_volume_accumulator",
        [0x5e, 0x06, 0xca, 0x73, 0xff, 0x60, 0xe8, 0xb7],
    ),
    (
        "initialize",
        [0xaf, 0xaf, 0x6d, 0x1f, 0x0d, 0x98, 0x9b, 0xed],
    ),
    (
        "initialize_quote_control",
        [0xef, 0x49, 0xf5, 0xad, 0xd1, 0xb1, 0x54, 0x42],
    ),
    ("migrate", [0x9b, 0xea, 0xe7, 0x92, 0xec, 0x9e, 0xa2, 0x1e]),
    (
        "migrate_bonding_curve_creator",
        [0x57, 0x7c, 0x34, 0xbf, 0x34, 0x26, 0xd6, 0xe8],
    ),
    (
        "migrate_v2",
        [0xbb, 0xcb, 0x12, 0x1f, 0xce, 0xed, 0xfe, 0x29],
    ),
    (
        "remove_quote_control_mint",
        [0xdf, 0x07, 0xfd, 0x1a, 0x51, 0xa5, 0xda, 0xa6],
    ),
    (
        "remove_quote_mint",
        [0xb1, 0x41, 0xdf, 0x26, 0x58, 0xd1, 0x9e, 0x9b],
    ),
    (
        "set_creator",
        [0xfe, 0x94, 0xff, 0x70, 0xcf, 0x8e, 0xaa, 0xa5],
    ),
    (
        "set_mayhem_virtual_params",
        [0x3d, 0xa9, 0xbc, 0xbf, 0x99, 0x95, 0x2a, 0x61],
    ),
    (
        "set_metaplex_creator",
        [0x8a, 0x60, 0xae, 0xd9, 0x30, 0x55, 0xc5, 0xf6],
    ),
    (
        "set_params",
        [0x1b, 0xea, 0xb2, 0x34, 0x93, 0x02, 0xbb, 0x8d],
    ),
    (
        "set_quote_control_admin",
        [0x3e, 0x4f, 0xa1, 0xd3, 0xa5, 0xaa, 0xd6, 0xd2],
    ),
    (
        "set_reserved_fee_recipients",
        [0x6f, 0xac, 0xa2, 0xe8, 0x72, 0x59, 0xd5, 0x8e],
    ),
    (
        "set_virtual_quote_reserves",
        [0x65, 0x87, 0xbf, 0x68, 0x09, 0x58, 0x14, 0x60],
    ),
    (
        "sync_user_volume_accumulator",
        [0x56, 0x1f, 0xc0, 0x57, 0xa3, 0x57, 0x4f, 0xee],
    ),
    (
        "toggle_cashback_enabled",
        [0x73, 0x67, 0xe0, 0xff, 0xbd, 0x59, 0x56, 0xc3],
    ),
    (
        "toggle_create_v2",
        [0x1c, 0xff, 0xe6, 0xf0, 0xac, 0x6b, 0xcb, 0xab],
    ),
    (
        "toggle_mayhem_mode",
        [0x01, 0x09, 0x6f, 0xd0, 0x64, 0x1f, 0xff, 0xa3],
    ),
    (
        "update_buyback_config",
        [0xfb, 0xe0, 0xab, 0x92, 0xa0, 0x1a, 0x71, 0xe9],
    ),
    (
        "update_creator_fee_config",
        [0x3d, 0xaf, 0xa0, 0xf9, 0x42, 0x42, 0x88, 0xaf],
    ),
    (
        "update_global_authority",
        [0xe3, 0xb5, 0x4a, 0xc4, 0xd0, 0x15, 0x61, 0xd5],
    ),
    (
        "update_holder_reward_config",
        [0xe1, 0xfc, 0x42, 0x04, 0xc7, 0x23, 0xec, 0x10],
    ),
    (EVENT_CPI_NAME, EVENT_CPI_DISCRIMINATOR),
];

/// Buy or sell, from the trader's perspective on the base token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TradeSide {
    Buy,
    Sell,
}

/// Evidence level of a decoded variant (invariant #16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VariantVerification {
    /// Successful real-tx golden fixture with user/mint asserted
    /// against that tx's token-balance deltas.
    FixtureVerified,
    /// IDL layout only; no successful real-tx fixture checked.
    IdlOnly,
}

impl VariantVerification {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FixtureVerified => "FixtureVerified",
            Self::IdlOnly => "IdlOnly",
        }
    }
}

/// The six IDL trade instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PumpTradeVariant {
    Buy,
    BuyExactSolIn,
    Sell,
    BuyV2,
    BuyExactQuoteInV2,
    SellV2,
}

/// IDL-derived layout of one trade variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpTradeSpec {
    pub variant: PumpTradeVariant,
    /// IDL instruction name.
    pub name: &'static str,
    pub discriminator: [u8; 8],
    pub side: TradeSide,
    /// IDL account count; live transactions may carry more (remaining
    /// accounts), never fewer.
    pub min_accounts: usize,
    /// IDL instruction data length (8 + all args, including a trailing
    /// `track_volume`). Live data may be shorter by the optional
    /// `track_volume` byte or longer by trailing bytes; see
    /// [`PumpTradeSpec::required_data_len`] and the module doc policy.
    pub data_len: usize,
    /// IDL names of the two leading `u64` args.
    pub arg_names: [&'static str; 2],
    /// Trailing 1-byte `OptionBool track_volume` present.
    pub has_track_volume: bool,
    pub mint_idx: usize,
    pub quote_mint_idx: Option<usize>,
    pub bonding_curve_idx: usize,
    pub user_idx: usize,
    pub verification: VariantVerification,
}

impl PumpTradeSpec {
    /// Minimum data length: discriminator + required args (everything
    /// except a trailing optional `track_volume`).
    #[must_use]
    pub const fn required_data_len(&self) -> usize {
        if self.has_track_volume {
            self.data_len - 1
        } else {
            self.data_len
        }
    }
}

const SPEC_BUY: PumpTradeSpec = PumpTradeSpec {
    variant: PumpTradeVariant::Buy,
    name: "buy",
    discriminator: BUY_INSTRUCTION_DISCRIMINATOR,
    side: TradeSide::Buy,
    min_accounts: 16,
    data_len: 25,
    arg_names: ["amount", "max_sol_cost"],
    has_track_volume: true,
    mint_idx: 2,
    quote_mint_idx: None,
    bonding_curve_idx: 3,
    user_idx: 6,
    verification: VariantVerification::FixtureVerified,
};
const SPEC_BUY_EXACT_SOL_IN: PumpTradeSpec = PumpTradeSpec {
    variant: PumpTradeVariant::BuyExactSolIn,
    name: "buy_exact_sol_in",
    discriminator: BUY_EXACT_SOL_IN_INSTRUCTION_DISCRIMINATOR,
    side: TradeSide::Buy,
    min_accounts: 16,
    data_len: 25,
    arg_names: ["spendable_sol_in", "min_tokens_out"],
    has_track_volume: true,
    mint_idx: 2,
    quote_mint_idx: None,
    bonding_curve_idx: 3,
    user_idx: 6,
    verification: VariantVerification::FixtureVerified,
};
const SPEC_SELL: PumpTradeSpec = PumpTradeSpec {
    variant: PumpTradeVariant::Sell,
    name: "sell",
    discriminator: SELL_INSTRUCTION_DISCRIMINATOR,
    side: TradeSide::Sell,
    min_accounts: 14,
    data_len: 24,
    arg_names: ["amount", "min_sol_output"],
    has_track_volume: false,
    mint_idx: 2,
    quote_mint_idx: None,
    bonding_curve_idx: 3,
    user_idx: 6,
    verification: VariantVerification::FixtureVerified,
};
const SPEC_BUY_V2: PumpTradeSpec = PumpTradeSpec {
    variant: PumpTradeVariant::BuyV2,
    name: "buy_v2",
    discriminator: BUY_V2_INSTRUCTION_DISCRIMINATOR,
    side: TradeSide::Buy,
    min_accounts: 27,
    data_len: 24,
    arg_names: ["amount", "max_sol_cost"],
    has_track_volume: false,
    mint_idx: 1,
    quote_mint_idx: Some(2),
    bonding_curve_idx: 10,
    user_idx: 13,
    verification: VariantVerification::FixtureVerified,
};
const SPEC_BUY_EXACT_QUOTE_IN_V2: PumpTradeSpec = PumpTradeSpec {
    variant: PumpTradeVariant::BuyExactQuoteInV2,
    name: "buy_exact_quote_in_v2",
    discriminator: BUY_EXACT_QUOTE_IN_V2_INSTRUCTION_DISCRIMINATOR,
    side: TradeSide::Buy,
    min_accounts: 27,
    data_len: 24,
    arg_names: ["spendable_quote_in", "min_tokens_out"],
    has_track_volume: false,
    mint_idx: 1,
    quote_mint_idx: Some(2),
    bonding_curve_idx: 10,
    user_idx: 13,
    verification: VariantVerification::FixtureVerified,
};
const SPEC_SELL_V2: PumpTradeSpec = PumpTradeSpec {
    variant: PumpTradeVariant::SellV2,
    name: "sell_v2",
    discriminator: SELL_V2_INSTRUCTION_DISCRIMINATOR,
    side: TradeSide::Sell,
    min_accounts: 26,
    data_len: 24,
    arg_names: ["amount", "min_sol_output"],
    has_track_volume: false,
    mint_idx: 1,
    quote_mint_idx: Some(2),
    bonding_curve_idx: 10,
    user_idx: 13,
    verification: VariantVerification::FixtureVerified,
};

impl PumpTradeVariant {
    /// Every trade variant, in IDL-table order for reporting.
    pub const ALL: [Self; 6] = [
        Self::Buy,
        Self::BuyExactSolIn,
        Self::Sell,
        Self::BuyV2,
        Self::BuyExactQuoteInV2,
        Self::SellV2,
    ];
    /// Number of variants (array size for per-variant counters).
    pub const COUNT: usize = 6;

    #[must_use]
    pub const fn spec(self) -> &'static PumpTradeSpec {
        match self {
            Self::Buy => &SPEC_BUY,
            Self::BuyExactSolIn => &SPEC_BUY_EXACT_SOL_IN,
            Self::Sell => &SPEC_SELL,
            Self::BuyV2 => &SPEC_BUY_V2,
            Self::BuyExactQuoteInV2 => &SPEC_BUY_EXACT_QUOTE_IN_V2,
            Self::SellV2 => &SPEC_SELL_V2,
        }
    }

    /// Stable index into per-variant counter arrays (`0..COUNT`).
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Buy => 0,
            Self::BuyExactSolIn => 1,
            Self::Sell => 2,
            Self::BuyV2 => 3,
            Self::BuyExactQuoteInV2 => 4,
            Self::SellV2 => 5,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.spec().name
    }

    #[must_use]
    pub const fn side(self) -> TradeSide {
        self.spec().side
    }

    #[must_use]
    pub const fn verification(self) -> VariantVerification {
        self.spec().verification
    }

    fn from_discriminator(discriminator: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|v| v.spec().discriminator.as_slice() == discriminator)
    }
}

/// One raw `u64` instruction argument with its IDL name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamedU64 {
    pub name: &'static str,
    pub value: u64,
}

/// A decoded trade instruction of any of the six variants.
///
/// `args[0]` / `args[1]` are the two leading `u64` IDL args in order
/// (e.g. `amount`, `max_sol_cost`). They are declared sizes/limits, not
/// executed amounts; economics come from balance deltas elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBondingCurveTrade {
    pub variant: PumpTradeVariant,
    pub side: TradeSide,
    pub user: SolanaPubkey,
    /// Base (traded) token mint.
    pub mint: SolanaPubkey,
    /// Quote mint, present for the v2 layouts only.
    pub quote_mint: Option<SolanaPubkey>,
    pub bonding_curve: SolanaPubkey,
    pub args: [NamedU64; 2],
    /// Raw `OptionBool` byte: `Some` only when the variant has
    /// `track_volume` and exactly one trailing byte was present.
    pub track_volume: Option<u8>,
    /// Number of data bytes beyond the required args that were NOT
    /// interpreted as `track_volume` (0 when absent or consumed as
    /// `track_volume`). Their encoding is deliberately not guessed.
    pub trailing_arg_bytes: usize,
    pub slot: u64,
    pub transaction_index: u64,
    pub instruction_index: u32,
}

impl DecodedBondingCurveTrade {
    #[must_use]
    pub fn verification(&self) -> VariantVerification {
        self.variant.verification()
    }
}

/// Rich classification of one instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpInstructionOutcome {
    /// Program id is not the confirmed program.
    NotMine,
    Trade(DecodedBondingCurveTrade),
    /// Known non-trade (IDL instruction or event-CPI tag); the name.
    NonTrade(&'static str),
    /// Trade discriminator with broken structure, or data < 8 bytes.
    /// COVERAGE GAP.
    Malformed {
        variant: Option<PumpTradeVariant>,
        reason: String,
    },
    /// 8-byte discriminator in neither table. COVERAGE GAP.
    UnknownDiscriminator {
        discriminator: [u8; 8],
    },
}

/// Payload of the `TxDecoder` impl: what a successful decode yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpInstruction {
    Trade(DecodedBondingCurveTrade),
    NonTrade(&'static str),
}

/// A `TxDecoder` for the pump.fun bonding-curve program, scoped to one
/// `DeploymentScope` (invariant #16: mandatory at registration).
#[derive(Debug, Clone)]
pub struct BondingCurveBuyDecoder {
    scope: DeploymentScope,
}

impl BondingCurveBuyDecoder {
    #[must_use]
    pub fn new(scope: DeploymentScope) -> Self {
        Self { scope }
    }

    /// Program-id gate: whether the instruction belongs to the scoped
    /// deployment. An empty `contract_addresses` matches nothing.
    #[must_use]
    pub(crate) fn is_program(&self, instruction: &RawSolanaInstruction) -> bool {
        self.scope.contract_addresses.iter().any(
            |addr| matches!(addr, scout_core::AddressBytes::Solana(p) if *p == instruction.program_id),
        )
    }

    /// Classify one instruction. Applies the program-id gate first: an
    /// empty `contract_addresses` means nothing is ever "mine".
    #[must_use]
    pub fn classify(
        &self,
        instruction: &RawSolanaInstruction,
        slot: u64,
        transaction_index: u64,
    ) -> PumpInstructionOutcome {
        if !self.is_program(instruction) {
            return PumpInstructionOutcome::NotMine;
        }
        classify_pump_instruction(instruction, slot, transaction_index)
    }
}

impl TxDecoder<RawSolanaInstruction, PumpInstruction> for BondingCurveBuyDecoder {
    fn scope(&self) -> &DeploymentScope {
        &self.scope
    }

    fn decode(&self, instruction: &RawSolanaInstruction) -> DecodeOutcome<PumpInstruction> {
        match self.classify(instruction, 0, 0) {
            PumpInstructionOutcome::NotMine => DecodeOutcome::NotMine,
            PumpInstructionOutcome::Trade(t) => DecodeOutcome::Decoded(PumpInstruction::Trade(t)),
            PumpInstructionOutcome::NonTrade(n) => {
                DecodeOutcome::Decoded(PumpInstruction::NonTrade(n))
            }
            PumpInstructionOutcome::Malformed { reason, .. } => DecodeOutcome::Malformed(reason),
            PumpInstructionOutcome::UnknownDiscriminator { discriminator } => {
                DecodeOutcome::Malformed(format!(
                    "unknown pump.fun instruction discriminator {} (not in IDL {PUMP_IDL_COMMIT})",
                    hex8(&discriminator)
                ))
            }
        }
    }
}

/// Lowercase hex of an 8-byte discriminator.
#[must_use]
pub fn hex8(bytes: &[u8; 8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(16);
    for b in bytes {
        // Writing to a String cannot fail.
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Classify an instruction ASSUMED to belong to the confirmed program
/// (no program-id check; use [`BondingCurveBuyDecoder::classify`]).
#[must_use]
pub fn classify_pump_instruction(
    instruction: &RawSolanaInstruction,
    slot: u64,
    transaction_index: u64,
) -> PumpInstructionOutcome {
    let Some(disc_slice) = instruction.data.get(0..8) else {
        return PumpInstructionOutcome::Malformed {
            variant: None,
            reason: format!(
                "instruction of the pump.fun program has {} data bytes, fewer than the 8-byte discriminator",
                instruction.data.len()
            ),
        };
    };
    if let Some(variant) = PumpTradeVariant::from_discriminator(disc_slice) {
        return match decode_trade(variant, instruction, slot, transaction_index) {
            Ok(trade) => PumpInstructionOutcome::Trade(trade),
            Err(reason) => PumpInstructionOutcome::Malformed {
                variant: Some(variant),
                reason,
            },
        };
    }
    if let Some((name, _)) = NON_TRADE_INSTRUCTIONS
        .iter()
        .find(|(_, d)| d.as_slice() == disc_slice)
    {
        return PumpInstructionOutcome::NonTrade(name);
    }
    let Ok(discriminator) = <[u8; 8]>::try_from(disc_slice) else {
        return PumpInstructionOutcome::Malformed {
            variant: None,
            reason: "discriminator slice is not 8 bytes".to_string(),
        };
    };
    PumpInstructionOutcome::UnknownDiscriminator { discriminator }
}

fn decode_trade(
    variant: PumpTradeVariant,
    instruction: &RawSolanaInstruction,
    slot: u64,
    transaction_index: u64,
) -> Result<DecodedBondingCurveTrade, String> {
    let spec = variant.spec();
    let name = spec.name;
    let required = spec.required_data_len();
    let Some(trailing_arg_bytes) = instruction.data.len().checked_sub(required) else {
        return Err(format!(
            "instruction matches {name} discriminator but data has {} bytes, fewer than the \
             {required} required by the IDL args (IDL commit {PUMP_IDL_COMMIT})",
            instruction.data.len(),
        ));
    };
    if trailing_arg_bytes > MAX_TRAILING_ARG_BYTES {
        return Err(format!(
            "instruction matches {name} discriminator but data has {} bytes: {trailing_arg_bytes} \
             trailing bytes exceed the {MAX_TRAILING_ARG_BYTES}-byte bound (required {required})",
            instruction.data.len(),
        ));
    }
    if instruction.accounts.len() < spec.min_accounts {
        return Err(format!(
            "instruction matches {name} discriminator but has {} accounts, expected at least {} \
             per the official IDL's {name} account list",
            instruction.accounts.len(),
            spec.min_accounts
        ));
    }
    let [arg0_name, arg1_name] = spec.arg_names;
    let arg0 = read_u64_le(&instruction.data, 8)
        .ok_or_else(|| format!("{name}: {arg0_name} is unreadable"))?;
    let arg1 = read_u64_le(&instruction.data, 16)
        .ok_or_else(|| format!("{name}: {arg1_name} is unreadable"))?;
    let (track_volume, trailing_arg_bytes) = if spec.has_track_volume && trailing_arg_bytes == 1 {
        (
            Some(
                *instruction
                    .data
                    .get(required)
                    .ok_or_else(|| format!("{name}: track_volume byte is unreadable"))?,
            ),
            0,
        )
    } else {
        (None, trailing_arg_bytes)
    };
    let account = |idx: usize, what: &str| -> Result<SolanaPubkey, String> {
        instruction
            .accounts
            .get(idx)
            .copied()
            .ok_or_else(|| format!("{name}: {what} account at position {idx} is missing"))
    };
    let quote_mint = match spec.quote_mint_idx {
        Some(idx) => Some(account(idx, "quote_mint")?),
        None => None,
    };
    Ok(DecodedBondingCurveTrade {
        variant,
        side: spec.side,
        user: account(spec.user_idx, "user")?,
        mint: account(spec.mint_idx, "mint")?,
        quote_mint,
        bonding_curve: account(spec.bonding_curve_idx, "bonding_curve")?,
        args: [
            NamedU64 {
                name: arg0_name,
                value: arg0,
            },
            NamedU64 {
                name: arg1_name,
                value: arg1,
            },
        ],
        track_volume,
        trailing_arg_bytes,
        slot,
        transaction_index,
        instruction_index: instruction.instruction_index,
    })
}

fn read_u64_le(data: &[u8], offset: usize) -> Option<u64> {
    let slice = data.get(offset..offset.checked_add(8)?)?;
    let array: [u8; 8] = slice.try_into().ok()?;
    Some(u64::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use sha2::{Digest as _, Sha256};

    use super::*;

    fn idl_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/p0/measurements/fixtures/pump_idl_e0687ae9.json")
    }

    fn idl() -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(idl_path()).unwrap()).unwrap()
    }

    fn idl_disc(ix: &serde_json::Value) -> [u8; 8] {
        let v: Vec<u8> = ix["discriminator"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| u8::try_from(n.as_u64().unwrap()).unwrap())
            .collect();
        v.try_into().unwrap()
    }

    fn idl_ix<'a>(idl: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
        idl["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == name)
            .unwrap()
    }

    fn real_program_id() -> SolanaPubkey {
        decode_pubkey_for_test("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
    }

    fn real_scope() -> DeploymentScope {
        DeploymentScope {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Solana,
                network_id: scout_core::NetworkId::SolanaCluster(
                    scout_core::SolanaCluster::Mainnet,
                ),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            contract_addresses: vec![scout_core::AddressBytes::Solana(real_program_id())],
            active_from: 0,
            active_until: None,
        }
    }

    fn decoder() -> BondingCurveBuyDecoder {
        BondingCurveBuyDecoder::new(real_scope())
    }

    fn account_list(count: usize) -> Vec<SolanaPubkey> {
        (0..count)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = u8::try_from(i).expect("test account count fits in u8");
                key[1] = 0xA5;
                key
            })
            .collect()
    }

    /// Synthetic instruction laid out per the variant's IDL spec.
    fn synthetic(variant: PumpTradeVariant) -> RawSolanaInstruction {
        let spec = variant.spec();
        let mut data = spec.discriminator.to_vec();
        data.extend_from_slice(&1111u64.to_le_bytes());
        data.extend_from_slice(&2222u64.to_le_bytes());
        if spec.has_track_volume {
            data.push(1);
        }
        RawSolanaInstruction {
            program_id: real_program_id(),
            accounts: account_list(spec.min_accounts),
            data,
            instruction_index: 4,
        }
    }

    fn trade_of(outcome: PumpInstructionOutcome) -> DecodedBondingCurveTrade {
        match outcome {
            PumpInstructionOutcome::Trade(t) => t,
            other => panic!("expected Trade, got {other:?}"),
        }
    }

    #[test]
    fn idl_file_sha256_matches_pinned_constant() {
        let bytes = std::fs::read(idl_path()).unwrap();
        let digest = Sha256::digest(&bytes);
        assert_eq!(hex_lower(&digest[..]), PUMP_IDL_SHA256);
    }

    #[test]
    fn trade_discriminators_and_layouts_equal_the_idl_file() {
        let idl = idl();
        for variant in PumpTradeVariant::ALL {
            let spec = variant.spec();
            let ix = idl_ix(&idl, spec.name);
            assert_eq!(idl_disc(ix), spec.discriminator, "{}", spec.name);
            let accounts = ix["accounts"].as_array().unwrap();
            assert_eq!(accounts.len(), spec.min_accounts, "{}", spec.name);
            let name_at = |i: usize| accounts[i]["name"].as_str().unwrap().to_string();
            assert_eq!(name_at(spec.user_idx), "user", "{}", spec.name);
            assert_eq!(name_at(spec.bonding_curve_idx), "bonding_curve");
            match spec.quote_mint_idx {
                Some(q) => {
                    assert_eq!(name_at(spec.mint_idx), "base_mint");
                    assert_eq!(name_at(q), "quote_mint");
                }
                None => assert_eq!(name_at(spec.mint_idx), "mint"),
            }
            let args = ix["args"].as_array().unwrap();
            let mut len = 8usize;
            for (i, arg) in args.iter().enumerate() {
                if i < 2 {
                    assert_eq!(arg["name"], spec.arg_names[i], "{}", spec.name);
                    assert_eq!(arg["type"], "u64");
                    len += 8;
                } else {
                    assert_eq!(arg["name"], "track_volume");
                    assert_eq!(arg["type"]["defined"]["name"], "OptionBool");
                    len += 1;
                }
            }
            assert_eq!(args.len(), if spec.has_track_volume { 3 } else { 2 });
            assert_eq!(len, spec.data_len, "{}", spec.name);
        }
        // Per-variant public constants are the same bytes.
        assert_eq!(idl_disc(idl_ix(&idl, "buy")), BUY_INSTRUCTION_DISCRIMINATOR);
        assert_eq!(
            idl_disc(idl_ix(&idl, "sell")),
            SELL_INSTRUCTION_DISCRIMINATOR
        );
    }

    #[test]
    fn non_trade_table_is_exactly_the_remaining_idl_instructions_plus_event_cpi() {
        let idl = idl();
        let trade_names: BTreeSet<&str> = PumpTradeVariant::ALL.iter().map(|v| v.name()).collect();
        let mut expected: BTreeSet<(String, [u8; 8])> = BTreeSet::new();
        for ix in idl["instructions"].as_array().unwrap() {
            let name = ix["name"].as_str().unwrap();
            if !trade_names.contains(name) {
                expected.insert((name.to_string(), idl_disc(ix)));
            }
        }
        assert_eq!(expected.len(), 41);
        expected.insert((EVENT_CPI_NAME.to_string(), EVENT_CPI_DISCRIMINATOR));
        let actual: BTreeSet<(String, [u8; 8])> = NON_TRADE_INSTRUCTIONS
            .iter()
            .map(|(n, d)| ((*n).to_string(), *d))
            .collect();
        assert_eq!(actual, expected);
        assert_eq!(NON_TRADE_INSTRUCTIONS.len(), 42);
        // No discriminator appears twice across trade + non-trade.
        let mut all: BTreeSet<[u8; 8]> = actual.iter().map(|(_, d)| *d).collect();
        for v in PumpTradeVariant::ALL {
            assert!(all.insert(v.spec().discriminator));
        }
        assert_eq!(all.len(), 48);
        assert_eq!(hex8(&EVENT_CPI_DISCRIMINATOR), "e445a52e51cb9a1d");
    }

    #[test]
    fn variant_verification_statuses() {
        use PumpTradeVariant as V;
        use VariantVerification::FixtureVerified;
        let got: Vec<_> = V::ALL
            .iter()
            .map(|v| (v.name(), v.verification()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("buy", FixtureVerified),
                ("buy_exact_sol_in", FixtureVerified),
                ("sell", FixtureVerified),
                ("buy_v2", FixtureVerified),
                ("buy_exact_quote_in_v2", FixtureVerified),
                ("sell_v2", FixtureVerified),
            ]
        );
        for (i, v) in V::ALL.iter().enumerate() {
            assert_eq!(v.index(), i);
        }
    }

    #[test]
    fn every_variant_decodes_from_a_synthetic_idl_layout() {
        for variant in PumpTradeVariant::ALL {
            let spec = variant.spec();
            let accounts = account_list(spec.min_accounts);
            let t = trade_of(decoder().classify(&synthetic(variant), 77, 9));
            assert_eq!(t.variant, variant);
            assert_eq!(t.side, spec.side);
            assert_eq!(t.user, accounts[spec.user_idx]);
            assert_eq!(t.mint, accounts[spec.mint_idx]);
            assert_eq!(t.quote_mint, spec.quote_mint_idx.map(|i| accounts[i]));
            assert_eq!(t.bonding_curve, accounts[spec.bonding_curve_idx]);
            assert_eq!(t.args[0].name, spec.arg_names[0]);
            assert_eq!(t.args[0].value, 1111);
            assert_eq!(t.args[1].name, spec.arg_names[1]);
            assert_eq!(t.args[1].value, 2222);
            assert_eq!(t.track_volume, spec.has_track_volume.then_some(1));
            assert_eq!(t.trailing_arg_bytes, 0);
            assert_eq!(
                (t.slot, t.transaction_index, t.instruction_index),
                (77, 9, 4)
            );
        }
    }

    #[test]
    fn arg_length_policy_per_variant() {
        for variant in PumpTradeVariant::ALL {
            let spec = variant.spec();
            let required = spec.required_data_len();
            assert_eq!(required, 24, "{}", spec.name);
            let with_len = |len: usize| {
                let mut ix = synthetic(variant);
                ix.data.truncate(24);
                ix.data.resize(len, 0x07);
                ix
            };
            // Shorter than required: Malformed.
            for len in [8, 16, 23] {
                assert!(
                    matches!(
                        decoder().classify(&with_len(len), 0, 0),
                        PumpInstructionOutcome::Malformed { variant: Some(v), .. } if v == variant
                    ),
                    "{} len {len}",
                    spec.name
                );
            }
            // Exactly required: no track_volume, no trailing.
            let t = trade_of(decoder().classify(&with_len(24), 0, 0));
            assert_eq!((t.track_volume, t.trailing_arg_bytes), (None, 0));
            assert_eq!((t.args[0].value, t.args[1].value), (1111, 2222));
            // One trailing byte.
            let t = trade_of(decoder().classify(&with_len(25), 0, 0));
            if spec.has_track_volume {
                assert_eq!((t.track_volume, t.trailing_arg_bytes), (Some(7), 0));
            } else {
                assert_eq!((t.track_volume, t.trailing_arg_bytes), (None, 1));
            }
            // Two trailing bytes (26 B buy_exact_sol_in live case).
            let t = trade_of(decoder().classify(&with_len(26), 0, 0));
            assert_eq!((t.track_volume, t.trailing_arg_bytes), (None, 2));
            // Upper bound.
            let max = required + MAX_TRAILING_ARG_BYTES;
            let t = trade_of(decoder().classify(&with_len(max), 0, 0));
            assert_eq!(t.trailing_arg_bytes, MAX_TRAILING_ARG_BYTES);
            assert!(matches!(
                decoder().classify(&with_len(max + 1), 0, 0),
                PumpInstructionOutcome::Malformed { variant: Some(v), .. } if v == variant
            ));
        }
    }

    #[test]
    fn too_few_accounts_are_malformed_extra_accounts_ok() {
        for variant in PumpTradeVariant::ALL {
            let mut few = synthetic(variant);
            few.accounts.pop();
            assert!(matches!(
                decoder().classify(&few, 0, 0),
                PumpInstructionOutcome::Malformed { .. }
            ));
            let mut extra = synthetic(variant);
            extra.accounts = account_list(variant.spec().min_accounts + 2);
            assert!(matches!(
                decoder().classify(&extra, 0, 0),
                PumpInstructionOutcome::Trade(_)
            ));
        }
    }

    #[test]
    fn known_non_trade_is_classified_not_decoded() {
        for (name, disc) in NON_TRADE_INSTRUCTIONS {
            let ix = RawSolanaInstruction {
                program_id: real_program_id(),
                accounts: vec![],
                data: disc.to_vec(),
                instruction_index: 0,
            };
            assert_eq!(
                decoder().classify(&ix, 0, 0),
                PumpInstructionOutcome::NonTrade(name)
            );
        }
    }

    #[test]
    fn unknown_discriminator_under_the_program_is_a_gap_not_not_mine() {
        let ix = RawSolanaInstruction {
            program_id: real_program_id(),
            accounts: vec![],
            data: vec![1, 2, 3, 4, 5, 6, 7, 8, 9],
            instruction_index: 0,
        };
        assert_eq!(
            decoder().classify(&ix, 0, 0),
            PumpInstructionOutcome::UnknownDiscriminator {
                discriminator: [1, 2, 3, 4, 5, 6, 7, 8]
            }
        );
        assert!(decoder().decode(&ix).is_malformed());
    }

    #[test]
    fn data_shorter_than_a_discriminator_under_the_program_is_malformed() {
        let ix = RawSolanaInstruction {
            program_id: real_program_id(),
            accounts: vec![],
            data: vec![0x01, 0x02],
            instruction_index: 0,
        };
        assert!(matches!(
            decoder().classify(&ix, 0, 0),
            PumpInstructionOutcome::Malformed { variant: None, .. }
        ));
    }

    #[test]
    fn tx_decoder_trait_maps_outcomes() {
        let d = decoder();
        assert!(matches!(
            d.decode(&synthetic(PumpTradeVariant::Buy)),
            DecodeOutcome::Decoded(PumpInstruction::Trade(_))
        ));
        let mut cpi = synthetic(PumpTradeVariant::Buy);
        cpi.data = EVENT_CPI_DISCRIMINATOR.to_vec();
        assert_eq!(
            d.decode(&cpi),
            DecodeOutcome::Decoded(PumpInstruction::NonTrade(EVENT_CPI_NAME))
        );
    }

    #[test]
    fn decoder_rejects_matching_discriminator_from_an_unregistered_program() {
        let mut instruction = synthetic(PumpTradeVariant::Buy);
        instruction.program_id = [0xBB; 32];
        assert_eq!(
            decoder().classify(&instruction, 0, 0),
            PumpInstructionOutcome::NotMine
        );
        assert_eq!(decoder().decode(&instruction), DecodeOutcome::NotMine);
    }

    #[test]
    fn real_pumpswap_event_cpi_payload_is_not_decoded_as_a_trade() {
        // Real bytes from a live PumpSwap AMM (pAMMBay6...) Anchor
        // #[event_cpi] log: discriminator 66063d1201daebea ==
        // BUY_INSTRUCTION_DISCRIMINATOR but a different program. The
        // program-id gate must keep it NotMine.
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&BUY_INSTRUCTION_DISCRIMINATOR);
        data.extend_from_slice(&hex_decode("f1250300000000000a00000000000000"));
        let instruction = RawSolanaInstruction {
            program_id: decode_pubkey_for_test("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"),
            accounts: vec![[0x11; 32], [0x22; 32], [0x33; 32], [0x44; 32]],
            data,
            instruction_index: 7,
        };
        assert_eq!(
            decoder().classify(&instruction, 0, 0),
            PumpInstructionOutcome::NotMine
        );
    }

    #[test]
    fn real_buy_and_sell_data_decode_with_live_argument_values() {
        let mut buy = synthetic(PumpTradeVariant::Buy);
        buy.data = hex_decode("66063d1201daebeab6f512c1b502000000f2ac290000000001");
        let t = trade_of(decoder().classify(&buy, 452_380_124, 645));
        assert_eq!(t.args[0].value, 2_979_651_581_366);
        assert_eq!(t.args[1].value, 699_200_000);
        assert_eq!(t.track_volume, Some(1));
        let mut sell = synthetic(PumpTradeVariant::Sell);
        sell.data = hex_decode("33e685a4017f83adddede2ca280000000000000000000000");
        let t = trade_of(decoder().classify(&sell, 452_380_124, 671));
        assert_eq!(t.args[0].value, 175_202_561_501);
        assert_eq!(t.args[1].value, 0);
    }

    fn hex_lower(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Minimal base58 decode for a Solana pubkey, test-only.
    fn decode_pubkey_for_test(s: &str) -> SolanaPubkey {
        const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
        let mut bytes = vec![0u8; 1];
        for c in s.chars() {
            let byte_value = u8::try_from(c).expect("test fixture pubkey must be ASCII base58");
            let digit = u32::try_from(
                ALPHABET
                    .iter()
                    .position(|&b| b == byte_value)
                    .expect("test fixture pubkey must be valid base58"),
            )
            .expect("base58 alphabet index fits in u32");
            let mut carry = digit;
            for byte in bytes.iter_mut() {
                carry += u32::from(*byte) * 58;
                *byte = u8::try_from(carry & 0xFF).expect("masked byte fits in u8");
                carry >>= 8;
            }
            while carry > 0 {
                bytes.push(u8::try_from(carry & 0xFF).expect("masked byte fits in u8"));
                carry >>= 8;
            }
        }
        for c in s.chars() {
            if c == '1' {
                bytes.push(0);
            } else {
                break;
            }
        }
        bytes.reverse();
        while bytes.len() < 32 {
            bytes.insert(0, 0);
        }
        let mut out = [0u8; 32];
        let start = bytes.len().saturating_sub(32);
        out.copy_from_slice(&bytes[start..]);
        out
    }

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("test fixture hex must be valid"))
            .collect()
    }
}
