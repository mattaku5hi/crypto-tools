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
mod dflow_event;
mod dlmm_event;
mod jupiter_event;
mod okx_event;
mod okx_schema;
mod pump_accounts;
mod pump_amm;
mod pump_amm_event;
mod pump_amm_reconcile;
mod raydium_clmm_event;
mod raydium_cpmm_event;
mod swap_math;
mod trade_event;
mod venue_legs;
mod venue_log;
mod venue_wire;
mod whirlpool_event;

pub use bonding_curve_buy::{
    BUY_EXACT_QUOTE_IN_V2_INSTRUCTION_DISCRIMINATOR, BUY_EXACT_SOL_IN_INSTRUCTION_DISCRIMINATOR,
    BUY_INSTRUCTION_DISCRIMINATOR, BUY_V2_INSTRUCTION_DISCRIMINATOR, BondingCurveBuyDecoder,
    DecodedBondingCurveTrade, EVENT_CPI_DISCRIMINATOR, EVENT_CPI_NAME, NON_TRADE_INSTRUCTIONS,
    NamedU64, PUMP_IDL_COMMIT, PUMP_IDL_SHA256, PumpInstruction, PumpInstructionOutcome,
    PumpTradeSpec, PumpTradeVariant, SELL_INSTRUCTION_DISCRIMINATOR,
    SELL_V2_INSTRUCTION_DISCRIMINATOR, TradeSide, VariantVerification, classify_pump_instruction,
    hex8,
};
pub use dflow_event::{
    DFLOW_EVENT_AUTHORITY_BYTES, DFLOW_EVENT_HEADER_LEN, DFLOW_FEE_EVENT_DISCRIMINATOR,
    DFLOW_FEE_EVENT_LEN, DFLOW_SCHEMA_COMMIT, DFLOW_SCHEMA_SHA256, DFLOW_SWAP_EVENT_DISCRIMINATOR,
    DFLOW_SWAP_EVENT_LEN, DFLOW_V4_PROGRAM_ID, DFLOW_V4_PROGRAM_ID_BYTES, DflowEventDecoder,
    DflowEventOutcome, DflowFeeEvent, DflowSwapLeg, classify_dflow_event,
    dflow_swap_event_verification,
};
pub use dlmm_event::{
    DLMM_EVENT_AUTHORITY_BYTES, DLMM_EVENT_HEADER_LEN, DLMM_EVENTS, DLMM_IDL_SHA256,
    DLMM_PROGRAM_ID, DLMM_PROGRAM_ID_BYTES, DLMM_SWAP_EVENT_DISCRIMINATOR, DLMM_SWAP_EVENT_LEN,
    DLMM_SWAP2_EVENT_DISCRIMINATOR, DLMM_SWAP2_EVENT_LEN, DlmmEventOutcome, DlmmSwap2Event,
    DlmmSwapEvent, DlmmSwapIx, DlmmSwapVariant, classify_dlmm_event, parse_dlmm_swap_instruction,
};
pub use dlmm_event::{
    DLMM_SWAP_DISCRIMINATOR, DLMM_SWAP_EXACT_OUT_DISCRIMINATOR, DLMM_SWAP_EXACT_OUT2_DISCRIMINATOR,
    DLMM_SWAP_WITH_PRICE_IMPACT_DISCRIMINATOR, DLMM_SWAP_WITH_PRICE_IMPACT2_DISCRIMINATOR,
    DLMM_SWAP2_DISCRIMINATOR,
};
pub use jupiter_event::{
    JUPITER_BEST_SWAP_OUT_AMOUNT_VIOLATION_DISCRIMINATOR,
    JUPITER_CANDIDATE_SWAP_QUOTE_ERROR_DISCRIMINATOR, JUPITER_CANDIDATE_SWAP_RESULTS_DISCRIMINATOR,
    JUPITER_EVENT_AUTHORITY_BYTES, JUPITER_EVENT_HEADER_LEN, JUPITER_FEE_EVENT_DISCRIMINATOR,
    JUPITER_FEE_EVENT_LEN, JUPITER_IDL_COMMIT, JUPITER_IDL_SHA256, JUPITER_ONCHAIN_IDL_SHA256,
    JUPITER_SWAP_EVENT_DISCRIMINATOR, JUPITER_SWAP_ITEM_LEN, JUPITER_SWAPS_EVENT_DISCRIMINATOR,
    JUPITER_V6_PROGRAM_ID, JUPITER_V6_PROGRAM_ID_BYTES, JupiterEventDecoder, JupiterEventKind,
    JupiterEventOutcome, JupiterFeeEvent, JupiterSwapLeg, MAX_SWAPS_PER_EVENT,
    classify_jupiter_event,
};
pub use okx_event::{
    OKX_DEX_ROUTER_PROGRAM_ID, OKX_DEX_ROUTER_PROGRAM_ID_BYTES, OKX_EVENT_AUTHORITY_BYTES,
    OKX_EVENT_HEADER_LEN, OKX_IDL_SHA256, OKX_ORDER_EVENT_COMMON_LEN, OKX_SWAP_EVENT_DISCRIMINATOR,
    OkxEventDecoder, OkxEventOutcome, OkxHop, OkxOrderEvent, OkxOrderEventKind, classify_okx_event,
};
pub use pump_accounts::{
    AccountDecodeError, BONDING_CURVE_ACCOUNT_DISCRIMINATOR, BONDING_CURVE_REQUIRED_LEN,
    BondingCurveAccount, POOL_ACCOUNT_DISCRIMINATOR, POOL_REQUIRED_LEN, PUMP_PROGRAM_ID_BYTES,
    PoolAccount, SPL_TOKEN_2022_PROGRAM_ID_BYTES, SPL_TOKEN_PROGRAM_ID_BYTES,
    TOKEN_ACCOUNT_BASE_LEN, TokenAccountBalance, decode_bonding_curve, decode_pool,
    decode_token_account,
};
pub use pump_amm::{
    AMM_BUY_DISCRIMINATOR, AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR, AMM_EVENT_CPI_NAME,
    AMM_NON_TRADE_INSTRUCTIONS, AMM_SELL_DISCRIMINATOR, BASE_MINT_IDX, DecodedPumpAmmTrade,
    POOL_IDX, PUMP_AMM_IDL_COMMIT, PUMP_AMM_IDL_SHA256, PUMP_AMM_PROGRAM_ID,
    PUMP_AMM_PROGRAM_ID_BYTES, PumpAmmDecoder, PumpAmmInstruction, PumpAmmInstructionOutcome,
    PumpAmmTradeSpec, PumpAmmTradeVariant, QUOTE_MINT_IDX, TRADE_DATA_LEN_REQUIRED,
    TRADE_DATA_LEN_TWO_BYTE_OPTION, TrackVolumeEncoding, USER_BASE_TOKEN_ACCOUNT_IDX, USER_IDX,
    USER_QUOTE_TOKEN_ACCOUNT_IDX, WRAPPED_SOL_MINT, classify_pump_amm_instruction,
    pump_amm_mainnet_scope,
};
pub use pump_amm_event::{
    AMM_BUY_EVENT_DISCRIMINATOR, AMM_EVENT_CPI_HEADER_LEN, AMM_EVENT_REQUIRED_LEN,
    AMM_OTHER_EVENTS, AMM_SELL_EVENT_DISCRIMINATOR, AmmPairMismatch, AmmPairingReport,
    AmmTradeEventPairing, BuyEvent, MAX_AMM_IX_NAME_BYTES, MAX_AMM_TRAILING_EVENT_BYTES,
    PairedAmmTrade, PumpAmmEvent, PumpAmmEventOutcome, SellEvent, classify_pump_amm_event,
    pair_amm_trades_with_events,
};
pub use pump_amm_reconcile::{
    AmmAttribution, AmmMintLeg, AmmTxReconciliation, AmmUserReconciliation,
    reconcile_pump_amm_transaction,
};
pub use raydium_clmm_event::{
    ClmmEventLayout, RAYDIUM_CLMM_EVENTS, RAYDIUM_CLMM_IDL_SHA256, RAYDIUM_CLMM_PROGRAM_ID,
    RAYDIUM_CLMM_PROGRAM_ID_BYTES, RAYDIUM_CLMM_SWAP_EVENT_DISCRIMINATOR,
    RAYDIUM_CLMM_SWAP_EVENT_IDL_LEN, RAYDIUM_CLMM_SWAP_EVENT_LIVE_LEN, RAYDIUM_IDL_COMMIT,
    RaydiumClmmSwapEvent, RaydiumClmmSwapIx, RaydiumClmmSwapVariant, classify_raydium_clmm_event,
    parse_raydium_clmm_swap_instruction,
};
pub use raydium_clmm_event::{
    RAYDIUM_CLMM_SWAP_DISCRIMINATOR, RAYDIUM_CLMM_SWAP_ROUTER_BASE_IN_DISCRIMINATOR,
    RAYDIUM_CLMM_SWAP_V2_DISCRIMINATOR,
};
pub use raydium_cpmm_event::{
    RAYDIUM_CPMM_EVENTS, RAYDIUM_CPMM_IDL_SHA256, RAYDIUM_CPMM_PROGRAM_ID,
    RAYDIUM_CPMM_PROGRAM_ID_BYTES, RAYDIUM_CPMM_SWAP_EVENT_DISCRIMINATOR,
    RAYDIUM_CPMM_SWAP_EVENT_LEN, RaydiumCpmmSwapEvent, RaydiumCpmmSwapIx, RaydiumCpmmSwapVariant,
    classify_raydium_cpmm_event, parse_raydium_cpmm_swap_instruction,
};
pub use raydium_cpmm_event::{
    RAYDIUM_CPMM_SWAP_BASE_INPUT_DISCRIMINATOR, RAYDIUM_CPMM_SWAP_BASE_OUTPUT_DISCRIMINATOR,
};
pub use swap_math::{
    AmmBuyQuote, AmmSellQuote, CurveSellQuote, amm_buy_quote, amm_sell_quote, curve_buy_tokens_out,
    curve_sell_quote, effective_quote_reserve, fee_ceil, price_impact_bps,
};
pub use trade_event::{
    EVENT_CPI_HEADER_LEN, EVENT_DISCRIMINATORS, MAX_IX_NAME_BYTES, MAX_SHAREHOLDERS,
    MAX_TRAILING_EVENT_BYTES, PairMismatch, PairedTrade, PumpEventOutcome, Shareholder,
    TRADE_EVENT_DISCRIMINATOR, TRADE_EVENT_REQUIRED_LEN, TradeEvent, TradeEventPairing,
    TradeEventPairingReport, classify_pump_event, event_name, pair_trades_with_events,
};
pub use venue_legs::{
    MintSource, VenueIssue, VenueIssueKind, VenueKind, VenueLeg, VenueScan, scan_venue_events,
};
pub use venue_log::{LogAttribution, ProgramDataEvent, attribute_program_data};
pub use venue_wire::{SwapInstructionParse, VenueEventOutcome};
pub use whirlpool_event::{
    WHIRLPOOL_EVENTS, WHIRLPOOL_IDL_SHA256, WHIRLPOOL_PROGRAM_ID, WHIRLPOOL_PROGRAM_ID_BYTES,
    WHIRLPOOL_TRADED_DISCRIMINATOR, WHIRLPOOL_TRADED_LEN, WhirlpoolSwapIx, WhirlpoolSwapVariant,
    WhirlpoolTraded, classify_whirlpool_event, parse_whirlpool_swap_instruction,
    whirlpool_traded_verification,
};
pub use whirlpool_event::{
    WHIRLPOOL_SWAP_DISCRIMINATOR, WHIRLPOOL_SWAP_V2_DISCRIMINATOR,
    WHIRLPOOL_TWO_HOP_SWAP_DISCRIMINATOR, WHIRLPOOL_TWO_HOP_SWAP_V2_DISCRIMINATOR,
};
