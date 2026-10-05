//! scout-rpc: thin HTTP/JSON-RPC transport layer for scout-providers.
//!
//! Scope (see `docs/ARCHITECTURE.md`): this crate knows how to make an
//! HTTP call, retry it sensibly, and parse a JSON-RPC envelope
//! honestly — it does not know about Solana/EVM method semantics
//! (that's `scout-solana`/`scout-evm`), does not know about specific
//! vendors like Helius/Blockscout (that's `scout-providers`), and does
//! not decode any protocol payload (that's `scout-dex-*`).
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

mod backoff;
mod client;
mod endpoint;
mod jsonrpc;
mod ratelimit;

pub use backoff::{
    CounterJitter, JitterSource, NoJitter, RetryPolicy, SleepFuture, Sleeper, TokioSleeper,
};
pub use client::{
    DEFAULT_MAX_RESPONSE_BYTES, DEFAULT_MAX_RETRY_AFTER, RATE_LIMIT_EXTRA_RETRIES,
    RATE_LIMIT_MAX_BACKOFF, RequestBudgetExhausted, ResponseTooLarge, RpcClient,
};
pub use endpoint::RpcEndpoint;
pub use jsonrpc::{JsonRpcEnvelopeError, JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use ratelimit::{
    DEFAULT_MAX_WAITERS, HalvingHook, MIN_RATE_MILLI, RateLimiter, RateLimiterSaturated,
    RateLimiterStats,
};
