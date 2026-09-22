//! Input parsing for the three CLI binaries: `lines`, `csv`, `jsonl`.
//! See workspace docs/CLI.md §1-2 for the full contract.
//!
//! Closes ACCEPTANCE A01-A05 entirely offline: no network call happens
//! during parsing, so ambiguous-chain detection, duplicate canonicalization,
//! and invalid-address rejection are all testable without credentials.

use std::collections::BTreeMap;
use std::io::BufRead;

use scout_core::{AddressBytes, ChainFamily, ChainKey, ChainResolution};

/// Which structured format the input is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    Lines,
    Csv,
    Jsonl,
    /// Detect from content; per CLI.md this must pick one of the
    /// documented formats deterministically, never do implicit symbol
    /// resolution.
    Auto,
}

/// What kind of identity a parsed line represents. Token vs wallet
/// disambiguation happens at the CLI-argument level (which binary is
/// running), not here — this layer only produces canonical addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityKind {
    /// Chain explicitly given on the line (e.g. `solana:<addr>`).
    Explicit(ChainTag),
    /// Bare address, chain to be resolved by caller (single-network input
    /// or `--evm-scope all` fan-out).
    Bare,
}

/// Textual chain tag as it appears in `lines`/`csv` input, before
/// resolution to a verified `ChainKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChainTag {
    Solana,
    Bsc,
    Base,
    Robinhood,
}

impl ChainTag {
    fn parse(s: &str) -> Option<ChainTag> {
        match s {
            "solana" => Some(ChainTag::Solana),
            "bsc" => Some(ChainTag::Bsc),
            "base" => Some(ChainTag::Base),
            "robinhood" => Some(ChainTag::Robinhood),
            _ => None,
        }
    }

    #[must_use]
    pub fn family(&self) -> ChainFamily {
        match self {
            ChainTag::Solana => ChainFamily::Solana,
            ChainTag::Bsc | ChainTag::Base | ChainTag::Robinhood => ChainFamily::Evm,
        }
    }
}

/// One canonicalized identity from the input, before any network call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityRecord {
    pub line_number: usize,
    pub kind: IdentityKind,
    pub address_text: String,
    pub canonical: AddressBytes,
}

/// Result of parsing an entire input source.
#[derive(Debug, Clone)]
pub struct ParsedInput {
    pub records: Vec<IdentityRecord>,
    /// Count of input lines that canonicalized to an identity already
    /// seen (CLI.md §1: "Повторные canonical identities дедуплицируются,
    /// count duplicates выводится в stderr/manifest").
    pub duplicate_count: usize,
}

/// A parse failure. Always carries a 1-indexed line number so the caller
/// can report "line N: reason" before any paid scanning starts
/// (ACCEPTANCE A04).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InputError {
    #[error("line {line}: invalid address: {reason}")]
    InvalidAddress { line: usize, reason: String },

    #[error("line {line}: unknown chain tag `{tag}`")]
    UnknownChainTag { line: usize, tag: String },

    #[error(
        "line {line}: chain conflict: line specifies `{line_chain}`, but --chain is `{cli_chain}`"
    )]
    ChainConflict {
        line: usize,
        line_chain: String,
        cli_chain: String,
    },

    #[error("empty input: no identities found")]
    EmptyInput,
}

/// Parse `lines`-format input: one `chain:address` or bare `address` per
/// line. Blank lines and lines starting with `#` are comments and are
/// skipped (CLI.md §1: "Комментарии/пустые строки разрешены в lines").
///
/// `cli_chain`, when given, is the `--chain` flag value; any per-line
/// chain tag that conflicts with it is a hard error (CLI.md §1: "Конфликт
/// per-line chain и `--chain` — ошибка, а не silent override").
pub fn parse_input<R: BufRead>(
    reader: R,
    format: InputFormat,
    cli_chain: Option<ChainTag>,
) -> Result<ParsedInput, InputError> {
    match format {
        InputFormat::Lines | InputFormat::Auto => parse_lines(reader, cli_chain),
        InputFormat::Csv => parse_csv(reader, cli_chain),
        InputFormat::Jsonl => parse_jsonl(reader, cli_chain),
    }
}

fn parse_lines<R: BufRead>(
    reader: R,
    cli_chain: Option<ChainTag>,
) -> Result<ParsedInput, InputError> {
    let mut records = Vec::new();
    // Keyed by canonical bytes, not display text, so different textual
    // forms of the same address dedupe correctly (ACCEPTANCE A04).
    let mut seen: BTreeMap<AddressBytes, usize> = BTreeMap::new();
    let mut duplicate_count = 0usize;

    for (idx, line_result) in reader.lines().enumerate() {
        let line_number = idx + 1;
        let raw = line_result.map_err(|_| InputError::InvalidAddress {
            line: line_number,
            reason: "could not read line".to_string(),
        })?;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let (kind, address_text) = split_chain_prefix(trimmed, line_number, cli_chain)?;
        let family = match &kind {
            IdentityKind::Explicit(tag) => tag.family(),
            IdentityKind::Bare => {
                // Family for a bare address is inferred from address shape
                // (EVM: 0x-hex; Solana: base58) — chain resolution proper
                // (which network within that family) is the caller's job,
                // not this parser's (ADR-002/CLI.md §1).
                infer_family_from_shape(address_text, line_number)?
            }
        };

        let canonical = canonicalize_address(address_text, family, line_number)?;

        if let Some(&first_line) = seen.get(&canonical) {
            duplicate_count += 1;
            let _ = first_line; // first occurrence retained; duplicates just counted
            continue;
        }
        seen.insert(canonical.clone(), line_number);

        records.push(IdentityRecord {
            line_number,
            kind,
            address_text: address_text.to_string(),
            canonical,
        });
    }

    if records.is_empty() {
        return Err(InputError::EmptyInput);
    }

    Ok(ParsedInput {
        records,
        duplicate_count,
    })
}

fn split_chain_prefix(
    line: &str,
    line_number: usize,
    cli_chain: Option<ChainTag>,
) -> Result<(IdentityKind, &str), InputError> {
    if let Some((prefix, rest)) = line.split_once(':') {
        if let Some(tag) = ChainTag::parse(prefix) {
            if let Some(cli) = cli_chain
                && cli != tag
            {
                return Err(InputError::ChainConflict {
                    line: line_number,
                    line_chain: prefix.to_string(),
                    cli_chain: format!("{cli:?}"),
                });
            }
            return Ok((IdentityKind::Explicit(tag), rest));
        }
        // Not a recognized chain tag but contains a colon — for EVM
        // addresses this cannot happen (no colon in 0x-hex), so this is
        // a genuine unknown tag, not a false positive on address syntax.
        return Err(InputError::UnknownChainTag {
            line: line_number,
            tag: prefix.to_string(),
        });
    }
    Ok((IdentityKind::Bare, line))
}

fn infer_family_from_shape(
    address_text: &str,
    line_number: usize,
) -> Result<ChainFamily, InputError> {
    if address_text.starts_with("0x") || address_text.starts_with("0X") {
        Ok(ChainFamily::Evm)
    } else if bs58::decode(address_text).into_vec().is_ok() {
        Ok(ChainFamily::Solana)
    } else {
        Err(InputError::InvalidAddress {
            line: line_number,
            reason: "could not infer chain family from address shape".to_string(),
        })
    }
}

fn canonicalize_address(
    address_text: &str,
    family: ChainFamily,
    line_number: usize,
) -> Result<AddressBytes, InputError> {
    match family {
        ChainFamily::Evm => {
            let hex_part = address_text
                .strip_prefix("0x")
                .or_else(|| address_text.strip_prefix("0X"))
                .ok_or_else(|| InputError::InvalidAddress {
                    line: line_number,
                    reason: "EVM address must start with 0x".to_string(),
                })?;
            let bytes = hex_decode(hex_part).ok_or_else(|| InputError::InvalidAddress {
                line: line_number,
                reason: "invalid hex in EVM address".to_string(),
            })?;
            let array: [u8; 20] =
                bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| InputError::InvalidAddress {
                        line: line_number,
                        reason: format!("EVM address must be 20 bytes, got {}", bytes.len()),
                    })?;
            Ok(AddressBytes::Evm(array))
        }
        ChainFamily::Solana => {
            let bytes =
                bs58::decode(address_text)
                    .into_vec()
                    .map_err(|_| InputError::InvalidAddress {
                        line: line_number,
                        reason: "invalid base58 in Solana address".to_string(),
                    })?;
            let array: [u8; 32] =
                bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| InputError::InvalidAddress {
                        line: line_number,
                        reason: format!("Solana address must be 32 bytes, got {}", bytes.len()),
                    })?;
            Ok(AddressBytes::Solana(array))
        }
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len().div_euclid(2));
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_nibble(bytes.get(i).copied()?)?;
        let lo = hex_nibble(bytes.get(i + 1).copied()?)?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_csv<R: BufRead>(
    reader: R,
    cli_chain: Option<ChainTag>,
) -> Result<ParsedInput, InputError> {
    // CLI.md §1: CSV headers `chain,address`. Reuse the same line-based
    // canonicalization logic by translating each data row to the
    // `chain:address` textual form the lines-parser already handles,
    // keeping one source of truth for canonicalization.
    let mut translated = String::new();
    let mut lines_iter = reader.lines();
    let header = lines_iter.next();
    match header {
        Some(Ok(h)) if h.trim() == "chain,address" => {}
        Some(Ok(_)) | None => {
            return Err(InputError::InvalidAddress {
                line: 1,
                reason: "CSV must start with header `chain,address`".to_string(),
            });
        }
        Some(Err(_)) => {
            return Err(InputError::InvalidAddress {
                line: 1,
                reason: "could not read CSV header".to_string(),
            });
        }
    }
    for line_result in lines_iter {
        let line = line_result.map_err(|_| InputError::InvalidAddress {
            line: 0,
            reason: "could not read CSV row".to_string(),
        })?;
        if line.trim().is_empty() {
            continue;
        }
        // Translate the CSV `chain,address` row into the `chain:address`
        // textual form the lines-parser already handles, so both formats
        // share one canonicalization implementation (CLI.md §1: "Lines/
        // CSV/JSONL дают одинаковый список identities").
        let Some((chain, address)) = line.split_once(',') else {
            return Err(InputError::InvalidAddress {
                line: 0,
                reason: "CSV row must be `chain,address`".to_string(),
            });
        };
        translated.push_str(chain);
        translated.push(':');
        translated.push_str(address);
        translated.push('\n');
    }
    parse_lines(translated.as_bytes(), cli_chain).map_err(shift_csv_line_number)
}

fn shift_csv_line_number(err: InputError) -> InputError {
    // CSV data rows are offset by the header line; report 1-indexed CSV
    // line numbers (header = line 1) rather than the translated buffer's
    // own line numbers.
    match err {
        InputError::InvalidAddress { line, reason } => InputError::InvalidAddress {
            line: line + 1,
            reason,
        },
        InputError::UnknownChainTag { line, tag } => InputError::UnknownChainTag {
            line: line + 1,
            tag,
        },
        InputError::ChainConflict {
            line,
            line_chain,
            cli_chain,
        } => InputError::ChainConflict {
            line: line + 1,
            line_chain,
            cli_chain,
        },
        other => other,
    }
}

fn parse_jsonl<R: BufRead>(
    _reader: R,
    _cli_chain: Option<ChainTag>,
) -> Result<ParsedInput, InputError> {
    // JSONL adapter parses prior CLI output records (run_meta/wallet_ref/
    // buyer_match/...), not raw addresses — deferred to when the JSONL
    // envelope schema (CLI.md §7) is implemented alongside output
    // formatting, so both sides share one schema definition.
    Err(InputError::InvalidAddress {
        line: 0,
        reason: "jsonl input format not yet implemented".to_string(),
    })
}

/// Resolve a bare address's chain against the set of enabled EVM chains.
/// Never picks the first responder (ACCEPTANCE A01/A02).
#[must_use]
pub fn resolve_chain(_address: &AddressBytes, enabled_chains: &[ChainKey]) -> ChainResolution {
    // Placeholder resolution policy: with zero live capability data (no
    // credentials configured yet, per ADR-006), we cannot claim to have
    // checked which chains actually have this address active. Returning
    // `Ambiguous` for any bare EVM address against >1 enabled EVM chain
    // is the only honest answer available offline; a real capability
    // check (P2+) narrows this using live/fixture-verified deployment
    // data instead of guessing.
    let evm_candidates: Vec<ChainKey> = enabled_chains
        .iter()
        .filter(|c| c.family == ChainFamily::Evm)
        .cloned()
        .collect();
    match evm_candidates.as_slice() {
        [] => ChainResolution::NotFoundOrUnobserved,
        [single] => ChainResolution::Resolved(single.clone()),
        _ => ChainResolution::Ambiguous(evm_candidates),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_chain() -> ChainKey {
        ChainKey {
            family: ChainFamily::Evm,
            network_id: scout_core::NetworkId::EvmChainId(8453),
            genesis_identity: scout_core::GenesisIdentity::Verified("base-genesis".to_string()),
        }
    }

    fn bsc_chain() -> ChainKey {
        ChainKey {
            family: ChainFamily::Evm,
            network_id: scout_core::NetworkId::EvmChainId(56),
            genesis_identity: scout_core::GenesisIdentity::Verified("bsc-genesis".to_string()),
        }
    }

    #[test]
    fn explicit_chain_prefix_resolves_unambiguously() {
        // ACCEPTANCE A01: explicit chain snaps ambiguity away.
        let input = "base:0x1111111111111111111111111111111111111111\n";
        let parsed = parse_input(input.as_bytes(), InputFormat::Lines, None).unwrap();
        assert_eq!(parsed.records.len(), 1);
        assert_eq!(
            parsed.records[0].kind,
            IdentityKind::Explicit(ChainTag::Base)
        );
    }

    #[test]
    fn bare_evm_wallet_without_chain_stays_bare_not_auto_resolved() {
        // ACCEPTANCE A02: bare EVM wallet must not have a chain silently
        // picked by this parsing layer.
        let input = "0x1111111111111111111111111111111111111111\n";
        let parsed = parse_input(input.as_bytes(), InputFormat::Lines, None).unwrap();
        assert_eq!(parsed.records[0].kind, IdentityKind::Bare);
    }

    #[test]
    fn resolve_chain_returns_ambiguous_for_multiple_evm_candidates() {
        // ACCEPTANCE A01/A02: never pick the first responder.
        let addr = AddressBytes::Evm([0x11; 20]);
        let resolution = resolve_chain(&addr, &[base_chain(), bsc_chain()]);
        match resolution {
            ChainResolution::Ambiguous(candidates) => assert_eq!(candidates.len(), 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn conflicting_per_line_chain_and_cli_chain_is_an_error() {
        // CLI.md §1: conflict is a hard error, not silent override.
        let input = "bsc:0x1111111111111111111111111111111111111111\n";
        let result = parse_input(input.as_bytes(), InputFormat::Lines, Some(ChainTag::Base));
        assert!(matches!(result, Err(InputError::ChainConflict { .. })));
    }

    #[test]
    fn duplicate_textual_forms_of_same_address_collapse_to_one_identity() {
        // ACCEPTANCE A04: same canonical identity in different textual
        // forms dedupes to one record; duplicates are counted.
        let input = "0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n\
                      0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n";
        let parsed = parse_input(input.as_bytes(), InputFormat::Lines, None).unwrap();
        assert_eq!(parsed.records.len(), 1);
        assert_eq!(parsed.duplicate_count, 1);
    }

    #[test]
    fn invalid_address_reports_line_number_before_any_scanning() {
        // ACCEPTANCE A04: invalid line -> error with line number.
        let input = "0x1111111111111111111111111111111111111111\n\
                      0xnotvalidhex\n";
        let result = parse_input(input.as_bytes(), InputFormat::Lines, None);
        match result {
            Err(InputError::InvalidAddress { line, .. }) => assert_eq!(line, 2),
            other => panic!("expected InvalidAddress at line 2, got {other:?}"),
        }
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let input = "# comment\n\n0x1111111111111111111111111111111111111111\n";
        let parsed = parse_input(input.as_bytes(), InputFormat::Lines, None).unwrap();
        assert_eq!(parsed.records.len(), 1);
    }

    #[test]
    fn empty_input_is_an_explicit_error_not_silent_empty_success() {
        let result = parse_input("".as_bytes(), InputFormat::Lines, None);
        assert!(matches!(result, Err(InputError::EmptyInput)));
    }

    #[test]
    fn lines_and_csv_give_same_canonical_identity_for_same_address() {
        // CLI.md §1: "Lines/CSV/JSONL дают одинаковый список identities."
        let lines_input = "base:0x1111111111111111111111111111111111111111\n";
        let csv_input = "chain,address\nbase,0x1111111111111111111111111111111111111111\n";
        let from_lines = parse_input(lines_input.as_bytes(), InputFormat::Lines, None).unwrap();
        let from_csv = parse_input(csv_input.as_bytes(), InputFormat::Csv, None).unwrap();
        assert_eq!(
            from_lines.records[0].canonical,
            from_csv.records[0].canonical
        );
    }

    #[test]
    fn solana_address_canonicalizes_via_base58() {
        // A valid 32-byte base58 Solana address (System Program id).
        let input = "solana:11111111111111111111111111111111\n";
        let parsed = parse_input(input.as_bytes(), InputFormat::Lines, None).unwrap();
        assert_eq!(parsed.records.len(), 1);
        assert!(matches!(
            parsed.records[0].canonical,
            AddressBytes::Solana(_)
        ));
    }
}
