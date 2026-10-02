//! Offline tests for the Solana buyer-intersect engine path: real
//! committed Helius-shaped fixtures (served through the real
//! `HeliusProvider` via wiremock) plus a scripted stub provider for
//! intersection / threshold / truncation / failure cases.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use futures::StreamExt as _;
use futures::stream::{self, BoxStream};
use scout_api::{
    HistoryProvider, ProviderError, ScanEnvelope, ScanPlan, ScanRequest, ScanTask,
    SourceCapabilities,
};
use scout_core::{
    AddressBytes, AssetKey, RawPayload, RawSolanaInstruction, RawSolanaTransaction,
    SolanaExecutionStatus, SolanaPubkey, SolanaTokenBalanceChange, WalletKey,
};
use scout_dex_solana::{
    BUY_EXACT_QUOTE_IN_V2_INSTRUCTION_DISCRIMINATOR, BUY_EXACT_SOL_IN_INSTRUCTION_DISCRIMINATOR,
    BUY_INSTRUCTION_DISCRIMINATOR, BUY_V2_INSTRUCTION_DISCRIMINATOR, EVENT_CPI_DISCRIMINATOR,
    PumpInstructionOutcome, PumpTradeVariant, SELL_INSTRUCTION_DISCRIMINATOR, TradeSide,
    VariantVerification,
};
use scout_engine::{
    PUMP_BONDING_CURVE_PROGRAM_ID, ScanFailureKind, ScanStop, TokenScanStatus,
    classify_provider_error, pump_bonding_curve_decoder, qualify_bonding_curve_buys,
    qualify_bonding_curve_buys_with_policy, run_solana_buyer_intersect,
    run_solana_buyer_intersect_with_policy, sanitize_provider_text, solana_mainnet_chain,
};
use scout_providers::HeliusProvider;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROBE_MINT: &str = "AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump";
const PROBE_BUYER: &str = "HgwBZM6kQE8qpYBdM2aXDxaEs5GTpDryNxuREeVP8f8B";
const MINT1: &str = "NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump";
const MINT2: &str = "GGf4EX9qbzxuboefDTEvqHdysHqtZSQC7Sahprjpump";

fn pubkey(s: &str) -> SolanaPubkey {
    bs58::decode(s).into_vec().unwrap().try_into().unwrap()
}

fn asset_b58(s: &str) -> AssetKey {
    AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(pubkey(s)))
}

fn fixture_json(name: &str) -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(name);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    // The committed captures carry a continuation token; drop it so the
    // offline run is a complete, non-truncated page regardless of the
    // provider's pagination policy.
    value["result"]["paginationToken"] = serde_json::Value::Null;
    value
}

async fn mount_fixture(server: &MockServer, mint_in_body: &str, fixture: &str) {
    Mock::given(method("POST"))
        .and(body_string_contains(mint_in_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture_json(fixture)))
        .mount(server)
        .await;
}

fn helius(server: &MockServer) -> HeliusProvider {
    HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1).unwrap()
}

async fn collect_transactions(provider: &HeliusProvider, mint: &str) -> Vec<RawSolanaTransaction> {
    let mut stream = provider.scan(
        ScanTask {
            request: ScanRequest::TokenMarketActivity {
                asset: asset_b58(mint),
            },
            description: "test".to_string(),
        },
        CancellationToken::new(),
    );
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        if let RawPayload::SolanaTransaction(tx) = item.unwrap().payload {
            out.push(tx);
        }
    }
    out
}

#[tokio::test]
async fn real_probe_fixture_yields_exactly_the_known_bonding_curve_buyer() {
    let server = MockServer::start().await;
    mount_fixture(&server, PROBE_MINT, "pump_bonding_curve_buy_probe.json").await;
    let provider = helius(&server);

    let decoder = pump_bonding_curve_decoder().unwrap();
    let txs = collect_transactions(&provider, PROBE_MINT).await;
    assert_eq!(txs.len(), 5);
    let all_buys: Vec<_> = txs
        .iter()
        .flat_map(|tx| qualify_bonding_curve_buys(tx, &decoder).buys)
        .collect();
    // The capture holds a second, real bonding-curve buy of a different
    // mint (another wallet); it must be recognized but must not leak
    // into the AB48 result.
    assert_eq!(all_buys.len(), 2);
    let buys: Vec<_> = all_buys
        .into_iter()
        .filter(|b| b.asset == asset_b58(PROBE_MINT))
        .collect();
    assert_eq!(buys.len(), 1, "{buys:?}");
    assert_eq!(buys[0].asset, asset_b58(PROBE_MINT));
    assert_eq!(
        buys[0].wallet,
        WalletKey {
            chain: solana_mainnet_chain(),
            address: AddressBytes::Solana(pubkey(PROBE_BUYER)),
        }
    );
    assert_eq!(buys[0].slot, 452_380_124);
    assert_eq!(buys[0].decoded_amount, 2_979_651_581_366);

    // End to end through the engine.
    let report = run_solana_buyer_intersect(
        &provider,
        &[asset_b58(PROBE_MINT)],
        1,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.base.matches.len(), 1);
    assert_eq!(report.base.matches[0].hit_count, 1);
    assert_eq!(
        report.base.matches[0].matched_assets,
        vec![asset_b58(PROBE_MINT)]
    );
    assert_eq!(report.per_token[0].transactions_scanned, 5);
    assert_eq!(report.per_token[0].qualified_buyers, 1);
    assert!(report.diagnostics.decoded_buys >= 1);
    assert!(
        !report.is_coverage_incomplete(),
        "{:?}",
        report.incomplete_reasons()
    );
}

#[tokio::test]
async fn real_post_migration_fixtures_have_zero_bonding_curve_buys() {
    let server = MockServer::start().await;
    mount_fixture(&server, MINT1, "pump_mint1_full.json").await;
    mount_fixture(&server, MINT2, "pump_mint2_full.json").await;
    let provider = helius(&server);
    let decoder = pump_bonding_curve_decoder().unwrap();

    for mint in [MINT1, MINT2] {
        let txs = collect_transactions(&provider, mint).await;
        assert!(!txs.is_empty());
        for tx in &txs {
            let q = qualify_bonding_curve_buys(tx, &decoder);
            assert!(q.buys.is_empty());
            assert_eq!(q.diagnostics.decoded_buys, 0);
            assert_eq!(q.diagnostics.malformed_instructions, 0);
            assert_eq!(q.diagnostics.unknown_discriminator_instructions, 0);
            assert!(q.unverified_buys.is_empty());
        }
    }

    let report = run_solana_buyer_intersect(
        &provider,
        &[asset_b58(MINT1), asset_b58(MINT2)],
        1,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(report.base.matches.is_empty());
    assert_eq!(report.base.input_token_count, 2);
    assert_eq!(report.per_token.len(), 2);
    assert_eq!(report.diagnostics.decoded_buys, 0);
    assert_eq!(report.diagnostics.unknown_discriminator_instructions, 0);
    assert!(!report.is_coverage_incomplete());
    // Empty result is complete-within-scope, but the scope block states
    // the lower-bound caveat.
    assert!(report.scope.not_decoded.contains("PumpSwap"));
    // Positive deltas without decoded instructions are counted, not hidden.
    assert!(report.positive_delta_without_instruction > 0);
}

// ---- scripted stub provider -------------------------------------------

#[derive(Clone)]
enum Script {
    Txs {
        txs: Vec<RawSolanaTransaction>,
        truncated: bool,
    },
    TxsThenError {
        txs: Vec<RawSolanaTransaction>,
    },
    PlanError(String),
    Config,
    Foreign,
}

struct Stub {
    by_mint: BTreeMap<SolanaPubkey, Script>,
}

#[async_trait::async_trait]
impl HistoryProvider for Stub {
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::empty()
    }

    async fn plan(&self, request: &ScanRequest) -> Result<ScanPlan, ProviderError> {
        if let ScanRequest::TokenMarketActivity {
            asset: AssetKey::Token(_, AddressBytes::Solana(mint)),
        } = request
        {
            match self.by_mint.get(mint) {
                Some(Script::PlanError(msg)) => {
                    return Err(ProviderError::Other(Box::new(std::io::Error::other(
                        msg.clone(),
                    ))));
                }
                Some(Script::Config) => {
                    return Err(ProviderError::ConfigurationRequired {
                        port: "solana_history".to_string(),
                        detail: "stub".to_string(),
                    });
                }
                _ => {}
            }
        }
        Ok(ScanPlan {
            request_echo: String::new(),
            capabilities: SourceCapabilities::empty(),
        })
    }

    fn scan(
        &self,
        task: ScanTask,
        _cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>> {
        let ScanRequest::TokenMarketActivity {
            asset: AssetKey::Token(_, AddressBytes::Solana(mint)),
        } = &task.request
        else {
            return Box::pin(stream::empty());
        };
        let items: Vec<Result<ScanEnvelope, ProviderError>> = match self.by_mint.get(mint) {
            Some(Script::Txs { txs, truncated }) => txs
                .iter()
                .map(|tx| {
                    Ok(ScanEnvelope {
                        payload: RawPayload::SolanaTransaction(tx.clone()),
                        truncated: *truncated,
                    })
                })
                .collect(),
            Some(Script::TxsThenError { txs }) => {
                let mut v: Vec<_> = txs
                    .iter()
                    .map(|tx| {
                        Ok(ScanEnvelope {
                            payload: RawPayload::SolanaTransaction(tx.clone()),
                            truncated: false,
                        })
                    })
                    .collect();
                v.push(Err(ProviderError::Other(Box::new(std::io::Error::other(
                    "boom https://x.example/?api-key=SECRETKEY123&y=1",
                )))));
                v
            }
            Some(Script::Foreign) => vec![Ok(ScanEnvelope {
                payload: RawPayload::SolanaInstruction(RawSolanaInstruction {
                    program_id: [0; 32],
                    accounts: vec![],
                    data: vec![],
                    instruction_index: 0,
                }),
                truncated: false,
            })],
            _ => vec![],
        };
        Box::pin(stream::iter(items))
    }
}

fn pk(b: u8) -> SolanaPubkey {
    [b; 32]
}

fn buy_ix(user: u8, mint: u8, idx: u32) -> RawSolanaInstruction {
    let mut accounts: Vec<SolanaPubkey> = (0..16u8).map(|i| pk(100 + i)).collect();
    accounts[2] = pk(mint);
    accounts[6] = pk(user);
    let mut data = BUY_INSTRUCTION_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&2u64.to_le_bytes());
    data.push(1);
    RawSolanaInstruction {
        program_id: pubkey(PUMP_BONDING_CURVE_PROGRAM_ID),
        accounts,
        data,
        instruction_index: idx,
    }
}

fn sell_ix(user: u8, mint: u8, idx: u32) -> RawSolanaInstruction {
    let mut accounts: Vec<SolanaPubkey> = (0..14u8).map(|i| pk(100 + i)).collect();
    accounts[2] = pk(mint);
    accounts[6] = pk(user);
    let mut data = SELL_INSTRUCTION_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    RawSolanaInstruction {
        program_id: pubkey(PUMP_BONDING_CURVE_PROGRAM_ID),
        accounts,
        data,
        instruction_index: idx,
    }
}

fn bal(mint: u8, owner: u8, pre: Option<u64>, post: u64) -> SolanaTokenBalanceChange {
    SolanaTokenBalanceChange {
        mint: pk(mint),
        owner: Some(pk(owner)),
        decimals: 6,
        pre_amount: pre,
        post_amount: post,
        closed: false,
    }
}

fn buy_tx(user: u8, mint: u8) -> RawSolanaTransaction {
    RawSolanaTransaction {
        signature: [user; 64],
        execution: SolanaExecutionStatus::Succeeded,
        slot: 1000,
        transaction_index: 0,
        instructions: vec![buy_ix(user, mint, 0)],
        token_balance_changes: vec![bal(mint, user, None, 10)],
        fee_lamports: 5_000,
        fee_payer: pk(user),
        signers: vec![pk(user)],
        native_balance_changes: vec![],
    }
}

fn token(b: u8) -> AssetKey {
    AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(pk(b)))
}

fn wallet(b: u8) -> WalletKey {
    WalletKey {
        chain: solana_mainnet_chain(),
        address: AddressBytes::Solana(pk(b)),
    }
}

fn stub(entries: Vec<(u8, Script)>) -> Stub {
    Stub {
        by_mint: entries.into_iter().map(|(m, s)| (pk(m), s)).collect(),
    }
}

fn txs(t: Vec<RawSolanaTransaction>) -> Script {
    Script::Txs {
        txs: t,
        truncated: false,
    }
}

#[tokio::test]
async fn multi_token_intersection_threshold_and_deterministic_order() {
    // wallets: 3 buys T1,T2,T3 ; 1 buys T1,T2 ; 2 buys T1,T2 ; 9 buys only T1 (100 buys).
    let mut t1 = vec![buy_tx(3, 1), buy_tx(1, 1), buy_tx(2, 1)];
    t1.extend((0..100).map(|_| buy_tx(9, 1)));
    let provider = stub(vec![
        (1, txs(t1)),
        (2, txs(vec![buy_tx(3, 2), buy_tx(2, 2), buy_tx(1, 2)])),
        (3, txs(vec![buy_tx(3, 3)])),
    ]);
    let report = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2), token(3)],
        2,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let order: Vec<_> = report
        .base
        .matches
        .iter()
        .map(|m| (m.wallet.clone(), m.hit_count))
        .collect();
    // hit_count desc, then wallet asc; wallet 9 (100 buys of one token) excluded (B02).
    assert_eq!(order, vec![(wallet(3), 3), (wallet(1), 2), (wallet(2), 2)]);
    assert_eq!(report.base.input_token_count, 3);
    assert!(!report.is_coverage_incomplete());
}

#[tokio::test]
async fn k_threshold_excludes_single_token_buyers() {
    let provider = stub(vec![
        (1, txs(vec![buy_tx(1, 1), buy_tx(2, 1)])),
        (2, txs(vec![buy_tx(1, 2)])),
    ]);
    let report = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2)],
        2,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.base.matches.len(), 1);
    assert_eq!(report.base.matches[0].wallet, wallet(1));
    let k1 = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2)],
        1,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(k1.base.matches.len(), 2);
}

#[tokio::test]
async fn buy_of_non_input_mint_does_not_count() {
    // T1 scan also contains a buy of unrelated mint 50 by wallet 1.
    let provider = stub(vec![
        (1, txs(vec![buy_tx(1, 1), buy_tx(1, 50)])),
        (2, txs(vec![buy_tx(1, 2)])),
    ]);
    let report = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2)],
        2,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.base.matches[0].hit_count, 2);
    assert_eq!(
        report.base.matches[0].matched_assets,
        vec![token(1), token(2)]
    );
}

#[tokio::test]
async fn truncated_envelope_latches_flag_and_marks_incomplete() {
    let provider = stub(vec![
        (
            1,
            Script::Txs {
                txs: vec![buy_tx(1, 1)],
                truncated: true,
            },
        ),
        (2, txs(vec![buy_tx(1, 2)])),
    ]);
    let report = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2)],
        2,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(report.base.coverage_truncated);
    assert!(report.per_token[0].truncated);
    assert!(!report.per_token[1].truncated);
    assert!(report.is_coverage_incomplete());
    assert_eq!(report.base.matches.len(), 1);
}

#[tokio::test]
async fn one_token_failure_keeps_n_and_marks_partial_b08() {
    let provider = stub(vec![
        (1, txs(vec![buy_tx(1, 1)])),
        (2, Script::PlanError("rate budget".to_string())),
        (3, txs(vec![buy_tx(1, 3)])),
    ]);
    let report = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2), token(3)],
        2,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.base.input_token_count, 3);
    assert!(matches!(
        report.per_token[1].status,
        TokenScanStatus::Failed {
            kind: ScanFailureKind::Other,
            ..
        }
    ));
    // A non-terminal error keeps scanning: token 3 was scanned.
    assert_eq!(report.per_token[2].status, TokenScanStatus::Ok);
    assert!(report.stop.is_none());
    assert!(report.is_coverage_incomplete());
    // Hits from the healthy tokens survive.
    assert_eq!(report.base.matches.len(), 1);
}

#[tokio::test]
async fn mid_stream_error_is_recorded_and_secret_is_redacted() {
    let provider = stub(vec![
        (
            1,
            Script::TxsThenError {
                txs: vec![buy_tx(1, 1)],
            },
        ),
        (2, txs(vec![buy_tx(1, 2)])),
    ]);
    let report = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2)],
        2,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let error = report.per_token[0].error_text().unwrap().to_string();
    assert!(!error.contains("SECRETKEY123"), "{error}");
    assert!(error.contains("<redacted>"));
    assert!(report.is_coverage_incomplete());
    assert_eq!(report.base.matches.len(), 1);
}

#[tokio::test]
async fn all_tokens_failing_is_an_error_and_config_required_aborts() {
    let provider = stub(vec![
        (1, Script::PlanError("down".to_string())),
        (2, Script::PlanError("down".to_string())),
    ]);
    let result = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2)],
        2,
        CancellationToken::new(),
    )
    .await;
    assert!(result.is_err());

    let provider = stub(vec![(1, txs(vec![buy_tx(1, 1)])), (2, Script::Config)]);
    let result = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2)],
        2,
        CancellationToken::new(),
    )
    .await;
    assert!(matches!(
        result,
        Err(ProviderError::ConfigurationRequired { .. })
    ));
}

#[tokio::test]
async fn malformed_instruction_makes_coverage_incomplete() {
    let mut bad = buy_tx(1, 1);
    bad.instructions[0].data.truncate(23);
    let provider = stub(vec![(1, txs(vec![bad])), (2, txs(vec![buy_tx(1, 2)]))]);
    let report = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2)],
        1,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.diagnostics.malformed_instructions, 1);
    assert_eq!(report.malformed_samples.len(), 1);
    assert!(report.is_coverage_incomplete());
}

#[tokio::test]
async fn roundtrip_tx_is_not_a_hit_and_unexpected_payload_is_a_gap() {
    let roundtrip = RawSolanaTransaction {
        signature: [1; 64],
        execution: SolanaExecutionStatus::Succeeded,
        slot: 1,
        transaction_index: 0,
        instructions: vec![buy_ix(1, 1, 0), sell_ix(1, 1, 1)],
        token_balance_changes: vec![bal(1, 1, Some(5), 5)],
        fee_lamports: 5_000,
        fee_payer: pk(1),
        signers: vec![pk(1)],
        native_balance_changes: vec![],
    };
    let provider = stub(vec![(1, txs(vec![roundtrip])), (2, Script::Foreign)]);
    let report = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2)],
        1,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(report.base.matches.is_empty());
    assert_eq!(report.unexpected_payloads, 1);
    assert!(report.is_coverage_incomplete());
}

#[tokio::test]
async fn non_solana_or_native_input_is_unsupported_and_cancel_is_reported() {
    let provider = stub(vec![]);
    let evm = AssetKey::Native(solana_mainnet_chain());
    let result = run_solana_buyer_intersect(&provider, &[evm], 1, CancellationToken::new()).await;
    assert!(matches!(result, Err(ProviderError::Unsupported { .. })));

    let cancel = CancellationToken::new();
    cancel.cancel();
    let report = run_solana_buyer_intersect(&provider, &[token(1), token(2)], 1, cancel)
        .await
        .unwrap();
    assert!(report.cancelled);
    assert!(report.is_coverage_incomplete());
    assert_eq!(report.base.input_token_count, 2);
}

#[test]
fn sanitize_strips_key_and_control_chars() {
    let s = sanitize_provider_text("err at https://h/?api-key=abc-123_X&z=1\u{1b}[31m end");
    assert!(!s.contains("abc-123_X"));
    assert!(s.contains("api-key=<redacted>&z=1"));
    assert!(!s.contains('\u{1b}'));
}

// ---- all-variant coverage (invariants 16 and 18) ------------------------

/// Net owner delta from the normalized provider output.
fn owner_delta(tx: &RawSolanaTransaction, mint: &SolanaPubkey, owner: &SolanaPubkey) -> i128 {
    tx.token_balance_changes
        .iter()
        .filter(|c| c.mint == *mint && c.owner.as_ref() == Some(owner))
        .map(|c| i128::from(c.post_amount) - i128::from(c.pre_amount.unwrap_or(0)))
        .sum()
}

fn trades_in(tx: &RawSolanaTransaction) -> Vec<scout_dex_solana::DecodedBondingCurveTrade> {
    let decoder = pump_bonding_curve_decoder().unwrap();
    tx.instructions
        .iter()
        .filter_map(
            |ix| match decoder.classify(ix, tx.slot, tx.transaction_index) {
                PumpInstructionOutcome::Trade(t) => Some(t),
                _ => None,
            },
        )
        .collect()
}

#[tokio::test]
async fn fixture_verified_variants_match_real_tx_balance_deltas() {
    let server = MockServer::start().await;
    mount_fixture(&server, PROBE_MINT, "pump_bonding_curve_buy_probe.json").await;
    let provider = helius(&server);
    let txs = collect_transactions(&provider, PROBE_MINT).await;
    assert_eq!(txs.len(), 5);
    let fixture = fixture_json("pump_bonding_curve_buy_probe.json");

    let mut seen: BTreeMap<PumpTradeVariant, u32> = BTreeMap::new();
    for (pos, tx) in txs.iter().enumerate() {
        assert_eq!(
            tx.transaction_index,
            fixture["result"]["data"][pos]["transactionIndex"]
                .as_u64()
                .unwrap()
        );
        for t in trades_in(tx) {
            *seen.entry(t.variant).or_default() += 1;
            // Skip the failed-tx sample (checked separately).
            if t.variant == PumpTradeVariant::BuyExactQuoteInV2 {
                continue;
            }
            // Normalized provider data (closed accounts included).
            let delta = owner_delta(tx, &t.mint, &t.user);
            let amount = i128::from(t.args[0].value);
            match t.side {
                // Real balances: the decoded user (owner) lost exactly
                // `amount` base tokens (sell/sell_v2) ...
                TradeSide::Sell => assert_eq!(delta, -amount, "{:?}", t.variant),
                // ... or the decoded user gained a positive amount.
                TradeSide::Buy => assert!(delta > 0, "{:?} delta {delta}", t.variant),
            }
            assert_eq!(
                t.variant.verification(),
                VariantVerification::FixtureVerified
            );
        }
    }
    // sell_v2 comes from tx0's INNER instruction.
    assert_eq!(seen.get(&PumpTradeVariant::SellV2), Some(&1));
    assert_eq!(seen.get(&PumpTradeVariant::Sell), Some(&1));
    assert_eq!(seen.get(&PumpTradeVariant::Buy), Some(&2));
    assert_eq!(seen.get(&PumpTradeVariant::BuyExactQuoteInV2), Some(&1));

    // The v2 sell's decoded quote mint is distinct from the base mint
    // and its user lost the base token (positions [1]/[2]/[13]).
    let sell_v2 = txs
        .iter()
        .flat_map(trades_in)
        .find(|t| t.variant == PumpTradeVariant::SellV2)
        .unwrap();
    assert_ne!(Some(sell_v2.mint), sell_v2.quote_mint);
    assert_eq!(
        sell_v2.mint,
        pubkey("67266Ha2icrdCKHyrYKyG4oJyJ7RqheaGbuGd6vwXbLD")
    );
    assert_eq!(
        sell_v2.user,
        pubkey("46cZwSYHg2Cpc1ViiNr7kBxRtkyNJyJEzzdkMJv8Kkku")
    );
    assert_eq!(
        sell_v2.quote_mint,
        Some(pubkey("So11111111111111111111111111111111111111112"))
    );
}

#[tokio::test]
async fn failed_quote_in_v2_tx_decodes_but_never_qualifies() {
    let server = MockServer::start().await;
    mount_fixture(&server, PROBE_MINT, "pump_bonding_curve_buy_probe.json").await;
    let provider = helius(&server);
    let decoder = pump_bonding_curve_decoder().unwrap();
    let txs = collect_transactions(&provider, PROBE_MINT).await;
    let tx = txs
        .iter()
        .find(|tx| {
            trades_in(tx)
                .iter()
                .any(|t| t.variant == PumpTradeVariant::BuyExactQuoteInV2)
        })
        .unwrap();
    let trade = trades_in(tx)
        .into_iter()
        .find(|t| t.variant == PumpTradeVariant::BuyExactQuoteInV2)
        .unwrap();
    assert_eq!(trade.side, TradeSide::Buy);
    assert_eq!(trade.mint, pubkey(PROBE_MINT));
    assert!(trade.quote_mint.is_some());
    // Failed tx: no positive owner delta for the decoded user.
    assert!(owner_delta(tx, &trade.mint, &trade.user) <= 0);
    let q = qualify_bonding_curve_buys(tx, &decoder);
    assert!(q.buys.is_empty());
    assert!(q.unverified_buys.is_empty());
    assert_eq!(q.diagnostics.malformed_instructions, 0);
    assert_eq!(q.diagnostics.unknown_discriminator_instructions, 0);
    assert_eq!(q.diagnostics.failed_transactions, 1);
    assert_eq!(q.diagnostics.decoded_buys, 0);
    assert!(q.uninstructed_positive_deltas.is_empty());
}

#[tokio::test]
async fn execution_status_and_closed_accounts_from_probe_fixture() {
    let server = MockServer::start().await;
    mount_fixture(&server, PROBE_MINT, "pump_bonding_curve_buy_probe.json").await;
    let provider = helius(&server);
    let all = collect_transactions(&provider, PROBE_MINT).await;
    for tx in &all[..4] {
        assert_eq!(tx.execution, SolanaExecutionStatus::Succeeded);
    }
    match &all[4].execution {
        SolanaExecutionStatus::Failed { error } => {
            assert!(error.contains("InstructionError") && error.contains("6042"));
        }
        other => panic!("tx4 must be Failed, got {other:?}"),
    }
    // Failed tx counts as failed and yields zero buys through the engine.
    let report = run_solana_buyer_intersect(
        &stub(vec![(1, txs(all.clone()))]),
        &[token(1)],
        1,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.diagnostics.failed_transactions, 1);
    assert_eq!(report.per_token[0].diagnostics.failed_transactions, 1);

    // tx1: full `sell` closes the seller's ATA; the normalized data now
    // carries the closure and the exact outflow.
    let sell = all
        .iter()
        .flat_map(trades_in)
        .find(|t| t.variant == PumpTradeVariant::Sell)
        .unwrap();
    let sell_tx = all
        .iter()
        .find(|tx| {
            trades_in(tx)
                .iter()
                .any(|t| t.variant == PumpTradeVariant::Sell)
        })
        .unwrap();
    assert_eq!(sell.args[0].value, 175_202_561_501);
    assert_eq!(
        owner_delta(sell_tx, &sell.mint, &sell.user),
        -175_202_561_501
    );
    assert!(bs58::encode(sell.user).into_string().starts_with("FoaRt"));
    assert!(
        sell_tx
            .token_balance_changes
            .iter()
            .any(|c| c.closed && c.owner == Some(sell.user) && c.post_amount == 0)
    );
}

#[tokio::test]
async fn event_cpi_is_counted_as_known_non_trade_across_the_probe_fixture() {
    let server = MockServer::start().await;
    mount_fixture(&server, PROBE_MINT, "pump_bonding_curve_buy_probe.json").await;
    let provider = helius(&server);
    let decoder = pump_bonding_curve_decoder().unwrap();
    let txs = collect_transactions(&provider, PROBE_MINT).await;
    let mut cpi = 0u64;
    let mut direct_total = 0usize;
    for tx in &txs {
        let q = qualify_bonding_curve_buys(tx, &decoder);
        assert_eq!(q.diagnostics.unknown_discriminator_instructions, 0);
        cpi = cpi.saturating_add(q.diagnostics.known_non_trade_instructions);
        let direct = tx
            .instructions
            .iter()
            .filter(|ix| {
                ix.program_id == pubkey(PUMP_BONDING_CURVE_PROGRAM_ID)
                    && ix.data.get(0..8) == Some(&EVENT_CPI_DISCRIMINATOR[..])
            })
            .count();
        assert_eq!(
            q.diagnostics.known_non_trade_instructions,
            u64::try_from(direct).unwrap()
        );
        direct_total += direct;
    }
    // tx0..tx3 each carry one pump-program event-CPI; tx4 (failed
    // before emitting) carries none; the router-level event-CPI in tx3
    // belongs to another program and is NotMine.
    assert_eq!(cpi, 4);
    assert_eq!(direct_total, 4);
}

fn program_ix(
    accounts: usize,
    user: u8,
    mint_idx: usize,
    user_idx: usize,
    mint: u8,
    data: Vec<u8>,
) -> RawSolanaInstruction {
    let mut a: Vec<SolanaPubkey> = (0..accounts)
        .map(|i| pk(100u8 + u8::try_from(i).unwrap()))
        .collect();
    a[mint_idx] = pk(mint);
    a[user_idx] = pk(user);
    RawSolanaInstruction {
        program_id: pubkey(PUMP_BONDING_CURVE_PROGRAM_ID),
        accounts: a,
        data,
        instruction_index: 0,
    }
}

fn args_data(disc: [u8; 8], track: bool) -> Vec<u8> {
    let mut d = disc.to_vec();
    d.extend_from_slice(&5u64.to_le_bytes());
    d.extend_from_slice(&6u64.to_le_bytes());
    if track {
        d.push(1);
    }
    d
}

fn tx_with(ix: RawSolanaInstruction, user: u8, mint: u8) -> RawSolanaTransaction {
    RawSolanaTransaction {
        signature: [user; 64],
        execution: SolanaExecutionStatus::Succeeded,
        slot: 1000,
        transaction_index: 0,
        instructions: vec![ix],
        token_balance_changes: vec![bal(mint, user, None, 10)],
        fee_lamports: 5_000,
        fee_payer: pk(user),
        signers: vec![pk(user)],
        native_balance_changes: vec![],
    }
}

/// Test-only policy keeping the `IdlOnly` path covered now that no
/// shipped variant is `IdlOnly`.
fn sol_in_idl_only(variant: PumpTradeVariant) -> VariantVerification {
    if variant == PumpTradeVariant::BuyExactSolIn {
        VariantVerification::IdlOnly
    } else {
        variant.verification()
    }
}

#[tokio::test]
async fn buy_exact_sol_in_with_positive_delta_is_a_match_under_default_policy() {
    let ix = program_ix(
        16,
        1,
        2,
        6,
        1,
        args_data(BUY_EXACT_SOL_IN_INSTRUCTION_DISCRIMINATOR, true),
    );
    let provider = stub(vec![(1, txs(vec![tx_with(ix, 1, 1)]))]);
    let report = run_solana_buyer_intersect(&provider, &[token(1)], 1, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.per_token[0].qualified_buyers, 1);
    assert_eq!(report.diagnostics.decoded_buys, 1);
    assert_eq!(report.diagnostics.unverified_variant_buys, [0; 6]);
    assert!(!report.is_coverage_incomplete());
}

#[tokio::test]
async fn idl_only_buy_with_positive_delta_is_not_a_match_is_counted_and_incomplete() {
    let variant = PumpTradeVariant::BuyExactSolIn;
    let ix = program_ix(
        16,
        1,
        2,
        6,
        1,
        args_data(BUY_EXACT_SOL_IN_INSTRUCTION_DISCRIMINATOR, true),
    );
    let provider = stub(vec![(1, txs(vec![tx_with(ix, 1, 1)]))]);
    let report = run_solana_buyer_intersect_with_policy(
        &provider,
        &[token(1)],
        1,
        CancellationToken::new(),
        sol_in_idl_only,
    )
    .await
    .unwrap();
    assert!(report.base.matches.is_empty());
    assert_eq!(report.per_token[0].qualified_buyers, 0);
    assert_eq!(report.diagnostics.decoded_buys, 0);
    assert_eq!(
        report.diagnostics.unverified_variant_buys[variant.index()],
        1
    );
    assert!(report.is_coverage_incomplete());
    let reasons = report.incomplete_reasons();
    assert!(
        reasons.iter().any(|r| r.contains(variant.name())),
        "{reasons:?}"
    );
}

#[tokio::test]
async fn promoted_variant_buy_with_positive_delta_is_a_match_and_complete() {
    let cases: Vec<(PumpTradeVariant, RawSolanaInstruction)> = vec![
        (
            PumpTradeVariant::BuyV2,
            program_ix(
                27,
                1,
                1,
                13,
                1,
                args_data(BUY_V2_INSTRUCTION_DISCRIMINATOR, false),
            ),
        ),
        (
            PumpTradeVariant::BuyExactQuoteInV2,
            program_ix(
                27,
                1,
                1,
                13,
                1,
                args_data(BUY_EXACT_QUOTE_IN_V2_INSTRUCTION_DISCRIMINATOR, false),
            ),
        ),
    ];
    for (variant, ix) in cases {
        let provider = stub(vec![(1, txs(vec![tx_with(ix, 1, 1)]))]);
        let report =
            run_solana_buyer_intersect(&provider, &[token(1)], 1, CancellationToken::new())
                .await
                .unwrap();
        assert_eq!(report.base.matches.len(), 1, "{variant:?}");
        assert_eq!(report.per_token[0].qualified_buyers, 1);
        assert_eq!(report.diagnostics.decoded_buys, 1);
        assert_eq!(
            report.diagnostics.unverified_variant_buys,
            [0; PumpTradeVariant::COUNT]
        );
        assert_eq!(report.diagnostics.decoded_by_variant[variant.index()], 1);
        assert!(!report.is_coverage_incomplete(), "{variant:?}");
    }
}

#[tokio::test]
async fn idl_only_buy_of_a_non_input_mint_does_not_flag_the_run() {
    let ix = program_ix(
        27,
        1,
        1,
        13,
        7,
        args_data(BUY_V2_INSTRUCTION_DISCRIMINATOR, false),
    );
    let provider = stub(vec![(1, txs(vec![tx_with(ix, 1, 7)]))]);
    let report = run_solana_buyer_intersect(&provider, &[token(1)], 1, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        report.diagnostics.unverified_variant_buys,
        [0; PumpTradeVariant::COUNT]
    );
    assert!(!report.is_coverage_incomplete());
}

#[tokio::test]
async fn idl_only_buy_without_positive_delta_is_not_flagged() {
    let ix = program_ix(
        27,
        1,
        1,
        13,
        1,
        args_data(BUY_V2_INSTRUCTION_DISCRIMINATOR, false),
    );
    let mut tx = tx_with(ix, 1, 1);
    tx.token_balance_changes = vec![bal(1, 1, Some(10), 10)];
    let provider = stub(vec![(1, txs(vec![tx]))]);
    let report = run_solana_buyer_intersect(&provider, &[token(1)], 1, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        report.diagnostics.unverified_variant_buys,
        [0; PumpTradeVariant::COUNT]
    );
    assert!(!report.is_coverage_incomplete());
}

#[tokio::test]
async fn unknown_discriminator_under_the_program_is_a_coverage_gap() {
    let mut ix = buy_ix(1, 1, 0);
    ix.data[0..8].copy_from_slice(&[9, 9, 9, 9, 9, 9, 9, 9]);
    let provider = stub(vec![(1, txs(vec![tx_with(ix, 1, 1)]))]);
    let report = run_solana_buyer_intersect(&provider, &[token(1)], 1, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.diagnostics.unknown_discriminator_instructions, 1);
    assert_eq!(report.diagnostics.malformed_instructions, 0);
    assert_eq!(
        report.unknown_discriminator_samples,
        vec!["0909090909090909"]
    );
    assert!(report.base.matches.is_empty());
    assert!(report.is_coverage_incomplete());
    assert!(
        report
            .incomplete_reasons()
            .iter()
            .any(|r| r.contains("not in the pinned IDL"))
    );
}

#[tokio::test]
async fn known_non_trade_instruction_is_counted_and_not_a_gap() {
    let mut ix = buy_ix(1, 1, 0);
    ix.data = EVENT_CPI_DISCRIMINATOR.to_vec();
    let provider = stub(vec![(1, txs(vec![tx_with(ix, 1, 1)]))]);
    let report = run_solana_buyer_intersect(&provider, &[token(1)], 1, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.diagnostics.known_non_trade_instructions, 1);
    assert_eq!(report.diagnostics.unknown_discriminator_instructions, 0);
    assert!(!report.is_coverage_incomplete());
}

#[test]
fn scope_lists_every_variant_with_status_and_idl_hash() {
    let scope = scout_engine::SolanaProtocolScope::pump_bonding_curve();
    assert_eq!(scope.idl_sha256, scout_dex_solana::PUMP_IDL_SHA256);
    assert_eq!(
        scope.qualification_version,
        "pump-bonding-curve-buy/idl-e0687ae/v4"
    );
    let v = scout_engine::SolanaProtocolScope::variants();
    assert_eq!(v.len(), 6);
    assert!(v.contains(&("buy", "buy", "FixtureVerified")));
    assert!(v.contains(&("sell_v2", "sell", "FixtureVerified")));
    assert!(v.contains(&("buy_exact_sol_in", "buy", "FixtureVerified")));
    assert!(v.contains(&("buy_v2", "buy", "FixtureVerified")));
    assert!(v.contains(&("buy_exact_quote_in_v2", "buy", "FixtureVerified")));
}

const WSOL: &str = "So11111111111111111111111111111111111111112";
const LIVE_VARIANTS_FIXTURE: &str = "pump_variants_live_2026-10-02.json";

/// Serve the committed live capture's 16 successful txs through the real
/// `HeliusProvider` (the same decode path production uses) and return
/// them paired with the fixture's selection label.
async fn live_variant_txs() -> Vec<(String, RawSolanaTransaction)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(LIVE_VARIANTS_FIXTURE);
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(fixture["captured_at_utc"], "2026-10-02T13:40:20Z");
    let data: Vec<serde_json::Value> = fixture["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["data"].as_array().unwrap().clone())
        .collect();
    let labels: BTreeMap<String, String> = fixture["selection"]["signatures"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect();
    assert_eq!(labels.len(), 16);
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "data": data, "paginationToken": null }
    });
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let provider = helius(&server);
    let txs = collect_transactions(&provider, MINT1).await;
    assert_eq!(txs.len(), data.len());
    txs.into_iter()
        .filter_map(|tx| {
            let sig = bs58::encode(tx.signature).into_string();
            labels.get(&sig).map(|l| (l.clone(), tx))
        })
        .collect()
}

fn label_variant(label: &str) -> PumpTradeVariant {
    let name = label
        .strip_prefix("malformed:")
        .map_or(label, |r| r.split(':').next().unwrap());
    PumpTradeVariant::ALL
        .into_iter()
        .find(|v| v.name() == name)
        .unwrap()
}

/// Signature (prefix) of the one live `buy_exact_sol_in` tx whose decoded
/// `user` has a net-zero delta for the decoded mint: the 2928987168
/// base tokens land on another owner's new account in the same tx.
/// A router-forward: layout is right, user net is 0, the tokens end on
/// another owner (`CNudZYFg...`) with no instruction evidence. A correct
/// negative golden case for the promoted `buy_exact_sol_in`.
const SOL_IN_ZERO_DELTA_SIG_PREFIX: &str = "bUh87USDuBC4";

#[tokio::test]
async fn live_fixture_variant_promotion_evidence_via_owner_keyed_deltas() {
    let txs = live_variant_txs().await;
    assert_eq!(txs.len(), 16);
    // (variant, positive-delta txs, non-positive-delta txs)
    let mut tally: BTreeMap<PumpTradeVariant, (u32, u32)> = BTreeMap::new();
    let mut quote_mints: BTreeMap<String, u32> = BTreeMap::new();
    let wsol = pubkey(WSOL);
    for (label, tx) in &txs {
        let variant = label_variant(label);
        let sig = bs58::encode(tx.signature).into_string();
        assert!(tx.execution.is_success(), "{label}");
        let trades: Vec<_> = trades_in(tx)
            .into_iter()
            .filter(|t| t.variant == variant)
            .collect();
        assert_eq!(trades.len(), 1, "{label}: expected one decoded {variant:?}");
        let t = &trades[0];
        assert_eq!(t.side, TradeSide::Buy);
        // Arg-length policy per fixture row (observed live shapes).
        let expected = match (variant, label.starts_with("malformed:")) {
            (PumpTradeVariant::BuyExactSolIn, false) => (Some(0), 0),
            (PumpTradeVariant::BuyExactQuoteInV2, true) => (None, 1),
            _ => (None, 0),
        };
        assert_eq!((t.track_volume, t.trailing_arg_bytes), expected, "{label}");

        let delta = owner_delta(tx, &t.mint, &t.user);
        let entry = tally.entry(variant).or_default();
        if delta > 0 {
            entry.0 += 1;
        } else {
            entry.1 += 1;
            assert_eq!(variant, PumpTradeVariant::BuyExactSolIn, "{label}");
            assert!(sig.starts_with(SOL_IN_ZERO_DELTA_SIG_PREFIX), "{sig}");
            assert_eq!(delta, 0);
            continue;
        }
        let min_out = i128::from(t.args[1].value);
        match variant {
            // `amount` is the exact token amount requested.
            PumpTradeVariant::BuyV2 | PumpTradeVariant::Buy => {
                assert_eq!(delta, i128::from(t.args[0].value), "{label} amount");
            }
            // Exact-in: spend is fixed, tokens out has a floor.
            _ => assert!(
                delta >= min_out,
                "{label}: delta {delta} < min_out {min_out}"
            ),
        }
        if let Some(q) = t.quote_mint {
            let seen = tx.token_balance_changes.iter().any(|c| c.mint == q);
            assert!(
                q == wsol || seen,
                "{label}: quote mint not wSOL nor in balances"
            );
            *quote_mints
                .entry(bs58::encode(q).into_string())
                .or_default() += 1;
        }
    }
    assert_eq!(tally[&PumpTradeVariant::Buy], (3, 0));
    assert_eq!(tally[&PumpTradeVariant::BuyV2], (2, 0));
    assert_eq!(tally[&PumpTradeVariant::BuyExactQuoteInV2], (5, 0));
    // 6/6 confirm the layout: 5 positive user deltas, 1 router-forward
    // (user net 0, tokens on another owner; see the dedicated test).
    assert_eq!(tally[&PumpTradeVariant::BuyExactSolIn], (5, 1));
    for v in [
        PumpTradeVariant::Buy,
        PumpTradeVariant::BuyExactSolIn,
        PumpTradeVariant::BuyV2,
        PumpTradeVariant::BuyExactQuoteInV2,
    ] {
        assert_eq!(v.verification(), VariantVerification::FixtureVerified);
    }
    eprintln!("quote_mints={quote_mints:?}");
}

#[tokio::test]
async fn live_non_idl_length_txs_decode_and_qualify_under_default_policy() {
    let txs = live_variant_txs().await;
    let decoder = pump_bonding_curve_decoder().unwrap();
    let mut seen = 0;
    for (label, tx) in txs.iter().filter(|(l, _)| l.starts_with("malformed:")) {
        seen += 1;
        let variant = label_variant(label);
        let q = qualify_bonding_curve_buys(tx, &decoder);
        assert_eq!(q.diagnostics.malformed_instructions, 0, "{label}");
        assert!(q.malformed_reasons.is_empty(), "{label}");
        assert_eq!(
            q.diagnostics.decoded_by_variant[variant.index()],
            1,
            "{label}"
        );
        let sig = bs58::encode(tx.signature).into_string();
        if sig.starts_with(SOL_IN_ZERO_DELTA_SIG_PREFIX) {
            // Router-forward: neither a buy nor an unverified buy.
            assert!(q.buys.is_empty(), "{label}");
            assert!(q.unverified_buys.is_empty());
            assert_eq!(q.diagnostics.unverified_variant_buys, [0; 6]);
        } else {
            assert_eq!(q.buys.len(), 1, "{label}");
            assert!(q.buys[0].net_delta.to_string().parse::<u128>().unwrap() > 0);
        }
    }
    assert_eq!(seen, 8);
}

#[tokio::test]
async fn live_buy_exact_sol_in_under_injected_idl_only_policy_is_never_a_buyer() {
    let txs = live_variant_txs().await;
    let decoder = pump_bonding_curve_decoder().unwrap();
    let mut sol_in = 0;
    for (label, tx) in &txs {
        if label_variant(label) != PumpTradeVariant::BuyExactSolIn {
            continue;
        }
        sol_in += 1;
        let q = qualify_bonding_curve_buys_with_policy(tx, &decoder, sol_in_idl_only);
        assert!(q.buys.is_empty(), "{label}");
        let forward = bs58::encode(tx.signature)
            .into_string()
            .starts_with(SOL_IN_ZERO_DELTA_SIG_PREFIX);
        assert_eq!(q.unverified_buys.len(), usize::from(!forward), "{label}");
    }
    assert_eq!(sol_in, 6);
}

#[tokio::test]
async fn router_forwarded_buy_exact_sol_in_is_not_attributed_to_user_or_recipient() {
    const USER: &str = "ARu4n5mFdZogZAravu7CcizaojWnS6oqka37gdLT5SZn";
    const RECIPIENT: &str = "CNudZYFgpbT26fidsiNrWfHeGTBMMeVWqruZXsEkcUPc";
    const FORWARD_MINT: &str = "26GNNvy4BkuTyGRX2YQm3hey1JUgTKNsUZRKS8gcpump";
    let txs = live_variant_txs().await;
    let (_, tx) = txs
        .iter()
        .find(|(_, tx)| {
            bs58::encode(tx.signature)
                .into_string()
                .starts_with(SOL_IN_ZERO_DELTA_SIG_PREFIX)
        })
        .unwrap();
    let (user, recipient, mint) = (pubkey(USER), pubkey(RECIPIENT), pubkey(FORWARD_MINT));
    // Instruction evidence names ARu4 and the mint; its net is exactly 0.
    let trade = trades_in(tx)
        .into_iter()
        .find(|t| t.variant == PumpTradeVariant::BuyExactSolIn)
        .unwrap();
    assert_eq!((trade.user, trade.mint), (user, mint));
    assert_eq!(owner_delta(tx, &mint, &user), 0);
    // The recipient gains tokens but no instruction names it.
    assert!(owner_delta(tx, &mint, &recipient) > 0);

    let decoder = pump_bonding_curve_decoder().unwrap();
    let q = qualify_bonding_curve_buys(tx, &decoder);
    assert!(q.buys.is_empty());
    assert!(q.unverified_buys.is_empty());
    assert_eq!(q.diagnostics.unverified_variant_buys, [0; 6]);
    assert_eq!(q.diagnostics.decoded_buys, 1);
    assert_eq!(q.diagnostics.buys_without_positive_delta, 1);
    assert!(q.uninstructed_positive_deltas.contains(&(mint, recipient)));
    assert!(
        !q.uninstructed_positive_deltas
            .iter()
            .any(|(_, owner)| *owner == user)
    );
}

/// Provider whose token `fail_mint` yields one transaction then a
/// terminal error; records every mint it was asked to plan or scan.
struct StopStub {
    fail_mint: SolanaPubkey,
    make_error: fn() -> ProviderError,
    requested: std::sync::Mutex<Vec<SolanaPubkey>>,
}

impl StopStub {
    fn new(fail_mint: u8, make_error: fn() -> ProviderError) -> Self {
        Self {
            fail_mint: pk(fail_mint),
            make_error,
            requested: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn requested_mints(&self) -> Vec<SolanaPubkey> {
        self.requested.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl HistoryProvider for StopStub {
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::empty()
    }

    async fn plan(&self, request: &ScanRequest) -> Result<ScanPlan, ProviderError> {
        if let ScanRequest::TokenMarketActivity {
            asset: AssetKey::Token(_, AddressBytes::Solana(mint)),
        } = request
        {
            self.requested.lock().unwrap().push(*mint);
        }
        Ok(ScanPlan {
            request_echo: String::new(),
            capabilities: SourceCapabilities::empty(),
        })
    }

    fn scan(
        &self,
        task: ScanTask,
        _cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>> {
        let ScanRequest::TokenMarketActivity {
            asset: AssetKey::Token(_, AddressBytes::Solana(mint)),
        } = &task.request
        else {
            return Box::pin(stream::empty());
        };
        self.requested.lock().unwrap().push(*mint);
        let mint_byte = mint[0];
        let mut items = vec![Ok(ScanEnvelope {
            payload: RawPayload::SolanaTransaction(buy_tx(1, mint_byte)),
            truncated: false,
        })];
        if *mint == self.fail_mint {
            items.push(Err((self.make_error)()));
        }
        Box::pin(stream::iter(items))
    }
}

fn budget_error() -> ProviderError {
    ProviderError::Other(Box::new(scout_rpc::RequestBudgetExhausted { limit: 4 }))
}

fn rate_limited_error() -> ProviderError {
    ProviderError::RateLimited {
        retry_after: Some(std::time::Duration::from_secs(3600)),
    }
}

async fn stop_run(
    fail: fn() -> ProviderError,
) -> (scout_engine::SolanaBuyerIntersectReport, Vec<SolanaPubkey>) {
    let provider = StopStub::new(2, fail);
    let report = run_solana_buyer_intersect(
        &provider,
        &[token(1), token(2), token(3)],
        2,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    (report, provider.requested_mints())
}

fn assert_stopped_after_token_2(
    report: &scout_engine::SolanaBuyerIntersectReport,
    requested: &[SolanaPubkey],
    stop: ScanStop,
    kind: ScanFailureKind,
) {
    assert_eq!(report.base.input_token_count, 3);
    assert_eq!(report.per_token.len(), 3, "N never shrinks");
    assert_eq!(report.per_token[0].status, TokenScanStatus::Ok);
    assert!(matches!(
        &report.per_token[1].status,
        TokenScanStatus::Failed { kind: k, .. } if *k == kind
    ));
    assert_eq!(
        report.per_token[2].status,
        TokenScanStatus::NotScanned { reason: stop }
    );
    assert!(report.per_token[2].is_unknown());
    assert_eq!(report.stop, Some(stop));
    // Provider saw plan+scan for tokens 1 and 2 only, never token 3.
    assert!(!requested.contains(&pk(3)), "{requested:?}");
    assert_eq!(requested, &[pk(1), pk(1), pk(2), pk(2)]);
    // Never presented as complete.
    assert!(report.is_coverage_incomplete());
    let reasons = report.incomplete_reasons().join("\n");
    assert!(reasons.contains("not scanned"), "{reasons}");
    // Hits observed before the stop (tokens 1 and 2) are kept, but the
    // run is incomplete: the record is never presented as complete.
    assert_eq!(report.base.matches.len(), 1);
    assert!(report.per_token[1].qualified_buyers >= 1);
}

#[tokio::test]
async fn budget_exhausted_mid_run_stops_and_marks_rest_not_scanned() {
    let (report, requested) = stop_run(budget_error).await;
    assert_stopped_after_token_2(
        &report,
        &requested,
        ScanStop::BudgetExhausted { limit: 4 },
        ScanFailureKind::BudgetExhausted { limit: 4 },
    );
}

#[tokio::test]
async fn rate_limited_mid_run_stops_and_marks_rest_not_scanned() {
    let (report, requested) = stop_run(rate_limited_error).await;
    assert_stopped_after_token_2(
        &report,
        &requested,
        ScanStop::RateLimited {
            retry_after_secs: Some(3600),
        },
        ScanFailureKind::RateLimited {
            retry_after_secs: Some(3600),
        },
    );
}

#[test]
fn classification_is_typed_and_other_errors_do_not_stop() {
    assert_eq!(
        classify_provider_error(&budget_error()),
        ScanFailureKind::BudgetExhausted { limit: 4 }
    );
    assert_eq!(
        classify_provider_error(&ProviderError::RateLimited { retry_after: None }),
        ScanFailureKind::RateLimited {
            retry_after_secs: None
        }
    );
    let other = ProviderError::Other(Box::new(std::io::Error::other(
        "request budget exhausted (text only, wrong type)",
    )));
    let kind = classify_provider_error(&other);
    assert_eq!(kind, ScanFailureKind::Other);
    assert_eq!(kind.stop(), None);
}
