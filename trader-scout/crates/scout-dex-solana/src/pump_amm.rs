//! PumpSwap AMM (`pump_amm`) instruction decoder: the three trade
//! instructions `buy`, `buy_exact_quote_in`, `sell` plus an explicit
//! non-trade table.
//!
//! Scope note (AGENTS.md invariants #16, #18): decodes
//! `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA` against the official IDL
//! (`pump-fun/pump-public-docs`, `idl/pump_amm.json`, commit
//! [`PUMP_AMM_IDL_COMMIT`]) committed verbatim as
//! `docs/p0/measurements/fixtures/pump_amm_idl_e0687ae9.json` (sha256
//! [`PUMP_AMM_IDL_SHA256`]). Every discriminator, account position and
//! argument layout below is derived from that file and the unit tests
//! re-read the JSON and assert equality.
//!
//! ## Instruction classes (invariant #18)
//!
//! For an instruction of the scoped program the outcome is exactly one of
//! trade (decoded), known non-trade (the other 29 IDL instructions and the
//! Anchor event-CPI tag), malformed (a trade discriminator with a wrong
//! data length or too few accounts, or < 8 data bytes) and unknown
//! discriminator (a COVERAGE GAP). `NotMine` is only for other programs:
//! the program-id gate is load-bearing because the `buy` discriminator
//! also exists in unrelated programs.
//!
//! ## Arg-length policy
//!
//! IDL args: `buy` / `buy_exact_quote_in` are `u64, u64, OptionBool`
//! (25 bytes with `track_volume`), `sell` is `u64, u64` (24 bytes).
//! Live fixtures contain both 24 and 25 byte buys (Anchor tolerates the
//! absent trailing `OptionBool`). Accepted: 24 or 25 bytes for the two buy
//! variants (25 => `track_volume = Some(raw byte)`), exactly 24 for
//! `sell`. Anything else is Malformed and counted, never guessed.
//!
//! ## Accounts
//!
//! At least the IDL count (23 / 23 / 21); live buys carry 25-26 (Anchor
//! remaining accounts trail the fixed list and never shift fixed
//! positions). Positions read: `pool` 0, `user` 1, `base_mint` 3,
//! `quote_mint` 4, `user_base_token_account` 5,
//! `user_quote_token_account` 6 (identical for all three variants).
//!
//! ## Verification
//!
//! See [`PumpAmmTradeVariant::verification`] and the reconciliation module
//! ([`crate::reconcile_pump_amm_transaction`]) for the evidence behind the
//! statuses.

use scout_api::{DecodeOutcome, DeploymentScope, TxDecoder};
use scout_core::{
    AddressBytes, ChainFamily, ChainKey, GenesisIdentity, NetworkId, RawSolanaInstruction,
    SolanaCluster, SolanaPubkey,
};

use crate::bonding_curve_buy::EVENT_CPI_DISCRIMINATOR;
use crate::bonding_curve_buy::{NamedU64, TradeSide, VariantVerification, hex8};

/// Official IDL commit the tables below were derived from.
pub const PUMP_AMM_IDL_COMMIT: &str = "e0687ae9b7e064a0f54efc7297c65eecfbba3a8f";
/// sha256 of `docs/p0/measurements/fixtures/pump_amm_idl_e0687ae9.json`.
pub const PUMP_AMM_IDL_SHA256: &str =
    "2091433899b07d003d98118ae6cd3c628960fd393b40710b6e15bce6d0e7f2d1";
/// PumpSwap AMM program id (base58).
pub const PUMP_AMM_PROGRAM_ID: &str = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";
/// PumpSwap AMM program id bytes.
pub const PUMP_AMM_PROGRAM_ID_BYTES: SolanaPubkey = [
    0x0c, 0x14, 0xde, 0xfc, 0x82, 0x5e, 0xc6, 0x76, 0x94, 0x25, 0x08, 0x18, 0xbb, 0x65, 0x40, 0x65,
    0xf4, 0x29, 0x8d, 0x31, 0x56, 0xd5, 0x71, 0xb4, 0xd4, 0xf8, 0x09, 0x0c, 0x18, 0xe9, 0xa8, 0x63,
];
/// Wrapped SOL mint (`So111...112`) bytes.
pub const WRAPPED_SOL_MINT: SolanaPubkey = [
    0x06, 0x9b, 0x88, 0x57, 0xfe, 0xab, 0x81, 0x84, 0xfb, 0x68, 0x7f, 0x63, 0x46, 0x18, 0xc0, 0x35,
    0xda, 0xc4, 0x39, 0xdc, 0x1a, 0xeb, 0x3b, 0x55, 0x98, 0xa0, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x01,
];

/// Data length without the trailing optional `track_volume`.
pub const TRADE_DATA_LEN_REQUIRED: usize = 24;

/// `buy` (`66063d1201daebea`).
pub const AMM_BUY_DISCRIMINATOR: [u8; 8] = [0x66, 0x06, 0x3d, 0x12, 0x01, 0xda, 0xeb, 0xea];
/// `buy_exact_quote_in` (`c62e1552b4d9e870`).
pub const AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR: [u8; 8] =
    [0xc6, 0x2e, 0x15, 0x52, 0xb4, 0xd9, 0xe8, 0x70];
/// `sell` (`33e685a4017f83ad`).
pub const AMM_SELL_DISCRIMINATOR: [u8; 8] = [0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad];

/// The 29 non-trade IDL instructions plus the Anchor event-CPI tag.
pub const AMM_NON_TRADE_INSTRUCTIONS: [(&str, [u8; 8]); 30] = [
    (
        "admin_cto_pool",
        [0x2d, 0x3d, 0xa5, 0x97, 0x68, 0x00, 0x31, 0xbd],
    ),
    (
        "admin_update_token_incentives",
        [0xd1, 0x0b, 0x73, 0x57, 0xd5, 0x17, 0x7c, 0xcc],
    ),
    (
        "boost_buy_and_burn",
        [0x69, 0x44, 0x06, 0xaf, 0x00, 0x07, 0x23, 0xa2],
    ),
    (
        "claim_cashback",
        [0x25, 0x3a, 0x23, 0x7e, 0xbe, 0x35, 0xe4, 0xc5],
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
        "collect_coin_creator_fee",
        [0xa0, 0x39, 0x59, 0x2a, 0xb5, 0x8b, 0x2b, 0x42],
    ),
    (
        "create_config",
        [0xc9, 0xcf, 0xf3, 0x72, 0x4b, 0x6f, 0x2f, 0xbd],
    ),
    (
        "create_pool",
        [0xe9, 0x92, 0xd1, 0x8e, 0xcf, 0x68, 0x40, 0xbc],
    ),
    ("deposit", [0xf2, 0x23, 0xc6, 0x89, 0x52, 0xe1, 0xf2, 0xb6]),
    ("disable", [0xb9, 0xad, 0xbb, 0x5a, 0xd8, 0x0f, 0xee, 0xe9]),
    (
        "extend_account",
        [0xea, 0x66, 0xc2, 0xcb, 0x96, 0x48, 0x3e, 0xe5],
    ),
    (
        "init_boost",
        [0x8c, 0xe9, 0x21, 0x5e, 0x84, 0x5a, 0xc2, 0x8f],
    ),
    (
        "init_user_volume_accumulator",
        [0x5e, 0x06, 0xca, 0x73, 0xff, 0x60, 0xe8, 0xb7],
    ),
    (
        "migrate_pool_coin_creator",
        [0xd0, 0x08, 0x9f, 0x04, 0x4a, 0xaf, 0x10, 0x3a],
    ),
    (
        "set_boost_authority",
        [0xe3, 0x95, 0x4c, 0x2a, 0x82, 0x27, 0xea, 0xcd],
    ),
    (
        "set_coin_creator",
        [0xd2, 0x95, 0x80, 0x2d, 0xbc, 0x3a, 0x4e, 0xaf],
    ),
    (
        "set_reserved_fee_recipients",
        [0x6f, 0xac, 0xa2, 0xe8, 0x72, 0x59, 0xd5, 0x8e],
    ),
    (
        "sync_user_volume_accumulator",
        [0x56, 0x1f, 0xc0, 0x57, 0xa3, 0x57, 0x4f, 0xee],
    ),
    (
        "toggle_boost",
        [0x75, 0xa1, 0xa0, 0x4a, 0xdf, 0x89, 0x76, 0x63],
    ),
    (
        "toggle_cashback_enabled",
        [0x73, 0x67, 0xe0, 0xff, 0xbd, 0x59, 0x56, 0xc3],
    ),
    (
        "toggle_mayhem_mode",
        [0x01, 0x09, 0x6f, 0xd0, 0x64, 0x1f, 0xff, 0xa3],
    ),
    (
        "transfer_creator_fees_to_pump",
        [0x8b, 0x34, 0x86, 0x55, 0xe4, 0xe5, 0x6c, 0xf1],
    ),
    (
        "transfer_creator_fees_to_pump_v2",
        [0x01, 0x21, 0x4e, 0xb9, 0x21, 0x43, 0x2c, 0x5c],
    ),
    (
        "update_admin",
        [0xa1, 0xb0, 0x28, 0xd5, 0x3c, 0xb8, 0xb3, 0xe4],
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
        "update_fee_config",
        [0x68, 0xb8, 0x67, 0xf2, 0x58, 0x97, 0x6b, 0x14],
    ),
    ("withdraw", [0xb7, 0x12, 0x46, 0x9c, 0x94, 0x6d, 0xa1, 0x22]),
    (AMM_EVENT_CPI_NAME, EVENT_CPI_DISCRIMINATOR),
];
/// Name used for the event-CPI tag in diagnostics.
pub const AMM_EVENT_CPI_NAME: &str = "<anchor-event-cpi>";

/// The three IDL trade instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PumpAmmTradeVariant {
    Buy,
    BuyExactQuoteIn,
    Sell,
}

/// IDL-derived layout of one trade variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpAmmTradeSpec {
    pub variant: PumpAmmTradeVariant,
    pub name: &'static str,
    pub discriminator: [u8; 8],
    pub side: TradeSide,
    /// IDL account count; live transactions may carry more.
    pub min_accounts: usize,
    /// IDL instruction data length (8 + all args incl. `track_volume`).
    pub data_len: usize,
    pub arg_names: [&'static str; 2],
    pub has_track_volume: bool,
    pub verification: VariantVerification,
}

/// Account positions shared by all three variants (IDL names).
pub const POOL_IDX: usize = 0;
pub const USER_IDX: usize = 1;
pub const BASE_MINT_IDX: usize = 3;
pub const QUOTE_MINT_IDX: usize = 4;
pub const USER_BASE_TOKEN_ACCOUNT_IDX: usize = 5;
pub const USER_QUOTE_TOKEN_ACCOUNT_IDX: usize = 6;

const SPEC_BUY: PumpAmmTradeSpec = PumpAmmTradeSpec {
    variant: PumpAmmTradeVariant::Buy,
    name: "buy",
    discriminator: AMM_BUY_DISCRIMINATOR,
    side: TradeSide::Buy,
    min_accounts: 23,
    data_len: 25,
    arg_names: ["base_amount_out", "max_quote_amount_in"],
    has_track_volume: true,
    verification: VariantVerification::FixtureVerified,
};
const SPEC_BUY_EXACT_QUOTE_IN: PumpAmmTradeSpec = PumpAmmTradeSpec {
    variant: PumpAmmTradeVariant::BuyExactQuoteIn,
    name: "buy_exact_quote_in",
    discriminator: AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR,
    side: TradeSide::Buy,
    min_accounts: 23,
    data_len: 25,
    arg_names: ["spendable_quote_in", "min_base_amount_out"],
    has_track_volume: true,
    verification: VariantVerification::FixtureVerified,
};
const SPEC_SELL: PumpAmmTradeSpec = PumpAmmTradeSpec {
    variant: PumpAmmTradeVariant::Sell,
    name: "sell",
    discriminator: AMM_SELL_DISCRIMINATOR,
    side: TradeSide::Sell,
    min_accounts: 21,
    data_len: 24,
    arg_names: ["base_amount_in", "min_quote_amount_out"],
    has_track_volume: false,
    verification: VariantVerification::FixtureVerified,
};

impl PumpAmmTradeVariant {
    pub const ALL: [Self; 3] = [Self::Buy, Self::BuyExactQuoteIn, Self::Sell];
    pub const COUNT: usize = 3;

    #[must_use]
    pub const fn spec(self) -> &'static PumpAmmTradeSpec {
        match self {
            Self::Buy => &SPEC_BUY,
            Self::BuyExactQuoteIn => &SPEC_BUY_EXACT_QUOTE_IN,
            Self::Sell => &SPEC_SELL,
        }
    }

    /// Stable index into per-variant counter arrays (`0..COUNT`).
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Buy => 0,
            Self::BuyExactQuoteIn => 1,
            Self::Sell => 2,
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

    /// Evidence level. All three are `FixtureVerified`: every successful
    /// attributable fixture tx has an exact base leg and the quote leg
    /// reconciles with the event (residuals explained, see the
    /// reconciliation module and the fixture tests).
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

/// A decoded PumpSwap trade instruction.
///
/// `args` are the declared sizes/limits (not executed amounts). `side` is
/// relative to the pool's BASE mint: for pools whose base mint is wSOL a
/// `sell` spends wSOL for the quote token (economically a buy of the quote
/// token). Consumers must look at `base_mint`/`quote_mint`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedPumpAmmTrade {
    pub variant: PumpAmmTradeVariant,
    pub side: TradeSide,
    pub user: SolanaPubkey,
    pub pool: SolanaPubkey,
    pub base_mint: SolanaPubkey,
    pub quote_mint: SolanaPubkey,
    pub user_base_token_account: SolanaPubkey,
    pub user_quote_token_account: SolanaPubkey,
    pub args: [NamedU64; 2],
    /// Raw `OptionBool` byte: `Some` only for a 25-byte buy.
    pub track_volume: Option<u8>,
    pub slot: u64,
    pub transaction_index: u64,
    pub instruction_index: u32,
}

impl DecodedPumpAmmTrade {
    #[must_use]
    pub fn verification(&self) -> VariantVerification {
        self.variant.verification()
    }
}

/// Classification of one instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpAmmInstructionOutcome {
    /// Program id is not the scoped program.
    NotMine,
    Trade(Box<DecodedPumpAmmTrade>),
    /// Known non-trade (IDL instruction or event-CPI tag).
    NonTrade(&'static str),
    /// Trade discriminator with broken structure, or < 8 data bytes.
    Malformed {
        variant: Option<PumpAmmTradeVariant>,
        reason: String,
    },
    /// 8-byte discriminator in neither table. COVERAGE GAP.
    UnknownDiscriminator {
        discriminator: [u8; 8],
    },
}

/// Payload of the `TxDecoder` impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpAmmInstruction {
    Trade(Box<DecodedPumpAmmTrade>),
    NonTrade(&'static str),
}

/// PumpSwap AMM decoder scoped to one `DeploymentScope`.
#[derive(Debug, Clone)]
pub struct PumpAmmDecoder {
    scope: DeploymentScope,
}

/// Solana mainnet scope of the PumpSwap AMM program. `active_from = 0`
/// means "no lower bound claimed", not a verified activation slot.
#[must_use]
pub fn pump_amm_mainnet_scope() -> DeploymentScope {
    DeploymentScope {
        chain: ChainKey {
            family: ChainFamily::Solana,
            network_id: NetworkId::SolanaCluster(SolanaCluster::Mainnet),
            genesis_identity: GenesisIdentity::Unverified,
        },
        contract_addresses: vec![AddressBytes::Solana(PUMP_AMM_PROGRAM_ID_BYTES)],
        active_from: 0,
        active_until: None,
    }
}

impl PumpAmmDecoder {
    #[must_use]
    pub fn new(scope: DeploymentScope) -> Self {
        Self { scope }
    }

    /// Decoder for [`pump_amm_mainnet_scope`].
    #[must_use]
    pub fn mainnet() -> Self {
        Self::new(pump_amm_mainnet_scope())
    }

    /// Program-id gate. An empty `contract_addresses` matches nothing.
    #[must_use]
    pub fn is_program(&self, instruction: &RawSolanaInstruction) -> bool {
        self.scope
            .contract_addresses
            .iter()
            .any(|addr| matches!(addr, AddressBytes::Solana(p) if *p == instruction.program_id))
    }

    /// Classify one instruction (program-id gate first).
    #[must_use]
    pub fn classify(
        &self,
        instruction: &RawSolanaInstruction,
        slot: u64,
        transaction_index: u64,
    ) -> PumpAmmInstructionOutcome {
        if !self.is_program(instruction) {
            return PumpAmmInstructionOutcome::NotMine;
        }
        classify_pump_amm_instruction(instruction, slot, transaction_index)
    }
}

impl TxDecoder<RawSolanaInstruction, PumpAmmInstruction> for PumpAmmDecoder {
    fn scope(&self) -> &DeploymentScope {
        &self.scope
    }

    fn decode(&self, instruction: &RawSolanaInstruction) -> DecodeOutcome<PumpAmmInstruction> {
        match self.classify(instruction, 0, 0) {
            PumpAmmInstructionOutcome::NotMine => DecodeOutcome::NotMine,
            PumpAmmInstructionOutcome::Trade(t) => {
                DecodeOutcome::Decoded(PumpAmmInstruction::Trade(t))
            }
            PumpAmmInstructionOutcome::NonTrade(n) => {
                DecodeOutcome::Decoded(PumpAmmInstruction::NonTrade(n))
            }
            PumpAmmInstructionOutcome::Malformed { reason, .. } => DecodeOutcome::Malformed(reason),
            PumpAmmInstructionOutcome::UnknownDiscriminator { discriminator } => {
                DecodeOutcome::Malformed(format!(
                    "unknown PumpSwap AMM instruction discriminator {} (not in IDL {PUMP_AMM_IDL_COMMIT})",
                    hex8(&discriminator)
                ))
            }
        }
    }
}

/// Classify an instruction ASSUMED to belong to the AMM program (no
/// program-id check; use [`PumpAmmDecoder::classify`]).
#[must_use]
pub fn classify_pump_amm_instruction(
    instruction: &RawSolanaInstruction,
    slot: u64,
    transaction_index: u64,
) -> PumpAmmInstructionOutcome {
    let Some(disc_slice) = instruction.data.get(0..8) else {
        return PumpAmmInstructionOutcome::Malformed {
            variant: None,
            reason: format!(
                "instruction of the PumpSwap AMM program has {} data bytes, fewer than the 8-byte discriminator",
                instruction.data.len()
            ),
        };
    };
    if let Some(variant) = PumpAmmTradeVariant::from_discriminator(disc_slice) {
        return match decode_trade(variant, instruction, slot, transaction_index) {
            Ok(trade) => PumpAmmInstructionOutcome::Trade(Box::new(trade)),
            Err(reason) => PumpAmmInstructionOutcome::Malformed {
                variant: Some(variant),
                reason,
            },
        };
    }
    if let Some((name, _)) = AMM_NON_TRADE_INSTRUCTIONS
        .iter()
        .find(|(_, d)| d.as_slice() == disc_slice)
    {
        return PumpAmmInstructionOutcome::NonTrade(name);
    }
    let Ok(discriminator) = <[u8; 8]>::try_from(disc_slice) else {
        return PumpAmmInstructionOutcome::Malformed {
            variant: None,
            reason: "discriminator slice is not 8 bytes".to_string(),
        };
    };
    PumpAmmInstructionOutcome::UnknownDiscriminator { discriminator }
}

fn decode_trade(
    variant: PumpAmmTradeVariant,
    instruction: &RawSolanaInstruction,
    slot: u64,
    transaction_index: u64,
) -> Result<DecodedPumpAmmTrade, String> {
    let spec = variant.spec();
    let name = spec.name;
    let len = instruction.data.len();
    let length_ok =
        len == TRADE_DATA_LEN_REQUIRED || (spec.has_track_volume && len == spec.data_len);
    if !length_ok {
        return Err(format!(
            "instruction matches {name} discriminator but data has {len} bytes; accepted: {TRADE_DATA_LEN_REQUIRED}{} (IDL commit {PUMP_AMM_IDL_COMMIT})",
            if spec.has_track_volume {
                format!(" or {}", spec.data_len)
            } else {
                String::new()
            }
        ));
    }
    if instruction.accounts.len() < spec.min_accounts {
        return Err(format!(
            "instruction matches {name} discriminator but has {} accounts, expected at least {} per the official IDL",
            instruction.accounts.len(),
            spec.min_accounts
        ));
    }
    let [arg0_name, arg1_name] = spec.arg_names;
    let arg0 = read_u64_le(&instruction.data, 8)
        .ok_or_else(|| format!("{name}: {arg0_name} is unreadable"))?;
    let arg1 = read_u64_le(&instruction.data, 16)
        .ok_or_else(|| format!("{name}: {arg1_name} is unreadable"))?;
    let track_volume = if len == spec.data_len && spec.has_track_volume {
        Some(
            *instruction
                .data
                .get(TRADE_DATA_LEN_REQUIRED)
                .ok_or_else(|| format!("{name}: track_volume byte is unreadable"))?,
        )
    } else {
        None
    };
    let account = |idx: usize, what: &str| -> Result<SolanaPubkey, String> {
        instruction
            .accounts
            .get(idx)
            .copied()
            .ok_or_else(|| format!("{name}: {what} account at position {idx} is missing"))
    };
    Ok(DecodedPumpAmmTrade {
        variant,
        side: spec.side,
        user: account(USER_IDX, "user")?,
        pool: account(POOL_IDX, "pool")?,
        base_mint: account(BASE_MINT_IDX, "base_mint")?,
        quote_mint: account(QUOTE_MINT_IDX, "quote_mint")?,
        user_base_token_account: account(USER_BASE_TOKEN_ACCOUNT_IDX, "user_base_token_account")?,
        user_quote_token_account: account(
            USER_QUOTE_TOKEN_ACCOUNT_IDX,
            "user_quote_token_account",
        )?,
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
