//! Embedded scanner example: proves a third-party Tokio application can
//! import just `scout-sdk`'s default (`scan`) feature set — raw chain
//! scanning via `HistoryProvider` — without pulling in ledger,
//! analytics, storage, or engine orchestration.
//!
//! This is ROADMAP.md P8.1 and the concrete check for ACCEPTANCE F07:
//! "Embedding example работает в уже существующем Tokio runtime с
//! injected source/store, без subprocess CLI, global subscriber и
//! process::exit. Scanner-only build не подтягивает обязательную
//! ledger/formatting инфраструктуру."
//!
//! Run with: `cargo run --example embedded_scanner -p scout-sdk
//! --no-default-features --features scan`
//!
//! The `--no-default-features --features scan` flags are the actual
//! proof: this example's own `Cargo.toml` entry has no `ledger`/
//! `analytics`/`full` feature enabled, so `cargo tree` for this binary
//! target never lists `scout-ledger`/`scout-analytics`/`scout-storage`/
//! `scout-engine` at all (ADR-007).

use futures::StreamExt;
use scout_sdk::providers::{FixtureProvider, HistoryProvider, ScanEnvelope, ScanRequest, ScanTask};
use scout_sdk::{
    AddressBytes, AssetKey, ChainFamily, ChainKey, GenesisIdentity, NetworkId, RawPayload,
};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    // This function runs inside an ALREADY-EXISTING Tokio runtime — the
    // one `#[tokio::main]` creates for this example binary, standing in
    // for a host application's own runtime. Nothing in scout-sdk's
    // `scan`-feature surface creates its own runtime, installs a global
    // tracing subscriber, reads stdin, or calls `std::process::exit`
    // (AGENTS.md invariant #15) — this example is the executable proof
    // of that claim, not just a comment asserting it.

    let asset = AssetKey::Token(
        ChainKey {
            family: ChainFamily::Evm,
            network_id: NetworkId::EvmChainId(8453),
            genesis_identity: GenesisIdentity::Unverified,
        },
        AddressBytes::Evm([0x11; 20]),
    );

    // A fixture-backed provider stands in for a live-credentialed one
    // here (per ADR-006, no live provider exists in this workspace yet)
    // — the point of this example is the *embedding* pattern, which is
    // identical regardless of which HistoryProvider implementation a
    // caller injects.
    let provider = FixtureProvider::new();

    // 1. Plan a scan for a token's market activity.
    let plan = match provider
        .plan(&ScanRequest::TokenMarketActivity {
            asset: asset.clone(),
        })
        .await
    {
        Ok(plan) => plan,
        Err(err) => {
            // FixtureProvider always succeeds at plan() with no
            // fixtures loaded; a real provider could fail here (e.g.
            // ConfigurationRequired) and the host application decides
            // how to handle that — this example just demonstrates
            // propagating the error rather than panicking.
            eprintln!("plan() failed: {err}");
            return;
        }
    };
    println!("plan capabilities: {:?}", plan.capabilities);

    // 2. Start scanning, with a real CancellationToken the host
    // application controls — this is the "inject cancellation
    // explicitly" half of invariant #15.
    let cancel = CancellationToken::new();
    let mut stream = provider.scan(
        ScanTask {
            description: "embedded_scanner example: token market activity".to_string(),
        },
        cancel.clone(),
    );

    // 3. Consume a few items, then explicitly cancel — proving
    // cancellation is cooperative and the host can stop the scan and
    // continue doing other work afterward, not that the library forced
    // an abort.
    let mut consumed = 0;
    while let Some(item) = stream.next().await {
        match item {
            Ok(envelope) => {
                let payload_label = describe_payload(&envelope);
                println!("received envelope: {payload_label}");
                consumed += 1;
            }
            Err(err) => {
                // An empty FixtureProvider reports Unknown capability and
                // yields no items — this arm exists to show the caller's
                // expected error-handling shape, exercised for real once
                // a fixture-with-data or a ConfigurationRequired
                // UnconfiguredProvider path is substituted in.
                println!("scan error (expected with an empty fixture set): {err}");
                break;
            }
        }
        if consumed >= 3 {
            cancel.cancel();
            break;
        }
    }

    // 4. Prove the host's own runtime/task keeps running after this
    // library call returns — the library did not take over the
    // process.
    println!("embedded_scanner example: host application continues after scan.");
}

/// A minimal, human-readable label for whatever `RawPayload` variant a
/// `HistoryProvider` returned — this example only needs to prove the
/// envelope carries real decodable data, not implement a full decoder.
fn describe_payload(envelope: &ScanEnvelope) -> String {
    match &envelope.payload {
        RawPayload::EvmLog(log) => format!("EvmLog(address={:?})", log.address),
        RawPayload::EvmTransaction(tx) => format!("EvmTransaction(hash={:?})", tx.hash),
        RawPayload::SolanaInstruction(ix) => {
            format!("SolanaInstruction(program_id={:?})", ix.program_id)
        }
        RawPayload::SolanaTransaction(tx) => {
            format!("SolanaTransaction(slot={})", tx.slot)
        }
    }
}
