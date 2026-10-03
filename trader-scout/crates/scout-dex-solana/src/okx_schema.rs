//! Borsh schema tables of the OKX DEX Router program
//! (`proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u`), transcribed from the
//! pinned on-chain Anchor IDL
//! `docs/p0/measurements/fixtures/okx_dex_router_onchain_idl_2026-10-03.json`
//! (sha256 `c1f85197...54c9`). The tables are **checked field by field
//! against that file** by the IDL-equality tests of `okx_event`; nothing here
//! is guessed.
//!
//! Only what the event decoder needs is present: the `Dex` enum (the first
//! field of the per-hop `SwapEvent`, 143 variants of which a dozen carry
//! data) with the types reachable from it, and the fee tails of the six
//! order events. A tail/variant is *validated* (exact structure, bounded
//! depth, no read past the buffer) and skipped; fee values are not retained.

/// A Borsh type of the IDL subset used by this program's events.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ty {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I64,
    U128,
    Pubkey,
    /// `bytes`: `u32` length + payload.
    Bytes,
    Array(&'static Ty, usize),
    Vec(&'static Ty),
    Option(&'static Ty),
    Defined(&'static Def),
}

/// A named struct or enum of the IDL `types` list.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Def {
    Struct {
        name: &'static str,
        fields: &'static [(&'static str, Ty)],
    },
    Enum {
        name: &'static str,
        /// `(variant name, field types)`; the Borsh tag is the index (`u8`).
        variants: &'static [(&'static str, &'static [Ty])],
    },
}

/// Maximum nesting depth accepted by [`skip_ty`].
const MAX_DEPTH: u8 = 16;

fn take(buf: &[u8], pos: &mut usize, n: usize) -> Option<()> {
    let end = pos.checked_add(n)?;
    if end > buf.len() {
        return None;
    }
    *pos = end;
    Some(())
}

fn read_u8(buf: &[u8], pos: &mut usize) -> Option<u8> {
    let b = *buf.get(*pos)?;
    *pos += 1;
    Some(b)
}

fn read_u32(buf: &[u8], pos: &mut usize) -> Option<u32> {
    let a: [u8; 4] = buf.get(*pos..pos.checked_add(4)?)?.try_into().ok()?;
    *pos += 4;
    Some(u32::from_le_bytes(a))
}

/// Validate one value of type `ty` at `*pos` and advance past it. `None` on
/// truncation, an invalid enum tag / option flag / bool, an element count
/// larger than the bytes left, or nesting deeper than 16.
pub(crate) fn skip_ty(buf: &[u8], pos: &mut usize, ty: &Ty, depth: u8) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    match ty {
        Ty::Bool => (read_u8(buf, pos)? <= 1).then_some(()),
        Ty::U8 => take(buf, pos, 1),
        Ty::U16 => take(buf, pos, 2),
        Ty::U32 => take(buf, pos, 4),
        Ty::U64 | Ty::I64 => take(buf, pos, 8),
        Ty::U128 => take(buf, pos, 16),
        Ty::Pubkey => take(buf, pos, 32),
        Ty::Bytes => {
            let n = usize::try_from(read_u32(buf, pos)?).ok()?;
            take(buf, pos, n)
        }
        Ty::Array(inner, n) => {
            for _ in 0..*n {
                skip_ty(buf, pos, inner, depth + 1)?;
            }
            Some(())
        }
        Ty::Vec(inner) => {
            let n = usize::try_from(read_u32(buf, pos)?).ok()?;
            // Every element is at least one byte: bounds the loop by the buffer.
            if n > buf.len().saturating_sub(*pos) {
                return None;
            }
            for _ in 0..n {
                skip_ty(buf, pos, inner, depth + 1)?;
            }
            Some(())
        }
        Ty::Option(inner) => match read_u8(buf, pos)? {
            0 => Some(()),
            1 => skip_ty(buf, pos, inner, depth + 1),
            _ => None,
        },
        Ty::Defined(def) => match def {
            Def::Struct { fields, .. } => {
                for (_, t) in *fields {
                    skip_ty(buf, pos, t, depth + 1)?;
                }
                Some(())
            }
            Def::Enum { variants, .. } => {
                let tag = usize::from(read_u8(buf, pos)?);
                let (_, fields) = variants.get(tag)?;
                for t in *fields {
                    skip_ty(buf, pos, t, depth + 1)?;
                }
                Some(())
            }
        },
    }
}

/// Validate a sequence of named fields (an event tail).
pub(crate) fn skip_fields(buf: &[u8], pos: &mut usize, fields: &[(&str, Ty)]) -> Option<()> {
    for (_, t) in fields {
        skip_ty(buf, pos, t, 0)?;
    }
    Some(())
}

/// Decode a `Dex` value: `(variant index, variant name)`; its data (if any)
/// is validated and skipped.
pub(crate) fn read_dex(buf: &[u8], pos: &mut usize) -> Option<(u8, &'static str)> {
    let Def::Enum { variants, .. } = &DEX else {
        return None;
    };
    let tag = *buf.get(*pos)?;
    let (name, _) = variants.get(usize::from(tag))?;
    skip_ty(buf, pos, &Ty::Defined(&DEX), 0)?;
    Some((tag, name))
}

/// The `Dex` enum of the IDL (for the IDL-equality tests).
#[cfg(test)]
pub(crate) fn dex_def() -> &'static Def {
    &DEX
}

// ---- transcribed from the pinned IDL (checked by `okx_event` tests) ----

static DEX: Def = Def::Enum {
    name: "Dex",
    variants: &[
        ("SplTokenSwap", &[]),
        ("StableSwap", &[]),
        ("Whirlpool", &[]),
        ("MeteoraDynamicpool", &[]),
        ("GoonfiV2WithSig", &[]),
        ("RaydiumStableSwap", &[]),
        ("RaydiumClmmSwap", &[]),
        ("AldrinExchangeV1", &[]),
        ("AldrinExchangeV2", &[]),
        ("LifinityV1", &[]),
        ("LifinityV2", &[]),
        ("RaydiumClmmSwapV2", &[]),
        ("FluxBeam", &[]),
        ("MeteoraDlmm", &[]),
        ("RaydiumCpmmSwap", &[]),
        ("OpenBookV2", &[]),
        ("WhirlpoolV2", &[]),
        ("Phoenix", &[]),
        ("ObricV2", &[]),
        ("SanctumAddLiq", &[]),
        ("SanctumRemoveLiq", &[]),
        ("SanctumNonWsolSwap", &[]),
        ("SanctumWsolSwap", &[]),
        ("PumpfunBuy", &[Ty::Bool]),
        ("PumpfunSell", &[Ty::Bool]),
        ("StabbleSwap", &[]),
        ("SanctumRouter", &[]),
        ("MeteoraVaultDeposit", &[]),
        ("MeteoraVaultWithdraw", &[]),
        ("Saros", &[]),
        ("MeteoraLst", &[]),
        ("Solfi", &[]),
        ("QualiaSwap", &[]),
        ("Zerofi", &[]),
        ("PumpfunammBuy", &[Ty::Bool]),
        ("PumpfunammSell", &[Ty::Bool]),
        ("Virtuals", &[]),
        ("VertigoBuy", &[]),
        ("VertigoSell", &[]),
        ("PerpetualsAddLiq", &[]),
        ("PerpetualsRemoveLiq", &[]),
        ("PerpetualsSwap", &[]),
        ("RaydiumLaunchpad", &[]),
        ("LetsBonkFun", &[]),
        ("Woofi", &[]),
        ("MeteoraDbc", &[]),
        ("MeteoraDlmmSwap2", &[]),
        ("MeteoraDAMMV2", &[]),
        ("Gavel", &[]),
        ("BoopfunBuy", &[]),
        ("BoopfunSell", &[]),
        ("MeteoraDbc2", &[]),
        ("GooseFX", &[]),
        ("Dooar", &[]),
        ("Numeraire", &[]),
        ("SaberDecimalWrapperDeposit", &[]),
        ("SaberDecimalWrapperWithdraw", &[]),
        ("SarosDlmm", &[]),
        ("OneDexSwap", &[]),
        ("Manifest", &[]),
        ("ByrealClmm", &[]),
        ("PancakeSwapV3Swap", &[]),
        ("PancakeSwapV3SwapV2", &[]),
        ("Tessera", &[]),
        (
            "SolRfq",
            &[
                Ty::U64,
                Ty::U64,
                Ty::U64,
                Ty::U64,
                Ty::U64,
                Ty::U64,
                Ty::Bool,
                Ty::Bool,
            ],
        ),
        ("Humidifi", &[]),
        ("HeavenBuy", &[]),
        ("HeavenSell", &[]),
        ("SolfiV2", &[]),
        ("Goonfi", &[]),
        ("MoonitBuy", &[]),
        ("MoonitSell", &[]),
        ("RaydiumSwapV2", &[]),
        ("Whalestreet", &[]),
        ("SugarMoneyBuy", &[Ty::U8, Ty::U8]),
        ("SugarMoneySell", &[Ty::U8, Ty::U8]),
        ("MeteoraDAMMV2Swap2", &[]),
        ("AlphaQ", &[]),
        ("FutarchyAmm", &[]),
        ("PumpfunBuyV2", &[]),
        ("PumpfunSellV2", &[]),
        ("HumidifiSwap2", &[Ty::U64]),
        ("Scorch", &[Ty::U128]),
        ("JupiterLendDeposit", &[]),
        ("JupiterLendRedeem", &[]),
        ("TaurusFi", &[]),
        ("BisonFi", &[]),
        ("GoonfiV2", &[]),
        ("Quantum", &[]),
        ("BoopfunBuy2", &[]),
        ("BoopfunSell2", &[]),
        ("ByrealClmm2", &[]),
        ("Dooar2", &[]),
        ("HeavenBuy2", &[]),
        ("HeavenSell2", &[]),
        ("MoonitBuy2", &[]),
        ("MoonitSell2", &[]),
        ("SaberDecimalWrapperDeposit2", &[]),
        ("SaberDecimalWrapperWithdraw2", &[]),
        ("ByrealPropAmm", &[]),
        ("SanctumPrefundSwapViaStake", &[Ty::U64]),
        ("AbyssAmm", &[]),
        ("Aquifer", &[]),
        ("WhalestreetV2", &[Ty::U64, Ty::U64]),
        (
            "SolfiV2WithSig",
            &[
                Ty::U64,
                Ty::U64,
                Ty::U64,
                Ty::U16,
                Ty::U64,
                Ty::Array(&Ty::U8, 64),
                Ty::U8,
            ],
        ),
        ("FusionAmm", &[]),
        ("ScaleAmmBuy", &[]),
        ("ScaleAmmSell", &[]),
        ("ScaleVmmBuy", &[]),
        ("ScaleVmmSell", &[]),
        ("PumpfunBuy3", &[]),
        ("PumpfunSell3", &[]),
        ("PumpfunammBuy2", &[]),
        ("PumpfunammSell2", &[]),
        ("Riptide", &[]),
        ("ZerofiSwapV2", &[]),
        ("TaurusFiV2", &[]),
        (
            "SolRfqV2",
            &[
                Ty::Defined(&RFQ_SIDE),
                Ty::U64,
                Ty::I64,
                Ty::Vec(&Ty::Defined(&RFQ_LEVEL)),
            ],
        ),
        ("FluxMM", &[Ty::U64]),
        (
            "MeteoraDlmmSwap2Hook",
            &[Ty::Defined(&REMAINING_ACCOUNTS_INFO), Ty::U8],
        ),
        (
            "BisonFiWithSig",
            &[
                Ty::U64,
                Ty::U64,
                Ty::U64,
                Ty::U16,
                Ty::U64,
                Ty::Array(&Ty::U8, 64),
                Ty::U8,
            ],
        ),
        ("MeteoraDbcSwapWithHook", &[Ty::U8]),
        ("ZerofiSwapV4", &[]),
        ("Moonpay", &[]),
        ("BinaryFi", &[]),
        ("Archer", &[Ty::U8]),
        ("DynamicRouteV1", &[Ty::Defined(&DYNAMIC_ROUTE_SPEC)]),
        ("Flint", &[]),
        ("RaydiumStableSwapV2", &[]),
        ("Kipseli", &[]),
        ("Hadron", &[]),
        ("DenaliPropAmm", &[]),
        ("ZerofiWithSig", &[Ty::Array(&Ty::U8, 40)]),
        ("Native", &[Ty::Bytes]),
        (
            "HumidifiSwapRouter",
            &[
                Ty::U64,
                Ty::U64,
                Ty::Array(&Ty::U8, 32),
                Ty::Array(&Ty::U8, 16),
                Ty::Pubkey,
            ],
        ),
        ("Deriverse", &[]),
        ("MSwap", &[]),
        ("ByrealClmm3", &[]),
        ("JupiterLendAmm", &[Ty::U8]),
        ("Flowdesk", &[]),
        ("TesseraV2", &[]),
        ("GatorSwap", &[Ty::Bool, Ty::U8]),
        ("NativePamm", &[]),
    ],
};
static DYNAMIC_ROUTE_SPEC: Def = Def::Struct {
    name: "DynamicRouteSpec",
    fields: &[
        ("candidates", Ty::Vec(&Ty::Defined(&DYNAMIC_CANDIDATE))),
        ("mode", Ty::Defined(&SELECTION_MODE)),
    ],
};
static REMAINING_ACCOUNTS_INFO: Def = Def::Struct {
    name: "RemainingAccountsInfo",
    fields: &[("slices", Ty::Vec(&Ty::Defined(&REMAINING_ACCOUNTS_SLICE)))],
};
static RFQ_LEVEL: Def = Def::Struct {
    name: "RfqLevel",
    fields: &[("base_atoms", Ty::U64), ("quote_atoms", Ty::U64)],
};
static RFQ_SIDE: Def = Def::Enum {
    name: "RfqSide",
    variants: &[("Bid", &[]), ("Ask", &[])],
};
static DYNAMIC_CANDIDATE: Def = Def::Enum {
    name: "DynamicCandidate",
    variants: &[
        ("Manifest", &[]),
        ("Tessera", &[]),
        ("GoonfiV2", &[]),
        ("HumidifiSwap2", &[Ty::U64]),
        ("SolfiV2", &[]),
        ("Scorch", &[Ty::U128]),
        ("BisonFi", &[]),
        ("RaydiumSwapV2", &[]),
        ("RaydiumClmmSwapV2", &[]),
        ("WhirlpoolV2", &[]),
        (
            "BisonFiWithSig",
            &[
                Ty::U64,
                Ty::U64,
                Ty::U64,
                Ty::U16,
                Ty::U64,
                Ty::Array(&Ty::U8, 64),
                Ty::U8,
            ],
        ),
        ("Quantum", &[]),
        ("Zerofi", &[]),
        ("AlphaQ", &[]),
        ("FluxMM", &[Ty::U64]),
        (
            "HumidifiSwapRouter",
            &[
                Ty::U64,
                Ty::U64,
                Ty::Array(&Ty::U8, 32),
                Ty::Array(&Ty::U8, 16),
                Ty::Pubkey,
            ],
        ),
        ("ScorchV2", &[Ty::U128]),
        ("TesseraV2", &[]),
        ("ZerofiWithSig", &[Ty::Array(&Ty::U8, 40)]),
    ],
};
static REMAINING_ACCOUNTS_SLICE: Def = Def::Struct {
    name: "RemainingAccountsSlice",
    fields: &[
        ("accounts_type", Ty::Defined(&ACCOUNTS_TYPE)),
        ("length", Ty::U8),
    ],
};
static SELECTION_MODE: Def = Def::Enum {
    name: "SelectionMode",
    variants: &[("Auto", &[]), ("Forced", &[Ty::U8]), ("Hint", &[Ty::U8])],
};
static ACCOUNTS_TYPE: Def = Def::Enum {
    name: "AccountsType",
    variants: &[("TransferHookInput", &[]), ("TransferHookOutput", &[])],
};
static TRIM_FEE_INFO: Def = Def::Struct {
    name: "TrimFeeInfo",
    fields: &[
        ("trim_rate", Ty::U16),
        ("trim_amount", Ty::U64),
        ("trim_account", Ty::Pubkey),
        ("charge_rate", Ty::U16),
        ("charge_amount", Ty::U64),
        ("charge_account", Ty::Pubkey),
    ],
};

pub(crate) const SWAP_CPI_EVENT2_TAIL: &[(&str, Ty)] = &[];
pub(crate) const SWAP_TOB_V2_CPI_EVENT2_TAIL: &[(&str, Ty)] = &[
    ("commission_direction", Ty::Bool),
    ("total_commission_rate", Ty::U32),
    ("parent_commission_rate", Ty::U32),
    ("parent_commission_amount", Ty::U64),
    ("parent_commission_account", Ty::Pubkey),
    ("child_commission_rate", Ty::U32),
    ("child_commission_amount", Ty::U64),
    ("child_commission_account", Ty::Pubkey),
    ("platform_fee_rate", Ty::U16),
    ("platform_fee_amount", Ty::U64),
    ("platform_fee_account", Ty::Pubkey),
    ("trim_rate", Ty::U8),
    ("trim_amount", Ty::U64),
    ("trim_account", Ty::Pubkey),
];
pub(crate) const SWAP_TOC_V2_CPI_EVENT2_TAIL: &[(&str, Ty)] = &[
    ("commission_direction", Ty::Bool),
    ("total_commission_rate", Ty::U32),
    ("parent_commission_rate", Ty::U32),
    ("parent_commission_amount", Ty::U64),
    ("parent_commission_account", Ty::Pubkey),
    ("child_commission_rate", Ty::U32),
    ("child_commission_amount", Ty::U64),
    ("child_commission_account", Ty::Pubkey),
    ("platform_fee_rate", Ty::U16),
    ("platform_fee_amount", Ty::U64),
    ("platform_fee_account", Ty::Pubkey),
];
pub(crate) const SWAP_WITH_FEE_CPI_EVENT_V3_TAIL: &[(&str, Ty)] = &[
    ("commission_direction", Ty::Bool),
    ("commission_paid_in_sol", Ty::Bool),
    ("total_commission_rate", Ty::U32),
    ("total_commission_amount", Ty::U64),
    ("num_commission_levels", Ty::U8),
    ("commission_rates", Ty::Vec(&Ty::U32)),
    ("commission_amounts", Ty::Vec(&Ty::U64)),
    ("commission_accounts", Ty::Vec(&Ty::Pubkey)),
    ("trim_info", Ty::Option(&Ty::Defined(&TRIM_FEE_INFO))),
];
pub(crate) const SWAP_WITH_FEES_CPI_EVENT2_TAIL: &[(&str, Ty)] = &[
    ("commission_direction", Ty::Bool),
    ("commission_rate", Ty::U32),
    ("commission_amount", Ty::U64),
    ("commission_account", Ty::Pubkey),
    ("platform_fee_rate", Ty::U16),
    ("platform_fee_amount", Ty::U64),
    ("platform_fee_account", Ty::Pubkey),
    ("trim_rate", Ty::U8),
    ("trim_amount", Ty::U64),
    ("trim_account", Ty::Pubkey),
];
pub(crate) const SWAP_WITH_FEES_CPI_EVENT_ENHANCED2_TAIL: &[(&str, Ty)] = &[
    ("commission_direction", Ty::Bool),
    ("commission_rate", Ty::U32),
    ("commission_amount", Ty::U64),
    ("commission_account", Ty::Pubkey),
    ("platform_fee_rate", Ty::U16),
    ("platform_fee_amount", Ty::U64),
    ("platform_fee_account", Ty::Pubkey),
    ("trim_rate", Ty::U8),
    ("trim_amount", Ty::U64),
    ("trim_account", Ty::Pubkey),
    ("charge_rate", Ty::U16),
    ("charge_amount", Ty::U64),
    ("charge_account", Ty::Pubkey),
];
