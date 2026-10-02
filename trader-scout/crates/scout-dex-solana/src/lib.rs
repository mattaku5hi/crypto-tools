//! scout-dex-solana: Solana DEX protocol decoders. See workspace
//! docs/ARCHITECTURE.md §5 ("Матрица DEX").
//!
//! `bonding_curve_buy` decodes pump.fun's `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`
//! all six trade instructions (`buy`, `buy_exact_sol_in`, `sell`, `buy_v2`,
//! `buy_exact_quote_in_v2`, `sell_v2`) and an explicit non-trade table -- a **confirmed** deployment per
//! `docs/p0/deployment-registry.md`'s "2026-10-01 confirmation" (on-chain
//! `executable` state + official IDL from `pump-fun/pump-public-docs`,
//! commit `e0687ae9b7e064a0f54efc7297c65eecfbba3a8f`, cross-validated
//! against live balance-delta data). See that module's own doc comment
//! for the full confirmation trail and argument/account layout.
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod bonding_curve_buy;
mod trade_event;

pub use bonding_curve_buy::{
    BUY_EXACT_QUOTE_IN_V2_INSTRUCTION_DISCRIMINATOR, BUY_EXACT_SOL_IN_INSTRUCTION_DISCRIMINATOR,
    BUY_INSTRUCTION_DISCRIMINATOR, BUY_V2_INSTRUCTION_DISCRIMINATOR, BondingCurveBuyDecoder,
    DecodedBondingCurveTrade, EVENT_CPI_DISCRIMINATOR, EVENT_CPI_NAME, NON_TRADE_INSTRUCTIONS,
    NamedU64, PUMP_IDL_COMMIT, PUMP_IDL_SHA256, PumpInstruction, PumpInstructionOutcome,
    PumpTradeSpec, PumpTradeVariant, SELL_INSTRUCTION_DISCRIMINATOR,
    SELL_V2_INSTRUCTION_DISCRIMINATOR, TradeSide, VariantVerification, classify_pump_instruction,
    hex8,
};
pub use trade_event::{
    EVENT_CPI_HEADER_LEN, EVENT_DISCRIMINATORS, MAX_IX_NAME_BYTES, MAX_SHAREHOLDERS,
    MAX_TRAILING_EVENT_BYTES, PairMismatch, PairedTrade, PumpEventOutcome, Shareholder,
    TRADE_EVENT_DISCRIMINATOR, TRADE_EVENT_REQUIRED_LEN, TradeEvent, TradeEventPairing,
    TradeEventPairingReport, classify_pump_event, event_name, pair_trades_with_events,
};
