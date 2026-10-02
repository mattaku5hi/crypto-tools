//! Stable JSONL output DTOs for `buyer-intersect` (docs/CLI.md §7).
//!
//! These types are the wire contract. They are built field-by-field from
//! engine reports and never derive output from internal types' serde
//! shape, so an internal refactor cannot silently change the schema.
//! Addresses are full strings (base58 for Solana, `0x` lowercase hex for
//! EVM); counts are JSON numbers (all bounded `u64` far below 2^53 here);
//! unknown values are `null` with an explicit status, never `0`.

use scout_app::{SCHEMA_VERSION, chain_profile_name};
use scout_core::{AssetKey, ChainKey, WalletKey};
use scout_engine::{
    BuyerMatch, PumpTradeVariant, SolanaBuyerIntersectReport, SolanaProtocolScope,
    SolanaTokenScanSummary, TxQualificationDiagnostics,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct WalletDto {
    pub chain: &'static str,
    pub address: String,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum AssetDto {
    Token { chain: &'static str, token: String },
    Native { chain: &'static str, native: bool },
}

#[derive(Debug, Serialize)]
pub struct BuyerMatchRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub wallet: WalletDto,
    pub hit_count: usize,
    pub matched_assets: Vec<AssetDto>,
}

#[derive(Debug, Serialize)]
pub struct VariantDto {
    pub name: &'static str,
    pub side: &'static str,
    pub verification: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ScopeDto {
    pub chain: &'static str,
    pub program_id: &'static str,
    pub idl_commit: &'static str,
    pub idl_sha256: &'static str,
    pub qualification_version: &'static str,
    pub recognized: &'static str,
    pub not_decoded: &'static str,
    pub variants: Vec<VariantDto>,
}

#[derive(Debug, Serialize)]
pub struct BudgetDto {
    /// Provider pages requested per input token (`--max-pages-per-token`).
    /// Not the spec's `--max-requests` (retries are not counted).
    pub max_pages_per_token: u32,
}

#[derive(Debug, Serialize)]
pub struct RunMetaRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub run_id: String,
    pub captured_at: String,
    pub scope: ScopeDto,
    pub budget: BudgetDto,
    pub input_tokens: Vec<AssetDto>,
    pub input_token_count: usize,
    pub min_token_hits: usize,
}

#[derive(Debug, Serialize)]
pub struct TokenDiagnosticsDto {
    pub decoded_buys: u64,
    pub malformed_instructions: u64,
    pub unknown_discriminator_instructions: u64,
    pub unverified_variant_buys: u64,
    pub failed_transactions: u64,
    pub positive_delta_without_instruction: u64,
}

/// Per-token scan status. For `failed` every count is `null`: unknown is
/// never reported as zero. For `truncated` counts are lower bounds.
#[derive(Debug, Serialize)]
pub struct TokenStatusDto {
    pub token: AssetDto,
    /// `ok`, `truncated` or `failed`.
    pub status: &'static str,
    pub error: Option<String>,
    pub transactions_scanned: Option<u64>,
    pub qualified_buyers: Option<u64>,
    pub diagnostics: Option<TokenDiagnosticsDto>,
}

#[derive(Debug, Serialize)]
pub struct RunSummaryRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub run_id: String,
    /// `complete` or `partial` (operational completion, CLI.md §7).
    pub status: &'static str,
    pub cancelled: bool,
    pub records: usize,
    pub incomplete_reasons: Vec<String>,
    pub tokens: Vec<TokenStatusDto>,
}

fn chain_name(chain: &ChainKey) -> Result<&'static str, String> {
    chain_profile_name(chain).ok_or_else(|| format!("no output profile name for chain {chain:?}"))
}

pub fn wallet_dto(wallet: &WalletKey) -> Result<WalletDto, String> {
    Ok(WalletDto {
        chain: chain_name(&wallet.chain)?,
        address: wallet.address.to_string(),
    })
}

pub fn asset_dto(asset: &AssetKey) -> Result<AssetDto, String> {
    Ok(match asset {
        AssetKey::Token(chain, address) => AssetDto::Token {
            chain: chain_name(chain)?,
            token: address.to_string(),
        },
        AssetKey::Native(chain) => AssetDto::Native {
            chain: chain_name(chain)?,
            native: true,
        },
    })
}

pub fn buyer_match_record(m: &BuyerMatch) -> Result<BuyerMatchRecord, String> {
    Ok(BuyerMatchRecord {
        schema_version: SCHEMA_VERSION,
        kind: "buyer_match",
        wallet: wallet_dto(&m.wallet)?,
        hit_count: m.hit_count,
        matched_assets: m
            .matched_assets
            .iter()
            .map(asset_dto)
            .collect::<Result<_, _>>()?,
    })
}

pub fn run_meta_record(
    run_id: &str,
    captured_at: &str,
    report: &SolanaBuyerIntersectReport,
    input_tokens: &[AssetKey],
    max_pages_per_token: u32,
) -> Result<RunMetaRecord, String> {
    let scope: &SolanaProtocolScope = &report.scope;
    Ok(RunMetaRecord {
        schema_version: SCHEMA_VERSION,
        kind: "run_meta",
        run_id: run_id.to_string(),
        captured_at: captured_at.to_string(),
        scope: ScopeDto {
            chain: "solana",
            program_id: scope.program_id,
            idl_commit: scope.idl_commit,
            idl_sha256: scope.idl_sha256,
            qualification_version: scope.qualification_version,
            recognized: scope.recognized,
            not_decoded: scope.not_decoded,
            variants: SolanaProtocolScope::variants()
                .into_iter()
                .map(|(name, side, verification)| VariantDto {
                    name,
                    side,
                    verification,
                })
                .collect(),
        },
        budget: BudgetDto {
            max_pages_per_token,
        },
        input_tokens: input_tokens
            .iter()
            .map(asset_dto)
            .collect::<Result<_, _>>()?,
        input_token_count: report.base.input_token_count,
        min_token_hits: report.base.min_token_hits,
    })
}

fn unverified_total(d: &TxQualificationDiagnostics) -> u64 {
    (0..PumpTradeVariant::COUNT)
        .map(|i| d.unverified_variant_buys.get(i).copied().unwrap_or(0))
        .fold(0u64, u64::saturating_add)
}

fn token_status(
    token: &SolanaTokenScanSummary,
    redact: &dyn Fn(&str) -> String,
) -> Result<TokenStatusDto, String> {
    let asset = asset_dto(&token.asset)?;
    if let Some(error) = &token.error {
        return Ok(TokenStatusDto {
            token: asset,
            status: "failed",
            error: Some(redact(error)),
            transactions_scanned: None,
            qualified_buyers: None,
            diagnostics: None,
        });
    }
    let d = &token.diagnostics;
    Ok(TokenStatusDto {
        token: asset,
        status: if token.truncated { "truncated" } else { "ok" },
        error: None,
        transactions_scanned: Some(token.transactions_scanned),
        qualified_buyers: Some(token.qualified_buyers),
        diagnostics: Some(TokenDiagnosticsDto {
            decoded_buys: d.decoded_buys,
            malformed_instructions: d.malformed_instructions,
            unknown_discriminator_instructions: d.unknown_discriminator_instructions,
            unverified_variant_buys: unverified_total(d),
            failed_transactions: d.failed_transactions,
            positive_delta_without_instruction: token.positive_delta_without_instruction,
        }),
    })
}

pub fn run_summary_record(
    run_id: &str,
    report: &SolanaBuyerIntersectReport,
    incomplete: bool,
    redact: &dyn Fn(&str) -> String,
) -> Result<RunSummaryRecord, String> {
    Ok(RunSummaryRecord {
        schema_version: SCHEMA_VERSION,
        kind: "run_summary",
        run_id: run_id.to_string(),
        status: if incomplete { "partial" } else { "complete" },
        cancelled: report.cancelled,
        records: report.base.matches.len(),
        incomplete_reasons: report
            .incomplete_reasons()
            .iter()
            .map(|r| redact(r))
            .collect(),
        tokens: report
            .per_token
            .iter()
            .map(|t| token_status(t, redact))
            .collect::<Result<_, _>>()?,
    })
}

/// All JSONL lines for a Solana run: `run_meta`, `buyer_match`*,
/// `run_summary`.
pub fn solana_jsonl_lines(
    run_id: &str,
    captured_at: &str,
    report: &SolanaBuyerIntersectReport,
    input_tokens: &[AssetKey],
    max_pages_per_token: u32,
    incomplete: bool,
    redact: &dyn Fn(&str) -> String,
) -> Result<Vec<String>, String> {
    let ser = |r: Result<String, serde_json::Error>| r.map_err(|e| e.to_string());
    let mut lines = Vec::with_capacity(report.base.matches.len() + 2);
    lines.push(ser(serde_json::to_string(&run_meta_record(
        run_id,
        captured_at,
        report,
        input_tokens,
        max_pages_per_token,
    )?))?);
    for m in &report.base.matches {
        lines.push(ser(serde_json::to_string(&buyer_match_record(m)?))?);
    }
    lines.push(ser(serde_json::to_string(&run_summary_record(
        run_id, report, incomplete, redact,
    )?))?);
    Ok(lines)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;
    use scout_core::AddressBytes;
    use scout_engine::{BuyerIntersectReport, solana_mainnet_chain};
    use serde_json::Value;

    const MINT_A: &str = "AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump";
    const MINT_B: &str = "NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump";

    fn mint(text: &str) -> AssetKey {
        let bytes: [u8; 32] = bs58::decode(text).into_vec().unwrap().try_into().unwrap();
        AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(bytes))
    }

    fn token(asset: AssetKey, failed: bool, truncated: bool) -> SolanaTokenScanSummary {
        SolanaTokenScanSummary {
            asset,
            transactions_scanned: 250,
            truncated,
            error: failed.then(|| "scan failed at https://h/?api-key=SECRET99".to_string()),
            qualified_buyers: 3,
            diagnostics: TxQualificationDiagnostics::default(),
            positive_delta_without_instruction: 0,
            unexpected_payloads: 0,
        }
    }

    fn report(failed: bool) -> SolanaBuyerIntersectReport {
        let (a, b) = (mint(MINT_A), mint(MINT_B));
        let wallet = WalletKey {
            chain: solana_mainnet_chain(),
            address: AddressBytes::Solana([7; 32]),
        };
        SolanaBuyerIntersectReport {
            base: BuyerIntersectReport {
                matches: vec![BuyerMatch {
                    wallet,
                    hit_count: 2,
                    matched_assets: vec![a.clone(), b.clone()],
                }],
                input_token_count: 2,
                min_token_hits: 2,
                coverage_truncated: false,
            },
            scope: SolanaProtocolScope::pump_bonding_curve(),
            per_token: vec![token(a, false, false), token(b, failed, !failed)],
            diagnostics: TxQualificationDiagnostics::default(),
            positive_delta_without_instruction: 0,
            unexpected_payloads: 0,
            malformed_samples: vec![],
            unknown_discriminator_samples: vec![],
            cancelled: false,
        }
    }

    fn lines(failed: bool) -> Vec<Value> {
        let r = report(failed);
        let tokens = vec![mint(MINT_A), mint(MINT_B)];
        let incomplete = r.is_coverage_incomplete();
        solana_jsonl_lines(
            "run-1",
            "2026-10-02T12:34:56Z",
            &r,
            &tokens,
            25,
            incomplete,
            &|t| t.replace("SECRET99", "<redacted>"),
        )
        .unwrap()
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
    }

    #[test]
    fn record_order_and_wallet_shape() {
        let v = lines(false);
        let kinds: Vec<&str> = v.iter().map(|r| r["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["run_meta", "buyer_match", "run_summary"]);
        assert!(v.iter().all(|r| r["schema_version"] == 1));
        let m = &v[1];
        assert_eq!(m["wallet"]["chain"], "solana");
        assert_eq!(
            m["wallet"]["address"].as_str().unwrap(),
            bs58::encode([7u8; 32]).into_string()
        );
        assert_eq!(m["hit_count"], 2);
        assert_eq!(m["matched_assets"][0]["chain"], "solana");
        assert_eq!(m["matched_assets"][0]["token"], MINT_A);
        assert_eq!(m["matched_assets"][1]["token"], MINT_B);
    }

    #[test]
    fn run_meta_carries_scope_budget_and_inputs() {
        let v = lines(false);
        let meta = &v[0];
        println!("{meta}");
        assert_eq!(meta["run_id"], "run-1");
        assert_eq!(meta["captured_at"], "2026-10-02T12:34:56Z");
        assert_eq!(meta["budget"]["max_pages_per_token"], 25);
        assert_eq!(meta["input_token_count"], 2);
        assert_eq!(meta["min_token_hits"], 2);
        assert_eq!(meta["input_tokens"][0]["token"], MINT_A);
        let scope = &meta["scope"];
        assert_eq!(scope["chain"], "solana");
        for key in [
            "program_id",
            "idl_commit",
            "idl_sha256",
            "qualification_version",
        ] {
            assert!(scope[key].as_str().is_some_and(|s| !s.is_empty()), "{key}");
        }
        assert!(!scope["variants"].as_array().unwrap().is_empty());
    }

    #[test]
    fn summary_marks_truncated_token_partial_with_counts() {
        let v = lines(false);
        println!("{}", v[1]);
        let s = &v[2];
        println!("{s}");
        assert_eq!(s["status"], "partial");
        assert_eq!(s["records"], 1);
        assert_eq!(s["tokens"][0]["status"], "ok");
        assert_eq!(s["tokens"][1]["status"], "truncated");
        assert_eq!(s["tokens"][1]["transactions_scanned"], 250);
        assert!(!s["incomplete_reasons"].as_array().unwrap().is_empty());
    }

    #[test]
    fn failed_token_counts_are_null_and_error_redacted() {
        let v = lines(true);
        let t = &v[2]["tokens"][1];
        assert_eq!(t["status"], "failed");
        assert!(t["transactions_scanned"].is_null());
        assert!(t["qualified_buyers"].is_null());
        assert!(t["diagnostics"].is_null());
        let text = v[2].to_string();
        assert!(!text.contains("SECRET99"));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn evm_wallet_is_0x_hex_with_profile_name() {
        let wallet = WalletKey {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            address: AddressBytes::Evm([0x11; 20]),
        };
        let dto = serde_json::to_value(wallet_dto(&wallet).unwrap()).unwrap();
        assert_eq!(dto["chain"], "base");
        assert_eq!(dto["address"], "0x1111111111111111111111111111111111111111");
    }
}
