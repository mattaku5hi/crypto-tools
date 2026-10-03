//! On-chain ACCOUNT decoders needed to value an open position (ADR-019):
//! the pump.fun `BondingCurve` account, the PumpSwap `Pool` account and the
//! SPL token-account balance of a pool vault. Layouts come from the pinned
//! IDLs (`e0687ae9`); every decoder is gated on the 8-byte Anchor account
//! discriminator AND on the owning program, and refuses short data with a
//! typed error. Accounts written by older program versions are SHORTER (the
//! IDL fields were appended over time): the fields up to `creator` /
//! `coin_creator` are required, later fields are `Option` (absent = `None`,
//! never a guessed zero), except `Pool::virtual_quote_reserves` whose IDL
//! doc says "for non-boost pools, value is 0": an account without that field
//! is a legacy pool, so it is exposed as `0` AND flagged by
//! `virtual_quote_reserves_present = false`. Bytes after the known layout are
//! counted in `trailing_bytes` (appended fields), not interpreted.

use scout_core::SolanaPubkey;

use crate::pump_amm::PUMP_AMM_PROGRAM_ID_BYTES;

/// pump.fun program id (`6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`).
pub const PUMP_PROGRAM_ID_BYTES: SolanaPubkey = [
    1, 86, 224, 246, 147, 102, 90, 207, 68, 219, 21, 104, 191, 23, 91, 170, 81, 137, 203, 151, 245,
    210, 255, 59, 101, 93, 43, 182, 253, 109, 24, 176,
];
/// SPL Token program.
pub const SPL_TOKEN_PROGRAM_ID_BYTES: SolanaPubkey = [
    6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172, 28, 180, 133, 237,
    95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
];
/// SPL Token-2022 program.
pub const SPL_TOKEN_2022_PROGRAM_ID_BYTES: SolanaPubkey = [
    6, 221, 246, 225, 238, 117, 143, 222, 24, 66, 93, 188, 228, 108, 205, 218, 182, 26, 252, 77,
    131, 185, 13, 39, 254, 189, 249, 40, 216, 161, 139, 252,
];

/// Anchor account discriminator of `BondingCurve`.
pub const BONDING_CURVE_ACCOUNT_DISCRIMINATOR: [u8; 8] = [23, 183, 248, 55, 96, 216, 172, 96];
/// Anchor account discriminator of PumpSwap `Pool`.
pub const POOL_ACCOUNT_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

/// Bytes of a `BondingCurve` up to and including `creator` (required).
pub const BONDING_CURVE_REQUIRED_LEN: usize = 8 + 5 * 8 + 1 + 32;
/// Bytes of a `Pool` up to and including `coin_creator` (required).
pub const POOL_REQUIRED_LEN: usize = 8 + 1 + 2 + 6 * 32 + 8 + 32;
/// Length of the SPL token account base layout (also the Token-2022 base).
pub const TOKEN_ACCOUNT_BASE_LEN: usize = 165;

/// Why an account was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccountDecodeError {
    #[error("account owner is not the expected program")]
    WrongOwner,
    #[error("account data shorter than the required layout ({got} < {need})")]
    TooShort { got: usize, need: usize },
    #[error("account discriminator mismatch")]
    WrongDiscriminator,
    #[error("boolean field holds a byte other than 0/1")]
    InvalidBool,
    #[error("token account is not initialized")]
    TokenAccountUninitialized,
}

struct Cur<'a> {
    d: &'a [u8],
    at: usize,
}

impl<'a> Cur<'a> {
    fn left(&self) -> usize {
        self.d.len().saturating_sub(self.at)
    }
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.at.checked_add(N)?;
        let s = self.d.get(self.at..end)?;
        self.at = end;
        <[u8; N]>::try_from(s).ok()
    }
    fn u64(&mut self) -> Option<u64> {
        self.take::<8>().map(u64::from_le_bytes)
    }
    fn u16(&mut self) -> Option<u16> {
        self.take::<2>().map(u16::from_le_bytes)
    }
    fn i128(&mut self) -> Option<i128> {
        self.take::<16>().map(i128::from_le_bytes)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|b| b[0])
    }
    fn key(&mut self) -> Option<SolanaPubkey> {
        self.take::<32>()
    }
    fn bool(&mut self) -> Result<Option<bool>, AccountDecodeError> {
        match self.u8() {
            None => Ok(None),
            Some(0) => Ok(Some(false)),
            Some(1) => Ok(Some(true)),
            Some(_) => Err(AccountDecodeError::InvalidBool),
        }
    }
}

fn gate(
    owner: &SolanaPubkey,
    expected_owner: &SolanaPubkey,
    data: &[u8],
    disc: &[u8; 8],
    need: usize,
) -> Result<(), AccountDecodeError> {
    if owner != expected_owner {
        return Err(AccountDecodeError::WrongOwner);
    }
    if data.len() < need {
        return Err(AccountDecodeError::TooShort {
            got: data.len(),
            need,
        });
    }
    if data.get(..8) != Some(disc.as_slice()) {
        return Err(AccountDecodeError::WrongDiscriminator);
    }
    Ok(())
}

/// Decoded pump.fun `BondingCurve` account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BondingCurveAccount {
    pub virtual_token_reserves: u64,
    /// IDL name `virtual_quote_reserves` (SOL lamports for a SOL curve).
    pub virtual_quote_reserves: u64,
    pub real_token_reserves: u64,
    pub real_quote_reserves: u64,
    pub token_total_supply: u64,
    /// The curve graduated (migrated to PumpSwap): no further curve trades.
    pub complete: bool,
    pub creator: SolanaPubkey,
    pub is_mayhem_mode: Option<bool>,
    pub is_cashback_coin: Option<bool>,
    /// Absent on legacy accounts (SOL quote); `[0;32]` also means native SOL.
    pub quote_mint: Option<SolanaPubkey>,
    pub creator_fee_bps: Option<u64>,
    pub can_edit_creator_fee: Option<bool>,
    pub is_holder_reward: Option<bool>,
    pub trailing_bytes: usize,
}

/// Decode a `BondingCurve` (owner must be the pump.fun program).
pub fn decode_bonding_curve(
    owner: &SolanaPubkey,
    data: &[u8],
) -> Result<BondingCurveAccount, AccountDecodeError> {
    gate(
        owner,
        &PUMP_PROGRAM_ID_BYTES,
        data,
        &BONDING_CURVE_ACCOUNT_DISCRIMINATOR,
        BONDING_CURVE_REQUIRED_LEN,
    )?;
    let mut c = Cur { d: data, at: 8 };
    let short = || AccountDecodeError::TooShort {
        got: data.len(),
        need: BONDING_CURVE_REQUIRED_LEN,
    };
    let virtual_token_reserves = c.u64().ok_or_else(short)?;
    let virtual_quote_reserves = c.u64().ok_or_else(short)?;
    let real_token_reserves = c.u64().ok_or_else(short)?;
    let real_quote_reserves = c.u64().ok_or_else(short)?;
    let token_total_supply = c.u64().ok_or_else(short)?;
    let complete = c.bool()?.ok_or_else(short)?;
    let creator = c.key().ok_or_else(short)?;
    let is_mayhem_mode = c.bool()?;
    let is_cashback_coin = c.bool()?;
    let quote_mint = c.key();
    let creator_fee_bps = c.u64();
    let can_edit_creator_fee = c.bool()?;
    let is_holder_reward = c.bool()?;
    Ok(BondingCurveAccount {
        virtual_token_reserves,
        virtual_quote_reserves,
        real_token_reserves,
        real_quote_reserves,
        token_total_supply,
        complete,
        creator,
        is_mayhem_mode,
        is_cashback_coin,
        quote_mint,
        creator_fee_bps,
        can_edit_creator_fee,
        is_holder_reward,
        trailing_bytes: c.left(),
    })
}

/// Decoded PumpSwap `Pool` account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolAccount {
    pub pool_bump: u8,
    pub index: u16,
    pub creator: SolanaPubkey,
    pub base_mint: SolanaPubkey,
    pub quote_mint: SolanaPubkey,
    pub lp_mint: SolanaPubkey,
    pub pool_base_token_account: SolanaPubkey,
    pub pool_quote_token_account: SolanaPubkey,
    pub lp_supply: u64,
    pub coin_creator: SolanaPubkey,
    pub is_mayhem_mode: Option<bool>,
    pub is_cashback_coin: Option<bool>,
    /// `0` for legacy accounts without the field (see module docs).
    pub virtual_quote_reserves: i128,
    pub virtual_quote_reserves_present: bool,
    pub creator_fee_bps: Option<u64>,
    pub can_edit_creator_fee: Option<bool>,
    pub is_holder_reward: Option<bool>,
    pub trailing_bytes: usize,
}

/// Decode a `Pool` (owner must be the PumpSwap program).
pub fn decode_pool(owner: &SolanaPubkey, data: &[u8]) -> Result<PoolAccount, AccountDecodeError> {
    gate(
        owner,
        &PUMP_AMM_PROGRAM_ID_BYTES,
        data,
        &POOL_ACCOUNT_DISCRIMINATOR,
        POOL_REQUIRED_LEN,
    )?;
    let mut c = Cur { d: data, at: 8 };
    let short = || AccountDecodeError::TooShort {
        got: data.len(),
        need: POOL_REQUIRED_LEN,
    };
    let pool_bump = c.u8().ok_or_else(short)?;
    let index = c.u16().ok_or_else(short)?;
    let creator = c.key().ok_or_else(short)?;
    let base_mint = c.key().ok_or_else(short)?;
    let quote_mint = c.key().ok_or_else(short)?;
    let lp_mint = c.key().ok_or_else(short)?;
    let pool_base_token_account = c.key().ok_or_else(short)?;
    let pool_quote_token_account = c.key().ok_or_else(short)?;
    let lp_supply = c.u64().ok_or_else(short)?;
    let coin_creator = c.key().ok_or_else(short)?;
    let is_mayhem_mode = c.bool()?;
    let is_cashback_coin = c.bool()?;
    let vq = c.i128();
    let creator_fee_bps = c.u64();
    let can_edit_creator_fee = c.bool()?;
    let is_holder_reward = c.bool()?;
    Ok(PoolAccount {
        pool_bump,
        index,
        creator,
        base_mint,
        quote_mint,
        lp_mint,
        pool_base_token_account,
        pool_quote_token_account,
        lp_supply,
        coin_creator,
        is_mayhem_mode,
        is_cashback_coin,
        virtual_quote_reserves: vq.unwrap_or(0),
        virtual_quote_reserves_present: vq.is_some(),
        creator_fee_bps,
        can_edit_creator_fee,
        is_holder_reward,
        trailing_bytes: c.left(),
    })
}

/// Balance of an SPL / Token-2022 token account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenAccountBalance {
    pub mint: SolanaPubkey,
    pub owner: SolanaPubkey,
    pub amount: u64,
    /// SPL `AccountState` byte: 1 = initialized, 2 = frozen.
    pub state: u8,
}

/// Decode the 165-byte base layout (Token-2022 accounts carry extensions
/// after byte 165; the base fields are identical). The owning program must
/// be SPL Token or Token-2022.
pub fn decode_token_account(
    owner_program: &SolanaPubkey,
    data: &[u8],
) -> Result<TokenAccountBalance, AccountDecodeError> {
    if *owner_program != SPL_TOKEN_PROGRAM_ID_BYTES
        && *owner_program != SPL_TOKEN_2022_PROGRAM_ID_BYTES
    {
        return Err(AccountDecodeError::WrongOwner);
    }
    if data.len() < TOKEN_ACCOUNT_BASE_LEN {
        return Err(AccountDecodeError::TooShort {
            got: data.len(),
            need: TOKEN_ACCOUNT_BASE_LEN,
        });
    }
    let mut c = Cur { d: data, at: 0 };
    let short = || AccountDecodeError::TooShort {
        got: data.len(),
        need: TOKEN_ACCOUNT_BASE_LEN,
    };
    let mint = c.key().ok_or_else(short)?;
    let owner = c.key().ok_or_else(short)?;
    let amount = c.u64().ok_or_else(short)?;
    // delegate COption (36), state (1) at offset 108.
    let state = *data.get(108).ok_or_else(short)?;
    if state != 1 && state != 2 {
        return Err(AccountDecodeError::TokenAccountUninitialized);
    }
    Ok(TokenAccountBalance {
        mint,
        owner,
        amount,
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: u8) -> [u8; 32] {
        [t; 32]
    }

    fn curve_bytes(complete: u8, extended: bool) -> Vec<u8> {
        let mut d = BONDING_CURVE_ACCOUNT_DISCRIMINATOR.to_vec();
        for v in [11u64, 22, 33, 44, 55] {
            d.extend(v.to_le_bytes());
        }
        d.push(complete);
        d.extend(key(7));
        if extended {
            d.extend([1, 0]);
            d.extend(key(0));
            d.extend(30u64.to_le_bytes());
            d.extend([1, 0]);
        }
        d
    }

    #[test]
    fn curve_legacy_and_extended_layouts_decode() {
        let legacy = decode_bonding_curve(&PUMP_PROGRAM_ID_BYTES, &curve_bytes(0, false)).unwrap();
        assert_eq!(legacy.virtual_token_reserves, 11);
        assert_eq!(legacy.virtual_quote_reserves, 22);
        assert_eq!(legacy.real_token_reserves, 33);
        assert_eq!(legacy.real_quote_reserves, 44);
        assert_eq!(legacy.token_total_supply, 55);
        assert!(!legacy.complete);
        assert_eq!(legacy.creator, key(7));
        assert_eq!(legacy.is_mayhem_mode, None);
        assert_eq!(legacy.quote_mint, None);
        assert_eq!(legacy.trailing_bytes, 0);
        let full = decode_bonding_curve(&PUMP_PROGRAM_ID_BYTES, &curve_bytes(1, true)).unwrap();
        assert!(full.complete);
        assert_eq!(full.is_mayhem_mode, Some(true));
        assert_eq!(full.is_cashback_coin, Some(false));
        assert_eq!(full.quote_mint, Some([0; 32]));
        assert_eq!(full.creator_fee_bps, Some(30));
        assert_eq!(full.can_edit_creator_fee, Some(true));
        assert_eq!(full.is_holder_reward, Some(false));
        // Appended unknown bytes are counted, not interpreted.
        let mut longer = curve_bytes(0, true);
        longer.extend([9, 9, 9]);
        let l = decode_bonding_curve(&PUMP_PROGRAM_ID_BYTES, &longer).unwrap();
        assert_eq!(l.trailing_bytes, 3);
    }

    #[test]
    fn curve_gates_owner_discriminator_length_and_bool() {
        let ok = curve_bytes(0, false);
        assert_eq!(
            decode_bonding_curve(&key(1), &ok),
            Err(AccountDecodeError::WrongOwner)
        );
        let mut bad = ok.clone();
        bad[0] ^= 1;
        assert_eq!(
            decode_bonding_curve(&PUMP_PROGRAM_ID_BYTES, &bad),
            Err(AccountDecodeError::WrongDiscriminator)
        );
        assert!(matches!(
            decode_bonding_curve(&PUMP_PROGRAM_ID_BYTES, &ok[..ok.len() - 1]),
            Err(AccountDecodeError::TooShort { .. })
        ));
        let mut b = ok;
        b[8 + 40] = 2;
        assert_eq!(
            decode_bonding_curve(&PUMP_PROGRAM_ID_BYTES, &b),
            Err(AccountDecodeError::InvalidBool)
        );
    }

    fn pool_bytes(extended: bool) -> Vec<u8> {
        let mut d = POOL_ACCOUNT_DISCRIMINATOR.to_vec();
        d.push(254);
        d.extend(3u16.to_le_bytes());
        for t in 1..=6u8 {
            d.extend(key(t));
        }
        d.extend(999u64.to_le_bytes());
        d.extend(key(8));
        if extended {
            d.extend([0, 1]);
            d.extend((-5i128).to_le_bytes());
            d.extend(95u64.to_le_bytes());
            d.extend([0, 1]);
        }
        d
    }

    #[test]
    fn pool_legacy_and_extended_layouts_decode() {
        let legacy = decode_pool(&PUMP_AMM_PROGRAM_ID_BYTES, &pool_bytes(false)).unwrap();
        assert_eq!(legacy.pool_bump, 254);
        assert_eq!(legacy.index, 3);
        assert_eq!(legacy.creator, key(1));
        assert_eq!(legacy.base_mint, key(2));
        assert_eq!(legacy.quote_mint, key(3));
        assert_eq!(legacy.lp_mint, key(4));
        assert_eq!(legacy.pool_base_token_account, key(5));
        assert_eq!(legacy.pool_quote_token_account, key(6));
        assert_eq!(legacy.lp_supply, 999);
        assert_eq!(legacy.coin_creator, key(8));
        assert_eq!(legacy.virtual_quote_reserves, 0);
        assert!(!legacy.virtual_quote_reserves_present);
        assert_eq!(legacy.is_mayhem_mode, None);
        let full = decode_pool(&PUMP_AMM_PROGRAM_ID_BYTES, &pool_bytes(true)).unwrap();
        assert_eq!(full.virtual_quote_reserves, -5);
        assert!(full.virtual_quote_reserves_present);
        assert_eq!(full.is_cashback_coin, Some(true));
        assert_eq!(full.creator_fee_bps, Some(95));
        assert_eq!(full.is_holder_reward, Some(true));
        assert_eq!(full.trailing_bytes, 0);
    }

    #[test]
    fn pool_gates_owner_and_discriminator() {
        let d = pool_bytes(true);
        assert_eq!(
            decode_pool(&PUMP_PROGRAM_ID_BYTES, &d),
            Err(AccountDecodeError::WrongOwner)
        );
        let mut bad = d.clone();
        bad[3] ^= 0xff;
        assert_eq!(
            decode_pool(&PUMP_AMM_PROGRAM_ID_BYTES, &bad),
            Err(AccountDecodeError::WrongDiscriminator)
        );
        assert!(matches!(
            decode_pool(&PUMP_AMM_PROGRAM_ID_BYTES, &d[..100]),
            Err(AccountDecodeError::TooShort { .. })
        ));
    }

    fn token_bytes(amount: u64, state: u8, extra: usize) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(key(1));
        d.extend(key(2));
        d.extend(amount.to_le_bytes());
        d.extend([0u8; 36]);
        d.push(state);
        d.resize(TOKEN_ACCOUNT_BASE_LEN + extra, 0);
        d
    }

    #[test]
    fn token_account_base_layout_for_both_token_programs() {
        for program in [SPL_TOKEN_PROGRAM_ID_BYTES, SPL_TOKEN_2022_PROGRAM_ID_BYTES] {
            let b = decode_token_account(&program, &token_bytes(123_456_789, 1, 0)).unwrap();
            assert_eq!(b.mint, key(1));
            assert_eq!(b.owner, key(2));
            assert_eq!(b.amount, 123_456_789);
        }
        // Token-2022 extension bytes after the base layout are ignored.
        let ext =
            decode_token_account(&SPL_TOKEN_2022_PROGRAM_ID_BYTES, &token_bytes(5, 2, 20)).unwrap();
        assert_eq!((ext.amount, ext.state), (5, 2));
        assert_eq!(
            decode_token_account(&key(9), &token_bytes(1, 1, 0)),
            Err(AccountDecodeError::WrongOwner)
        );
        assert_eq!(
            decode_token_account(&SPL_TOKEN_PROGRAM_ID_BYTES, &token_bytes(1, 0, 0)),
            Err(AccountDecodeError::TokenAccountUninitialized)
        );
        assert!(matches!(
            decode_token_account(&SPL_TOKEN_PROGRAM_ID_BYTES, &[0u8; 164]),
            Err(AccountDecodeError::TooShort { .. })
        ));
    }

    #[test]
    fn constants_match_base58_ids_and_idl_discriminators() {
        let dec =
            |s: &str| -> SolanaPubkey { bs58::decode(s).into_vec().unwrap().try_into().unwrap() };
        assert_eq!(
            PUMP_PROGRAM_ID_BYTES,
            dec("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
        );
        assert_eq!(
            SPL_TOKEN_PROGRAM_ID_BYTES,
            dec("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
        );
        assert_eq!(
            SPL_TOKEN_2022_PROGRAM_ID_BYTES,
            dec("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
        );
        let fx = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/p0/measurements/fixtures");
        for (file, name, want) in [
            (
                "pump_idl_e0687ae9.json",
                "BondingCurve",
                BONDING_CURVE_ACCOUNT_DISCRIMINATOR,
            ),
            (
                "pump_amm_idl_e0687ae9.json",
                "Pool",
                POOL_ACCOUNT_DISCRIMINATOR,
            ),
        ] {
            let idl: serde_json::Value =
                serde_json::from_slice(&std::fs::read(fx.join(file)).unwrap()).unwrap();
            let acc = idl["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["name"] == name)
                .unwrap();
            let got: Vec<u8> = acc["discriminator"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| u8::try_from(v.as_u64().unwrap()).unwrap())
                .collect();
            assert_eq!(got, want, "{name}");
            // Field count of the layout implemented above.
            let ty = idl["types"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == name)
                .unwrap();
            let fields = ty["type"]["fields"].as_array().unwrap().len();
            assert_eq!(
                fields,
                if name == "Pool" { 16 } else { 13 },
                "{name} field count"
            );
        }
    }
}
