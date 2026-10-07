//! pump.fun lifecycle events for the dev tracker (B2): `CreateEvent` (a
//! launch) and `CompletePumpAmmMigrationEvent` (a graduation to PumpSwap).
//!
//! Same event-CPI framing as `TradeEvent` (`EVENT_CPI_DISCRIMINATOR (8) ++
//! event discriminator (8) ++ Borsh event`, self-invocation of the pump.fun
//! program). Layouts from the pinned IDL (`pump_idl_e0687ae9.json`,
//! `types.CreateEvent` / `types.CompletePumpAmmMigrationEvent`); only the
//! leading fields the tracker stores are decoded, later fields are ignored.
//! `CreateEvent` starts with three Borsh strings (`name`, `symbol`, `uri`)
//! which are skipped by length (bounded, never interpreted).

use scout_core::{RawSolanaInstruction, SolanaPubkey};

use crate::bonding_curve_buy::EVENT_CPI_DISCRIMINATOR;
use crate::pump_accounts::PUMP_PROGRAM_ID_BYTES;
use crate::trade_event::EVENT_CPI_HEADER_LEN;

/// IDL `events.CreateEvent.discriminator`.
pub const CREATE_EVENT_DISCRIMINATOR: [u8; 8] = [27, 114, 169, 77, 222, 235, 99, 118];
/// IDL `events.CompletePumpAmmMigrationEvent.discriminator`.
pub const COMPLETE_PUMP_AMM_MIGRATION_EVENT_DISCRIMINATOR: [u8; 8] =
    [189, 233, 93, 185, 92, 148, 234, 148];
/// Upper bound on each skipped `CreateEvent` string (name / symbol / uri).
pub const MAX_CREATE_STRING_BYTES: usize = 1024;

/// Decoded `CreateEvent` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpCreate {
    pub mint: SolanaPubkey,
    pub bonding_curve: SolanaPubkey,
    /// The transaction signer that called `create` / `create_v2`.
    pub user: SolanaPubkey,
    /// The coin creator (creator-fee recipient); the dev.
    pub creator: SolanaPubkey,
    pub timestamp: i64,
}

/// Decoded `CompletePumpAmmMigrationEvent` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpMigration {
    pub user: SolanaPubkey,
    pub mint: SolanaPubkey,
    pub bonding_curve: SolanaPubkey,
    pub timestamp: i64,
    pub pool: SolanaPubkey,
}

/// A lifecycle event of one instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpLifecycleEvent {
    Create(PumpCreate),
    Migration(PumpMigration),
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize, field: &'static str) -> Result<&'a [u8], String> {
        let s = self
            .pos
            .checked_add(n)
            .and_then(|e| self.buf.get(self.pos..e))
            .ok_or_else(|| format!("event ends inside field `{field}`"))?;
        self.pos += n;
        Ok(s)
    }

    fn pubkey(&mut self, field: &'static str) -> Result<SolanaPubkey, String> {
        self.take(32, field)?
            .try_into()
            .map_err(|_| format!("`{field}` not 32 bytes"))
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

    fn skip_string(&mut self, field: &'static str) -> Result<(), String> {
        let a: [u8; 4] = self
            .take(4, field)?
            .try_into()
            .map_err(|_| format!("`{field}` length not 4 bytes"))?;
        let len = usize::try_from(u32::from_le_bytes(a))
            .map_err(|_| format!("`{field}` length does not fit usize"))?;
        if len > MAX_CREATE_STRING_BYTES {
            return Err(format!(
                "`{field}` length {len} exceeds the {MAX_CREATE_STRING_BYTES}-byte bound"
            ));
        }
        self.take(len, field).map(|_| ())
    }
}

fn decode_create(payload: &[u8]) -> Result<PumpCreate, String> {
    let mut c = Cursor {
        buf: payload,
        pos: 0,
    };
    c.skip_string("name")?;
    c.skip_string("symbol")?;
    c.skip_string("uri")?;
    Ok(PumpCreate {
        mint: c.pubkey("mint")?,
        bonding_curve: c.pubkey("bonding_curve")?,
        user: c.pubkey("user")?,
        creator: c.pubkey("creator")?,
        timestamp: c.i64("timestamp")?,
    })
}

fn decode_migration(payload: &[u8]) -> Result<PumpMigration, String> {
    let mut c = Cursor {
        buf: payload,
        pos: 0,
    };
    let user = c.pubkey("user")?;
    let mint = c.pubkey("mint")?;
    c.u64("mint_amount")?;
    c.u64("sol_amount")?;
    c.u64("pool_migration_fee")?;
    let bonding_curve = c.pubkey("bonding_curve")?;
    let timestamp = c.i64("timestamp")?;
    let pool = c.pubkey("pool")?;
    Ok(PumpMigration {
        user,
        mint,
        bonding_curve,
        timestamp,
        pool,
    })
}

/// Decode a pump.fun lifecycle event from one instruction: `Ok(None)` for
/// anything else (another program, not an event-CPI, another event),
/// `Err` for a lifecycle event with a broken layout (a coverage gap the
/// caller counts, never guessed).
///
/// # Errors
/// A `CreateEvent` / `CompletePumpAmmMigrationEvent` that does not decode.
pub fn decode_pump_lifecycle(
    instruction: &RawSolanaInstruction,
) -> Result<Option<PumpLifecycleEvent>, String> {
    if instruction.program_id != PUMP_PROGRAM_ID_BYTES {
        return Ok(None);
    }
    let data = &instruction.data;
    if data.get(0..8) != Some(EVENT_CPI_DISCRIMINATOR.as_slice()) {
        return Ok(None);
    }
    let Some(disc) = data.get(8..EVENT_CPI_HEADER_LEN) else {
        return Ok(None);
    };
    let payload = data.get(EVENT_CPI_HEADER_LEN..).unwrap_or_default();
    if disc == CREATE_EVENT_DISCRIMINATOR {
        decode_create(payload).map(|e| Some(PumpLifecycleEvent::Create(e)))
    } else if disc == COMPLETE_PUMP_AMM_MIGRATION_EVENT_DISCRIMINATOR {
        decode_migration(payload).map(|e| Some(PumpLifecycleEvent::Migration(e)))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix(event: [u8; 8], payload: &[u8]) -> RawSolanaInstruction {
        let mut data = EVENT_CPI_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&event);
        data.extend_from_slice(payload);
        RawSolanaInstruction {
            program_id: PUMP_PROGRAM_ID_BYTES,
            accounts: vec![],
            data,
            instruction_index: 3,
        }
    }

    fn string(s: &str) -> Vec<u8> {
        let mut v = u32::try_from(s.len()).unwrap().to_le_bytes().to_vec();
        v.extend_from_slice(s.as_bytes());
        v
    }

    #[test]
    fn discriminators_match_the_pinned_idl() {
        let idl: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/p0/measurements/fixtures/pump_idl_e0687ae9.json"
        ))
        .unwrap();
        let disc = |name: &str| -> Vec<u8> {
            idl["events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["name"] == name)
                .unwrap()["discriminator"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
                .collect()
        };
        assert_eq!(disc("CreateEvent"), CREATE_EVENT_DISCRIMINATOR);
        assert_eq!(
            disc("CompletePumpAmmMigrationEvent"),
            COMPLETE_PUMP_AMM_MIGRATION_EVENT_DISCRIMINATOR
        );
        // field order the decoders rely on
        let fields = |name: &str| -> Vec<String> {
            idl["types"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == name)
                .unwrap()["type"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| format!("{}:{}", f["name"].as_str().unwrap(), f["type"]))
                .collect()
        };
        assert_eq!(
            fields("CreateEvent")[..8],
            [
                "name:\"string\"",
                "symbol:\"string\"",
                "uri:\"string\"",
                "mint:\"pubkey\"",
                "bonding_curve:\"pubkey\"",
                "user:\"pubkey\"",
                "creator:\"pubkey\"",
                "timestamp:\"i64\""
            ]
        );
        assert_eq!(
            fields("CompletePumpAmmMigrationEvent")[..8],
            [
                "user:\"pubkey\"",
                "mint:\"pubkey\"",
                "mint_amount:\"u64\"",
                "sol_amount:\"u64\"",
                "pool_migration_fee:\"u64\"",
                "bonding_curve:\"pubkey\"",
                "timestamp:\"i64\"",
                "pool:\"pubkey\""
            ]
        );
    }

    #[test]
    fn create_event_skips_strings_and_reads_the_creator() {
        let mut p = string("Name");
        p.extend(string("SYM"));
        p.extend(string("https://ipfs.io/x"));
        for b in [1u8, 2, 3, 4] {
            p.extend([b; 32]);
        }
        p.extend(1_760_000_000i64.to_le_bytes());
        p.extend([9u8; 40]); // later fields, ignored
        let ev = decode_pump_lifecycle(&ix(CREATE_EVENT_DISCRIMINATOR, &p)).unwrap();
        assert_eq!(
            ev,
            Some(PumpLifecycleEvent::Create(PumpCreate {
                mint: [1; 32],
                bonding_curve: [2; 32],
                user: [3; 32],
                creator: [4; 32],
                timestamp: 1_760_000_000,
            }))
        );
    }

    #[test]
    fn migration_event_reads_mint_and_pool() {
        let mut p = [5u8; 32].to_vec();
        p.extend([6u8; 32]);
        p.extend([0u8; 24]);
        p.extend([7u8; 32]);
        p.extend(42i64.to_le_bytes());
        p.extend([8u8; 32]);
        p.extend([0u8; 32]); // quote_mint
        let ev = decode_pump_lifecycle(&ix(COMPLETE_PUMP_AMM_MIGRATION_EVENT_DISCRIMINATOR, &p))
            .unwrap();
        assert_eq!(
            ev,
            Some(PumpLifecycleEvent::Migration(PumpMigration {
                user: [5; 32],
                mint: [6; 32],
                bonding_curve: [7; 32],
                timestamp: 42,
                pool: [8; 32],
            }))
        );
    }

    #[test]
    fn other_programs_events_and_broken_layouts() {
        let mut other = ix(CREATE_EVENT_DISCRIMINATOR, &[]);
        other.program_id = [0; 32];
        assert_eq!(decode_pump_lifecycle(&other), Ok(None));
        assert_eq!(
            decode_pump_lifecycle(&ix([1; 8], &[0; 300])),
            Ok(None),
            "a trade or any other event is not a lifecycle event"
        );
        assert!(decode_pump_lifecycle(&ix(CREATE_EVENT_DISCRIMINATOR, &string("x"))).is_err());
        let huge = u32::MAX.to_le_bytes();
        assert!(decode_pump_lifecycle(&ix(CREATE_EVENT_DISCRIMINATOR, &huge)).is_err());
    }
}
